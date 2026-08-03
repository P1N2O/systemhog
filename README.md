# systemhog

A CPU and RAM resource maintainer that holds system usage at your
configured levels — for load testing, benchmarks, or just keeping a
machine busy. Pure Rust, zero dependencies, one small static musl binary
for Linux (x86_64, aarch64, armv7, i686, riscv64).

## Features

- **Fine-grained CPU control** — one worker thread per core with an
  adjustable duty cycle; a proportional controller keeps total usage inside
  the configured band without the oscillation of all-or-nothing loaders
- **RAM steering** — 10 MiB blocks are added while system usage sits below
  the target and freed when it climbs above it, yielding memory back to
  other workloads
- **System-aware** — measures total system usage, not its own
  contribution: if other services already keep the machine busy, it adds
  nothing
- **Interactive setup** — `systemhog init` wizard: service name, CPU band,
  RAM target, check interval, log file — every answer validated
- **Zero dependencies** — reads `/proc` directly, no libc; fully static
  musl binaries
- **Hot-reloaded config** — edit the config file and the running daemon
  picks it up on the next interval
- **Runs as a service** — systemd unit with `Restart=always`,
  single-instance locking, graceful SIGTERM/SIGINT shutdown
- **Self-contained logging** — UTC timestamps, file + stderr/journal,
  size-based rotation
- **Self-updating** — `systemhog update` checks GitHub, replaces the
  binary and restarts the service

## Installation

### One-liner (recommended)

```sh
curl -fsSL https://github.com/p1n2o/systemhog/releases/latest/download/install.sh | sudo bash
```

The installer detects the architecture, downloads the matching release
binary, verifies its SHA-256 checksum (falls back to a warning if the
release ships no checksum asset, plus an ELF magic check), installs it
to `/usr/local/bin/systemhog`, then runs `systemhog init` (wizard on an
interactive terminal, defaults otherwise) and, with systemd present,
installs and starts the service.

All locations are unified — root or non-root, every command and the
service use the same paths:

|       | location                        |
| ----- | ------------------------------- |
| binary | `/usr/local/bin/systemhog`      |
| config | `/etc/systemhog/config.conf`    |
| lock   | `/run/systemhog-<name>.lock`    |
| log    | `/var/log/systemhog.log`        |

Setup (install, init, service management, update) needs root. The
**binary itself runs as any user**: `systemhog status` reads the
world-readable config, and `systemhog run` in the foreground works too —
with the unified log path it falls back to stderr-only logging.

Verify the result:

```sh
systemhog status                # config summary + live CPU/RAM usage
systemctl status systemhog      # service state
```

Overrides (environment variables):

| variable             | meaning                             | default           |
| -------------------- | ----------------------------------- | ----------------- |
| `SYSTEMHOG_REPO`     | GitHub `owner/repo`                 | `p1n2o/systemhog` |
| `SYSTEMHOG_VERSION`  | release tag without the leading `v` | latest            |
| `SYSTEMHOG_BASE_URL` | full download base URL              | GitHub releases   |

Example — pin a specific release:

```sh
SYSTEMHOG_VERSION=0.4.0 curl -fsSL https://github.com/p1n2o/systemhog/releases/latest/download/install.sh | sudo bash
```

### Manual

Release assets are named `systemhog-<target>` (e.g.
`systemhog-x86_64-unknown-linux-musl`, plus a `.sha256` checksum next to
each). Download, make executable, put on PATH:

