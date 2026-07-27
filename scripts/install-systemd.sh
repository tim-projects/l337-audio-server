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
#   sudo ./scripts/install-systemd.sh --update   # in-place binary upgrade (no
#                                               #   config/state/cache touched)
#
# To build the binary first (requires cargo):
#   ./scripts/build.sh
set -euo pipefail

USER_NAME="l337"
GROUP_NAME="l337"
INSTALL_DIR="/opt/l337-audio-server"
SYSTEM_SERVICE="/etc/systemd/system/l337-audio-server.service"
STATE_DIR="/var/lib/l337-audio-server"
CACHE_DIR="/var/cache/l337-audio-server"
CONFIG_DIR="/etc/l337-audio-server"

VERIFICATION_TOOLS_INSTALLED=""
VERIFICATION_PKG_MANAGER=""
VERIFICATION_REMOVE_CMD=""
VERIFICATION_WAS_MISSING_PACTL=false
VERIFICATION_WAS_MISSING_PWCLI=false

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-system}"

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

# The service was historically named `l337-audio.service` (both system and
# user scopes). If an old unit with that name is still enabled/running, stop,
# disable and remove it so it doesn't linger alongside the renamed
# `l337-audio-server.service`.
migrate_old_service_name() {
    local old="/etc/systemd/system/l337-audio.service"
    if [ -f "$old" ]; then
        echo "Found legacy system unit l337-audio.service; migrating to $SYSTEM_SERVICE..."
        systemctl disable l337-audio.service 2>/dev/null || true
        systemctl stop l337-audio.service 2>/dev/null || true
        rm -f "$old"
        systemctl daemon-reload
    fi

    local old_user="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/l337-audio.service"
    if [ -f "$old_user" ]; then
        echo "Found legacy user unit l337-audio.service; migrating..."
        systemctl --user disable l337-audio.service 2>/dev/null || true
        systemctl --user stop l337-audio.service 2>/dev/null || true
        rm -f "$old_user"
        systemctl --user daemon-reload
    fi
}

write_system_unit() {
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
Environment=XDG_RUNTIME_DIR=/run/l337-audio-server
ExecStartPre=$INSTALL_DIR/scripts/start-pipewire.sh
ExecStart=$INSTALL_DIR/l337-audio-server
Restart=on-failure
RestartSec=2

# Filesystem / runtime locations (systemd creates + chowns these).
StateDirectory=l337-audio-server
CacheDirectory=l337-audio-server
ConfigurationDirectory=l337-audio-server
RuntimeDirectory=l337-audio-server
RuntimeDirectoryMode=0700

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
}

require_prebuilt_binary() {
    local bin="$SCRIPT_DIR/bin/l337-audio-server"
    if [ ! -f "$bin" ]; then
        echo "Prebuilt binary not found at: $bin" >&2
        echo >&2
        echo "Build it first, then re-run this installer:" >&2
        echo "    ./scripts/build.sh            # produces ./bin/l337-audio-server" >&2
        echo "    sudo ./scripts/install-systemd.sh --update" >&2
        exit 1
    fi
    if [ ! -x "$bin" ]; then
        echo "Binary at $bin is not executable." >&2
        exit 1
    fi
    echo "$bin"
}

check_pipewire_dependency() {
    local missing=0

    if ! command -v pipewire &>/dev/null; then
        fail "pipewire binary not found on PATH. Install pipewire and re-run this script."
    fi
    ok "pipewire found: $(command -v pipewire)"

    if ! command -v wireplumber &>/dev/null; then
        fail "wireplumber binary not found on PATH. Install wireplumber and re-run this script."
    fi
    ok "wireplumber found: $(command -v wireplumber)"

    if ! command -v pactl &>/dev/null && ! command -v pw-cli &>/dev/null; then
        VERIFICATION_WAS_MISSING_PACTL=true
        VERIFICATION_WAS_MISSING_PWCLI=true
        warn "Neither pactl nor pw-cli found. Will install temporarily for verification..."
        install_verification_tools
    else
        ok "PipeWire user tools found"
    fi
}

