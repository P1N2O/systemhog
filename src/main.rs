mod cli;
mod config;
mod log;
mod metrics;
mod pump;
mod sys;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let rest = args.get(2..).unwrap_or(&[]);
    let code = match cmd {
        "" | "run" => cli::run(rest),
        "init" => cli::init(rest),
        "install" => cli::install(rest),
        "uninstall" => cli::uninstall(rest),
        "status" => cli::status(rest),
        "version" | "-V" | "--version" => {
            println!("systemhog {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "help" | "-h" | "--help" => {
            cli::help();
            0
        }
        // Bare invocation with options: `systemhog --config /path` runs the
        // maintainer with those options. `--help`/`--version` are handled
        // above, so this branch only sees run-related options.
        other if other.starts_with('-') => cli::run(args.get(1..).unwrap_or(&[])),
        other => {
            eprintln!("systemhog: unknown command '{other}'");
            eprintln!();
            cli::help();
            2
        }
    };
    std::process::exit(code);
}
