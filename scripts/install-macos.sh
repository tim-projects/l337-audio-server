#!/bin/bash
# install-macos.sh — macOS launchd installer
#
# Downloads l337-audio-server-{arch}-apple-darwin from GitHub releases and
# installs it as a launchd user service.
#
# Usage:
#   ./scripts/install-macos.sh
#   ./scripts/install-macos.sh --pre-release
#   ./scripts/install-macos.sh --uninstall
#   ./scripts/install-macos.sh --uninstall --remove-data
#   ./scripts/install-macos.sh --dry-run
#   ./scripts/install-macos.sh --force
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
INSTALL_PRERELEASE=false
FORCE=false
NO_AUDIO=false

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
        --dry-run) DRY_RUN=true; shift ;;
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        --force|-f) FORCE=true; shift ;;
        --no-audio) NO_AUDIO=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [OPTIONS]

macOS installer for L337 Audio Server.
Downloads prebuilt binary from GitHub releases and installs as launchd user service.

Options:
  --dry-run              Show what would happen without making changes
  --pre-release, --prerelease
                         Install the latest prerelease instead of stable release
  --force, -f            Force reinstall even if the same version is already installed
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
    case "$os-$arch" in
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
# Uninstall
# ---------------------------------------------------------------------------
uninstall_macos() {
    info "Uninstalling from macOS..."

    local real_home
    real_home=$(eval echo "~${REAL_USER}")
    local plist_dir
    plist_dir=$(eval echo "~${REAL_USER}/Library/LaunchAgents")
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"

    if launchctl list | grep -q "$PLIST_LABEL"; then
        launchctl unload "$plist_path" 2>/dev/null || true
    fi

    rm -f "$plist_path"
    rm -rf "$INSTALL_DIR"

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
                echo "  $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
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
                echo "  $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
                exit 0
            fi
        fi
    else
        warn "Could not determine installed version; will reinstall"
    fi
fi

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"
NEW_BIN=""
if mkdir -p "$INSTALL_DIR/.tmp" 2>/dev/null && [ -w "$INSTALL_DIR/.tmp" ]; then
    NEW_BIN="$INSTALL_DIR/.tmp/l337-audio-server.new"
else
    warn "Cannot write to $INSTALL_DIR/.tmp; falling back to /tmp (non-atomic copy)"
    NEW_BIN="/tmp/${ASSET_NAME}.tmp"
fi

download_binary "$DOWNLOAD_URL" "$NEW_BIN"

setup_launchd "$NEW_BIN"

rm -f "$NEW_BIN"
rm -rf "$INSTALL_DIR/.tmp"
ok "Installation complete"
echo
echo "Next steps:"
echo "  Check status:    launchctl list | grep $PLIST_LABEL"
echo "  View logs:       tail -f ~/Library/Logs/$PLIST_LABEL.log"
echo "  Configuration:   ~/Library/Application Support/l337-audio-server/server.ini"
