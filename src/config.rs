//! Settings live in %APPDATA%\Grovekeeper\config.ini, a plain text file the user edits.

use std::fs;
use std::path::PathBuf;

const DEFAULT_BASIC: &str = "\
# Grovekeeper settings
# Lines starting with # are comments. Save the file and the changes apply
# the next time you open the panel or the tray menu (auto mode: within a minute).

# How much Ctrl+Alt+Up / Ctrl+Alt+Down changes brightness, in percent.
step = 10

# Presets appear at the top of the tray menu and apply to every monitor.
# Values are percent (0-100). Leave a line out to keep that setting as it is.
# Supported settings: brightness, contrast, volume.

[preset Day]
brightness = 80
contrast = 60

[preset Evening]
brightness = 45
contrast = 50

[preset Night]
brightness = 10
contrast = 40

[preset Gaming]
brightness = 100
contrast = 70
";

const DEFAULT_AUTO: &str = "
# Auto mode: brightness by time of day, a slight night light at night,
# and game settings while a game is in front.
# Changing a slider or using a hotkey overrides auto mode until the next
# change (switching apps, or the schedule moving on).
[auto]
enabled = true
day_start = 07:00
night_start = 20:00
# How long the change from day to night (and back) takes.
fade_minutes = 30
day_brightness = 85
night_brightness = 65
# Optional: day_contrast = 50 / night_contrast = 45
# Night light colour temperature in kelvin. 6500 = off, lower = warmer.
night_light = 5500

[games]
# Treat any fullscreen app as a game.
fullscreen_is_game = true
brightness = 100
contrast = 65
# A lighter night light while gaming, in kelvin. Never warmer than the normal
# night light, and off in the daytime. 6500 = no night light in games.
night_light = 6000
# Fullscreen apps that are not games (videos, browsers).
not_games = chrome.exe, msedge.exe, firefox.exe, brave.exe, opera.exe, vlc.exe, mpc-hc64.exe, potplayermini64.exe, netflix.exe, powerpnt.exe

# Per-app rules. Use the exe name of the app.
#   game = true    always use the game settings
#   game = false   never treat it as a game
#   brightness / contrast / volume = custom values while that app is in front
# Example:
# [app valorant-win64-shipping.exe]
# game = true
#
# [app vlc.exe]
# brightness = 70
";

pub struct Preset {
    pub name: String,
    /// Indexed like `ddc::FEATURES`, in percent.
    pub values: [Option<u32>; 3],
}

pub struct Schedule {
    /// Minutes after midnight.
    pub day_start: u32,
    pub night_start: u32,
    pub fade_minutes: u32,
    pub day_brightness: u32,
    pub night_brightness: u32,
    pub day_contrast: Option<u32>,
    pub night_contrast: Option<u32>,
    /// Kelvin. 6500 means no tint.
    pub night_light: u32,
}

pub struct Games {
    pub fullscreen_is_game: bool,
    /// Indexed like `ddc::FEATURES`, in percent.
    pub values: [Option<u32>; 3],
    /// Kelvin while a game is in front. 6500 means no tint.
    pub night_light: u32,
    pub not_games: Vec<String>,
}

pub struct AppRule {
    /// Lower-case exe file name.
    pub exe: String,
    pub game: Option<bool>,
    pub values: [Option<u32>; 3],
}

pub struct Config {
    pub step: u32,
    pub presets: Vec<Preset>,
    pub auto_enabled: bool,
    pub schedule: Schedule,
    pub games: Games,
    pub apps: Vec<AppRule>,
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
        .join("Grovekeeper")
        .join("config.ini")
}

/// Creates the file, or adds the auto-mode block to a file written by an older version.
pub fn ensure_exists() {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(text) => {
            if !text.lines().any(|l| l.trim().eq_ignore_ascii_case("[auto]")) {
                let _ = fs::write(&path, format!("{}\n{}", text.trim_end(), DEFAULT_AUTO));
            }
        }
        Err(_) => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&path, format!("{DEFAULT_BASIC}{DEFAULT_AUTO}"));
        }
    }
}

impl Config {
    pub fn load() -> Config {
        ensure_exists();
        let text = fs::read_to_string(path()).unwrap_or_else(|_| format!("{DEFAULT_BASIC}{DEFAULT_AUTO}"));
        parse(&text)
    }

    pub fn app_is_game(&self, exe: &str) -> Option<bool> {
        self.apps.iter().find(|r| r.exe == exe).and_then(|r| r.game)
    }
}

