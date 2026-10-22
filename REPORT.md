# Report (0.2.0)

Built without Windows or Logitech hardware; checked by unit tests and the
Windows CI only.

## Verified
- Windows CI: `cargo test --release`, `cargo build --release`, preview PNGs,
  smoke `--probe` on the runner, release `latest` republished at head.
- Mouse (user's probe.txt, 0.1.0): receiver C543, device index 0x01,
  UNIFIED_BATTERY at feature index 0x07. This path is unchanged.
- G522 (user's probe.txt): PID 0B18, interface 3, collections FF13 (in/out
  62) and FFA0 (in/out 64) open shared read/write. The old 20-byte HID++
  write failed with Win32 error 87; the headset was never queried.
- Unit tests (50): packet builder for variants A/B/C at exact lengths 62 and
  64, signature decoder (valid, wrong prefix/type/marker, percent 0/101/255
  rejected, 1/100 accepted, charging flag), ACK-then-data, at most 4 reads,
  variant fallback order and stop on first reply, WriteFile ->
  HidD_SetOutputReport fallback, per-variant error text, silent-device
  timeouts, remembered variant in polling, FF13/FFA0 never get HID++.
  The example bytes in the tests are marked "unverified on G522".
- Windows code type-checked locally against windows-sys 0.59.

## Still unverified (needs the real PC)
- Whether the G522 answers this request at all, on FF13 or FFA0, and with
  which variant. The protocol comes from HeadsetControl PR #478 (PRO X 2).
- Reply layout (byte 10 percent, byte 12 = 02 charging) on the G522.
- Report IDs of the G522 collections (now logged from HidP caps).
- Whether HidD_SetOutputReport behaves differently from WriteFile there.

## Behaviour
- Headset value is shown only for a matching reply; else grey "H --".
- Polling: mouse is read and published first; the headset request runs
  afterwards with 700 ms read timeouts (1 s write timeout), so a silent
  headset never delays the mouse. 60 s interval unchanged.

## Next step
Run "Run probe" from the tray (or `--probe`) and send probe.txt; the
"Headset probe" section shows the raw replies.
