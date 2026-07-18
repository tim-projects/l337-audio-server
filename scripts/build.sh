#!/bin/bash
# Build the L337 Audio Server and place the binary in ./bin
#
# Requires cargo/rustc. Run this on a machine that has the Rust toolchain
# (the install script only deploys the prebuilt binary and needs no cargo).
#
# Usage:
#   ./scripts/build.sh                       # native release build
#   ./scripts/build.sh --target aarch64-unknown-linux-gnu   # cross build
set -e

TARGET=""
if [ "${1:-}" = "--target" ]; then
    TARGET="$2"
fi

if [ -n "$TARGET" ]; then
    echo "Building L337 Audio Server for target $TARGET (release)..."
    cargo build --release --target "$TARGET"
    SRC="target/$TARGET/release/l337-audio-server"
else
    echo "Building L337 Audio Server (release)..."
    cargo build --release
    SRC="target/release/l337-audio-server"
fi

mkdir -p bin
cp -f "$SRC" bin/l337-audio-server
chmod +x bin/l337-audio-server

echo "Build complete. Binary available at bin/l337-audio-server"
