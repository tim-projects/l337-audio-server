#!/bin/bash
# build.sh — Cross-platform build dispatcher for L337 Audio Server
#
# Detects OS/architecture and runs the correct platform-specific build
# script under scripts/.
#
# Usage:
#   ./build.sh                       # native build (Linux/macOS)
#   ./build.sh --target aarch64-unknown-linux-gnu   # cross-build
#   ./build.sh --help
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/scripts" && pwd)"

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --target TARGET        Cross-build for TARGET (passed to cargo)
  -h, --help             Show this help message

Platforms:
  Linux/macOS            scripts/build.sh
  Windows                scripts/build.ps1 (PowerShell)
EOF
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

OS="$(uname -s)"

case "$OS" in
    Linux*|Darwin*)
        info "Detected Unix-like system: $OS"
        BUILD_SCRIPT="$SCRIPT_DIR/build.sh"
        if [ ! -x "$BUILD_SCRIPT" ]; then
            fail "Build script not found or not executable: $BUILD_SCRIPT"
        fi
        if [ -n "${TARGET:-}" ]; then
            "$BUILD_SCRIPT" --target "$TARGET"
        else
            "$BUILD_SCRIPT"
        fi
        ;;
    MINGW*|MSYS*|CYGWIN*)
        info "Detected Windows. Please run the PowerShell build script instead:"
        info "  scripts\\build.ps1"
        exit 0
        ;;
    *)
        fail "Unsupported OS: $OS"
        ;;
esac

ok "Build complete"
