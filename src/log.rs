//! Minimal file + stderr logger with UTC timestamps and size-based rotation.
//! No chrono: the civil-date conversion is ~15 lines of pure integer math.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1 << 20; // rotate at 1 MiB
const ROTATED: &str = ".1"; // keep one rotated file

pub struct Logger {
    path: PathBuf,
    file: Option<File>,
}

impl Logger {
    pub fn new(path: &Path) -> Logger {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = OpenOptions::new().create(true).append(true).open(path).ok();
        if file.is_none() {
            eprintln!(
                "warning: cannot open log file {} (permissions?); logging to stderr only",
                path.display()
            );
        }
        Logger {
            path: path.to_path_buf(),
            file,
        }
    }

    fn rotate(&mut self) {
        let size = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);
        if size < MAX_BYTES {
            return;
        }
        self.file = None; // drop handle so rename works
        let old = PathBuf::from(format!("{}{}", self.path.display(), ROTATED));
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&self.path, &old);
        self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
    }

    pub fn log(&mut self, level: &str, msg: &str) {
        let line = format!("{} {} {}\n", ts_utc(), level, msg);
        self.rotate();
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
        // Also to stderr: visible in the foreground and in journald.
        eprint!("{line}");
    }

    pub fn info(&mut self, msg: &str) {
        self.log("INFO", msg);
    }

    pub fn warn(&mut self, msg: &str) {
        self.log("WARN", msg);
    }
}

fn ts_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian, UTC.
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn known_dates() {
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        assert_eq!(civil_from_days(11017), (2000, 3, 1)); // leap year
        assert_eq!(civil_from_days(20668), (2026, 8, 3));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
