# systemhog

A CPU and RAM resource maintainer that holds system usage at your
configured levels — for load testing, benchmarks, or just keeping a
machine busy. Pure Rust, zero runtime dependencies, one small static
binary per platform.

## Features

- **Fine-grained CPU control** — one worker thread per core with an
  adjustable duty cycle; a proportional controller keeps total usage inside
  the configured band without the oscillation of all-or-nothing loaders
- **RAM steering** — 10 MiB blocks are added while system usage sits below
  the target and freed when it climbs above it, yielding memory back to
  other workloads
- **Interactive setup** — `systemhog init` wizard: service name, CPU band,
  RAM target, check interval, log file — every answer validated
- **Zero dependencies** — reads `/proc` directly on Linux; fully static
  musl binaries; also builds for macOS and Windows
- **Hot-reloaded config** — edit the config file and the running daemon
  picks it up on the next interval
- **Runs as a service** — systemd unit (system or user scope) with
  `Restart=always`, single-instance locking, graceful SIGTERM/SIGINT
  shutdown
- **Self-contained logging** — UTC timestamps, file + stderr/journal,
  size-based rotation
- **Cross-platform metrics** — Linux `/proc`, macOS `host_statistics64`,
  Windows `GetSystemTimes`/`GlobalMemoryStatusEx`

## Installation

### One-liner (recommended)

```sh
curl -fsSL https://github.com/p1n2o/systemhog/releases/latest/download/install.sh | bash
```

No root needed: the installer detects OS + architecture, downloads the
matching release binary (static musl on Linux, native on macOS),
verifies its SHA-256 checksum (falls back to a warning if the release
ships no checksum asset, plus an ELF/Mach-O magic check), and installs
to `~/.local/bin/systemhog`. It then runs `systemhog init` (wizard on
an interactive terminal, defaults otherwise) and, with systemd present,
installs and starts the service.

Scope follows the invoking user:

| | user install (default, no sudo) | system install (`sudo bash`) |
| ------- | ------------------------------ | ---------------------------- |
| binary  | `~/.local/bin/systemhog`       | `/usr/local/bin/systemhog`   |
| config  | `~/.config/systemhog/`         | `/etc/systemhog/`            |
| service | `systemctl --user` unit        | system unit                  |
| log     | `~/.local/state/systemhog/`    | `/var/log/systemhog.log`     |

Verify the result:

```sh
systemhog status                  # config summary + live CPU/RAM usage
systemctl --user status systemhog # service state (user install)
```

Overrides (environment variables):

| variable             | meaning                             | default           |
| -------------------- | ----------------------------------- | ----------------- |
| `SYSTEMHOG_REPO`     | GitHub `owner/repo`                 | `p1n2o/systemhog` |
| `SYSTEMHOG_VERSION`  | release tag without the leading `v` | latest            |
| `SYSTEMHOG_BASE_URL` | full download base URL              | GitHub releases   |
| `SYSTEMHOG_BIN_DIR`  | install dir for non-root installs   | `~/.local/bin`    |

Example — pin a specific release:

```sh
SYSTEMHOG_VERSION=0.2.0 curl -fsSL https://github.com/p1n2o/systemhog/releases/latest/download/install.sh | bash
```

### Manual

Release assets are named `systemhog-<target>` (e.g.
`systemhog-x86_64-unknown-linux-musl`, plus a `.sha256` checksum next to
each). Download, make executable, put on PATH:

```sh
curl -fsSL -o systemhog https://github.com/p1n2o/systemhog/releases/latest/download/systemhog-x86_64-unknown-linux-musl
chmod +x systemhog
mkdir -p ~/.local/bin && mv systemhog ~/.local/bin/
systemhog version
systemhog init --yes    # write the default user config (~/.config/systemhog)
systemhog status
```

