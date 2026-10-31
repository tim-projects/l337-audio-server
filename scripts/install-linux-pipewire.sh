#!/bin/bash
# install-linux-pipewire.sh — Linux PipeWire binary installer (per-user services)
#
# Downloads l337-audio-server-{arch}-linux-pipewire from GitHub releases and
# installs it as a systemd user service for every user with an active PipeWire
# session.
#
# Every release ships a release-info.json beside the binaries recording the glibc
# each asset requires. The installer walks releases newest-first and installs the
# newest one whose release-info.json says its PipeWire binary runs on this
# system's glibc.
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
PREFIX_INSTALL_DIR=""
USER_NAME="l337"
GROUP_NAME="l337"

# ---------------------------------------------------------------------------
# Flags
# ---------------------------------------------------------------------------
DRY_RUN=false
UNINSTALL=false
REMOVE_DATA=false
NO_AUDIO=false
TARGET_USER=""
AUTO_INSTALL_DEPS=true
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
    ensure_base_tools curl file readelf ldconfig
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

run_as_user() {
    local user="$1"
    shift
    if command -v sudo &>/dev/null; then
        local user_uid
        user_uid=$(id -u "$user" 2>/dev/null || echo "")
        if [ -n "$user_uid" ]; then
            sudo -u "$user" XDG_RUNTIME_DIR="/run/user/$user_uid" "$@"
        else
            sudo -u "$user" "$@"
        fi
    elif command -v runuser &>/dev/null; then
        runuser -u "$user" -- "$@"
    else
        su - "$user" -c "$*"
    fi
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --binary) BINARY_INPUT="$2"; shift 2 ;;
        --release) RELEASE_TAG="$2"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        --no-install-deps) AUTO_INSTALL_DEPS=false; shift ;;
        --user) TARGET_USER="$2"; shift 2 ;;
        --prefix) PREFIX_INSTALL_DIR="$2"; shift 2 ;;
        --no-audio) NO_AUDIO=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [OPTIONS] --binary <path>

Linux PipeWire installer for L337 Audio Server.
Installs a downloaded binary as systemd user services for every user with an
active PipeWire session. Release lookup, the version check and the download
itself live in install.sh, which resolves the newest release that ships a
PipeWire binary for this platform and passes --binary here.

Options:
  --binary <path>        Binary to install (required for install; passed by install.sh)
  --release <tag>        Release tag the binary came from (informational)
  --dry-run              Show what would happen without making changes
  --no-install-deps      Do not auto-install missing base tools and audio libraries
  --user <username>      Install for a specific user only (default: all PipeWire users)
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

    if run_as_user "$user" -- systemctl --user is-active --quiet pipewire 2>/dev/null; then
        return 0
    fi

    if run_as_user "$user" -- sh -c 'command -v pactl >/dev/null 2>&1 && pactl info >/dev/null 2>&1' 2>/dev/null; then
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
    local xdg_env
    xdg_env=$(run_as_user "$real_user" sh -c 'echo XDG_CONFIG_HOME=${XDG_CONFIG_HOME:-} XDG_CACHE_HOME=${XDG_CACHE_HOME:-} XDG_STATE_HOME=${XDG_STATE_HOME:-} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-}') || true
    eval "$xdg_env" 2>/dev/null || true
    local user_config_dir="${XDG_CONFIG_HOME:-$real_home/.config}/l337-audio-server"
    local user_cache_dir="${XDG_CACHE_HOME:-$real_home/.cache}/l337-audio-server"
    local user_state_dir="${XDG_STATE_HOME:-$real_home/.local/state}/l337-audio-server"
    local user_runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u "$real_user")}/l337-audio-server"
    local user_service_dir="$real_home/.config/systemd/user"
    local user_service_file="$user_service_dir/l337-audio-server.service"

    mkdir -p "$INSTALL_DIR" "$user_config_dir" "$user_cache_dir" "$user_state_dir" "$user_service_dir"
    if [ -d "$user_runtime_dir" ]; then
        chown "${real_user}:${real_user}" "$user_runtime_dir"
        chmod 0700 "$user_runtime_dir"
    fi

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

RuntimeDirectory=l337-audio-server
RuntimeDirectoryMode=0700

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
    run_as_user "$real_user" -- systemctl --user stop l337-audio-server.service 2>/dev/null || true
    if run_as_user "$real_user" -- systemctl --user is-active --quiet l337-audio-server.service 2>/dev/null; then
        fail "Service l337-audio-server.service failed to stop"
    fi

    info "Replacing binary..."
    if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
        mv "$INSTALL_DIR/l337-audio-server" "$INSTALL_DIR/l337-audio-server.bak"
    fi
    mv "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"

    run_as_user "$real_user" -- systemctl --user daemon-reload || true
    run_as_user "$real_user" -- systemctl --user enable l337-audio-server.service || true
    run_as_user "$real_user" -- systemctl --user start l337-audio-server.service || true

    # Verify it actually came up (the unit can take seconds after `exec start`).
    # wait_until_ok is a function of this shell, so it must poll locally and let
    # each check go through run_as_user to reach the user's systemd session.
    unit_active() {
        run_as_user "$real_user" -- systemctl --user is-active --quiet l337-audio-server.service 2>/dev/null
    }
    if wait_until_ok 15 unit_active; then
        ok "Systemd user service installed and started for $real_user"
        rm -f "$INSTALL_DIR/l337-audio-server.bak"
        rm -rf "$INSTALL_DIR/.tmp"
        run_as_user "$real_user" -- systemctl --user status l337-audio-server.service --no-pager || true
        return 0
    fi

    run_as_user "$real_user" -- journalctl --user -u l337-audio-server.service --since '1 minute ago' --no-pager 2>/dev/null || true
    warn "New binary failed to start for $real_user. Rolling back..."
    run_as_user "$real_user" -- systemctl --user stop l337-audio-server.service 2>/dev/null || true
    if [ -f "$INSTALL_DIR/l337-audio-server.bak" ]; then
        if mv "$INSTALL_DIR/l337-audio-server.bak" "$INSTALL_DIR/l337-audio-server" 2>/dev/null; then
            chmod 0755 "$INSTALL_DIR/l337-audio-server"
            run_as_user "$real_user" -- systemctl --user daemon-reload || true
            run_as_user "$real_user" -- systemctl --user start l337-audio-server.service 2>/dev/null || true
            if wait_until_ok 15 unit_active; then
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