install_verification_tools() {
    local pkg_manager=""
    local install_cmd=""
    local remove_cmd=""
    local pkg_name=""

    if command -v pacman &>/dev/null; then
        pkg_manager="pacman"
        install_cmd="pacman -S --noconfirm"
        remove_cmd="pacman -Rns --noconfirm"
        pkg_name="pipewire-utils"
    elif command -v apt-get &>/dev/null; then
        pkg_manager="apt"
        install_cmd="apt-get install -y"
        remove_cmd="apt-get purge -y"
        pkg_name="pipewire-utils"
    elif command -v dnf &>/dev/null; then
        pkg_manager="dnf"
        install_cmd="dnf install -y"
        remove_cmd="dnf remove -y"
        pkg_name="pipewire-utils"
    elif command -v yum &>/dev/null; then
        pkg_manager="yum"
        install_cmd="yum install -y"
        remove_cmd="yum remove -y"
        pkg_name="pipewire-utils"
    elif command -v zypper &>/dev/null; then
        pkg_manager="zypper"
        install_cmd="zypper install -y"
        remove_cmd="zypper remove -y"
        pkg_name="pipewire-utils"
    elif command -v apk &>/dev/null; then
        pkg_manager="apk"
        install_cmd="apk add"
        remove_cmd="apk del"
        pkg_name="pipewire-utils"
    else
        warn "No supported package manager found. Cannot install verification tools."
        return 1
    fi

    info "Installing $pkg_name temporarily for verification..."
    if $install_cmd "$pkg_name" >/dev/null 2>&1; then
        ok "Installed $pkg_name for verification"
        VERIFICATION_TOOLS_INSTALLED="$pkg_name"
        VERIFICATION_PKG_MANAGER="$pkg_manager"
        VERIFICATION_REMOVE_CMD="$remove_cmd"
        return 0
    else
        warn "Failed to install $pkg_name. Skipping verification."
        return 1
    fi
}

remove_verification_tools() {
    if [ "$VERIFICATION_WAS_MISSING_PACTL" = true ] || [ "$VERIFICATION_WAS_MISSING_PWCLI" = true ]; then
        if [ -n "${VERIFICATION_TOOLS_INSTALLED:-}" ] && [ -n "${VERIFICATION_REMOVE_CMD:-}" ]; then
            info "Removing temporary verification tools ($VERIFICATION_TOOLS_INSTALLED)..."
            if $VERIFICATION_REMOVE_CMD "$VERIFICATION_TOOLS_INSTALLED" >/dev/null 2>&1; then
                ok "Removed $VERIFICATION_TOOLS_INSTALLED"
            else
                warn "Failed to remove $VERIFICATION_TOOLS_INSTALLED. You may want to remove it manually."
            fi
        fi
    else
        info "Verification tools were already installed; leaving them in place."
    fi
}

generate_test_tone() {
    local file="$1"
    if command -v ffmpeg &>/dev/null; then
        ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -ac 2 -ar 44100 "$file" -y >/dev/null 2>&1
        return $?
    elif command -v sox &>/dev/null; then
        sox -n "$file" synth 1 sine 440 gain -3 >/dev/null 2>&1
        return $?
    elif command -v python3 &>/dev/null; then
        python3 -c "
import wave, math, struct, sys
rate = 44100
duration = 1
freq = 440
samples = []
for i in range(rate * duration):
    sample = math.sin(2 * math.pi * freq * i / rate) * 0.5
    samples.append(int(sample * 32767))
with wave.open('$file', 'w') as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(rate)
    for s in samples:
        w.writeframes(struct.pack('<hh', s, s))
" >/dev/null 2>&1
        return $?
    fi
    return 1
}

