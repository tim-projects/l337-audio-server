#!/bin/bash
# install-linux-pipewire.sh — Linux PipeWire binary installer (per-user services)
#
# Downloads l337-audio-server-{arch}-linux-pipewire from GitHub releases and
# installs it as a systemd user service for every user with an active
# PipeWire session.
#
# Usage:
#   sudo ./scripts/install-linux-pipewire.sh
#   sudo ./scripts/install-linux-pipewire.sh --pre-release
#   sudo ./scripts/install-linux-pipewire.sh --user <username>
#   sudo ./scripts/install-linux-pipewire.sh --uninstall
#   sudo ./scripts/install-linux-pipewire.sh --uninstall --remove-data
#   sudo ./scripts/install-linux-pipewire.sh --dry-run
#   sudo ./scripts/install-linux-pipewire.sh --force
#   sudo ./scripts/install-linux-pipewire.sh --no-audio
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO="tim-projects/l337-audio-server"
INSTALL_DIR="/opt/l337-audio-server"
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
NO_AUDIO=false
TARGET_USER=""

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
    check_command "systemctl"
    check_command "file"
    check_command "groupadd"
    check_command "useradd"
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
        --user) TARGET_USER="$2"; shift 2 ;;
        --no-audio) NO_AUDIO=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [OPTIONS]

Linux PipeWire installer for L337 Audio Server.
Downloads prebuilt PipeWire binary from GitHub releases and installs as
systemd user services for every user with an active PipeWire session.

Options:
  --dry-run              Show what would happen without making changes
  --pre-release, --prerelease
                         Install the latest prerelease instead of stable release
  --force, -f            Force reinstall even if the same version is already installed
  --user <username>      Install for a specific user only (default: all PipeWire users)
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
    Linux*)     OS_TYPE="linux" ;;
    *)          fail "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH_TYPE="x86_64" ;;
    aarch64|arm64) ARCH_TYPE="aarch64" ;;
    *)             fail "Unsupported architecture: $ARCH" ;;
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
        linux-x86_64)
            echo "l337-audio-server-x86_64-linux-pipewire"
            ;;
        linux-aarch64)
            fail "aarch64 Linux binaries are not yet available in GitHub releases. Please build from source."
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
# User detection
# ---------------------------------------------------------------------------
get_all_login_users() {
    while IFS=: read -r username _ uid _ _ home _; do
        if [ "$uid" -ge 1000 ] 2>/dev/null && [ -n "$home" ]; then
            echo "$username"
        fi
    done < /etc/passwd
}

user_has_pipewire() {
    local user="$1"
    local user_uid
    user_uid=$(id -u "$user" 2>/dev/null || echo "")

    if [ -z "$user_uid" ]; then
        return 1
    fi

    local runtime_dir="/run/user/$user_uid"
    if [ ! -d "$runtime_dir" ]; then
        return 1
    fi

    if [ -S "$runtime_dir/pipewire-0" ] || [ -S "$runtime_dir/pulse/native" ]; then
        return 0
    fi

    if su - "$user" -c "systemctl --user is-active --quiet pipewire 2>/dev/null" 2>/dev/null; then
        return 0
    fi

    if su - "$user" -c 'command -v pactl >/dev/null 2>&1 && pactl info >/dev/null 2>&1' 2>/dev/null; then
        return 0
    fi

    return 1
}

