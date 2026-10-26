//! Auto mode: what the screen should look like right now, from the time of day and the app in front.

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::{CloseHandle, RECT, SYSTEMTIME};
use windows_sys::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::ColorSystem::SetDeviceGammaRamp;
use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
    GWL_STYLE, WS_CAPTION,
};

use crate::config::{Config, Schedule};
use crate::util::{from_wide, wide};

pub const NEUTRAL_KELVIN: u32 = 6500;

/// Windows that take focus without being "the app you are using": taskbar, tray, Start, desktop.
const SHELL_CLASSES: [&str; 10] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "ForegroundStaging",
];

#[derive(Clone, Debug)]
pub struct Foreground {
    /// Lower-case exe file name, empty when Windows won't tell (some anti-cheat protected games).
    pub exe: String,
    /// GDI device of the monitor the window is on, e.g. \\.\DISPLAY1.
    pub device: String,
    pub fullscreen: bool,
}

pub struct Profile {
    pub label: String,
    pub game: bool,
    pub device: String,
    /// Indexed like `ddc::FEATURES`, in percent.
    pub values: [Option<u32>; 3],
}

pub struct Base {
    pub night: bool,
    pub brightness: u32,
    pub contrast: Option<u32>,
    pub kelvin: u32,
}

/// `None` when focus is on the shell or on Grovekeeper itself; callers keep the previous app then,
/// so opening the tray menu mid-game does not drop the game settings.
pub fn detect_foreground(own_pid: u32) -> Option<Foreground> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == own_pid {
            return None;
        }
        let mut class = [0u16; 128];
        let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
        if SHELL_CLASSES.contains(&class.as_str()) {
            return None;
        }

        let hmonitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFOEXW = zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        GetMonitorInfoW(hmonitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO);
        let screen = info.monitorInfo.rcMonitor;

        let mut rect: RECT = zeroed();
        GetWindowRect(hwnd, &mut rect);
        let covers = rect.left <= screen.left
            && rect.top <= screen.top
            && rect.right >= screen.right
            && rect.bottom >= screen.bottom;
        let has_title_bar = (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION) == WS_CAPTION;
        let mut state = 0;
        let d3d_fullscreen = SHQueryUserNotificationState(&mut state) == 0 && state == QUNS_RUNNING_D3D_FULL_SCREEN;

        Some(Foreground {
            exe: process_name(pid).unwrap_or_default(),
            device: from_wide(&info.szDevice),
            fullscreen: (covers && !has_title_bar) || d3d_fullscreen,
        })
    }
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit('\\').next().map(|s| s.to_ascii_lowercase())
    }
}

pub fn resolve(config: &Config, fg: &Foreground) -> Option<Profile> {
    let rule = config.apps.iter().find(|r| !fg.exe.is_empty() && r.exe == fg.exe);
    let game = match rule.and_then(|r| r.game) {
        Some(explicit) => explicit,
        None => {
            fg.fullscreen && config.games.fullscreen_is_game && !config.games.not_games.contains(&fg.exe)
        }
    };

    let mut values = if game { config.games.values } else { [None; 3] };
    if let Some(rule) = rule {
        for (slot, custom) in values.iter_mut().zip(rule.values) {
            if custom.is_some() {
                *slot = custom;
            }
        }
    }
    if !game && values.iter().all(Option::is_none) {
        return None;
    }

    let name = if fg.exe.is_empty() { "fullscreen app".to_string() } else { fg.exe.clone() };
    Some(Profile {
        label: if game { format!("Game · {name}") } else { name },
        game,
        device: fg.device.clone(),
        values,
    })
}

pub fn local_minutes() -> u32 {
    unsafe {
        let mut now: SYSTEMTIME = zeroed();
        GetLocalTime(&mut now);
        now.wHour as u32 * 60 + now.wMinute as u32
    }
}

