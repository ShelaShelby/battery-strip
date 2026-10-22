//! Non-HID++ battery query for Logitech headsets that expose a vendor
//! collection on usage page 0xFF13 or 0xFFA0 (e.g. G522 LIGHTSPEED, PRO X 2).
//!
//! Protocol is a HYPOTHESIS taken from HeadsetControl PR #478 (PRO X 2),
//! UNVERIFIED on the G522. Only one read-only battery request is ever sent;
//! a number is shown only if a reply matches the exact signature.

use crate::hidpp::{hex, Battery, VID_LOGITECH};
use crate::scan::HidInfo;
use std::time::{Duration, Instant};

/// Battery request (unverified on G522). Sent zero-padded to the full
/// output report length.
pub const REQUEST: [u8; 16] = [
    0x51, 0x08, 0x00, 0x03, 0x1A, 0x00, 0x03, 0x00, 0x04, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
pub const PREFIX: u8 = 0x51;
/// Reports read after each send.
pub const MAX_REPLIES: usize = 4;
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(700);
pub const LISTEN: Duration = Duration::from_secs(3);

/// Logitech collection the headset query is tried on (any PID).
pub fn is_headset_iface(i: &HidInfo) -> bool {
    i.vid == VID_LOGITECH && (i.usage_page == 0xFF13 || i.usage_page == 0xFFA0)
}

/// First non-zero output report ID from the HID caps, 0 if none.
pub fn caps_report_id(i: &HidInfo) -> u8 {
    i.output_ids.iter().copied().find(|&x| x != 0).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The packet itself starts at byte 0 (0x51 acts as report ID).
    A,
    /// 0x00 (no report ID), then the packet.
    B,
    /// Report ID from the caps, then the packet.
    C(u8),
}

impl Variant {
    pub fn label(&self) -> String {
        match self {
            Variant::A => "A (packet at byte 0, 0x51 as report ID)".into(),
            Variant::B => "B (0x00 report ID + packet)".into(),
            Variant::C(id) => format!("C (caps report ID 0x{:02X} + packet)", id),
        }
    }
}

/// Variants in the order they are tried.
pub fn variants(report_id: u8) -> Vec<Variant> {
    let mut v = vec![Variant::A, Variant::B];
    if report_id != 0 {
        v.push(Variant::C(report_id));
    }
    v
}

/// Request buffer of exactly `out_len` bytes (the interface's
/// OutputReportByteLength).
pub fn build_packet(v: Variant, out_len: usize) -> Result<Vec<u8>, String> {
    let (lead, need) = match v {
        Variant::A => (None, REQUEST.len()),
        Variant::B => (Some(0x00), REQUEST.len() + 1),
        Variant::C(id) => (Some(id), REQUEST.len() + 1),
    };
    if out_len < need {
        return Err(format!("output report length {} too short (need {})", out_len, need));
    }
    let mut buf = vec![0u8; out_len];
    match lead {
        None => buf[..REQUEST.len()].copy_from_slice(&REQUEST),
        Some(id) => {
            buf[0] = id;
            buf[1..need].copy_from_slice(&REQUEST);
        }
    }
    Ok(buf)
}

/// Battery answer: byte0 0x51, byte1 0x0B, byte8 0x04, byte10 = 1..=100 %,
/// byte12 0x02 = charging. Anything else is not a battery answer.
pub fn decode(rep: &[u8]) -> Option<Battery> {
    if rep.len() < 13 || rep[0] != PREFIX || rep[1] != 0x0B || rep[8] != 0x04 {
        return None;
    }
    let p = rep[10];
    if !(1..=100).contains(&p) {
        return None;
    }
    Some(Battery {
        percent: p,
        approx: false,
        charging: rep[12] == 0x02,
    })
}

pub fn is_ack(rep: &[u8]) -> bool {
    rep.len() >= 2 && rep[0] == PREFIX && rep[1] == 0x03
}

/// Human-readable tag for the probe log.
pub fn describe(rep: &[u8]) -> String {
    if let Some(b) = decode(rep) {
        return format!("battery answer: {}%{}", b.percent, if b.charging { " charging" } else { "" });
    }
    if is_ack(rep) {
        return "ACK".into();
    }
    if rep.len() >= 7 && rep[0] == PREFIX && rep[1] == 0x05 {
        return format!("power event? byte6={}", rep[6]);
    }
    if rep.len() >= 2 && rep[0] == PREFIX && rep[1] == 0x0B {
        return "0x51 0x0B but signature/percent not valid -> ignored".into();
    }
    "unrelated".into()
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

/// Raw access to one HID collection.
pub trait RawLink {
    fn write_file(&mut self, buf: &[u8]) -> Result<(), String>;
    fn set_output_report(&mut self, buf: &[u8]) -> Result<(), String>;
    /// One input report, Ok(None) on timeout.
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String>;
}

/// Log every unsolicited report for `dur`; returns how many arrived.
pub fn listen<L: RawLink>(link: &mut L, dur: Duration, log: &mut Vec<String>) -> usize {
    let deadline = Instant::now() + dur;
    let mut n = 0;
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match link.read(deadline - now) {
            Ok(Some(r)) => {
                n += 1;
                log.push(format!("    unsolicited: {}  [{}]", hex(&r), describe(&r)));
            }
            Ok(None) => break,
            Err(e) => {
                log.push(format!("    read failed: {}", e));
                break;
            }
        }
    }
    n
}

#[derive(Debug, Default)]
pub struct QueryResult {
    /// First error text if the request could not be sent.
    pub error: Option<String>,
    pub replies: usize,
    pub battery: Option<Battery>,
}

/// Send the battery request with one variant and read up to MAX_REPLIES
/// reports (ACK first, then data, is expected).
pub fn query<L: RawLink>(link: &mut L, v: Variant, out_len: usize, log: &mut Vec<String>) -> QueryResult {
    let mut q = QueryResult::default();
    let buf = match build_packet(v, out_len) {
        Ok(b) => b,
        Err(e) => {
            log.push(format!("    variant {}: not sent: {}", v.label(), e));
            q.error = Some(e);
            return q;
        }
    };
    log.push(format!("    variant {}", v.label()));
    log.push(format!("    TX {}", hex_trim(&buf)));
    match link.write_file(&buf) {
        Ok(()) => log.push("    sent with WriteFile".into()),
        Err(e1) => {
            log.push(format!("    WriteFile failed: {}", e1));
            match link.set_output_report(&buf) {
                Ok(()) => log.push("    sent with HidD_SetOutputReport".into()),
                Err(e2) => {
                    log.push(format!("    HidD_SetOutputReport failed: {}", e2));
                    q.error = Some(format!("WriteFile: {}", e1));
                    return q;
                }
            }
        }
    }
    for _ in 0..MAX_REPLIES {
        match link.read(REPLY_TIMEOUT) {
            Ok(Some(r)) => {
                q.replies += 1;
                log.push(format!("    RX {}  [{}]", hex(&r), describe(&r)));
                if let Some(b) = decode(&r) {
                    q.battery = Some(b);
                    break;
                }
            }
            Ok(None) => {
                log.push(format!("    no (more) reports within {} ms", REPLY_TIMEOUT.as_millis()));
                break;
            }
            Err(e) => {
                log.push(format!("    read failed: {}", e));
                break;
            }
        }
    }
    q
}

#[derive(Debug, Default)]
pub struct ProbeResult {
    /// Variant that got any reply (remembered for polling).
    pub variant: Option<Variant>,
    pub battery: Option<Battery>,
    /// (variant, first error text) for variants that could not be sent.
    pub errors: Vec<(Variant, String)>,
}

/// Try the variants in order; stop at the first that gets any reply.
pub fn try_variants<L: RawLink>(link: &mut L, report_id: u8, out_len: usize, log: &mut Vec<String>) -> ProbeResult {
    let mut res = ProbeResult::default();
    for v in variants(report_id) {
        let q = query(link, v, out_len, log);
        if let Some(e) = q.error {
            res.errors.push((v, e));
        }
        if q.replies > 0 {
            res.variant = Some(v);
            res.battery = q.battery;
            break;
        }
    }
    res
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;

    // Example packets from HeadsetControl PR #478 (PRO X 2): unverified on G522.
    /// unverified on G522: battery 57 %, charging.
    pub const EX_BATTERY_CHARGING: [u8; 16] = [
        0x51, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 57, 0x00, 0x02, 0x00, 0x00, 0x00,
    ];
    /// unverified on G522: ACK.
    pub const EX_ACK: [u8; 4] = [0x51, 0x03, 0x00, 0x00];
    /// unverified on G522: power-on event.
    pub const EX_POWER_ON: [u8; 8] = [0x51, 0x05, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00];

    pub fn pad(b: &[u8], n: usize) -> Vec<u8> {
        let mut v = b.to_vec();
        v.resize(n, 0);
        v
    }

    /// Fake headset collection. `accept` decides which first byte of a
    /// request it answers (None = silent).
    pub struct FakeHeadset {
        pub out_len: usize,
        pub accept: Option<u8>,
        pub writefile_fails: bool,
        pub setoutput_fails: bool,
        pub replies: Vec<Vec<u8>>,
        pub queue: VecDeque<Vec<u8>>,
        pub sent: Vec<(&'static str, Vec<u8>)>,
        pub reads: usize,
    }

    impl FakeHeadset {
        pub fn new(out_len: usize, accept: Option<u8>, replies: Vec<Vec<u8>>) -> Self {
            FakeHeadset {
                out_len,
                accept,
                writefile_fails: false,
                setoutput_fails: false,
                replies,
                queue: VecDeque::new(),
                sent: vec![],
                reads: 0,
            }
        }
        fn take(&mut self, how: &'static str, buf: &[u8]) -> Result<(), String> {
            if buf.len() != self.out_len {
                return Err("Win32 error 87".into());
            }
            self.sent.push((how, buf.to_vec()));
            if self.accept == Some(buf[0]) {
                self.queue.extend(self.replies.iter().cloned());
            }
            Ok(())
        }
    }

    impl RawLink for FakeHeadset {
        fn write_file(&mut self, buf: &[u8]) -> Result<(), String> {
            if self.writefile_fails {
                return Err("Win32 error 87".into());
            }
            self.take("WriteFile", buf)
        }
        fn set_output_report(&mut self, buf: &[u8]) -> Result<(), String> {
            if self.setoutput_fails {
                return Err("Win32 error 1".into());
            }
            self.take("SetOutputReport", buf)
        }
        fn read(&mut self, _t: Duration) -> Result<Option<Vec<u8>>, String> {
            self.reads += 1;
            Ok(self.queue.pop_front())
        }
    }

    #[test]
    fn packet_variants_exact_lengths() {
        for n in [62usize, 64] {
            let a = build_packet(Variant::A, n).unwrap();
            assert_eq!(a.len(), n);
            assert_eq!(&a[..16], &REQUEST);
            assert!(a[16..].iter().all(|&b| b == 0));
            let b = build_packet(Variant::B, n).unwrap();
            assert_eq!(b.len(), n);
            assert_eq!(b[0], 0x00);
            assert_eq!(&b[1..17], &REQUEST);
            assert!(b[17..].iter().all(|&x| x == 0));
            let c = build_packet(Variant::C(0x51), n).unwrap();
            assert_eq!(c.len(), n);
            assert_eq!(c[0], 0x51);
            assert_eq!(&c[1..17], &REQUEST);
        }
        // Request bytes as published (unverified on G522).
        assert_eq!(hex(&REQUEST[..10]), "51 08 00 03 1A 00 03 00 04 0A");
        assert!(build_packet(Variant::A, 15).is_err());
        assert!(build_packet(Variant::B, 16).is_err());
        assert!(build_packet(Variant::A, 0).is_err());
        assert_eq!(build_packet(Variant::A, 16).unwrap().len(), 16);
    }

    #[test]
    fn variant_order() {
        assert_eq!(variants(0), vec![Variant::A, Variant::B]);
        assert_eq!(variants(0x20), vec![Variant::A, Variant::B, Variant::C(0x20)]);
    }

    #[test]
    fn decoder_signature() {
        let b = decode(&pad(&EX_BATTERY_CHARGING, 64)).unwrap();
        assert_eq!(b.percent, 57);
        assert!(b.charging);
        assert!(!b.approx);
        let mut x = pad(&EX_BATTERY_CHARGING, 62);
        x[12] = 0x00;
        assert!(!decode(&x).unwrap().charging);
        x[12] = 0x01;
        assert!(!decode(&x).unwrap().charging);
        for (i, v) in [(0usize, 0x50u8), (1, 0x0A), (8, 0x05)] {
            let mut y = pad(&EX_BATTERY_CHARGING, 64);
            y[i] = v;
            assert_eq!(decode(&y), None, "byte {} changed", i);
        }
        for p in [0u8, 101, 255] {
            let mut y = pad(&EX_BATTERY_CHARGING, 64);
            y[10] = p;
            assert_eq!(decode(&y), None, "percent {}", p);
        }
        for p in [1u8, 100] {
            let mut y = pad(&EX_BATTERY_CHARGING, 64);
            y[10] = p;
            assert_eq!(decode(&y).unwrap().percent, p);
        }
        // Report-ID-0 prefixed copy is not accepted, nor too short.
        let mut z = vec![0u8];
        z.extend_from_slice(&EX_BATTERY_CHARGING);
        assert_eq!(decode(&z), None);
        assert_eq!(decode(&EX_BATTERY_CHARGING[..12]), None);
        assert_eq!(decode(&pad(&EX_ACK, 64)), None);
        assert_eq!(decode(&pad(&EX_POWER_ON, 64)), None);
        assert!(is_ack(&EX_ACK));
        assert!(describe(&EX_POWER_ON).starts_with("power event"));
    }

    #[test]
    fn ack_then_data() {
        let mut d = FakeHeadset::new(64, Some(0x51), vec![pad(&EX_ACK, 64), pad(&EX_BATTERY_CHARGING, 64)]);
        let mut log = Vec::new();
        let q = query(&mut d, Variant::A, 64, &mut log);
        assert_eq!(q.replies, 2);
        assert_eq!(q.battery.unwrap().percent, 57);
        assert_eq!(d.reads, 2);
        assert_eq!(d.sent.len(), 1);
        let t = log.join("\n");
        assert!(t.contains("[ACK]"), "{}", t);
        assert!(t.contains("TX 51 08 00 03 1A 00 03 00 04 0A + 54 zero bytes (64 total)"), "{}", t);
    }

    #[test]
    fn unrelated_reports_read_at_most_four() {
        let junk = pad(&[0x51, 0x05, 0, 0, 0, 0, 1], 62);
        let mut d = FakeHeadset::new(62, Some(0x51), vec![junk.clone(); 6]);
        let mut log = Vec::new();
        let q = query(&mut d, Variant::A, 62, &mut log);
        assert_eq!(q.replies, MAX_REPLIES);
        assert_eq!(q.battery, None);
        assert_eq!(d.reads, MAX_REPLIES);
    }

    #[test]
    fn fallback_order_stops_on_first_reply() {
        // Answers only variant B (first byte 0x00).
        let mut d = FakeHeadset::new(64, Some(0x00), vec![pad(&EX_ACK, 64), pad(&EX_BATTERY_CHARGING, 64)]);
        let mut log = Vec::new();
        let r = try_variants(&mut d, 0x20, 64, &mut log);
        assert_eq!(r.variant, Some(Variant::B));
        assert_eq!(r.battery.unwrap().percent, 57);
        assert_eq!(d.sent.len(), 2);
        assert_eq!(d.sent[0].1[0], 0x51);
        assert_eq!(d.sent[1].1[0], 0x00);
    }

    #[test]
    fn reply_without_battery_still_stops() {
        let mut d = FakeHeadset::new(64, Some(0x51), vec![pad(&EX_ACK, 64)]);
        let mut log = Vec::new();
        let r = try_variants(&mut d, 0x20, 64, &mut log);
        assert_eq!(r.variant, Some(Variant::A));
        assert_eq!(r.battery, None);
        assert_eq!(d.sent.len(), 1);
    }

    #[test]
    fn writefile_fails_then_set_output_report() {
        let mut d = FakeHeadset::new(62, Some(0x51), vec![pad(&EX_BATTERY_CHARGING, 62)]);
        d.writefile_fails = true;
        let mut log = Vec::new();
        let r = try_variants(&mut d, 0, 62, &mut log);
        assert_eq!(r.variant, Some(Variant::A));
        assert_eq!(d.sent[0].0, "SetOutputReport");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn all_fail_logs_first_error_per_variant() {
        let mut d = FakeHeadset::new(64, Some(0x51), vec![]);
        d.writefile_fails = true;
        d.setoutput_fails = true;
        let mut log = Vec::new();
        let r = try_variants(&mut d, 0x07, 64, &mut log);
        assert_eq!(r.variant, None);
        assert_eq!(r.battery, None);
        let vs: Vec<Variant> = r.errors.iter().map(|e| e.0).collect();
        assert_eq!(vs, vec![Variant::A, Variant::B, Variant::C(0x07)]);
        assert!(r.errors.iter().all(|e| e.1 == "WriteFile: Win32 error 87"));
        assert_eq!(d.reads, 0);
    }

    #[test]
    fn silent_device_times_out_quickly() {
        let mut d = FakeHeadset::new(64, None, vec![]);
        let mut log = Vec::new();
        let r = try_variants(&mut d, 0, 64, &mut log);
        assert_eq!(r.variant, None);
        assert_eq!(r.battery, None);
        // One timed-out read per variant, never more.
        assert_eq!(d.reads, 2);
        assert_eq!(d.sent.len(), 2);
        assert!(log.join("\n").contains("no (more) reports within 700 ms"));
        assert_eq!(listen(&mut d, LISTEN, &mut log), 0);
    }

    #[test]
    fn listen_logs_unsolicited() {
        let mut d = FakeHeadset::new(64, None, vec![]);
        d.queue.push_back(pad(&EX_POWER_ON, 64));
        let mut log = Vec::new();
        assert_eq!(listen(&mut d, LISTEN, &mut log), 1);
        assert!(log[0].contains("unsolicited: 51 05 00 00 00 00 01 00"));
        assert!(d.sent.is_empty());
    }

    #[test]
    fn interface_filter() {
        let mut i = HidInfo {
            vid: 0x046D,
            pid: 0x0B18,
            usage_page: 0xFFA0,
            output_len: 64,
            ..Default::default()
        };
        assert!(is_headset_iface(&i));
        i.usage_page = 0xFF13;
        assert!(is_headset_iface(&i));
        i.usage_page = 0x000C;
        assert!(!is_headset_iface(&i));
        i.usage_page = 0xFF00;
        assert!(!is_headset_iface(&i));
        i.usage_page = 0xFFA0;
        i.vid = 0x1234;
        assert!(!is_headset_iface(&i));
        i.output_ids = vec![0, 0x51];
        assert_eq!(caps_report_id(&i), 0x51);
    }
}
