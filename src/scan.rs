//! Device discovery and battery reading over an abstract HID link.
//! The Windows HID backend implements `Link`/`Backend`; tests use a fake.
//! Only read-only HID++ getters are ever sent.

use crate::headset::{self, RawLink};
use crate::hidpp::{self, feature, Battery, Kind, Reply, LONG_LEN, PARAMS_LEN};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct HidInfo {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    /// Report IDs from HidP_GetValueCaps/HidP_GetButtonCaps (sorted, unique).
    pub input_ids: Vec<u8>,
    pub output_ids: Vec<u8>,
    pub feature_ids: Vec<u8>,
    pub interface: Option<u8>,
    pub product: String,
}

impl HidInfo {
    pub fn describe(&self) -> String {
        format!(
            "VID {:04X} PID {:04X}  usage page {:04X} usage {:04X}  interface {}  in {} out {}  product \"{}\"",
            self.vid,
            self.pid,
            self.usage_page,
            self.usage,
            self.interface.map(|i| i.to_string()).unwrap_or_else(|| "-".into()),
            self.input_len,
            self.output_len,
            self.product
        )
    }

    /// HidP_GetCaps report lengths and report IDs.
    pub fn caps_line(&self) -> String {
        fn ids(v: &[u8]) -> String {
            if v.is_empty() {
                "none".into()
            } else {
                v.iter().map(|b| format!("0x{:02X}", b)).collect::<Vec<_>>().join(",")
            }
        }
        format!(
            "caps: InputReportByteLength {}, OutputReportByteLength {}, FeatureReportByteLength {}; report IDs input {} output {} feature {}",
            self.input_len,
            self.output_len,
            self.feature_len,
            ids(&self.input_ids),
            ids(&self.output_ids),
            ids(&self.feature_ids)
        )
    }
}

/// Logitech vendor-specific collection that can carry long HID++ reports.
/// Headset collections (0xFF13/0xFFA0) are not HID++ and never get HID++
/// requests (the battery is read over Centurion, see `headset`).
pub fn is_candidate(i: &HidInfo) -> bool {
    i.vid == hidpp::VID_LOGITECH
        && i.usage_page >= 0xFF00
        && i.output_len as usize >= LONG_LEN
        && i.usage_page != 0xFF13
        && i.usage_page != 0xFFA0
}

/// Parse "&mi_02" from a Windows HID path.
pub fn interface_from_path(path: &str) -> Option<u8> {
    let p = path.to_ascii_lowercase();
    let i = p.find("&mi_")?;
    let hex = p.get(i + 4..i + 6)?;
    u8::from_str_radix(hex, 16).ok()
}

/// Path with the "&colNN" part removed: identical for all top-level
/// collections of one USB interface.
pub fn collection_group(path: &str) -> String {
    let p = path.to_ascii_lowercase();
    match p.find("&col") {
        Some(i) => {
            let end = (i + 6).min(p.len());
            format!("{}{}", &p[..i], &p[end..])
        }
        None => p,
    }
}

/// Short-report (0x10) sibling collection of a long one, used only to read
/// HID++ 1.0 error replies quickly.
pub fn short_sibling<'a>(all: &'a [HidInfo], long: &HidInfo) -> Option<&'a HidInfo> {
    let g = collection_group(&long.path);
    all.iter().find(|i| {
        i.path != long.path
            && i.vid == long.vid
            && i.pid == long.pid
            && i.usage_page == long.usage_page
            && i.input_len as usize == hidpp::SHORT_LEN
            && collection_group(&i.path) == g
    })
}

pub trait Link {
    fn write(&mut self, report: &[u8]) -> Result<(), String>;
    /// One input report, Ok(None) on timeout.
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String>;
}

pub trait Backend {
    type L: Link;
    fn enumerate(&mut self) -> Result<Vec<HidInfo>, String>;
    /// Open shared (never exclusive); `sibling` is an optional short-report
    /// collection to also read from.
    fn open(&mut self, info: &HidInfo, sibling: Option<&HidInfo>) -> Result<Self::L, String>;
    type R: RawLink;
    /// Open the Centurion collection shared read/write (one per poll).
    fn open_raw(&mut self, info: &HidInfo) -> Result<Self::R, String>;
}

#[derive(Debug)]
pub enum ReqErr {
    Timeout,
    Io(String),
}

pub struct Session<'a, L: Link> {
    link: &'a mut L,
    log: &'a mut Vec<String>,
    verbose: bool,
    sw: u8,
    pub timeout: Duration,
}

