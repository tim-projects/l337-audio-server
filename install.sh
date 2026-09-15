#!/bin/bash
# install.sh — Self-contained installer for L337 Audio Server
#
# Downloads prebuilt binaries from GitHub releases and installs them
# with systemd (Linux) or launchd (macOS).
#
# Usage:
#   ./install.sh                          # install or update latest stable release
#   ./install.sh --pre-release            # install or update latest prerelease
#   ./install.sh --uninstall              # remove installation
#   ./install.sh --uninstall --remove-data # remove installation and data
#   ./install.sh --dry-run                # show what would happen
#   ./install.sh --help
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO="tim-projects/l337-audio-server"
INSTALL_DIR="/opt/l337-audio-server"
CONFIG_DIR="/etc/l337-audio-server"
STATE_DIR="/var/lib/l337-audio-server"
CACHE_DIR="/var/cache/l337-audio-server"
SYSTEMD_SERVICE="/etc/systemd/system/l337-audio-server.service"
PLIST_LABEL="com.l337.audio-server"
USER_NAME="l337"
GROUP_NAME="l337"

# ---------------------------------------------------------------------------
# Flags
# ---------------------------------------------------------------------------
DRY_RUN=false
UNINSTALL=false
REMOVE_DATA=false
INSTALL_PRERELEASE=false
FORCE=false
USER_INSTALL=false

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Self-contained installer for L337 Audio Server.
Downloads prebuilt binaries from GitHub releases and sets up the system service.

Options:
  --dry-run              Show what would happen without making changes
  --pre-release, --prerelease
                         Install the latest prerelease instead of stable release
  --force, -f            Force reinstall even if the same version is already installed
  --user                 Force user-level systemd service (requires PipeWire)
  --uninstall, -u        Remove the service and installed files
  --remove-data          Also remove configuration and data directories
  -h, --help             Show this help message

Platforms:
  Linux (systemd)        Installs to /opt/l337-audio-server
  Linux (systemd --user) Installs to /opt/l337-audio-server, config in ~/.config/
  macOS (launchd)        Installs to /opt/l337-audio-server

Notes:
  - On Linux, the installer auto-detects PipeWire and installs accordingly:
      user PipeWire   → systemd user service (PipeWire)
      system PipeWire → system-wide service (PipeWire)
      no PipeWire     → system-wide service (ALSA)
  - Use --user to force a user-level service even when system PipeWire is present.
  - All Linux installs require sudo because /opt/l337-audio-server is system-owned.
EOF
    exit 0
}

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

check_command() {
    if command -v "$1" &>/dev/null; then
        return 0
    else
        fail "Required command not found: $1"
    fi
}

check_dependencies() {
    case "$OS_TYPE" in
        linux)
            check_command "curl"
            check_command "systemctl"
            check_command "file"
            check_command "groupadd"
            check_command "useradd"
            ;;
        macos)
            check_command "curl"
            check_command "launchctl"
            check_command "file"
            ;;
    esac
}

check_dependencies_user() {
    case "$OS_TYPE" in
        linux)
            check_command "curl"
            check_command "systemctl"
            check_command "file"
            ;;
        macos)
            check_command "curl"
            check_command "launchctl"
            check_command "file"
            ;;
    esac
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        fail "System installation requires root privileges. Run with: sudo $0 [OPTIONS]"
    fi
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        --force|-f) FORCE=true; shift ;;
        --user) USER_INSTALL=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help) usage ;;
        *) fail "Unknown option: $1" ;;
    esac
done

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux*)     OS_TYPE="linux" ;;
    Darwin*)    OS_TYPE="macos" ;;
    *)          fail "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH_TYPE="x86_64" ;;
    aarch64|arm64) ARCH_TYPE="aarch64" ;;
    *)             fail "Unsupported architecture: $ARCH" ;;
esac

info "Platform: $OS_TYPE / $ARCH_TYPE"

