//! The settings window: everything in config.ini, edited with switches, steppers and sliders.
//! Each change is written to the file straight away and applied immediately.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush, DeleteDC,
    DeleteObject, EndPaint, FrameRect, GetMonitorInfoW, InvalidateRect, MonitorFromPoint,
    MonitorFromWindow, ScreenToClient, SelectObject, SetBkMode, DT_CENTER, DT_END_ELLIPSIS,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, Config};
use crate::ui::{contains, fill, rc, text, Fonts, ACCENT, BG, BORDER, KNOB, KNOB_ACTIVE, MUTED, TEXT, TRACK};
use crate::util::wide;
use crate::{autostart, icon};

const CLASS: &str = "GrovekeeperSettings";
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    AutoEnabled,
    Autostart,
    Step,
    DayStart,
    NightStart,
    Fade,
    DayBrightness,
    NightBrightness,
    NightLight,
    GamesFullscreen,
    GameBrightness,
    GameContrast,
    GameNightLight,
}

impl Field {
    /// (min, max, step)
    fn range(self) -> (i32, i32, i32) {
        match self {
            Field::AutoEnabled | Field::Autostart | Field::GamesFullscreen => (0, 1, 1),
            Field::Step => (1, 50, 1),
            Field::DayStart | Field::NightStart => (0, 1425, 15),
            Field::Fade => (0, 120, 5),
            _ => (0, 100, 1),
        }
    }

    fn display(self, v: i32) -> String {
        match self {
            Field::DayStart | Field::NightStart => format!("{:02}:{:02}", v / 60, v % 60),
            Field::Fade if v == 0 => "Instant".into(),
            Field::Fade => format!("{v} min"),
            Field::NightLight | Field::GameNightLight if v == 0 => "Off".into(),
            _ => format!("{v}%"),
        }
    }

    fn stepped(self, v: i32, delta: i32) -> i32 {
        let (min, max, _) = self.range();
        match self {
            Field::DayStart | Field::NightStart => (v + delta).rem_euclid(24 * 60),
            _ => (v + delta).clamp(min, max),
        }
    }
}

/// Night light as a 0-100% strength: 0% = 6500 K (off), 100% = 3500 K.
fn strength(kelvin: u32) -> i32 {
    ((6500 - kelvin.min(6500)) as i32 + 15) / 30
}

fn kelvin(strength: i32) -> u32 {
    (6500 - strength.clamp(0, 100) * 30) as u32
}

struct Model {
    config: Config,
    autostart: bool,
    recent: Vec<String>,
}

impl Model {
    fn load() -> Model {
        Model {
            config: Config::load(),
            autostart: autostart::is_enabled(),
            recent: crate::with_app(|a| a.recent.clone()).unwrap_or_default(),
        }
    }

    fn get(&self, f: Field) -> i32 {
        let c = &self.config;
        match f {
            Field::AutoEnabled => c.auto_enabled as i32,
            Field::Autostart => self.autostart as i32,
            Field::Step => c.step as i32,
            Field::DayStart => c.schedule.day_start as i32,
            Field::NightStart => c.schedule.night_start as i32,
            Field::Fade => c.schedule.fade_minutes as i32,
            Field::DayBrightness => c.schedule.day_brightness as i32,
            Field::NightBrightness => c.schedule.night_brightness as i32,
            Field::NightLight => strength(c.schedule.night_light),
            Field::GamesFullscreen => c.games.fullscreen_is_game as i32,
            Field::GameBrightness => c.games.values[0].unwrap_or(100) as i32,
            Field::GameContrast => c.games.values[1].unwrap_or(65) as i32,
            Field::GameNightLight => strength(c.games.night_light),
        }
    }
}