/// Day/night values for this minute, fading over `fade_minutes` after each switch.
pub fn schedule_at(s: &Schedule, minutes: u32) -> Base {
    let since_day = (minutes as i32 - s.day_start as i32).rem_euclid(1440) as u32;
    let since_night = (minutes as i32 - s.night_start as i32).rem_euclid(1440) as u32;
    let night = since_night < since_day;
    let since = if night { since_night } else { since_day };
    let t = if s.fade_minutes == 0 { 1.0 } else { (since as f32 / s.fade_minutes as f32).min(1.0) };

    let lerp = |day: u32, night_value: u32| -> u32 {
        let (from, to) = if night { (day, night_value) } else { (night_value, day) };
        (from as f32 + (to as f32 - from as f32) * t).round() as u32
    };

    let contrast = match (s.day_contrast, s.night_contrast) {
        (Some(d), Some(n)) => Some(lerp(d, n)),
        (Some(d), None) if !night => Some(d),
        (None, Some(n)) if night => Some(n),
        _ => None,
    };

    Base {
        night,
        brightness: lerp(s.day_brightness, s.night_brightness),
        contrast,
        kelvin: lerp(NEUTRAL_KELVIN, s.night_light),
    }
}

/// Warms the colours of one display through its gamma ramp. Costs nothing after it is set:
/// the graphics card applies the ramp in hardware. Has no effect on HDR displays.
pub fn set_night_light(device: &str, kelvin: u32) {
    let (r, g, b) = kelvin_to_rgb(kelvin);
    let mut ramp = [0u16; 3 * 256];
    for i in 0..256 {
        let level = (i * 257) as f32;
        ramp[i] = (level * r).round().min(65535.0) as u16;
        ramp[256 + i] = (level * g).round().min(65535.0) as u16;
        ramp[512 + i] = (level * b).round().min(65535.0) as u16;
    }
    unsafe {
        let name = wide(device);
        let hdc = CreateDCW(null(), name.as_ptr(), null(), null());
        if hdc.is_null() {
            return;
        }
        SetDeviceGammaRamp(hdc, ramp.as_ptr() as *const _);
        DeleteDC(hdc);
    }
}

/// Channel multipliers for a colour temperature, normalised so 6500 K is exactly neutral.
fn kelvin_to_rgb(kelvin: u32) -> (f32, f32, f32) {
    if kelvin >= NEUTRAL_KELVIN {
        return (1.0, 1.0, 1.0);
    }
    let raw = |k: f32| -> (f32, f32, f32) {
        let t = k / 100.0;
        let r = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
        let g = if t <= 66.0 {
            99.470_8 * t.ln() - 161.119_57
        } else {
            288.122_17 * (t - 60.0).powf(-0.075_514_85)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_73 * (t - 10.0).ln() - 305.044_8
        };
        (r.clamp(0.0, 255.0), g.clamp(0.0, 255.0), b.clamp(0.0, 255.0))
    };
    let (nr, ng, nb) = raw(NEUTRAL_KELVIN as f32);
    let (r, g, b) = raw(kelvin as f32);
    ((r / nr).min(1.0), (g / ng).min(1.0), (b / nb).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule() -> Schedule {
        Schedule {
            day_start: 7 * 60,
            night_start: 20 * 60,
            fade_minutes: 30,
            day_brightness: 85,
            night_brightness: 65,
            day_contrast: None,
            night_contrast: None,
            night_light: 5500,
        }
    }

    #[test]
    fn day_and_night_values() {
        let s = schedule();
        let noon = schedule_at(&s, 12 * 60);
        assert!(!noon.night);
        assert_eq!((noon.brightness, noon.kelvin), (85, 6500));
        let late = schedule_at(&s, 23 * 60);
        assert!(late.night);
        assert_eq!((late.brightness, late.kelvin), (65, 5500));
        let early = schedule_at(&s, 3 * 60);
        assert!(early.night);
        assert_eq!(early.brightness, 65);
    }

    #[test]
    fn fades_after_each_switch() {
        let s = schedule();
        assert_eq!(schedule_at(&s, 20 * 60).brightness, 85);
        assert_eq!(schedule_at(&s, 20 * 60 + 15).brightness, 75);
        assert_eq!(schedule_at(&s, 20 * 60 + 30).brightness, 65);
        assert_eq!(schedule_at(&s, 7 * 60 + 15).brightness, 75);
        assert_eq!(schedule_at(&s, 7 * 60 + 15).kelvin, 6000);
    }

    #[test]
    fn neutral_is_exactly_neutral() {
        assert_eq!(kelvin_to_rgb(6500), (1.0, 1.0, 1.0));
        let (r, g, b) = kelvin_to_rgb(5500);
        assert!(r == 1.0 && g < 1.0 && b < g && b > 0.8);
    }
}
