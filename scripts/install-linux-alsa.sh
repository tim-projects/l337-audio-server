#!/bin/bash
# install-linux-alsa.sh — Linux ALSA binary installer (system service)
#
# Downloads l337-audio-server-{arch}-linux-alsa from GitHub releases and installs
# it as a systemd system service.
#
# Every release ships a release-info.json beside the binaries recording the glibc
# each asset requires. The installer walks releases newest-first and installs the
# newest one whose release-info.json says its ALSA binary runs on this system's
# glibc. Systems older than every published release must build from source.
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
AUTO_INSTALL_DEPS=true

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Source shared library (lib-distro.sh)
# ---------------------------------------------------------------------------
# Prefers a local copy when running from a checkout; falls back to downloading
# from raw.githubusercontent.com using REPO and SCRIPT_TAG (both available when
# launched via install.sh).
SOURCE_LIB() {
    if [ -n "${LIB_DISTRO_LOADED:-}" ]; then
        return 0
    fi

    local lib=""
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

    for candidate in \
        "${script_dir}/lib-distro.sh" \
        "${SCRIPT_DIR:-${script_dir}}/scripts/lib-distro.sh" \
        "${LIB_DIR:-}/lib-distro.sh"; do
        if [ -f "$candidate" ]; then
            lib="$candidate"
            break
        fi
    done

    if [ -z "$lib" ]; then
        if [ -z "${REPO:-}" ] || [ -z "${SCRIPT_TAG:-}" ]; then
            fail "Cannot locate lib-distro.sh: no local copy found and REPO/SCRIPT_TAG are not set."
        fi
        local url="https://raw.githubusercontent.com/${REPO}/${SCRIPT_TAG}/scripts/lib-distro.sh"
        lib="${TMPDIR:-/tmp}/l337-lib-distro.$$.sh"
        info "Downloading lib-distro.sh from GitHub..."
        if ! curl -fsSL --connect-timeout 15 --max-time 30 -o "$lib" "$url"; then
            rm -f "$lib"
            fail "Failed to download lib-distro.sh from $url"
        fi
    fi

    # shellcheck disable=SC1090
    source "$lib"
    LIB_DISTRO_LOADED=true
}

check_command() {
    if command -v "$1" &>/dev/null; then
        return 0
    else
        fail "Required command not found: $1"
    fi
}

check_dependencies() {
    SOURCE_LIB
    ensure_base_tools curl file readelf ldconfig groupadd useradd
    check_command "systemctl"
}

# Runtime shared-library dependency handling relies on lib-distro.sh functions
# (pkg_for_lib, pkg_install_prefix). Those are sourced by SOURCE_LIB() above.