# ---------------------------------------------------------------------------
# Cross-platform helpers
# ---------------------------------------------------------------------------
get_mtime() {
    if [ "$OS_TYPE" = "linux" ]; then
        stat -c %Y "$1" 2>/dev/null || echo "0"
    else
        stat -f %m "$1" 2>/dev/null || echo "0"
    fi
}

date_to_epoch() {
    if [ "$OS_TYPE" = "linux" ]; then
        date -d "$1" +%s 2>/dev/null || echo "0"
    else
        date -j -f "%Y-%m-%dT%H:%M:%SZ" "$1" +%s 2>/dev/null || echo "0"
    fi
}

# ---------------------------------------------------------------------------
# GitHub release helpers
# ---------------------------------------------------------------------------
extract_prerelease() {
    awk '
        /^  \{/ {
            release=$0 "\n"
            in_release=1
            next
        }
        in_release {
            release=release $0 "\n"
            if ($0 ~ /^  \},?$/) {
                if (release ~ /"prerelease"[[:space:]]*:[[:space:]]*true/) {
                    printf "%s", release
                    exit
                }
                release=""
                in_release=0
            }
        }
    '
}

get_latest_release_json() {
    local endpoint
    local url
    local response_file
    local http_code
    local message
    local release

    if [ "$INSTALL_PRERELEASE" = true ]; then
        endpoint="releases?per_page=100"
    else
        endpoint="releases/latest"
    fi

    url="https://api.github.com/repos/${REPO}/${endpoint}"
    response_file=$(mktemp "${TMPDIR:-/tmp}/l337-release.XXXXXX") || \
        fail "Could not create a temporary file for GitHub release data."

    if ! http_code=$(curl -sS -L --connect-timeout 15 --max-time 60 \
        -o "$response_file" -w '%{http_code}' "$url"); then
        rm -f "$response_file"
        fail "Failed to contact GitHub Releases API. Check network connectivity or proxy settings."
    fi

    if [ "$http_code" != "200" ]; then
        message=$(parse_json_field "$(cat "$response_file")" "message" 2>/dev/null || true)
        rm -f "$response_file"
        if [ "$INSTALL_PRERELEASE" = true ]; then
            fail "GitHub returned HTTP $http_code while querying prereleases. ${message:-}"
        fi
        fail "GitHub returned HTTP $http_code while querying stable releases. ${message:-} Re-run with --pre-release to install a prerelease."
    fi

    if [ "$INSTALL_PRERELEASE" = true ]; then
        release=$(extract_prerelease < "$response_file")
    else
        release=$(cat "$response_file")
    fi
    rm -f "$response_file"

    if [ -z "$release" ]; then
        if [ "$INSTALL_PRERELEASE" = true ]; then
            fail "No prerelease found for ${REPO}."
        fi
        fail "No stable release found for ${REPO}."
    fi

    printf '%s\n' "$release"
}

parse_json_field() {
    local json="$1"
    local field="$2"
    echo "$json" | grep -o "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -1 | sed 's/.*"\([^"]*\)"$/\1/' || true
}

get_asset_name() {
    local os="$1"
    local arch="$2"
    local service_context="${3:-}"
    case "$os-$arch" in
        linux-x86_64)
            if [ "$service_context" = "user" ]; then
                if _check_pipewire; then
                    echo "l337-audio-server-x86_64-linux-pipewire"
                else
                    fail "PipeWire is required for user-level installation. Start PipeWire or use a system-wide installation."
                fi
            else
                if _check_pipewire; then
                    echo "l337-audio-server-x86_64-linux-pipewire"
                else
                    echo "l337-audio-server-x86_64-linux-alsa"
                fi
            fi
            ;;
        linux-aarch64)
            fail "aarch64 Linux binaries are not yet available in GitHub releases. Please build from source."
            ;;
        macos-x86_64)
            echo "l337-audio-server-x86_64-apple-darwin"
            ;;
        macos-aarch64)
            warn "Apple Silicon Mac detected. Downloading x86_64 build (runs under Rosetta 2)."
            echo "l337-audio-server-x86_64-apple-darwin"
            ;;
        *)
            fail "Unsupported platform: $os/$arch"
            ;;
    esac
}

