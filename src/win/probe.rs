//! `--probe`: write probe.txt next to the exe with everything we see.

use super::hid::{enumerate_all, WinBackend};
use super::wide;
use crate::headset::{self, Variant};
use crate::hidpp::{Battery, Kind};
use crate::scan::{self, Backend, HidInfo};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

/// Passive listen + battery request on every Logitech FF13/FFA0 collection.
/// Returns the first collection that replied, its variant and battery.
fn headset_probe(be: &mut WinBackend, logi: &[HidInfo], out: &mut Vec<String>) -> Option<(HidInfo, Variant, Option<Battery>)> {
    let mut found: Option<(HidInfo, Variant, Option<Battery>)> = None;
    let cands: Vec<&HidInfo> = logi.iter().filter(|i| headset::is_headset_iface(i)).collect();
    if cands.is_empty() {
        out.push("No Logitech interface with usage page FF13 or FFA0.".into());
    }
    for info in cands {
        out.push(format!("-- {}", info.describe()));
        out.push(format!("   {}", info.caps_line()));
        let mut link = match be.open_raw(info) {
            Ok(l) => {
                out.push("   opened shared read/write".into());
                l
            }
            Err(e) => {
                out.push(format!("   open failed: {}", e));
                continue;
            }
        };
        out.push(format!("   a) passive listen {} s:", headset::LISTEN.as_secs()));
        let n = headset::listen(&mut link, headset::LISTEN, out);
        out.push(format!("      {} unsolicited report(s)", n));
        let id = headset::caps_report_id(info);
        out.push(format!(
            "   b) battery request, buffer length = OutputReportByteLength {}, variants: {}",
            info.output_len,
            headset::variants(id).iter().map(|v| v.label()).collect::<Vec<_>>().join(", ")
        ));
        let r = headset::try_variants(&mut link, id, info.output_len as usize, out);
        for (v, e) in &r.errors {
            out.push(format!("   variant {} failed: {}", v.label(), e));
        }
        match (r.variant, &r.battery) {
            (Some(v), Some(b)) => out.push(format!(
                "   result: battery {}%{} (variant {})",
                b.percent,
                if b.charging { " charging" } else { "" },
                v.label()
            )),
            (Some(v), None) => out.push(format!(
                "   result: variant {} got a reply, but none matched the battery signature",
                v.label()
            )),
            (None, _) => out.push("   result: no reply to any variant".into()),
        }
        if let Some(v) = r.variant {
            let better = match &found {
                None => true,
                Some((_, _, b)) => b.is_none() && r.battery.is_some(),
            };
            if better {
                found = Some((info.clone(), v, r.battery));
            }
        }
    }
    found
}

