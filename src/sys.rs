//! Platform plumbing: signals, single-instance locking, root detection,
//! and systemd service management.

use std::path::Path;
use std::sync::atomic::AtomicBool;

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::io::AsRawFd;
    use std::sync::atomic::Ordering;

    static STOP: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
        fn geteuid() -> u32;
        fn flock(fd: i32, operation: i32) -> i32;
    }

    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    extern "C" fn on_signal(_: i32) {
        STOP.store(true, Ordering::SeqCst);
    }

    pub fn stop_flag() -> &'static AtomicBool {
        &STOP
    }

    pub fn install_signal_handlers() {
        unsafe {
            signal(SIGINT, on_signal as *const () as usize);
            signal(SIGTERM, on_signal as *const () as usize);
        }
    }

    pub fn is_root() -> bool {
        unsafe { geteuid() == 0 }
    }

    pub fn uid() -> u32 {
        unsafe { geteuid() }
    }

    /// Exclusive, non-blocking flock. The returned File keeps the lock
    /// held for as long as it lives (dropped on process exit).
    pub fn acquire_lock(path: &Path) -> Result<Option<std::fs::File>, String> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("cannot open lock file {}: {e}", path.display()))?;
        let rc = unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc != 0 {
            return Err(format!(
                "another instance is already running (lock: {})",
                path.display()
            ));
        }
        Ok(Some(f))
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub fn stop_flag() -> &'static AtomicBool {
        static STOP: AtomicBool = AtomicBool::new(false);
        &STOP
    }

    pub fn install_signal_handlers() {}

    pub fn is_root() -> bool {
        false
    }

    pub fn uid() -> u32 {
        0
    }

    pub fn acquire_lock(_path: &Path) -> Result<Option<std::fs::File>, String> {
        Ok(None)
    }
}

pub use imp::{acquire_lock, install_signal_handlers, is_root, stop_flag, uid};

#[cfg(target_os = "linux")]
pub mod systemd {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    pub fn available() -> bool {
        Path::new("/run/systemd/system").is_dir()
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let out = Command::new("systemctl")
            .args(args)
            .output()
            .map_err(|e| format!("cannot run systemctl: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "`systemctl {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn unit_dir() -> PathBuf {
        if is_root() {
            PathBuf::from("/etc/systemd/system")
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .unwrap_or_else(|| PathBuf::from(".config"))
                .join("systemd")
                .join("user")
        }
    }

    fn unit_path(name: &str) -> PathBuf {
        unit_dir().join(format!("{name}.service"))
    }

    pub fn install(name: &str, bin: &Path, config: &Path) -> Result<(), String> {
        let user = !is_root();
        let dir = unit_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let unit = unit_path(name);
        let wanted = if user { "default.target" } else { "multi-user.target" };
        let body = format!(
            "[Unit]\nDescription={name} - CPU and RAM resource maintainer\nAfter=network.target\n\n\
             [Service]\nExecStart={} --config {}\nRestart=always\nRestartSec=5\n\n\
             [Install]\nWantedBy={}\n",
            bin.display(),
            config.display(),
            wanted
        );
        std::fs::write(&unit, body).map_err(|e| format!("cannot write {}: {e}", unit.display()))?;
        if user {
            systemctl(&["--user", "daemon-reload"])?;
            systemctl(&["--user", "enable", "--now", name])?;
        } else {
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "--now", name])?;
        }
        Ok(())
    }

    pub fn uninstall(name: &str) -> Result<(), String> {
        let user = !is_root();
        if user {
            let _ = systemctl(&["--user", "disable", "--now", name]);
        } else {
            let _ = systemctl(&["disable", "--now", name]);
        }
        let unit = unit_path(name);
        if unit.exists() {
            std::fs::remove_file(&unit)
                .map_err(|e| format!("cannot remove {}: {e}", unit.display()))?;
        }
        if user {
            systemctl(&["--user", "daemon-reload"])
        } else {
            systemctl(&["daemon-reload"])
        }
    }

    pub fn is_active(name: &str) -> String {
        let args: Vec<String> = if is_root() {
            vec!["is-active".into(), name.into()]
        } else {
            vec!["--user".into(), "is-active".into(), name.into()]
        };
        Command::new("systemctl")
            .args(&args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".into())
    }

    pub fn restart(name: &str) -> Result<(), String> {
        if is_root() {
            systemctl(&["restart", name])
        } else {
            systemctl(&["--user", "restart", name])
        }
    }

    /// Whether the invoking user's user-manager lingers after logout
    /// (user services keep running at boot without a login). None when
    /// logind is unavailable or the answer is unknown.
    pub fn linger_enabled() -> Option<bool> {
        let user = std::env::var("USER").ok()?;
        let out = Command::new("loginctl")
            .args(["show-user", "-p", "Linger", &user])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .strip_prefix("Linger=")
            .map(|v| v == "yes")
    }
}

#[cfg(not(target_os = "linux"))]
pub mod systemd {
    use std::path::Path;

    pub fn available() -> bool {
        false
    }

    pub fn install(_name: &str, _bin: &Path, _config: &Path) -> Result<(), String> {
        Err("systemd services are only supported on Linux".into())
    }

    pub fn uninstall(_name: &str) -> Result<(), String> {
        Err("systemd services are only supported on Linux".into())
    }

    pub fn is_active(_name: &str) -> String {
        "n/a".into()
    }

    pub fn restart(_name: &str) -> Result<(), String> {
        Err("systemd services are only supported on Linux".into())
    }

    pub fn linger_enabled() -> Option<bool> {
        None
    }
}
