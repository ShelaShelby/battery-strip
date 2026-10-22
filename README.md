# Battery Strip

A thin, always-visible strip on the Windows 11 taskbar of every monitor that
shows the battery of two Logitech devices:

```
M 85%      <- mouse   (Logitech G PRO 2 LIGHTSPEED)
H 60%      <- headset (Logitech G522 LIGHTSPEED)
```

* Green above 50 %, yellow 20–50 %, red below 20 %, blue while charging,
  grey (`--`) when disconnected / asleep / unreadable. A number is only ever
  shown when the device reported it. `~50%` means the device reported only a
  coarse level.
* Placed just left of the notification area (secondary taskbars: at a
  configurable offset from the right end), sized to the taskbar, per-monitor
  DPI aware, click-through, hidden while a fullscreen app is in front.
* Reads the battery over HID++ 2.0 (UNIFIED_BATTERY 0x1004, then
  BATTERY_STATUS 0x1000, BATTERY_VOLTAGE 0x1001, ADC_MEASUREMENT 0x1F20).
  Devices are opened in shared mode and only read-only queries are sent, so
  Logitech G HUB keeps working alongside.
* Polls every 60 s (and at start). Tray icon menu: strip per monitor,
  mouse / headset on/off, start with Windows, refresh, run probe, open log
  folder, exit. No admin rights needed.

## Download and run

1. Download `battery-strip.exe` from the
   [latest release](../../releases/tag/latest).
2. The exe is unsigned. If Windows SmartScreen says "Windows protected your
   PC", click **More info** and then **Run anyway**.
3. Double-click it. The strip appears on the taskbar and a battery icon in the
   tray (right-click it for the menu).

## First test: probe mode (please do this first)

Open a command prompt in the folder with the exe and run:

```
battery-strip.exe --probe
```

After a few seconds a dialog says where `probe.txt` was written (next to the
exe; if that folder is not writable, `%APPDATA%\battery-strip\probe.txt`).
Please send that file. It lists every Logitech HID interface (VID/PID, usage
page, usage, interface, path), every query sent and every reply in hex, the
feature table of each device that answered, and the battery result or the
reason it failed. It contains nothing else. You can also start the probe from
the tray menu ("Run probe").

## Files

* `%APPDATA%\battery-strip\settings.ini` – settings (`show_mouse`,
  `show_headset`, `hidden_monitors`, `fallback_offset_x/y` in logical px).
* `%APPDATA%\battery-strip\log.txt` – short log (max 200 KB, then rotated to
  `log.txt.old`).
* Start with Windows uses `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

## Building

```
cargo test
cargo build --release
battery-strip.exe --preview out   # software-rendered preview PNGs
```

CI (GitHub Actions, windows-latest) runs the tests, builds the release exe,
renders light/dark preview PNGs (artifact `battery-strip`), runs a smoke
probe, and republishes the `latest` release with `battery-strip.exe`.

See [REPORT.md](REPORT.md) for what is verified and what is not.
