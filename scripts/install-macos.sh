#!/bin/bash
# install-macos.sh — macOS launchd installer
#
# Installs a downloaded l337-audio-server binary as a launchd user service.
#
# This installer never talks to GitHub: install.sh resolves the newest release
# that ships the macOS binary (walking releases and reading each release's
# release-info.json, so a partial release is skipped instead of downloading into
# a 404), downloads it, and passes the path with --binary. Apple Silicon installs
# the x86_64 build, which runs under Rosetta 2.
#
# Usage:
#   install.sh                                              # install/update (recommended)
#   ./scripts/install-macos.sh --binary <path>              # install a downloaded binary
#   ./scripts/install-macos.sh --uninstall
#   ./scripts/install-macos.sh --uninstall --remove-data
#   ./scripts/install-macos.sh --dry-run --binary <path>
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO="tim-projects/l337-audio-server"
INSTALL_DIR="/opt/l337-audio-server"
PLIST_LABEL="com.l337.audio-server"
REAL_USER="${SUDO_USER:-${USER}}"

# ---------------------------------------------------------------------------
# Flags
# ---------------------------------------------------------------------------
DRY_RUN=false
UNINSTALL=false
REMOVE_DATA=false
NO_AUDIO=false
# Binary handed over by install.sh (which owns release lookup and the download).
BINARY_INPUT="${L337_BINARY_PATH:-}"
RELEASE_TAG="${L337_RELEASE_TAG:-}"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
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
    check_command "curl"
    check_command "launchctl"
    check_command "file"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --binary) BINARY_INPUT="$2"; shift 2 ;;
        --release) RELEASE_TAG="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        --no-audio) NO_AUDIO=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [OPTIONS] --binary <path>

macOS installer for L337 Audio Server.
Installs a downloaded binary as a launchd user service. Release lookup, the
version check and the download itself live in install.sh, which resolves the
newest release that ships a macOS binary and passes --binary here.

Options:
  --binary <path>        Binary to install (required for install; passed by install.sh)
  --release <tag>        Release tag the binary came from (informational)
  --dry-run              Show what would happen without making changes
  --no-audio             Set dummy = true in server.ini (run without audio hardware)
  --uninstall, -u        Remove the service and installed files
  --remove-data          Also remove configuration and data directories
  -h, --help             Show this help message
EOF
            exit 0 ;;
        *) fail "Unknown option: $1" ;;
    esac
done

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin*)    OS_TYPE="macos" ;;
    *)          fail "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64)     ARCH_TYPE="x86_64" ;;
    aarch64)    ARCH_TYPE="aarch64" ;;
    *)          fail "Unsupported architecture: $ARCH" ;;
esac

