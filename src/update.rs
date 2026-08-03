//! Self-update: finds the newest release of this tool on GitHub, downloads
//! the binary for the current platform, verifies it (SHA-256 when the
//! release ships a checksum, plus a format magic check), swaps it into
//! place, and restarts the systemd service.
//!
//! curl is reused (install.sh already requires it) so the binary keeps
//! zero Rust dependencies. Overridable via environment, same knobs as
//! install.sh:
//!   SYSTEMHOG_REPO       owner/repo       (default: p1n2o/systemhog)
//!   SYSTEMHOG_VERSION    pin a tag, no leading 'v' (default: latest)
//!   SYSTEMHOG_BASE_URL   download base URL for mirrors (no version check
//!                        is possible, so pair it with SYSTEMHOG_VERSION)

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config;
use crate::sys;

const DEFAULT_REPO: &str = "p1n2o/systemhog";

/// Target triple + whether the binary carries a `.exe` suffix, matching
/// install.sh and the release workflow asset naming exactly.
fn triple_for(os: &str, arch: &str) -> Result<(String, bool), String> {
    let (triple, exe) = match os {
        "linux" => match arch {
            "x86_64" | "aarch64" | "i686" => (format!("{arch}-unknown-linux-musl"), false),
            "arm" => ("armv7-unknown-linux-musleabihf".to_string(), false),
            "riscv64" => ("riscv64gc-unknown-linux-musl".to_string(), false),
            _ => return Err(format!("unsupported architecture: {arch}")),
        },
        "macos" => match arch {
            "x86_64" | "aarch64" => (format!("{arch}-apple-darwin"), false),
            _ => return Err(format!("unsupported architecture: {arch}")),
        },
        "windows" => match arch {
            "x86_64" | "aarch64" => (format!("{arch}-pc-windows-msvc"), true),
            _ => return Err(format!("unsupported architecture: {arch}")),
        },
        other => return Err(format!("unsupported OS: {other}")),
    };
    Ok((triple, exe))
}

fn asset_name() -> Result<String, String> {
    let (triple, exe) = triple_for(std::env::consts::OS, std::env::consts::ARCH)?;
    Ok(format!("systemhog-{triple}{}", if exe { ".exe" } else { "" }))
}

/// "v0.2.0" / "0.2" -> (0, 2, 0). Pre-release suffixes fail loudly rather
/// than silently mis-comparing.
fn parse_version(s: &str) -> Result<(u32, u32, u32), String> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    let mut next = |n: usize| -> Result<u32, String> {
        parts
            .next()
            .unwrap_or("0")
            .parse()
            .map_err(|_| format!("cannot parse version {s:?} (component {n})"))
    };
    Ok((next(1)?, next(2)?, next(3)?))
}

/// Latest release tag (no leading 'v') by following the /releases/latest
/// redirect to the asset's canonical URL, which embeds the tag. No GitHub
/// API involved, so no rate limits.
fn find_latest(repo: &str, asset: &str) -> Result<String, String> {
    let url = format!("https://github.com/{repo}/releases/latest/download/{asset}");
    let out = Command::new("curl")
        .args(["-fsSI", "-o", "/dev/null", "-w", "%{redirect_url}"])
        .arg(&url)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("no releases found for {repo} (curl exit {})", out.status));
    }
    let redir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let tag = redir
        .split("/releases/download/")
        .nth(1)
        .and_then(|p| p.split('/').next())
        .ok_or_else(|| format!("cannot determine latest release for {repo}"))?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    parse_version(&version)?;
    Ok(version)
}

/// SHA-256 of a file, or None when no tool is available. First tool that
/// works: sha256sum (Linux), shasum (macOS), certutil (Windows).
fn sha256_of(path: &Path) -> Option<String> {
    let mut tools: Vec<Vec<String>> = vec![
        vec!["sha256sum".into()],
        vec!["shasum".into(), "-a".into(), "256".into()],
        vec!["certutil".into(), "-hashfile".into()],
    ];
    for tool in &mut tools {
        let mut cmd = Command::new(&tool[0]);
        for a in tool.iter().skip(1) {
            cmd.arg(a);
        }
        cmd.arg(path);
        if let Ok(out) = cmd.output() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(h) = text.split_whitespace().find(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())) {
                return Some(h.to_string());
            }
        }
    }
    None
}

