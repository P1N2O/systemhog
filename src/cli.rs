//! Command-line interface: init (interactive), run, install, uninstall,
//! status, help. No clap — argument surface is tiny and hand-rolled
//! parsing keeps the binary at its smallest.

use crate::config::{self, Config};
use crate::log::Logger;
use crate::metrics;
use crate::sys;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Load the config, printing a consistent error + hint on failure. The
/// hint adapts to the scope: root installs need sudo, user installs don't.
fn load_config(cfg_path: &Path) -> Result<Config, i32> {
    match config::load(cfg_path) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            eprintln!("error: {e}");
            let init = if sys::is_root() { "sudo systemhog init" } else { "systemhog init" };
            eprintln!("hint: run `{init}` to create a configuration");
            Err(1)
        }
    }
}

fn parse_common(args: &[String]) -> (Option<PathBuf>, bool) {
    let mut path = None;
    let mut yes = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--config" {
            path = it.next().map(PathBuf::from);
        } else if let Some(v) = a.strip_prefix("--config=") {
            path = Some(PathBuf::from(v));
        } else if a == "--yes" {
            yes = true;
        } else {
            eprintln!("systemhog: ignoring unknown option {a:?}");
        }
    }
    (path, yes)
}

fn read_line() -> Option<String> {
    let mut s = String::new();
    match std::io::stdin().read_line(&mut s) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(s),
    }
}

/// Ask for input; empty input (or EOF) takes the default.
fn prompt(label: &str, default: &str, validate: &dyn Fn(&str) -> Result<String, String>) -> String {
    loop {
        print!("{label} [{default}]: ");
        let _ = std::io::stdout().flush();
        let input = read_line().map(|s| s.trim().to_string()).unwrap_or_default();
        let value = if input.is_empty() { default.to_string() } else { input };
        match validate(&value) {
            Ok(v) => return v,
            Err(e) => eprintln!("  {e}"),
        }
    }
}

fn confirm(label: &str, default_yes: bool) -> bool {
    print!("{label} [{}]: ", if default_yes { "Y/n" } else { "y/N" });
    let _ = std::io::stdout().flush();
    let ans = read_line().map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if ans.is_empty() {
        default_yes
    } else {
        ans == "y" || ans == "yes"
    }
}

fn num_range(s: &str, lo: u32, hi: u32, what: &str) -> Result<String, String> {
    let v: u32 = s
        .trim()
        .parse()
        .map_err(|_| format!("{what} must be a number, got {s:?}"))?;
    if !(lo..=hi).contains(&v) {
        return Err(format!("{what} must be {lo}..={hi}, got {v}"));
    }
    Ok(v.to_string())
}

