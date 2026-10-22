//! Windows HID backend: SetupDi enumeration, shared (non-exclusive) opens,
//! overlapped reads/writes with timeouts.

use super::wide;
use crate::headset::RawLink;
use crate::scan::{interface_from_path, Backend, HidInfo, Link};
use std::time::{Duration, Instant};
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::System::IO::*;

fn last_error() -> String {
    win32_text(unsafe { GetLastError() })
}

/// "Win32 error N (meaning)" for the codes seen with HID writes.
fn win32_text(e: u32) -> String {
    let what = match e {
        1 => " (incorrect function)",
        5 => " (access denied)",
        6 => " (invalid handle)",
        31 => " (device not functioning)",
        87 => " (invalid parameter)",
        121 => " (semaphore timeout)",
        995 => " (I/O aborted)",
        1167 => " (device not connected)",
        _ => "",
    };
    format!("Win32 error {}{}", e, what)
}

/// Sorted unique report IDs of one report type (value + button caps).
unsafe fn report_ids(pp: PHIDP_PREPARSED_DATA, kind: HIDP_REPORT_TYPE, nvalues: u16, nbuttons: u16) -> Vec<u8> {
    let mut ids = Vec::new();
    if nvalues > 0 {
        let mut v: Vec<HIDP_VALUE_CAPS> = vec![std::mem::zeroed(); nvalues as usize];
        let mut n = nvalues;
        if HidP_GetValueCaps(kind, v.as_mut_ptr(), &mut n, pp) == HIDP_STATUS_SUCCESS {
            ids.extend(v[..(n as usize).min(v.len())].iter().map(|c| c.ReportID));
        }
    }
    if nbuttons > 0 {
        let mut b: Vec<HIDP_BUTTON_CAPS> = vec![std::mem::zeroed(); nbuttons as usize];
        let mut n = nbuttons;
        if HidP_GetButtonCaps(kind, b.as_mut_ptr(), &mut n, pp) == HIDP_STATUS_SUCCESS {
            ids.extend(b[..(n as usize).min(b.len())].iter().map(|c| c.ReportID));
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn open_path(path: &str, access: u32, overlapped: bool) -> Result<Handle, String> {
    let w = wide(path);
    let flags = if overlapped { FILE_FLAG_OVERLAPPED } else { 0 };
    // Shared read/write: G HUB keeps working; never exclusive.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        Err(last_error())
    } else {
        Ok(Handle(h))
    }
}

fn read_info(path: &str) -> Option<HidInfo> {
    // Access 0: attribute queries only, works even for system-owned devices.
    let h = open_path(path, 0, false).ok()?;
    let mut attr: HIDD_ATTRIBUTES = unsafe { std::mem::zeroed() };
    attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
    if unsafe { HidD_GetAttributes(h.0, &mut attr) } == 0 {
        return None;
    }
    let mut info = HidInfo {
        path: path.to_string(),
        vid: attr.VendorID,
        pid: attr.ProductID,
        interface: interface_from_path(path),
        ..Default::default()
    };
    if attr.VendorID != crate::hidpp::VID_LOGITECH {
        return Some(info);
    }
    let mut pp: PHIDP_PREPARSED_DATA = 0;
    if unsafe { HidD_GetPreparsedData(h.0, &mut pp) } != 0 {
        let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
        if unsafe { HidP_GetCaps(pp, &mut caps) } == HIDP_STATUS_SUCCESS {
            info.usage_page = caps.UsagePage;
            info.usage = caps.Usage;
            info.input_len = caps.InputReportByteLength;
            info.output_len = caps.OutputReportByteLength;
            info.feature_len = caps.FeatureReportByteLength;
            unsafe {
                info.input_ids = report_ids(pp, HidP_Input, caps.NumberInputValueCaps, caps.NumberInputButtonCaps);
                info.output_ids = report_ids(pp, HidP_Output, caps.NumberOutputValueCaps, caps.NumberOutputButtonCaps);
                info.feature_ids = report_ids(pp, HidP_Feature, caps.NumberFeatureValueCaps, caps.NumberFeatureButtonCaps);
            }
        }
        unsafe { HidD_FreePreparsedData(pp) };
    }
    let mut buf = [0u16; 128];
    if unsafe { HidD_GetProductString(h.0, buf.as_mut_ptr() as *mut _, (buf.len() * 2) as u32) } != 0 {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        info.product = String::from_utf16_lossy(&buf[..n]);
    }
    Some(info)
}

/// All present HID interfaces (Logitech ones carry full caps).
pub fn enumerate_all() -> Result<Vec<HidInfo>, String> {
    let mut guid: GUID = unsafe { std::mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut guid) };
    let set = unsafe { SetupDiGetClassDevsW(&guid, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) };
    if set == INVALID_HANDLE_VALUE as HDEVINFO || set == 0 {
        return Err(format!("SetupDiGetClassDevs failed: {}", last_error()));
    }
    let mut out = Vec::new();
    let mut i = 0u32;
    loop {
        let mut ifd: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
        ifd.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if unsafe { SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &guid, i, &mut ifd) } == 0 {
            break;
        }
        i += 1;
        if i > 4096 {
            break;
        }
        let mut need = 0u32;
        unsafe { SetupDiGetDeviceInterfaceDetailW(set, &ifd, std::ptr::null_mut(), 0, &mut need, std::ptr::null_mut()) };
        if !(8..=64 * 1024).contains(&need) {
            continue;
        }
        // u64 buffer for alignment of the detail struct.
        let mut buf = vec![0u64; (need as usize).div_ceil(8) + 1];
        let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        unsafe { (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
        if unsafe { SetupDiGetDeviceInterfaceDetailW(set, &ifd, detail, need, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            continue;
        }
        let off = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
        let max_chars = (need as usize).saturating_sub(off) / 2;
        let p = unsafe { (buf.as_ptr() as *const u8).add(off) as *const u16 };
        let chars = unsafe { std::slice::from_raw_parts(p, max_chars) };
        let n = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
        let path = String::from_utf16_lossy(&chars[..n]);
        if let Some(info) = read_info(&path) {
            out.push(info);
        }
    }
    unsafe { SetupDiDestroyDeviceInfoList(set) };
    Ok(out)
}

struct Reader {
    file: Handle,
    event: Handle,
    ov: Box<OVERLAPPED>,
    buf: Box<[u8]>,
    pending: bool,
}

impl Reader {
    fn new(file: Handle, len: usize) -> Result<Self, String> {
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err(last_error());
        }
        Ok(Reader {
            file,
            event: Handle(ev),
            ov: Box::new(unsafe { std::mem::zeroed() }),
            buf: vec![0u8; len.max(64)].into_boxed_slice(),
            pending: false,
        })
    }

    /// Arm a read if none pending. Returns a report if it completed at once.
    fn arm(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.pending {
            return Ok(None);
        }
        *self.ov = unsafe { std::mem::zeroed() };
        self.ov.hEvent = self.event.0;
        let ok = unsafe {
            ReadFile(
                self.file.0,
                self.buf.as_mut_ptr(),
                self.buf.len() as u32,
                std::ptr::null_mut(),
                &mut *self.ov,
            )
        };
        if ok != 0 {
            return self.finish().map(Some);
        }
        let e = unsafe { GetLastError() };
        if e == ERROR_IO_PENDING {
            self.pending = true;
            Ok(None)
        } else {
            Err(format!("ReadFile failed: Win32 error {}", e))
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, String> {
        let mut n = 0u32;
        let ok = unsafe { GetOverlappedResult(self.file.0, &*self.ov, &mut n, 0) };
        self.pending = false;
        if ok == 0 {
            return Err(format!("read failed: {}", last_error()));
        }
        Ok(self.buf[..(n as usize).min(self.buf.len())].to_vec())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if self.pending {
            let mut n = 0u32;
            unsafe {
                CancelIoEx(self.file.0, &*self.ov);
                // Wait for cancellation so the buffer/OVERLAPPED outlive the I/O.
                GetOverlappedResult(self.file.0, &*self.ov, &mut n, 1);
            }
            self.pending = false;
        }
    }
}

/// WriteFile on an overlapped handle, 1 s timeout.
fn overlapped_write(file: HANDLE, event: HANDLE, buf: &[u8]) -> Result<(), String> {
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event;
    let ok = unsafe { WriteFile(file, buf.as_ptr(), buf.len() as u32, std::ptr::null_mut(), &mut ov) };
    if ok == 0 {
        let e = unsafe { GetLastError() };
        if e != ERROR_IO_PENDING {
            return Err(format!("WriteFile failed: {}", win32_text(e)));
        }
        let w = unsafe { WaitForSingleObject(event, 1000) };
        if w != WAIT_OBJECT_0 {
            let mut n = 0u32;
            unsafe {
                CancelIoEx(file, &ov);
                GetOverlappedResult(file, &ov, &mut n, 1);
            }
            return Err("write timeout".into());
        }
    }
    let mut n = 0u32;
    if unsafe { GetOverlappedResult(file, &ov, &mut n, 1) } == 0 {
        return Err(format!("write failed: {}", last_error()));
    }
    Ok(())
}

pub struct WinLink {
    out_len: usize,
    readers: Vec<Reader>,
    write_event: Handle,
}

impl Link for WinLink {
    fn write(&mut self, report: &[u8]) -> Result<(), String> {
        let mut buf = vec![0u8; self.out_len.max(report.len())];
        buf[..report.len()].copy_from_slice(report);
        overlapped_write(self.readers[0].file.0, self.write_event.0, &buf)
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        let deadline = Instant::now() + timeout;
        for (k, r) in self.readers.iter_mut().enumerate() {
            match r.arm() {
                Ok(Some(rep)) => return Ok(Some(rep)),
                Ok(None) => {}
                // The optional short sibling must not break the main link.
                Err(e) if k == 0 => return Err(e),
                Err(_) => {}
            }
        }
        let events: Vec<HANDLE> = self.readers.iter().filter(|r| r.pending).map(|r| r.event.0).collect();
        let idx: Vec<usize> = self.readers.iter().enumerate().filter(|(_, r)| r.pending).map(|(i, _)| i).collect();
        if events.is_empty() {
            return Ok(None);
        }
        let ms = deadline.saturating_duration_since(Instant::now()).as_millis().min(10_000) as u32;
        let w = unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, ms) };
        if w == WAIT_TIMEOUT {
            return Ok(None);
        }
        let i = w.wrapping_sub(WAIT_OBJECT_0) as usize;
        if i >= events.len() {
            return Err(format!("wait failed: {}", last_error()));
        }
        let k = idx[i];
        match self.readers[k].finish() {
            Ok(r) => Ok(Some(r)),
            Err(e) if k == 0 => Err(e),
            Err(_) => Ok(None),
        }
    }
}

/// The Centurion collection: 64-byte WriteFile, 64-byte overlapped reads.
pub struct WinRawLink {
    reader: Reader,
    write_event: Handle,
}

impl RawLink for WinRawLink {
    fn write(&mut self, buf: &[u8]) -> Result<(), String> {
        overlapped_write(self.reader.file.0, self.write_event.0, buf)
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        if let Some(rep) = self.reader.arm()? {
            return Ok(Some(rep));
        }
        if !self.reader.pending {
            return Ok(None);
        }
        let ms = timeout.as_millis().min(10_000) as u32;
        let w = unsafe { WaitForSingleObject(self.reader.event.0, ms) };
        if w == WAIT_TIMEOUT {
            return Ok(None);
        }
        if w != WAIT_OBJECT_0 {
            return Err(format!("wait failed: {}", last_error()));
        }
        self.reader.finish().map(Some)
    }
}

pub struct WinBackend;

impl Backend for WinBackend {
    type L = WinLink;

    fn enumerate(&mut self) -> Result<Vec<HidInfo>, String> {
        enumerate_all().map(|v| v.into_iter().filter(|i| i.vid == crate::hidpp::VID_LOGITECH).collect())
    }

    fn open(&mut self, info: &HidInfo, sibling: Option<&HidInfo>) -> Result<WinLink, String> {
        let main = open_path(&info.path, GENERIC_READ | GENERIC_WRITE, true)?;
        let mut readers = vec![Reader::new(main, info.input_len as usize)?];
        if let Some(s) = sibling {
            if let Ok(h) = open_path(&s.path, GENERIC_READ, true) {
                if let Ok(r) = Reader::new(h, s.input_len as usize) {
                    readers.push(r);
                }
            }
        }
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err(last_error());
        }
        Ok(WinLink {
            out_len: info.output_len as usize,
            readers,
            write_event: Handle(ev),
        })
    }

    type R = WinRawLink;

    fn open_raw(&mut self, info: &HidInfo) -> Result<WinRawLink, String> {
        let h = open_path(&info.path, GENERIC_READ | GENERIC_WRITE, true)?;
        let reader = Reader::new(h, crate::headset::FRAME_LEN)?;
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err(last_error());
        }
        Ok(WinRawLink {
            reader,
            write_event: Handle(ev),
        })
    }
}