fn persist(f: Field, v: i32) {
    let flag = if v != 0 { "true" } else { "false" };
    let num = v.to_string();
    let (section, key, value) = match f {
        Field::Autostart => {
            autostart::set(v != 0);
            return;
        }
        Field::AutoEnabled => ("auto", "enabled", flag.to_string()),
        Field::Step => ("", "step", num),
        Field::DayStart => ("auto", "day_start", f.display(v)),
        Field::NightStart => ("auto", "night_start", f.display(v)),
        Field::Fade => ("auto", "fade_minutes", num),
        Field::DayBrightness => ("auto", "day_brightness", num),
        Field::NightBrightness => ("auto", "night_brightness", num),
        Field::NightLight => ("auto", "night_light", kelvin(v).to_string()),
        Field::GamesFullscreen => ("games", "fullscreen_is_game", flag.to_string()),
        Field::GameBrightness => ("games", "brightness", num),
        Field::GameContrast => ("games", "contrast", num),
        Field::GameNightLight => ("games", "night_light", kelvin(v).to_string()),
    };
    config::set_value(section, key, &value);
}

enum Kind {
    Section,
    Note,
    Toggle(Field),
    Stepper(Field),
    Slider(Field),
    App { exe: String, game: Option<bool> },
    AddApp(String),
    OpenFile,
}

struct Item {
    kind: Kind,
    label: String,
    row: RECT,
    /// Control parts; what they mean depends on the kind (switch, minus/value/plus, bar/value, chip/remove).
    a: RECT,
    b: RECT,
    c: RECT,
}

enum Hit {
    Set(Field, i32),
    Drag(Field, i32),
    AppGame(String, bool),
    AppRemove(String),
    OpenFile,
}

enum Effect {
    None,
    Redraw,
    Save(Hit),
}

struct Settings {
    hwnd: HWND,
    scale: f32,
    fonts: Fonts,
    model: Model,
    items: Vec<Item>,
    width: i32,
    content_h: i32,
    scroll: i32,
    drag: Option<(Field, i32)>,
    icons: [HICON; 2],
}

thread_local! {
    static SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Settings) -> R) -> Option<R> {
    SETTINGS.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

impl Settings {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    fn value(&self, f: Field) -> i32 {
        match self.drag {
            Some((dragged, v)) if dragged == f => v,
            _ => self.model.get(f),
        }
    }

    fn relayout(&mut self) {
        let (items, width, content_h) = build(&self.model, self.scale);
        self.items = items;
        self.width = width;
        self.content_h = content_h;
        self.clamp_scroll();
    }

    fn client_height(&self) -> i32 {
        let mut client: RECT = unsafe { zeroed() };
        unsafe { GetClientRect(self.hwnd, &mut client) };
        client.bottom - client.top
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_h - self.client_height()).max(0);
        self.scroll = self.scroll.clamp(0, max);
    }

    fn slider_value(&self, f: Field, bar: &RECT, x: i32) -> i32 {
        let inset = self.px(6.0);
        let (left, right) = (bar.left + inset, bar.right - inset);
        let (min, max, _) = f.range();
        if right <= left {
            return min;
        }
        let t = ((x - left) as f32 / (right - left) as f32).clamp(0.0, 1.0);
        min + (t * (max - min) as f32).round() as i32
    }

    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        let y = y + self.scroll;
        for item in &self.items {
            match &item.kind {
                Kind::Toggle(f) if contains(&item.row, x, y) => return Some(Hit::Set(*f, 1 - self.model.get(*f))),
                Kind::Stepper(f) => {
                    let (_, _, step) = f.range();
                    let v = self.model.get(*f);
                    if contains(&item.a, x, y) {
                        return Some(Hit::Set(*f, f.stepped(v, -step)));
                    }
                    if contains(&item.c, x, y) {
                        return Some(Hit::Set(*f, f.stepped(v, step)));
                    }
                }
                Kind::Slider(f) => {
                    let area = RECT { left: item.a.left, right: item.a.right, ..item.row };
                    if contains(&area, x, y) {
                        return Some(Hit::Drag(*f, self.slider_value(*f, &item.a, x)));
                    }
                }
                Kind::App { exe, game } => {
                    if contains(&item.a, x, y) {
                        return Some(Hit::AppGame(exe.clone(), *game != Some(true)));
                    }
                    if contains(&item.b, x, y) {
                        return Some(Hit::AppRemove(exe.clone()));
                    }
                }
                Kind::AddApp(exe) if contains(&item.row, x, y) => return Some(Hit::AppGame(exe.clone(), true)),
                Kind::OpenFile if contains(&item.a, x, y) => return Some(Hit::OpenFile),
                _ => {}
            }
        }
        None
    }

    fn click(&mut self, x: i32, y: i32) -> (Effect, bool) {
        match self.hit(x, y) {
            Some(Hit::Drag(f, v)) => {
                self.drag = Some((f, v));
                (Effect::Redraw, true)
            }
            Some(hit) => (Effect::Save(hit), false),
            None => (Effect::None, false),
        }
    }

    fn drag_to(&mut self, x: i32) {
        let Some((f, _)) = self.drag else { return };
        let bar = self.items.iter().find_map(|item| match item.kind {
            Kind::Slider(field) if field == f => Some(item.a),
            _ => None,
        });
        if let Some(bar) = bar {
            self.drag = Some((f, self.slider_value(f, &bar, x)));
        }
    }

    fn wheel(&mut self, x: i32, y: i32, notches: i32) -> Effect {
        let content_y = y + self.scroll;
        let slider = self.items.iter().find_map(|item| match item.kind {
            Kind::Slider(f) if contains(&item.row, x, content_y) => Some(f),
            _ => None,
        });
        if let Some(f) = slider {
            let step = if f == Field::Step { 1 } else { 5 };
            let v = f.stepped(self.model.get(f), if notches > 0 { step } else { -step });
            return Effect::Save(Hit::Set(f, v));
        }
        self.scroll -= notches * self.px(48.0);
        self.clamp_scroll();
        Effect::Redraw
    }
}