# Verify every DT_NEEDED library of $1 is loadable, installing the matching
# package when it is not. Never a no-op on success: it prints the verified set
# so an operator can see what was checked.
ensure_runtime_dependencies() {
    local bin="$1"
    local needed missing=() lib pkgs install_cmd

    if ! command -v readelf &>/dev/null; then
        warn "readelf not available; cannot verify runtime libraries for $bin"
        return 0
    fi
    if ! command -v ldconfig &>/dev/null; then
        warn "ldconfig not available; cannot verify runtime libraries for $bin"
        return 0
    fi

    needed=$(readelf -d "$bin" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
    if [ -z "$needed" ]; then
        warn "No DT_NEEDED entries reported for $bin; skipping library check"
        return 0
    fi

    for lib in $needed; do
        if ldconfig -p 2>/dev/null | grep -q -F "$lib"; then
            continue
        fi
        missing+=("$lib")
    done

    if [ "${#missing[@]}" -eq 0 ]; then
        ok "Runtime libraries present: $(echo $needed | tr '\n' ' ')"
        return 0
    fi

    warn "Missing runtime libraries: ${missing[*]}. The server cannot start until these are installed."

    install_cmd=$(pkg_install_prefix) || install_cmd=""

    if [ "$AUTO_INSTALL_DEPS" = false ]; then
        local n=0
        for lib in "${missing[@]}"; do
            n=$((n + 1))
            pkgs=$(pkg_for_lib "$lib") || pkgs=""
            if [ -n "$pkgs" ]; then
                printf '  %d. Install: %s (runtime dependency: %s)\n' "$n" "$pkgs" "$lib" >&2
            else
                printf '  %d. Install: %s (unknown package — search manually)\n' "$n" "$lib" >&2
            fi
        done
        fail "Missing runtime libraries. Install them manually, then re-run the installer."
    fi

    for lib in "${missing[@]}"; do
        pkgs=$(pkg_for_lib "$lib") || {
            warn "No known package provides $lib on this distribution."
            warn "Install it manually; the server will not start without it."
            continue
        }
        if [ -z "$install_cmd" ]; then
            warn "No supported package manager to install $pkgs (for $lib). Install $lib manually."
            continue
        fi
        info "Installing $pkgs (runtime dependency for $lib)..."
        # shellcheck disable=SC2086
        if $install_cmd $pkgs >/dev/null 2>&1; then
            ok "Installed $pkgs"
        else
            warn "Could not install $pkgs. Install $lib ($pkgs) manually or the server will not start."
        fi
    done

    # Final gate: fail before the service is written so the operator never gets
    # a "successful" install whose server cannot exec.
    for lib in "${missing[@]}"; do
        if ! ldconfig -p 2>/dev/null | grep -q -F "$lib"; then
            fail "Runtime library $lib is still missing. Install it manually, then re-run this installer."
        fi
    done
    return 0
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
         --no-install-deps) AUTO_INSTALL_DEPS=false; shift ;;
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
  --no-install-deps      Do not auto-install missing base tools and audio libraries
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
# Every release ships a release-info.json beside its binaries recording the glibc
# each asset was measured to require. Releases are walked newest-first and the
# first one whose release-info.json says the wanted asset runs on this system's
# glibc wins, so a newer release needing a newer glibc never shadows an older
# release that still works here. Systems older than every published release must
# build from source.
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
# The filename comes from the asset's own "name" field, never from the URL, so a
# download URL that differs from the asset name cannot confuse the match.
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
        /"name"[[:space:]]*:/ {
            n=$0; sub(/.*"name"[[:space:]]*:[[:space:]]*"/,"",n); sub(/".*/,"",n); name=n; next
        }
        /"browser_download_url"[[:space:]]*:/ {
            u=$0; sub(/.*"browser_download_url"[[:space:]]*:[[:space:]]*"/,"",u); sub(/".*/,"",u)
            printf "%s\t%d\t%s\t%s\t%s\n", tag, pre, name, pub, u
            next
        }
    '
}

# The glibc a release-info.json requires for asset $1. Prints the version, or
# "none" when the asset is listed with no glibc requirement at all. Returns 1
# when the asset is not described in it (release predates the manifest, or the
# asset name is unknown): an unverifiable binary is never installed.
release_info_glibc() {
    local info_file="$1" asset="$2" line value

    [ -f "$info_file" ] || return 1
    # The trailing colon anchors the key so a longer asset name that starts with
    # this one can never be mistaken for it.
    line=$(grep -F "\"$asset\":" "$info_file" 2>/dev/null | head -1) || true
    [ -z "$line" ] && return 1

    case "$line" in
        *null*) echo "none"; return 0 ;;
    esac

    value=$(printf '%s' "$line" \
        | sed -n 's/.*:[[:space:]]*"\{0,1\}\([0-9][0-9.]*\)"\{0,1\}.*/\1/p')
    [ -z "$value" ] && return 1
    printf '%s' "$value"
}

