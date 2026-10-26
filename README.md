# Grovekeeper

A tiny, fast Windows tray toolkit for the small things you do on your PC every day.
It starts with full control of your external monitors over DDC/CI, and grows from there.

Grovekeeper is written in Rust against the plain Windows API: no browser engine, no .NET
runtime, no background polling. It sits in the tray using a few megabytes of memory and
effectively zero CPU and GPU, so it stays out of the way of your games.

## Features

**Monitor control** (any monitor with DDC/CI turned on)

- Brightness, contrast and volume sliders for every monitor
- One "All monitors" brightness slider when you have more than one screen
- Switch input: HDMI 1/2, DisplayPort 1/2, USB-C, DVI, VGA
- Turn a single monitor off or on, or put every screen to sleep
- Presets (Day, Evening, Night, Gaming — or your own) from the tray menu
- Scroll the mouse wheel over any slider to adjust it in 5% steps

**Hotkeys**

| Keys                | Action                        |
| ------------------- | ----------------------------- |
| `Ctrl + Alt + Up`   | Brightness up, all monitors   |
| `Ctrl + Alt + Down` | Brightness down, all monitors |
| `Ctrl + Alt + End`  | Turn all screens off          |

**Everything else**

- Start with Windows (tray menu toggle, no admin rights needed)
- Sharp at any display scaling (per-monitor DPI aware)
- Picks up monitor changes on its own after plugging in a screen or waking from sleep

## Using it

- **Left-click** the leaf in the tray to open the panel. Click outside it or press `Esc` to close.
- **Right-click** the leaf for presets, input switching, power, settings and Exit.

If a monitor doesn't show up or shows "no DDC/CI", open the monitor's own on-screen menu
and turn on **DDC/CI** (sometimes under "Other" or "System"), then choose
**Refresh monitors** from the tray menu. Laptop built-in screens are not supported yet.

## Settings

Choose **Edit settings…** in the tray menu. It opens `%APPDATA%\Grovekeeper\config.ini`:

```ini
# How much the brightness hotkeys change brightness, in percent.
step = 10

[preset Day]
brightness = 80
contrast = 60

[preset Movie]
brightness = 60
contrast = 70
volume = 30
```

Add, rename or remove `[preset ...]` sections as you like. Changes apply the next time you
open the panel or the menu.

## Building

Install Rust from [rustup.rs](https://rustup.rs), then:

```bash
cargo build --release
```

The app is `target/release/grovekeeper.exe` — a single file you can put anywhere.

## Why it's light

- Native Win32 window drawn with GDI, only while the panel is open
- No timers or polling while idle: it wakes up only for your clicks, hotkeys and display changes
- All DDC/CI traffic runs on one background thread, and slider drags are merged so a fast
  drag never queues up slow monitor writes

## Roadmap

Grovekeeper is meant to become a home for small everyday PC tools. Ideas for next modules:

- Laptop built-in screen brightness
- Night light / colour temperature schedule
- Audio output switcher
- Keep-awake toggle
- Per-game profiles

## License

[MIT](LICENSE)
