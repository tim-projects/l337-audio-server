#!/bin/bash
# Build the L337 Audio Server and place the binary in ./bin
#
# If cargo/rustc is not found on PATH, this script will download rustup and
# install a minimal Rust toolchain into /tmp/cargo/ automatically.
#
# The build uses a scratch directory under /tmp (CARGO_TARGET_DIR) so it works
# even when the in-tree ./target is a broken mount/symlink. The finished binary
# is always copied to ./bin/l337-audio-server for the installer to deploy.
#
# Usage:
#   ./scripts/build.sh                       # native release build
#   ./scripts/build.sh --target aarch64-unknown-linux-gnu   # cross build
set -e

LOG_FILE="/tmp/l337-audio-server-build.log"
exec > >(tee -a "$LOG_FILE") 2>&1
echo "[$(date '+%Y-%m-%d %H:%M:%S')] Build started"

TARGET=""
if [ "${1:-}" = "--target" ]; then
    TARGET="$2"
fi

# Build into /tmp so we never depend on an in-tree ./target (which can be a
# broken mount on some shared setups).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/l337-build}"

# If cargo is not on PATH, install a minimal toolchain into /tmp/cargo/.
if ! command -v cargo >/dev/null 2>&1; then
    CARGO_HOME="/tmp/cargo"
    RUSTUP_HOME="/tmp/rustup"
    export CARGO_HOME
    export RUSTUP_HOME
    export PATH="$CARGO_HOME/bin:$PATH"

    if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
        echo "cargo not found; installing Rust toolchain into $CARGO_HOME ..."
        mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain stable --no-modify-path
        echo "Rust toolchain installed at $CARGO_HOME"
    else
        echo "Using existing cargo at $CARGO_HOME/bin/cargo"
    fi
fi

if [ -n "$TARGET" ]; then
    echo "Building L337 Audio Server for target $TARGET (release) in $CARGO_TARGET_DIR ..."
    cargo build --release --target "$TARGET"
    SRC="$CARGO_TARGET_DIR/$TARGET/release/l337-audio-server"
else
    echo "Building L337 Audio Server (release) in $CARGO_TARGET_DIR ..."
    cargo build --release
    SRC="$CARGO_TARGET_DIR/release/l337-audio-server"
fi

if [ ! -f "$SRC" ]; then
    echo "Build succeeded but binary not found at: $SRC" >&2
    exit 1
fi

mkdir -p bin
cp -f "$SRC" bin/l337-audio-server
chmod +x bin/l337-audio-server

echo "Build complete. Binary available at bin/l337-audio-server"