info "Platform: $OS_TYPE / $ARCH_TYPE"
# ---------------------------------------------------------------------------
# macOS: launchd setup
# ---------------------------------------------------------------------------
setup_launchd() {
    local bin_path="$1"

    info "Configuring launchd service..."

    local real_home
    real_home=$(eval echo "~${REAL_USER}")
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
            local dummy_value="false"
            if [ "$NO_AUDIO" = "true" ]; then
                dummy_value="true"
            fi
            cat > "$config_file" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = ${dummy_value}
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
    elif [ "$NO_AUDIO" = "true" ]; then
        if grep -qE '^dummy\s*=' "$config_file" 2>/dev/null; then
            sed -i 's/^dummy\s*=.*/dummy = true/' "$config_file"
        else
            sed -i '/^\[server\]/a dummy = true' "$config_file"
        fi
    fi

    chown "$REAL_USER" "$config_file" || \
        fail "Failed to set ownership on $config_file."
    chmod 0600 "$config_file"

    local plist_dir
    plist_dir=$(eval echo "~${REAL_USER}/Library/LaunchAgents")
    mkdir -p "$plist_dir"
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"
    local log_dir
    log_dir=$(eval echo "~${REAL_USER}/Library/Logs")
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
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>${real_home}</string>
        <key>L337__SERVER__TOKEN</key>
        <string>${token}</string>
        <key>XDG_CONFIG_HOME</key>
        <string>${real_home}/.config</string>
        <key>XDG_CACHE_HOME</key>
        <string>${real_home}/.cache</string>
        <key>XDG_STATE_HOME</key>
        <string>${real_home}/.local/state</string>
    </dict>
    <key>WorkingDirectory</key>
    <string>${real_home}</string>
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

    info "Replacing binary..."
    if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
        mv "$INSTALL_DIR/l337-audio-server" "$INSTALL_DIR/l337-audio-server.bak"
    fi
    mv "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"

    launchctl load "$plist_path" || true
    if launchctl list | grep -q "$PLIST_LABEL"; then
        ok "Launchd service installed and started"
        rm -f "$INSTALL_DIR/l337-audio-server.bak"
        rm -rf "$INSTALL_DIR/.tmp"
        return 0
    fi

    warn "New binary failed to load. Rolling back..."
    if launchctl list | grep -q "$PLIST_LABEL"; then
        launchctl unload "$plist_path" 2>/dev/null || true
    fi
    if [ -f "$INSTALL_DIR/l337-audio-server.bak" ]; then
        if mv "$INSTALL_DIR/l337-audio-server.bak" "$INSTALL_DIR/l337-audio-server" 2>/dev/null; then
            chmod 0755 "$INSTALL_DIR/l337-audio-server"
            launchctl load "$plist_path" || true
            if launchctl list | grep -q "$PLIST_LABEL"; then
                warn "Rollback successful — old binary restored"
                rm -f "$INSTALL_DIR/l337-audio-server.bak"
                rm -rf "$INSTALL_DIR/.tmp"
                return 0
            fi
        fi
        fail "Rollback failed — manual recovery required"
    fi
    fail "Installation aborted: new binary failed validation"
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------
uninstall_macos() {
    info "Uninstalling from macOS..."

    local real_home
    real_home=$(eval echo "~${REAL_USER}")
    local plist_dir
    plist_dir=$(eval echo "~${REAL_USER}/Library/LaunchAgents")
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"
    local binary_path="${INSTALL_DIR}/l337-audio-server"

    if launchctl list | grep -q "$PLIST_LABEL"; then
        launchctl unload "$plist_path" 2>/dev/null || true
    fi

    # launchd only manages the agent. A foreground instance is invisible to it,
    # keeps the socket and the instance lock alive, and makes the next install
    # fail to bind — so refuse instead of half-removing.
    if pgrep -f "$binary_path" >/dev/null 2>&1; then
        local pids
        pids="$(pgrep -f "$binary_path" | tr '\n' ' ' || true)"
        fail "A manually started l337-audio-server is still running (pid: ${pids}).
Stop it and re-run:  kill ${pids}"
    fi

    rm -f "$plist_path"
    rm -rf "$INSTALL_DIR"

    # Runtime, not user data: the socket and the instance lock. The plist sets
    # XDG_CACHE_HOME=~/.cache, so the service's runtime lives there; a server
    # started by hand falls back to the macOS cache dir the same way the server
    # does (src/platform/common.rs). Only the ephemeral auth challenge token
    # shares this directory — playlists, the token file and logs do not.
    local runtime_dir
    for runtime_dir in "${real_home}/.cache/l337-audio-server" \
                       "${real_home}/Library/Caches/l337-audio-server"; do
        if [ -d "$runtime_dir" ]; then
            rm -rf "$runtime_dir"
            ok "Runtime removed: $runtime_dir"
        fi
    done

    # A download that fell back to /tmp leaves the whole binary there.
    # The binary install.sh downloads lands in TMPDIR; tidy it on uninstall.
    rm -f "${L337_BINARY_PATH:-}" "/tmp/l337-audio-server-download"

    if [ "$REMOVE_DATA" = true ]; then
        info "Removing user data directories..."
        rm -rf "${real_home}/Library/Application Support/l337-audio-server"
        rm -f "${real_home}/Library/Logs/${PLIST_LABEL}.log"
        ok "User data directories removed"
    else
        warn "User data directories retained:"
        warn "  ${real_home}/Library/Application Support/l337-audio-server"
        warn "  ${real_home}/Library/Logs/${PLIST_LABEL}.log"
        warn "Re-run with --remove-data to delete them."
    fi

    ok "Uninstallation complete"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    if [ "$UNINSTALL" = true ]; then
        info "  Remove the service and installed files"
        info "  Remove the runtime directories (socket and instance lock)"
        info "  Refuse if a manually started instance is still running"
        if [ "$REMOVE_DATA" = true ]; then
            info "  Remove user data directories"
        else
            info "  Retain user data directories"
        fi
    else
        info "  Query GitHub for latest release"
        info "  Download the latest binary for $OS_TYPE/$ARCH_TYPE"
        info "  Install to: $INSTALL_DIR/l337-audio-server"
        info "  Configure launchd plist (user-level service, no --user flag needed)"
        info "  Config: ~/Library/Application Support/l337-audio-server/server.ini"
        info "  Logs:   ~/Library/Logs/${PLIST_LABEL}.log"
    fi
    exit 0
fi

if [ "$UNINSTALL" = true ]; then
    check_command "launchctl"
    uninstall_macos
    exit 0
fi

check_dependencies

info "Installing ${BINARY_INPUT} (release ${RELEASE_TAG:-unknown})"

setup_launchd "$BINARY_INPUT"

# The binary was downloaded into TMPDIR by install.sh: drop it once it is
# safely deployed.
[ "$BINARY_INPUT" != "$INSTALL_DIR/l337-audio-server" ] && rm -f "$BINARY_INPUT"

ok "Installation complete"
echo
echo "Next steps:"
echo "  Check status:    launchctl list | grep $PLIST_LABEL"
echo "  View logs:       tail -f ~/Library/Logs/$PLIST_LABEL.log"
echo "  Configuration:   ~/Library/Application Support/l337-audio-server/server.ini"