fn build(model: &Model, scale: f32) -> (Vec<Item>, i32, i32) {
    let s = |v: f32| (v * scale).round() as i32;
    let width = s(460.0);
    let pad = s(22.0);
    let right = width - pad;
    let row_h = s(38.0);
    let label_w = s(190.0);
    let zero = rc(0, 0, 0, 0);

    let mut items = Vec::new();
    let mut y = s(6.0);

    let mut push = |kind: Kind, label: &str, y: &mut i32| {
        let (h, row) = match kind {
            Kind::Section => (s(44.0), rc(pad, *y + s(16.0), right, *y + s(42.0))),
            Kind::Note => (s(26.0), rc(pad, *y, right, *y + s(26.0))),
            _ => (row_h, rc(pad, *y, right, *y + row_h)),
        };
        let mid = (row.top + row.bottom) / 2;
        let (a, b, c) = match &kind {
            Kind::Toggle(_) => (rc(right - s(40.0), mid - s(10.0), right, mid + s(10.0)), zero, zero),
            Kind::Stepper(_) => {
                let plus = rc(right - s(30.0), mid - s(14.0), right, mid + s(14.0));
                let minus = rc(right - s(140.0), mid - s(14.0), right - s(110.0), mid + s(14.0));
                (minus, rc(minus.right, row.top, plus.left, row.bottom), plus)
            }
            Kind::Slider(_) => {
                let value = rc(right - s(48.0), row.top, right, row.bottom);
                (rc(pad + label_w, row.top, value.left - s(10.0), row.bottom), value, zero)
            }
            Kind::App { .. } => {
                let remove = rc(right - s(28.0), mid - s(13.0), right, mid + s(13.0));
                (rc(remove.left - s(112.0), mid - s(13.0), remove.left - s(8.0), mid + s(13.0)), remove, zero)
            }
            Kind::AddApp(_) => (rc(right - s(112.0), mid - s(13.0), right, mid + s(13.0)), zero, zero),
            Kind::OpenFile => (rc(pad, mid - s(16.0), pad + s(180.0), mid + s(16.0)), zero, zero),
            _ => (zero, zero, zero),
        };
        items.push(Item { kind, label: label.to_string(), row, a, b, c });
        *y += h;
    };

    push(Kind::Section, "General", &mut y);
    push(Kind::Toggle(Field::AutoEnabled), "Auto mode", &mut y);
    push(Kind::Toggle(Field::Autostart), "Start with Windows", &mut y);
    push(Kind::Slider(Field::Step), "Hotkey brightness step", &mut y);

    push(Kind::Section, "Day and night", &mut y);
    push(Kind::Stepper(Field::DayStart), "Day starts", &mut y);
    push(Kind::Stepper(Field::NightStart), "Night starts", &mut y);
    push(Kind::Stepper(Field::Fade), "Fade time", &mut y);
    push(Kind::Slider(Field::DayBrightness), "Day brightness", &mut y);
    push(Kind::Slider(Field::NightBrightness), "Night brightness", &mut y);
    push(Kind::Slider(Field::NightLight), "Night light", &mut y);

    push(Kind::Section, "Games", &mut y);
    push(Kind::Toggle(Field::GamesFullscreen), "Fullscreen apps are games", &mut y);
    push(Kind::Slider(Field::GameBrightness), "Brightness", &mut y);
    push(Kind::Slider(Field::GameContrast), "Contrast", &mut y);
    push(Kind::Slider(Field::GameNightLight), "Night light", &mut y);
    push(Kind::Note, "At night only. Never warmer than the normal night light.", &mut y);

    push(Kind::Section, "Apps", &mut y);
    if model.config.apps.is_empty() {
        push(Kind::Note, "No app rules yet. Add one from the recent apps below.", &mut y);
    }
    for rule in &model.config.apps {
        push(Kind::App { exe: rule.exe.clone(), game: rule.game }, &rule.exe, &mut y);
    }
    let recent: Vec<&String> = model
        .recent
        .iter()
        .filter(|exe| !model.config.apps.iter().any(|r| &r.exe == *exe))
        .take(6)
        .collect();
    if !recent.is_empty() {
        push(Kind::Note, "Recently used:", &mut y);
        for exe in recent {
            push(Kind::AddApp(exe.clone()), exe, &mut y);
        }
    }

    push(Kind::Section, "More", &mut y);
    push(Kind::OpenFile, "Open settings file", &mut y);
    push(Kind::Note, "Presets, per-app brightness and the not-a-game list are in the file.", &mut y);

    (items, width, y + s(16.0))
}

