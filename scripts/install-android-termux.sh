#!/bin/bash
# install-android-termux.sh — Termux binary installer (runit/termux-services, no systemd)
# Requirements: Termux + aarch64 + termux-services + running pulseaudio.
# Fails fast with actionable help if any are missing.
set -euo pipefail

REPO="tim-projects/l337-audio-server"
# ASSET is selected below based on the host Termux libc flavor.
INSTALL_DIR="${HOME}/.local/share/l337-player/bin"
BINARY_PATH="${INSTALL_DIR}/l337-audio-server"
SERVICE_NAME="l337-audio-server"
PREFIX="${PREFIX:-/data/data/com.termux/files/usr}"
TMPDIR="${TMPDIR:-/tmp}"
SERVICE_DIR="${PREFIX}/var/service/${SERVICE_NAME}"
RUN_SCRIPT="${SERVICE_DIR}/run"
SOCKET_SOURCE="${TMPDIR}/l337-audio-server/runtime/l337.sock"
SOCKET_TARGET="${HOME}/.cache/l337/l337-audio-server/l337.sock"

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

# Stop every running instance of the server before the binary is replaced, the
# way scripts/install-systemd.sh stops its unit before deploying. Two things
# can be running: the termux-services (runit) service and a manually started
# process. A survivor keeps the Unix socket actively listening, and the new
# binary refuses to remove/rebind it and exits — so an update appears to do
# nothing. Wait until the processes are actually gone before deploying.
stop_existing_instances() {
    if [ -d "${SERVICE_DIR}" ]; then
        sv down "${SERVICE_NAME}" >/dev/null 2>&1 || true
    fi

    if pgrep -f "${BINARY_PATH}" >/dev/null 2>&1; then
        info "Stopping running l337-audio-server instances..."
        pkill -f "${BINARY_PATH}" >/dev/null 2>&1 || true
    fi

    for _ in $(seq 1 10); do
        pgrep -f "${BINARY_PATH}" >/dev/null 2>&1 || return 0
        sleep 1
    done

    local pids
    pids="$(pgrep -f "${BINARY_PATH}" | tr '\n' ' ' || true)"
    fail "l337-audio-server is still running (pid: ${pids}).
A running instance holds the Unix socket and instance lock, so the new
binary cannot start. Stop it manually and re-run the installer:
    kill ${pids}"
}

# Parse arguments
INSTALL_PRERELEASE=false
while [ $# -gt 0 ]; do
    case "$1" in
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--pre-release]"
            echo ""
            echo "Installs l337-audio-server for Termux (android build) and"
            echo "registers it as a termux-services (runit) service."
            echo ""
            echo "Requirements:"
            echo "  - Running in Termux (TERMUX_VERSION set)"
            echo "  - aarch64 architecture"
            echo "  - pkg install termux-services"
            echo "  - pulseaudio installed and running:"
            echo "      pkg install pulseaudio"
            echo "      pulseaudio --start --exit-idle-time=-1"
            echo ""
            echo "Options:"
            echo "  --pre-release   Install latest prerelease instead of stable release"
            exit 0 ;;
        *) echo "Unknown option: $1" >&2; echo "Try $0 --help" >&2; exit 1 ;;
    esac
done

# Termux reports `uname -s` = Linux; identify as termux so logs don't look
# like a Linux/systemd install. Same signal the player uses: TERMUX_VERSION.
ARCH_TYPE="$(uname -m)"
case "$ARCH_TYPE" in
    aarch64|arm64) ARCH_TYPE="aarch64" ;;
    *) ARCH_TYPE="$(uname -m)" ;;
esac
info "Platform: termux / $ARCH_TYPE ($(uname -s) $(uname -m))"
info "User-space install — no root or sudo required (Termux has neither)"

# Termux uses the bionic (NDK) android build, which dlopens the PulseAudio
# client at runtime (built with `--features pulseaudio` in CI).
ASSET="l337-audio-server-aarch64-android-termux"

# --- Requirement checks (fail fast with actionable help) ---
MISSING=()

if [ -z "${TERMUX_VERSION:-}" ]; then
    MISSING+=("Running in Termux (TERMUX_VERSION is not set)")
fi

