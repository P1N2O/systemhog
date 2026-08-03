#!/usr/bin/env bash
# systemhog installer — run with:
#   curl -fsSL https://github.com/<owner>/<repo>/releases/latest/download/install.sh | sudo bash
#
# Linux only. Detects the architecture, downloads the matching release
# binary, verifies its SHA-256 checksum, installs it to /usr/local/bin,
# and sets up configuration + the systemd service. All locations are
# unified (binary /usr/local/bin, config /etc/systemhog, log
# /var/log) — the binary itself can then be *run* by any user; only
# install/setup needs root. On an interactive terminal it runs the setup
# wizard; otherwise it writes the default config and installs/starts the
# service automatically.
#
# Overridable via environment:
#   SYSTEMHOG_REPO       github owner/repo  (default: p1n2o/systemhog)
#   SYSTEMHOG_VERSION    release tag without leading 'v' (default: latest)
#   SYSTEMHOG_BASE_URL   download base URL (default: github releases)
set -euo pipefail

REPO="${SYSTEMHOG_REPO:-p1n2o/systemhog}"
BASE_URL="${SYSTEMHOG_BASE_URL:-https://github.com/$REPO/releases/latest/download}"
if [ -n "${SYSTEMHOG_VERSION:-}" ]; then
    BASE_URL="https://github.com/$REPO/releases/download/v$SYSTEMHOG_VERSION"
fi

say() { printf '%s\n' "$*"; }

[ "$(id -u)" = 0 ] || { say "error: installation requires root — run with sudo" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || { say "error: curl is required" >&2; exit 1; }

# --- detect architecture --------------------------------------------------
case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    aarch64 | arm64) ARCH=aarch64 ;;
    armv7l | armv7*) ARCH=armv7 ;;
    i686 | i386 | i486 | i586) ARCH=i686 ;;
    riscv64) ARCH=riscv64 ;;
    *) say "error: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

case "$ARCH" in
    armv7) TRIPLE="armv7-unknown-linux-musleabihf" ;;
    riscv64) TRIPLE="riscv64gc-unknown-linux-musl" ;;
    *) TRIPLE="$ARCH-unknown-linux-musl" ;;
esac

# --- download + verify ----------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="$BASE_URL/systemhog-$TRIPLE"

say "==> downloading $URL"
curl -fsSL "$URL" -o "$TMP/systemhog" || {
    say "error: download failed (does the release exist for $ARCH?)" >&2
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
    else
        say "warning: no sha256sum available; skipping verification"
    fi
    say "    checksum verified"
else
    say "    warning: no .sha256 asset for this release; skipping verification"
fi

# Sanity: must be an ELF binary.
head -c 4 "$TMP/systemhog" | od -An -tx1 | grep -q "7f 45 4c 46" || {
    say "error: downloaded file is not an ELF executable" >&2
    exit 1
}

# --- install (unified location) -------------------------------------------
DEST="/usr/local/bin/systemhog"
install -m 0755 "$TMP/systemhog" "$DEST"
say "==> installed $DEST ($("$DEST" version))"

# --- configure + service --------------------------------------------------
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
            say "warning: service install failed (run: sudo $DEST install)" >&2
        fi
    else
        say "    no systemd; skipping service install"
        say "    to enable later: sudo $DEST install"
    fi
fi

say
say "systemhog is installed."
say "  binary : $DEST"
say "  status : $DEST status"
say "  remove : curl -fsSL $BASE_URL/uninstall.sh | sudo bash"
