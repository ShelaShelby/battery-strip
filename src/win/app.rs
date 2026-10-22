//! The taskbar strip application: one layered click-through window per
//! monitor, tray icon + menu, background battery poller.

use super::hid::WinBackend;
use super::{data_dir, wide};
use crate::display;
use crate::layout::{self, Rect};
use crate::logfile;
use crate::render::{self, Canvas};
use crate::scan::{Poller, Readings};
use crate::settings::Settings;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_BATTERY: u32 = WM_APP + 2;
const WM_APP_RELAYOUT: u32 = WM_APP + 3;
const TIMER_TICK: usize = 1;
const TICK_MS: u32 = 2000;
const POLL: Duration = Duration::from_secs(60);

const CMD_MOUSE: u32 = 10;
const CMD_HEADSET: u32 = 11;
const CMD_AUTOSTART: u32 = 12;
const CMD_REFRESH: u32 = 13;
const CMD_LOGDIR: u32 = 14;
const CMD_PROBE: u32 = 15;
const CMD_EXIT: u32 = 99;
const CMD_MONITOR0: u32 = 100;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "BatteryStrip";

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main_hwnd() -> HWND {
    MAIN_HWND.load(Ordering::SeqCst) as HWND
}

struct Monitor {
    handle: HMONITOR,
    rect: Rect,
    name: String,
    dpi: u32,
}

struct Taskbar {
    rect: Rect,
    tray: Option<Rect>,
    monitor: HMONITOR,
}

struct Strip {
    name: String,
    hwnd: HWND,
    shown: bool,
    key: String,
}

struct App {
    hwnd: HWND,
    hinst: HINSTANCE,
    font: Option<fontdue::Font>,
    settings: Settings,
    settings_path: PathBuf,
    log_path: PathBuf,
    shared: Arc<Mutex<Readings>>,
    wake: mpsc::Sender<()>,
    readings: Readings,
    strips: Vec<Strip>,
    monitors: Vec<(String, bool)>,
    icon: HICON,
}

fn rect_of(r: &RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn window_rect(h: HWND) -> Option<Rect> {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(h, &mut r) } == 0 {
        None
    } else {
        Some(rect_of(&r))
    }
}

unsafe extern "system" fn enum_monitor(h: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
    let v = &mut *(data as *mut Vec<Monitor>);
    let mut mi: MONITORINFOEXW = std::mem::zeroed();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
        let n = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
            dx = 96;
        }
        v.push(Monitor {
            handle: h,
            rect: rect_of(&mi.monitorInfo.rcMonitor),
            name: String::from_utf16_lossy(&mi.szDevice[..n]),
            dpi: dx,
        });
    }
    1
}

fn monitors() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_monitor),
            &mut v as *mut Vec<Monitor> as LPARAM,
        )
    };
    v
}

unsafe extern "system" fn enum_secondary(h: HWND, data: LPARAM) -> BOOL {
    if class_name(h) == "Shell_SecondaryTrayWnd" {
        let v = &mut *(data as *mut Vec<HWND>);
        v.push(h);
    }
    1
}

fn taskbars() -> Vec<Taskbar> {
    let mut hwnds = Vec::new();
    let primary = unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null()) };
    if !primary.is_null() {
        hwnds.push(primary);
    }
    let mut sec: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(enum_secondary), &mut sec as *mut Vec<HWND> as LPARAM) };
    hwnds.extend(sec);
    let mut out = Vec::new();
    for h in hwnds {
        let Some(rect) = window_rect(h) else { continue };
        let notify = unsafe { FindWindowExW(h, std::ptr::null_mut(), wide("TrayNotifyWnd").as_ptr(), std::ptr::null()) };
        let tray = if notify.is_null() { None } else { window_rect(notify) };
        let monitor = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
        out.push(Taskbar { rect, tray, monitor });
    }
    out
}