case "$(uname -m)" in
    aarch64|arm64) ;;
    *) MISSING+=("aarch64 architecture (detected: $(uname -m))") ;;
esac

# `sv` is the termux-services (runit) supervision binary. This installer is a
# runit service installer, so it cannot do anything without it — check it
# first and name the exact install command. Note: $PREFIX/var/service is NOT
# a reliable signal (the openssh package ships service dirs there without
# termux-services), so only the binary is checked.
if ! command -v sv >/dev/null 2>&1; then
    fail "termux-services is not installed: 'sv' not found in PATH.

This installer registers the server as a runit service, which needs termux-services:
  pkg install termux-services

After installing, open a NEW Termux session — runit (runsvdir) only starts
with a new session — then re-run this installer."
fi
ok "sv found: $(command -v sv) (termux-services)"

# The android build (compiled with --features pulseaudio) prefers PulseAudio,
# because direct AAudio from a Termux console process is unreliable:
# AAudio_createStreamBuilder returns null and the server exits at startup.
if ! command -v pulseaudio &>/dev/null; then
    MISSING+=("pulseaudio  →  pkg install pulseaudio")
elif ! pulseaudio --check &>/dev/null; then
    MISSING+=("pulseaudio daemon  →  pulseaudio --start --exit-idle-time=-1")
fi

if [ "${#MISSING[@]}" -gt 0 ]; then
    echo -e "\033[1;31m[FAIL]\033[0m Requirements not met:" >&2
    for item in "${MISSING[@]}"; do
        echo "  - $item" >&2
    done
    echo >&2
    echo "Install missing requirements, then re-run:" >&2
    echo "  $0 --pre-release" >&2
    exit 1
fi

