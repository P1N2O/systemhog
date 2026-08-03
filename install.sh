#!/usr/bin/env bash
# systemhog installer — run with:
#   curl -fsSL https://github.com/<owner>/<repo>/releases/latest/download/install.sh | bash
#
# Detects OS/arch, downloads the matching release binary, verifies its
# SHA-256 checksum, installs it, and sets up configuration + the systemd
# service. Scope follows the invoking user: as root it installs
# system-wide (config in /etc/systemhog, system unit); as a normal user
# everything stays in user space (~/.local/bin, ~/.config/systemhog,
# systemctl --user unit) — no sudo required. On an interactive terminal
# it runs the setup wizard; otherwise it writes the default config and
# installs/starts the service automatically.
#
# Overridable via environment:
#   SYSTEMHOG_REPO       github owner/repo  (default: p1n2o/systemhog)
#   SYSTEMHOG_VERSION    release tag without leading 'v' (default: latest)
#   SYSTEMHOG_BASE_URL   download base URL (default: github releases)
#   SYSTEMHOG_BIN_DIR    install directory for non-root installs
#                        (default: $HOME/.local/bin)
set -euo pipefail

REPO="${SYSTEMHOG_REPO:-p1n2o/systemhog}"
BASE_URL="${SYSTEMHOG_BASE_URL:-https://github.com/$REPO/releases/latest/download}"
if [ -n "${SYSTEMHOG_VERSION:-}" ]; then
    BASE_URL="https://github.com/$REPO/releases/download/v$SYSTEMHOG_VERSION"
fi

say() { printf '%s\n' "$*"; }

# --- detect OS and architecture ------------------------------------------
case "$(uname -s)" in
    Linux) OS=linux ;;
    Darwin) OS=darwin ;;
    *) say "error: unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    armv7l | armv7*) ARCH=armv7 ;;
    i686 | i386 | i486 | i586) ARCH=i686 ;;
    riscv64) ARCH=riscv64 ;;
    *) say "error: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

case "$OS" in
    linux)
        case "$ARCH" in
            armv7) TRIPLE="armv7-unknown-linux-musleabihf" ;;
            riscv64) TRIPLE="riscv64gc-unknown-linux-musl" ;;
            *) TRIPLE="$ARCH-unknown-linux-musl" ;;
        esac
        ;;
    darwin)
        TRIPLE="$ARCH-apple-darwin"
        ;;
esac

command -v curl >/dev/null 2>&1 || { say "error: curl is required" >&2; exit 1; }

# --- download + verify ----------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="$BASE_URL/systemhog-$TRIPLE"

say "==> downloading $URL"
curl -fsSL "$URL" -o "$TMP/systemhog" || {
    say "error: download failed (does the release exist for $OS/$ARCH?)" >&2
    exit 1
}

# Strict when a checksum asset exists; warn otherwise.
if curl -fsSL "$URL.sha256" -o "$TMP/systemhog.sha256" 2>/dev/null; then
    SUM="$(awk '{print $1}' "$TMP/systemhog.sha256")"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$TMP" && printf '%s  systemhog\n' "$SUM" | sha256sum -c - >/dev/null 2>&1) || {
            say "error: SHA-256 checksum mismatch" >&2
            exit 1
        }
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$TMP" && printf '%s  systemhog\n' "$SUM" | shasum -a 256 -c - >/dev/null 2>&1) || {
            say "error: SHA-256 checksum mismatch" >&2
            exit 1
        }
    else
        say "warning: no sha256sum/shasum available; skipping verification"
    fi
    say "    checksum verified"
else
    say "    warning: no .sha256 asset for this release; skipping verification"
fi

# Sanity: must be an ELF (Linux) or Mach-O (macOS) binary.
case "$OS" in
    linux)
        head -c 4 "$TMP/systemhog" | od -An -tx1 | grep -q "7f 45 4c 46" || {
            say "error: downloaded file is not an ELF executable" >&2
            exit 1
        }
        ;;
    darwin)
        head -c 4 "$TMP/systemhog" | od -An -tx1 | grep -q "cf fa ed fe" || {
            say "error: downloaded file is not a Mach-O executable" >&2
            exit 1
        }
        ;;
esac

# --- install --------------------------------------------------------------
if [ "$(id -u)" = 0 ]; then
    DEST="/usr/local/bin/systemhog"
    install -m 0755 "$TMP/systemhog" "$DEST"
else
    DEST="${SYSTEMHOG_BIN_DIR:-$HOME/.local/bin/systemhog}"
    mkdir -p "$(dirname "$DEST")"
    install -m 0755 "$TMP/systemhog" "$DEST"
    case ":$PATH:" in
        *":$(dirname "$DEST"):"*) ;;
        *) say "    note: add $(dirname "$DEST") to your PATH" ;;
    esac
fi
say "==> installed $DEST ($("$DEST" version))"

# --- configure + service --------------------------------------------------
# Scope follows the invoking user: root installs are system-wide, user
# installs are per-user (user config + systemctl --user unit).
if [ -t 0 ]; then
    say "==> interactive setup"
    "$DEST" init
else
    say "==> writing default configuration"
    "$DEST" init --yes
    if [ -d /run/systemd/system ]; then
        say "==> installing systemd service"
        if "$DEST" install; then
            say "    service installed and started"
        else
            say "warning: service install failed (run: $DEST install)" >&2
        fi
    else
        say "    no systemd; skipping service install"
        say "    to enable later: $DEST install"
    fi
fi

say
say "systemhog is installed."
say "  binary : $DEST"
say "  status : $DEST status"
say "  remove : curl -fsSL $BASE_URL/uninstall.sh | bash"
