#!/bin/bash
# build.sh — Cross-platform build dispatcher for L337 Audio Server
#
# Detects OS/architecture and runs the correct platform-specific build
# script under scripts/.
#
# Usage:
#   ./build.sh                                          # native build (Linux/macOS)
#   ./build.sh --debug                                  # debug build with symbols
#   ./build.sh --target aarch64-unknown-linux-gnu        # cross-build
#   ./build.sh --cargo-home /path/to/cargo               # use existing cargo
#   ./build.sh --cargo-bin /path/to/cargo/bin            # add cargo to PATH
#   ./build.sh --help
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/scripts" && pwd)"

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --target TARGET        Cross-build for TARGET (passed to cargo)
  --debug                Build debug binary with symbols
  --cargo-home DIR       Use existing CARGO_HOME instead of /tmp/cargo
  --cargo-bin DIR        Add cargo bin dir to PATH
  -h, --help             Show this help message

Platforms:
  Linux/macOS            scripts/build.sh
  Windows                scripts/build.ps1 (PowerShell)
EOF
    exit 0
}

TARGET=""
CARGO_HOME_ARG=""
CARGO_BIN_ARG=""
DEBUG_BUILD=0

while [ $# -gt 0 ]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        --debug) DEBUG_BUILD=1; shift ;;
        --cargo-home) CARGO_HOME_ARG="$2"; shift 2 ;;
        --cargo-bin) CARGO_BIN_ARG="$2"; shift 2 ;;
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
        BUILD_ARGS=()
        if [ -n "$TARGET" ]; then
            BUILD_ARGS+=(--target "$TARGET")
        fi
        if [ "$DEBUG_BUILD" -eq 1 ]; then
            BUILD_ARGS+=(--debug)
        fi
        if [ -n "$CARGO_HOME_ARG" ]; then
            BUILD_ARGS+=(--cargo-home "$CARGO_HOME_ARG")
        fi
        if [ -n "$CARGO_BIN_ARG" ]; then
            BUILD_ARGS+=(--cargo-bin "$CARGO_BIN_ARG")
        fi
        "$BUILD_SCRIPT" "${BUILD_ARGS[@]}"
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
