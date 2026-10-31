#!/bin/bash
# install.sh — Downloads and executes the platform-specific installer
#
# This bootstrap owns EVERYTHING that talks to GitHub: it detects the platform,
# resolves the newest release that actually publishes this platform's binary
# (walking releases and reading each release's release-info.json, so a partial
# release and a binary needing a newer glibc are both skipped), checks the
# installed version, downloads the binary itself, and then hands the local path
# over to the platform installer, which no longer talks to GitHub at all.
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

export REPO="tim-projects/l337-audio-server"
export SCRIPT_TAG

fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }

# --- Argument parsing ---
# Flags owned by this script are consumed here and NOT forwarded: the platform
# installers no longer resolve releases or compare versions.
INSTALL_PRERELEASE=false
UNINSTALL_REQUESTED=false
FORCE=false
ORIGINAL_ARGS=("$@")
FORWARD_ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        --force|-f)                 FORCE=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--pre-release] [--force] [--no-install-deps] [--dry-run] ..."
            echo ""
            echo "Resolves the newest GitHub release that publishes a binary for this"
            echo "platform, downloads it, and runs the platform installer on it."
            echo ""
            echo "Options:"
            echo "  --pre-release       Install the newest prerelease instead of the newest stable release"
            echo "  --force, -f         Reinstall even if the installed version already matches"
            echo "  --no-install-deps   Do not auto-install missing system packages (Linux only)"
            echo ""
            echo "install.sh owns the release lookup and the binary download, so"
            echo "--pre-release and --force are not forwarded to the platform installer."
            echo "All other options (--dry-run, --group, --no-audio, --user, --prefix,"
            echo "--uninstall, --remove-data, ...) are forwarded unchanged."
            exit 0 ;;
        *) FORWARD_ARGS+=("$1"); shift ;;
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

# --- Which installer, and which binary asset, this platform installs ---
# The asset must match what the platform installer actually installs, not what
# the architecture would suggest: install-macos.sh runs the x86_64 build under
# Rosetta 2 on Apple Silicon, so aarch64 must resolve the x86_64 asset.
if [ "$OS_VARIANT" = "termux" ]; then
    SCRIPT_NAME="install-android-termux"
    BINARY_ASSET="l337-audio-server-aarch64-android-termux"
elif [ "$OS_TYPE" = "linux" ]; then
    if [ "$ARCH_TYPE" = "armv7" ] || [ "$ARCH_TYPE" = "riscv64" ]; then
        SCRIPT_NAME="install-linux-alsa"
        BINARY_ASSET="l337-audio-server-${ARCH_TYPE}-linux-alsa"
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
            BINARY_ASSET="l337-audio-server-${ARCH_TYPE}-linux-pipewire"
        else
            SCRIPT_NAME="install-linux-alsa"
            BINARY_ASSET="l337-audio-server-${ARCH_TYPE}-linux-alsa"
        fi
    fi
elif [ "$OS_TYPE" = "macos" ]; then
    # install-macos.sh always installs the x86_64 build (Rosetta 2 on aarch64).
    SCRIPT_NAME="install-macos"
    BINARY_ASSET="l337-audio-server-x86_64-apple-darwin"
else
    fail "Unsupported OS: $OS"
fi

# --- Version helpers (CalVer-style: date-N, e.g. 2026.10.29-7) ---
version_gt() {
    local a="$1" b="$2"
    if [ -z "$a" ] || [ -z "$b" ]; then return 1; fi
    local a_date="${a%-*}" b_date="${b%-*}"
    if [ "$a_date" != "$b_date" ]; then
        [ "$a_date" \> "$b_date" ]; return $?
    fi
    local a_build="${a##*-}" b_build="${b##*-}"
    if [ "$a_build" = "$a" ] || [ "$b_build" = "$b" ]; then return 1; fi
    [ "$a_build" -gt "$b_build" ] 2>/dev/null
}