# ---------------------------------------------------------------------------
# Linux: systemd user service setup (parameterized)
# ---------------------------------------------------------------------------
setup_systemd_user() {
    local bin_path="$1"
    local real_user="$2"

    info "Configuring systemd user service for $real_user..."

    local real_home
    real_home=$(eval echo "~${real_user}")
    local user_config_dir="$real_home/.config/l337-audio-server"
    local user_cache_dir="$real_home/.cache/l337-audio-server"
    local user_state_dir="$real_home/.local/state/l337-audio-server"
    local user_runtime_dir="$real_home/.local/run/l337-audio-server"
    local user_service_dir="$real_home/.config/systemd/user"
    local user_service_file="$user_service_dir/l337-audio-server.service"

    mkdir -p "$INSTALL_DIR" "$user_config_dir" "$user_cache_dir" "$user_state_dir" "$user_runtime_dir" "$user_service_dir"

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
            local dummy_value="false"
            if [ "$NO_AUDIO" = "true" ]; then
                dummy_value="true"
            fi
            cat > "$config_file" <<EOF
[server]
host = "127.0.0.1"
port = 1337
token = "${token}"
dummy = ${dummy_value}
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
    elif [ "$NO_AUDIO" = "true" ]; then
        if grep -qE '^dummy\s*=' "$config_file" 2>/dev/null; then
            sed -i 's/^dummy\s*=.*/dummy = true/' "$config_file"
        else
            sed -i '/^\[server\]/a dummy = true' "$config_file"
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

    info "Stopping user service..."
    su - "$real_user" -c "systemctl --user stop l337-audio-server.service" 2>/dev/null || true
    if su - "$real_user" -c "systemctl --user is-active --quiet l337-audio-server.service" 2>/dev/null; then
        fail "Service l337-audio-server.service failed to stop"
    fi

    info "Replacing binary..."
    if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
        mv "$INSTALL_DIR/l337-audio-server" "$INSTALL_DIR/l337-audio-server.bak"
    fi
    mv "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"

    su - "$real_user" -c "systemctl --user daemon-reload" || true
    su - "$real_user" -c "systemctl --user enable l337-audio-server.service" || true
    su - "$real_user" -c "systemctl --user start l337-audio-server.service" || true

    sleep 1
    if su - "$real_user" -c "systemctl --user is-active --quiet l337-audio-server.service" 2>/dev/null; then
        ok "Systemd user service installed and started for $real_user"
        rm -f "$INSTALL_DIR/l337-audio-server.bak"
        rm -rf "$INSTALL_DIR/.tmp"
        return 0
    fi

    su - "$real_user" -c "journalctl --user -u l337-audio-server.service --since '1 minute ago' --no-pager" 2>/dev/null || true
    warn "New binary failed to start for $real_user. Rolling back..."
    su - "$real_user" -c "systemctl --user stop l337-audio-server.service" 2>/dev/null || true
    if [ -f "$INSTALL_DIR/l337-audio-server.bak" ]; then
        if mv "$INSTALL_DIR/l337-audio-server.bak" "$INSTALL_DIR/l337-audio-server" 2>/dev/null; then
            chmod 0755 "$INSTALL_DIR/l337-audio-server"
            su - "$real_user" -c "systemctl --user daemon-reload" || true
            su - "$real_user" -c "systemctl --user start l337-audio-server.service" 2>/dev/null || true
            sleep 1
            if su - "$real_user" -c "systemctl --user is-active --quiet l337-audio-server.service" 2>/dev/null; then
                warn "Rollback successful — old binary restored and running for $real_user"
                rm -f "$INSTALL_DIR/l337-audio-server.bak"
                rm -rf "$INSTALL_DIR/.tmp"
                return 0
            fi
        fi
        fail "Rollback failed — manual recovery required"
    fi
    fail "Installation aborted: new binary failed validation for $real_user"
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
# Uninstall (multi-user)
# ---------------------------------------------------------------------------
uninstall_user_service() {
    info "Uninstalling systemd user services..."

    local any_installed=false
    while IFS= read -r user; do
        local real_home
        real_home=$(eval echo "~${user}")
        local user_service_dir="$real_home/.config/systemd/user"
        local user_service_file="$user_service_dir/l337-audio-server.service"

        if [ ! -f "$user_service_file" ]; then
            continue
        fi

        any_installed=true
        info "Removing user service for $user..."
        su - "$user" -c "systemctl --user stop l337-audio-server.service" 2>/dev/null || true
        su - "$user" -c "systemctl --user disable l337-audio-server.service" 2>/dev/null || true

        rm -f "$user_service_file"
        su - "$user" -c "systemctl --user daemon-reload" 2>/dev/null || true

        if [ "$REMOVE_DATA" = true ]; then
            info "Removing user data for $user..."
            rm -rf "$real_home/.config/l337-audio-server" \
                  "$real_home/.cache/l337-audio-server" \
                  "$real_home/.local/state/l337-audio-server" \
                  "$real_home/.local/run/l337-audio-server"
            ok "Data removed for $user"
        else
            warn "User data retained for $user:"
            warn "  $real_home/.config/l337-audio-server"
        fi
    done < <(get_all_login_users)

    if [ "$any_installed" = false ]; then
        warn "No user services found"
    fi

    if [ "$REMOVE_DATA" = true ]; then
        rm -rf "$INSTALL_DIR"
        ok "Shared binary removed"
    fi

    ok "User service uninstallation complete"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    if [ "$UNINSTALL" = true ]; then
        info "  Scan all users for installed l337-audio-server.service files"
        info "  For each user: stop, disable, remove service + config"
        if [ "$REMOVE_DATA" = true ]; then
            info "  Remove shared binary if no users remain"
        fi
    else
        info "  Query GitHub for latest release"
        info "  Download the latest PipeWire binary for $OS_TYPE/$ARCH_TYPE"
        info "  Install to: $INSTALL_DIR/l337-audio-server"
        if [ -n "$TARGET_USER" ]; then
            info "  Install systemd user service for: $TARGET_USER"
            info "  Config: ~/.config/l337-audio-server/server.ini"
        else
            info "  Auto-install systemd user service for all PipeWire users"
            info "  Config per user: ~/.config/l337-audio-server/server.ini"
        fi
    fi
    exit 0
fi

if [ "$UNINSTALL" = true ]; then
    require_root
    check_command "systemctl"
    uninstall_user_service
    exit 0
fi

require_root
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
                echo "  sudo $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
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
NEW_BIN=""
if mkdir -p "$INSTALL_DIR/.tmp" 2>/dev/null && [ -w "$INSTALL_DIR/.tmp" ]; then
    NEW_BIN="$INSTALL_DIR/.tmp/l337-audio-server.new"
else
    warn "Cannot write to $INSTALL_DIR/.tmp; falling back to /tmp (non-atomic copy)"
    NEW_BIN="/tmp/${ASSET_NAME}.tmp"
fi

download_binary "$DOWNLOAD_URL" "$NEW_BIN"

if [ -n "$TARGET_USER" ]; then
    if ! id "$TARGET_USER" &>/dev/null; then
        fail "User not found: $TARGET_USER"
    fi
    if ! user_has_pipewire "$TARGET_USER"; then
        fail "User $TARGET_USER does not have an active PipeWire session"
    fi
    setup_systemd_user "$NEW_BIN" "$TARGET_USER"
else
    info "Scanning for PipeWire users..."
    pw_users=()
    while IFS= read -r user; do
        if user_has_pipewire "$user"; then
            pw_users+=("$user")
        fi
    done < <(get_all_login_users)

    if [ "${#pw_users[@]}" -eq 0 ]; then
        fail "No users with active PipeWire sessions found. Start PipeWire or use install-linux-alsa.sh for ALSA."
    fi

    info "Found ${#pw_users[@]} PipeWire user(s): ${pw_users[*]}"
    for user in "${pw_users[@]}"; do
        setup_systemd_user "$NEW_BIN" "$user"
    done
fi

rm -f "$NEW_BIN"
rm -rf "$INSTALL_DIR/.tmp"
ok "Installation complete"
echo
if [ -n "$TARGET_USER" ]; then
    echo "Next steps for $TARGET_USER:"
    echo "  Check status:    su - $TARGET_USER -c 'systemctl --user status l337-audio-server.service'"
    echo "  View logs:       su - $TARGET_USER -c 'journalctl --user -u l337-audio-server -f'"
    echo "  Configuration:   ~/.config/l337-audio-server/server.ini"
else
    echo "Next steps:"
    echo "  Check status:    systemctl --user status l337-audio-server.service"
    echo "  View logs:       journalctl --user -u l337-audio-server -f"
    echo "  Configuration:   ~/.config/l337-audio-server/server.ini"
fi
