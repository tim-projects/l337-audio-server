#!/bin/bash
# install-linux-alsa.sh — Linux ALSA binary installer (system service)
#
# Downloads l337-audio-server-{arch}-linux-alsa-glibc{X.Y} from GitHub releases
# and installs it as a systemd system service. X.Y is the glibc the binary was
# built on; the newest release whose tier this system's glibc can run wins.
#
# Usage:
#   sudo ./scripts/install-linux-alsa.sh
#   sudo ./scripts/install-linux-alsa.sh --pre-release
#   sudo ./scripts/install-linux-alsa.sh --uninstall
#   sudo ./scripts/install-linux-alsa.sh --uninstall --remove-data
#   sudo ./scripts/install-linux-alsa.sh --dry-run
#   sudo ./scripts/install-linux-alsa.sh --force
#   sudo ./scripts/install-linux-alsa.sh --no-audio
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO="tim-projects/l337-audio-server"
INSTALL_DIR="/opt/l337-audio-server"
PREFIX_INSTALL_DIR=""
CONFIG_DIR="/etc/l337-audio-server"
STATE_DIR="/var/lib/l337-audio-server"
CACHE_DIR="/var/cache/l337-audio-server"
SYSTEMD_SERVICE="/etc/systemd/system/l337-audio-server.service"
USER_NAME="l337"
GROUP_NAME="l337"
CREATED_USER=false

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
TARGET_GROUP=""
NO_AUDIO_GROUP=false
GROUP_ACCESS=false
PREFIX_INSTALL_DIR=""

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
         --group) TARGET_GROUP="$2"; shift 2 ;;
         --group-access) GROUP_ACCESS=true; shift ;;
         --no-audio-group) NO_AUDIO_GROUP=true; shift ;;
         --prefix) PREFIX_INSTALL_DIR="$2"; shift 2 ;;
         --no-audio) NO_AUDIO=true; shift ;;
         --uninstall|-u) UNINSTALL=true; shift ;;
         --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [OPTIONS]

Linux ALSA installer for L337 Audio Server.
Downloads prebuilt ALSA binary from GitHub releases and installs as systemd service.

Options:
  --dry-run              Show what would happen without making changes
  --pre-release, --prerelease
                         Install the latest prerelease instead of stable release
  --force, -f            Force reinstall even if the same version is already installed
  --user <username>      Run service as this user (default: create l337 system user)
  --group <groupname>    Run service under this group (default: l337)
  --group-access         Add local users to the service group for Unix socket access
  --no-audio-group       Do not add service user to the audio group
  --prefix <path>        Install base path (default: /opt/l337-audio-server)
  --no-audio             Set dummy = true in server.ini (run without audio hardware)
  --uninstall, -u        Remove the service and installed files
  --remove-data          Also remove configuration and data directories
  -h, --help             Show this help message
EOF
            exit 0 ;;
        *) fail "Unknown option: $1" ;;
    esac
done

INSTALL_DIR="${PREFIX_INSTALL_DIR:-/opt/l337-audio-server}"

detect_nologin_shell() {
    for shell in /usr/sbin/nologin /sbin/nologin /bin/false; do
        if [ -f "$shell" ]; then
            echo "$shell"
            return
        fi
    done
    echo "/bin/false"
}

if [ "$(id -u)" -ne 0 ]; then
    USER_NAME="${TARGET_USER:-$(id -un)}"
    GROUP_NAME="${TARGET_GROUP:-$(id -gn)}"
else
    if [ -n "$TARGET_USER" ]; then
        USER_NAME="$TARGET_USER"
        GROUP_NAME="${TARGET_GROUP:-$TARGET_USER}"
    else
        USER_NAME="${USER_NAME:-l337}"
        GROUP_NAME="${GROUP_NAME:-l337}"
    fi
fi

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
    armv7l|armhf)  ARCH_TYPE="armv7" ;;
    riscv64)       ARCH_TYPE="riscv64" ;;
    *)             fail "Unsupported architecture: $ARCH" ;;
esac

info "Platform: $OS_TYPE / $ARCH_TYPE"

# ---------------------------------------------------------------------------
# GitHub release helpers
# ---------------------------------------------------------------------------
# Released Linux assets are named
#   l337-audio-server-<arch>-linux-<variant>-glibc<X.Y>
# where X.Y is the glibc the binary was built on (the oldest glibc it can run
# on). Releases are walked newest-first and the first one shipping a tier this
# system's glibc can run wins, so a newer release that needs a newer glibc never
# shadows an older release that still works here. Systems older than every
# published tier must build from source.
fetch_release_data() {
    local endpoint
    local url
    local response_file
    local http_code
    local message

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

    cat "$response_file"
    rm -f "$response_file"
}

