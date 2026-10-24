#!/bin/bash
# install-android-termux.sh — Termux binary installer (runit/termux-services, no systemd)
# Requirements: Termux + aarch64 + termux-services. Fails fast with help if missing.
set -euo pipefail

REPO="tim-projects/l337-audio-server"
ASSET="l337-audio-server-aarch64-android-termux"
INSTALL_DIR="${HOME}/.local/share/l337-player/bin"
BINARY_PATH="${INSTALL_DIR}/l337-audio-server"
SERVICE_NAME="l337-audio-server"

info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

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
            echo ""
            echo "Options:"
            echo "  --pre-release   Install latest prerelease instead of stable release"
            exit 0 ;;
        *) echo "Unknown option: $1" >&2; echo "Try $0 --help" >&2; exit 1 ;;
    esac
done

# --- Requirement checks (fail fast with actionable help) ---
MISSING=()

if [ -z "${TERMUX_VERSION:-}" ]; then
    MISSING+=("Running in Termux (TERMUX_VERSION is not set)")
fi

case "$(uname -m)" in
    aarch64|arm64) ;;
    *) MISSING+=("aarch64 architecture (detected: $(uname -m))") ;;
esac

if ! command -v sv &>/dev/null; then
    MISSING+=("termux-services  →  pkg install termux-services")
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
mv "${BINARY_PATH}.tmp" "${BINARY_PATH}"
ok "Installed ${BINARY_PATH}"

# --- Symlink socket for older player versions ---
# The android build binds at <TMPDIR>/l337-audio-server/runtime/l337.sock.
# Players with AndroidPlatformPlugin (which computes this path natively) don't
# need it; the symlink keeps older clients working.
TMPDIR="${TMPDIR:-/tmp}"
SOCKET_SOURCE="${TMPDIR}/l337-audio-server/runtime/l337.sock"
SOCKET_TARGET="${HOME}/.cache/l337/l337-audio-server/l337.sock"
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
SERVICE_DIR="${PREFIX}/var/service/${SERVICE_NAME}"
RUN_SCRIPT="${SERVICE_DIR}/run"

info "Registering termux-services service..."

# Stop any running instance so the new run script takes effect cleanly
sv down "${SERVICE_NAME}" 2>/dev/null || true
if [ -S "${SOCKET_SOURCE}" ]; then
    warn "Socket still active after sv down; a manual instance may be running"
    pkill -f "l337-audio-server --transport=socket" 2>/dev/null || true
    sleep 1
fi

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

exec "$BINARY" --transport=socket
SVCEOF
chmod +x "${RUN_SCRIPT}"

sv up "${SERVICE_NAME}" || fail "sv up failed; check 'sv status ${SERVICE_NAME}'"

# Give the server a moment to create socket + token, then link it
sleep 2
ensure_socket_symlink || warn "Socket not found yet; check 'sv status ${SERVICE_NAME}'"

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
