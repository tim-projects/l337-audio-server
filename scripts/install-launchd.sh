#!/bin/bash
# install-launchd.sh — macOS installer for L337 Audio Server
#
# Installs the server as a macOS launchd service.
# This is a placeholder implementation.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLIST_LABEL="com.l337.audio-server"
PLIST_DEST="${HOME}/Library/LaunchAgents/${PLIST_LABEL}.plist"

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

info "macOS installer (launchd) — not yet implemented"
info "This is a placeholder. Please install manually or use cargo run for development."

# TODO: Generate a launchd plist and load it with launchctl
# Example structure:
#   <key>Label</key><string>com.l337.audio-server</string>
#   <key>ProgramArguments</key><array><string>/opt/l337-audio-server/l337-audio-server</string></array>
#   <key>RunAtLoad</key><true/>

fail "macOS installer not yet implemented"
