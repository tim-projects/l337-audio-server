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
#   ./scripts/build.sh                                       # native release build
#   ./scripts/build.sh --debug                               # debug build with symbols
#   ./scripts/build.sh --release-debuginfo                   # release + debug symbols
#   ./scripts/build.sh --no-check                            # skip cargo check
#   ./scripts/build.sh --target aarch64-unknown-linux-gnu     # cross build
#   ./scripts/build.sh --cargo-home /path/to/cargo            # use existing cargo
#   ./scripts/build.sh --cargo-bin /path/to/cargo/bin         # add cargo to PATH
#   ./scripts/build.sh --target-dir /path/to/target           # override cargo target dir
set -e

LOG_FILE="/tmp/l337-audio-server-build.log"
exec > >(tee -a "$LOG_FILE") 2>&1
echo "[$(date '+%Y-%m-%d %H:%M:%S')] Build started"

TARGET=""
CARGO_HOME_ARG=""
CARGO_BIN_ARG=""
TARGET_DIR_ARG=""
DEBUG_BUILD=0
RELEASE_DEBUGINFO=0
NO_CHECK=0

while [ $# -gt 0 ]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        --debug) DEBUG_BUILD=1; shift ;;
        --release-debuginfo) RELEASE_DEBUGINFO=1; shift ;;
        --no-check) NO_CHECK=1; shift ;;
        --cargo-home) CARGO_HOME_ARG="$2"; shift 2 ;;
        --cargo-bin) CARGO_BIN_ARG="$2"; shift 2 ;;
        --target-dir) TARGET_DIR_ARG="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Allow caller to point at an existing cargo installation instead of
# downloading one into /tmp.
if [ -n "$CARGO_HOME_ARG" ]; then
    CARGO_HOME="$CARGO_HOME_ARG"
    export CARGO_HOME
fi

if [ -n "$CARGO_BIN_ARG" ]; then
    export PATH="$CARGO_BIN_ARG:$PATH"
fi

# Build into /tmp so we never depend on an in-tree ./target (which can be a
# broken mount on some shared setups). Caller can override with --target-dir.
export CARGO_TARGET_DIR="${TARGET_DIR_ARG:-/tmp/l337-build}"

# If cargo is not on PATH, install a minimal toolchain into /tmp/cargo/.
if ! command -v cargo >/dev/null 2>&1; then
    CARGO_HOME="${CARGO_HOME:-/tmp/cargo}"
    RUSTUP_HOME="${RUSTUP_HOME:-/tmp/rustup}"
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
    if [ "$DEBUG_BUILD" -eq 1 ]; then
        echo "Building L337 Audio Server (debug) in $CARGO_TARGET_DIR ..."
        cargo build
        SRC="$CARGO_TARGET_DIR/debug/l337-audio-server"
    elif [ "$RELEASE_DEBUGINFO" -eq 1 ]; then
        echo "Building L337 Audio Server (release with debuginfo) in $CARGO_TARGET_DIR ..."
        RUSTFLAGS="${RUSTFLAGS:-} -C debuginfo=1" cargo build --release
        SRC="$CARGO_TARGET_DIR/release/l337-audio-server"
    else
        echo "Building L337 Audio Server (release) in $CARGO_TARGET_DIR ..."
        cargo build --release
        SRC="$CARGO_TARGET_DIR/release/l337-audio-server"
    fi
fi

if [ ! -f "$SRC" ]; then
    echo "Build succeeded but binary not found at: $SRC" >&2
    exit 1
fi

mkdir -p bin
cp -f "$SRC" bin/l337-audio-server
chmod +x bin/l337-audio-server

echo "Build complete. Binary available at bin/l337-audio-server"

# Run a fast compilation check to catch breakage before deployment.
if [ "$NO_CHECK" -ne 1 ]; then
    if [ -n "$TARGET" ]; then
        echo "Running cargo check for target $TARGET ..."
        cargo check --target "$TARGET"
    else
        echo "Running cargo check ..."
        cargo check
    fi
    echo "[OK] Build and check complete"
else
    echo "[OK] Build complete (check skipped)"
fi
