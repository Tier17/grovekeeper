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

**Auto mode** (on by default, toggle it from the tray menu)

- Day and night brightness on a schedule: 85% by day, 65% at night, fading over 30 minutes
- A slight warm night light at night (5500 K), applied by the graphics card at zero cost
- Games get their own look: any fullscreen app counts as a game and gets 100% brightness,
  65% contrast and a lighter night light — only on the monitor the game is on
- When you leave the game, everything goes back to the schedule (and your old contrast)
- Mark any app as a game, or never a game, from the tray menu ("Treat app.exe as a game"),
  or give an app its own brightness, contrast and volume in the settings file
- Moving a slider or using a hotkey always wins until the next switch, so auto mode never
  fights you
- Reacts the moment you switch apps (Windows tells it); otherwise it only wakes once a minute

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

Choose **Settings…** in the tray menu, or start `grovekeeper.exe` again while it's running.
The settings window has switches, sliders and steppers for everything below; each change
is saved and applied immediately. Recently used apps can be added as games with one click.

Everything is stored in `%APPDATA%\Grovekeeper\config.ini`, which you can also edit by hand
(**Open settings file** in the settings window):

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

Auto mode lives in the same file:

```ini
[auto]
enabled = true
day_start = 07:00
night_start = 20:00
fade_minutes = 30
day_brightness = 85
night_brightness = 65
night_light = 5500        # kelvin; 6500 = off, lower = warmer

[games]
fullscreen_is_game = true
brightness = 100
contrast = 65
night_light = 6000        # lighter night light while gaming
not_games = chrome.exe, msedge.exe, firefox.exe, vlc.exe

[app valorant-win64-shipping.exe]
game = true

[app vlc.exe]
brightness = 70
```

The night light uses the display's gamma ramp, so it doesn't apply to HDR displays, and
it adds on top of Windows' own Night light if you have that switched on too.

## Building

Install Rust from [rustup.rs](https://rustup.rs), then:

```bash
cargo build --release
```

The app is `target/release/grovekeeper.exe` — a single file you can put anywhere.

## Why it's light

- Native Win32 window drawn with GDI, only while the panel is open
- No polling: it wakes up for your clicks, hotkeys, app switches and display changes, plus
  one tiny schedule check a minute
- All DDC/CI traffic runs on one background thread, and slider drags are merged so a fast
  drag never queues up slow monitor writes

## Roadmap

Grovekeeper is meant to become a home for small everyday PC tools. Ideas for next modules:

- Laptop built-in screen brightness
- Sunrise/sunset schedule from your location
- Audio output switcher
- Keep-awake toggle
- Power plan switching while gaming

## License

[MIT](LICENSE)