# Flatten release JSON into one line per published asset:
#   <tag> <prerelease:0|1> <asset filename> <published_at> <download url>
# Rows come out newest release first, exactly as the GitHub API orders them.
release_assets() {
    awk '
        /"tag_name"[[:space:]]*:/ {
            t=$0; sub(/.*"tag_name"[[:space:]]*:[[:space:]]*"/,"",t); sub(/".*/,"",t); tag=t; next
        }
        /"prerelease"[[:space:]]*:/ { pre=($0 ~ /: *true/) ? 1 : 0; next }
        /"published_at"[[:space:]]*:/ {
            p=$0; sub(/.*"published_at"[[:space:]]*:[[:space:]]*"/,"",p); sub(/".*/,"",p); pub=p; next
        }
        /"browser_download_url"[[:space:]]*:/ {
            u=$0; sub(/.*"browser_download_url"[[:space:]]*:[[:space:]]*"/,"",u); sub(/".*/,"",u)
            n=split(u, part, "/")
            printf "%s\t%d\t%s\t%s\t%s\n", tag, pre, part[n], pub, u
            next
        }
    '
}

# Pick the newest release that ships a glibc tier this system can run, and the
# highest such tier inside it (the lowest tier when the system glibc is unknown
# so an unreadable libc still gets the most compatible binary).
# Sets SELECTED_TAG, SELECTED_ASSET, SELECTED_URL, SELECTED_TIER,
# SELECTED_PUBLISHED; returns 1 when no compatible asset exists.
# shortcut: only the newest 100 releases are searched (the API page size and
# this script does not follow pagination); an old-glibc system older than that
# window has to build from source.
select_compatible_release() {
    local base="$1"
    local r_tag r_pre r_name r_pub r_url r_tier data

    SELECTED_TAG=""
    SELECTED_ASSET=""
    SELECTED_URL=""
    SELECTED_TIER=""
    SELECTED_PUBLISHED=""

    data=$(fetch_release_data)

    while IFS=$'\t' read -r r_tag r_pre r_name r_pub r_url; do
        if [ "$INSTALL_PRERELEASE" = true ]; then
            if [ "$r_pre" != "1" ]; then
                continue
            fi
        fi

        case "$r_name" in
            "$base"-glibc*) ;;
            *) continue ;;
        esac

        r_tier="${r_name##*-glibc}"

        if [ -n "$SYSTEM_GLIBC" ]; then
            if version_lt "$SYSTEM_GLIBC" "$r_tier"; then
                continue
            fi
        fi

        if [ -z "$SELECTED_TAG" ]; then
            SELECTED_TAG="$r_tag"
            SELECTED_ASSET="$r_name"
            SELECTED_URL="$r_url"
            SELECTED_TIER="$r_tier"
            SELECTED_PUBLISHED="$r_pub"
            continue
        fi

        # Only compare tiers inside the release that already matched.
        if [ "$r_tag" != "$SELECTED_TAG" ]; then
            continue
        fi

        if [ -n "$SYSTEM_GLIBC" ]; then
            if version_lt "$SELECTED_TIER" "$r_tier"; then
                SELECTED_ASSET="$r_name"
                SELECTED_URL="$r_url"
                SELECTED_TIER="$r_tier"
            fi
        else
            if version_lt "$r_tier" "$SELECTED_TIER"; then
                SELECTED_ASSET="$r_name"
                SELECTED_URL="$r_url"
                SELECTED_TIER="$r_tier"
            fi
        fi
    done <<<"$(printf '%s\n' "$data" | release_assets)"

    [ -n "$SELECTED_TAG" ]
}

# Highest glibc symbol version the running libc exports, i.e. the oldest glibc
# a binary may have been built on to still run here. Empty when undetectable.
system_glibc_version() {
    local v=""

    v=$(ldd --version 2>/dev/null | head -1 \
        | sed -n 's/.*[[:space:]]\([0-9][0-9]*\.[0-9][0-9.]*\)[[:space:]]*$/\1/p')
    if [ -z "$v" ]; then
        v=$(getconf GNU_LIBC_VERSION 2>/dev/null \
            | sed -n 's/.*glibc[[:space:]]*\([0-9][0-9]*\.[0-9][0-9.]*\).*/\1/p')
    fi

    printf '%s' "$v"
}