pub fn init(args: &[String]) -> i32 {
    let (cfg_path, yes) = parse_common(args);
    let cfg_path = cfg_path.unwrap_or_else(config::default_config_path);
    println!("systemhog {} - interactive setup", env!("CARGO_PKG_VERSION"));
    println!();

    let defaults = Config::default();
    let mut cfg = defaults.clone();
    if !yes {
        cfg.service_name = prompt("Service name", &defaults.service_name, &|s| config::valid_name(s));
        cfg.cpu_min = prompt("Minimum CPU usage (%)", &defaults.cpu_min.to_string(), &|s| {
            num_range(s, 0, 98, "cpu min")
        })
        .parse()
        .unwrap();
        let min = cfg.cpu_min;
        cfg.cpu_max = prompt("Maximum CPU usage (%)", &defaults.cpu_max.to_string(), &|s| {
            let v: u32 = s
                .trim()
                .parse()
                .map_err(|_| format!("cpu max must be a number, got {s:?}"))?;
            if v <= min {
                return Err(format!("cpu max must be above cpu min ({min})"));
            }
            if v > 100 {
                return Err("cpu max must be <= 100".into());
            }
            Ok(v.to_string())
        })
        .parse()
        .unwrap();
        cfg.ram_target = prompt(
            "Target RAM usage (%)",
            &defaults.ram_target.to_string(),
            &|s| num_range(s, 1, 90, "ram target"),
        )
        .parse()
        .unwrap();
        cfg.check_interval = prompt(
            "Check interval (seconds)",
            &defaults.check_interval.to_string(),
            &|s| num_range(s, 1, 3600, "interval"),
        )
        .parse()
        .unwrap();
        let log_default = defaults.log_file.to_string_lossy().into_owned();
        cfg.log_file = PathBuf::from(prompt("Log file", &log_default, &|s| {
            if s.trim().is_empty() {
                Err("log file must not be empty".into())
            } else {
                Ok(s.trim().to_string())
            }
        }));

        println!();
        println!("Summary");
        println!("  service name : {}", cfg.service_name);
        println!("  cpu          : {}% .. {}%", cfg.cpu_min, cfg.cpu_max);
        println!("  ram target   : {}%", cfg.ram_target);
        println!("  interval     : {}s", cfg.check_interval);
        println!("  log file     : {}", cfg.log_file.display());
        println!("  config file  : {}", cfg_path.display());
        println!();
        if !confirm("Write configuration?", true) {
            println!("aborted, nothing written");
            return 1;
        }
    }

    if let Err(e) = config::write(&cfg_path, &cfg) {
        eprintln!("error: {e}");
        let init = if sys::is_root() { "sudo systemhog init" } else { "systemhog init" };
        eprintln!(
            "hint: the config lives at {}; run `{init}` to write it",
            cfg_path.display()
        );
        return 1;
    }
    println!("configuration written to {}", cfg_path.display());

    if !yes && confirm("Install and start as a systemd service now?", false) {
        return install(&[String::from("--config"), cfg_path.to_string_lossy().into_owned()]);
    }
    println!();
    println!("next steps:");
    println!("  run it now : systemhog --config {}", cfg_path.display());
    if sys::systemd::available() {
        let install = if sys::is_root() {
            "sudo systemhog install"
        } else {
            "systemhog install"
        };
        println!("  install    : {install} --config {}", cfg_path.display());
    }
    0
}

