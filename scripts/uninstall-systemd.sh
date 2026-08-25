#!/bin/bash
# uninstall-systemd.sh — Uninstall the L337 Audio Server systemd service
#
# Removes the system service (or user service), systemd unit file, and optionally
# removes installed files, directories, and the dedicated system user.
#
# Usage:
#   sudo ./scripts/uninstall-systemd.sh                # system service
#   ./scripts/uninstall-systemd.sh --user              # user service
#   ./scripts/uninstall-systemd.sh --remove-data       # also remove /opt, config, state, cache
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-system}"

USER_NAME="l337"
GROUP_NAME="l337"
INSTALL_DIR="/opt/l337-audio-server"
SYSTEM_SERVICE="/etc/systemd/system/l337-audio-server.service"
STATE_DIR="/var/lib/l337-audio-server"
CACHE_DIR="/var/cache/l337-audio-server"
CONFIG_DIR="/etc/l337-audio-server"
REMOVE_DATA=false

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

uninstall_system_service() {
    if [ "$(id -u)" -ne 0 ]; then
        echo "Uninstalling the system service requires root (use sudo)." >&2
        exit 1
    fi

    info "Uninstalling system service..."

    # Migrate/clean legacy unit if present
    local legacy="/etc/systemd/system/l337-audio.service"
    if [ -f "$legacy" ]; then
        info "Removing legacy unit l337-audio.service..."
        systemctl disable l337-audio.service 2>/dev/null || true
        systemctl stop l337-audio.service 2>/dev/null || true
        rm -f "$legacy"
    fi

    if [ -f "$SYSTEM_SERVICE" ]; then
        info "Stopping and disabling l337-audio-server.service..."
        systemctl stop l337-audio-server.service 2>/dev/null || true
        systemctl disable l337-audio-server.service 2>/dev/null || true
        rm -f "$SYSTEM_SERVICE"
        systemctl daemon-reload
        ok "Systemd unit removed"
    else
        warn "Systemd unit not found at $SYSTEM_SERVICE"
    fi

    # Always remove the install directory
    if [ -d "$INSTALL_DIR" ]; then
        info "Removing installed files..."
        rm -rf "$INSTALL_DIR"
        ok "Install directory removed"
    else
        warn "Install directory not found at $INSTALL_DIR"
    fi

    if [ "$REMOVE_DATA" = true ]; then
        info "Removing data directories..."
        rm -rf "$CONFIG_DIR" "$STATE_DIR" "$CACHE_DIR"
        ok "Data directories removed"

        if id "$USER_NAME" &>/dev/null; then
            info "Removing system user '$USER_NAME'..."
            userdel "$USER_NAME" 2>/dev/null || true
            ok "System user removed"
        fi

        if getent group "$GROUP_NAME" &>/dev/null; then
            info "Removing group '$GROUP_NAME'..."
            groupdel "$GROUP_NAME" 2>/dev/null || true
            ok "Group removed"
        fi
    else
        warn "Data directories retained:"
        warn "  $CONFIG_DIR"
        warn "  $STATE_DIR"
        warn "  $CACHE_DIR"
        warn "Re-run with --remove-data to delete them."
    fi

    ok "System service uninstall complete"
}

uninstall_hybrid_service() {
    local real_user="${INSTALL_USER:-$(id -un 2>/dev/null || echo ${SUDO_USER:-${USER}})}"
    local real_uid
    real_uid=$(id -u "$real_user" 2>/dev/null || echo "")
    local runtime_dir="/run/user/$real_uid"

    info "Uninstalling hybrid service (user: $real_user)..."

    # Stop and disable the user service first
    if [ -n "$runtime_dir" ] && [ -d "$runtime_dir" ]; then
        info "Stopping user service for $real_user..."
        sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user stop l337-audio-server.service 2>/dev/null || true
        sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user disable l337-audio-server.service 2>/dev/null || true
    else
        warn "Could not determine runtime dir for $real_user; skipping user service stop"
    fi

    # Remove user unit file
    local real_home=$(getent passwd "$real_user" 2>/dev/null | cut -d: -f6)
    local unit_dir="${XDG_CONFIG_HOME:-${real_home:-/home/$real_user}/.config}/systemd/user"
    local unit="$unit_dir/l337-audio-server.service"
    if [ -f "$unit" ]; then
        rm -f "$unit"
        if [ -n "$runtime_dir" ] && [ -d "$runtime_dir" ]; then
            sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user daemon-reload 2>/dev/null || true
        fi
        ok "User unit removed"
    else
        warn "User unit not found at $unit"
    fi

    # Remove system-wide install dir
    if [ -d "$INSTALL_DIR" ]; then
        info "Removing $INSTALL_DIR..."
        rm -rf "$INSTALL_DIR"
        ok "Removed $INSTALL_DIR"
    else
        warn "$INSTALL_DIR not found"
    fi

    # Remove data directories
    for d in "$CONFIG_DIR" "$STATE_DIR" "$CACHE_DIR"; do
        if [ -d "$d" ]; then
            info "Removing $d..."
            rm -rf "$d"
            ok "Removed $d"
        else
            warn "$d not found"
        fi
    done

    # Remove system user/group
    if id "$USER_NAME" &>/dev/null; then
        info "Removing system user '$USER_NAME'..."
        userdel "$USER_NAME" 2>/dev/null || true
        ok "System user removed"
    else
        warn "User $USER_NAME not found"
    fi

    if getent group "$GROUP_NAME" &>/dev/null; then
        info "Removing group '$GROUP_NAME'..."
        groupdel "$GROUP_NAME" 2>/dev/null || true
        ok "Group removed"
    else
        warn "Group $GROUP_NAME not found"
    fi

    ok "Hybrid service uninstall complete"
}

