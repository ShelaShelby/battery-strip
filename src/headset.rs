//! Logitech Centurion protocol (G522 LIGHTSPEED headset), as implemented by
//! HeadsetControl 4.0.0 (lib/devices/protocols/logitech_centurion_protocol.hpp,
//! lib/devices/logitech_g522_lightspeed.hpp).
//!
//! Frames are 64 bytes on the dongle's FFA0 collection: report ID 0x50,
//! 0x23, LEN, flags 0x00, then the payload. The battery lives on the headset
//! behind the dongle and is reached through the dongle's bridge feature.
//! Only read-only getters are ever sent.

use crate::hidpp::{hex, Battery, VID_LOGITECH};
use crate::scan::HidInfo;
use std::fmt;
use std::time::Duration;

pub const PID_G522: u16 = 0x0B18;
pub const REPORT_ID: u8 = 0x50;
pub const MARKER: u8 = 0x23;
pub const FRAME_LEN: usize = 64;
/// Largest payload that fits a frame (LEN + 3 <= 64).
pub const MAX_PAYLOAD: usize = FRAME_LEN - 4;
/// Frames read per request at most.
pub const MAX_FRAMES: usize = 8;
pub const READ_TIMEOUT: Duration = Duration::from_millis(1000);

const SOFTWARE_ID: u8 = 0x01;
const BRIDGE_SEND: u8 = 0x10;
const FEATURE_BRIDGE: u16 = 0x0003;
const FEATURE_BATTERY_SOC: u16 = 0x0104;

/// The G522 dongle collection that carries Centurion frames.
pub fn is_headset_iface(i: &HidInfo) -> bool {
    i.vid == VID_LOGITECH && i.pid == PID_G522 && i.usage_page == 0xFFA0 && i.usage == 0x0001 && i.interface == Some(3)
}

/// 64-byte frame carrying `payload` (longer payloads are cut to MAX_PAYLOAD).
pub fn build_frame(payload: &[u8]) -> [u8; FRAME_LEN] {
    let p = &payload[..payload.len().min(MAX_PAYLOAD)];
    let mut f = [0u8; FRAME_LEN];
    f[0] = REPORT_ID;
    f[1] = MARKER;
    f[2] = (p.len() + 1) as u8;
    f[3] = 0x00;
    f[4..4 + p.len()].copy_from_slice(p);
    f
}

/// Payload of a Centurion frame, None for anything else.
pub fn parse_frame(rep: &[u8]) -> Option<&[u8]> {
    if rep.len() < 3 || rep[0] != REPORT_ID || rep[1] != MARKER {
        return None;
    }
    let len = rep[2] as usize;
    if len <= 1 || len + 3 > FRAME_LEN || len + 3 > rep.len() {
        return None;
    }
    Some(&rep[4..3 + len])
}

/// Request to a dongle feature.
pub fn direct_payload(feature_index: u8, function: u8, params: &[u8]) -> Vec<u8> {
    let mut v = vec![feature_index, (function & 0xF0) | SOFTWARE_ID];
    v.extend_from_slice(params);
    v
}

/// Request to a headset feature, wrapped for the dongle's bridge.
pub fn bridge_payload(bridge_index: u8, sub_feature_index: u8, function: u8, params: &[u8]) -> Vec<u8> {
    let mut sub = vec![0x00, sub_feature_index, (function & 0xF0) | SOFTWARE_ID];
    sub.extend_from_slice(params);
    let size = sub.len();
    let mut v = vec![
        bridge_index,
        BRIDGE_SEND | SOFTWARE_ID,
        ((size >> 8) & 0x0F) as u8,
        (size & 0xFF) as u8,
    ];
    v.extend_from_slice(&sub);
    v
}