# Detect whether PipeWire is running and how it is installed.
# Returns one of: user, system, none
detect_pipewire_mode() {
    local check_user="${SUDO_USER:-${USER}}"
    local user_uid
    user_uid=$(id -u "$check_user" 2>/dev/null || echo "")

    # 1. Check for per-user PipeWire socket/runtime dir
    if [ -n "$user_uid" ] && [ -d "/run/user/$user_uid" ]; then
        if [ -S "/run/user/$user_uid/pipewire-0" ] || [ -S "/run/user/$user_uid/pulse/native" ]; then
            echo "user"
            return
        fi
    fi

    # 2. Check for system-wide PipeWire socket/runtime dir
    if [ -S "/run/pipewire/0" ] || [ -d "/run/pipewire" ]; then
        echo "system"
        return
    fi

    # 3. Check via systemctl (as the relevant user)
    if [ "$check_user" != "root" ] && [ -n "$check_user" ]; then
        if su - "$check_user" -c "systemctl --user is-active --quiet pipewire 2>/dev/null" 2>/dev/null; then
            echo "user"
            return
        fi
    fi

    if systemctl is-active --quiet pipewire 2>/dev/null; then
        echo "system"
        return
    fi

    # 4. Fallback: pactl/pw-info checks as the desktop user
    if [ "$check_user" != "root" ] && [ -n "$check_user" ]; then
        if su - "$check_user" -c 'command -v pactl >/dev/null 2>&1 && pactl info >/dev/null 2>&1' 2>/dev/null; then
            echo "user"
            return
        fi
    fi

    echo "none"
}

# PipeWire/PulseAudio availability must be checked as the desktop user, not
# root, because the audio sockets live in the user's runtime dir. When the
# installer is invoked via sudo, SUDO_USER points to the real user.
_check_pipewire() {
    [ "$(detect_pipewire_mode)" != "none" ]
}

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------
download_binary() {
    local url="$1"
    local dest="$2"
    local tmp="${dest}.tmp.$$"

    info "Downloading: $url"
    if ! curl -fL --connect-timeout 15 --retry 2 --retry-delay 2 -o "$tmp" "$url"; then
        rm -f "$tmp"
        fail "Failed to download binary from $url"
    fi

    if [ ! -s "$tmp" ]; then
        rm -f "$tmp"
        fail "Downloaded file is empty. Check the release URL."
    fi

    if ! file "$tmp" | grep -qE "ELF|Mach-O"; then
        rm -f "$tmp"
        fail "Downloaded file is not a valid binary (got: $(file -b "$tmp")). Check the release URL."
    fi

    chmod +x "$tmp"
    mv "$tmp" "$dest"
    local size
    size=$(du -h "$dest" | cut -f1)
    ok "Downloaded: $dest ($size)"
}