/// Monitors on which a fullscreen app is in the foreground; `all` for
/// exclusive D3D fullscreen / presentation mode.
fn fullscreen(own: &[HWND]) -> (Option<HMONITOR>, bool) {
    let mut st: QUERY_USER_NOTIFICATION_STATE = 0;
    let all = unsafe { SHQueryUserNotificationState(&mut st) } == 0 && (st == QUNS_RUNNING_D3D_FULL_SCREEN || st == QUNS_PRESENTATION_MODE);
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() || own.contains(&fg) || unsafe { IsWindowVisible(fg) } == 0 || unsafe { IsIconic(fg) } != 0 {
        return (None, all);
    }
    let cls = class_name(fg);
    if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return (None, all);
    }
    let Some(r) = window_rect(fg) else {
        return (None, all);
    };
    let mon = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(mon, &mut mi) } == 0 {
        return (None, all);
    }
    if r.contains(&rect_of(&mi.rcMonitor)) {
        (Some(mon), all)
    } else {
        (None, all)
    }
}

fn present(hwnd: HWND, c: &Canvas, x: i32, y: i32) -> bool {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = c.w as i32;
        bmi.bmiHeader.biHeight = -(c.h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut ok = false;
        if !bmp.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(c.px.as_ptr(), bits as *mut u8, c.px.len());
            let old = SelectObject(mem, bmp);
            let pt = POINT { x, y };
            let size = SIZE {
                cx: c.w as i32,
                cy: c.h as i32,
            };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            ok = UpdateLayeredWindow(hwnd, screen, &pt, &size, mem, &src, 0, &blend, ULW_ALPHA) != 0;
            SelectObject(mem, old);
            DeleteObject(bmp);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        ok
    }
}

fn make_icon() -> HICON {
    let s = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let mut px = vec![0u8; (s * s * 4) as usize];
    let set = |px: &mut Vec<u8>, x: i32, y: i32, c: [u8; 4]| {
        if x >= 0 && y >= 0 && x < s && y < s {
            let i = ((y * s + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[c[2], c[1], c[0], c[3]]);
        }
    };
    let u = s as f32 / 16.0;
    let (l, t, r, b) = ((1.0 * u) as i32, (4.0 * u) as i32, (13.0 * u) as i32, (12.0 * u) as i32);
    let dark = [0x20, 0x20, 0x20, 255];
    let green = [0x4C, 0xD9, 0x64, 255];
    let white = [0xF0, 0xF0, 0xF0, 255];
    for y in t..b {
        for x in l..r {
            let edge = x == l || x == r - 1 || y == t || y == b - 1;
            let inner = x == l + 1 || x == r - 2 || y == t + 1 || y == b - 2;
            let fill = x < l + 2 + ((r - l - 4) * 7) / 10;
            set(
                &mut px,
                x,
                y,
                if edge {
                    dark
                } else if inner {
                    white
                } else if fill {
                    green
                } else {
                    white
                },
            );
        }
    }
    for y in (t + (b - t) / 4)..(b - (b - t) / 4) {
        for x in r..(r + (2.0 * u) as i32) {
            set(&mut px, x, y, dark);
        }
    }
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = s;
        bmi.bmiHeader.biHeight = -s;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), dc);
        if color.is_null() || bits.is_null() {
            return LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask_bits = vec![0u8; (((s + 15) / 16) * 2 * s) as usize];
        let mask = CreateBitmap(s, s, 1, 1, mask_bits.as_ptr() as *const _);
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(color);
        DeleteObject(mask);
        if icon.is_null() {
            LoadIconW(std::ptr::null_mut(), IDI_APPLICATION)
        } else {
            icon
        }
    }
}

fn set_wstr(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn autostart_enabled() -> bool {
    let k = wide(RUN_KEY);
    let n = wide(RUN_NAME);
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) == ERROR_SUCCESS
    }
}