```sh
curl -fsSL -o systemhog https://github.com/p1n2o/systemhog/releases/latest/download/systemhog-x86_64-unknown-linux-musl
chmod +x systemhog
sudo mv systemhog /usr/local/bin/
sudo systemhog init --yes    # write the default config
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
writes the config, then offers to install the service. The config is
system-wide (`/etc/systemhog/config.conf`), so init needs root.

Pressing Enter accepts the defaults — no typing needed:

| setting        | default                        |
| -------------- | ------------------------------ |
| service name   | `systemhog`                    |
| CPU min        | 10%                            |
| CPU max        | 20%                            |
| RAM target     | 10%                            |
| check interval | 5 s                            |
| log file       | `/var/log/systemhog.log`       |

`systemhog init --yes` writes exactly these defaults without prompting.

### Service management (systemd)

```sh
sudo systemhog install                # write unit, enable --now, start
systemctl status systemhog            # check it
journalctl -u systemhog -f            # service logs
tail -f /var/log/systemhog.log        # application log (also on stderr/journal)
sudo systemhog uninstall              # stop, disable, remove the unit (config kept)
```

The unit runs the same binary that invoked `install` and restarts on
failure (`Restart=always`). The service runs at boot, no login needed.

### Updating

```sh
sudo systemhog update          # check, download, verify, replace, restart
sudo systemhog self-update     # same thing
systemhog update --check       # report only; exit 1 when an update is available
```

`update` asks GitHub for the newest release, downloads the binary for
this platform, verifies its SHA-256 checksum (when the release ships
one), replaces the running binary in place, and restarts the systemd
service if it was running. It honors the same environment overrides as
the installer (`SYSTEMHOG_REPO`, `SYSTEMHOG_VERSION`, `SYSTEMHOG_BASE_URL`)
— pinning a version lets you upgrade or downgrade deliberately. Replacing
`/usr/local/bin/systemhog` needs root; `--check` works without it.

### Running without systemd (containers, minimal hosts)

```sh
sudo systemhog init && sudo systemhog
```

`systemhog` stays in the foreground; Ctrl-C / SIGTERM shuts it down
cleanly (workers joined, RAM released). Wrap it in your supervisor of
choice where systemd is unavailable.

## Uninstallation

### Complete removal (uninstall.sh)

```sh
curl -fsSL https://github.com/p1n2o/systemhog/releases/latest/download/uninstall.sh | sudo bash
```

Removes every trace, in order:

- running `systemhog` processes (plus any leftover `cpu_maintainer`
  process from an earlier version);
- systemd units whose `ExecStart` points at the systemhog binary —
  whatever the service name was named — plus the older
  `cpu_maintainer.service`, then `daemon-reload`;
- the binary from all known locations (`/usr/local/bin`, `/usr/bin`,
  `/usr/sbin`, `~/.local/bin`) and any other copy found on PATH;
- configuration (`/etc/systemhog`, plus `~/.config/systemhog` from older
  user-scope versions) and leftover `/root/cpu_maintainer.*` files;
- logs (`/var/log/systemhog.log*`, `~/.local/state/systemhog` from older
  versions, older `cpu_maintainer` logs);
- lock files (`/run/systemhog-*.lock`, `/tmp/systemhog-*.lock`);
- empty parent directories it created.

It then verifies each category and reports what is gone. It is idempotent
(re-running is a no-op). The repository clone itself is never touched.

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
LOG_FILE = /var/log/systemhog.log
```

One set of paths for everyone — root or not:

|       | location                     |
| ----- | ---------------------------- |
| config | `/etc/systemhog/config.conf` |
| lock   | `/run/systemhog-<name>.lock` (root) or `$TMPDIR` (non-root) |
| log    | `/var/log/systemhog.log`     |

Override with `LOG_FILE` (or the wizard's log-file prompt). A non-writable
log path falls back to stderr-only logging with a warning. Config edits
are picked up live; a broken file keeps the last good settings.

## How it works

- **CPU**: one worker thread per core, each burning `duty%` of its core on a
  100 ms busy/sleep cycle. Every interval the controller compares measured
  system-wide usage against the band and adjusts the shared duty in steps
  bounded by the deficit — so it converges in a few intervals and can never
  swing past the band by more than one step. With N workers the duty value
  _is_ the system-wide percentage contributed.
- **RAM**: 10 MiB blocks, committed with volatile per-page stores (a plain
  `vec![0u8; n]` memset gets dead-store-eliminated and the kernel never
  faults in pages — memory usage would stay at zero). Blocks are added while
  _system used_ is a block below target and freed when a block above it, so
  the process yields memory back when other workloads need it.
- **Measurement**: reads `/proc/stat` and `/proc/meminfo` — no libc.

## Building

```sh
cargo build --release                # native build
./build-all.sh                       # all Linux targets below
```

`tools/cross-toolchains.sh` bootstraps the ARM glibc cross compilers
without root (downloads and extracts the Ubuntu debs); with sudo,
`apt-get install gcc-aarch64-linux-gnu gcc-arm-linux-gnueabihf` does the
same. `SYSTEMHOG_TOOLCHAIN_DIR` overrides where the toolchains live.

The release workflow (`.github/workflows/release.yml`) builds the Linux
matrix (musl static: x86_64, aarch64, armv7, i686, riscv64; glibc:
x86_64, aarch64, armv7) on tag pushes and attaches them — with checksums
and the `install.sh`/`uninstall.sh` scripts — to the GitHub Release for
that tag.