/// First whitespace-separated token of a file (the .sha256 asset layout:
/// "<hex>  systemhog-<triple>").
fn first_token(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.split_whitespace().next().map(str::to_string))
}

fn magic_ok(path: &Path) -> bool {
    let head = std::fs::read(path).ok().map(|b| b.into_iter().take(4).collect::<Vec<u8>>());
    match std::env::consts::OS {
        "linux" => head.as_deref() == Some(&[0x7f, b'E', b'L', b'F']),
        "macos" => head.as_deref() == Some(&[0xcf, 0xfa, 0xed, 0xfe]),
        "windows" => head.as_deref().is_some_and(|h| h.starts_with(&[b'M', b'Z'])),
        _ => true,
    }
}

fn files_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Removes the file on drop — keeps temp files from lingering on any
/// early-return path.
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn curl_fetch(url: &str, dest: &Path) -> Result<(), String> {
    let st = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("download failed (curl exit {st}): {url}"))
    }
}

pub fn run(cfg_path: Option<&Path>, check_only: bool) -> i32 {
    let repo = std::env::var("SYSTEMHOG_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let pinned = std::env::var("SYSTEMHOG_VERSION").ok().filter(|s| !s.is_empty());
    let mirror = std::env::var("SYSTEMHOG_BASE_URL").ok().filter(|s| !s.is_empty());

    let asset = match asset_name() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let current = env!("CARGO_PKG_VERSION");
    println!("  current version : v{current}");

    // Which version are we moving to, and from where?
    let (target_label, download_url, target_ver) = if let Some(v) = pinned.clone() {
        let ver = match parse_version(&v) {
            Ok(ver) => ver,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        (
            format!("v{v}"),
            format!("https://github.com/{repo}/releases/download/v{v}/{asset}"),
            Some(ver),
        )
    } else if let Some(base) = mirror.clone() {
        if check_only {
            eprintln!("error: cannot check for updates with SYSTEMHOG_BASE_URL set");
            return 2;
        }
        (
            "(mirror)".to_string(),
            format!("{}/{}", base.trim_end_matches('/'), asset),
            None,
        )
    } else {
        let v = match find_latest(&repo, &asset) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let ver = parse_version(&v).unwrap();
        (
            format!("v{v}"),
            format!("https://github.com/{repo}/releases/latest/download/{asset}"),
            Some(ver),
        )
    };

    // Decide whether anything needs to happen.
    let cur = parse_version(current).ok();
    match (cur, target_ver) {
        (Some(c), Some(t)) if c == t => {
            println!("  already up to date (v{current})");
            return 0;
        }
        (Some(c), Some(t)) if t < c => {
            println!("  target version  : {}", fmt_version(t));
            println!("  note: installed v{current} is newer than the release; downgrading");
        }
        (_, Some(t)) => println!("  target version  : {}", fmt_version(t)),
        (_, None) => println!("  target          : {target_label}"),
    }
    if check_only {
        match (cur, target_ver) {
            (Some(c), Some(t)) if t < c => {
                println!("  nothing to update (installed {} is newer)", fmt_version(c));
                return 0;
            }
            _ => {
                println!(
                    "  update available: run `sudo systemhog update` (or `systemhog self-update`)"
                );
                return 1;
            }
        }
    }

    // Download next to the running binary (same filesystem, so the final
    // rename cannot cross devices) and verify.
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: cannot locate this binary: {e}");
            return 1;
        }
    };
    let dir = exe.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let tmp = dir.join(format!(".{}.update.{}", exe.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let _tmp_guard = TempFile(tmp.clone());

    println!("  downloading     : {download_url}");
    if let Err(e) = curl_fetch(&download_url, &tmp) {
        eprintln!("error: {e}");
        eprintln!("hint: if this binary is root-owned, run with sudo");
        return 1;
    }

    // Checksum: strict when the release ships one, warn otherwise (mirrors
    // install.sh's policy).
    let sum_url = format!("{download_url}.sha256");
    let sum_tmp = dir.join(format!(".{}.sha256.{}", exe.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    if curl_fetch(&sum_url, &sum_tmp).is_ok() {
        let _sum_guard = TempFile(sum_tmp.clone());
        match (first_token(&sum_tmp), sha256_of(&tmp)) {
            (Some(expected), Some(got)) if expected == got => {
                println!("  checksum        : verified ({got})");
            }
            (Some(expected), Some(got)) => {
                eprintln!("error: checksum mismatch (got {got}, expected {expected})");
                return 1;
            }
            (Some(_), None) => {
                println!("  warning: no sha256 tool available; skipping verification");
            }
            (None, _) => {
                eprintln!("error: malformed checksum asset {sum_url}");
                return 1;
            }
        }
    } else {
        println!("  warning: no checksum asset for this release; skipping verification");
    }

    if !magic_ok(&tmp) {
        eprintln!("error: downloaded file is not a valid binary for this platform");
        return 1;
    }

    if files_equal(&tmp, &exe) {
        println!("  binary unchanged; nothing to replace");
        return 0;
    }

    println!("  replacing       : {}", exe.display());
    // curl creates the temp file 0644; carry the running binary's
    // permissions (especially the exec bit) over to the replacement.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&exe) {
            let _ = std::fs::set_permissions(
                &tmp,
                std::fs::Permissions::from_mode(meta.permissions().mode()),
            );
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &exe) {
        eprintln!("error: cannot replace {}: {e}", exe.display());
        eprintln!("hint: run with sudo (the binary is likely root-owned), or stop running instances and retry");
        return 1;
    }

    // Restart the service when it was running; never surprise-start one.
    let mut restarted = false;
    if let Some(cfg_path) = cfg_path {
        match config::load(cfg_path) {
            Ok(cfg) if sys::systemd::available() => {
                if sys::systemd::is_active(&cfg.service_name) == "active" {
                    println!("  restarting service `{}`", cfg.service_name);
                    match sys::systemd::restart(&cfg.service_name) {
                        Ok(()) => restarted = true,
                        Err(e) => eprintln!("warning: service restart failed: {e}"),
                    }
                } else {
                    println!(
                        "  note: service `{}` is not running; not restarted",
                        cfg.service_name
                    );
                }
            }
            Ok(_) => println!("  note: systemd not available; service not restarted"),
            Err(_) => println!(
                "  note: no config found; service not restarted (run `systemhog init` first)"
            ),
        }
    }
    println!(
        "  updated to {target_label}{}",
        if restarted { " (service restarted)" } else { "" }
    );
    0
}

fn fmt_version(v: (u32, u32, u32)) -> String {
    format!("v{}.{}.{}", v.0, v.1, v.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("0.2.0"), Ok((0, 2, 0)));
        assert_eq!(parse_version("v10.4.2"), Ok((10, 4, 2)));
        assert_eq!(parse_version("0.2"), Ok((0, 2, 0)));
        assert!(parse_version("0.2.0-rc.1").is_err());
        assert!(parse_version("latest").is_err());
    }

    #[test]
    fn orders_versions() {
        assert!(parse_version("0.2.1").unwrap() > parse_version("0.2.0").unwrap());
        assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.9").unwrap());
        assert_eq!(parse_version("1.0.0").unwrap(), parse_version("v1.0.0").unwrap());
    }

    #[test]
    fn triples_match_release_assets() {
        assert_eq!(triple_for("linux", "x86_64"), Ok(("x86_64-unknown-linux-musl".into(), false)));
        assert_eq!(triple_for("linux", "arm"), Ok(("armv7-unknown-linux-musleabihf".into(), false)));
        assert_eq!(triple_for("linux", "riscv64"), Ok(("riscv64gc-unknown-linux-musl".into(), false)));
        assert_eq!(triple_for("macos", "aarch64"), Ok(("aarch64-apple-darwin".into(), false)));
        assert_eq!(triple_for("windows", "x86_64"), Ok(("x86_64-pc-windows-msvc".into(), true)));
        assert!(triple_for("linux", "mips").is_err());
        assert!(triple_for("plan9", "x86_64").is_err());
    }
}
