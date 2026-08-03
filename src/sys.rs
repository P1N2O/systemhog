//! Platform plumbing: signals, single-instance locking, root detection,
//! and systemd service management. Linux only.

use std::path::Path;
use std::sync::atomic::AtomicBool;

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

pub use imp::{acquire_lock, install_signal_handlers, is_root, stop_flag, uid};

/// systemd service management, system scope only. All operations require
/// root; every path is the standard system one, so the same binary,
/// config and log locations are used whether invoked by root or not.
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

    fn unit_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/etc/systemd/system/{name}.service"))
    }

    pub fn install(name: &str, bin: &Path, config: &Path) -> Result<(), String> {
        if !is_root() {
            return Err("service installation requires root: run with sudo".into());
        }
        std::fs::create_dir_all("/etc/systemd/system")
            .map_err(|e| format!("cannot create /etc/systemd/system: {e}"))?;
        let unit = unit_path(name);
        let body = format!(
            "[Unit]\nDescription={name} - CPU and RAM resource maintainer\nAfter=network.target\n\n\
             [Service]\nExecStart={} --config {}\nRestart=always\nRestartSec=5\n\n\
             [Install]\nWantedBy=multi-user.target\n",
            bin.display(),
            config.display()
        );
        std::fs::write(&unit, body).map_err(|e| format!("cannot write {}: {e}", unit.display()))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", name])?;
        Ok(())
    }

    pub fn uninstall(name: &str) -> Result<(), String> {
        let _ = systemctl(&["disable", "--now", name]);
        let unit = unit_path(name);
        if unit.exists() {
            std::fs::remove_file(&unit)
                .map_err(|e| format!("cannot remove {}: {e}", unit.display()))?;
        }
        systemctl(&["daemon-reload"])
    }

    pub fn is_active(name: &str) -> String {
        Command::new("systemctl")
            .args(["is-active", name])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".into())
    }

    pub fn restart(name: &str) -> Result<(), String> {
        systemctl(&["restart", name])
    }
}
