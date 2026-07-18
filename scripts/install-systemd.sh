#!/bin/bash
# Install the L337 Audio Server as a systemd service.
#
# This script does NOT build anything — it installs a PREBUILT binary from
# ./bin/l337-audio-server (produced by `scripts/build.sh` on a machine that has
# cargo/rustc). This keeps the install free of build toolchains, so target
# systems without cargo can still run the server.
#
# Two modes are supported:
#   1. System service (default)  - runs as a dedicated, unprivileged `l337`
#      system user (MPD-style), managed by root. Best for an always-on box that
#      serves audio to clients on the LAN/WWAN.
#   2. User service              - a per-user instance under `systemd --user`,
#      owned by whichever user enables it (e.g. the `l337` user after linger).
#
# Usage:
#   sudo ./scripts/install-systemd.sh            # system service as `l337` user
#   ./scripts/install-systemd.sh --user          # user service for $USER
#
# To build the binary first (requires cargo):
#   ./scripts/build.sh
set -euo pipefail

USER_NAME="l337"
GROUP_NAME="l337"
INSTALL_DIR="/opt/l337-audio-server"
SYSTEM_SERVICE="/etc/systemd/system/l337-audio.service"
STATE_DIR="/var/lib/l337-audio-server"
CACHE_DIR="/var/cache/l337-audio-server"
CONFIG_DIR="/etc/l337-audio-server"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-system}"

require_prebuilt_binary() {
    local bin="$SCRIPT_DIR/bin/l337-audio-server"
    if [ ! -x "$bin" ]; then
        echo "Prebuilt binary not found at $bin" >&2
        echo "Build it first on a machine with cargo:  ./scripts/build.sh" >&2
        exit 1
    fi
    echo "$bin"
}

install_system_service() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "System service install requires root (use sudo). For a per-user" >&2
        echo "instance run: $0 --user" >&2
        exit 1
    fi

    BIN="$(require_prebuilt_binary)"

    echo "Creating dedicated system user '$USER_NAME' (MPD-style)..."
    if ! id "$USER_NAME" &>/dev/null; then
        useradd --system --no-create-home --shell /usr/sbin/nologin \
            --comment "L337 Audio Server" "$USER_NAME"
    fi
    usermod -aG audio "$USER_NAME" 2>/dev/null || true

    echo "Installing to $INSTALL_DIR..."
    rm -rf "$INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
    cp "$SCRIPT_DIR/config.toml" "$INSTALL_DIR/"
    cp -r "$SCRIPT_DIR/scripts" "$INSTALL_DIR/"
    cp "$BIN" "$INSTALL_DIR/l337-audio-server"
    chmod +x "$INSTALL_DIR/l337-audio-server"

    echo "Creating state/cache/config directories owned by $USER_NAME..."
    install -d -m 0755 -o "$USER_NAME" -g "$GROUP_NAME" "$STATE_DIR"
    install -d -m 0750 -o "$USER_NAME" -g "$GROUP_NAME" "$CACHE_DIR"
    install -d -m 0755 -o "$USER_NAME" -g "$GROUP_NAME" "$CONFIG_DIR"
    [ -f "$CONFIG_DIR/config.toml" ] || cp "$SCRIPT_DIR/config.toml" "$CONFIG_DIR/config.toml"
    chown "$USER_NAME:$GROUP_NAME" "$CONFIG_DIR/config.toml"

    chown -R "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR"

    echo "Writing systemd unit $SYSTEM_SERVICE..."
    cat > "$SYSTEM_SERVICE" <<EOF
[Unit]
Description=L337 Audio Server
Documentation=https://github.com/l337-audio-server
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
User=$USER_NAME
Group=$GROUP_NAME
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/l337-audio-server
Restart=on-failure
RestartSec=2

# Filesystem / runtime locations (systemd creates + chowns these).
StateDirectory=l337-audio-server
CacheDirectory=l337-audio-server
ConfigurationDirectory=l337-audio-server

# Hardening (MPD-style, unprivileged service user).
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ProtectControlGroups=true
ProtectKernelModules=true
ProtectKernelTunables=true
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=false
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
ReadWritePaths=$STATE_DIR $CACHE_DIR

[Install]
WantedBy=multi-user.target
EOF

    echo "Reloading systemd and enabling service..."
    systemctl daemon-reload
    systemctl enable l337-audio.service
    systemctl restart l337-audio.service
    echo
    echo "L337 Audio Server installed as a system service running under user '$USER_NAME'."
    echo "Check status with:  sudo systemctl status l337-audio.service"
    echo "View logs with:     sudo journalctl -u l337-audio.service -f"
}

install_user_service() {
    BIN="$(require_prebuilt_binary)"

    UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    UNIT="$UNIT_DIR/l337-audio.service"
    mkdir -p "$UNIT_DIR"

    echo "Writing user unit $UNIT..."
    cat > "$UNIT" <<EOF
[Unit]
Description=L337 Audio Server (user)
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$SCRIPT_DIR
ExecStart=$BIN
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

    echo "Enabling + starting user service (lingering recommended for headless)..."
    systemctl --user daemon-reload
    systemctl --user enable l337-audio.service
    systemctl --user restart l337-audio.service
    echo
    echo "L337 Audio Server installed as a user service for '$USER'."
    echo "For headless/always-on, enable linger:  sudo loginctl enable-linger $USER"
    echo "Check status with:  systemctl --user status l337-audio.service"
}

case "$MODE" in
    --user|user) install_user_service ;;
    system|--system|"") install_system_service ;;
    *) echo "Unknown mode: $MODE (use --user or nothing)"; exit 1 ;;
esac
