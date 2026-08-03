#!/usr/bin/env bash
# Complete systemhog removal: stops and deletes the systemd service (any
# service name — units are matched by their ExecStart pointing at the
# systemhog binary), kills running processes, and deletes the binary,
# config, logs, lock files and state, including leftover files from older
# versions (cpu_maintainer).
#
# Run as root (or with sudo) for system-wide cleanup; as a normal user it
# removes the user-scope files and escalates (if sudo works) for the rest.
#
#   curl -fsSL https://github.com/<owner>/<repo>/releases/latest/download/uninstall.sh | sudo bash
set -uo pipefail

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }

# System-scope operations: run as root when possible; suppress sudo's
# password prompt noise when there is no terminal.
asroot() {
    if [ "$(id -u)" = 0 ]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        if ! sudo "$@" 2>/dev/null; then
            warn "skipped (needs root): $*"
            return 1
        fi
    else
        warn "skipped (no sudo): $*"
        return 1
    fi
}

# Home of the invoking user when running under sudo (sudo sets $HOME to /root).
USER_HOME="$HOME"
if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    USER_HOME="$(sudo -u "$SUDO_USER" sh -c 'printf %s "$HOME"')"
fi

SYSTEM_BINS="/usr/local/bin/systemhog /usr/bin/systemhog /usr/sbin/systemhog"
USER_BINS="$USER_HOME/.local/bin/systemhog"

# --- stop processes -------------------------------------------------------
say "==> stopping systemhog processes"
pkill -x systemhog 2>/dev/null || true
asroot pkill -x systemhog 2>/dev/null || true
for p in $SYSTEM_BINS $USER_BINS; do
    [ -x "$p" ] && { pkill -f "^$p" 2>/dev/null || true; asroot pkill -f "^$p" 2>/dev/null || true; }
done
pkill -f "cpu_maintainer.py" 2>/dev/null || true
asroot pkill -f "cpu_maintainer.py" 2>/dev/null || true
sleep 1

