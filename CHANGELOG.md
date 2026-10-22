# Changelog

## 0.3.0
- G522 headset battery now uses the real Logitech Centurion protocol
  (HeadsetControl 4.0.0): 64-byte frames with report ID 0x50 on the FFA0
  collection (PID 0B18, interface 3), bridge lookup, battery feature 0x0104
  lookup (cached per session) and battery read. Charging (state 1/2) is
  shown blue like the mouse; headset off shows grey `H --`.
- probe.txt: "Headset probe" replaced by "Centurion probe" (every TX/RX
  frame, parsed indices, percent/state, exact error).
- Removed the PRO X 2 guess (report 0x51, FF13 collection, variants A/B/C,
  HidD_SetOutputReport fallback) and its tests.
- Mouse path unchanged.

## 0.2.0
- Headset probe for Logitech FF13/FFA0 collections, HID caps logging.

## 0.1.0
- Taskbar battery strip for Logitech mouse/headset over HID++ 2.0.