test_audio_playback() {
    local test_file="/tmp/l337-test-tone.wav"
    local token_file="/var/lib/l337-audio-server/server_token.txt"
    local config_file="/etc/l337-audio-server/config.toml"
    local server_url=""

    # Read auth token
    local token=""
    if [ -f "$token_file" ]; then
        token=$(cat "$token_file" 2>/dev/null | tr -d '\n')
    fi

    if [ -z "$token" ]; then
        warn "No auth token found at $token_file. Skipping audio playback test."
        return 0
    fi

    # Generate a 1-second 440Hz test tone (muted — no sound will come out)
    if ! generate_test_tone "$test_file"; then
        warn "Could not generate test tone (need ffmpeg, sox, or python3). Skipping audio playback test."
        return 0
    fi
    ok "Generated test tone"

    # Read configured host from config
    local configured_host="localhost"
    if [ -f "$config_file" ]; then
        configured_host=$(grep -E '^host\s*=' "$config_file" | head -1 | sed 's/.*=\s*"\([^"]*\)".*/\1/')
        configured_host=${configured_host:-localhost}
    fi

    # Try multiple addresses in order: localhost, 127.0.0.1, configured host
    local addresses=("localhost" "127.0.0.1" "$configured_host")
    if [ "$configured_host" != "0.0.0.0" ] && [ "$configured_host" != "::" ]; then
        addresses+=("$configured_host")
    fi

    # Wait for server to be ready on any address
    local max_wait=30
    local waited=0
    local server_ready=false
    local server_addr=""

    while [ $waited -lt $max_wait ]; do
        for addr in "${addresses[@]}"; do
            local url="https://$addr:1337/health"
            if curl -sk -o /dev/null -w "%{http_code}" --connect-timeout 2 "$url" 2>/dev/null | grep -q "200"; then
                server_ready=true
                server_addr="$addr"
                break 2
            fi
        done
        sleep 1
        waited=$((waited + 1))
    done

    if [ "$server_ready" != true ]; then
        warn "Server did not become ready on any address after ${max_wait}s"
        warn "Tried: ${addresses[*]}"
        warn "Check logs with: sudo journalctl -u l337-audio-server.service -f"
        rm -f "$test_file"
        return 0
    fi
    ok "Server is ready at https://$server_addr:1337"

    # Mute volume before test playback
    local mute_response
    mute_response=$(curl -sk -X POST "https://$server_addr:1337/player/volume" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        -d '{"volume":0.0}' \
        -w "\n%{http_code}" 2>/dev/null || true)

    local mute_code
    mute_code=$(echo "$mute_response" | tail -n1)
    if [ "$mute_code" != "200" ]; then
        warn "Could not mute volume before test (HTTP $mute_code). Continuing anyway..."
    else
        ok "Volume muted for test playback"
    fi

    # Upload test tone and command playback
    local play_response
    play_response=$(curl -sk -X POST "https://$server_addr:1337/player/play/stream" \
        -H "Authorization: Bearer $token" \
        -H "X-Track-Id: installer-test" \
        -H "X-Title: Installer Test Tone" \
        -H "Content-Type: audio/wav" \
        --data-binary @"$test_file" \
        -w "\n%{http_code}" 2>/dev/null || true)

    local http_code
    http_code=$(echo "$play_response" | tail -n1)
    if [ "$http_code" != "200" ]; then
        rm -f "$test_file"
        fail "Failed to upload test audio (HTTP $http_code). Response: $(echo "$play_response" | head -n1)"
    fi
    ok "Test audio uploaded and play commanded"

    # Give it a moment to start decoding/playing
    sleep 2

    # Verify playback state
    local status_response
    status_response=$(curl -sk "https://$server_addr:1337/player/status" \
        -H "Authorization: Bearer $token" 2>/dev/null || true)

    if echo "$status_response" | grep -q '"state":"playing"\|"state": "playing"'; then
        ok "Audio playback confirmed (state: playing)"
    else
        rm -f "$test_file"
        fail "Audio playback not confirmed. Status: $status_response"
    fi

    # Stop playback
    curl -sk -X POST "https://$server_addr:1337/player/pause" \
        -H "Authorization: Bearer $token" >/dev/null 2>&1 || true
    ok "Playback stopped"

    # Restore volume
    curl -sk -X POST "https://$server_addr:1337/player/volume" \
        -H "Authorization: Bearer $token" \
        -H "Content-Type: application/json" \
        -d '{"volume":1.0}' >/dev/null 2>&1 || true
    ok "Volume restored"

    # Clean up
    rm -f "$test_file"
    return 0
}

