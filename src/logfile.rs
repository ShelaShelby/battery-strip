//! Size-limited log file with one rotation, UTC timestamps.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_LOG: u64 = 200 * 1024;

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn format_utc(secs: i64) -> String {
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}Z", y, mo, d, s / 3600, (s / 60) % 60, s % 60)
}

pub fn now_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

/// Rotate `path` to `<path>.old` when it exceeds `max` bytes.
pub fn rotate_if_needed(path: &Path, max: u64) {
    if let Ok(m) = std::fs::metadata(path) {
        if m.len() > max {
            let old = old_path(path);
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(path, &old);
        }
    }
}

pub fn old_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

pub fn append(path: &Path, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    rotate_if_needed(path, MAX_LOG);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = now_utc();
        for l in lines {
            let _ = writeln!(f, "[{}] {}", ts, l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(format_utc(1_792_665_296), "2026-10-22 10:34:56Z");
    }

    #[test]
    fn rotation() {
        let dir = std::env::temp_dir().join(format!("bs-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("log.txt");
        std::fs::write(&p, vec![b'x'; 300]).unwrap();
        rotate_if_needed(&p, 200);
        assert!(!p.exists());
        assert_eq!(std::fs::metadata(old_path(&p)).unwrap().len(), 300);
        append(&p, &["hello".to_string()]);
        assert!(std::fs::read_to_string(&p).unwrap().contains("] hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
