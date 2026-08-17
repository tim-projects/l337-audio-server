#!/bin/bash
# uninstall-launchd.sh — macOS uninstaller for L337 Audio Server
#
# This is a placeholder since install-launchd.sh is not yet implemented.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLIST_LABEL="com.l337.audio-server"
PLIST_DEST="${HOME}/Library/LaunchAgents/${PLIST_LABEL}.plist"

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

info "macOS uninstaller (launchd) — not yet implemented"
fail "macOS uninstaller not yet implemented (installer is a placeholder)"