# Highest glibc symbol version the running libc exports, i.e. the oldest glibc
# a binary may have been built on to still run here. Empty when undetectable
# (non-glibc platforms, where it is never consulted).
system_glibc_version() {
    local v="" loader prefix
    # 1. Native glibc systems (Debian, Fedora, ...)
    if command -v ldd >/dev/null 2>&1; then
        v=$(ldd --version 2>/dev/null | head -1 \
            | sed -n 's/.*[[:space:]]\([0-9][0-9]*\.[0-9][0-9.]*\)[[:space:]]*$/\1/p')
    fi
    if [ -z "$v" ] && command -v getconf >/dev/null 2>&1; then
        v=$(getconf GNU_LIBC_VERSION 2>/dev/null \
            | sed -n 's/.*glibc[[:space:]]*\([0-9][0-9]*\.[0-9][0-9.]*\).*/\1/p')
    fi
    # 2. Termux with the termux-glibc package. The system `ldd`/`getconf` are
    #    Bionic and report nothing here, so glibc would look absent and the
    #    glibc filter would silently accept any release. The glibc dynamic
    #    loader shipped in $PREFIX/glibc answers for itself.
    prefix="${PREFIX:-}"
    if [ -z "$v" ] && [ -n "$prefix" ] && [ -d "$prefix/glibc/lib" ]; then
        for loader in "$prefix"/glibc/lib/ld-linux-*.so.1; do
            [ -f "$loader" ] || continue
            v=$("$loader" --version 2>/dev/null | head -1 \
                | sed -n 's/.*version[[:space:]]\([0-9][0-9.]*\).*/\1/p')
            [ -n "$v" ] && break
        done
    fi
    printf '%s' "$v"
}

# version_lt: true when $1 sorts before $2. Equal versions are not "less than".
version_lt() {
    if [ "$1" = "$2" ]; then return 1; fi
    [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ]
}

SYSTEM_GLIBC=""
if [ "$OS_TYPE" = "linux" ]; then
    SYSTEM_GLIBC=$(system_glibc_version)
    if [ -n "$SYSTEM_GLIBC" ]; then
        if [ "$OS_VARIANT" = "termux" ]; then
            info "System glibc: $SYSTEM_GLIBC (termux-glibc; releases record the glibc each binary needs in release-info.json)"
        else
            info "System glibc: $SYSTEM_GLIBC (releases record the glibc each binary needs in release-info.json)"
        fi
    elif [ "$OS_VARIANT" = "termux" ]; then
        # Bionic only (no termux-glibc package): the Android build is statically
        # linked against Bionic, so there is no glibc to compare against.
        info "No glibc present (Bionic-only Termux); the Android build does not use glibc."
    else
        warn "Could not determine the system glibc version; picking the newest release with any glibc requirement."
    fi
fi

# --- GitHub release lookup, shared by the install path and the uninstall path ---
# Flatten the releases API payload into one line per published asset:
#   <tag> <prerelease:0|1> <asset filename> <browser download url>
# Every filename comes from the asset's own "name" field and every URL from its
# own "browser_download_url", so no URL is ever assembled from a tag here.
release_rows() {
    awk '
        /"tag_name"[[:space:]]*:/ {
            t=$0; sub(/.*"tag_name"[[:space:]]*:[[:space:]]*"/,"",t); sub(/".*/,"",t); tag=t; next
        }
        /"prerelease"[[:space:]]*:/ { pre=($0 ~ /: *true/) ? 1 : 0; next }
        /"name"[[:space:]]*:/ {
            n=$0; sub(/.*"name"[[:space:]]*:[[:space:]]*"/,"",n); sub(/".*/,"",n); name=n; next
        }
        /"browser_download_url"[[:space:]]*:/ {
            u=$0; sub(/.*"browser_download_url"[[:space:]]*:[[:space:]]*"/,"",u); sub(/".*/,"",u)
            printf "%s\t%d\t%s\t%s\n", tag, pre, name, u
            next
        }
    ' "$1"
}

fetch_release_rows() {
    local response http_code message

    response=$(mktemp "${TMPDIR:-/tmp}/l337-release.XXXXXX") || \
        fail "Could not create a temporary file for GitHub release data."

    # curl exits non-zero on a connection error; a 4xx/5xx body still exits 0, so
    # the status is read from -w rather than the exit code.
    http_code=$(curl -sS -L --connect-timeout 15 --max-time 60 \
        -o "$response" -w '%{http_code}' \
        "https://api.github.com/repos/${REPO}/releases?per_page=100") || {
        rm -f "$response"
        fail "Failed to contact GitHub Releases API. Check network connectivity or proxy settings."
    }

    if [ "$http_code" != "200" ]; then
        message=$(grep -o '"message"[[:space:]]*:[[:space:]]*"[^"]*"' "$response" \
            | head -1 | sed 's/.*"\([^"]*\)"$/\1/' || true)
        rm -f "$response"
        fail "GitHub returned HTTP ${http_code} while listing releases. ${message:-Check network connectivity and try again later.}"
    fi

    release_rows "$response"
    rm -f "$response"
}