# ---------------------------------------------------------------------------
# Linux: systemd setup
# ---------------------------------------------------------------------------
setup_systemd() {
    local bin_path="$1"

    info "Configuring systemd service..."

    if ! id -g "$GROUP_NAME" &>/dev/null; then
        info "Creating system group: $GROUP_NAME"
        groupadd --system "$GROUP_NAME" || \
            fail "Failed to create system group $GROUP_NAME. Run the installer with sudo and ensure groupadd is available."
    fi

    if ! id "$USER_NAME" &>/dev/null; then
        info "Creating system user: $USER_NAME"
        useradd --system --no-create-home --shell /usr/sbin/nologin --gid "$GROUP_NAME" "$USER_NAME" || \
            fail "Failed to create system user $USER_NAME. Run the installer with sudo and ensure useradd is available."
    fi

    if ! groups "$USER_NAME" 2>/dev/null | grep -qw "audio"; then
        info "Adding $USER_NAME to audio group for ALSA access"
        usermod -aG audio "$USER_NAME" || \
            warn "Failed to add $USER_NAME to audio group; ALSA access may not be available."
    fi

    mkdir -p "$INSTALL_DIR" "$STATE_DIR" "$CACHE_DIR" "$CONFIG_DIR"

    info "Installing binary to $INSTALL_DIR..."
    cp "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"
    chown "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR/l337-audio-server"

    chown "$USER_NAME:$GROUP_NAME" "$CONFIG_DIR" || \
        fail "Failed to set ownership on $CONFIG_DIR."
    chmod 0755 "$CONFIG_DIR"

    local config_file="$CONFIG_DIR/server.ini"
    local legacy_config_file="$CONFIG_DIR/config.toml"
    info "Ensuring configuration at $config_file..."
    if [ ! -f "$config_file" ]; then
        local token
        token=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32 || true)
        if [ "${#token}" -ne 32 ]; then
            fail "Failed to generate a secure server token."
        fi
        if [ -f "$legacy_config_file" ]; then
            info "Migrating legacy configuration: $legacy_config_file"
            cp "$legacy_config_file" "$config_file" || \
                fail "Failed to migrate legacy configuration to $config_file."
        else
            cat > "$config_file" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = false
transport = "auto"
EOF
            echo
            echo "========================================="
            echo " Server Token"
            echo "========================================="
            echo
            echo "  ${token}"
            echo
            echo "Add this token to your client configuration."
            echo "========================================="
            echo
        fi
    fi

    chown "$USER_NAME:$GROUP_NAME" "$config_file" || \
        fail "Failed to set ownership on $config_file."
    chmod 0640 "$config_file"
    ok "Configuration ready at $config_file"

    info "Writing systemd unit: $SYSTEMD_SERVICE"
    cat > "$SYSTEMD_SERVICE" <<EOF
[Unit]
Description=L337 Audio Server
Documentation=https://github.com/${REPO}
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
User=${USER_NAME}
Group=${GROUP_NAME}
WorkingDirectory=${INSTALL_DIR}
ExecStart=${INSTALL_DIR}/l337-audio-server
Restart=on-failure
RestartSec=2

StateDirectory=l337-audio-server
CacheDirectory=l337-audio-server
ConfigurationDirectory=l337-audio-server
RuntimeDirectory=l337-audio-server
RuntimeDirectoryMode=0700

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ProtectControlGroups=true
ProtectKernelModules=false
ProtectKernelTunables=false
RestrictNamespaces=false
RestrictRealtime=false
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=false
ReadWritePaths=${STATE_DIR} ${CACHE_DIR}

[Install]
WantedBy=multi-user.target
EOF

    chmod 0644 "$SYSTEMD_SERVICE"

    if systemctl is-active --quiet l337-audio-server.service 2>/dev/null; then
        info "Stopping running service..."
        systemctl stop l337-audio-server.service || true
    fi

    systemctl daemon-reload
    systemctl enable l337-audio-server.service
    systemctl start l337-audio-server.service

    ok "Systemd service installed and started"
}

# ---------------------------------------------------------------------------
# Linux: systemd user service setup (PipeWire)
# ---------------------------------------------------------------------------
setup_systemd_user() {
    local bin_path="$1"
    local real_user="${SUDO_USER:-${USER}}"

    info "Configuring systemd user service..."

    local real_home
    real_home=$(eval echo "~${real_user}")
    local user_config_dir="$real_home/.config/l337-audio-server"
    local user_cache_dir="$real_home/.cache/l337-audio-server"
    local user_state_dir="$real_home/.local/state/l337-audio-server"
    local user_runtime_dir="$real_home/.local/run/l337-audio-server"
    local user_service_dir="$real_home/.config/systemd/user"
    local user_service_file="$user_service_dir/l337-audio-server.service"

    mkdir -p "$INSTALL_DIR" "$user_config_dir" "$user_cache_dir" "$user_state_dir" "$user_runtime_dir" "$user_service_dir"

    info "Installing binary to $INSTALL_DIR..."
    cp "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"

    local config_file="$user_config_dir/server.ini"
    local legacy_config_file="$user_config_dir/config.toml"
    info "Ensuring configuration at $config_file..."
    if [ ! -f "$config_file" ]; then
        local token
        token=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32 || true)
        if [ "${#token}" -ne 32 ]; then
            fail "Failed to generate a secure server token."
        fi
        if [ -f "$legacy_config_file" ]; then
            info "Migrating legacy configuration: $legacy_config_file"
            cp "$legacy_config_file" "$config_file" || \
                fail "Failed to migrate legacy configuration to $config_file."
        else
            cat > "$config_file" <<EOF