# --- systemd units --------------------------------------------------------
# Match by content (ExecStart contains the systemhog binary path), not by
# unit name, so user-chosen service names are found too.
remove_units() { # $1 = unit dir, $2 = "root"|"user"
    local dir="$1" scope="$2" unit name
    for unit in "$dir"/*.service; do
        [ -f "$unit" ] || continue
        if grep -q "systemhog" "$unit" 2>/dev/null; then
            name="$(basename "$unit" .service)"
            if [ "$scope" = root ]; then
                asroot systemctl stop "$name" 2>/dev/null || true
                asroot systemctl disable "$name" 2>/dev/null || true
                asroot rm -f "$unit"
            else
                systemctl --user stop "$name" 2>/dev/null || true
                systemctl --user disable "$name" 2>/dev/null || true
                rm -f "$unit"
            fi
            say "  removed unit: $unit"
        fi
    done
}
say "==> removing systemd units"
remove_units "/etc/systemd/system" root
remove_units "$USER_HOME/.config/systemd/user" user

# Older cpu_maintainer service.
asroot systemctl stop cpu_maintainer 2>/dev/null || true
asroot systemctl disable cpu_maintainer 2>/dev/null || true
asroot rm -f /etc/systemd/system/cpu_maintainer.service 2>/dev/null || true

asroot systemctl daemon-reload 2>/dev/null || true
systemctl --user daemon-reload 2>/dev/null || true

# --- binary ---------------------------------------------------------------
say "==> removing binary"
for p in $SYSTEM_BINS; do
    [ -e "$p" ] && asroot rm -f "$p" && say "  removed: $p"
done
for p in $USER_BINS; do
    [ -e "$p" ] && rm -f "$p" && say "  removed: $p"
done
if command -v systemhog >/dev/null 2>&1; then
    OTHER="$(command -v systemhog)"
    rm -f "$OTHER" 2>/dev/null || asroot rm -f "$OTHER"
    say "  removed: $OTHER"
fi

# --- config ---------------------------------------------------------------
say "==> removing configuration"
if [ -e /etc/systemhog ]; then
    asroot rm -rf /etc/systemhog && say "  removed: /etc/systemhog"
fi
if [ -e "$USER_HOME/.config/systemhog" ]; then
    rm -rf "$USER_HOME/.config/systemhog" && say "  removed: $USER_HOME/.config/systemhog"
fi
if [ -e "$USER_HOME/AppData/Roaming/systemhog" ]; then
    rm -rf "$USER_HOME/AppData/Roaming/systemhog" && say "  removed: $USER_HOME/AppData/Roaming/systemhog"
fi
[ -e /root/cpu_maintainer.py ] && asroot rm -f /root/cpu_maintainer.py
[ -e /root/cpu_maintainer.conf ] && asroot rm -f /root/cpu_maintainer.conf

# --- logs / state ---------------------------------------------------------
say "==> removing logs"
if [ -e /var/log/systemhog.log ]; then
    asroot rm -f /var/log/systemhog.log /var/log/systemhog.log.1 && say "  removed: /var/log/systemhog.log"
fi
if [ -e /var/log/cpu_maintainer.log ]; then
    asroot rm -f /var/log/cpu_maintainer.log /var/log/cpu_maintainer.log.1
fi
if [ -e "$USER_HOME/.local/state/systemhog" ]; then
    rm -rf "$USER_HOME/.local/state/systemhog" && say "  removed: $USER_HOME/.local/state/systemhog"
fi
if [ -e "$USER_HOME/Library/Logs/systemhog.log" ]; then
    rm -f "$USER_HOME/Library/Logs/systemhog.log" && say "  removed: $USER_HOME/Library/Logs/systemhog.log"
fi
if [ -e "$USER_HOME/AppData/Local/systemhog" ]; then
    rm -rf "$USER_HOME/AppData/Local/systemhog" && say "  removed: $USER_HOME/AppData/Local/systemhog"
fi

# --- lock files -----------------------------------------------------------
say "==> removing lock files"
if compgen -G "/run/systemhog-*.lock" >/dev/null; then
    asroot rm -f /run/systemhog-*.lock
fi
rm -f /tmp/systemhog-*.lock 2>/dev/null || true
[ -n "${TMPDIR:-}" ] && rm -f "$TMPDIR"/systemhog-*.lock 2>/dev/null || true

# --- tidy empty dirs ------------------------------------------------------
asroot rmdir /etc/systemhog 2>/dev/null || true
rmdir "$USER_HOME/.config/systemhog" "$USER_HOME/.local/state/systemhog" \
      "$USER_HOME/.local/bin" 2>/dev/null || true

# --- verify ---------------------------------------------------------------
say "==> verifying cleanup"
if command -v systemhog >/dev/null 2>&1; then
    warn "still on PATH: $(command -v systemhog)"
else
    say "  binary: gone"
fi
[ -e /etc/systemhog ] && warn "/etc/systemhog still exists" || say "  config: gone"
[ -e "$USER_HOME/.config/systemhog" ] || [ -e "$USER_HOME/AppData/Roaming/systemhog" ] \
    && warn "user config still exists" || say "  user config: gone"
LOGS_LEFT=""
[ -e /var/log/systemhog.log ] && LOGS_LEFT="$LOGS_LEFT /var/log/systemhog.log"
[ -e "$USER_HOME/Library/Logs/systemhog.log" ] && LOGS_LEFT="$LOGS_LEFT $USER_HOME/Library/Logs/systemhog.log"
[ -e "$USER_HOME/AppData/Local/systemhog" ] && LOGS_LEFT="$LOGS_LEFT $USER_HOME/AppData/Local/systemhog"
[ -e "$USER_HOME/.local/state/systemhog" ] && LOGS_LEFT="$LOGS_LEFT $USER_HOME/.local/state/systemhog"
if [ -n "$LOGS_LEFT" ]; then
    warn "logs still exist:$LOGS_LEFT"
else
    say "  logs: gone"
fi
if pgrep -x systemhog >/dev/null 2>&1; then
    warn "systemhog is still running"
else
    say "  process: stopped"
fi

say
say "systemhog has been completely removed."