# Walk releases newest-first and pick the newest one whose release-info.json
# says the wanted asset runs on this system's glibc. The glibc requirement is
# read from the manifest shipped in the release, so asset filenames are plain
# and the installer depends on no GitHub-specific release metadata.
# Sets SELECTED_TAG, SELECTED_ASSET, SELECTED_URL, SELECTED_PUBLISHED,
# SELECTED_REQUIRED (glibc the chosen asset needs, or "none"); returns 1 when
# no release is runnable here.
# shortcut: only the newest 100 releases are searched (the API page size and
# this script does not follow pagination); an old-glibc system older than that
# window has to build from source.
select_compatible_release() {
    local base="$1"
    local r_tag r_pre r_name r_pub r_url r_req
    local data rows info_dir info_url info_file

    data=$(fetch_release_data)
    rows=$(printf '%s\n' "$data" | release_assets)
    info_dir=$(mktemp -d "${TMPDIR:-/tmp}/l337-revinfo.XXXXXX") || return 1

    SELECTED_TAG=""
    SELECTED_ASSET=""
    SELECTED_URL=""
    SELECTED_PUBLISHED=""
    SELECTED_REQUIRED=""

    while IFS=$'\t' read -r r_tag r_pre r_name r_pub r_url; do
        if [ "$INSTALL_PRERELEASE" = true ]; then
            if [ "$r_pre" != "1" ]; then
                continue
            fi
        fi

        if [ "$r_name" != "$base" ]; then
            continue
        fi

        info_url=$(printf '%s\n' "$rows" | awk -F'\t' \
            -v want_tag="$r_tag" -v want_name="release-info.json" \
            '$1 == want_tag && $3 == want_name { print $5; exit }')
        if [ -z "$info_url" ]; then
            continue
        fi

        # One fetch per release: the newest release almost always matches, so
        # this normally downloads a single small JSON file.
        info_file="$info_dir/$r_tag.json"
        if [ ! -s "$info_file" ]; then
            if ! curl -sS -L --connect-timeout 15 --max-time 30 \
                    -o "$info_file" "$info_url" 2>/dev/null; then
                rm -f "$info_file"
                continue
            fi
        fi

        r_req=$(release_info_glibc "$info_file" "$r_name") || continue

        if [ "$r_req" != "none" ] && [ -n "$SYSTEM_GLIBC" ]; then
            if version_lt "$SYSTEM_GLIBC" "$r_req"; then
                continue
            fi
        fi

        SELECTED_TAG="$r_tag"
        SELECTED_ASSET="$r_name"
        SELECTED_URL="$r_url"
        SELECTED_PUBLISHED="$r_pub"
        SELECTED_REQUIRED="$r_req"
        break
    done <<<"$rows"

    rm -rf "$info_dir"
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

# Asset names are plain: the glibc each binary requires is recorded in the
# release's release-info.json, not in its filename.
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
    # A download that fell back to /tmp leaves the whole binary there.
    rm -f "/tmp/$(get_asset_name "$OS_TYPE" "$ARCH_TYPE").tmp"

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
    info "System glibc: $SYSTEM_GLIBC (releases record the glibc each binary needs in release-info.json)"
else
    warn "Could not determine the system glibc version; picking the newest release with any glibc requirement."
fi

if ! select_compatible_release "$(get_asset_name "$OS_TYPE" "$ARCH_TYPE")"; then
    fail "No release of ${REPO} provides an ALSA binary for ${OS_TYPE}/${ARCH_TYPE} that runs on glibc ${SYSTEM_GLIBC:-unknown}.

The oldest published release needs a newer glibc. This system must build from source."
fi

LATEST_TAG="$SELECTED_TAG"
PUBLISHED_AT="$SELECTED_PUBLISHED"
ASSET_NAME="$SELECTED_ASSET"
DOWNLOAD_URL="$SELECTED_URL"

info "Latest compatible release: $LATEST_TAG (published: $PUBLISHED_AT)"
if [ "$SELECTED_REQUIRED" = "none" ]; then
    info "Selected asset: $ASSET_NAME (no glibc requirement)"
else
    info "Selected asset: $ASSET_NAME (needs glibc $SELECTED_REQUIRED, system has ${SYSTEM_GLIBC:-unknown})"
fi

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

# Ensure the audio backend libraries are present before checking DT_NEEDED
# entries — the binary may be statically linked against some libs and thus
# not list them, yet still need them at runtime.
ensure_audio_libs alsa

# A missing DT_NEEDED library is resolved by the loader before main(), so the
# binary cannot exec in ANY mode (dummy = true included). Check the binary we
# just downloaded and install the runtime packages before first boot.
ensure_runtime_dependencies "$NEW_BIN"

setup_systemd "$NEW_BIN"

rm -f "$NEW_BIN"
rm -rf "$INSTALL_DIR/.tmp"
ok "Installation complete"
echo
echo "Next steps:"
echo "  Check status:    systemctl status l337-audio-server.service"
echo "  View logs:       journalctl -u l337-audio-server.service -f"
echo "  Configuration:   $CONFIG_DIR/server.ini"
