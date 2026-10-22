//! `--probe`: write probe.txt next to the exe with everything we see.

use super::hid::{enumerate_all, WinBackend};
use super::wide;
use crate::headset;
use crate::hidpp::{Battery, Kind};
use crate::scan::{self, Backend, HidInfo};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

/// Centurion steps 1-3 on the G522 collection, every frame logged.
fn centurion_probe(be: &mut WinBackend, logi: &[HidInfo], out: &mut Vec<String>) -> Option<(HidInfo, Result<Battery, headset::Error>)> {
    let Some(info) = logi.iter().find(|i| headset::is_headset_iface(i)) else {
        out.push("No G522 Centurion interface (VID 046D PID 0B18, usage page FFA0, usage 0001, interface 3).".into());
        return None;
    };
    out.push(format!("-- {}", info.describe()));
    let mut link = match be.open_raw(info) {
        Ok(l) => l,
        Err(e) => {
            out.push(format!("   open failed: {}", e));
            return None;
        }
    };
    out.push("   opened shared read/write".into());
    let r = headset::Session::new(&mut link, out, true).query(&mut None);
    if let Err(e) = &r {
        out.push(format!("   failed: {}", e));
    }
    Some((info.clone(), r))
}

pub fn run(dialog: bool) -> i32 {
    let mut out: Vec<String> = Vec::new();
    out.push(format!("Battery Strip {} probe", env!("CARGO_PKG_VERSION")));
    out.push(format!("Time: {}", crate::logfile::now_utc()));
    out.push("Read-only HID++ 2.0 queries; devices opened in shared mode.".into());
    out.push("G522 headset: read-only Centurion battery requests on its FFA0 collection.".into());
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
                "  <- Centurion (G522) interface"
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
    out.push("== Centurion probe ==".into());
    let raw = centurion_probe(&mut be, &logi, &mut out);
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
            line = raw.as_ref().map(|(info, r)| match r {
                Ok(b) => format!(
                    "Headset: \"{}\" PID {:04X}: {}%{} via Centurion",
                    info.product,
                    info.pid,
                    b.percent,
                    if b.charging { " charging" } else { "" }
                ),
                Err(e) => format!(
                    "Headset: \"{}\" PID {:04X}: no battery reading: {} (strip shows \"H --\")",
                    info.product, info.pid, e
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