[server]
host = "127.0.0.1"
port = 1337
token = "${token}"
dummy = false
transport = "auto"
EOF
            echo
            echo "========================================="
            echo " Server Token"
            echo "========================================="
            echo
            echo "  ${token}"
            echo
            echo "Add this token to your client configuration."
            echo "========================================="
            echo
        fi
    fi

    chown "$real_user" "$config_file" 2>/dev/null || true
    chmod 0600 "$config_file"
    ok "Configuration ready at $config_file"

    info "Writing systemd user unit: $user_service_file"
    cat > "$user_service_file" <<EOF
[Unit]
Description=L337 Audio Server (user service, PipeWire)
Documentation=https://github.com/${REPO}
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
User=${real_user}
Group=${real_user}
WorkingDirectory=${INSTALL_DIR}
ExecStart=${INSTALL_DIR}/l337-audio-server
Restart=on-failure
RestartSec=2

Environment=HOME=${real_home}
Environment=XDG_CONFIG_HOME=${real_home}/.config
Environment=XDG_CACHE_HOME=${real_home}/.cache
Environment=XDG_STATE_HOME=${real_home}/.local/state
Environment=XDG_RUNTIME_DIR=${real_home}/.local/run/l337-audio-server

NoNewPrivileges=true
PrivateTmp=true
ProtectControlGroups=true
ProtectKernelModules=false
ProtectKernelTunables=false
RestrictNamespaces=false
RestrictRealtime=false
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=false

[Install]
WantedBy=default.target
EOF

    chown "$real_user" "$user_service_file" 2>/dev/null || true
    chmod 0644 "$user_service_file"

    info "Enabling and starting user service..."
    su - "$real_user" -c "systemctl --user daemon-reload" || true
    su - "$real_user" -c "systemctl --user enable l337-audio-server.service" || true
    su - "$real_user" -c "systemctl --user start l337-audio-server.service" || true

    ok "Systemd user service installed and started"
}

# ---------------------------------------------------------------------------
# macOS: launchd setup
# ---------------------------------------------------------------------------
setup_launchd() {
    local bin_path="$1"
    local real_user="${SUDO_USER:-${USER}}"

    info "Configuring launchd service..."

    mkdir -p "$INSTALL_DIR"

    info "Installing binary to $INSTALL_DIR..."
    cp "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"

    local real_home
    real_home=$(eval echo "~${real_user}")
    local config_dir
    config_dir="$real_home/Library/Application Support/l337-audio-server"
    mkdir -p "$config_dir"

    local config_file="$config_dir/server.ini"
    local legacy_config_file="$config_dir/config.toml"
    if [ ! -f "$legacy_config_file" ] && [ -f "$real_home/.config/l337-audio-server/config.toml" ]; then
        legacy_config_file="$real_home/.config/l337-audio-server/config.toml"
    fi
    if [ ! -f "$config_file" ]; then
        info "Creating default configuration..."
        local token
        token=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32 || true)
        if [ "${#token}" -ne 32 ]; then
            fail "Failed to generate a secure server token."
        fi
        if [ -f "$legacy_config_file" ]; then
            info "Migrating legacy configuration: $legacy_config_file"
            cp "$legacy_config_file" "$config_file" || \
                fail "Failed to migrate legacy configuration to $config_file."
        else
            cat > "$config_file" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = false
