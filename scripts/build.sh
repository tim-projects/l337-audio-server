#!/bin/bash
# Build the L337 Audio Server and place the binary in ./bin
#
# Requires cargo/rustc. Run this on a machine that has the Rust toolchain
# (the install script only deploys the prebuilt binary and needs no cargo).
#
# The build uses a scratch directory under /tmp (CARGO_TARGET_DIR) so it works
# even when the in-tree ./target is a broken mount/symlink. The finished binary
# is always copied to ./bin/l337-audio-server for the installer to deploy.
#
# Usage:
#   ./scripts/build.sh                       # native release build
#   ./scripts/build.sh --target aarch64-unknown-linux-gnu   # cross build
set -e

TARGET=""
if [ "${1:-}" = "--target" ]; then
    TARGET="$2"
fi

# Build into /tmp so we never depend on an in-tree ./target (which can be a
# broken mount on some shared setups).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/l337-build}"

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