uninstall_user_service() {
    local real_user="${INSTALL_USER:-$(id -un 2>/dev/null || echo ${SUDO_USER:-${USER}})}"
    local real_uid
    real_uid=$(id -u "$real_user" 2>/dev/null || echo "")
    local runtime_dir="/run/user/$real_uid"
    local real_home=$(getent passwd "$real_user" 2>/dev/null | cut -d: -f6)
    local unit_dir="${XDG_CONFIG_HOME:-${real_home:-/home/$real_user}/.config}/systemd/user"
    local unit="$unit_dir/l337-audio-server.service"

    info "Uninstalling user service..."

    if [ -n "$runtime_dir" ] && [ -d "$runtime_dir" ]; then
        sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user is-enabled --quiet l337-audio-server.service 2>/dev/null || true
        sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user disable l337-audio-server.service 2>/dev/null || true
        sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user stop l337-audio-server.service 2>/dev/null || true
    else
        systemctl --user is-enabled --quiet l337-audio-server.service 2>/dev/null || true
        systemctl --user disable l337-audio-server.service 2>/dev/null || true
        systemctl --user stop l337-audio-server.service 2>/dev/null || true
    fi

    if [ -f "$unit" ]; then
        rm -f "$unit"
        if [ -n "$runtime_dir" ] && [ -d "$runtime_dir" ]; then
            sudo -u "$real_user" XDG_RUNTIME_DIR="$runtime_dir" systemctl --user daemon-reload 2>/dev/null || true
        else
            systemctl --user daemon-reload 2>/dev/null || true
        fi
        ok "User unit removed"
    else
        warn "User unit not found at $unit"
    fi

    # Clean up /etc/l337-audio-server symlink if it points to this user's config
    local etc_symlink="/etc/l337-audio-server/server.ini"
    if [ -L "$etc_symlink" ]; then
        local etc_target
        etc_target=$(readlink -f "$etc_symlink")
        local real_config
        real_config=$(realpath "${real_home:-/home/$real_user}/.config/l337-audio-server/server.ini" 2>/dev/null || echo "${real_home:-/home/$real_user}/.config/l337-audio-server/server.ini")
        if [[ "$etc_target" == "$real_config" ]]; then
            rm -f "$etc_symlink"
            rmdir /etc/l337-audio-server 2>/dev/null || true
            ok "Removed /etc/l337-audio-server symlink"
        fi
    fi

    if [ "$REMOVE_DATA" = true ]; then
        local user_config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/l337-audio-server"
        local user_state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/l337-audio-server"
        local user_cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/l337-audio-server"
        local user_install_dir="${XDG_DATA_HOME:-$HOME/.local/share}/l337-audio-server"

        rm -rf "$user_config_dir" "$user_state_dir" "$user_cache_dir" "$user_install_dir"
        ok "User data directories and installed binary removed"
    else
        warn "User data directories retained. Re-run with --remove-data to delete them."
    fi

    ok "User service uninstall complete"
}

INSTALL_USER=""
while [ $# -gt 0 ]; do
    case "$1" in
        --user)
            if [ $# -gt 1 ] && [[ ! "$2" =~ ^- ]]; then
                INSTALL_USER="$2"
                shift 2
            else
                MODE="user"
                shift
            fi
            ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        system|--system|"") MODE="system"; shift ;;
        user) MODE="user"; shift ;;
        hybrid|--hybrid) MODE="hybrid"; shift ;;
        *) echo "Unknown mode: $1 (use --user, --hybrid, or run as root for system service)"; exit 1 ;;
    esac
done

case "$MODE" in
    system|--system|"") uninstall_system_service ;;
    user|--user)        uninstall_user_service ;;
    hybrid|--hybrid)    uninstall_hybrid_service ;;
    *) echo "Unknown mode: $MODE (use --user, --hybrid, or run as root for system service)"; exit 1 ;;
esac
