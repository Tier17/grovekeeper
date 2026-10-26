#![windows_subsystem = "windows"]

mod autostart;
mod config;
mod ddc;
mod icon;
mod ui;
mod util;
mod worker;

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::mpsc::Sender;

use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, ScreenToClient,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, ReleaseCapture, SetCapture, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_DOWN,
    VK_END, VK_ESCAPE, VK_UP,
};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use config::Config;
use ddc::{FEATURES, INPUTS, POWER_OFF, POWER_ON, VCP_INPUT, VCP_POWER};
use ui::{Fonts, Target};
use util::wide;
use worker::{from_percent, Cmd, Shared, Snapshot};

const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_REFRESHED: u32 = WM_APP + 2;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUNDSMALL: i32 = 3;

const TIMER_RESCAN: usize = 1;
const TIMER_SCREENS_OFF: usize = 2;

const HK_UP: i32 = 1;
const HK_DOWN: i32 = 2;
const HK_SCREENS_OFF: i32 = 3;

const ID_SCREENS_OFF: usize = 10;
const ID_REFRESH: usize = 11;
const ID_AUTOSTART: usize = 12;
const ID_SETTINGS: usize = 13;
const ID_EXIT: usize = 14;
const ID_PANEL: usize = 15;
const ID_PRESET: usize = 100;
const ID_MONITOR: usize = 1000;
const MON_POWER_ON: usize = 50;
const MON_POWER_OFF: usize = 51;

/// Ignore a tray click that lands right after the panel hid itself, so clicking the icon
/// while the panel is open closes it instead of reopening it.
const REOPEN_GUARD_MS: u64 = 400;

struct App {
    tx: Sender<Cmd>,
    shared: Shared,
    view: Snapshot,
    config: Config,
    scale: f32,
    fonts: Option<Fonts>,
    drag: Option<Target>,
    anchor: POINT,
    last_hide: u64,
    icon: HICON,
    taskbar_created: u32,
}

