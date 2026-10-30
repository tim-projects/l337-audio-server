#!/bin/bash
# install.sh — Downloads and executes the platform-specific installer
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

export REPO="tim-projects/l337-audio-server"
export SCRIPT_TAG

fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }

# --- Argument parsing ---
INSTALL_PRERELEASE=false
UNINSTALL_REQUESTED=false
ORIGINAL_ARGS=("$@")
while [ $# -gt 0 ]; do
    case "$1" in
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        --uninstall|-u)             UNINSTALL_REQUESTED=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--pre-release] [--uninstall] [--remove-data] [--no-install-deps]"
            echo ""
            echo "Delegates to the platform-specific installer on GitHub. All options"
            echo "are forwarded, so --uninstall behaves exactly like the platform's own"
            echo "installer."
            echo ""
            echo "Options:"
            echo "  --pre-release      Install the latest prerelease instead of the stable release"
            echo "  --uninstall, -u    Remove the service and installed binary (data kept)"
            echo "  --remove-data      With --uninstall: also delete playlists, token and logs"
            echo "  --no-install-deps  Do not auto-install missing system packages (Linux only)"
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

# Termux reports `uname -s` = Linux, so it must be detected as a variant of
# linux (same signal the player uses: TERMUX_VERSION). Termux is a user-space
# install: there is no sudo and no root, so it must be routed before the
# generic linux branch and never reach a systemd/root installer.
OS_VARIANT="$OS_TYPE"
if [ "$OS_TYPE" = "linux" ] && [ -n "${TERMUX_VERSION:-}" ]; then
    OS_VARIANT="termux"
fi

info "Platform: $OS_VARIANT / $ARCH_TYPE"

# --- Determine what version of the installer script to fetch ---
# The tag pins the release the *binary* is downloaded from, so for an install it
# must resolve. An uninstall downloads no binary, so it falls back to the
# installer script on main rather than failing: this project publishes
# prereleases only, so /releases/latest returns nothing and a user must still be
# able to uninstall while no release is reachable. Pass --pre-release together
# with --uninstall to pin the uninstall to the latest release instead.
resolve_release_tag() {
    local response tag
    response=$(mktemp "${TMPDIR:-/tmp}/l337-release.XXXXXX") || return 1

    if [ "$INSTALL_PRERELEASE" = true ]; then
        if ! curl -sS -L --connect-timeout 15 --max-time 60 \
            -o "$response" \
            "https://api.github.com/repos/${REPO}/releases?per_page=100"; then
            rm -f "$response"
            return 1
        fi
        tag=$(awk '/^  \{/{r=$0;in_r=1;next} in_r{r=r"\n"$0;if($0~/^  \},?$/){if(r~/"prerelease"[[:space:]]*:[[:space:]]*true/){printf "%s",r;exit};r="";in_r=0}}' "$response" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
    else
        if ! curl -sS -L --connect-timeout 15 --max-time 60 \
            -o "$response" \
            "https://api.github.com/repos/${REPO}/releases/latest"; then
            rm -f "$response"
            return 1
        fi
        tag=$(grep '"tag_name"' "$response" | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/') || true
    fi

    rm -f "$response"
    printf '%s' "$tag"
}

if ! TAG=$(resolve_release_tag); then
    if [ "$UNINSTALL_REQUESTED" = true ]; then
        warn "Could not reach the GitHub Releases API."
    else
        fail "Failed to contact GitHub Releases API. Check network connectivity."
    fi
fi

if [ -z "$TAG" ]; then
    if [ "$UNINSTALL_REQUESTED" = true ]; then
        SCRIPT_TAG="main"
        warn "No published release found — running the uninstall with the installer script from ${SCRIPT_TAG}."
    elif [ "$INSTALL_PRERELEASE" = true ]; then
        fail "Could not determine latest prerelease tag for ${REPO}."
    else
        fail "Could not determine latest stable release tag for ${REPO}. Re-run with --pre-release to install a prerelease."
    fi
else
    SCRIPT_TAG="$TAG"
fi
info "Installer script: ${SCRIPT_TAG}"

# --- Determine script to download ---
if [ "$OS_VARIANT" = "termux" ]; then
    SCRIPT_NAME="install-android-termux"
elif [ "$OS_TYPE" = "linux" ]; then
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
# The temp file has no filename extension: mktemp's X-run must be at the end of
# the template, and an `.sh` suffix after it is rejected by some
# implementations (e.g. Termux's /bin/mktemp) — bash does not care about it.
SCRIPT_URL="https://raw.githubusercontent.com/${REPO}/${SCRIPT_TAG}/scripts/${SCRIPT_NAME}.sh"
SCRIPT_PATH=$(mktemp "${TMPDIR:-/tmp}/l337-install.XXXXXX")
if ! curl -fsSL --connect-timeout 15 --max-time 60 -o "$SCRIPT_PATH" "$SCRIPT_URL"; then
    fail "Failed to download installer script from $SCRIPT_URL"
fi
chmod +x "$SCRIPT_PATH"
# Execute through bash rather than relying on the downloaded script's shebang:
# the platform installers use `#!/bin/bash`, and /bin/bash does not exist on
# Termux (where /bin is an empty /system/bin symlink).
exec bash "$SCRIPT_PATH" "${ORIGINAL_ARGS[@]}"
