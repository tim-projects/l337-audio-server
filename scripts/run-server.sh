#!/bin/bash
# Run the L337 Audio Server.
# Uses the prebuilt binary in ./bin (created by `scripts/build.sh`). Falls back
# to `cargo run` only if the binary is missing (e.g. local development).
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$SCRIPT_DIR/bin/l337-audio-server"

if [ -x "$BIN" ]; then
    exec "$BIN"
else
    echo "Binary not found at $BIN; falling back to 'cargo run'..." >&2
    cd "$SCRIPT_DIR"
    exec cargo run
fi