fn set_autostart(on: bool) -> Result<(), String> {
    let k = wide(RUN_KEY);
    let n = wide(RUN_NAME);
    let r = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let v = wide(&format!("\"{}\"", exe.display()));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                n.as_ptr(),
                REG_SZ,
                v.as_ptr() as *const _,
                (v.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) }
    };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry error {}", r))
    }
}

fn fmt_reading(b: Option<&crate::hidpp::Battery>) -> String {
    match b {
        Some(b) => format!(
            "{}%{}{}",
            b.percent,
            if b.approx { " (approx.)" } else { "" },
            if b.charging { " charging" } else { "" }
        ),
        None => "--".into(),
    }
}

impl App {
    fn log(&self, s: String) {
        logfile::append(&self.log_path, &[s]);
    }

    fn save_settings(&self) {
        if let Err(e) = std::fs::write(&self.settings_path, self.settings.serialize()) {
            self.log(format!("saving settings failed: {}", e));
        }
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = self.icon;
        let tip = format!(
            "Battery Strip\nMouse {}\nHeadset {}",
            fmt_reading(self.readings.mouse.as_ref()),
            fmt_reading(self.readings.headset.as_ref())
        );
        set_wstr(&mut nid.szTip, &tip);
        nid
    }

    fn add_tray(&self) {
        let nid = self.tray_data();
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
                Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
    }