enum Section {
    Top,
    Preset(usize),
    Auto,
    Games,
    App(usize),
    Unknown,
}

fn parse(text: &str) -> Config {
    let mut config = Config {
        step: 10,
        presets: Vec::new(),
        auto_enabled: true,
        schedule: Schedule {
            day_start: 7 * 60,
            night_start: 20 * 60,
            fade_minutes: 30,
            day_brightness: 85,
            night_brightness: 65,
            day_contrast: None,
            night_contrast: None,
            night_light: 5500,
        },
        games: Games {
            fullscreen_is_game: true,
            values: [Some(100), Some(65), None],
            night_light: 6000,
            not_games: Vec::new(),
        },
        apps: Vec::new(),
    };
    let mut section = Section::Top;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = parse_section(inner.trim(), &mut config);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();

        match section {
            Section::Top => {
                if key == "step" {
                    if let Some(v) = percent(value) {
                        config.step = v.clamp(1, 50);
                    }
                }
            }
            Section::Preset(i) => {
                if let (Some(f), Some(v)) = (feature_index(&key), percent(value)) {
                    config.presets[i].values[f] = Some(v);
                }
            }
            Section::Auto => {
                let s = &mut config.schedule;
                match key.as_str() {
                    "enabled" => config.auto_enabled = boolean(value).unwrap_or(config.auto_enabled),
                    "day_start" => s.day_start = clock(value).unwrap_or(s.day_start),
                    "night_start" => s.night_start = clock(value).unwrap_or(s.night_start),
                    "fade_minutes" => s.fade_minutes = value.parse::<u32>().map(|m| m.min(720)).unwrap_or(s.fade_minutes),
                    "day_brightness" => s.day_brightness = percent(value).unwrap_or(s.day_brightness),
                    "night_brightness" => s.night_brightness = percent(value).unwrap_or(s.night_brightness),
                    "day_contrast" => s.day_contrast = percent(value),
                    "night_contrast" => s.night_contrast = percent(value),
                    "night_light" => s.night_light = kelvin(value).unwrap_or(s.night_light),
                    _ => {}
                }
            }
            Section::Games => {
                let g = &mut config.games;
                match key.as_str() {
                    "fullscreen_is_game" => g.fullscreen_is_game = boolean(value).unwrap_or(g.fullscreen_is_game),
                    "night_light" => g.night_light = kelvin(value).unwrap_or(g.night_light),
                    "not_games" => {
                        g.not_games = value
                            .split(',')
                            .map(|s| s.trim().to_ascii_lowercase())
                            .filter(|s| !s.is_empty())
                            .collect()
                    }
                    k => {
                        if let Some(f) = feature_index(k) {
                            g.values[f] = percent(value);
                        }
                    }
                }
            }
            Section::App(i) => {
                let rule = &mut config.apps[i];
                if key == "game" {
                    rule.game = boolean(value);
                } else if let (Some(f), Some(v)) = (feature_index(&key), percent(value)) {
                    rule.values[f] = Some(v);
                }
            }
            Section::Unknown => {}
        }
    }
    config
}

fn parse_section(name: &str, config: &mut Config) -> Section {
    let lower = name.to_ascii_lowercase();
    if lower == "auto" {
        return Section::Auto;
    }
    if lower == "games" {
        return Section::Games;
    }
    if let Some(exe) = lower.strip_prefix("app ") {
        let exe = exe.trim().to_string();
        if !exe.is_empty() {
            config.apps.push(AppRule { exe, game: None, values: [None; 3] });
            return Section::App(config.apps.len() - 1);
        }
    }
    if let Some(preset) = name.strip_prefix("preset").map(str::trim) {
        if !preset.is_empty() {
            config.presets.push(Preset { name: preset.to_string(), values: [None; 3] });
            return Section::Preset(config.presets.len() - 1);
        }
    }
    Section::Unknown
}

fn feature_index(key: &str) -> Option<usize> {
    match key {
        "brightness" => Some(0),
        "contrast" => Some(1),
        "volume" => Some(2),
        _ => None,
    }
}

fn percent(value: &str) -> Option<u32> {
    value.trim_end_matches('%').trim().parse::<u32>().ok().map(|v| v.min(100))
}

fn kelvin(value: &str) -> Option<u32> {
    value.trim_end_matches(['k', 'K']).trim().parse::<u32>().ok().map(|k| k.clamp(3000, 6500))
}

