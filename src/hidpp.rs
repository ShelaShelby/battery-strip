//! HID++ 2.0 frame building, reply matching and battery decoding.
//! Pure code (no OS calls) so it is unit-tested on any platform.
//! All multi-byte values are big-endian on the wire (decoded explicitly).

pub const VID_LOGITECH: u16 = 0x046D;
pub const REPORT_SHORT: u8 = 0x10;
pub const REPORT_LONG: u8 = 0x11;
pub const SHORT_LEN: usize = 7;
pub const LONG_LEN: usize = 20;
pub const PARAMS_LEN: usize = LONG_LEN - 4;

/// Order in which device indices are tried on every interface: 0xFF for a
/// directly connected device (or the receiver itself), 1..6 for devices
/// attached to a receiver.
pub const DEVICE_INDICES: [u8; 7] = [0xFF, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

pub mod feature {
    pub const ROOT: u16 = 0x0000;
    pub const FEATURE_SET: u16 = 0x0001;
    pub const DEVICE_NAME: u16 = 0x0005;
    pub const BATTERY_STATUS: u16 = 0x1000;
    pub const BATTERY_VOLTAGE: u16 = 0x1001;
    pub const UNIFIED_BATTERY: u16 = 0x1004;
    pub const ADC_MEASUREMENT: u16 = 0x1F20;
}

/// Battery features in the order they are tried.
pub const BATTERY_FEATURES: [u16; 4] = [
    feature::UNIFIED_BATTERY,
    feature::BATTERY_STATUS,
    feature::BATTERY_VOLTAGE,
    feature::ADC_MEASUREMENT,
];

pub fn feature_name(id: u16) -> &'static str {
    match id {
        0x0000 => "ROOT",
        0x0001 => "FEATURE_SET",
        0x0002 => "FEATURE_INFO",
        0x0003 => "DEVICE_FW_VERSION",
        0x0005 => "DEVICE_NAME",
        0x0007 => "DEVICE_FRIENDLY_NAME",
        0x0020 => "CONFIG_CHANGE",
        0x1000 => "BATTERY_STATUS",
        0x1001 => "BATTERY_VOLTAGE",
        0x1004 => "UNIFIED_BATTERY",
        0x1D4B => "WIRELESS_DEVICE_STATUS",
        0x1F20 => "ADC_MEASUREMENT",
        0x8060 => "REPORT_RATE",
        0x8061 => "EXTENDED_REPORT_RATE",
        0x8070 => "COLOR_LED_EFFECTS",
        0x8071 => "RGB_EFFECTS",
        0x8100 => "ONBOARD_PROFILES",
        0x8110 => "MOUSE_BUTTON_SPY",
        0x2201 => "ADJUSTABLE_DPI",
        _ => "",
    }
}

/// Build a long (0x11) HID++ 2.0 request.
pub fn build_long(dev: u8, feat_idx: u8, func: u8, sw_id: u8, params: &[u8]) -> [u8; LONG_LEN] {
    let mut f = [0u8; LONG_LEN];
    f[0] = REPORT_LONG;
    f[1] = dev;
    f[2] = feat_idx;
    f[3] = ((func & 0x0F) << 4) | (sw_id & 0x0F);
    let n = params.len().min(PARAMS_LEN);
    f[4..4 + n].copy_from_slice(&params[..n]);
    f
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Successful answer: the 16 parameter bytes.
    Ok([u8; PARAMS_LEN]),
    /// HID++ 2.0 error (feature index 0xFF in a long report).
    Err20(u8),
    /// HID++ 1.0 error (sub-id 0x8F in a short report), e.g. from the
    /// receiver when no device is paired / awake at that index.
    Err10(u8),
}