# version_lt: true when $1 sorts before $2. Equal versions are not "less than".
version_lt() {
    if [ "$1" = "$2" ]; then
        return 1
    fi
    [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ]
}

parse_json_field() {
    local json="$1"
    local field="$2"
    echo "$json" | grep -o "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -1 | sed 's/.*"\([^"]*\)"$/\1/' || true
}

# Tier-less asset base name. The glibc tier is appended by the publisher as
# -glibc<X.Y>, and select_compatible_release matches it against the system.
get_asset_name() {
    local os="$1"
    local arch="$2"
    case "$os-$arch" in
        linux-x86_64)
            echo "l337-audio-server-x86_64-linux-alsa"
            ;;
        linux-aarch64)
            echo "l337-audio-server-aarch64-linux-alsa"
            ;;
        linux-armv7)
            echo "l337-audio-server-armv7-linux-alsa"
            ;;
        linux-riscv64)
            echo "l337-audio-server-riscv64-linux-alsa"
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
# Service lifecycle helpers
# ---------------------------------------------------------------------------
stop_service_systemd() {
    local service="$1"
    if systemctl is-active --quiet "$service" 2>/dev/null; then
        info "Stopping $service..."
        systemctl stop "$service" 2>/dev/null || true
    fi
    if systemctl is-active --quiet "$service" 2>/dev/null; then
        fail "Service $service failed to stop"
    fi
}