pub fn register(instance: HINSTANCE) {
    unsafe {
        let class = wide(CLASS);
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.lpszClassName = class.as_ptr();
        RegisterClassExW(&wc);
    }
}

pub fn open() {
    unsafe {
        if let Some(hwnd) = with(|s| s.hwnd) {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
            return;
        }

        let class = wide(CLASS);
        let title = wide("Grovekeeper settings");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let dark: i32 = 1;
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as _, &dark as *const i32 as _, size_of::<i32>() as u32);
        let icons = [icon::leaf(GetSystemMetrics(SM_CXSMICON)), icon::leaf(GetSystemMetrics(SM_CXICON))];
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icons[0] as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icons[1] as isize);

        // Move to the monitor the user is on first, so the DPI below is the right one.
        let mut cursor: POINT = zeroed();
        GetCursorPos(&mut cursor);
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut info);
        SetWindowPos(hwnd, null_mut(), info.rcWork.left + 40, info.rcWork.top + 40, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);

        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let model = Model::load();
        let (items, width, content_h) = build(&model, scale);
        SETTINGS.with(|cell| {
            *cell.borrow_mut() = Some(Settings {
                hwnd,
                scale,
                fonts: Fonts::new(scale),
                model,
                items,
                width,
                content_h,
                scroll: 0,
                drag: None,
                icons,
            })
        });

        size_window(hwnd, true);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