/// Data of a direct reply to `feature_index`.
pub fn direct_reply(payload: &[u8], feature_index: u8) -> Option<&[u8]> {
    if payload.len() >= 2 && payload[0] == feature_index {
        Some(&payload[2..])
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeReply<'a> {
    Ack,
    Connected,
    Offline,
    Rejected,
    /// Answer params (payload[7..]).
    Answer(&'a [u8]),
    Unrelated,
}

/// What a frame payload means for a bridge request to `sub_feature_index`.
pub fn bridge_reply(p: &[u8], bridge_index: u8, sub_feature_index: u8) -> BridgeReply<'_> {
    if p.len() < 2 || p[0] != bridge_index {
        return BridgeReply::Unrelated;
    }
    match p[1] {
        0x11 => BridgeReply::Ack,
        0x00 if p.len() >= 4 => match p[3] {
            0 => BridgeReply::Offline,
            1 => BridgeReply::Connected,
            _ => BridgeReply::Unrelated,
        },
        0x10 if p.len() >= 6 && p[4] == 0x00 => {
            if p[5] == sub_feature_index {
                BridgeReply::Answer(p.get(7..).unwrap_or(&[]))
            } else if p[5] == 0xFF {
                BridgeReply::Rejected
            } else {
                BridgeReply::Unrelated
            }
        }
        _ => BridgeReply::Unrelated,
    }
}

/// CenturionBatterySoc answer: [0] percent 0..=100, [2] state (1/2 charging).
pub fn parse_battery(params: &[u8]) -> Result<Battery, Error> {
    let Some(&p) = params.first() else {
        return Err(Error::Invalid("empty battery answer".into()));
    };
    if p > 100 {
        return Err(Error::Invalid(format!("battery percent {} out of range", p)));
    }
    let state = params.get(2).copied().unwrap_or(0);
    Ok(Battery {
        percent: p,
        approx: false,
        charging: state == 1 || state == 2,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Write(String),
    Read(String),
    Timeout,
    TooManyFrames,
    Rejected,
    Offline,
    BridgeNotFound,
    BatteryNotSupported,
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Write(e) => write!(f, "write error: {}", e),
            Error::Read(e) => write!(f, "read error: {}", e),
            Error::Timeout => write!(f, "timeout (no reply within {} ms)", READ_TIMEOUT.as_millis()),
            Error::TooManyFrames => write!(f, "no matching reply in {} frames", MAX_FRAMES),
            Error::Rejected => write!(f, "rejected by the headset (sub feature 0xFF)"),
            Error::Offline => write!(f, "headset off/disconnected"),
            Error::BridgeNotFound => write!(f, "bridge feature 0x{:04X} not found (index 0)", FEATURE_BRIDGE),
            Error::BatteryNotSupported => {
                write!(f, "battery feature 0x{:04X} not supported (index 0)", FEATURE_BATTERY_SOC)
            }
            Error::Invalid(e) => write!(f, "invalid reply: {}", e),
        }
    }
}

/// Raw access to the Centurion collection.
pub trait RawLink {
    /// One 64-byte output report.
    fn write(&mut self, buf: &[u8]) -> Result<(), String>;
    /// One input report, Ok(None) on timeout.
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String>;
}

/// Feature indices found by the two lookups (cached per session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indices {
    pub bridge: u8,
    pub battery: u8,
}

pub struct Session<'a, L: RawLink> {
    link: &'a mut L,
    log: &'a mut Vec<String>,
    /// Log every TX/RX frame (probe).
    verbose: bool,
}

impl<'a, L: RawLink> Session<'a, L> {
    pub fn new(link: &'a mut L, log: &'a mut Vec<String>, verbose: bool) -> Self {
        Session { link, log, verbose }
    }

    fn trace(&mut self, s: String) {
        if self.verbose {
            self.log.push(s);
        }
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), Error> {
        let f = build_frame(payload);
        self.trace(format!("      TX {}", hex_trim(&f)));
        self.link.write(&f).map_err(Error::Write)
    }

