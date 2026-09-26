#!/bin/bash
# install.sh — Downloads and executes the platform-specific installer
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

REPO="tim-projects/l337-audio-server"

fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

# --- Argument parsing ---
INSTALL_PRERELEASE=false
ORIGINAL_ARGS=("$@")
while [ $# -gt 0 ]; do
    case "$1" in
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--pre-release] [--uninstall] [--force] ..."
            echo "Delegates to platform-specific installer on GitHub."
            exit 0 ;;
        *) shift ;;
    esac
done

# --- Platform detection ---
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Linux*)  OS_TYPE="linux" ;;
    Darwin*) OS_TYPE="macos" ;;
    *) fail "Unsupported OS: $OS" ;;
esac
case "$ARCH" in
    x86_64|amd64)  ARCH_TYPE="x86_64" ;;
    aarch64|arm64) ARCH_TYPE="aarch64" ;;
    armv7l|armhf)  ARCH_TYPE="armv7" ;;
    riscv64)       ARCH_TYPE="riscv64" ;;
    *)             fail "Unsupported architecture: $ARCH" ;;
esac

# --- Determine release tag ---
RELEASE_RESPONSE=$(mktemp)
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

# --- Determine script to download ---
if [ "$OS_TYPE" = "linux" ]; then
    if [ "$ARCH_TYPE" = "armv7" ] || [ "$ARCH_TYPE" = "riscv64" ]; then
        SCRIPT_NAME="install-linux-alsa"
    else
        PW_DETECTED=false
        if systemctl is-active --quiet pipewire.service 2>/dev/null || \
           [ -S "/run/pipewire/0" ] || [ -d "/run/pipewire" ]; then
            PW_DETECTED=true
        fi
        if [ "$PW_DETECTED" = false ]; then
            while IFS=: read -r username _ uid _ _ home _; do
                if [ "$uid" -ge 1000 ] && [ -d "/run/user/$uid" ]; then
                    if [ -S "/run/user/$uid/pipewire-0" ] || [ -S "/run/user/$uid/pulse/native" ]; then
                        PW_DETECTED=true
                        break
                    fi
                fi
            done < /etc/passwd
        fi
        if [ "$PW_DETECTED" = true ]; then
            SCRIPT_NAME="install-linux-pipewire"
        else
            SCRIPT_NAME="install-linux-alsa"
        fi
    fi
elif [ "$OS_TYPE" = "macos" ]; then
    SCRIPT_NAME="install-macos"
else
    fail "Unsupported OS: $OS"
fi

# --- Download and execute ---
SCRIPT_URL="https://raw.githubusercontent.com/${REPO}/${TAG}/scripts/${SCRIPT_NAME}.sh"
SCRIPT_PATH=$(mktemp /tmp/l337-install-XXXXXX.sh)
if ! curl -fsSL --connect-timeout 15 --max-time 60 -o "$SCRIPT_PATH" "$SCRIPT_URL"; then
    fail "Failed to download installer script from $SCRIPT_URL"
fi
chmod +x "$SCRIPT_PATH"
exec "$SCRIPT_PATH" "${ORIGINAL_ARGS[@]}"