# The glibc a release-info.json requires for asset $1, "none" when the asset
# needs no particular glibc. Returns 1 when the asset is not described in it
# (release predates the manifest, or the asset is unknown): an unverifiable
# binary is never installed.
release_info_glibc() {
    local info_file="$1" asset="$2" line value

    [ -f "$info_file" ] || return 1
    # The trailing colon anchors the key so a longer asset name that merely
    # starts with this one can never be mistaken for it.
    line=$(grep -F "\"$asset\":" "$info_file" 2>/dev/null | head -1) || true
    [ -z "$line" ] && return 1

    case "$line" in
        *null*) echo "none"; return 0 ;;
    esac

    value=$(printf '%s' "$line" \
        | sed -n 's/.*:[[:space:]]*"\{0,1\}\([0-9][0-9.]*\)"\{0,1\}.*/\1/p')
    [ -z "$value" ] && return 1
    printf '%s' "$value"
}

# One field ("md5", "crc32", "size") of a release-info.json "checksums" entry
# for asset $2. Returns 1 when the manifest has no checksums for it (an older
# release): verification is then skipped, not failed.
release_info_checksum() {
    local info_file="$1" asset="$2" key="$3" section block

    [ -f "$info_file" ] || return 1
    # Restrict to the "checksums" object: the same asset key also appears under
    # "assets" and "needed_shared_libs", and the first match in the file would
    # be the wrong one.
    section=$(sed -n '/"checksums"[[:space:]]*:[[:space:]]*{/,/^  }/p' "$info_file" 2>/dev/null) || true
    [ -z "$section" ] && return 1
    block=$(printf '%s\n' "$section" \
        | sed -n "/\"$asset\"[[:space:]]*:[[:space:]]*{/,/^    }/p")
    [ -z "$block" ] && return 1

    printf '%s\n' "$block" | grep -o "\"$key\"[[:space:]]*:[[:space:]]*[^,]*" | head -1 \
        | sed 's/.*:[[:space:]]*//; s/^"//; s/,$//; s/"$//'
}

# Walk releases newest-first and resolve the newest one that is installable
# here: it must publish ${BINARY_ASSET}, and on glibc platforms its own
# release-info.json must record a glibc the running libc can satisfy. A release
# without our asset, one whose manifest does not list it, and one needing a
# newer glibc are all skipped instead of producing a 404 or a binary that
# cannot start. Sets TAG and DOWNLOAD_URL; returns 1 when nothing fits.
resolve_installable_release() {
    local rows tag pre name url info_url info_file req

    rows=$(fetch_release_rows)

    TAG=""
    DOWNLOAD_URL=""
    SELECTED_MD5=""
    SELECTED_CRC32=""
    SELECTED_SIZE=""

    while IFS=$'\t' read -r tag pre name url; do
        # --pre-release walks prereleases newest-first; a plain install walks
        # the stable releases newest-first.
        if [ "$INSTALL_PRERELEASE" = true ]; then
            [ "$pre" = "1" ] || continue
        else
            [ "$pre" = "0" ] || continue
        fi
        [ "$name" = "$BINARY_ASSET" ] || continue

        # The manifest shipped in the release is the authoritative record of
        # which assets it actually contains, and of the glibc each needs.
        info_url=$(printf '%s\n' "$rows" | awk -F'\t' \
            -v want_tag="$tag" -v want_name="release-info.json" \
            '$1 == want_tag && $3 == want_name { print $4; exit }')
        [ -n "$info_url" ] || continue

        info_file=$(mktemp "${TMPDIR:-/tmp}/l337-revinfo.XXXXXX") || continue
        if ! curl -fsSL --connect-timeout 15 --max-time 30 -o "$info_file" "$info_url"; then
            rm -f "$info_file"
            continue
        fi

        req=$(release_info_glibc "$info_file" "$name") || { rm -f "$info_file"; continue; }
        # The manifest's checksums for this asset, read while the manifest is
        # still around; releases that publish none are not rejected.
        SELECTED_MD5=$(release_info_checksum "$info_file" "$name" md5 || true)
        SELECTED_CRC32=$(release_info_checksum "$info_file" "$name" crc32 || true)
        SELECTED_SIZE=$(release_info_checksum "$info_file" "$name" size || true)
        rm -f "$info_file"

        if [ "$req" != "none" ] && [ -n "$SYSTEM_GLIBC" ] \
           && version_lt "${SYSTEM_GLIBC%%-*}" "${req%%-*}"; then
            info "Release ${tag} needs glibc ${req} (system has ${SYSTEM_GLIBC}) — skipping"
            continue
        fi

        TAG="$tag"
        DOWNLOAD_URL="$url"
        break
    done <<<"$rows"
}