pub fn run(args: &[String]) -> i32 {
    let (cfg_path, _) = parse_common(args);
    let cfg_path = cfg_path.unwrap_or_else(config::default_config_path);
    let cfg = match load_config(&cfg_path) {
        Ok(c) => c,
        Err(code) => return code,
    };

    let lock_path = if sys::is_root() {
        PathBuf::from(format!("/run/systemhog-{}.lock", cfg.service_name))
    } else {
        std::env::temp_dir().join(format!(
            "systemhog-{}-{}.lock",
            cfg.service_name,
            sys::uid()
        ))
    };
    let _lock = match sys::acquire_lock(&lock_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let mut log = Logger::new(&cfg.log_file);
    sys::install_signal_handlers();
    log.info(&format!(
        "systemhog {} starting (config {})",
        env!("CARGO_PKG_VERSION"),
        cfg_path.display()
    ));
    let code = crate::pump::run(&cfg, &cfg_path, &mut log, sys::stop_flag());
    log.info("systemhog exiting");
    code
}

pub fn install(args: &[String]) -> i32 {
    let (cfg_path, _) = parse_common(args);
    let cfg_path = cfg_path.unwrap_or_else(config::default_config_path);
    let cfg = match load_config(&cfg_path) {
        Ok(c) => c,
        Err(code) => return code,
    };
    if !sys::systemd::available() {
        eprintln!("systemd not detected on this system");
        let bin = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "systemhog".into());
        eprintln!("run it at boot instead, e.g.: {bin} --config {}", cfg_path.display());
        return 1;
    }
    let bin = match std::env::current_exe() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot locate binary: {e}");
            return 1;
        }
    };
    match sys::systemd::install(&cfg.service_name, &bin, &cfg_path) {
        Ok(()) => {
            println!("service `{}` installed and started", cfg.service_name);
            println!("  binary : {}", bin.display());
            println!("  config : {}", cfg_path.display());
            println!("  logs   : journalctl -u {} -f", cfg.service_name);
            println!("           tail -f {}", cfg.log_file.display());
            if !sys::is_root() && sys::systemd::linger_enabled() != Some(true) {
                println!();
                println!("note: user services stop at logout; run `sudo loginctl enable-linger`");
                println!("      to keep `{}` running at boot without a login", cfg.service_name);
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

pub fn uninstall(args: &[String]) -> i32 {
    let (cfg_path, _) = parse_common(args);
    let cfg_path = cfg_path.unwrap_or_else(config::default_config_path);
    let cfg = match load_config(&cfg_path) {
        Ok(c) => c,
        Err(code) => return code,
    };
    if !sys::systemd::available() {
        eprintln!("systemd not detected on this system");
        return 1;
    }
    match sys::systemd::uninstall(&cfg.service_name) {
        Ok(()) => {
            println!("service `{}` stopped and removed", cfg.service_name);
            println!(
                "config left at {} (delete it with `rm` if you no longer need it)",
                cfg_path.display()
            );
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// Check for and apply a newer release of this binary. `self-update` is
/// an alias for the same command.
pub fn update(args: &[String]) -> i32 {
    let check_only = args.iter().any(|a| a == "--check");
    let rest: Vec<String> = args.iter().filter(|a| *a != "--check").cloned().collect();
    let (cfg_path, _) = parse_common(&rest);
    let cfg_path = cfg_path.unwrap_or_else(config::default_config_path);
    crate::update::run(Some(&cfg_path), check_only)
}

pub fn status(args: &[String]) -> i32 {
    let (cfg_path, _) = parse_common(args);
    let cfg_path = cfg_path.unwrap_or_else(config::default_config_path);
    let cfg = match load_config(&cfg_path) {
        Ok(c) => c,
        Err(code) => return code,
    };
    println!("service name : {}", cfg.service_name);
    println!("cpu          : {}% .. {}%", cfg.cpu_min, cfg.cpu_max);
    println!("ram target   : {}%", cfg.ram_target);
    println!("interval     : {}s", cfg.check_interval);
    println!("config file  : {}", cfg_path.display());
    println!("log file     : {}", cfg.log_file.display());
    if let Some(a) = metrics::cpu_sample() {
        std::thread::sleep(Duration::from_secs(1));
        if let Some(b) = metrics::cpu_sample() {
            println!("cpu usage    : {:.1}%", metrics::cpu_percent(&a, &b));
        }
    }
    if let Some(m) = metrics::mem_info() {
        let used = m.total_kb.saturating_sub(m.avail_kb);
        println!(
            "memory       : {:.1}% used ({} MiB of {} MiB)",
            used as f64 * 100.0 / m.total_kb as f64,
            used / 1024,
            m.total_kb / 1024
        );
    }
    if sys::systemd::available() {
        println!("service      : {} ({})", cfg.service_name, sys::systemd::is_active(&cfg.service_name));
    }
    0
}

pub fn help() {
    println!("systemhog {} - CPU and RAM resource maintainer", env!("CARGO_PKG_VERSION"));
    println!();
    println!("USAGE");
    println!("  systemhog [command] [options]");
    println!();
    println!("  With no command, the maintainer runs (same as `systemhog run`).");
    println!();
    println!("COMMANDS");
    println!("  init        interactive setup (service name, cpu, ram, interval)");
    println!("  run         run the maintainer in the foreground (default)");
    println!("  install     create, enable and start the systemd service");
    println!("  uninstall   stop, disable and remove the systemd service");
    println!("  status      show config, current usage and service state");
    println!("  update      check for and apply a newer release of this binary");
    println!("  self-update alias for update");
    println!("  version     print version information");
    println!("  help        show this help");
    println!();
    println!("OPTIONS");
    println!("  --config PATH   use an alternate configuration file");
    println!("  --yes           init: skip prompts and use defaults");
    println!("  --check         update: report only; exit 1 when an update is available");
}
