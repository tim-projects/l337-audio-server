#!/bin/bash
# install.sh — Cross-platform installer for L337 Audio Server
#
# Detects OS/architecture, checks dependencies, and dispatches to the
# correct platform-specific installer under scripts/.
#
# Usage:
#   ./install.sh                        # install (Linux → systemd)
#   ./install.sh --dry-run              # show what would happen
#   ./install.sh --uninstall            # remove installed service/files
#   ./install.sh --help
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/scripts" && pwd)"
DRY_RUN=false
UNINSTALL=false

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --dry-run              Show what would happen without making changes
  --uninstall, -u        Uninstall the service and remove installed files
  -h, --help             Show this help message

Platforms:
  Linux (systemd)        scripts/install-systemd.sh
  macOS (launchd)        scripts/install-launchd.sh (placeholder)
  Windows                scripts/install.ps1 (PowerShell)
EOF
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        -h|--help) usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux*)     OS_TYPE="linux" ;;
    Darwin*)    OS_TYPE="macos" ;;
    MINGW*|MSYS*|CYGWIN*) OS_TYPE="windows" ;;
    *)          fail "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH_TYPE="x64" ;;
    aarch64|arm64) ARCH_TYPE="arm64" ;;
    armv7l)        ARCH_TYPE="armv7" ;;
    i686|i386)     ARCH_TYPE="x86" ;;
    *)             fail "Unsupported architecture: $ARCH" ;;
esac

info "Detected platform: $OS_TYPE / $ARCH_TYPE ($OS $ARCH)"

# ---------------------------------------------------------------------------
# Dependency checks
# ---------------------------------------------------------------------------
check_command() {
    if command -v "$1" &>/dev/null; then
        ok "$1 found: $(command -v "$1")"
        return 0
    else
        warn "$1 not found"
        return 1
    fi
}

check_linux_deps() {
    info "Checking Linux dependencies..."

    local missing=0

    if ! check_command "bash"; then missing=1; fi
    if ! check_command "systemctl"; then
        warn "systemd not found — systemd installer requires systemd"
        missing=1
    fi

    if [ "$missing" -ne 0 ]; then
        fail "Missing dependencies. Install them and re-run this script."
    fi
}

check_macos_deps() {
    info "Checking macOS dependencies..."

    if ! check_command "bash"; then
        fail "bash is required"
    fi

    if ! check_command "brew"; then
        warn "Homebrew not found. Some dependencies may need to be installed manually."
    fi
}

check_deps() {
    case "$OS_TYPE" in
        linux)   check_linux_deps ;;
        macos)   check_macos_deps ;;
        windows) ;;
    esac
}

# ---------------------------------------------------------------------------
# Install dispatch
# ---------------------------------------------------------------------------
run_install() {
    if [ "$DRY_RUN" = true ]; then
        info "Dry-run mode — would run: $INSTALL_CMD"
        return 0
    fi

    info "Running installer: $INSTALL_CMD"
    eval "$INSTALL_CMD"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$UNINSTALL" = false ]; then
    check_deps
fi

case "$OS_TYPE" in
    linux)
        if [ "$UNINSTALL" = true ]; then
UNINSTALL_SCRIPT="$SCRIPT_DIR/uninstall-systemd.sh"
        if [ ! -x "$UNINSTALL_SCRIPT" ]; then
            fail "Uninstaller script not found or not executable: $UNINSTALL_SCRIPT"
        fi
        UNINSTALL_CMD="\"$UNINSTALL_SCRIPT\" --user"
        else
            INSTALL_SCRIPT="$SCRIPT_DIR/install-systemd.sh"
            if [ ! -x "$INSTALL_SCRIPT" ]; then
                fail "Installer script not found or not executable: $INSTALL_SCRIPT"
            fi
            INSTALL_CMD="\"$INSTALL_SCRIPT\" --auto"
        fi
        ;;
    macos)
        if [ "$UNINSTALL" = true ]; then
            UNINSTALL_SCRIPT="$SCRIPT_DIR/uninstall-launchd.sh"
            if [ ! -x "$UNINSTALL_SCRIPT" ]; then
                fail "Uninstaller script not found or not executable: $UNINSTALL_SCRIPT"
            fi
            UNINSTALL_CMD="\"$UNINSTALL_SCRIPT\""
        else
            INSTALL_SCRIPT="$SCRIPT_DIR/install-launchd.sh"
            if [ ! -x "$INSTALL_SCRIPT" ]; then
                fail "macOS installer not found or not executable: $INSTALL_SCRIPT"
            fi
            INSTALL_CMD="\"$INSTALL_SCRIPT\""
        fi
        ;;
    windows)
        info "Windows detected. Please run the PowerShell installer/uninstaller instead."
        exit 0
        ;;
esac

if [ "$UNINSTALL" = true ]; then
    info "Running uninstaller: $UNINSTALL_CMD"
    eval "$UNINSTALL_CMD"
    ok "Uninstallation complete"
else
    run_install
    ok "Installation complete"
fi
