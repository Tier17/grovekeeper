//! Settings live in %APPDATA%\Grovekeeper\config.ini, a plain text file the user edits.

use std::fs;
use std::path::PathBuf;

const DEFAULT: &str = "\
# Grovekeeper settings
# Lines starting with # are comments. Save the file and the changes apply
# the next time you open the panel or the tray menu.

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

pub struct Preset {
    pub name: String,
    /// Indexed like `ddc::FEATURES`, in percent.
    pub values: [Option<u32>; 3],
}

pub struct Config {
    pub step: u32,
    pub presets: Vec<Preset>,
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
        .join("Grovekeeper")
        .join("config.ini")
}

pub fn ensure_exists() {
    let path = path();
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&path, DEFAULT);
    }
}

impl Config {
    pub fn load() -> Config {
        ensure_exists();
        let text = fs::read_to_string(path()).unwrap_or_else(|_| DEFAULT.to_string());
        parse(&text)
    }
}

fn parse(text: &str) -> Config {
    let mut config = Config { step: 10, presets: Vec::new() };
    // None = top level, Some(None) = an unknown section, Some(Some(i)) = preset i.
    let mut section: Option<Option<usize>> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = inner.trim().strip_prefix("preset").map(str::trim).unwrap_or("");
            if name.is_empty() {
                section = Some(None);
            } else {
                config.presets.push(Preset { name: name.to_string(), values: [None; 3] });
                section = Some(Some(config.presets.len() - 1));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let Ok(value) = value.trim().trim_end_matches('%').trim().parse::<u32>() else { continue };

        match section {
            None if key == "step" => config.step = value.clamp(1, 50),
            Some(Some(i)) => {
                let feature = match key.as_str() {
                    "brightness" => 0,
                    "contrast" => 1,
                    "volume" => 2,
                    _ => continue,
                };
                config.presets[i].values[feature] = Some(value.min(100));
            }
            _ => {}
        }
    }
    config
}
