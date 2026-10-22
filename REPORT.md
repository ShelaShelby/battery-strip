# Report

Built without Windows or Logitech hardware. Hardware/OS behaviour is written
from documented HID++ 2.0 / Win32 knowledge and checked only by the Windows CI.

## Verified
- Windows CI (windows-latest): `cargo test --release`, `cargo build --release`,
  release `latest` republished with `battery-strip.exe` at the head commit.
- Unit tests (35): HID++ frame building, reply/error matching (incl. ignoring
  G HUB traffic), feature-table lookup, battery decoding for 0x1004, 0x1000,
  0x1001, 0x1F20, device classification, colour thresholds, text format,
  taskbar placement maths (tray, fallback, multi-monitor, vertical, auto-hide,
  DPI), settings round-trip, log rotation, PNG encoder, renderer invariants,
  and a full scan/poll against an emulated receiver (mouse + headset).
- CI runs `--preview` (same renderer + placement code, real Segoe UI) and
  uploads `preview-light.png` / `preview-dark.png` as artifacts.
- CI runs `--probe --no-dialog` on the runner: HID enumeration, open and
  probe.txt writing run without crashing (no Logitech devices there).

## Not verified (needs the real PC)
- Mouse readout: G PRO 2 LIGHTSPEED is expected to answer on device index
  0x01..0x06 of the receiver's 0xFF00 long-report collection with
  UNIFIED_BATTERY; not tested on hardware.
- Headset readout (G522): UNCONFIRMED. Third-party access to its battery is
  undocumented; all interfaces (usage page >= 0xFF00) and indices (0xFF,
  1-6) are tried. If nothing answers the strip shows grey "H --".
- ADC_MEASUREMENT (0x1F20) flag meaning (bit0 valid, bit1 charging) and the
  voltage-to-% curve are taken from public reverse-engineering (Solaar).
- Taskbar placement on real Windows 11: TrayNotifyWnd rect on the primary
  taskbar, Shell_SecondaryTrayWnd fallback offset (default 140 logical px,
  configurable), staying above the taskbar after clicks, fullscreen hiding.
- Co-existence with a running G HUB (shared opens, rotating software IDs
  0xA-0xF, feature index cross-check) is reasoned, not observed.
- Tray menu, autostart key, Explorer restart (TaskbarCreated) handling.

## Next step
Run `battery-strip.exe --probe` and send probe.txt.