transport = "auto"
EOF
            ok "Configuration written to $config_file"
            echo
            echo "========================================="
            echo " Server Token"
            echo "========================================="
            echo
            echo "  ${token}"
            echo
            echo "Add this token to your client configuration."
            echo "========================================="
            echo
        fi
        chown "$real_user" "$config_file" || \
            fail "Failed to set ownership on $config_file."
        chmod 0600 "$config_file"
    fi

    local plist_dir
    plist_dir=$(eval echo "~${real_user}/Library/LaunchAgents")
    mkdir -p "$plist_dir"
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"
    local log_dir
    log_dir=$(eval echo "~${real_user}/Library/Logs")
    mkdir -p "$log_dir"
    local log_path="${log_dir}/${PLIST_LABEL}.log"

    info "Writing launchd plist: $plist_path"
    cat > "$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${PLIST_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${INSTALL_DIR}/l337-audio-server</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${log_path}</string>
    <key>StandardErrorPath</key>
    <string>${log_path}</string>
</dict>
</plist>
EOF

    if launchctl list | grep -q "$PLIST_LABEL"; then
        info "Stopping existing service..."
        launchctl unload "$plist_path" 2>/dev/null || true
    fi

    launchctl load "$plist_path"
    ok "Launchd service installed and started"
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------
uninstall_linux() {
    info "Uninstalling from Linux..."

    if systemctl is-active --quiet l337-audio-server.service 2>/dev/null; then
        systemctl stop l337-audio-server.service || true
    fi
    if systemctl is-enabled --quiet l337-audio-server.service 2>/dev/null; then
        systemctl disable l337-audio-server.service || true
    fi

    rm -f "$SYSTEMD_SERVICE"
    systemctl daemon-reload 2>/dev/null || true

    rm -rf "$INSTALL_DIR"

    if [ "$REMOVE_DATA" = true ]; then
        info "Removing data directories..."
        rm -rf "$CONFIG_DIR" "$STATE_DIR" "$CACHE_DIR"
        if id "$USER_NAME" &>/dev/null; then
            userdel "$USER_NAME" 2>/dev/null || true
        fi
        if getent group "$GROUP_NAME" &>/dev/null; then
            groupdel "$GROUP_NAME" 2>/dev/null || true
        fi
        ok "Data directories removed"
    else
        warn "Data directories retained:"
        warn "  $CONFIG_DIR"
        warn "  $STATE_DIR"
        warn "  $CACHE_DIR"
        warn "Re-run with --remove-data to delete them."
    fi

    ok "Uninstallation complete"
}

uninstall_macos() {
    info "Uninstalling from macOS..."

    local real_user="${SUDO_USER:-${USER}}"
    local plist_dir
    plist_dir=$(eval echo "~${real_user}/Library/LaunchAgents")
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"

    if launchctl list | grep -q "$PLIST_LABEL"; then
        launchctl unload "$plist_path" 2>/dev/null || true
    fi

    rm -f "$plist_path"
    rm -rf "$INSTALL_DIR"

    ok "Uninstallation complete"
}

uninstall_user_service() {
    info "Uninstalling systemd user service..."

    local real_user="${SUDO_USER:-${USER}}"
    local real_home
    real_home=$(eval echo "~${real_user}")
    local user_service_dir="$real_home/.config/systemd/user"
    local user_service_file="$user_service_dir/l337-audio-server.service"

    su - "$real_user" -c "systemctl --user stop l337-audio-server.service" 2>/dev/null || true
    su - "$real_user" -c "systemctl --user disable l337-audio-server.service" 2>/dev/null || true

    rm -f "$user_service_file"
    su - "$real_user" -c "systemctl --user daemon-reload" 2>/dev/null || true

    rm -rf "$INSTALL_DIR"

    if [ "$REMOVE_DATA" = true ]; then
        info "Removing user data directories..."
        rm -rf "$real_home/.config/l337-audio-server" \
              "$real_home/.cache/l337-audio-server" \
              "$real_home/.local/state/l337-audio-server" \
              "$real_home/.local/run/l337-audio-server"
        ok "User data directories removed"
    else
        warn "User data directories retained:"
        warn "  $real_home/.config/l337-audio-server"
        warn "  $real_home/.cache/l337-audio-server"
        warn "  $real_home/.local/state/l337-audio-server"
        warn "  $real_home/.local/run/l337-audio-server"
        warn "Re-run with --remove-data to delete them."
    fi

    ok "User service uninstallation complete"
}