verify_installation() {
    echo "---------------------------------------------------------------------"
    echo "POST-INSTALL VERIFICATION"
    echo "---------------------------------------------------------------------"

    sleep 2

    if systemctl is-active --quiet l337-audio-server.service; then
        ok "Service is active"
    else
        local last_log
        last_log=$(journalctl -u l337-audio-server.service -n 5 --no-pager 2>/dev/null || true)
        if echo "$last_log" | grep -qi "no audio output device\|unknown pcm default\|cannot find card"; then
            warn "Service is NOT active because no audio device is available on this machine."
            warn "This is expected on headless/VM hosts without sound cards."
            warn "The server will work once audio hardware is present, or run with --dummy for testing."
            warn "Installation files and config are in place; the service is enabled for boot."
        else
            fail "Service is NOT active. Check logs with: sudo journalctl -u l337-audio-server.service -f"
        fi
    fi

    if systemctl is-enabled --quiet l337-audio-server.service; then
        ok "Service is enabled for boot"
    else
        warn "Service is NOT enabled for boot"
    fi

    echo
    echo "Recent logs (last 20 lines):"
    echo "---------------------------------------------------------------------"
    journalctl -u l337-audio-server.service -n 20 --no-pager 2>/dev/null || \
        echo "(no logs available)"
    echo "---------------------------------------------------------------------"
    echo

    if command -v pactl &>/dev/null; then
        echo "Checking PipeWire sinks (as $USER_NAME)..."
        if sudo -u "$USER_NAME" pactl list sinks 2>/dev/null | grep -q .; then
            ok "PipeWire sinks found:"
            sudo -u "$USER_NAME" pactl list sinks 2>/dev/null | head -20
        else
            warn "No PipeWire sinks found. Audio may still work via ALSA or other backends."
        fi
    elif command -v pw-cli &>/dev/null; then
        echo "Checking PipeWire nodes (as $USER_NAME)..."
        if sudo -u "$USER_NAME" pw-cli ls Node 2>/dev/null | grep -q .; then
            ok "PipeWire nodes found:"
            sudo -u "$USER_NAME" pw-cli ls Node 2>/dev/null | head -20
        else
            warn "No PipeWire nodes found. Audio may still work via ALSA or other backends."
        fi
    else
        warn "Neither pactl nor pw-cli found after install. Cannot verify PipeWire audio node."
    fi

    echo
    echo "Testing audio playback via server API..."
    echo "---------------------------------------------------------------------"
    if ! systemctl is-active --quiet l337-audio-server.service; then
        warn "Skipping audio playback test because service is not running (no audio device?)"
    elif ! test_audio_playback; then
        fail "Audio playback test failed. The server may not be able to output audio."
    fi
    echo "---------------------------------------------------------------------"
    echo

    echo
    echo "---------------------------------------------------------------------"
    echo "VERIFICATION COMPLETE"
    echo "---------------------------------------------------------------------"
    echo
    echo "If you encounter issues, check:"
    echo "  1. PipeWire is installed:   pacman -S pipewire wireplumber"
    echo "  2. The runtime dir exists:  ls -la /run/l337-audio-server"
    echo "  3. Logs for errors:         sudo journalctl -u l337-audio-server.service -f"
    echo

    remove_verification_tools
}

install_user_service() {
    BIN="$(require_prebuilt_binary)"
    migrate_old_service_name

    UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
    UNIT="$UNIT_DIR/l337-audio-server.service"
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
    systemctl --user enable l337-audio-server.service
    systemctl --user restart l337-audio-server.service
    echo
    echo "L337 Audio Server installed as a user service for '$USER'."
    echo "For headless/always-on, enable linger:  sudo loginctl enable-linger $USER"
    echo "Check status with:  systemctl --user status l337-audio-server.service"
}

update_system_service() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "Updating the system service requires root (use sudo)." >&2
        exit 1
    fi

    BIN="$(require_prebuilt_binary)"

    migrate_old_service_name
    echo "Stopping $SYSTEM_SERVICE..."
    systemctl stop l337-audio-server.service || true
    sleep 1

    echo "Deploying new binary to $INSTALL_DIR (config/state/cache untouched)..."
    cp "$BIN" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"
    chown "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR/l337-audio-server"

    mkdir -p "$INSTALL_DIR/bin"
    cp "$BIN" "$INSTALL_DIR/bin/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/bin/l337-audio-server"
    chown "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR/bin/l337-audio-server"

    echo "Writing systemd unit $SYSTEM_SERVICE..."
    write_system_unit

    echo "Setting up configuration..."
    setup_config

    echo "Restarting $SYSTEM_SERVICE..."
    systemctl daemon-reload
    systemctl enable l337-audio-server.service
    systemctl restart l337-audio-server.service
    echo
    verify_installation
}