    fn next_frame(&mut self) -> Result<Vec<u8>, Error> {
        match self.link.read(READ_TIMEOUT) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(Error::Timeout),
            Err(e) => Err(Error::Read(e)),
        }
    }

    /// Direct request; returns the reply data (payload[2..]).
    pub fn direct(&mut self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, Error> {
        self.send(&direct_payload(feature_index, function, params))?;
        for _ in 0..MAX_FRAMES {
            let rep = self.next_frame()?;
            let data = parse_frame(&rep).and_then(|p| direct_reply(p, feature_index));
            self.trace(format!(
                "      RX {}  [{}]",
                hex_trim(&rep),
                if data.is_some() { "reply" } else { "unrelated, skipped" }
            ));
            if let Some(d) = data {
                return Ok(d.to_vec());
            }
        }
        Err(Error::TooManyFrames)
    }

    /// Bridge request to the headset; returns the answer params.
    pub fn bridge(&mut self, bridge_index: u8, sub_feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, Error> {
        self.send(&bridge_payload(bridge_index, sub_feature_index, function, params))?;
        for _ in 0..MAX_FRAMES {
            let rep = self.next_frame()?;
            let r = parse_frame(&rep).map(|p| bridge_reply(p, bridge_index, sub_feature_index));
            let tag = match r {
                Some(BridgeReply::Ack) => "ACK",
                Some(BridgeReply::Connected) => "connected event, skipped",
                Some(BridgeReply::Offline) => "disconnect event",
                Some(BridgeReply::Rejected) => "rejected",
                Some(BridgeReply::Answer(_)) => "answer",
                Some(BridgeReply::Unrelated) | None => "unrelated, skipped",
            };
            self.trace(format!("      RX {}  [{}]", hex_trim(&rep), tag));
            match r {
                Some(BridgeReply::Answer(a)) => return Ok(a.to_vec()),
                Some(BridgeReply::Offline) => return Err(Error::Offline),
                Some(BridgeReply::Rejected) => return Err(Error::Rejected),
                _ => {}
            }
        }
        Err(Error::TooManyFrames)
    }

    /// Step 1: Root lookup of the bridge feature on the dongle.
    pub fn lookup_bridge(&mut self) -> Result<u8, Error> {
        let d = self.direct(0x00, 0x00, &FEATURE_BRIDGE.to_be_bytes())?;
        match d.first() {
            Some(&i) if i != 0 => Ok(i),
            _ => Err(Error::BridgeNotFound),
        }
    }

    /// Step 2: Root lookup of CenturionBatterySoc on the headset.
    pub fn lookup_battery(&mut self, bridge_index: u8) -> Result<u8, Error> {
        let a = self.bridge(bridge_index, 0x00, 0x00, &FEATURE_BATTERY_SOC.to_be_bytes())?;
        match a.first() {
            Some(&i) if i != 0 => Ok(i),
            _ => Err(Error::BatteryNotSupported),
        }
    }

    /// Step 3: battery read.
    pub fn read_battery(&mut self, ix: Indices) -> Result<Battery, Error> {
        let a = self.bridge(ix.bridge, ix.battery, 0x00, &[])?;
        parse_battery(&a)
    }

    /// Steps 1-3, reusing `cache`. Any error except Offline clears it.
    pub fn query(&mut self, cache: &mut Option<Indices>) -> Result<Battery, Error> {
        let ix = match *cache {
            Some(ix) => ix,
            None => {
                self.trace("   1) bridge lookup (direct Root, feature 0x0003)".into());
                let bridge = self.lookup_bridge()?;
                self.trace(format!("      bridge_index 0x{:02X}", bridge));
                self.trace("   2) battery lookup (bridge Root, feature 0x0104)".into());
                let battery = self.lookup_battery(bridge)?;
                self.trace(format!("      battery_index 0x{:02X}", battery));
                Indices { bridge, battery }
            }
        };
        self.trace("   3) battery read".into());
        match self.read_battery(ix) {
            Ok(b) => {
                *cache = Some(ix);
                self.trace(format!(
                    "      percent {}, {}",
                    b.percent,
                    if b.charging { "charging" } else { "not charging" }
                ));
                Ok(b)
            }
            Err(Error::Offline) => {
                *cache = Some(ix);
                Err(Error::Offline)
            }
            Err(e) => {
                *cache = None;
                Err(e)
            }
        }
    }
}

/// Hex with the trailing zero bytes summarised.
pub fn hex_trim(b: &[u8]) -> String {
    let n = b.iter().rposition(|&x| x != 0).map(|i| i + 1).unwrap_or(0).max(1);
    if n >= b.len() {
        format!("{} ({} bytes)", hex(b), b.len())
    } else {
        format!("{} + {} zero bytes ({} total)", hex(&b[..n]), b.len() - n, b.len())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;

    pub fn pad(b: &[u8], n: usize) -> Vec<u8> {
        let mut v = b.to_vec();
        v.resize(n, 0);
        v
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        Online,
        Offline,
        Rejected,
        Silent,
    }

    /// Fake G522 dongle + headset answering the three Centurion requests.
    pub struct FakeHeadset {
        pub bridge: u8,
        pub battery: u8,
        pub percent: u8,
        pub state: u8,
        pub mode: Mode,
        /// Unrelated frame queued before every reply.
        pub noise: bool,
        pub write_fails: bool,
        pub queue: VecDeque<Vec<u8>>,
        pub sent: Vec<Vec<u8>>,
        pub reads: usize,
    }

    impl FakeHeadset {
        pub fn new(mode: Mode) -> Self {
            FakeHeadset {
                bridge: 0x05,
                battery: 0x07,
                percent: 85,
                state: 1,
                mode,
                noise: false,
                write_fails: false,
                queue: VecDeque::new(),
                sent: vec![],
                reads: 0,
            }
        }

        fn push(&mut self, payload: &[u8]) {
            if self.noise {
                self.queue.push_back(pad(&[0x51, 0x0B, 0x00, 0x00], 64));
            }
            self.queue.push_back(build_frame(payload).to_vec());
        }

        fn answer(&mut self, req: &[u8]) {
            let p = parse_frame(req).expect("request is a Centurion frame").to_vec();
            if self.mode == Mode::Silent {
                return;
            }
            if p[0] == 0x00 {
                if p[1] == 0x01 && p[2..] == [0x00, 0x03] {
                    let b = self.bridge;
                    self.push(&[0x00, 0x01, b, 0x00, 0x00]);
                }
                return;
            }
            if p[0] != self.bridge || p[1] != 0x11 {
                return;
            }
            let b = self.bridge;
            self.push(&[b, 0x11, 0x00, 0x00]);
            let sub = &p[4..];
            match self.mode {
                Mode::Offline => self.push(&[b, 0x00, 0x00, 0x00]),
                Mode::Rejected => self.push(&[b, 0x10, 0x00, 0x04, 0x00, 0xFF, sub[1], 0x05]),
                _ => {
                    let params: Vec<u8> = match (sub[1], &sub[3..]) {
                        (0x00, [0x01, 0x04]) => vec![self.battery, 0x00, 0x00],
                        (s, []) if s == self.battery => vec![self.percent, 0x00, self.state],
                        _ => return,
                    };
                    let mut a = vec![b, 0x10, 0x00, (params.len() + 3) as u8, 0x00, sub[1], sub[2]];
                    a.extend_from_slice(&params);
                    self.push(&a);
                }
            }
        }
    }

    impl RawLink for FakeHeadset {
        fn write(&mut self, buf: &[u8]) -> Result<(), String> {
            if self.write_fails {
                return Err("WriteFile failed: Win32 error 87 (invalid parameter)".into());
            }
            assert_eq!(buf.len(), FRAME_LEN);
            self.sent.push(buf.to_vec());
            self.answer(buf);
            Ok(())
        }
        fn read(&mut self, _t: Duration) -> Result<Option<Vec<u8>>, String> {
            self.reads += 1;
            Ok(self.queue.pop_front())
        }
    }

    fn bytes(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|x| u8::from_str_radix(x, 16).unwrap()).collect()
    }

    fn assert_frame(f: &[u8; FRAME_LEN], head: &str) {
        let h = bytes(head);
        assert_eq!(&f[..h.len()], &h[..], "{}", hex(f));
        assert!(f[h.len()..].iter().all(|&x| x == 0), "{}", hex(f));
    }

    #[test]
    fn vector_a_direct_root_lookup() {
        let f = build_frame(&direct_payload(0x00, 0x00, &[0x00, 0x03]));
        assert_frame(&f, "50 23 05 00 00 01 00 03");
    }

    #[test]
    fn vector_b_bridge_root_lookup() {
        let f = build_frame(&bridge_payload(0x05, 0x00, 0x00, &[0x01, 0x04]));
        assert_frame(&f, "50 23 0A 00 05 11 00 05 00 00 01 01 04");
    }

    #[test]
    fn vector_c_bridge_battery_read() {
        let f = build_frame(&bridge_payload(0x05, 0x07, 0x00, &[]));
        assert_frame(&f, "50 23 08 00 05 11 00 03 00 07 01");
    }

    #[test]
    fn vector_d_answer() {
        let p = bytes("05 10 00 06 00 07 00 55 00 01");
        let BridgeReply::Answer(params) = bridge_reply(&p, 0x05, 0x07) else {
            panic!("not an answer");
        };
        assert_eq!(params, &[0x55, 0x00, 0x01]);
        let b = parse_battery(params).unwrap();
        assert_eq!((b.percent, b.charging, b.approx), (85, true, false));
        // Round trip through a frame.
        let f = build_frame(&p);
        assert_eq!(parse_frame(&f), Some(&p[..]));
    }

    #[test]
    fn vector_e_ack_disconnect_rejected() {
        assert_eq!(bridge_reply(&bytes("05 11 00 00"), 5, 7), BridgeReply::Ack);
        assert_eq!(bridge_reply(&bytes("05 00 00 00"), 5, 7), BridgeReply::Offline);
        assert_eq!(bridge_reply(&bytes("05 00 00 01"), 5, 7), BridgeReply::Connected);
        assert_eq!(bridge_reply(&bytes("05 10 00 04 00 FF 07 05"), 5, 7), BridgeReply::Rejected);
        // Other bridge, other sub feature, short connection event.
        assert_eq!(bridge_reply(&bytes("06 10 00 06 00 07 00 55 00 01"), 5, 7), BridgeReply::Unrelated);
        assert_eq!(bridge_reply(&bytes("05 10 00 06 00 08 00 55 00 01"), 5, 7), BridgeReply::Unrelated);
        assert_eq!(bridge_reply(&bytes("05 10 00 06 01 07 00 55 00 01"), 5, 7), BridgeReply::Unrelated);
        assert_eq!(bridge_reply(&bytes("05 00 00"), 5, 7), BridgeReply::Unrelated);
        assert_eq!(bridge_reply(&bytes("05"), 5, 7), BridgeReply::Unrelated);
    }

    #[test]
    fn vector_f_bad_frames() {
        let good = pad(&bytes("50 23 05 00 00 01 05 00 00"), 64);
        assert_eq!(parse_frame(&good), Some(&[0x00, 0x01, 0x05, 0x00][..]));
        for (i, v) in [(0usize, 0x51u8), (1, 0x22)] {
            let mut x = good.clone();
            x[i] = v;
            assert_eq!(parse_frame(&x), None, "prefix byte {}", i);
        }
        for len in [0u8, 1, 62, 0xFF] {
            let mut x = good.clone();
            x[2] = len;
            assert_eq!(parse_frame(&x), None, "LEN {}", len);
        }
        let mut x = good.clone();
        x[2] = 61;
        assert_eq!(parse_frame(&x).unwrap().len(), 60);
        // Short read: LEN beyond the bytes received.
        assert_eq!(parse_frame(&good[..7]), None);
        assert_eq!(parse_frame(&good[..2]), None);
        assert_eq!(parse_frame(&[]), None);
    }

    #[test]
    fn battery_values() {
        assert_eq!(parse_battery(&[0, 0, 0]).unwrap().percent, 0);
        assert_eq!(parse_battery(&[100, 0, 0]).unwrap().percent, 100);
        assert!(parse_battery(&[101, 0, 0]).is_err());
        assert!(parse_battery(&[0xFF]).is_err());
        assert!(parse_battery(&[]).is_err());
        for (state, charging) in [(0u8, false), (1, true), (2, true), (3, false), (0xFF, false)] {
            assert_eq!(parse_battery(&[50, 0, state]).unwrap().charging, charging, "state {}", state);
        }
        assert!(!parse_battery(&[50]).unwrap().charging);
    }

    #[test]
    fn interface_filter() {
        let mut i = HidInfo {
            vid: 0x046D,
            pid: 0x0B18,
            usage_page: 0xFFA0,
            usage: 1,
            interface: Some(3),
            ..Default::default()
        };
        assert!(is_headset_iface(&i));
        i.usage_page = 0xFF13;
        assert!(!is_headset_iface(&i));
        i.usage_page = 0xFFA0;
        i.interface = Some(2);
        assert!(!is_headset_iface(&i));
        i.interface = Some(3);
        i.pid = 0x0B19;
        assert!(!is_headset_iface(&i));
        i.pid = 0x0B18;
        i.usage = 2;
        assert!(!is_headset_iface(&i));
    }

    #[test]
    fn full_query_then_cached() {
        let mut d = FakeHeadset::new(Mode::Online);
        d.noise = true;
        let mut log = Vec::new();
        let mut cache = None;
        let b = Session::new(&mut d, &mut log, true).query(&mut cache).unwrap();
        assert_eq!((b.percent, b.charging), (85, true));
        assert_eq!(cache, Some(Indices { bridge: 5, battery: 7 }));
        assert_eq!(d.sent.len(), 3);
        assert_frame(&d.sent[0].clone().try_into().unwrap(), "50 23 05 00 00 01 00 03");
        assert_frame(&d.sent[1].clone().try_into().unwrap(), "50 23 0A 00 05 11 00 05 00 00 01 01 04");
        assert_frame(&d.sent[2].clone().try_into().unwrap(), "50 23 08 00 05 11 00 03 00 07 01");
        let t = log.join("\n");
        assert!(t.contains("TX 50 23 05 00 00 01 00 03 + 56 zero bytes (64 total)"), "{}", t);
        assert!(t.contains("bridge_index 0x05") && t.contains("battery_index 0x07"), "{}", t);
        assert!(t.contains("[ACK]") && t.contains("[unrelated, skipped]"), "{}", t);
        assert!(t.contains("percent 85, charging"), "{}", t);
        // Second poll: only the battery read.
        d.sent.clear();
        d.state = 0;
        d.percent = 40;
        let mut quiet = Vec::new();
        let b = Session::new(&mut d, &mut quiet, false).query(&mut cache).unwrap();
        assert_eq!((b.percent, b.charging), (40, false));
        assert_eq!(d.sent.len(), 1);
        assert!(quiet.is_empty());
    }

    #[test]
    fn offline_keeps_cache_rejected_clears_it() {
        let ix = Some(Indices { bridge: 5, battery: 7 });
        let mut d = FakeHeadset::new(Mode::Offline);
        let mut log = Vec::new();
        let mut cache = ix;
        assert_eq!(Session::new(&mut d, &mut log, false).query(&mut cache), Err(Error::Offline));
        assert_eq!(cache, ix);
        let mut d = FakeHeadset::new(Mode::Rejected);
        assert_eq!(Session::new(&mut d, &mut log, false).query(&mut cache), Err(Error::Rejected));
        assert_eq!(cache, None);
        // Offline during the lookup: nothing cached.
        let mut d = FakeHeadset::new(Mode::Offline);
        assert_eq!(Session::new(&mut d, &mut log, false).query(&mut cache), Err(Error::Offline));
        assert_eq!(cache, None);
        assert_eq!(d.sent.len(), 2);
    }

    #[test]
    fn timeout_and_write_error() {
        let mut d = FakeHeadset::new(Mode::Silent);
        let mut log = Vec::new();
        let mut cache = Some(Indices { bridge: 5, battery: 7 });
        assert_eq!(Session::new(&mut d, &mut log, false).query(&mut cache), Err(Error::Timeout));
        assert_eq!(cache, None);
        assert_eq!(d.reads, 1);
        let mut d = FakeHeadset::new(Mode::Online);
        d.write_fails = true;
        let e = Session::new(&mut d, &mut log, false).query(&mut cache).unwrap_err();
        assert_eq!(e.to_string(), "write error: WriteFile failed: Win32 error 87 (invalid parameter)");
        assert_eq!(Error::Timeout.to_string(), "timeout (no reply within 1000 ms)");
    }

    #[test]
    fn at_most_eight_frames() {
        let mut d = FakeHeadset::new(Mode::Silent);
        for _ in 0..20 {
            d.queue.push_back(pad(&[0x50, 0x23, 0x05, 0x00, 0x09, 0x01], 64));
        }
        let mut log = Vec::new();
        assert_eq!(Session::new(&mut d, &mut log, false).lookup_bridge(), Err(Error::TooManyFrames));
        assert_eq!(d.reads, MAX_FRAMES);
    }

    #[test]
    fn index_zero_is_not_found() {
        let mut d = FakeHeadset::new(Mode::Online);
        d.bridge = 0;
        let mut log = Vec::new();
        assert_eq!(Session::new(&mut d, &mut log, false).lookup_bridge(), Err(Error::BridgeNotFound));
        let mut d = FakeHeadset::new(Mode::Online);
        d.battery = 0;
        assert_eq!(
            Session::new(&mut d, &mut log, false).lookup_battery(5),
            Err(Error::BatteryNotSupported)
        );
    }
}