# --- Determine release tag ---
RELEASE_RESPONSE=$(mktemp "${TMPDIR:-/tmp}/l337-release.XXXXXX")
if [ "$INSTALL_PRERELEASE" = true ]; then
    if ! curl -sS -L --connect-timeout 15 --max-time 60 \
        -o "$RELEASE_RESPONSE" \
        "https://api.github.com/repos/${REPO}/releases?per_page=100"; then
        rm -f "$RELEASE_RESPONSE"
        fail "Failed to contact GitHub Releases API. Check network connectivity."
    fi
    TAG=$(awk '/^  \{/{r=$0;in_r=1;next} in_r{r=r"\n"$0;if($0~/^  \},?$/){if(r~/"prerelease"[[:space:]]*:[[:space:]]*true/){printf "%s",r;exit};r="";in_r=0}}' "$RELEASE_RESPONSE" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
else
    if ! curl -sS -L --connect-timeout 15 --max-time 60 \
        -o "$RELEASE_RESPONSE" \
        "https://api.github.com/repos/${REPO}/releases/latest"; then
        rm -f "$RELEASE_RESPONSE"
        fail "Failed to contact GitHub Releases API. Check network connectivity."
    fi
    TAG=$(grep '"tag_name"' "$RELEASE_RESPONSE" | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/') || true
fi
rm -f "$RELEASE_RESPONSE"

if [ -z "$TAG" ]; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        fail "Could not determine latest prerelease tag for ${REPO}."
    fi
    fail "Could not determine latest stable release tag for ${REPO}. Re-run with --pre-release to install a prerelease."
fi

info "Latest release: $TAG"

# --- Download binary ---
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET}"
mkdir -p "${INSTALL_DIR}"

info "Downloading ${ASSET}..."
if ! curl -fL --connect-timeout 15 --max-time 120 -o "${BINARY_PATH}.tmp" "${DOWNLOAD_URL}"; then
    rm -f "${BINARY_PATH}.tmp"
    fail "Failed to download binary from ${DOWNLOAD_URL}"
fi

# Verify ELF magic
if ! head -c4 "${BINARY_PATH}.tmp" | grep -q $'\x7fELF'; then
    rm -f "${BINARY_PATH}.tmp"
    fail "Downloaded file is not a valid binary (got: $(file -b "${BINARY_PATH}.tmp"))"
fi

chmod +x "${BINARY_PATH}.tmp"
stop_existing_instances
mv "${BINARY_PATH}.tmp" "${BINARY_PATH}"
ok "Installed ${BINARY_PATH}"

# --- Symlink socket for older player versions ---
# The android build binds at <TMPDIR>/l337-audio-server/runtime/l337.sock.
# Players with AndroidPlatformPlugin (which computes this path natively) don't
# need it; the symlink keeps older clients working.
ensure_socket_symlink() {
    if [ ! -S "${SOCKET_SOURCE}" ]; then
        return 1
    fi
    if [ -L "${SOCKET_TARGET}" ] && [ "$(readlink "${SOCKET_TARGET}")" = "${SOCKET_SOURCE}" ]; then
        return 0
    fi
    mkdir -p "$(dirname "${SOCKET_TARGET}")"
    ln -sf "${SOCKET_SOURCE}" "${SOCKET_TARGET}"
    ok "Socket linked: ${SOCKET_TARGET} -> ${SOCKET_SOURCE}"
}

# --- Register as termux-services service ---
# Old instances were already stopped before the binary was deployed above.
info "Registering termux-services service..."

mkdir -p "${SERVICE_DIR}"
    cat > "${RUN_SCRIPT}" <<'SVCEOF'
#!/data/data/com.termux/files/usr/bin/sh
# Termux-services (runit) run script for l337-audio-server.
# runit restarts this script if the process exits, giving basic supervision.
BINARY="${HOME}/.local/share/l337-player/bin/l337-audio-server"

if [ ! -x "$BINARY" ]; then
    echo "l337-audio-server binary not found at $BINARY" >&2
    exit 1
fi

# The termux build has DT_NEEDED libpulse.so (linked at build time against a
# stub, resolved here at runtime). The NDK-built binary has no DT_RUNPATH and
# Termux leaves LD_LIBRARY_PATH unset, so libpulse.so is not found on its own.
# Without this export the loader fails outright (CANNOT LINK EXECUTABLE).
export LD_LIBRARY_PATH="${PREFIX:-/data/data/com.termux/files/usr}/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

exec "$BINARY" --transport=socket
SVCEOF
chmod +x "${RUN_SCRIPT}"

# runsvdir scans SVDIR only every few seconds and creates supervise/ok once it
# has launched runsv for this dir. `sv up` needs that file or it errors out with
# "unable to open supervise/ok: file does not exist", so wait for supervision to
# appear rather than racing it.
supervised=false
for _ in $(seq 1 20); do
    if [ -e "${SERVICE_DIR}/supervise/ok" ]; then supervised=true; break; fi
    sleep 1
done
if [ "$supervised" != true ]; then
    fail "Could not register the service: runit (runsvdir) is not running.
termux-services starts runit with a new session, so if you just installed
termux-services, open a NEW Termux session and re-run this installer.
You can verify with: 'pgrep -x runsvdir'"
fi

sv up "${SERVICE_NAME}" || fail "sv up failed.
If termux-services was installed in THIS session, runit is not running yet —
open a NEW Termux session and re-run. Otherwise: 'sv status ${SERVICE_NAME}'"

# Give the server a moment to create its socket + token, then link it. If the
# service is not actually running, the server exited at startup (typically an
# audio-backend init failure) — warn instead of silently declaring success.
ok=false
for _ in $(seq 1 10); do
    case "$(sv status "${SERVICE_NAME}" 2>/dev/null)" in
        run:*) ok=true; break ;;
    esac
    sleep 1
done
if [ "$ok" = true ]; then
    ensure_socket_symlink || warn "Socket not found yet; check 'sv status ${SERVICE_NAME}'"
else
    warn "Service is not running: 'sv status ${SERVICE_NAME}'"
    warn "The server is exiting on startup, usually an audio-backend failure."
    warn "On devices without a usable audio device, set dummy = true in"
    warn "${SERVICE_DIR}/server.ini and run: sv restart ${SERVICE_NAME}"
fi

ok "Service registered: ${SERVICE_NAME}"
echo
echo "  Service controls:"
echo "    sv up ${SERVICE_NAME}       # start"
echo "    sv down ${SERVICE_NAME}     # stop"
echo "    sv status ${SERVICE_NAME}   # check"
echo "    sv-enable ${SERVICE_NAME}   # auto-start on Termux launch"
echo
echo "  Token: ${HOME}/.cache/l337/l337-audio-server/server_token.txt"

echo
echo "Done. Start l337 to connect."