# --- Uninstall: the platform installer needs no binary or release ---
if [ "$UNINSTALL_REQUESTED" = true ]; then
    SCRIPT_PATH=""
    # Only a real checkout is trusted as a local script (see the note on the
    # install path below); otherwise fetch the installer from main, which is
    # always the newest code and needs no release resolution at all.
    script_dir="$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)"
    if [ -n "$script_dir" ] && [ -f "$script_dir/scripts/${SCRIPT_NAME}.sh" ] \
       && [ -d "$script_dir/.git" ]; then
        SCRIPT_PATH="$script_dir/scripts/${SCRIPT_NAME}.sh"
    else
        SCRIPT_TAG="main"
        SCRIPT_URL="https://raw.githubusercontent.com/${REPO}/${SCRIPT_TAG}/scripts/${SCRIPT_NAME}.sh"
        SCRIPT_PATH=$(mktemp "${TMPDIR:-/tmp}/l337-install.XXXXXX") || \
            fail "Could not create a temporary file for the installer script."
        if ! curl -fsSL --connect-timeout 15 --max-time 60 -o "$SCRIPT_PATH" "$SCRIPT_URL"; then
            rm -f "$SCRIPT_PATH"
            fail "Failed to download uninstall script from $SCRIPT_URL"
        fi
        chmod +x "$SCRIPT_PATH"
    fi
    info "Uninstall requested — using installer script: ${SCRIPT_PATH}"
    exec bash "$SCRIPT_PATH" "${ORIGINAL_ARGS[@]}"
fi

# --- Resolve the release to install ---
if ! resolve_installable_release; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        fail "No prerelease for ${REPO} publishes a ${BINARY_ASSET} that runs on this system. Check https://github.com/${REPO}/releases."
    fi
    fail "No stable release for ${REPO} publishes a ${BINARY_ASSET} that runs on this system. Re-run with --pre-release."
fi

if [ -z "$TAG" ]; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        fail "No prerelease for ${REPO} publishes ${BINARY_ASSET}. Check https://github.com/${REPO}/releases."
    fi
    fail "No stable release for ${REPO} publishes ${BINARY_ASSET}. Re-run with --pre-release to install a prerelease."
fi

SCRIPT_TAG="$TAG"
info "Selected release: ${TAG}"

# --- Already-installed version check ---
# If the installed binary reports the same version as the release being
# installed, stop unless --force was given: a pointless download/replace cycle
# otherwise. The platform installers no longer do this check themselves.
INSTALLED_VERSION=""
INSTALLED_BIN=""
case "$OS_VARIANT" in
    termux)
        INSTALLED_BIN="${HOME}/.local/share/l337-player/bin/l337-audio-server" ;;
    *)
        INSTALLED_BIN="/opt/l337-audio-server/l337-audio-server" ;;
esac

