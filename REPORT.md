# Report (0.3.0)

Built without Windows or Logitech hardware; checked by unit tests and the
Windows CI only.

## Verified
- Windows CI: `cargo test --release`, `cargo build --release`, preview PNGs,
  smoke `--probe` on the runner, release `latest` republished at head.
- Protocol taken from HeadsetControl 4.0.0
  (`logitech_centurion_protocol.hpp`, `logitech_g522_lightspeed.hpp`).
- Unit tests: the worked vectors a) to f) (direct Root lookup frame, bridge
  Root lookup of 0x0104, bridge battery read, answer parse 85 % charging,
  ACK / disconnect / rejected, bad prefix / LEN 0, 1 / LEN too long), percent
  0..100 and state 1/2, at most 8 frames per request, timeout and write
  error text, index 0 = not found, cached indices (later polls send only the
  battery read), offline keeps the cache, any other error clears it, only
  the G522 FFA0 interface 3 gets Centurion frames, FF13/FFA0 never get HID++.
- Windows code type-checked and clippy-clean for x86_64-pc-windows-msvc.

## Still unverified (needs the real PC)
- The G522 replies themselves (real bridge_index, battery_index, percent).

## Behaviour
- Mouse first, then one short Centurion session (open, query, close),
  shared mode so G HUB stays open. 1000 ms per read, at most 8 frames.
- Headset off (disconnect event): grey "H --", not logged as an error.

## Next step
Run "Run probe" from the tray (or `--probe`) and send probe.txt; the
"Centurion probe" section shows every frame.