pub fn run(dialog: bool) -> i32 {
    let mut out: Vec<String> = Vec::new();
    out.push(format!("Battery Strip {} probe", env!("CARGO_PKG_VERSION")));
    out.push(format!("Time: {}", crate::logfile::now_utc()));
    out.push("Read-only HID++ 2.0 queries; devices opened in shared mode.".into());
    out.push("Headset collections (usage page FF13/FFA0) get only one read-only battery request.".into());
    out.push(String::new());
    let all = match enumerate_all() {
        Ok(a) => a,
        Err(e) => {
            out.push(format!("HID enumeration failed: {}", e));
            Vec::new()
        }
    };
    let logi: Vec<HidInfo> = all.iter().filter(|i| i.vid == crate::hidpp::VID_LOGITECH).cloned().collect();
    out.push(format!(
        "HID interfaces present: {} total, {} Logitech (VID 046D)",
        all.len(),
        logi.len()
    ));
    out.push("== Logitech HID interfaces ==".into());
    for (n, i) in logi.iter().enumerate() {
        out.push(format!(
            "[{}] {}{}",
            n,
            i.describe(),
            if scan::is_candidate(i) {
                "  <- HID++ long-report candidate"
            } else if headset::is_headset_iface(i) {
                "  <- headset probe candidate"
            } else {
                ""
            }
        ));
        out.push(format!("    {}", i.caps_line()));
        out.push(format!("    path {}", i.path));
    }
    out.push(String::new());
    out.push("== Queries ==".into());
    let mut be = WinBackend;
    let res = scan::scan(&mut be, &logi, true, true, &mut out);
    out.push(String::new());
    out.push("== Headset probe (non-HID++, usage page FF13/FFA0) ==".into());
    out.push("Protocol is a guess from HeadsetControl (PRO X 2), unverified on this headset.".into());
    let raw = headset_probe(&mut be, &logi, &mut out);
    out.push(String::new());
    out.push("== Summary ==".into());
    let mut found_any = false;
    for want in [Kind::Mouse, Kind::Headset] {
        let mut line = None;
        for ir in &res {
            for d in &ir.devices {
                if d.kind == want {
                    let txt = match (&d.battery, d.battery_feature) {
                        (Ok(b), Some((id, idx))) => format!(
                            "{}%{}{} via {} (0x{:04X}) index 0x{:02X}",
                            b.percent,
                            if b.approx { " (coarse)" } else { "" },
                            if b.charging { " charging" } else { "" },
                            crate::hidpp::feature_name(id),
                            id,
                            idx
                        ),
                        (Err(e), _) => format!("no battery reading: {}", e),
                        _ => "no battery reading".into(),
                    };
                    line.get_or_insert(format!(
                        "{:?}: \"{}\" PID {:04X} device index 0x{:02X} (HID++ {}.{}): {}",
                        want, d.name, ir.info.pid, d.dev, d.protocol.0, d.protocol.1, txt
                    ));
                }
            }
        }
        if want == Kind::Headset && line.is_none() {
            line = raw.as_ref().map(|(info, v, b)| match b {
                Some(b) => format!(
                    "Headset: \"{}\" PID {:04X} usage page {:04X}: {}%{} via headset request variant {}",
                    info.product,
                    info.pid,
                    info.usage_page,
                    b.percent,
                    if b.charging { " charging" } else { "" },
                    v.label()
                ),
                None => format!(
                    "Headset: \"{}\" PID {:04X} usage page {:04X} replied to variant {}, but no reply matched the battery signature (strip shows \"H --\")",
                    info.product,
                    info.pid,
                    info.usage_page,
                    v.label()
                ),
            });
        }
        found_any |= line.is_some();
        out.push(line.unwrap_or_else(|| {
            format!(
                "{:?}: not found (no HID++ 2.0 device of this type answered on any interface/index)",
                want
            )
        }));
    }
    for ir in &res {
        for d in ir.devices.iter().filter(|d| matches!(d.kind, Kind::Other | Kind::Unknown)) {
            out.push(format!(
                "Other device: \"{}\" PID {:04X} index 0x{:02X} type {:?}: battery {:?}",
                d.name, ir.info.pid, d.dev, d.dev_type, d.battery
            ));
        }
    }
    for ir in res.iter().filter(|r| r.open_error.is_some()) {
        out.push(format!(
            "Could not open PID {:04X} usage {:04X}: {}",
            ir.info.pid,
            ir.info.usage,
            ir.open_error.as_deref().unwrap_or("")
        ));
    }
    if !found_any {
        out.push("Nothing readable found. The strip will show grey \"--\".".into());
    }
    let raw_battery = raw.as_ref().is_some_and(|r| r.2.is_some());
    if logi.iter().any(headset::is_headset_iface) && !raw_battery {
        out.push("Headset battery: no answer matched (51 0B .. byte8 04, byte10 1-100). Strip shows grey \"H --\".".into());
    }
    let text = out.join("\r\n") + "\r\n";
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut written = None;
    if let Some(d) = exe_dir {
        let p = d.join("probe.txt");
        if std::fs::write(&p, &text).is_ok() {
            written = Some(p);
        }
    }
    if written.is_none() {
        let p = super::data_dir().join("probe.txt");
        if std::fs::write(&p, &text).is_ok() {
            written = Some(p);
        }
    }
    if dialog {
        let msg = match &written {
            Some(p) => format!("Probe finished.\n\nResults written to:\n{}\n\nPlease send this file.", p.display()),
            None => "Probe finished but probe.txt could not be written.".into(),
        };
        let m = wide(&msg);
        let t = wide("Battery Strip probe");
        unsafe { MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONINFORMATION) };
    }
    if written.is_some() {
        0
    } else {
        1
    }
}