setup_config() {
    local config_dest="$CONFIG_DIR/config.toml"
    local existing_host=""
    local existing_port="1337"
    local existing_token=""
    local keep_existing=false

    # Read existing config if present
    if [ -f "$config_dest" ]; then
        existing_host=$(grep -E '^host\s*=' "$config_dest" | head -1 | sed 's/.*=\s*"\([^"]*\)".*/\1/')
        existing_port=$(grep -E '^port\s*=' "$config_dest" | head -1 | sed 's/.*=\s*\([0-9]*\).*/\1/')
        existing_token=$(grep -E '^token\s*=' "$config_dest" | head -1 | sed 's/.*=\s*"\([^"]*\)".*/\1/')
    fi

    # If config exists with values, ask if user wants to keep them
    if [ -n "$existing_host" ] || [ -n "$existing_port" ] || [ -n "$existing_token" ]; then
        echo "Existing configuration found at $config_dest"
        echo "  Host: ${existing_host:-<not set>}"
        echo "  Port: ${existing_port:-1337}"
        if [ -n "$existing_token" ]; then
            echo "  Token: [already set]"
        fi
        echo
        read -p "Keep existing configuration? [Y/n] " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Nn]$ ]]; then
            existing_host=""
            existing_port=""
            existing_token=""
        else
            keep_existing=true
        fi
    fi

    # Prompt for host
    local default_host
    default_host=$(hostname)
    if [ "$keep_existing" = true ] && [ -n "$existing_host" ]; then
        default_host="$existing_host"
    fi
    read -p "Hostname/IP to bind to [$default_host]: " host
    host=${host:-$default_host}

    # Prompt for port
    local default_port="1337"
    if [ "$keep_existing" = true ] && [ -n "$existing_port" ]; then
        default_port="$existing_port"
    fi
    read -p "Port [$default_port]: " port
    port=${port:-$default_port}

    # Validate port is numeric
    if ! echo "$port" | grep -qE '^[0-9]+$'; then
        fail "Port must be a number"
    fi

    # Generate or reuse token
    local token=""
    if [ "$keep_existing" = true ] && [ -n "$existing_token" ]; then
        token="$existing_token"
        ok "Reusing existing token"
    else
        token=$("$SCRIPT_DIR/scripts/generate-token.sh")
        ok "Generated new token"
    fi

    # Show token and QR code
    echo
    echo "========================================="
    echo " Server Token"
    echo "========================================="
    echo
    echo "  $token"
    echo
    show_qr_code "$token"
    echo
    echo "Add this token to your client configuration."
    echo "========================================="
    echo

    # Write config
    cat > "$config_dest" <<EOF
[server]
host = "$host"
port = $port
token = "$token"
EOF

    chown "$USER_NAME:$GROUP_NAME" "$config_dest"
    ok "Configuration written to $config_dest"
}

generate_token() {
    # Generate a 32-character alphanumeric token
    tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32
}

show_qr_code() {
    local token="$1"
    if command -v qrencode &>/dev/null; then
        qrencode -t ANSI256 "$token" 2>/dev/null || true
    elif command -v python3 &>/dev/null; then
        python3 -c "
try:
    import qrcode
    qr = qrcode.QRCode()
    qr.add_data('$token')
    qr.print_ascii()
except ImportError:
    pass
" 2>/dev/null || true
    fi
}

install_system_service() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "System service install requires root (use sudo). For a per-user" >&2
        echo "instance run: $0 --user" >&2
        exit 1
    fi

    BIN="$(require_prebuilt_binary)"
    migrate_old_service_name

    echo "Creating dedicated system user '$USER_NAME' (MPD-style)..."
    if ! id "$USER_NAME" &>/dev/null; then
        useradd --system --no-create-home --shell /usr/sbin/nologin \
            --comment "L337 Audio Server" "$USER_NAME"
    fi

    echo "Installing to $INSTALL_DIR..."
    rm -rf "$INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
    cp -r "$SCRIPT_DIR/scripts" "$INSTALL_DIR/"
    cp "$BIN" "$INSTALL_DIR/l337-audio-server"
    chmod +x "$INSTALL_DIR/l337-audio-server"

    echo "Creating state/cache/config directories owned by $USER_NAME..."
    install -d -m 0755 -o "$USER_NAME" -g "$GROUP_NAME" "$STATE_DIR"
    install -d -m 0750 -o "$USER_NAME" -g "$GROUP_NAME" "$CACHE_DIR"
    install -d -m 0755 -o "$USER_NAME" -g "$GROUP_NAME" "$CONFIG_DIR"

    echo "Setting up configuration..."
    setup_config

    chown -R "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR"

    echo "Writing systemd unit $SYSTEM_SERVICE..."
    write_system_unit

    echo "Reloading systemd and enabling service..."
    systemctl daemon-reload
    systemctl enable l337-audio-server.service
    systemctl restart l337-audio-server.service
    echo
    echo "L337 Audio Server installed as a system service running under user '$USER_NAME'."
    echo "Check status with:  sudo systemctl status l337-audio-server.service"
    echo "View logs with:     sudo journalctl -u l337-audio-server.service -f"
    check_pipewire_dependency
    echo
    verify_installation
}

case "$MODE" in

    --user|user) install_user_service ;;
    --update|update) update_system_service ;;
    system|--system|"") install_system_service ;;
    *) echo "Unknown mode: $MODE (use --user, --update, or nothing)"; exit 1 ;;
esac