Or build from source — see [Building](#building).

## Usage

```
systemhog [command] [options]

  With no command, the maintainer runs (same as `systemhog run`).

  init        interactive setup (service name, cpu, ram, interval)
  run         run the maintainer in the foreground (default)
  install     create, enable and start the systemd service
  uninstall   stop, disable and remove the systemd service
  status      show config, current usage and service state
  update      check for and apply a newer release of this binary
  self-update alias for update
  version     print version information
  help        show this help

Options:
  --config PATH   use an alternate configuration file
  --yes           init: skip prompts and use defaults
  --check         update: report only; exit 1 when an update is available
```

### Interactive setup

```sh
sudo systemhog init
```

Walks through service name (used for the systemd unit), CPU usage band,
RAM target, check interval and log file — every answer is validated —
writes the config, then offers to install the service.

Scope follows the invoking user: plain `systemhog init` writes your
user config at `~/.config/systemhog/config.conf` (no sudo);
`sudo systemhog init` writes the system-wide config at
`/etc/systemhog/config.conf`.

Pressing Enter accepts the defaults — no typing needed:

| setting        | default                                            |
| -------------- | -------------------------------------------------- |
| service name   | `systemhog`                                        |
| CPU min        | 10%                                                |
| CPU max        | 20%                                                |
| RAM target     | 10%                                                |
| check interval | 5 s                                                |
| log file       | platform default — Linux: `/var/log/systemhog.log` |

`systemhog init --yes` writes exactly these defaults without prompting.

### Service management (systemd)

User scope — no sudo:

```sh
systemhog install                # user unit in ~/.config/systemd/user, enable --now
systemctl --user status systemhog
journalctl --user -u systemhog -f
sudo loginctl enable-linger      # optional: keep it running at boot without a login
systemhog uninstall              # stop, disable, remove the unit (config kept)
```

System scope — as root:

```sh
sudo systemhog install           # system unit, enable --now, start
systemctl status systemhog
journalctl -u systemhog -f
sudo systemhog uninstall         # stop, disable, remove the unit (config kept)
```

The unit runs the same binary that invoked `install` and restarts on
failure (`Restart=always`). User units stop when you log out unless
linger is enabled (`sudo loginctl enable-linger $USER`).

### Updating

```sh
systemhog update          # check, download, verify, replace, restart
systemhog self-update     # same thing
systemhog update --check  # report only; exit 1 when an update is available
```

`update` asks GitHub for the newest release, downloads the binary for
this platform, verifies its SHA-256 checksum (when the release ships
one), replaces the running binary in place, and restarts the systemd
service if it was running. It honors the same environment overrides as
the installer (`SYSTEMHOG_REPO`, `SYSTEMHOG_VERSION`, `SYSTEMHOG_BASE_URL`)
— pinning a version lets you upgrade or downgrade deliberately. The
binary must be writable: a user install under `~/.local/bin` needs no
sudo; use `sudo systemhog update` only for a system install.

### Running without systemd (macOS, Windows, containers)

```sh
systemhog init && systemhog
```

`systemhog` stays in the foreground; Ctrl-C / SIGTERM shuts it down
cleanly (workers joined, RAM released). On Windows/macOS, run it at boot
with your platform's scheduler and a supervisor that restarts it — the
binary itself is fully functional everywhere.

## Uninstallation

### Complete removal (uninstall.sh)

```sh
curl -fsSL https://github.com/p1n2o/systemhog/releases/latest/download/uninstall.sh | bash
```

Removes every trace, in order:

- running `systemhog` processes (plus any leftover `cpu_maintainer`
  process from an earlier version);
- systemd units whose `ExecStart` points at the systemhog binary — system
  and user scope, whatever the service name was named — plus the older
  `cpu_maintainer.service`, then `daemon-reload`;
- the binary from all known locations (`/usr/local/bin`, `/usr/bin`,
  `/usr/sbin`, `~/.local/bin`) and any other copy found on PATH;
- configuration (`/etc/systemhog`, `~/.config/systemhog`, and
  `%APPDATA%\systemhog` on Windows) and leftover
  `/root/cpu_maintainer.*` files;
- logs (`/var/log/systemhog.log*` on Linux,
  `~/Library/Logs/systemhog.log` on macOS, `%LOCALAPPDATA%\systemhog` on
  Windows, `~/.local/state/systemhog`, older `cpu_maintainer` logs);
- lock files (`/run/systemhog-*.lock`, `/tmp/systemhog-*.lock`);
- empty parent directories it created.

It then verifies each category and reports what is gone. It is idempotent
(re-running is a no-op), and when run as a normal user it removes
user-scope files and escalates via sudo for system paths. The repository
clone itself is never touched.

### Service only (keep the config)

```sh
sudo systemhog uninstall        # stop, disable, remove the unit
sudo rm -rf /etc/systemhog      # also drop the config/logs if desired
```

## Configuration

INI format; key names are case-insensitive and the classic key names are
kept, so configs from earlier versions work unchanged:

```ini
[CPU_SETTINGS]
TARGET_CPU_MIN = 10        # if usage drops below this, add load
TARGET_CPU_MAX = 20        # if usage rises above this, remove load
[RAM_SETTINGS]
TARGET_RAM_USAGE = 10      # % of total RAM to keep used
[SETTINGS]
CHECK_INTERVAL = 5         # seconds between adjustments
SERVICE_NAME = systemhog   # systemd unit name
LOG_FILE = /var/log/systemhog.log   # root scope; ~/.local/state for user scope
```

Paths follow the install scope:

| | root / system install | user install |
| ----- | --------------------- | ------------ |
| config | `/etc/systemhog/config.conf` | `~/.config/systemhog/config.conf` |
| lock | `/run/systemhog-<name>.lock` | `$TMPDIR` |
| log | `/var/log/systemhog.log` | `~/.local/state/systemhog/systemhog.log` |

| platform   | default log                               |
| ---------- | ----------------------------------------- |
| Linux      | `/var/log/systemhog.log` (root) / `~/.local/state/systemhog/systemhog.log` (user) |
| macOS      | `~/Library/Logs/systemhog.log`            |
| Windows    | `%LOCALAPPDATA%\systemhog\systemhog.log`  |
| other Unix | `$XDG_STATE_HOME/systemhog/systemhog.log` |

Override with `LOG_FILE` (or the wizard's log-file prompt). A non-writable
log path falls back to stderr-only logging with a warning. Config edits
are picked up live; a broken file keeps the last good settings.

## How it works

- **CPU**: one worker thread per core, each burning `duty%` of its core on a
  100 ms busy/sleep cycle. Every interval the controller compares measured
  usage against the band and adjusts the shared duty in steps bounded by the
  deficit — so it converges in a few intervals and can never swing past the
  band by more than one step. With N workers the duty value _is_ the
  system-wide percentage contributed.
- **RAM**: 10 MiB blocks, committed with volatile per-page stores (a plain
  `vec![0u8; n]` memset gets dead-store-eliminated and the kernel never
  faults in pages — memory usage would stay at zero). Blocks are added while
  _system used_ is a block below target and freed when a block above it, so
  the process yields memory back when other workloads need it.
- **Measurement**: Linux reads `/proc/stat` and `/proc/meminfo` (no libc);
  macOS uses `host_statistics64`/`sysconf`; Windows uses
  `GetSystemTimes`/`GlobalMemoryStatusEx`.

## Building

```sh
cargo build --release                # native build
./build-all.sh                       # all Linux targets below
```

`tools/cross-toolchains.sh` bootstraps the ARM glibc cross compilers
without root (downloads and extracts the Ubuntu debs); with sudo,
`apt-get install gcc-aarch64-linux-gnu gcc-arm-linux-gnueabihf` does the
same. `SYSTEMHOG_TOOLCHAIN_DIR` overrides where the toolchains live.

Built by `./build-all.sh` (all in `dist/`):

| target             | libc  | linked  | size |
| ------------------ | ----- | ------- | ---- |
| x86_64             | musl  | static  | 548K |
| x86_64             | glibc | dynamic | 448K |
| aarch64            | musl  | static  | 488K |
| aarch64            | glibc | dynamic | 412K |
| armv7 (hard-float) | musl  | static  | 464K |
| armv7 (hard-float) | glibc | dynamic | 428K |
| i686               | musl  | static  | 548K |
| riscv64            | musl  | static  | 504K |

musl targets link with the `rust-lld` shipped with rustup (no C toolchain
needed). The release profile is tuned for size (`opt-level=z`, fat LTO,
single codegen unit, `panic=abort`, stripped). GitHub Actions
(`.github/workflows/release.yml`) builds the same Linux matrix plus native
Windows (x86_64/arm64) and macOS (x86_64/arm64) binaries on tag pushes,
and attaches them — with checksums and the `install.sh`/`uninstall.sh`
scripts — to the GitHub Release for that tag.

## License

GPL-3.0.
