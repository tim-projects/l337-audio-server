#!/bin/bash
# Build the L337 Audio Server
set -e

echo "Building L337 Audio Server..."
cargo build --release

echo "Build complete. Binary available at target/release/l337-audio-server"