/// Decide whether `report` (as read from the device, starting with the
/// report id) answers `req`. Returns None for unrelated traffic.
pub fn match_reply(req: &[u8; LONG_LEN], report: &[u8]) -> Option<Reply> {
    if report.len() < 2 || report[1] != req[1] {
        return None;
    }
    match report[0] {
        REPORT_LONG if report.len() >= LONG_LEN => {
            if report[2] == req[2] && report[3] == req[3] {
                let mut p = [0u8; PARAMS_LEN];
                p.copy_from_slice(&report[4..LONG_LEN]);
                Some(Reply::Ok(p))
            } else if report[2] == 0xFF && report[3] == req[2] && report[4] == req[3] {
                Some(Reply::Err20(report[5]))
            } else {
                None
            }
        }
        REPORT_SHORT if report.len() >= SHORT_LEN => {
            if report[2] == 0x8F && report[3] == req[2] && report[4] == req[3] {
                Some(Reply::Err10(report[5]))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn err20_name(code: u8) -> &'static str {
    match code {
        0 => "no error",
        1 => "unknown",
        2 => "invalid argument",
        3 => "out of range",
        4 => "hardware error",
        5 => "logitech internal",
        6 => "invalid feature index",
        7 => "invalid function id",
        8 => "busy",
        9 => "unsupported",
        _ => "?",
    }
}

pub fn err10_name(code: u8) -> &'static str {
    match code {
        0x01 => "invalid sub-id (HID++ 1.0 only device/receiver)",
        0x02 => "invalid address",
        0x03 => "invalid value",
        0x04 => "connection failed",
        0x05 => "too many devices",
        0x06 => "already exists",
        0x07 => "busy",
        0x08 => "unknown device",
        0x09 => "resource error (device not connected/asleep)",
        0x0A => "request unavailable",
        0x0B => "invalid parameter value",
        0x0C => "wrong PIN code",
        _ => "?",
    }
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{:02X}", b));
    }
    s
}

/// Parameters of ROOT.GetFeature(featureId).
pub fn get_feature_params(id: u16) -> [u8; 2] {
    id.to_be_bytes()
}

/// ROOT.GetFeature reply -> feature index (None when not supported).
pub fn parse_get_feature(p: &[u8; PARAMS_LEN]) -> Option<u8> {
    if p[0] == 0 {
        None
    } else {
        Some(p[0])
    }
}

/// FEATURE_SET.GetFeatureID reply -> feature id.
pub fn parse_feature_id(p: &[u8; PARAMS_LEN]) -> u16 {
    u16::from_be_bytes([p[0], p[1]])
}

/// Look up the index of a feature id in a (index, id) table.
pub fn lookup(table: &[(u8, u16)], id: u16) -> Option<u8> {
    table.iter().find(|(_, f)| *f == id).map(|(i, _)| *i)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Headset,
    Other,
    Unknown,
}

pub fn device_type_name(t: u8) -> &'static str {
    match t {
        0 => "keyboard",
        1 => "remote control",
        2 => "numpad",
        3 => "mouse",
        4 => "touchpad",
        5 => "trackball",
        6 => "presenter",
        7 => "receiver",
        8 => "headset",
        9 => "webcam",
        10 => "steering wheel",
        11 => "joystick",
        12 => "gamepad",
        13 => "dock",
        14 => "speaker",
        15 => "microphone",
        16 => "illumination light",
        17 => "programmable controller",
        18 => "car sim pedals",
        19 => "adapter",
        _ => "?",
    }
}