# ---------------------------------------------------------------------------
# Version helpers
# ---------------------------------------------------------------------------
get_installed_version() {
    if [ -x "$INSTALL_DIR/l337-audio-server" ]; then
        "$INSTALL_DIR/l337-audio-server" --version 2>/dev/null | awk '{print $NF}'
    else
        echo ""
    fi
}

version_gt() {
    local a="$1"
    local b="$2"

    if [ -z "$a" ] || [ -z "$b" ]; then
        return 1
    fi

    local a_date="${a%-*}"
    local a_build="${a##*-}"
    local b_date="${b%-*}"
    local b_build="${b##*-}"

    if [ "$a_date" != "$b_date" ]; then
        [ "$a_date" \> "$b_date" ]
        return $?
    fi

    if [ "$a_build" = "$a" ] || [ "$b_build" = "$b" ]; then
        return 1
    fi

    [ "$a_build" -gt "$b_build" ] 2>/dev/null
}

version_eq() {
    [ "$1" = "$2" ]
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    if [ "$UNINSTALL" = true ]; then
        info "  Remove the service and installed files"
        if [ "$REMOVE_DATA" = true ]; then
            info "  Remove configuration and data directories"
        else
            info "  Retain configuration and data directories"
        fi
    elif [ "$INSTALL_PRERELEASE" = true ]; then
        info "  Query GitHub for latest prerelease"
    else
        info "  Query GitHub for latest stable release"
    fi
    if [ "$UNINSTALL" = false ]; then
        info "  Download the latest binary for $OS_TYPE/$ARCH_TYPE"
        info "  Install to: $INSTALL_DIR/l337-audio-server"
        if [ "$USER_INSTALL" = true ]; then
            info "  Install systemd user service"
            info "  Config: ~/.config/l337-audio-server/server.ini"
        else
            case "$OS_TYPE" in
                linux)
                    info "  Configure systemd service: $SYSTEMD_SERVICE"
                    info "  Create user/group: $USER_NAME/$GROUP_NAME"
                    info "  Add $USER_NAME to audio group"
                    info "  Create directories: $INSTALL_DIR, $STATE_DIR, $CACHE_DIR, $CONFIG_DIR"
                    ;;
                macos)
                    info "  Configure launchd plist"
                    info "  Create directories: $INSTALL_DIR"
                    ;;
            esac
        fi
    fi
    exit 0
fi

if [ "$UNINSTALL" = true ]; then
    if [ "$USER_INSTALL" = true ]; then
        uninstall_user_service
    else
        require_root
        case "$OS_TYPE" in
            linux)
                check_command "systemctl"
                uninstall_linux
                ;;
            macos)
                check_command "launchctl"
                uninstall_macos
                ;;
        esac
    fi
    exit 0
fi

if [ "$USER_INSTALL" = true ]; then
    check_dependencies_user
else
    require_root
    check_dependencies
fi

info "Checking for latest release..."
RELEASE_JSON=$(get_latest_release_json)
LATEST_TAG=$(parse_json_field "$RELEASE_JSON" "tag_name")
PUBLISHED_AT=$(parse_json_field "$RELEASE_JSON" "published_at")

if [ -z "$LATEST_TAG" ]; then
    fail "Could not determine latest release tag"
fi

info "Latest release: $LATEST_TAG (published: $PUBLISHED_AT)"