/// Fits the window to its content, up to most of the screen height (the rest scrolls).
unsafe fn size_window(hwnd: HWND, center: bool) {
    let Some((width, content_h, scale)) = with(|s| (s.width, s.content_h, s.scale)) else { return };
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
    let work = info.rcWork;
    let client_h = content_h.min(((work.bottom - work.top) as f32 * 0.88) as i32);

    let mut outer = rc(0, 0, width, client_h);
    AdjustWindowRectExForDpi(
        &mut outer,
        GetWindowLongW(hwnd, GWL_STYLE) as u32,
        0,
        GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
        (scale * 96.0).round() as u32,
    );
    let (w, h) = (outer.right - outer.left, outer.bottom - outer.top);
    if center {
        let x = work.left + (work.right - work.left - w) / 2;
        let y = work.top + (work.bottom - work.top - h) / 2;
        SetWindowPos(hwnd, null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    } else {
        SetWindowPos(hwnd, null_mut(), 0, 0, w, h, SWP_NOZORDER | SWP_NOMOVE | SWP_NOACTIVATE);
    }
    with(|s| s.clamp_scroll());
}

/// Saves a change, applies it to the running app, and reloads the window from the file.
/// Runs outside any borrow, because opening a file or applying settings can pump messages.
unsafe fn apply(hwnd: HWND, effect: Effect) {
    let hit = match effect {
        Effect::None => return,
        Effect::Redraw => {
            InvalidateRect(hwnd, null(), 0);
            return;
        }
        Effect::Save(hit) => hit,
    };
    match hit {
        Hit::Set(f, v) | Hit::Drag(f, v) => persist(f, v),
        Hit::AppGame(exe, game) => config::set_value(&format!("app {exe}"), "game", if game { "true" } else { "false" }),
        Hit::AppRemove(exe) => config::remove_section(&format!("app {exe}")),
        Hit::OpenFile => {
            config::ensure_exists();
            let file = wide(&config::path().to_string_lossy());
            let verb = wide("open");
            ShellExecuteW(hwnd, verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL);
            return;
        }
    }
    crate::with_app(|a| {
        a.config = Config::load();
        a.evaluate(true);
    });
    reload(hwnd);
}

unsafe fn reload(hwnd: HWND) {
    with(|s| {
        s.model = Model::load();
        s.relayout();
    });
    size_window(hwnd, false);
    InvalidateRect(hwnd, null(), 0);
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            with(|s| paint(s, hdc));
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let (effect, capture) = with(|s| s.click(x, y)).unwrap_or((Effect::None, false));
            if capture {
                SetCapture(hwnd);
            }
            apply(hwnd, effect);
            0
        }
        WM_MOUSEMOVE => {
            if with(|s| s.drag.is_some()).unwrap_or(false) {
                let (x, _) = lparam_xy(lparam);
                with(|s| s.drag_to(x));
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            if let Some((f, v)) = with(|s| s.drag.take()).flatten() {
                if msg == WM_LBUTTONUP {
                    ReleaseCapture();
                }
                apply(hwnd, Effect::Save(Hit::Set(f, v)));
            }
            0
        }
        WM_MOUSEWHEEL => {
            let notches = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
            let (x, y) = lparam_xy(lparam);
            let mut pt = POINT { x, y };
            ScreenToClient(hwnd, &mut pt);
            let effect = with(|s| s.wheel(pt.x, pt.y, notches)).unwrap_or(Effect::None);
            apply(hwnd, effect);
            0
        }
        WM_ACTIVATE => {
            if wparam & 0xFFFF != WA_INACTIVE as usize {
                reload(hwnd);
            }
            0
        }
        WM_KEYDOWN => {
            if wparam == VK_ESCAPE as usize {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            with(|s| {
                s.scale = dpi.max(96) as f32 / 96.0;
                s.fonts = Fonts::new(s.scale);
                s.relayout();
            });
            let suggested = &*(lparam as *const RECT);
            SetWindowPos(hwnd, null_mut(), suggested.left, suggested.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            size_window(hwnd, false);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DESTROY => {
            let state = SETTINGS.with(|cell| cell.try_borrow_mut().ok().and_then(|mut g| g.take()));
            if let Some(s) = state {
                for icon in s.icons {
                    DestroyIcon(icon);
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn frame(hdc: HDC, r: &RECT, color: u32) {
    let brush = CreateSolidBrush(color);
    FrameRect(hdc, r, brush);
    DeleteObject(brush);
}

unsafe fn paint(s: &Settings, hdc: HDC) {
    let mut client: RECT = zeroed();
    GetClientRect(s.hwnd, &mut client);
    let (w, h) = (client.right, client.bottom);
    let mem = CreateCompatibleDC(hdc);
    let bitmap = CreateCompatibleBitmap(hdc, w, h);
    let old_bitmap = SelectObject(mem, bitmap);
    fill(mem, &client, BG);
    SetBkMode(mem, TRANSPARENT as _);

    let one_line = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;
    let centered = DT_CENTER | DT_SINGLELINE | DT_VCENTER;
    let shift = |r: &RECT| RECT { top: r.top - s.scroll, bottom: r.bottom - s.scroll, ..*r };

    for item in &s.items {
        let row = shift(&item.row);
        if row.bottom < 0 || row.top > h {
            continue;
        }
        let (a, b, c) = (shift(&item.a), shift(&item.b), shift(&item.c));
        match &item.kind {
            Kind::Section => text(mem, s.fonts.title, TEXT, &item.label, row, DT_LEFT | one_line),
            Kind::Note => text(mem, s.fonts.small, MUTED, &item.label, row, DT_LEFT | DT_WORDBREAK),
            Kind::Toggle(f) => {
                text(mem, s.fonts.normal, TEXT, &item.label, row, DT_LEFT | one_line);
                let on = s.value(*f) != 0;
                fill(mem, &a, if on { ACCENT } else { TRACK });
                let inset = s.px(3.0);
                let knob = (a.bottom - a.top) - 2 * inset;
                let kx = if on { a.right - inset - knob } else { a.left + inset };
                fill(mem, &rc(kx, a.top + inset, kx + knob, a.bottom - inset), KNOB);
            }
            Kind::Stepper(f) => {
                text(mem, s.fonts.normal, TEXT, &item.label, row, DT_LEFT | one_line);
                frame(mem, &a, BORDER);
                frame(mem, &c, BORDER);
                text(mem, s.fonts.normal, TEXT, "−", a, centered);
                text(mem, s.fonts.normal, TEXT, "+", c, centered);
                text(mem, s.fonts.normal, TEXT, &f.display(s.value(*f)), b, centered);
            }
            Kind::Slider(f) => {
                text(mem, s.fonts.normal, TEXT, &item.label, row, DT_LEFT | one_line);
                let v = s.value(*f);
                let (min, max, _) = f.range();
                let inset = s.px(6.0);
                let (left, right) = (a.left + inset, a.right - inset);
                let mid = (a.top + a.bottom) / 2;
                let track_h = s.px(4.0).max(2);
                let top = mid - track_h / 2;
                fill(mem, &rc(left, top, right, top + track_h), TRACK);
                let x = left + ((right - left) as f32 * (v - min) as f32 / (max - min).max(1) as f32) as i32;
                fill(mem, &rc(left, top, x, top + track_h), ACCENT);
                let active = s.drag.is_some_and(|(d, _)| d == *f);
                let (kw, kh) = (s.px(4.0).max(2), s.px(16.0));
                fill(mem, &rc(x - kw / 2, mid - kh / 2, x - kw / 2 + kw, mid - kh / 2 + kh), if active { KNOB_ACTIVE } else { KNOB });
                text(mem, s.fonts.normal, MUTED, &f.display(v), b, DT_LEFT | one_line | DT_SINGLELINE);
            }
            Kind::App { game, .. } => {
                text(mem, s.fonts.normal, TEXT, &item.label, RECT { right: a.left - s.px(8.0), ..row }, DT_LEFT | one_line);
                fill(mem, &a, TRACK);
                let (label, color) = match game {
                    Some(true) => ("Game", ACCENT),
                    Some(false) => ("Not a game", MUTED),
                    None => ("Custom", TEXT),
                };
                text(mem, s.fonts.small, color, label, a, centered);
                text(mem, s.fonts.normal, MUTED, "×", b, centered);
            }
            Kind::AddApp(_) => {
                text(mem, s.fonts.normal, MUTED, &item.label, RECT { right: a.left - s.px(8.0), ..row }, DT_LEFT | one_line);
                frame(mem, &a, BORDER);
                text(mem, s.fonts.small, ACCENT, "+ Add as game", a, centered);
            }
            Kind::OpenFile => {
                fill(mem, &a, TRACK);
                text(mem, s.fonts.normal, TEXT, &item.label, a, centered);
            }
        }
    }

    if s.content_h > h {
        let thumb_h = (h * h / s.content_h).max(s.px(24.0));
        let thumb_top = s.scroll * (h - thumb_h) / (s.content_h - h).max(1);
        fill(mem, &rc(w - s.px(5.0), thumb_top, w - s.px(1.0), thumb_top + thumb_h), BORDER);
    }

    BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bitmap);
    DeleteObject(bitmap);
    DeleteDC(mem);
}
