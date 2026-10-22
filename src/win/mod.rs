//! Windows-only parts: HID backend, probe mode, taskbar strip app.

mod app;
pub mod hid;
mod probe;

use std::path::PathBuf;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// %APPDATA%\battery-strip (created on demand).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join("battery-strip");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn main(args: &[String]) -> i32 {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if args.iter().any(|a| a == "--probe") {
        return probe::run(!args.iter().any(|a| a == "--no-dialog"));
    }
    app::run()
}