const SW_IDS: [u8; 6] = [0xA, 0xB, 0xC, 0xD, 0xE, 0xF];

impl<'a, L: Link> Session<'a, L> {
    pub fn new(link: &'a mut L, log: &'a mut Vec<String>, verbose: bool) -> Self {
        Session {
            link,
            log,
            verbose,
            sw: 0,
            timeout: Duration::from_millis(700),
        }
    }

    fn next_sw(&mut self) -> u8 {
        self.sw = (self.sw + 1) % SW_IDS.len() as u8;
        SW_IDS[self.sw as usize]
    }

    pub fn note(&mut self, s: String) {
        self.log.push(s);
    }

    pub fn request(&mut self, dev: u8, idx: u8, func: u8, params: &[u8], timeout: Duration) -> Result<Reply, ReqErr> {
        let sw = self.next_sw();
        let req = hidpp::build_long(dev, idx, func, sw, params);
        if self.verbose {
            self.log.push(format!("    TX {}", hidpp::hex(&req)));
        }
        self.link.write(&req).map_err(|e| {
            self.log.push(format!("    write failed: {}", e));
            ReqErr::Io(e)
        })?;
        let deadline = Instant::now() + timeout;
        let mut other = 0usize;
        loop {
            let now = Instant::now();
            if now >= deadline {
                if self.verbose {
                    self.log.push(format!(
                        "    RX timeout ({} ms, {} unrelated reports ignored)",
                        timeout.as_millis(),
                        other
                    ));
                }
                return Err(ReqErr::Timeout);
            }
            let rep = match self.link.read(deadline - now) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(e) => {
                    self.log.push(format!("    read failed: {}", e));
                    return Err(ReqErr::Io(e));
                }
            };
            match hidpp::match_reply(&req, &rep) {
                Some(r) => {
                    if self.verbose {
                        let n = if rep[0] == hidpp::REPORT_SHORT {
                            hidpp::SHORT_LEN
                        } else {
                            LONG_LEN
                        };
                        let what = match &r {
                            Reply::Ok(_) => "ok".to_string(),
                            Reply::Err20(c) => {
                                format!("HID++ 2.0 error 0x{:02X} ({})", c, hidpp::err20_name(*c))
                            }
                            Reply::Err10(c) => {
                                format!("HID++ 1.0 error 0x{:02X} ({})", c, hidpp::err10_name(*c))
                            }
                        };
                        self.log
                            .push(format!("    RX {}  -> {}", hidpp::hex(&rep[..n.min(rep.len())]), what));
                    }
                    return Ok(r);
                }
                None => {
                    other += 1;
                    if self.verbose && other <= 3 {
                        let n = rep.len().min(LONG_LEN);
                        self.log.push(format!("    (unrelated report) {}", hidpp::hex(&rep[..n])));
                    }
                }
            }
        }
    }

    fn ok(&mut self, dev: u8, idx: u8, func: u8, params: &[u8]) -> Result<[u8; PARAMS_LEN], String> {
        let t = self.timeout;
        match self.request(dev, idx, func, params, t) {
            Ok(Reply::Ok(p)) => Ok(p),
            Ok(Reply::Err20(c)) => Err(format!("HID++ 2.0 error 0x{:02X} ({})", c, hidpp::err20_name(c))),
            Ok(Reply::Err10(c)) => Err(format!("HID++ 1.0 error 0x{:02X} ({})", c, hidpp::err10_name(c))),
            Err(ReqErr::Timeout) => Err("timeout".into()),
            Err(ReqErr::Io(e)) => Err(format!("I/O error: {}", e)),
        }
    }

    pub fn get_feature(&mut self, dev: u8, id: u16) -> Result<Option<u8>, String> {
        let p = self.ok(dev, 0, 0, &hidpp::get_feature_params(id))?;
        Ok(hidpp::parse_get_feature(&p))
    }

    /// Read a battery value using an already known feature index.
    pub fn read_battery(&mut self, dev: u8, id: u16, idx: u8) -> Result<Battery, String> {
        match id {
            feature::UNIFIED_BATTERY => {
                let caps = self.ok(dev, idx, 0, &[]).ok();
                let st = self.ok(dev, idx, 1, &[])?;
                hidpp::decode_unified(&st, caps.as_ref())
            }
            feature::BATTERY_STATUS => hidpp::decode_battery_status(&self.ok(dev, idx, 0, &[])?),
            feature::BATTERY_VOLTAGE => hidpp::decode_voltage(&self.ok(dev, idx, 0, &[])?),
            feature::ADC_MEASUREMENT => hidpp::decode_adc(&self.ok(dev, idx, 0, &[])?),
            _ => Err("not a battery feature".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub dev: u8,
    pub protocol: (u8, u8),
    pub name: String,
    pub dev_type: Option<u8>,
    pub kind: Kind,
    pub features: Vec<(u8, u16)>,
    /// (feature id, index) used for the battery, if any.
    pub battery_feature: Option<(u16, u8)>,
    pub battery: Result<Battery, String>,
}

pub enum Ping {
    Device((u8, u8)),
    Absent(String),
    /// Writing failed: do not try other indices on this interface.
    Dead(String),
}

pub fn ping<L: Link>(s: &mut Session<L>, dev: u8) -> Ping {
    let t = Duration::from_millis(400);
    match s.request(dev, 0, 1, &[0, 0, 0x5A], t) {
        Ok(Reply::Ok(p)) => Ping::Device((p[0], p[1])),
        Ok(Reply::Err20(c)) => Ping::Absent(format!("HID++ 2.0 error 0x{:02X} ({})", c, hidpp::err20_name(c))),
        Ok(Reply::Err10(c)) => Ping::Absent(format!("HID++ 1.0 error 0x{:02X} ({})", c, hidpp::err10_name(c))),
        Err(ReqErr::Timeout) => Ping::Absent("no answer (timeout)".into()),
        Err(ReqErr::Io(e)) => Ping::Dead(e),
    }
}

fn read_name<L: Link>(s: &mut Session<L>, dev: u8, idx: u8) -> (String, Option<u8>) {
    let mut name = Vec::new();
    if let Ok(p) = s.ok(dev, idx, 0, &[]) {
        let len = (p[0] as usize).min(64);
        while name.len() < len {
            match s.ok(dev, idx, 1, &[name.len() as u8]) {
                Ok(c) => {
                    let take = (len - name.len()).min(PARAMS_LEN);
                    let chunk: Vec<u8> = c[..take].iter().copied().take_while(|&b| b != 0).collect();
                    let done = chunk.len() < take;
                    name.extend_from_slice(&chunk);
                    if done || chunk.is_empty() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    let t = s.ok(dev, idx, 2, &[]).ok().map(|p| p[0]);
    let name: String = String::from_utf8_lossy(&name).chars().filter(|c| !c.is_control()).collect();
    (name, t)
}

/// Query one responding device: features, name/type and battery.
pub fn query_device<L: Link>(s: &mut Session<L>, dev: u8, protocol: (u8, u8), full_table: bool) -> DeviceInfo {
    let mut features: Vec<(u8, u16)> = vec![(0, feature::ROOT)];
    let fs_idx = s.get_feature(dev, feature::FEATURE_SET).ok().flatten();
    if let Some(fs) = fs_idx {
        features.push((fs, feature::FEATURE_SET));
        if full_table {
            if let Ok(p) = s.ok(dev, fs, 0, &[]) {
                let count = p[0];
                for i in 1..=count {
                    if let Ok(q) = s.ok(dev, fs, 1, &[i]) {
                        let id = hidpp::parse_feature_id(&q);
                        if !features.iter().any(|(x, _)| *x == i) {
                            features.push((i, id));
                        }
                    }
                }
            }
        }
    }
    // Ensure the features we need are known even without a table.
    for id in [
        feature::DEVICE_NAME,
        feature::UNIFIED_BATTERY,
        feature::BATTERY_STATUS,
        feature::BATTERY_VOLTAGE,
        feature::ADC_MEASUREMENT,
    ] {
        if hidpp::lookup(&features, id).is_none() && !(full_table && fs_idx.is_some()) {
            if let Ok(Some(i)) = s.get_feature(dev, id) {
                // Cross-check with FEATURE_SET when available (guards
                // against a reply meant for another program).
                let ok = match fs_idx {
                    Some(fs) => s.ok(dev, fs, 1, &[i]).map(|q| hidpp::parse_feature_id(&q) == id).unwrap_or(true),
                    None => true,
                };
                if ok {
                    features.push((i, id));
                } else {
                    s.note(format!("    feature 0x{:04X}: index 0x{:02X} failed cross-check, ignored", id, i));
                }
            }
        }
    }
    features.sort();
    let (name, dev_type) = match hidpp::lookup(&features, feature::DEVICE_NAME) {
        Some(i) => read_name(s, dev, i),
        None => (String::new(), None),
    };
    let kind = hidpp::classify(dev_type, &name);
    let mut reasons = Vec::new();
    let mut battery = Err(String::new());
    let mut battery_feature = None;
    for id in hidpp::BATTERY_FEATURES {
        let Some(idx) = hidpp::lookup(&features, id) else {
            reasons.push(format!("{} (0x{:04X}) not supported", hidpp::feature_name(id), id));
            continue;
        };
        match s.read_battery(dev, id, idx) {
            Ok(b) => {
                battery = Ok(b);
                battery_feature = Some((id, idx));
                break;
            }
            Err(e) => reasons.push(format!("{} (0x{:04X}) at index 0x{:02X}: {}", hidpp::feature_name(id), id, idx, e)),
        }
    }
    if battery_feature.is_none() {
        battery = Err(reasons.join("; "));
    }
    DeviceInfo {
        dev,
        protocol,
        name,
        dev_type,
        kind,
        features,
        battery_feature,
        battery,
    }
}

pub struct InterfaceResult {
    pub info: HidInfo,
    pub open_error: Option<String>,
    pub devices: Vec<DeviceInfo>,
}

/// Scan every candidate interface and every device index.
pub fn scan<B: Backend>(
    backend: &mut B,
    infos: &[HidInfo],
    full_table: bool,
    verbose: bool,
    log: &mut Vec<String>,
) -> Vec<InterfaceResult> {
    let mut out = Vec::new();
    for (n, info) in infos.iter().enumerate() {
        if !is_candidate(info) {
            continue;
        }
        let sib = short_sibling(infos, info);
        log.push(format!("== Interface [{}] {}", n, info.describe()));
        if verbose {
            log.push(format!("   path {}", info.path));
        }
        let mut link = match backend.open(info, sib) {
            Ok(l) => {
                log.push(format!(
                    "   opened shared read/write{}",
                    if sib.is_some() {
                        " (+ short-report sibling for errors)"
                    } else {
                        ""
                    }
                ));
                l
            }
            Err(e) => {
                log.push(format!("   open failed: {}", e));
                out.push(InterfaceResult {
                    info: info.clone(),
                    open_error: Some(e),
                    devices: vec![],
                });
                continue;
            }
        };
        let mut devices = Vec::new();
        for &dev in hidpp::DEVICE_INDICES.iter() {
            let mut s = Session::new(&mut link, log, verbose);
            s.note(format!("  -- device index 0x{:02X}: ping (ROOT.GetProtocolVersion)", dev));
            match ping(&mut s, dev) {
                Ping::Device(v) => {
                    s.note(format!("   HID++ {}.{} device answers", v.0, v.1));
                    let d = query_device(&mut s, dev, v, full_table);
                    if verbose || full_table {
                        s.note("   feature table (index: id name):".into());
                        for (i, id) in &d.features {
                            s.note(format!("     0x{:02X}: 0x{:04X} {}", i, id, hidpp::feature_name(*id)));
                        }
                    }
                    s.note(format!(
                        "   name \"{}\", type {} -> {:?}",
                        d.name,
                        d.dev_type
                            .map(|t| format!("{} ({})", t, hidpp::device_type_name(t)))
                            .unwrap_or_else(|| "unknown".into()),
                        d.kind
                    ));
                    match (&d.battery, d.battery_feature) {
                        (Ok(b), Some((id, idx))) => s.note(format!(
                            "   battery via {} (0x{:04X}) index 0x{:02X}: {}%{}{}",
                            hidpp::feature_name(id),
                            id,
                            idx,
                            b.percent,
                            if b.approx { " (coarse level)" } else { "" },
                            if b.charging { ", charging" } else { "" }
                        )),
                        (Err(e), _) => s.note(format!("   battery: not readable: {}", e)),
                        _ => {}
                    }
                    devices.push(d);
                }
                Ping::Absent(r) => s.note(format!("   nothing usable: {}", r)),
                Ping::Dead(e) => {
                    s.note(format!("   interface not writable ({}), skipping remaining indices", e));
                    break;
                }
            }
        }
        out.push(InterfaceResult {
            info: info.clone(),
            open_error: None,
            devices,
        });
    }
    out
}

/// Remembered location of a device's battery feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    pub dev: u8,
    pub feature: u16,
    pub idx: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readings {
    pub mouse: Option<Battery>,
    pub headset: Option<Battery>,
}

/// Normal-mode poller: re-read known devices, rescan for missing ones.
#[derive(Default)]
pub struct Poller {
    pub mouse: Option<Found>,
    pub headset: Option<Found>,
    /// Centurion bridge/battery indices (in memory only).
    pub centurion: Option<headset::Indices>,
}

impl Poller {
    pub fn enumerate<B: Backend>(backend: &mut B, log: &mut Vec<String>) -> Option<Vec<HidInfo>> {
        match backend.enumerate() {
            Ok(i) => Some(i),
            Err(e) => {
                log.push(format!("HID enumeration failed: {}", e));
                None
            }
        }
    }

    /// Full poll in one go (the app splits it to publish the mouse first).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn poll<B: Backend>(&mut self, backend: &mut B, log: &mut Vec<String>) -> Readings {
        let Some(infos) = Self::enumerate(backend, log) else {
            return Readings::default();
        };
        let mut r = self.poll_hidpp(backend, &infos, log);
        if r.headset.is_none() {
            r.headset = self.poll_headset(backend, &infos, log);
        }
        r
    }

    /// HID++ devices (mouse, and a HID++ headset if there is one).
    pub fn poll_hidpp<B: Backend>(&mut self, backend: &mut B, infos: &[HidInfo], log: &mut Vec<String>) -> Readings {
        let mut r = Readings::default();
        for (slot, out, name) in [
            (&mut self.mouse, &mut r.mouse, "mouse"),
            (&mut self.headset, &mut r.headset, "headset"),
        ] {
            if let Some(f) = slot.clone() {
                match read_found(backend, infos, &f, log) {
                    Ok(b) => *out = Some(b),
                    Err(e) => {
                        log.push(format!("{}: re-read failed ({}), will rescan", name, e));
                        *slot = None;
                    }
                }
            }
        }
        if r.mouse.is_none() || r.headset.is_none() {
            let mut scan_log = Vec::new();
            let res = scan(backend, infos, false, false, &mut scan_log);
            for ir in &res {
                for d in &ir.devices {
                    let (Ok(b), Some((id, idx))) = (&d.battery, d.battery_feature) else {
                        continue;
                    };
                    let f = Found {
                        path: ir.info.path.clone(),
                        dev: d.dev,
                        feature: id,
                        idx,
                    };
                    match d.kind {
                        Kind::Mouse if r.mouse.is_none() => {
                            log.push(format!("mouse found: \"{}\" idx 0x{:02X} via 0x{:04X}", d.name, d.dev, id));
                            r.mouse = Some(*b);
                            self.mouse = Some(f);
                        }
                        Kind::Headset if r.headset.is_none() => {
                            log.push(format!("headset found: \"{}\" idx 0x{:02X} via 0x{:04X}", d.name, d.dev, id));
                            r.headset = Some(*b);
                            self.headset = Some(f);
                        }
                        _ => {}
                    }
                }
            }
        }
        r
    }

    /// Centurion battery read on the G522 collection: open, query, close.
    /// Bounded by the read timeouts; run after the mouse has been published.
    /// None (grey "H --") when the headset is off or the read failed.
    pub fn poll_headset<B: Backend>(&mut self, backend: &mut B, infos: &[HidInfo], log: &mut Vec<String>) -> Option<Battery> {
        let Some(info) = infos.iter().find(|i| headset::is_headset_iface(i)) else {
            self.centurion = None;
            return None;
        };
        let mut link = match backend.open_raw(info) {
            Ok(l) => l,
            Err(e) => {
                log.push(format!("headset: open failed ({}), will retry", e));
                self.centurion = None;
                return None;
            }
        };
        let mut scratch = Vec::new();
        match headset::Session::new(&mut link, &mut scratch, false).query(&mut self.centurion) {
            Ok(b) => Some(b),
            Err(headset::Error::Offline) => None,
            Err(e) => {
                log.push(format!("headset: {}, will redo lookups", e));
                None
            }
        }
    }
}

fn read_found<B: Backend>(backend: &mut B, infos: &[HidInfo], f: &Found, log: &mut Vec<String>) -> Result<Battery, String> {
    let info = infos.iter().find(|i| i.path == f.path).ok_or("interface gone")?;
    let sib = short_sibling(infos, info);
    let mut link = backend.open(info, sib)?;
    let mut s = Session::new(&mut link, log, false);
    s.read_battery(f.dev, f.feature, f.idx)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Emulates a receiver with HID++ 2.0 devices at some indices.
    pub struct FakeDevice {
        pub dev: u8,
        pub features: Vec<u16>,
        pub name: &'static str,
        pub dev_type: u8,
        pub battery: [u8; 3],
    }

    pub struct FakeLink {
        pub devices: Vec<FakeDevice>,
        pub queue: VecDeque<Vec<u8>>,
        pub writes: Vec<Vec<u8>>,
        pub noise: bool,
    }

    impl FakeLink {
        fn answer(&self, req: &[u8]) -> Option<Vec<u8>> {
            let dev = req[1];
            let Some(d) = self.devices.iter().find(|d| d.dev == dev) else {
                return Some(vec![0x10, dev, 0x8F, req[2], req[3], 0x09, 0x00]);
            };
            let mut rep = vec![0u8; 20];
            rep[..4].copy_from_slice(&req[..4]);
            let func = req[3] >> 4;
            let idx = req[2] as usize;
            let table: Vec<u16> = std::iter::once(0x0000).chain(d.features.iter().copied()).collect();
            let Some(&fid) = table.get(idx) else {
                return Some(vec![
                    0x11, dev, 0xFF, req[2], req[3], 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ]);
            };
            match (fid, func) {
                (0x0000, 0) => {
                    let want = u16::from_be_bytes([req[4], req[5]]);
                    rep[4] = table.iter().position(|&f| f == want).unwrap_or(0) as u8;
                }
                (0x0000, 1) => {
                    rep[4] = 4;
                    rep[5] = 2;
                    rep[6] = req[6];
                }
                (0x0001, 0) => rep[4] = d.features.len() as u8,
                (0x0001, 1) => {
                    let f = table.get(req[4] as usize).copied().unwrap_or(0);
                    rep[4..6].copy_from_slice(&f.to_be_bytes());
                }
                (0x0005, 0) => rep[4] = d.name.len() as u8,
                (0x0005, 1) => {
                    let start = req[4] as usize;
                    let b = d.name.as_bytes();
                    for (k, c) in b[start.min(b.len())..].iter().take(16).enumerate() {
                        rep[4 + k] = *c;
                    }
                }
                (0x0005, 2) => rep[4] = d.dev_type,
                (0x1004, 0) => {
                    rep[4] = 0x0F;
                    rep[5] = 0x03;
                }
                (0x1004, 1) | (0x1000, 0) | (0x1001, 0) | (0x1F20, 0) => rep[4..7].copy_from_slice(&d.battery),
                _ => {
                    return Some(vec![
                        0x11, dev, 0xFF, req[2], req[3], 0x07, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    ]);
                }
            }
            Some(rep)
        }
    }

    impl Link for FakeLink {
        fn write(&mut self, report: &[u8]) -> Result<(), String> {
            assert_eq!(report.len(), 20);
            self.writes.push(report.to_vec());
            if self.noise {
                // unrelated G HUB-style traffic first
                self.queue
                    .push_back(vec![0x11, report[1], 0x00, 0x11, 9, 9, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            }
            if let Some(a) = self.answer(report) {
                self.queue.push_back(a);
            }
            Ok(())
        }
        fn read(&mut self, _t: Duration) -> Result<Option<Vec<u8>>, String> {
            Ok(self.queue.pop_front())
        }
    }

    struct FakeBackend {
        infos: Vec<HidInfo>,
        devices: fn() -> Vec<FakeDevice>,
        /// How the fake G522 behaves.
        raw_mode: crate::headset::tests::Mode,
        raw_sent: Rc<RefCell<Vec<Vec<u8>>>>,
    }

    fn backend(infos: Vec<HidInfo>, devices: fn() -> Vec<FakeDevice>) -> FakeBackend {
        FakeBackend {
            infos,
            devices,
            raw_mode: crate::headset::tests::Mode::Silent,
            raw_sent: Rc::default(),
        }
    }

    impl Backend for FakeBackend {
        type L = FakeLink;
        fn enumerate(&mut self) -> Result<Vec<HidInfo>, String> {
            Ok(self.infos.clone())
        }
        fn open(&mut self, info: &HidInfo, _s: Option<&HidInfo>) -> Result<FakeLink, String> {
            let devices = if info.usage == 2 { (self.devices)() } else { vec![] };
            Ok(FakeLink {
                devices,
                queue: VecDeque::new(),
                writes: vec![],
                noise: true,
            })
        }
        type R = FakeRaw;
        fn open_raw(&mut self, info: &HidInfo) -> Result<FakeRaw, String> {
            assert!(crate::headset::is_headset_iface(info));
            Ok(FakeRaw {
                dev: crate::headset::tests::FakeHeadset::new(self.raw_mode),
                sent: self.raw_sent.clone(),
            })
        }
    }

    /// Records what was sent to the fake headset in the backend.
    struct FakeRaw {
        dev: crate::headset::tests::FakeHeadset,
        sent: Rc<RefCell<Vec<Vec<u8>>>>,
    }

    impl RawLink for FakeRaw {
        fn write(&mut self, buf: &[u8]) -> Result<(), String> {
            self.sent.borrow_mut().push(buf.to_vec());
            self.dev.write(buf)
        }
        fn read(&mut self, t: Duration) -> Result<Option<Vec<u8>>, String> {
            self.dev.read(t)
        }
    }

    fn info(path: &str, usage: u16, len: u16) -> HidInfo {
        HidInfo {
            path: path.into(),
            vid: 0x046D,
            pid: 0xC54D,
            usage_page: 0xFF00,
            usage,
            input_len: len,
            output_len: len,
            interface: interface_from_path(path),
            product: "USB Receiver".into(),
            ..Default::default()
        }
    }

    fn devices() -> Vec<FakeDevice> {
        vec![
            FakeDevice {
                dev: 1,
                features: vec![0x0001, 0x0005, 0x1004],
                name: "PRO 2 LIGHTSPEED",
                dev_type: 3,
                battery: [85, 0x04, 0x00],
            },
            FakeDevice {
                dev: 2,
                features: vec![0x0001, 0x0005, 0x1F20],
                name: "G522 LIGHTSPEED",
                dev_type: 8,
                battery: [0x0E, 0xE3, 0x01],
            },
        ]
    }

    fn g522(usage_page: u16, len: u16, col: u8) -> HidInfo {
        HidInfo {
            path: format!(r"\\?\hid#vid_046d&pid_0b18&mi_03&col{:02}#x", col),
            vid: 0x046D,
            pid: 0x0B18,
            usage_page,
            usage: 1,
            input_len: len,
            output_len: len,
            interface: Some(3),
            product: "G522 LIGHTSPEED".into(),
            ..Default::default()
        }
    }

    fn no_devices() -> Vec<FakeDevice> {
        vec![]
    }

    #[test]
    fn headset_ifaces_never_get_hidpp() {
        let i = g522(0xFFA0, 64, 3);
        assert!(!is_candidate(&i));
        assert!(!is_candidate(&g522(0xFF13, 62, 2)));
        let mut be = backend(vec![g522(0x000C, 2, 1), g522(0xFF13, 62, 2), i], no_devices);
        let infos = be.enumerate().unwrap();
        let mut log = Vec::new();
        assert!(scan(&mut be, &infos, true, true, &mut log).is_empty());
        assert!(be.raw_sent.borrow().is_empty());
    }

    #[test]
    fn poller_headset_centurion_cached() {
        use crate::headset::tests::Mode;
        let mut be = backend(vec![g522(0x000C, 2, 1), g522(0xFF13, 62, 2), g522(0xFFA0, 64, 3)], no_devices);
        be.raw_mode = Mode::Online;
        let mut poller = Poller::default();
        let mut log = Vec::new();
        let r = poller.poll(&mut be, &mut log);
        assert_eq!(r.mouse, None);
        let h = r.headset.unwrap();
        assert_eq!((h.percent, h.charging, h.approx), (85, true, false));
        assert_eq!(poller.centurion, Some(crate::headset::Indices { bridge: 5, battery: 7 }));
        {
            let sent = be.raw_sent.borrow();
            assert_eq!(sent.len(), 3);
            assert!(sent.iter().all(|f| f.len() == 64 && f[0] == 0x50));
        }
        be.raw_sent.borrow_mut().clear();
        let r2 = poller.poll(&mut be, &mut log);
        assert_eq!(r2, r);
        // Only the battery read on later polls.
        assert_eq!(be.raw_sent.borrow().len(), 1);
        assert!(log.is_empty(), "{:?}", log);
    }

    #[test]
    fn headset_offline_or_silent_shows_nothing() {
        use crate::headset::tests::Mode;
        for (mode, logged) in [(Mode::Offline, false), (Mode::Rejected, true), (Mode::Silent, true)] {
            let mut be = backend(vec![g522(0xFF13, 62, 2), g522(0xFFA0, 64, 3)], no_devices);
            be.raw_mode = mode;
            let mut poller = Poller::default();
            let mut log = Vec::new();
            let r = poller.poll(&mut be, &mut log);
            assert_eq!(r, Readings::default());
            assert_eq!(poller.centurion, None);
            assert_eq!(!log.is_empty(), logged, "{:?} {:?}", mode, log);
        }
    }

    #[test]
    fn no_g522_no_headset_request() {
        let mut be = backend(vec![g522(0xFF13, 62, 2)], no_devices);
        let mut poller = Poller::default();
        let mut log = Vec::new();
        assert_eq!(poller.poll(&mut be, &mut log), Readings::default());
        assert!(be.raw_sent.borrow().is_empty());
    }

    #[test]
    fn path_helpers() {
        let p = r"\\?\HID#VID_046D&PID_C54D&MI_02&Col02#8&1a2b&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(interface_from_path(p), Some(2));
        assert_eq!(interface_from_path("nothing"), None);
        let s = p.replace("Col02", "Col01");
        assert_eq!(collection_group(p), collection_group(&s));
        let infos = vec![info(&s, 1, 7), info(p, 2, 20)];
        assert!(is_candidate(&infos[1]));
        assert!(!is_candidate(&infos[0]));
        assert_eq!(short_sibling(&infos, &infos[1]).unwrap().usage, 1);
    }

    #[test]
    fn scan_finds_mouse_and_headset_with_table() {
        let p2 = r"\\?\hid#vid_046d&pid_c54d&mi_02&col02#x";
        let p1 = r"\\?\hid#vid_046d&pid_c54d&mi_02&col01#x";
        let mut be = backend(vec![info(p1, 1, 7), info(p2, 2, 20)], devices);
        let infos = be.enumerate().unwrap();
        let mut log = Vec::new();
        let res = scan(&mut be, &infos, true, true, &mut log);
        assert_eq!(res.len(), 1);
        let devs = &res[0].devices;
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].kind, Kind::Mouse);
        assert_eq!(devs[0].name, "PRO 2 LIGHTSPEED");
        assert_eq!(devs[0].battery_feature, Some((0x1004, 3)));
        assert_eq!(devs[0].battery.as_ref().unwrap().percent, 85);
        assert_eq!(devs[0].features, vec![(0, 0), (1, 1), (2, 5), (3, 0x1004)]);
        assert_eq!(devs[1].kind, Kind::Headset);
        assert_eq!(devs[1].battery_feature, Some((0x1F20, 3)));
        assert_eq!(devs[1].battery.as_ref().unwrap().percent, 50);
        let text = log.join("\n");
        assert!(text.contains("TX 11 FF 00"));
        assert!(text.contains("HID++ 1.0 error 0x09"));
        assert!(text.contains("0x03: 0x1004 UNIFIED_BATTERY"));
    }

    #[test]
    fn poller_reads_then_rereads_cached() {
        let p2 = r"\\?\hid#vid_046d&pid_c54d&mi_02&col02#x";
        let mut be = backend(vec![info(p2, 2, 20)], devices);
        let mut poller = Poller::default();
        let mut log = Vec::new();
        let r = poller.poll(&mut be, &mut log);
        assert_eq!(r.mouse.unwrap().percent, 85);
        assert_eq!(r.headset.unwrap().percent, 50);
        assert_eq!(poller.mouse.as_ref().unwrap().dev, 1);
        let r2 = poller.poll(&mut be, &mut log);
        assert_eq!(r, r2);
    }

    #[test]
    fn nothing_answering_gives_no_number() {
        fn none() -> Vec<FakeDevice> {
            vec![FakeDevice {
                dev: 1,
                features: vec![0x0001, 0x0005],
                name: "PRO 2 LIGHTSPEED",
                dev_type: 3,
                battery: [0, 0, 0],
            }]
        }
        let p2 = r"\\?\hid#vid_046d&pid_c54d&mi_02&col02#x";
        let mut be = backend(vec![info(p2, 2, 20)], none);
        let mut log = Vec::new();
        let r = Poller::default().poll(&mut be, &mut log);
        assert_eq!(r, Readings::default());
    }

    #[test]
    fn non_logitech_or_short_only_is_skipped() {
        let mut i = info("a", 2, 20);
        i.vid = 0x1234;
        assert!(!is_candidate(&i));
        let mut j = info("b", 2, 20);
        j.usage_page = 0x0001;
        assert!(!is_candidate(&j));
    }
}