    fn update_tray(&self) {
        let nid = self.tray_data();
        unsafe {
            if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
                Shell_NotifyIconW(NIM_ADD, &nid);
            }
        }
    }

    fn strip_for(&mut self, name: &str) -> Option<usize> {
        if let Some(i) = self.strips.iter().position(|s| s.name == name) {
            return Some(i);
        }
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let cls = wide("BatteryStripBar");
        let title = wide("Battery Strip");
        let h = unsafe {
            CreateWindowExW(
                ex,
                cls.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.hinst,
                std::ptr::null(),
            )
        };
        if h.is_null() {
            self.log(format!("CreateWindowEx failed: {}", unsafe { GetLastError() }));
            return None;
        }
        self.strips.push(Strip {
            name: name.to_string(),
            hwnd: h,
            shown: false,
            key: String::new(),
        });
        Some(self.strips.len() - 1)
    }

    fn hide(&mut self, i: usize) {
        let s = &mut self.strips[i];
        if s.shown {
            unsafe { ShowWindow(s.hwnd, SW_HIDE) };
            s.shown = false;
        }
    }

    /// Re-place / re-render every strip. Cheap enough to run every tick.
    fn refresh(&mut self) {
        let mons = monitors();
        let tbs = taskbars();
        let own: Vec<HWND> = self.strips.iter().map(|s| s.hwnd).collect();
        let (fs_mon, fs_all) = fullscreen(&own);
        self.monitors = mons
            .iter()
            .map(|m| (m.name.clone(), self.settings.monitor_enabled(&m.name)))
            .collect();
        // Drop strips of monitors that disappeared.
        let mut k = 0;
        while k < self.strips.len() {
            if mons.iter().any(|m| m.name == self.strips[k].name) {
                k += 1;
            } else {
                let s = self.strips.remove(k);
                unsafe { DestroyWindow(s.hwnd) };
            }
        }
        let lines = display::lines(
            self.settings.show_mouse,
            self.readings.mouse.as_ref(),
            self.settings.show_headset,
            self.readings.headset.as_ref(),
        );
        for m in &mons {
            let tb = tbs.iter().find(|t| t.monitor == m.handle);
            let want = self.settings.monitor_enabled(&m.name)
                && tb.is_some()
                && !fs_all
                && fs_mon != Some(m.handle)
                && !lines.is_empty()
                && self.font.is_some();
            let Some(i) = self.strip_for(&m.name) else {
                continue;
            };
            if !want {
                self.hide(i);
                continue;
            }
            let tb = tb.unwrap();
            let Some(vis) = layout::visible_taskbar(&tb.rect, &m.rect) else {
                self.hide(i);
                continue;
            };
            let horizontal = layout::edge(&vis, &m.rect).horizontal();
            let thickness = if horizontal { vis.h() } else { vis.w() };
            let px = layout::font_px(thickness, lines.len());
            let box_h = if horizontal {
                thickness as u32
            } else {
                (px * 1.4 * lines.len() as f32).ceil() as u32 + 4
            };
            let font = self.font.as_ref().unwrap();
            let canvas = render::render_strip(font, &lines, px, box_h, layout::outline_px(m.dpi));
            let off = (
                layout::scale(self.settings.fallback_offset_x, m.dpi),
                layout::scale(self.settings.fallback_offset_y, m.dpi),
            );
            let Some(p) = layout::place(
                &tb.rect,
                &m.rect,
                tb.tray.as_ref(),
                canvas.w as i32,
                canvas.h as i32,
                layout::scale(6, m.dpi),
                off,
            ) else {
                self.hide(i);
                continue;
            };
            let key = format!("{:?}|{:?}|{}", lines, p.rect, m.dpi);
            let place_key = format!("{:?}|{}", p.rect, p.used_tray);
            if !self.strips[i].key.contains(&place_key) {
                let how = if p.used_tray {
                    "left of tray area"
                } else {
                    "fallback offset (tray area not found)"
                };
                self.log(format!(
                    "{}: strip at {:?}, {}, taskbar {:?}, dpi {}",
                    m.name, p.rect, how, tb.rect, m.dpi
                ));
            }
            let key = format!("{}|{}", key, place_key);
            let s = &mut self.strips[i];
            if s.key != key || !s.shown {
                // Placement may clip the canvas; keep the canvas size.
                if present(s.hwnd, &canvas, p.rect.l, p.rect.t) {
                    s.key = key;
                } else {
                    s.key.clear();
                }
                if !s.shown {
                    unsafe { ShowWindow(s.hwnd, SW_SHOWNOACTIVATE) };
                    s.shown = true;
                }
            }
            // Stay above the taskbar in the topmost band.
            unsafe {
                SetWindowPos(
                    s.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                )
            };
        }
    }

    fn take_readings(&mut self) {
        let r = match self.shared.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        };
        self.readings = r;
        self.update_tray();
        self.refresh();
    }

    fn menu_items(&self) -> Vec<(u32, String, bool)> {
        let mut v = Vec::new();
        for (i, (name, on)) in self.monitors.iter().enumerate() {
            v.push((
                CMD_MONITOR0 + i as u32,
                format!("Strip on monitor {} ({})", i + 1, name.trim_start_matches(r"\\.\")),
                *on,
            ));
        }
        v.push((0, String::new(), false));
        v.push((
            CMD_MOUSE,
            format!("Show mouse  (M {})", fmt_reading(self.readings.mouse.as_ref())),
            self.settings.show_mouse,
        ));
        v.push((
            CMD_HEADSET,
            format!("Show headset  (H {})", fmt_reading(self.readings.headset.as_ref())),
            self.settings.show_headset,
        ));
        v.push((0, String::new(), false));
        v.push((CMD_AUTOSTART, "Start with Windows".into(), autostart_enabled()));
        v.push((CMD_REFRESH, "Refresh now".into(), false));
        v.push((CMD_PROBE, "Run probe (writes probe.txt)".into(), false));
        v.push((CMD_LOGDIR, "Open log folder".into(), false));
        v.push((0, String::new(), false));
        v.push((CMD_EXIT, "Exit".into(), false));
        v
    }

    /// Returns true when the app should exit.
    fn command(&mut self, cmd: u32) -> bool {
        match cmd {
            CMD_MOUSE => {
                self.settings.show_mouse = !self.settings.show_mouse;
                self.save_settings();
            }
            CMD_HEADSET => {
                self.settings.show_headset = !self.settings.show_headset;
                self.save_settings();
            }
            CMD_AUTOSTART => {
                if let Err(e) = set_autostart(!autostart_enabled()) {
                    self.log(format!("autostart change failed: {}", e));
                }
            }
            CMD_REFRESH => {
                let _ = self.wake.send(());
            }
            CMD_PROBE => {
                if let Ok(exe) = std::env::current_exe() {
                    if let Err(e) = std::process::Command::new(exe).arg("--probe").spawn() {
                        self.log(format!("starting probe failed: {}", e));
                    }
                }
            }
            CMD_LOGDIR => {
                let dir = data_dir();
                let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
            }
            CMD_EXIT => return true,
            c if c >= CMD_MONITOR0 => {
                if let Some((name, _)) = self.monitors.get((c - CMD_MONITOR0) as usize).cloned() {
                    self.settings.toggle_monitor(&name);
                    self.save_settings();
                }
            }
            _ => {}
        }
        self.refresh();
        false
    }
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

fn show_menu(h: HWND) {
    let Some(items) = with_app(|a| a.menu_items()) else {
        return;
    };
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for (id, text, checked) in &items {
            if *id == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            } else {
                let t = wide(text);
                AppendMenuW(
                    menu,
                    MF_STRING | if *checked { MF_CHECKED } else { MF_UNCHECKED },
                    *id as usize,
                    t.as_ptr(),
                );
            }
        }
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(h);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            h,
            std::ptr::null(),
        );
        PostMessageW(h, WM_NULL, 0, 0);
        DestroyMenu(menu);
        if cmd > 0 && with_app(|a| a.command(cmd as u32)) == Some(true) {
            DestroyWindow(h);
        }
    }
}