fn boolean(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// "07:30" or "7" -> minutes after midnight.
fn clock(value: &str) -> Option<u32> {
    let (h, m) = value.split_once(':').unwrap_or((value, "0"));
    let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Sets `key = value` inside `[section]` (an empty section means the top of the file),
/// adding the key or the section when missing. Every other line and comment stays as it is.
pub fn set_value(section: &str, key: &str, value: &str) {
    edit_file(|text| with_value(text, section, key, value));
}

/// Deletes `[section]` and its lines from the file.
pub fn remove_section(section: &str) {
    edit_file(|text| without_section(text, section));
}

fn edit_file(edit: impl FnOnce(&str) -> String) {
    ensure_exists();
    let path = path();
    if let Ok(text) = fs::read_to_string(&path) {
        let _ = fs::write(&path, edit(&text));
    }
}

fn header(line: &str) -> Option<String> {
    line.trim()
        .strip_prefix('[')
        .and_then(|l| l.strip_suffix(']'))
        .map(|s| s.trim().to_ascii_lowercase())
}

/// Line range of a section's body: (index of its first line after the header, end index).
fn section_body(lines: &[String], section: &str) -> Option<(usize, usize)> {
    let start = if section.is_empty() {
        0
    } else {
        let wanted = section.to_ascii_lowercase();
        lines.iter().position(|l| header(l).as_deref() == Some(wanted.as_str()))? + 1
    };
    let end = lines[start..].iter().position(|l| header(l).is_some()).map_or(lines.len(), |i| start + i);
    Some((start, end))
}

fn with_value(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let new_line = format!("{key} = {value}");
    match section_body(&lines, section) {
        Some((start, end)) => {
            let existing = (start..end).find(|&i| {
                let line = lines[i].trim_start();
                !line.starts_with('#')
                    && line.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
            });
            match existing {
                Some(i) => lines[i] = new_line,
                None if section.is_empty() => {
                    let at = (start..end).rev().find(|&i| !lines[i].trim().is_empty()).map_or(start, |i| i + 1);
                    lines.insert(at, new_line);
                }
                None => lines.insert(start, new_line),
            }
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(new_line);
        }
    }
    lines.join("\n") + "\n"
}

fn without_section(text: &str, section: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if section.is_empty() {
        return text.to_string();
    }
    if let Some((start, end)) = section_body(&lines, section) {
        let mut from = start - 1;
        while from > 0 && lines[from - 1].trim().is_empty() {
            from -= 1;
        }
        lines.drain(from..end);
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let c = parse(&format!("{DEFAULT_BASIC}{DEFAULT_AUTO}"));
        assert_eq!(c.presets.len(), 4);
        assert!(c.auto_enabled);
        assert_eq!((c.schedule.day_start, c.schedule.night_start), (420, 1200));
        assert_eq!((c.schedule.day_brightness, c.schedule.night_brightness), (85, 65));
        assert_eq!(c.schedule.night_light, 5500);
        assert_eq!(c.games.values, [Some(100), Some(65), None]);
        assert!(c.games.not_games.contains(&"chrome.exe".to_string()));
        assert!(c.apps.is_empty());
    }

    #[test]
    fn app_rules() {
        let c = parse("[app Valorant.exe]\ngame = true\n[app vlc.exe]\nbrightness = 70%\n");
        assert_eq!(c.app_is_game("valorant.exe"), Some(true));
        assert_eq!(c.apps[1].values[0], Some(70));
    }

    #[test]
    fn games_night_light() {
        let c = parse(&format!("{DEFAULT_BASIC}{DEFAULT_AUTO}"));
        assert_eq!(c.games.night_light, 6000);
        assert_eq!(parse("[games]\nnight_light = 6200\n").games.night_light, 6200);
    }

    #[test]
    fn editing_keeps_the_rest() {
        let text = "# top\nstep = 10\n\n[auto]\nenabled = true\n# note\nday_brightness = 85\n\n[app a.exe]\ngame = true\n";
        let t = with_value(text, "auto", "day_brightness", "90");
        assert!(t.contains("day_brightness = 90") && !t.contains("= 85") && t.contains("# note"));
        let t = with_value(&t, "", "step", "15");
        assert!(t.starts_with("# top\nstep = 15\n"));
        let t = with_value(&t, "app b.exe", "game", "false");
        assert!(t.ends_with("[app b.exe]\ngame = false\n"));
        let t = without_section(&t, "app a.exe");
        assert!(!t.contains("a.exe") && t.contains("[app b.exe]") && t.contains("day_brightness = 90"));
        let c = parse(&t);
        assert_eq!((c.step, c.schedule.day_brightness, c.app_is_game("b.exe")), (15, 90, Some(false)));
    }
}