/// Classify a device from its DEVICE_NAME type (preferred) or its name.
pub fn classify(dev_type: Option<u8>, name: &str) -> Kind {
    match dev_type {
        Some(3) | Some(4) | Some(5) => return Kind::Mouse,
        Some(8) | Some(14) | Some(15) => return Kind::Headset,
        Some(_) => return Kind::Other,
        None => {}
    }
    let n = name.to_ascii_lowercase();
    if n.contains("g522") || n.contains("headset") {
        Kind::Headset
    } else if n.contains("mouse") || n.contains("pro 2") || n.contains("superlight") {
        Kind::Mouse
    } else {
        Kind::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    /// True when only a coarse level was reported (shown with "~").
    pub approx: bool,
    pub charging: bool,
}

/// Approximate Li-ion discharge curve (mV, %), same table Solaar uses.
const VOLTAGE_CURVE: [(u16, u8); 13] = [
    (4186, 100),
    (4067, 90),
    (3989, 80),
    (3922, 70),
    (3859, 60),
    (3811, 50),
    (3778, 40),
    (3751, 30),
    (3717, 20),
    (3671, 10),
    (3646, 5),
    (3579, 2),
    (3500, 0),
];

/// Estimate percentage from a cell voltage in mV (integer interpolation).
pub fn voltage_to_percent(mv: u16) -> u8 {
    if mv >= VOLTAGE_CURVE[0].0 {
        return 100;
    }
    for w in VOLTAGE_CURVE.windows(2) {
        let (hv, hp) = w[0];
        let (lv, lp) = w[1];
        if mv >= lv {
            let span_v = (hv - lv) as u32;
            let span_p = (hp - lp) as u32;
            let d = (mv - lv) as u32;
            return lp + ((d * span_p + span_v / 2) / span_v) as u8;
        }
    }
    0
}

/// UNIFIED_BATTERY (0x1004) get_status (func 1), with optional
/// get_capabilities (func 0) reply.
pub fn decode_unified(status: &[u8; PARAMS_LEN], caps: Option<&[u8; PARAMS_LEN]>) -> Result<Battery, String> {
    let soc = status[0];
    let level = status[1];
    let charge = status[2];
    let charging = matches!(charge, 1..=3);
    let soc_supported = match caps {
        Some(c) => c[1] & 0x02 != 0,
        None => soc > 0,
    };
    if soc_supported {
        if soc > 100 {
            return Err(format!("state of charge out of range ({})", soc));
        }
        return Ok(Battery {
            percent: soc,
            approx: false,
            charging,
        });
    }
    let percent = if level & 0x08 != 0 {
        100
    } else if level & 0x04 != 0 {
        50
    } else if level & 0x02 != 0 {
        20
    } else if level & 0x01 != 0 {
        5
    } else {
        return Err("no state of charge and no level reported".into());
    };
    Ok(Battery {
        percent,
        approx: true,
        charging,
    })
}

/// BATTERY_STATUS (0x1000) GetBatteryLevelStatus (func 0).
pub fn decode_battery_status(p: &[u8; PARAMS_LEN]) -> Result<Battery, String> {
    let level = p[0];
    let status = p[2];
    let charging = matches!(status, 1..=4);
    match status {
        5 => return Err("battery status: invalid battery".into()),
        6 => return Err("battery status: thermal error".into()),
        7 => return Err("battery status: other error".into()),
        s if s > 7 => return Err(format!("battery status: unknown status {}", s)),
        _ => {}
    }
    if level > 100 {
        return Err(format!("battery level out of range ({})", level));
    }
    if level == 0 {
        if status == 3 {
            return Ok(Battery {
                percent: 100,
                approx: false,
                charging,
            });
        }
        return Err("battery level not reported (0)".into());
    }
    Ok(Battery {
        percent: level,
        approx: false,
        charging,
    })
}

fn plausible_mv(mv: u16) -> Result<(), String> {
    if (2500..=5000).contains(&mv) {
        Ok(())
    } else {
        Err(format!("implausible voltage {} mV", mv))
    }
}

/// BATTERY_VOLTAGE (0x1001) GetBatteryInfo (func 0): voltage mV (BE), flags.
pub fn decode_voltage(p: &[u8; PARAMS_LEN]) -> Result<Battery, String> {
    let mv = u16::from_be_bytes([p[0], p[1]]);
    plausible_mv(mv)?;
    let charging = p[2] & 0x80 != 0;
    Ok(Battery {
        percent: voltage_to_percent(mv),
        approx: false,
        charging,
    })
}

/// ADC_MEASUREMENT (0x1F20) func 0: voltage mV (BE), flags
/// (bit0 = measurement valid / connected, bit1 = charging).
pub fn decode_adc(p: &[u8; PARAMS_LEN]) -> Result<Battery, String> {
    let mv = u16::from_be_bytes([p[0], p[1]]);
    let flags = p[2];
    if flags & 0x01 == 0 {
        return Err(format!("ADC measurement flagged invalid (flags 0x{:02X})", flags));
    }
    plausible_mv(mv)?;
    Ok(Battery {
        percent: voltage_to_percent(mv),
        approx: false,
        charging: flags & 0x02 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(b: &[u8]) -> [u8; PARAMS_LEN] {
        let mut p = [0u8; PARAMS_LEN];
        p[..b.len()].copy_from_slice(b);
        p
    }

    #[test]
    fn builds_get_feature_request() {
        let f = build_long(0x01, 0x00, 0x0, 0x0A, &get_feature_params(0x1004));
        assert_eq!(f.len(), 20);
        assert_eq!(&f[..7], &[0x11, 0x01, 0x00, 0x0A, 0x10, 0x04, 0x00]);
        assert!(f[7..].iter().all(|&b| b == 0));
    }

    #[test]
    fn builds_function_nibbles() {
        let f = build_long(0xFF, 0x06, 0x1, 0x0F, &[]);
        assert_eq!(&f[..4], &[0x11, 0xFF, 0x06, 0x1F]);
        // over-long params are truncated, never overflow
        let f = build_long(1, 2, 3, 4, &[0xAA; 30]);
        assert_eq!(f[19], 0xAA);
        assert_eq!(f[3], 0x34);
    }

    #[test]
    fn matches_success_reply() {
        let req = build_long(0x01, 0x00, 0, 0x0A, &[0x10, 0x04]);
        let mut rep = [0u8; 20];
        rep[..7].copy_from_slice(&[0x11, 0x01, 0x00, 0x0A, 0x06, 0x00, 0x02]);
        let r = match_reply(&req, &rep).unwrap();
        match r {
            Reply::Ok(p) => assert_eq!(parse_get_feature(&p), Some(0x06)),
            _ => panic!(),
        }
    }

    #[test]
    fn matches_errors_and_ignores_unrelated() {
        let req = build_long(0x02, 0x00, 1, 0x0A, &[0, 0, 0x5A]);
        let mut e20 = [0u8; 20];
        e20[..6].copy_from_slice(&[0x11, 0x02, 0xFF, 0x00, 0x1A, 0x09]);
        assert_eq!(match_reply(&req, &e20), Some(Reply::Err20(9)));
        let e10 = [0x10, 0x02, 0x8F, 0x00, 0x1A, 0x09, 0x00];
        assert_eq!(match_reply(&req, &e10), Some(Reply::Err10(9)));
        // other device index
        let other = [0x10, 0x03, 0x8F, 0x00, 0x1A, 0x09, 0x00];
        assert_eq!(match_reply(&req, &other), None);
        // other software id (e.g. G HUB traffic)
        let mut gh = [0u8; 20];
        gh[..4].copy_from_slice(&[0x11, 0x02, 0x00, 0x11]);
        assert_eq!(match_reply(&req, &gh), None);
        // truncated
        assert_eq!(match_reply(&req, &[0x11, 0x02, 0x00]), None);
        assert_eq!(match_reply(&req, &[]), None);
    }

    #[test]
    fn feature_table_lookup() {
        let table = [(0x00, 0x0000), (0x01, 0x0001), (0x02, 0x0005), (0x07, 0x1004), (0x08, 0x1F20)];
        assert_eq!(lookup(&table, feature::UNIFIED_BATTERY), Some(0x07));
        assert_eq!(lookup(&table, feature::ADC_MEASUREMENT), Some(0x08));
        assert_eq!(lookup(&table, feature::BATTERY_STATUS), None);
        assert_eq!(parse_feature_id(&params(&[0x10, 0x04, 0x00, 0x02])), 0x1004);
        assert_eq!(parse_get_feature(&params(&[0, 0, 0])), None);
    }

    #[test]
    fn unified_battery_decoding() {
        // 85 %, level good, discharging; caps: levels 0x0F, flags rechargeable+soc
        let caps = params(&[0x0F, 0x03]);
        let b = decode_unified(&params(&[85, 0x04, 0x00, 0x00]), Some(&caps)).unwrap();
        assert_eq!(
            b,
            Battery {
                percent: 85,
                approx: false,
                charging: false
            }
        );
        let b = decode_unified(&params(&[42, 0x04, 0x01, 0x01]), Some(&caps)).unwrap();
        assert!(b.charging && b.percent == 42);
        // no SoC support -> level only (approximate)
        let caps = params(&[0x0F, 0x01]);
        let b = decode_unified(&params(&[0, 0x02, 0x00, 0x00]), Some(&caps)).unwrap();
        assert_eq!(
            b,
            Battery {
                percent: 20,
                approx: true,
                charging: false
            }
        );
        assert!(decode_unified(&params(&[0, 0, 0, 0]), Some(&caps)).is_err());
        assert!(decode_unified(&params(&[150, 0, 0, 0]), None).is_err());
        // no caps: non-zero soc trusted
        assert_eq!(decode_unified(&params(&[60, 4, 0, 0]), None).unwrap().percent, 60);
    }

    #[test]
    fn battery_status_decoding() {
        let b = decode_battery_status(&params(&[50, 20, 0])).unwrap();
        assert_eq!(
            b,
            Battery {
                percent: 50,
                approx: false,
                charging: false
            }
        );
        assert!(decode_battery_status(&params(&[90, 50, 1])).unwrap().charging);
        assert_eq!(decode_battery_status(&params(&[0, 0, 3])).unwrap().percent, 100);
        assert!(decode_battery_status(&params(&[0, 0, 0])).is_err());
        assert!(decode_battery_status(&params(&[50, 0, 6])).is_err());
        assert!(decode_battery_status(&params(&[101, 0, 0])).is_err());
    }

    #[test]
    fn voltage_decoding() {
        // 0x0F 0xA0 = 4000 mV -> between 80 (3989) and 90 (4067)
        let b = decode_voltage(&params(&[0x0F, 0xA0, 0x00])).unwrap();
        assert_eq!(b.percent, 81);
        assert!(!b.charging);
        let b = decode_voltage(&params(&[0x10, 0x5A, 0x80])).unwrap(); // 4186
        assert_eq!(b.percent, 100);
        assert!(b.charging);
        assert!(decode_voltage(&params(&[0, 0, 0])).is_err());
        assert_eq!(voltage_to_percent(3500), 0);
        assert_eq!(voltage_to_percent(3000), 0);
        assert_eq!(voltage_to_percent(3811), 50);
        assert_eq!(voltage_to_percent(5000), 100);
        // monotonic
        let mut last = 0;
        for mv in 3400..4300u16 {
            let p = voltage_to_percent(mv);
            assert!(p >= last);
            last = p;
        }
    }

    #[test]
    fn adc_decoding() {
        let b = decode_adc(&params(&[0x0E, 0xE3, 0x01])).unwrap(); // 3811 mV
        assert_eq!(
            b,
            Battery {
                percent: 50,
                approx: false,
                charging: false
            }
        );
        assert!(decode_adc(&params(&[0x0E, 0xE3, 0x03])).unwrap().charging);
        assert!(decode_adc(&params(&[0x0E, 0xE3, 0x00])).is_err());
        assert!(decode_adc(&params(&[0x00, 0x00, 0x01])).is_err());
    }

    #[test]
    fn classification() {
        assert_eq!(classify(Some(3), ""), Kind::Mouse);
        assert_eq!(classify(Some(8), "x"), Kind::Headset);
        assert_eq!(classify(Some(0), "G522"), Kind::Other);
        assert_eq!(classify(None, "G522 LIGHTSPEED"), Kind::Headset);
        assert_eq!(classify(None, "PRO 2 LIGHTSPEED"), Kind::Mouse);
        assert_eq!(classify(None, ""), Kind::Unknown);
    }

    #[test]
    fn hex_format() {
        assert_eq!(hex(&[0x11, 0x01, 0xAB]), "11 01 AB");
        assert_eq!(hex(&[]), "");
    }
}