impl App {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// Updates the panel immediately and queues the DDC write.
    fn set_pct(&mut self, target: Target, pct: u32) {
        match target {
            Target::All => {
                let mut changed = false;
                for m in &mut self.view.monitors {
                    if let Some(fv) = m.features[0].as_mut() {
                        let value = from_percent(pct, fv.max);
                        changed |= value != fv.cur;
                        fv.cur = value;
                    }
                }
                if changed {
                    self.send(Cmd::SetAllPercent { feature: 0, pct });
                }
            }
            Target::Mon(mon, feature) => {
                let Some(fv) = self.view.monitors.get_mut(mon).and_then(|m| m.features[feature].as_mut()) else {
                    return;
                };
                let value = from_percent(pct, fv.max);
                if value != fv.cur {
                    fv.cur = value;
                    self.send(Cmd::Set { mon, feature, value });
                }
            }
        }
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Win32 calls can re-enter the window procedure, so never hold this borrow across one that
/// sends messages; a re-entrant call just gets `None`.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

fn main() {
    unsafe {
        let mutex_name = wide("Local\\Grovekeeper.SingleInstance");
        let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance = GetModuleHandleW(null());
        let class_name = wide("GrovekeeperPanel");
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.style = CS_DROPSHADOW;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassExW(&wc);

        let title = wide("Grovekeeper");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &DWMWCP_ROUNDSMALL as *const i32 as _,
            size_of::<i32>() as u32,
        );

        let icon = icon::leaf(GetSystemMetrics(SM_CXSMICON).max(16));
        let shared = Shared::default();
        let tx = worker::spawn(shared.clone(), hwnd as isize, WM_APP_REFRESHED);
        let _ = tx.send(Cmd::Rescan);

        let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, null_mut());

        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                tx,
                shared,
                view: Snapshot::default(),
                config: Config::load(),
                scale: 1.0,
                fonts: None,
                drag: None,
                anchor: POINT { x: 0, y: 0 },
                last_hide: 0,
                icon,
                taskbar_created,
            })
        });

        add_tray(hwnd, icon);
        RegisterHotKey(hwnd, HK_UP, MOD_CONTROL | MOD_ALT, VK_UP as u32);
        RegisterHotKey(hwnd, HK_DOWN, MOD_CONTROL | MOD_ALT, VK_DOWN as u32);
        RegisterHotKey(hwnd, HK_SCREENS_OFF, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_END as u32);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn add_tray(hwnd: HWND, icon: HICON) {
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    nid.hIcon = icon;
    let tip = wide("Grovekeeper");
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn remove_tray(hwnd: HWND) {
    let nid = tray_data(hwnd);
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            match (lparam as u32) & 0xFFFF {
                WM_LBUTTONUP => toggle_panel(hwnd),
                WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_APP_REFRESHED => {
            let updated = with_app(|a| {
                if a.drag.is_some() {
                    return false;
                }
                a.view = a.shared.lock().unwrap().clone();
                true
            })
            .unwrap_or(false);
            if updated && IsWindowVisible(hwnd) != 0 {
                place_panel(hwnd, false);
            }
            0
        }
        WM_HOTKEY => {
            match wparam as i32 {
                HK_UP | HK_DOWN => {
                    with_app(|a| {
                        let step = a.config.step as i32;
                        let delta = if wparam as i32 == HK_UP { step } else { -step };
                        a.send(Cmd::AdjustAllPercent { feature: 0, delta });
                    });
                }
                HK_SCREENS_OFF => {
                    SetTimer(hwnd, TIMER_SCREENS_OFF, 500, None);
                }
                _ => {}
            }
            0
        }
        WM_ACTIVATE => {
            if wparam & 0xFFFF == WA_INACTIVE as usize {
                hide_panel(hwnd);
            }
            0
        }
        WM_KEYDOWN => {
            if wparam == VK_ESCAPE as usize {
                hide_panel(hwnd);
            }
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut client: RECT = zeroed();
            GetClientRect(hwnd, &mut client);
            with_app(|a| {
                if let Some(fonts) = &a.fonts {
                    let lay = ui::layout(&a.view, a.scale);
                    ui::paint(hdc, &client, &lay, fonts, a.drag);
                }
            });
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let hit = with_app(|a| {
                let hit = ui::layout(&a.view, a.scale).hit(x, y);
                a.drag = hit;
                hit
            })
            .flatten();
            if hit.is_some() {
                SetCapture(hwnd);
                drag_to(hwnd, x);
            }
            0
        }
        WM_MOUSEMOVE => {
            if with_app(|a| a.drag.is_some()).unwrap_or(false) {
                drag_to(hwnd, lparam_xy(lparam).0);
            }
            0
        }
        WM_LBUTTONUP => {
            if with_app(|a| a.drag.take().is_some()).unwrap_or(false) {
                ReleaseCapture();
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_CAPTURECHANGED => {
            with_app(|a| a.drag = None);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            let notches = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
            let (x, y) = lparam_xy(lparam);
            let mut pt = POINT { x, y };
            ScreenToClient(hwnd, &mut pt);
            with_app(|a| {
                let lay = ui::layout(&a.view, a.scale);
                if let Some(row) = lay.hit(pt.x, pt.y).and_then(|t| lay.row(t)) {
                    let step = if notches > 0 { 5 } else { -5 };
                    let pct = (row.pct as i32 + step).clamp(0, 100) as u32;
                    let target = row.target;
                    a.set_pct(target, pct);
                }
            });
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DISPLAYCHANGE => {
            SetTimer(hwnd, TIMER_RESCAN, 1500, None);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_POWERBROADCAST => {
            if wparam == PBT_APMRESUMEAUTOMATIC {
                SetTimer(hwnd, TIMER_RESCAN, 3000, None);
            }
            1
        }
        WM_TIMER => {
            KillTimer(hwnd, wparam);
            match wparam {
                TIMER_RESCAN => {
                    with_app(|a| a.send(Cmd::Rescan));
                }
                TIMER_SCREENS_OFF => {
                    DefWindowProcW(hwnd, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            remove_tray(hwnd);
            with_app(|a| a.send(Cmd::Quit));
            PostQuitMessage(0);
            0
        }
        _ => {
            let taskbar_created = with_app(|a| (a.taskbar_created, a.icon));
            if let Some((id, icon)) = taskbar_created {
                if id != 0 && msg == id {
                    add_tray(hwnd, icon);
                    return 0;
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
    }
}

unsafe fn drag_to(hwnd: HWND, x: i32) {
    with_app(|a| {
        let Some(target) = a.drag else { return };
        let lay = ui::layout(&a.view, a.scale);
        let Some(row) = lay.row(target) else { return };
        let pct = lay.pct_at(row, x);
        a.set_pct(target, pct);
    });
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn toggle_panel(hwnd: HWND) {
    if IsWindowVisible(hwnd) != 0 {
        hide_panel(hwnd);
        return;
    }
    let just_hidden = with_app(|a| GetTickCount64().saturating_sub(a.last_hide) < REOPEN_GUARD_MS).unwrap_or(false);
    if just_hidden {
        return;
    }
    open_panel(hwnd);
}

unsafe fn open_panel(hwnd: HWND) {
    let mut cursor: POINT = zeroed();
    GetCursorPos(&mut cursor);
    with_app(|a| {
        a.anchor = cursor;
        a.config = Config::load();
        a.send(Cmd::Reread);
    });
    place_panel(hwnd, true);
}

unsafe fn hide_panel(hwnd: HWND) {
    if IsWindowVisible(hwnd) == 0 {
        return;
    }
    ShowWindow(hwnd, SW_HIDE);
    with_app(|a| {
        a.last_hide = GetTickCount64();
        a.drag = None;
    });
}

/// Sizes the panel to its content and keeps it next to where the tray icon was clicked,
/// inside the work area of that monitor.
unsafe fn place_panel(hwnd: HWND, show: bool) {
    let Some((anchor, view)) = with_app(|a| (a.anchor, a.view.clone())) else { return };

    let hmonitor = MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST);
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    let scale = dpi_x as f32 / 96.0;
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(hmonitor, &mut info);
    let work = info.rcWork;

    let lay = ui::layout(&view, scale);
    let margin = (12.0 * scale) as i32;
    let x = (anchor.x - lay.width / 2).clamp(work.left + margin, (work.right - lay.width - margin).max(work.left + margin));
    let y = (anchor.y - lay.height - margin).clamp(work.top + margin, (work.bottom - lay.height - margin).max(work.top + margin));

    with_app(|a| {
        if a.fonts.is_none() || (a.scale - scale).abs() > 0.01 {
            a.fonts = Some(Fonts::new(scale));
        }
        a.scale = scale;
    });

    let flags = if show { SWP_SHOWWINDOW } else { SWP_NOACTIVATE };
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, lay.width, lay.height, flags);
    if show {
        SetForegroundWindow(hwnd);
    }
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn append(menu: HMENU, flags: u32, id: usize, label: &str) {
    let text = wide(label);
    AppendMenuW(menu, flags, id, text.as_ptr());
}

unsafe fn show_menu(hwnd: HWND) {
    hide_panel(hwnd);

    let Some((monitors, presets)) = with_app(|a| {
        a.config = Config::load();
        let monitors: Vec<(String, Option<u32>)> =
            a.view.monitors.iter().map(|m| (m.name.clone(), m.input)).collect();
        let presets: Vec<String> = a.config.presets.iter().map(|p| p.name.clone()).collect();
        (monitors, presets)
    }) else {
        return;
    };

    let menu = CreatePopupMenu();
    append(menu, MF_STRING, ID_PANEL, "Open panel");
    append(menu, MF_SEPARATOR, 0, "");
    for (i, name) in presets.iter().enumerate().take(ID_MONITOR - ID_PRESET) {
        append(menu, MF_STRING, ID_PRESET + i, name);
    }
    if !presets.is_empty() {
        append(menu, MF_SEPARATOR, 0, "");
    }
    for (m, (name, input)) in monitors.iter().enumerate() {
        let sub = CreatePopupMenu();
        for (k, (label, code)) in INPUTS.iter().enumerate() {
            let checked = if *input == Some(*code) { MF_CHECKED } else { MF_UNCHECKED };
            append(sub, MF_STRING | checked, ID_MONITOR + m * 100 + k, &format!("Input: {label}"));
        }
        append(sub, MF_SEPARATOR, 0, "");
        append(sub, MF_STRING, ID_MONITOR + m * 100 + MON_POWER_ON, "Turn on");
        append(sub, MF_STRING, ID_MONITOR + m * 100 + MON_POWER_OFF, "Turn off");
        append(menu, MF_STRING | MF_POPUP, sub as usize, name);
    }
    if !monitors.is_empty() {
        append(menu, MF_SEPARATOR, 0, "");
    }
    append(menu, MF_STRING, ID_SCREENS_OFF, "Turn all screens off\tCtrl+Alt+End");
    append(menu, MF_STRING, ID_REFRESH, "Refresh monitors");
    append(menu, MF_SEPARATOR, 0, "");
    let autostart = if autostart::is_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    append(menu, MF_STRING | autostart, ID_AUTOSTART, "Start with Windows");
    append(menu, MF_STRING, ID_SETTINGS, "Edit settings…");
    append(menu, MF_SEPARATOR, 0, "");
    append(menu, MF_STRING, ID_EXIT, "Exit");

    let mut cursor: POINT = zeroed();
    GetCursorPos(&mut cursor);
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, cursor.x, cursor.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);

    run_command(hwnd, cmd as usize);
}

unsafe fn run_command(hwnd: HWND, cmd: usize) {
    match cmd {
        0 => {}
        ID_PANEL => {
            with_app(|a| a.last_hide = 0);
            open_panel(hwnd);
        }
        ID_EXIT => {
            DestroyWindow(hwnd);
        }
        ID_REFRESH => {
            with_app(|a| a.send(Cmd::Rescan));
        }
        ID_SCREENS_OFF => {
            SetTimer(hwnd, TIMER_SCREENS_OFF, 500, None);
        }
        ID_AUTOSTART => autostart::set(!autostart::is_enabled()),
        ID_SETTINGS => {
            config::ensure_exists();
            let file = wide(&config::path().to_string_lossy());
            let verb = wide("open");
            ShellExecuteW(hwnd, verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL);
        }
        c if (ID_PRESET..ID_MONITOR).contains(&c) => {
            with_app(|a| {
                let Some(preset) = a.config.presets.get(c - ID_PRESET) else { return };
                let cmds: Vec<Cmd> = preset
                    .values
                    .iter()
                    .enumerate()
                    .take(FEATURES.len())
                    .filter_map(|(feature, v)| v.map(|pct| Cmd::SetAllPercent { feature, pct }))
                    .collect();
                for cmd in cmds {
                    a.send(cmd);
                }
            });
        }
        c if c >= ID_MONITOR => {
            let (mon, item) = ((c - ID_MONITOR) / 100, (c - ID_MONITOR) % 100);
            let cmd = match item {
                k if k < INPUTS.len() => Some(Cmd::Raw { mon, code: VCP_INPUT, value: INPUTS[k].1 }),
                MON_POWER_ON => Some(Cmd::Raw { mon, code: VCP_POWER, value: POWER_ON }),
                MON_POWER_OFF => Some(Cmd::Raw { mon, code: VCP_POWER, value: POWER_OFF }),
                _ => None,
            };
            if let Some(cmd) = cmd {
                with_app(|a| a.send(cmd));
            }
        }
        _ => {}
    }
}
