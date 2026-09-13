#!/bin/bash
# install.sh — Self-contained installer for L337 Audio Server
#
# Downloads prebuilt binaries from GitHub releases and installs them
# with systemd (Linux) or launchd (macOS).
#
# Usage:
#   ./install.sh                          # install or update
#   ./install.sh --uninstall              # remove installation
#   ./install.sh --uninstall --remove-data # remove installation and data
#   ./install.sh --dry-run                # show what would happen
#   ./install.sh --help
set -euo pipefail

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
  --uninstall, -u        Remove the service and installed files
  --remove-data          Also remove configuration and data directories
  -h, --help             Show this help message

Platforms:
  Linux (systemd)        Installs to /opt/l337-audio-server
  macOS (launchd)        Installs to /opt/l337-audio-server
EOF
    exit 0
}

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
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
# Dependency checks
# ---------------------------------------------------------------------------
check_command() {
    if command -v "$1" &>/dev/null; then
        return 0
    else
        fail "Required command not found: $1"
    fi
}

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
get_latest_release_json() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null || \
        fail "Failed to query GitHub releases. Check network connectivity."
}

parse_json_field() {
    local json="$1"
    local field="$2"
    echo "$json" | grep -o "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -1 | sed 's/.*"\([^"]*\)"$/\1/'
}

get_asset_name() {
    local os="$1"
    local arch="$2"
    case "$os-$arch" in
        linux-x86_64)
            if command -v pactl &>/dev/null && pactl info &>/dev/null 2>&1; then
                echo "l337-audio-server-x86_64-linux-pipewire"
            else
                echo "l337-audio-server-x86_64-linux-alsa"
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

    if ! id "$USER_NAME" &>/dev/null; then
        info "Creating system user: $USER_NAME"
        groupadd --system "$GROUP_NAME" 2>/dev/null || true
        useradd --system --no-create-home --shell /usr/sbin/nologin --gid "$GROUP_NAME" "$USER_NAME" 2>/dev/null || true
    fi

    mkdir -p "$INSTALL_DIR" "$STATE_DIR" "$CACHE_DIR" "$CONFIG_DIR"

    info "Installing binary to $INSTALL_DIR..."
    cp "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"
    chown "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR/l337-audio-server"

    if [ ! -f "$CONFIG_DIR/config.toml" ]; then
        info "Creating default configuration..."
        local token
        token=$(tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32)
        cat > "$CONFIG_DIR/config.toml" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = false
transport = "auto"
EOF
        chown "$USER_NAME:$GROUP_NAME" "$CONFIG_DIR/config.toml"
        chmod 0640 "$CONFIG_DIR/config.toml"
        ok "Configuration written to $CONFIG_DIR/config.toml"
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

    local config_dir
    config_dir=$(eval echo "~${real_user}/.config/l337-audio-server")
    mkdir -p "$config_dir"

    if [ ! -f "$config_dir/config.toml" ]; then
        info "Creating default configuration..."
        local token
        token=$(tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32)
        cat > "$config_dir/config.toml" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = false
transport = "auto"
EOF
        ok "Configuration written to $config_dir/config.toml"
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
if [ "$UNINSTALL" = true ]; then
    case "$OS_TYPE" in
        linux)  uninstall_linux ;;
        macos)  uninstall_macos ;;
    esac
    exit 0
fi

if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    info "  Query GitHub for latest release"
    info "  Download the latest binary for $OS_TYPE/$ARCH_TYPE"
    info "  Install to: $INSTALL_DIR/l337-audio-server"
    case "$OS_TYPE" in
        linux)
            info "  Configure systemd service: $SYSTEMD_SERVICE"
            info "  Create user/group: $USER_NAME/$GROUP_NAME"
            info "  Create directories: $INSTALL_DIR, $STATE_DIR, $CACHE_DIR, $CONFIG_DIR"
            ;;
        macos)
            info "  Configure launchd plist"
            info "  Create directories: $INSTALL_DIR"
            ;;
    esac
    exit 0
fi

info "Checking for latest release..."
RELEASE_JSON=$(get_latest_release_json)
LATEST_TAG=$(parse_json_field "$RELEASE_JSON" "tag_name")
PUBLISHED_AT=$(parse_json_field "$RELEASE_JSON" "published_at")

if [ -z "$LATEST_TAG" ]; then
    fail "Could not determine latest release tag"
fi

info "Latest release: $LATEST_TAG (published: $PUBLISHED_AT)"

ASSET_NAME=$(get_asset_name "$OS_TYPE" "$ARCH_TYPE")
info "Selected asset: $ASSET_NAME"

# Check if already installed and up-to-date
INSTALLED_VERSION=""
if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
    info "Existing installation found at: $INSTALL_DIR/l337-audio-server"
    INSTALLED_VERSION=$(get_installed_version)

    if [ -n "$INSTALLED_VERSION" ]; then
        info "Installed version: $INSTALLED_VERSION"
        if version_eq "$INSTALLED_VERSION" "$LATEST_TAG"; then
            ok "Installed binary is up-to-date ($INSTALLED_VERSION)"
            exit 0
        elif version_gt "$LATEST_TAG" "$INSTALLED_VERSION"; then
            info "Update available ($INSTALLED_VERSION -> $LATEST_TAG)"
        else
            ok "Installed binary ($INSTALLED_VERSION) is newer than latest release ($LATEST_TAG)"
            exit 0
        fi
    else
        warn "Could not determine installed version; will reinstall"
    fi
fi

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"
TMP_BIN="/tmp/${ASSET_NAME}.tmp"

download_binary "$DOWNLOAD_URL" "$TMP_BIN"

if [ "$OS_TYPE" = "linux" ]; then
    setup_systemd "$TMP_BIN"
elif [ "$OS_TYPE" = "macos" ]; then
    setup_launchd "$TMP_BIN"
fi

rm -f "$TMP_BIN"
ok "Installation complete"
echo
if [ "$OS_TYPE" = "linux" ]; then
    echo "Next steps:"
    echo "  Check status:    systemctl status l337-audio-server.service"
    echo "  View logs:       journalctl -u l337-audio-server.service -f"
    echo "  Configuration:   $CONFIG_DIR/config.toml"
elif [ "$OS_TYPE" = "macos" ]; then
    echo "Next steps:"
    echo "  Check status:    launchctl list | grep $PLIST_LABEL"
    echo "  View logs:       tail -f ~/Library/Logs/$PLIST_LABEL.log"
    echo "  Configuration:   ~/.config/l337-audio-server/config.toml"
fi