if [ -n "$INSTALLED_BIN" ] && [ -f "$INSTALLED_BIN" ] && [ -x "$INSTALLED_BIN" ]; then
    # The Android build has no DT_RUNPATH, so libpulse.so is only resolved via
    # LD_LIBRARY_PATH — without it the binary cannot even run --version.
    if [ "$OS_VARIANT" = "termux" ]; then
        INSTALLED_VERSION=$(LD_LIBRARY_PATH="${PREFIX:-/data/data/com.termux/files/usr}/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
            "$INSTALLED_BIN" --version 2>/dev/null | awk "{print \$NF}" || true)
    else
        INSTALLED_VERSION=$("$INSTALLED_BIN" --version 2>/dev/null | awk "{print \$NF}" || true)
    fi
    # Only a well-formed release tag can be compared: an unparseable string
    # would otherwise compare as "newer than release" above and silently block
    # every future update.
    if [ -n "$INSTALLED_VERSION" ] && \
       ! printf '%s' "$INSTALLED_VERSION" | grep -qE '^[0-9]{4}\.[0-9]{2}\.[0-9]{2}-[0-9]+$'; then
        warn "Installed version '${INSTALLED_VERSION}' is not a release tag; will reinstall"
        INSTALLED_VERSION=""
    fi
    if [ -n "$INSTALLED_VERSION" ]; then
        info "Existing installation found at: $INSTALLED_BIN"
        info "Installed version: $INSTALLED_VERSION"
        if [ "$INSTALLED_VERSION" = "$SCRIPT_TAG" ]; then
            if [ "$FORCE" = true ]; then
                info "Force reinstall requested for the same version ($INSTALLED_VERSION) — re-downloading"
            else
                ok "Installed binary is up-to-date ($INSTALLED_VERSION)"
                echo
                echo "To reinstall anyway, run with --force:"
                echo "  $0 --force$([ "$INSTALL_PRERELEASE" = true ] && echo ' --pre-release')"
                exit 0
            fi
        elif version_gt "$SCRIPT_TAG" "$INSTALLED_VERSION"; then
            info "Update available ($INSTALLED_VERSION -> $SCRIPT_TAG)"
        else
            if [ "$FORCE" = true ]; then
                info "Force reinstall requested even though installed ($INSTALLED_VERSION) is newer than release ($SCRIPT_TAG) — re-downloading"
            else
                ok "Installed binary ($INSTALLED_VERSION) is newer than latest release ($SCRIPT_TAG)"
                echo
                echo "To reinstall anyway, run with --force:"
                echo "  $0 --force$([ "$INSTALL_PRERELEASE" = true ] && echo ' --pre-release')"
                exit 0
            fi
        fi
    else
        warn "Could not determine installed version; will reinstall"
    fi
fi

# --- Download the binary ---
# A partial download, or an asset the release no longer serves, must not be
# mistaken for a good binary: the transfer status decides success, and any
# previous file at the destination is removed first so a stale one can never be
# installed by accident.
BINARY_FILE="${TMPDIR:-/tmp}/l337-audio-server-download"
rm -f "$BINARY_FILE"

info "Downloading ${BINARY_ASSET} (release ${TAG})..."
HTTP_CODE=$(curl -fL --connect-timeout 15 --max-time 300 \
    -o "$BINARY_FILE" -w '%{http_code}' "$DOWNLOAD_URL" 2>/dev/null) \
    || HTTP_CODE="${HTTP_CODE:-000}"

case "$HTTP_CODE" in
    2??)
        [ -s "$BINARY_FILE" ] || { rm -f "$BINARY_FILE"; fail "Downloaded binary from ${TAG} is empty."; }
        ;;
    *)  rm -f "$BINARY_FILE"
        fail "Failed to download ${BINARY_ASSET} from ${DOWNLOAD_URL} (HTTP ${HTTP_CODE:-no response}).
The release was verified to publish this asset moments ago; if this repeats the
asset may have been removed. Re-run the installer." ;;
esac

# Verify the download before anything is installed.
#
# Layered on purpose, because the verification tools are not equally portable:
#   1. SIZE   — needs no tool at all (curl reports what it transferred), so a
#               truncated transfer is caught everywhere.
#   2. MD5    — strongest, via md5sum (coreutils).
#   3. CRC32  — POSIX `cksum`, which both coreutils and busybox ship, so it
#               covers a host without md5sum.
# A missing hash tool is never a hard failure: the size check plus the install
# verification the platform installer already does are the fallback, and the
# user is told precisely which check was applied.
# (A pure-bash hash is not an option: bash cannot hold NUL bytes, and a per-byte
# loop in bash measures ~4 KB/s, i.e. about an hour for a 15 MB binary.)
DOWNLOAD_SIZE=$(wc -c < "$BINARY_FILE" 2>/dev/null | tr -d '[:space:]')
VERIFY_METHOD=""
if [ -n "$SELECTED_SIZE" ] && [ -n "$DOWNLOAD_SIZE" ]; then
    if [ "$DOWNLOAD_SIZE" != "$SELECTED_SIZE" ]; then
        rm -f "$BINARY_FILE"
        fail "Downloaded ${BINARY_ASSET} is ${DOWNLOAD_SIZE} bytes but release ${TAG} publishes ${SELECTED_SIZE}.
The download was truncated; nothing was installed. Re-run."
    fi
    VERIFY_METHOD="size"
    ok "Verified ${BINARY_ASSET} (size ${DOWNLOAD_SIZE} bytes)"
fi