# Uninstall (multi-user)
# ---------------------------------------------------------------------------
uninstall_user_service() {
    info "Uninstalling systemd user services..."

    local binary_path="${INSTALL_DIR}/l337-audio-server"
    local any_installed=false
    while IFS= read -r user; do
        local real_home
        real_home=$(eval echo "~${user}")
        local xdg_env
        xdg_env=$(run_as_user "$user" sh -c 'echo XDG_CONFIG_HOME=${XDG_CONFIG_HOME:-} XDG_CACHE_HOME=${XDG_CACHE_HOME:-} XDG_STATE_HOME=${XDG_STATE_HOME:-} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-}') || true
        eval "$xdg_env" 2>/dev/null || true
        local user_config_dir="${XDG_CONFIG_HOME:-$real_home/.config}/l337-audio-server"
        local user_cache_dir="${XDG_CACHE_HOME:-$real_home/.cache}/l337-audio-server"
        local user_state_dir="${XDG_STATE_HOME:-$real_home/.local/state}/l337-audio-server"
        local user_runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u "$user")}/l337-audio-server"
        local user_service_dir="$real_home/.config/systemd/user"
        local user_service_file="$user_service_dir/l337-audio-server.service"

        if [ ! -f "$user_service_file" ]; then
            continue
        fi

        any_installed=true
        info "Removing user service for $user..."
        run_as_user "$user" -- systemctl --user stop l337-audio-server.service 2>/dev/null || true
        run_as_user "$user" -- systemctl --user disable l337-audio-server.service 2>/dev/null || true

        rm -f "$user_service_file"
        run_as_user "$user" -- systemctl --user daemon-reload 2>/dev/null || true

        # The runtime dir holds only the socket and instance.lock, so it goes
        # away whether or not --remove-data was given.
        if [ -d "$user_runtime_dir" ]; then
            rm -rf "$user_runtime_dir"
            ok "Runtime removed for $user: $user_runtime_dir"
        fi

        if [ "$REMOVE_DATA" = true ]; then
            info "Removing user data for $user..."
            rm -rf "$user_config_dir" \
                  "$user_cache_dir" \
                  "$user_state_dir"
            ok "Data removed for $user"
        else
            warn "User data retained for $user:"
            warn "  $user_config_dir"
        fi
    done < <(get_all_login_users)

    if [ "$any_installed" = false ]; then
        warn "No user services found"
    fi

    # Every user's service is stopped now. $INSTALL_DIR is shared, so it may
    # only go once no instance still holds the socket or the instance lock —
    # a foreground run is invisible to systemctl and would survive it.
    if pgrep -f "$binary_path" >/dev/null 2>&1; then
        local pids
        pids="$(pgrep -f "$binary_path" | tr '\n' ' ' || true)"
        fail "A manually started l337-audio-server is still running (pid: ${pids}).
Stop it and re-run:  kill ${pids}"
    fi

    rm -rf "$INSTALL_DIR"
    ok "Shared binary removed"

    # A download that fell back to /tmp leaves the whole binary there.
    # A download that fell back to /tmp leaves the whole binary there.
    # The binary install.sh downloads lands in TMPDIR; tidy it on uninstall.
    rm -f "${L337_BINARY_PATH:-}" "/tmp/l337-audio-server-download"

    ok "User service uninstallation complete"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    if [ "$UNINSTALL" = true ]; then
        info "  Scan all users for installed l337-audio-server.service files"
        info "  For each user: stop, disable, remove service and runtime dir"
        info "  Refuse if a manually started instance is still running"
        info "  Remove the shared binary at $INSTALL_DIR"
        if [ "$REMOVE_DATA" = true ]; then
            info "  Remove configuration, cache and state directories per user"
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

if [ ! -f "${BINARY_INPUT}" ]; then
    fail "No binary given. This installer does not download one: run install.sh, or
pass the path yourself:
    bash $0 --binary <path-to-l337-audio-server>
(uninstall always works:  bash $0 --uninstall)"
fi

require_root
check_dependencies

info "Installing ${BINARY_INPUT} (release ${RELEASE_TAG:-unknown})"

# A missing DT_NEEDED library is resolved by the loader before main(), so the
# binary cannot exec in ANY mode (dummy = true included). Check the provided
# binary and install the runtime packages before first boot.
ensure_audio_libs pipewire
ensure_runtime_dependencies "$BINARY_INPUT"

if [ -n "$TARGET_USER" ]; then
    if ! id "$TARGET_USER" &>/dev/null; then
        fail "User not found: $TARGET_USER"
    fi
    if ! user_has_pipewire "$TARGET_USER"; then
        fail "User $TARGET_USER does not have an active PipeWire session"
    fi
    setup_systemd_user "$BINARY_INPUT" "$TARGET_USER"
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
        setup_systemd_user "$BINARY_INPUT" "$user"
    done
fi

# The binary was downloaded into TMPDIR by install.sh: drop it once it is
# safely deployed.
[ "$BINARY_INPUT" != "$INSTALL_DIR/l337-audio-server" ] && rm -f "$BINARY_INPUT"

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