# Auto-detect PipeWire installation mode when the user did not explicitly
# request --user. We match the server's service model to the existing
# PipeWire model: user PipeWire → user service, system PipeWire → system
# service, no PipeWire → system service with ALSA.
if [ "$OS_TYPE" = "linux" ] && [ "$USER_INSTALL" = false ]; then
    PW_MODE=$(detect_pipewire_mode)
    info "Detected PipeWire mode: $PW_MODE"
    if [ "$PW_MODE" = "user" ]; then
        USER_INSTALL=true
        info "User-level PipeWire detected; installing as systemd user service."
    elif [ "$PW_MODE" = "system" ]; then
        info "System-level PipeWire detected; installing system-wide."
    else
        info "No PipeWire detected; installing system-wide with ALSA."
    fi
fi

if [ "$OS_TYPE" = "linux" ]; then
    if [ "$USER_INSTALL" = true ]; then
        ASSET_NAME=$(get_asset_name "$OS_TYPE" "$ARCH_TYPE" "user")
    else
        ASSET_NAME=$(get_asset_name "$OS_TYPE" "$ARCH_TYPE" "system")
    fi
else
    ASSET_NAME=$(get_asset_name "$OS_TYPE" "$ARCH_TYPE" "")
fi
info "Selected asset: $ASSET_NAME"

# Check if already installed and up-to-date
INSTALLED_VERSION=""
if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
    info "Existing installation found at: $INSTALL_DIR/l337-audio-server"
    INSTALLED_VERSION=$(get_installed_version)

    if [ -n "$INSTALLED_VERSION" ]; then
        info "Installed version: $INSTALLED_VERSION"
        if version_eq "$INSTALLED_VERSION" "$LATEST_TAG"; then
            if [ "$FORCE" = true ]; then
                info "Force reinstall requested for the same version ($INSTALLED_VERSION)"
            else
                ok "Installed binary is up-to-date ($INSTALLED_VERSION)"
                echo
                echo "To reinstall anyway, run with --force:"
                echo "  sudo $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')-u"
                exit 0
            fi
        elif version_gt "$LATEST_TAG" "$INSTALLED_VERSION"; then
            info "Update available ($INSTALLED_VERSION -> $LATEST_TAG)"
        else
            if [ "$FORCE" = true ]; then
                info "Force reinstall requested even though installed ($INSTALLED_VERSION) is newer than release ($LATEST_TAG)"
            else
                ok "Installed binary ($INSTALLED_VERSION) is newer than latest release ($LATEST_TAG)"
                echo
                echo "To reinstall anyway, run with --force:"
                echo "  sudo $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
                exit 0
            fi
        fi
    else
        warn "Could not determine installed version; will reinstall"
    fi
fi

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"
TMP_BIN="/tmp/${ASSET_NAME}.tmp"

download_binary "$DOWNLOAD_URL" "$TMP_BIN"

if [ "$USER_INSTALL" = true ]; then
    setup_systemd_user "$TMP_BIN"
elif [ "$OS_TYPE" = "linux" ]; then
    setup_systemd "$TMP_BIN"
elif [ "$OS_TYPE" = "macos" ]; then
    setup_launchd "$TMP_BIN"
fi

rm -f "$TMP_BIN"
ok "Installation complete"
echo
if [ "$USER_INSTALL" = true ]; then
    echo "Next steps:"
    echo "  Check status:    systemctl --user status l337-audio-server.service"
    echo "  View logs:       journalctl --user -u l337-audio-server -f"
    echo "  Configuration:   ~/.config/l337-audio-server/server.ini"
elif [ "$OS_TYPE" = "linux" ]; then
    echo "Next steps:"
    echo "  Check status:    systemctl status l337-audio-server.service"
    echo "  View logs:       journalctl -u l337-audio-server.service -f"
    echo "  Configuration:   $CONFIG_DIR/server.ini"
elif [ "$OS_TYPE" = "macos" ]; then
    echo "Next steps:"
    echo "  Check status:    launchctl list | grep $PLIST_LABEL"
    echo "  View logs:       tail -f ~/Library/Logs/$PLIST_LABEL.log"
    echo "  Configuration:   ~/Library/Application Support/l337-audio-server/server.ini"
fi
