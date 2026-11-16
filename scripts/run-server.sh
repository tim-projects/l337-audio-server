#!/bin/bash
# Run the L337 Audio Server.
# Uses the prebuilt binary from the cargo target dir (created by `scripts/build.sh`).
# Falls back to `cargo run` only if the binary is missing (e.g. local development).
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Binary is in CARGO_TARGET_DIR/release/l337-audio-server or /tmp/l337-build/release/
BIN="${CARGO_TARGET_DIR:-/tmp/l337-build}/release/l337-audio-server"

if [ -x "$BIN" ]; then
    exec "$BIN"
else
    echo "Binary not found at $BIN; falling back to 'cargo run'..." >&2
    cd "$SCRIPT_DIR"
    exec cargo run
fi