# ---------------------------------------------------------------------------
# Linux: systemd setup
# ---------------------------------------------------------------------------
setup_systemd() {
    local bin_path="$1"

    info "Configuring systemd service..."

    if [ "$(id -u)" -ne 0 ]; then
        USER_NAME="${TARGET_USER:-$(id -un)}"
        GROUP_NAME="${TARGET_GROUP:-$(id -gn)}"
        info "Non-root install: using existing user $USER_NAME / group $GROUP_NAME"
    else
        if [ -n "$TARGET_USER" ]; then
            USER_NAME="$TARGET_USER"
            GROUP_NAME="${TARGET_GROUP:-$USER_NAME}"
            info "Using specified user $USER_NAME / group $GROUP_NAME"
        else
            USER_NAME="${USER_NAME:-l337}"
            GROUP_NAME="${GROUP_NAME:-l337}"
            if ! id -g "$GROUP_NAME" &>/dev/null; then
                info "Creating system group: $GROUP_NAME"
                groupadd --system "$GROUP_NAME" || \
                    fail "Failed to create system group $GROUP_NAME. Run the installer with sudo and ensure groupadd is available."
            fi
            if ! id "$USER_NAME" &>/dev/null; then
                info "Creating system user: $USER_NAME"
                local nologin_shell
                nologin_shell=$(detect_nologin_shell)
                useradd --system --no-create-home --shell "$nologin_shell" --gid "$GROUP_NAME" "$USER_NAME" || \
                    fail "Failed to create system user $USER_NAME. Run the installer with sudo and ensure useradd is available."
                CREATED_USER=true
            fi
            if [ "$NO_AUDIO_GROUP" = false ]; then
                if ! groups "$USER_NAME" 2>/dev/null | grep -qw "audio"; then
                    info "Adding $USER_NAME to audio group for ALSA access"
                    usermod -aG audio "$USER_NAME" || \
                        warn "Failed to add $USER_NAME to audio group; ALSA access may not be available."
                fi
            else
                info "Skipping audio group membership (--no-audio-group)"
            fi
        fi
    fi

    if [ "$GROUP_ACCESS" = true ]; then
        local client_users=()
        [ -n "${SUDO_USER:-}" ] && client_users+=("$SUDO_USER")
        [ -n "$TARGET_USER" ] && client_users+=("$TARGET_USER")
        for u in "${client_users[@]}"; do
            if id "$u" &>/dev/null; then
                if ! id -nG "$u" 2>/dev/null | tr ' ' '\n' | grep -qx "$GROUP_NAME"; then
                    info "Adding $u to group $GROUP_NAME (Unix socket access)"
                    usermod -aG "$GROUP_NAME" "$u" || \
                        warn "Failed to add $u to $GROUP_NAME; they will not be able to reach the socket."
                fi
            fi
        done
    fi

    mkdir -p "$INSTALL_DIR" "$STATE_DIR" "$CACHE_DIR" "$CONFIG_DIR"
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
            local dummy_value="false"
            if [ "$NO_AUDIO" = "true" ]; then
                dummy_value="true"
            fi
            local socket_mode_value="600"
            if [ "$GROUP_ACCESS" = true ]; then
                socket_mode_value="660"
            fi
            cat > "$config_file" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = ${dummy_value}
transport = "auto"
socket_mode = "${socket_mode_value}"
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

    chown "$USER_NAME:$GROUP_NAME" "$config_file" 2>/dev/null || \
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
RuntimeDirectoryMode=0750

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

    stop_service_systemd "l337-audio-server.service"

    info "Replacing binary..."
    if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
        mv "$INSTALL_DIR/l337-audio-server" "$INSTALL_DIR/l337-audio-server.bak"
    fi
    mv "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"
    chown "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR/l337-audio-server"

    systemctl daemon-reload
    systemctl enable l337-audio-server.service
    systemctl start l337-audio-server.service || true

    sleep 1
    if systemctl is-active --quiet l337-audio-server.service 2>/dev/null; then
        ok "Systemd service installed and started"
        rm -f "$INSTALL_DIR/l337-audio-server.bak"
        rm -rf "$INSTALL_DIR/.tmp"
        systemctl status l337-audio-server.service --no-pager || true
        if [ "$GROUP_ACCESS" = true ]; then
            echo
            echo "========================================="
            echo " Unix Socket Access"
            echo "========================================="
            echo
            echo "  Socket: /run/l337-audio-server/l337.sock"
            echo "  Group:  $GROUP_NAME (mode 0660)"
            echo
            echo "  Token auth is enforced on the socket."
            echo "  Log out and back in (or run \`newgrp $GROUP_NAME\`)"
            echo "  before the client can connect."
            echo "========================================="
            echo
        fi
        return 0
    fi

    journalctl -u l337-audio-server.service --since "1 minute ago" --no-pager || true
    warn "New binary failed to start. Rolling back..."
    systemctl stop l337-audio-server.service 2>/dev/null || true
    if [ -f "$INSTALL_DIR/l337-audio-server.bak" ]; then
        if mv "$INSTALL_DIR/l337-audio-server.bak" "$INSTALL_DIR/l337-audio-server" 2>/dev/null; then
            chmod 0755 "$INSTALL_DIR/l337-audio-server"
            chown "$USER_NAME:$GROUP_NAME" "$INSTALL_DIR/l337-audio-server"
            systemctl daemon-reload
            systemctl start l337-audio-server.service 2>/dev/null || true
            sleep 1
            if systemctl is-active --quiet l337-audio-server.service 2>/dev/null; then
                warn "Rollback successful — old binary restored and running"
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
uninstall_linux() {
    info "Uninstalling from Linux..."

    local binary_path="${INSTALL_DIR}/l337-audio-server"

    if systemctl is-active --quiet l337-audio-server.service 2>/dev/null; then
        systemctl stop l337-audio-server.service || true
    fi
    if systemctl is-enabled --quiet l337-audio-server.service 2>/dev/null; then
        systemctl disable l337-audio-server.service || true
    fi

    # systemd only manages the service instance. A foreground instance is not
    # stopped by the unit, keeps the socket and the instance lock alive, and
    # makes the next install fail to bind — so refuse instead of half-removing.
    if pgrep -f "$binary_path" >/dev/null 2>&1; then
        local pids
        pids="$(pgrep -f "$binary_path" | tr '\n' ' ' || true)"
        fail "A manually started l337-audio-server is still running (pid: ${pids}).
Stop it and re-run:  kill ${pids}"
    fi

    rm -f "$SYSTEMD_SERVICE"
    systemctl daemon-reload 2>/dev/null || true

    rm -rf "$INSTALL_DIR"

    # Runtime artifacts (socket + instance.lock), not user data: the unit's
    # RuntimeDirectory, and a run that had XDG_RUNTIME_DIR of its own.
    local runtime
    for runtime in "/run/l337-audio-server" /run/user/*/l337-audio-server; do
        [ -d "$runtime" ] || continue
        rm -rf "$runtime"
        ok "Runtime removed: $runtime"
    done

    # A download that fell back to /tmp leaves the whole binary there.
    # A download that fell back to /tmp leaves the whole binary there. The
    # temp name carries the glibc tier, and pre-tier releases had no suffix.
    rm -f "/tmp/$(get_asset_name "$OS_TYPE" "$ARCH_TYPE").tmp" \
          "/tmp/$(get_asset_name "$OS_TYPE" "$ARCH_TYPE")-glibc"*.tmp

    if [ "$REMOVE_DATA" = true ]; then
        info "Removing data directories..."
        rm -rf "$CONFIG_DIR" "$STATE_DIR" "$CACHE_DIR"
        if [ "$CREATED_USER" = true ]; then
            if id "$USER_NAME" &>/dev/null; then
                userdel "$USER_NAME" 2>/dev/null || true
            fi
            if getent group "$GROUP_NAME" &>/dev/null; then
                groupdel "$GROUP_NAME" 2>/dev/null || true
            fi
        else
            info "Skipping user/group removal (not created by installer)"
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

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    if [ "$UNINSTALL" = true ]; then
        info "  Remove the service and installed files"
        info "  Remove the runtime directories (/run/l337-audio-server, /run/user/*/l337-audio-server)"
        info "  Refuse if a manually started instance is still running"
        if [ "$REMOVE_DATA" = true ]; then
            info "  Remove configuration and data directories"
        else
            info "  Retain configuration and data directories"
        fi
    else
        info "  Query GitHub for latest release"
        info "  Download the latest ALSA binary for $OS_TYPE/$ARCH_TYPE"
        info "  Install to: $INSTALL_DIR/l337-audio-server"
        info "  Configure systemd service: $SYSTEMD_SERVICE"
        if [ "$(id -u)" -ne 0 ] || [ -n "$TARGET_USER" ]; then
            info "  Run as existing user: $USER_NAME / group: $GROUP_NAME"
        else
            info "  Create user/group: $USER_NAME/$GROUP_NAME"
            if [ "$NO_AUDIO_GROUP" = true ]; then
                info "  Skip audio group membership (--no-audio-group)"
            else
                info "  Add $USER_NAME to audio group"
            fi
        fi
        info "  Create directories: $INSTALL_DIR, $STATE_DIR, $CACHE_DIR, $CONFIG_DIR"
    fi
    exit 0
fi

if [ "$UNINSTALL" = true ]; then
    require_root
    check_command "systemctl"
    uninstall_linux
    exit 0
fi

require_root
check_dependencies

info "Checking for latest release..."

SYSTEM_GLIBC=$(system_glibc_version)
if [ -n "$SYSTEM_GLIBC" ]; then
    info "System glibc: $SYSTEM_GLIBC (assets are tagged -glibc<X.Y> by the glibc they were built on)"
else
    warn "Could not determine the system glibc version; using the most compatible asset tier."
fi

if ! select_compatible_release "$(get_asset_name "$OS_TYPE" "$ARCH_TYPE")"; then
    fail "No release of ${REPO} provides an ALSA binary for ${OS_TYPE}/${ARCH_TYPE} on glibc ${SYSTEM_GLIBC:-unknown}.

The oldest published tier needs a newer glibc. This system must build from source."
fi

LATEST_TAG="$SELECTED_TAG"
PUBLISHED_AT="$SELECTED_PUBLISHED"
ASSET_NAME="$SELECTED_ASSET"
DOWNLOAD_URL="$SELECTED_URL"

info "Latest compatible release: $LATEST_TAG (published: $PUBLISHED_AT)"
info "Selected asset: $ASSET_NAME (glibc $SELECTED_TIER)"

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
                echo "  sudo ./install.sh --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
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
                echo "  sudo ./install.sh --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
                exit 0
            fi
        fi
    else
        warn "Could not determine installed version; will reinstall"
    fi
fi

# DOWNLOAD_URL and ASSET_NAME were resolved above by select_compatible_release.
NEW_BIN=""
if mkdir -p "$INSTALL_DIR/.tmp" 2>/dev/null && [ -w "$INSTALL_DIR/.tmp" ]; then
    NEW_BIN="$INSTALL_DIR/.tmp/l337-audio-server.new"
else
    warn "Cannot write to $INSTALL_DIR/.tmp; falling back to /tmp (non-atomic copy)"
    NEW_BIN="/tmp/${ASSET_NAME}.tmp"
fi

download_binary "$DOWNLOAD_URL" "$NEW_BIN"

setup_systemd "$NEW_BIN"

rm -f "$NEW_BIN"
rm -rf "$INSTALL_DIR/.tmp"
ok "Installation complete"
echo
echo "Next steps:"
echo "  Check status:    systemctl status l337-audio-server.service"
echo "  View logs:       journalctl -u l337-audio-server.service -f"
echo "  Configuration:   $CONFIG_DIR/server.ini"