if [ -n "$SELECTED_MD5" ] && command -v md5sum >/dev/null 2>&1; then
    DOWNLOAD_MD5=$(md5sum "$BINARY_FILE" | awk '{print $1}')
    if [ -n "$DOWNLOAD_MD5" ] && [ "$DOWNLOAD_MD5" != "$SELECTED_MD5" ]; then
        rm -f "$BINARY_FILE"
        fail "Downloaded ${BINARY_ASSET} does not match the published checksum.
  expected md5: ${SELECTED_MD5}
  actual md5:   ${DOWNLOAD_MD5}
The download was corrupted; nothing was installed. Re-run."
    fi
    VERIFY_METHOD="size+md5"
    ok "Verified ${BINARY_ASSET} (md5 ${DOWNLOAD_MD5})"
elif [ -n "$SELECTED_CRC32" ] && command -v cksum >/dev/null 2>&1; then
    # cksum prints "<crc32> <bytes> <filename>", so it also re-checks the length.
    CKSUM_OUT=$(cksum "$BINARY_FILE" 2>/dev/null) || CKSUM_OUT=""
    read -r CK_CRC CK_LEN _ <<<"$CKSUM_OUT"
    if [ -n "$CK_CRC" ] && [ "$CK_CRC" != "$SELECTED_CRC32" ]; then
        rm -f "$BINARY_FILE"
        fail "Downloaded ${BINARY_ASSET} does not match the published checksum.
  expected crc32: ${SELECTED_CRC32}
  actual crc32:   ${CK_CRC}
The download was corrupted; nothing was installed. Re-run."
    fi
    if [ -n "$SELECTED_SIZE" ] && [ -n "$CK_LEN" ] && [ "$CK_LEN" != "$SELECTED_SIZE" ]; then
        rm -f "$BINARY_FILE"
        fail "Downloaded ${BINARY_ASSET} is ${CK_LEN} bytes but release ${TAG} publishes ${SELECTED_SIZE}.
The download was truncated; nothing was installed. Re-run."
    fi
    VERIFY_METHOD="size+crc32"
    ok "Verified ${BINARY_ASSET} (crc32 ${CK_CRC})"
elif [ -n "$SELECTED_MD5" ]; then
    # The manifest has a checksum but no tool on this host can compute it.
    info "No md5sum/cksum on PATH; verified ${BINARY_ASSET} by size only (${DOWNLOAD_SIZE:-?} bytes)"
else
    # A release published before checksums were recorded: nothing to check against.
    info "Release ${TAG} publishes no checksum for ${BINARY_ASSET}; nothing to verify against"
fi

ok "Downloaded ${BINARY_ASSET} (release ${TAG}) -> ${BINARY_FILE}"

# --- Locate the platform installer script ---
# A local copy is only used when it sits in a real repository checkout. The bare
# CWD-relative path is deliberately rejected: the documented
# `curl ... install.sh | sudo bash` feeds the script over stdin, so $0 is "bash"
# and a CWD-relative path would execute whatever file happens to be there as
# root; the HTTPS download is the only trusted source in that case.
SCRIPT_PATH=""
script_dir="$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)"
if [ -n "$script_dir" ] && [ -f "$script_dir/scripts/${SCRIPT_NAME}.sh" ] \
   && [ -d "$script_dir/.git" ]; then
    SCRIPT_PATH="$script_dir/scripts/${SCRIPT_NAME}.sh"
    SCRIPT_TAG="$TAG"
    info "Using local installer: ${SCRIPT_PATH}"
else
    SCRIPT_URL="https://raw.githubusercontent.com/${REPO}/main/scripts/${SCRIPT_NAME}.sh"
    SCRIPT_PATH=$(mktemp "${TMPDIR:-/tmp}/l337-install.XXXXXX") || \
        fail "Could not create a temporary file for the installer script."
    if ! curl -fsSL --connect-timeout 15 --max-time 60 -o "$SCRIPT_PATH" "$SCRIPT_URL"; then
        rm -f "$SCRIPT_PATH"
        fail "Failed to download installer script from $SCRIPT_URL"
    fi
    chmod +x "$SCRIPT_PATH"
    info "Using installer from ${SCRIPT_URL}"
fi

# --- Execute ---
# The installer gets the resolved release tag and the already-downloaded binary
# path, so it does no GitHub work of its own. Exec through bash rather than
# relying on the script's shebang: /bin/bash does not exist on Termux (where
# /bin is an empty /system/bin symlink).
info "Handing ${BINARY_FILE} to ${SCRIPT_NAME}.sh"
exec bash "$SCRIPT_PATH" --binary "$BINARY_FILE" --release "$TAG" "${FORWARD_ARGS[@]+"${FORWARD_ARGS[@]}"}"