unsafe extern "system" fn main_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let tbc = TASKBAR_CREATED.load(Ordering::SeqCst);
    match msg {
        WM_APP_TRAY => {
            let ev = (l as u32) & 0xFFFF;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP || ev == WM_CONTEXTMENU {
                show_menu(h);
            }
            0
        }
        WM_APP_BATTERY => {
            if with_app(|a| a.take_readings()).is_none() {
                PostMessageW(h, WM_APP_BATTERY, 0, 0);
            }
            0
        }
        WM_TIMER | WM_APP_RELAYOUT | WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            with_app(|a| a.refresh());
            if msg == WM_TIMER || msg == WM_APP_RELAYOUT {
                0
            } else {
                DefWindowProcW(h, msg, w, l)
            }
        }
        WM_DESTROY => {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = h;
            nid.uID = 1;
            Shell_NotifyIconW(NIM_DELETE, &nid);
            PostQuitMessage(0);
            0
        }
        m if tbc != 0 && m == tbc => {
            // Explorer restarted: re-add tray icon, re-place strips.
            with_app(|a| {
                a.log("Explorer restarted (TaskbarCreated)".into());
                a.add_tray();
                for s in &mut a.strips {
                    s.key.clear();
                }
                a.refresh();
            });
            0
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

unsafe extern "system" fn strip_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            PostMessageW(main_hwnd(), WM_APP_RELAYOUT, 0, 0);
            0
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

unsafe extern "system" fn on_foreground(_h: HWINEVENTHOOK, _ev: u32, _w: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
    PostMessageW(main_hwnd(), WM_APP_RELAYOUT, 0, 0);
}

fn spawn_poller(hwnd: HWND, shared: Arc<Mutex<Readings>>, log_path: PathBuf, rx: mpsc::Receiver<()>) {
    let hwnd_val = hwnd as isize;
    std::thread::spawn(move || {
        let mut poller = Poller::default();
        let mut last: Option<Readings> = None;
        loop {
            let mut log = Vec::new();
            let publish = |r: Readings| {
                match shared.lock() {
                    Ok(mut g) => *g = r,
                    Err(p) => *p.into_inner() = r,
                }
                unsafe { PostMessageW(hwnd_val as HWND, WM_APP_BATTERY, 0, 0) };
            };
            let infos = Poller::enumerate(&mut WinBackend, &mut log);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &infos {
                Some(i) => poller.poll_hidpp(&mut WinBackend, i, &mut log),
                None => Readings::default(),
            }));
            let mut readings = match r {
                Ok(r) => r,
                Err(_) => {
                    log.push("poll failed unexpectedly (panic); will retry".into());
                    poller = Poller::default();
                    Readings::default()
                }
            };
            if let (None, Some(infos)) = (readings.headset, &infos) {
                // Mouse first (keeping the last headset value), so a silent
                // headset never delays it; then the bounded headset request.
                publish(Readings {
                    headset: last.and_then(|l| l.headset),
                    ..readings
                });
                let h = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    poller.poll_headset(&mut WinBackend, infos, &mut log)
                }));
                readings.headset = h.unwrap_or_else(|_| {
                    log.push("headset poll failed unexpectedly (panic); will retry".into());
                    poller.centurion = None;
                    None
                });
            }
            if last != Some(readings) {
                log.push(format!(
                    "mouse {} | headset {}",
                    fmt_reading(readings.mouse.as_ref()),
                    fmt_reading(readings.headset.as_ref())
                ));
                last = Some(readings);
            }
            logfile::append(&log_path, &log);
            publish(readings);
            match rx.recv_timeout(POLL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

pub fn run() -> i32 {
    unsafe {
        let mname = wide("Local\\BatteryStrip.SingleInstance");
        let mutex = CreateMutexW(std::ptr::null(), 0, mname.as_ptr());
        if !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            return 0;
        }
        let dir = data_dir();
        let log_path = dir.join("log.txt");
        let settings_path = dir.join("settings.ini");
        let settings = std::fs::read_to_string(&settings_path)
            .map(|t| Settings::parse(&t))
            .unwrap_or_default();
        logfile::append(&log_path, &[format!("Battery Strip {} started", env!("CARGO_PKG_VERSION"))]);
        let font = match render::load_font() {
            Ok(f) => Some(f),
            Err(e) => {
                logfile::append(&log_path, &[format!("font: {}", e)]);
                None
            }
        };

        let hinst = GetModuleHandleW(std::ptr::null());
        let main_cls = wide("BatteryStripMain");
        let strip_cls = wide("BatteryStripBar");
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(main_proc);
        wc.hInstance = hinst;
        wc.lpszClassName = main_cls.as_ptr();
        RegisterClassExW(&wc);
        let mut wc2: WNDCLASSEXW = std::mem::zeroed();
        wc2.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc2.lpfnWndProc = Some(strip_proc);
        wc2.hInstance = hinst;
        wc2.lpszClassName = strip_cls.as_ptr();
        RegisterClassExW(&wc2);

        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::SeqCst);
        let title = wide("Battery Strip");
        // Hidden top-level window (not message-only: it must receive the
        // TaskbarCreated / WM_DISPLAYCHANGE broadcasts).
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            main_cls.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            logfile::append(&log_path, &[format!("main window creation failed: {}", GetLastError())]);
            return 1;
        }
        MAIN_HWND.store(hwnd as isize, Ordering::SeqCst);

        let shared = Arc::new(Mutex::new(Readings::default()));
        let (tx, rx) = mpsc::channel();
        let app = App {
            hwnd,
            hinst,
            font,
            settings,
            settings_path,
            log_path: log_path.clone(),
            shared: shared.clone(),
            wake: tx,
            readings: Readings::default(),
            strips: Vec::new(),
            monitors: Vec::new(),
            icon: make_icon(),
        };
        app.add_tray();
        APP.with(|c| *c.borrow_mut() = Some(app));
        with_app(|a| a.refresh());

        SetTimer(hwnd, TIMER_TICK, TICK_MS, None);
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        spawn_poller(hwnd, shared, log_path, rx);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|c| {
            if let Ok(mut g) = c.try_borrow_mut() {
                if let Some(a) = g.take() {
                    a.log("exit".into());
                    for s in &a.strips {
                        DestroyWindow(s.hwnd);
                    }
                }
            }
        });
        0
    }
}
