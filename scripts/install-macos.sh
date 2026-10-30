#!/bin/bash
# install-macos.sh — macOS launchd installer
#
# Downloads l337-audio-server-{arch}-apple-darwin from GitHub releases and
# installs it as a launchd user service.
#
# Every release ships a release-info.json beside its binaries listing the assets
# that release actually contains. Releases are walked newest-first and the newest
# one whose release-info.json confirms this platform's binary is in it is
# installed, so a partial release (e.g. a targeted CI build that publishes only
# some targets) is skipped instead of downloading into a 404.
#
# Usage:
#   ./scripts/install-macos.sh
#   ./scripts/install-macos.sh --pre-release
#   ./scripts/install-macos.sh --uninstall
#   ./scripts/install-macos.sh --uninstall --remove-data
#   ./scripts/install-macos.sh --dry-run
#   ./scripts/install-macos.sh --force
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
REPO="tim-projects/l337-audio-server"
INSTALL_DIR="/opt/l337-audio-server"
PLIST_LABEL="com.l337.audio-server"
REAL_USER="${SUDO_USER:-${USER}}"

# ---------------------------------------------------------------------------
# Flags
# ---------------------------------------------------------------------------
DRY_RUN=false
UNINSTALL=false
REMOVE_DATA=false
INSTALL_PRERELEASE=false
FORCE=false
NO_AUDIO=false

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }

check_command() {
    if command -v "$1" &>/dev/null; then
        return 0
    else
        fail "Required command not found: $1"
    fi
}

check_dependencies() {
    check_command "curl"
    check_command "launchctl"
    check_command "file"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        --force|-f) FORCE=true; shift ;;
        --no-audio) NO_AUDIO=true; shift ;;
        --uninstall|-u) UNINSTALL=true; shift ;;
        --remove-data) REMOVE_DATA=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [OPTIONS]

macOS installer for L337 Audio Server.
Downloads prebuilt binary from GitHub releases and installs as launchd user service.

Options:
  --dry-run              Show what would happen without making changes
  --pre-release, --prerelease
                         Install the latest prerelease instead of stable release
  --force, -f            Force reinstall even if the same version is already installed
  --no-audio             Set dummy = true in server.ini (run without audio hardware)
  --uninstall, -u        Remove the service and installed files
  --remove-data          Also remove configuration and data directories
  -h, --help             Show this help message
EOF
            exit 0 ;;
        *) fail "Unknown option: $1" ;;
    esac
done

# ---------------------------------------------------------------------------
# Platform detection
# ---------------------------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin*)    OS_TYPE="macos" ;;
    *)          fail "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64)     ARCH_TYPE="x86_64" ;;
    aarch64)    ARCH_TYPE="aarch64" ;;
    *)          fail "Unsupported architecture: $ARCH" ;;
esac

info "Platform: $OS_TYPE / $ARCH_TYPE"

# ---------------------------------------------------------------------------
# GitHub release helpers
# ---------------------------------------------------------------------------
# Flatten the release list into one line per published asset:
#   <tag> <prerelease:0|1> <asset name> <published_at> <browser download url>
# Every name comes from the API's own "name" field, never from a URL this script
# builds, so only an asset GitHub actually publishes in that release can be
# selected.
release_asset_rows() {
    awk '
        /"tag_name"[[:space:]]*:/ {
            t=$0; sub(/.*"tag_name"[[:space:]]*:[[:space:]]*"/,"",t); sub(/".*/,"",t); tag=t; next
        }
        /"prerelease"[[:space:]]*:/ { pre=($0 ~ /: *true/) ? 1 : 0; next }
        /"published_at"[[:space:]]*:/ {
            p=$0; sub(/.*"published_at"[[:space:]]*:[[:space:]]*"/,"",p); sub(/".*/,"",p); pub=p; next
        }
        /"name"[[:space:]]*:/ {
            n=$0; sub(/.*"name"[[:space:]]*:[[:space:]]*"/,"",n); sub(/".*/,"",n); name=n; next
        }
        /"browser_download_url"[[:space:]]*:/ {
            u=$0; sub(/.*"browser_download_url"[[:space:]]*:[[:space:]]*"/,"",u); sub(/".*/,"",u)
            printf "%s\t%d\t%s\t%s\t%s\n", tag, pre, name, pub, u
            next
        }
    ' "$1"
}

# Walk releases newest-first and select the newest one that publishes $1 AND
# confirms it in that release's own release-info.json (generated from the
# artifacts that actually downloaded, so its asset map is authoritative). A
# partial release — e.g. a targeted CI build that publishes only some targets —
# is skipped instead of being downloaded into a 404. Sets SELECTED_TAG,
# SELECTED_PUBLISHED and SELECTED_URL; returns 1 when no release qualifies.
# shortcut: only the newest 100 releases are searched (the API page size; this
# script does not follow pagination).
select_release_with_asset() {
    local want_name="$1"
    local response rows r_tag r_pre r_name r_pub r_url
    local r_info_url r_info_file info_dir

    response=$(mktemp "${TMPDIR:-/tmp}/l337-release.XXXXXX") || \
        fail "Could not create a temporary file for GitHub release data."
    http_code=$(curl -sS -L --connect-timeout 15 --max-time 60 \
        -o "$response" -w '%{http_code}' \
        "https://api.github.com/repos/${REPO}/releases?per_page=100") || {
        rm -f "$response"
        fail "Failed to contact GitHub Releases API. Check network connectivity or proxy settings."
    }
    # curl exits 0 on a 403/5xx body (rate limits, outages), and an error body
    # flattens to zero asset rows — which would be misreported below as "no
    # release publishes our asset". Surface the API's own message instead.
    if [ "$http_code" != "200" ]; then
        message=$(grep -o '"message"[[:space:]]*:[[:space:]]*"[^"]*"' "$response" \
            | head -1 | sed 's/.*"\([^"]*\)"$/\1/' || true)
        rm -f "$response"
        fail "GitHub returned HTTP ${http_code} while listing releases. ${message:-Check network connectivity or proxy settings.}"
    fi
    rows=$(release_asset_rows "$response")
    rm -f "$response"

    SELECTED_TAG=""
    SELECTED_PUBLISHED=""
    SELECTED_URL=""
    NEWEST_TAG=""

    # tag -> release-info.json URL, built in one pass so the walk below never
    # has to re-scan every row (and spawn an awk) per candidate release.
    declare -A INFO_URL=()
    while IFS=$'\t' read -r r_tag r_url; do
        [ -n "$r_tag" ] && INFO_URL["$r_tag"]="$r_url"
    done < <(printf '%s\n' "$rows" | awk -F'\t' \
        '$3 == "release-info.json" { print $1 "\t" $5 }')

    while IFS=$'\t' read -r r_tag r_pre r_name r_pub r_url; do
        # --pre-release walks prereleases; a plain install walks the stable
        # releases. Both stop at the newest one that publishes our asset.
        if [ "$INSTALL_PRERELEASE" = true ]; then
            [ "$r_pre" = "1" ] || continue
        else
            [ "$r_pre" = "0" ] || continue
        fi
        # Newest release this mode would use if it published every asset, so the
        # caller can say why an older one is installed instead.
        [ -z "$NEWEST_TAG" ] && NEWEST_TAG="$r_tag"
        [ "$r_name" = "$want_name" ] || continue

        r_info_url="${INFO_URL[$r_tag]:-}"
        if [ -z "$r_info_url" ]; then
            continue
        fi
        r_info_file=$(mktemp "${TMPDIR:-/tmp}/l337-revinfo.XXXXXX") || return 1
        # -f: a 403/404/5xx body would otherwise be saved as the "manifest" and
        # read below as "does not list the asset" — a silent skip that walks the
        # install back to an older release. A network/API failure must fail loudly.
        if ! curl -fsSL --connect-timeout 15 --max-time 30 \
                -o "$r_info_file" "$r_info_url"; then
            rm -f "$r_info_file"
            fail "Could not download release-info.json for ${r_tag} from ${r_info_url} — the release cannot be verified. Check network connectivity or proxy settings."
        fi
        # The trailing colon anchors the key so a longer asset name that merely
        # starts with this one can never be mistaken for it.
        if ! grep -qF "\"$want_name\":" "$r_info_file"; then
            rm -f "$r_info_file"
            continue
        fi
        rm -f "$r_info_file"

        SELECTED_TAG="$r_tag"
        SELECTED_PUBLISHED="$r_pub"
        SELECTED_URL="$r_url"
        break
    done <<<"$rows"

    [ -n "$SELECTED_TAG" ]
}

get_asset_name() {
    local os="$1"
    local arch="$2"
    case "$os-$arch" in
        macos-x86_64)
            echo "l337-audio-server-x86_64-apple-darwin"
            ;;
        macos-aarch64)
            warn "Apple Silicon Mac detected. Downloading x86_64 build (runs under Rosetta 2)."
            echo "l337-audio-server-x86_64-apple-darwin"
            ;;
        *)
            fail "Unsupported platform: $os/$arch"
            ;;
    esac
}

# ---------------------------------------------------------------------------
# Download
# ---------------------------------------------------------------------------
download_binary() {
    local url="$1"
    local dest="$2"
    local tmp="${dest}.tmp.$$"

    info "Downloading: $url"
    if ! curl -fL --connect-timeout 15 --retry 2 --retry-delay 2 -o "$tmp" "$url"; then
        rm -f "$tmp"
        fail "Failed to download binary from $url"
    fi

    if [ ! -s "$tmp" ]; then
        rm -f "$tmp"
        fail "Downloaded file is empty. Check the release URL."
    fi

    if ! file "$tmp" | grep -qE "ELF|Mach-O"; then
        rm -f "$tmp"
        fail "Downloaded file is not a valid binary (got: $(file -b "$tmp")). Check the release URL."
    fi

    chmod +x "$tmp"
    mv "$tmp" "$dest"
    local size
    size=$(du -h "$dest" | cut -f1)
    ok "Downloaded: $dest ($size)"
}

# ---------------------------------------------------------------------------
# macOS: launchd setup
# ---------------------------------------------------------------------------
setup_launchd() {
    local bin_path="$1"

    info "Configuring launchd service..."

    local real_home
    real_home=$(eval echo "~${REAL_USER}")
    local config_dir
    config_dir="$real_home/Library/Application Support/l337-audio-server"
    mkdir -p "$config_dir"

    local config_file="$config_dir/server.ini"
    local legacy_config_file="$config_dir/config.toml"
    if [ ! -f "$legacy_config_file" ] && [ -f "$real_home/.config/l337-audio-server/config.toml" ]; then
        legacy_config_file="$real_home/.config/l337-audio-server/config.toml"
    fi
    if [ ! -f "$config_file" ]; then
        info "Creating default configuration..."
        local token
        token=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32 || true)
        if [ "${#token}" -ne 32 ]; then
            fail "Failed to generate a secure server token."
        fi
        if [ -f "$legacy_config_file" ]; then
            info "Migrating legacy configuration: $legacy_config_file"
            cp "$legacy_config_file" "$config_file" || \
                fail "Failed to migrate legacy configuration to $config_file."
        else
            local dummy_value="false"
            if [ "$NO_AUDIO" = "true" ]; then
                dummy_value="true"
            fi
            cat > "$config_file" <<EOF
[server]
host = "0.0.0.0"
port = 1337
token = "${token}"
dummy = ${dummy_value}
transport = "auto"
EOF
            ok "Configuration written to $config_file"
            echo
            echo "========================================="
            echo " Server Token"
            echo "========================================="
            echo
            echo "  ${token}"
            echo
            echo "Add this token to your client configuration."
            echo "========================================="
            echo
        fi
    elif [ "$NO_AUDIO" = "true" ]; then
        if grep -qE '^dummy\s*=' "$config_file" 2>/dev/null; then
            sed -i 's/^dummy\s*=.*/dummy = true/' "$config_file"
        else
            sed -i '/^\[server\]/a dummy = true' "$config_file"
        fi
    fi

    chown "$REAL_USER" "$config_file" || \
        fail "Failed to set ownership on $config_file."
    chmod 0600 "$config_file"

    local plist_dir
    plist_dir=$(eval echo "~${REAL_USER}/Library/LaunchAgents")
    mkdir -p "$plist_dir"
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"
    local log_dir
    log_dir=$(eval echo "~${REAL_USER}/Library/Logs")
    mkdir -p "$log_dir"
    local log_path="${log_dir}/${PLIST_LABEL}.log"

    info "Writing launchd plist: $plist_path"
    cat > "$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${PLIST_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${INSTALL_DIR}/l337-audio-server</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>${real_home}</string>
        <key>L337__SERVER__TOKEN</key>
        <string>${token}</string>
        <key>XDG_CONFIG_HOME</key>
        <string>${real_home}/.config</string>
        <key>XDG_CACHE_HOME</key>
        <string>${real_home}/.cache</string>
        <key>XDG_STATE_HOME</key>
        <string>${real_home}/.local/state</string>
    </dict>
    <key>WorkingDirectory</key>
    <string>${real_home}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${log_path}</string>
    <key>StandardErrorPath</key>
    <string>${log_path}</string>
</dict>
</plist>
EOF

    if launchctl list | grep -q "$PLIST_LABEL"; then
        info "Stopping existing service..."
        launchctl unload "$plist_path" 2>/dev/null || true
    fi

    info "Replacing binary..."
    if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
        mv "$INSTALL_DIR/l337-audio-server" "$INSTALL_DIR/l337-audio-server.bak"
    fi
    mv "$bin_path" "$INSTALL_DIR/l337-audio-server"
    chmod 0755 "$INSTALL_DIR/l337-audio-server"

    launchctl load "$plist_path" || true
    if launchctl list | grep -q "$PLIST_LABEL"; then
        ok "Launchd service installed and started"
        rm -f "$INSTALL_DIR/l337-audio-server.bak"
        rm -rf "$INSTALL_DIR/.tmp"
        return 0
    fi

    warn "New binary failed to load. Rolling back..."
    if launchctl list | grep -q "$PLIST_LABEL"; then
        launchctl unload "$plist_path" 2>/dev/null || true
    fi
    if [ -f "$INSTALL_DIR/l337-audio-server.bak" ]; then
        if mv "$INSTALL_DIR/l337-audio-server.bak" "$INSTALL_DIR/l337-audio-server" 2>/dev/null; then
            chmod 0755 "$INSTALL_DIR/l337-audio-server"
            launchctl load "$plist_path" || true
            if launchctl list | grep -q "$PLIST_LABEL"; then
                warn "Rollback successful — old binary restored"
                rm -f "$INSTALL_DIR/l337-audio-server.bak"
                rm -rf "$INSTALL_DIR/.tmp"
                return 0
            fi
        fi
        fail "Rollback failed — manual recovery required"
    fi
    fail "Installation aborted: new binary failed validation"
}

# ---------------------------------------------------------------------------
# Version helpers
# ---------------------------------------------------------------------------
get_installed_version() {
    if [ -x "$INSTALL_DIR/l337-audio-server" ]; then
        "$INSTALL_DIR/l337-audio-server" --version 2>/dev/null | awk '{print $NF}'
    else
        echo ""
    fi
}

version_gt() {
    local a="$1"
    local b="$2"

    if [ -z "$a" ] || [ -z "$b" ]; then
        return 1
    fi

    local a_date="${a%-*}"
    local a_build="${a##*-}"
    local b_date="${b%-*}"
    local b_build="${b##*-}"

    if [ "$a_date" != "$b_date" ]; then
        [ "$a_date" \> "$b_date" ]
        return $?
    fi

    if [ "$a_build" = "$a" ] || [ "$b_build" = "$b" ]; then
        return 1
    fi

    [ "$a_build" -gt "$b_build" ] 2>/dev/null
}

version_eq() {
    [ "$1" = "$2" ]
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------
uninstall_macos() {
    info "Uninstalling from macOS..."

    local real_home
    real_home=$(eval echo "~${REAL_USER}")
    local plist_dir
    plist_dir=$(eval echo "~${REAL_USER}/Library/LaunchAgents")
    local plist_path="${plist_dir}/${PLIST_LABEL}.plist"
    local binary_path="${INSTALL_DIR}/l337-audio-server"

    if launchctl list | grep -q "$PLIST_LABEL"; then
        launchctl unload "$plist_path" 2>/dev/null || true
    fi

    # launchd only manages the agent. A foreground instance is invisible to it,
    # keeps the socket and the instance lock alive, and makes the next install
    # fail to bind — so refuse instead of half-removing.
    if pgrep -f "$binary_path" >/dev/null 2>&1; then
        local pids
        pids="$(pgrep -f "$binary_path" | tr '\n' ' ' || true)"
        fail "A manually started l337-audio-server is still running (pid: ${pids}).
Stop it and re-run:  kill ${pids}"
    fi

    rm -f "$plist_path"
    rm -rf "$INSTALL_DIR"

    # Runtime, not user data: the socket and the instance lock. The plist sets
    # XDG_CACHE_HOME=~/.cache, so the service's runtime lives there; a server
    # started by hand falls back to the macOS cache dir the same way the server
    # does (src/platform/common.rs). Only the ephemeral auth challenge token
    # shares this directory — playlists, the token file and logs do not.
    local runtime_dir
    for runtime_dir in "${real_home}/.cache/l337-audio-server" \
                       "${real_home}/Library/Caches/l337-audio-server"; do
        if [ -d "$runtime_dir" ]; then
            rm -rf "$runtime_dir"
            ok "Runtime removed: $runtime_dir"
        fi
    done

    # A download that fell back to /tmp leaves the whole binary there.
    rm -f "/tmp/$(get_asset_name "$OS_TYPE" "$ARCH_TYPE").tmp"

    if [ "$REMOVE_DATA" = true ]; then
        info "Removing user data directories..."
        rm -rf "${real_home}/Library/Application Support/l337-audio-server"
        rm -f "${real_home}/Library/Logs/${PLIST_LABEL}.log"
        ok "User data directories removed"
    else
        warn "User data directories retained:"
        warn "  ${real_home}/Library/Application Support/l337-audio-server"
        warn "  ${real_home}/Library/Logs/${PLIST_LABEL}.log"
        warn "Re-run with --remove-data to delete them."
    fi

    ok "Uninstallation complete"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    info "Dry-run mode — would perform the following actions:"
    if [ "$UNINSTALL" = true ]; then
        info "  Remove the service and installed files"
        info "  Remove the runtime directories (socket and instance lock)"
        info "  Refuse if a manually started instance is still running"
        if [ "$REMOVE_DATA" = true ]; then
            info "  Remove user data directories"
        else
            info "  Retain user data directories"
        fi
    else
        info "  Query GitHub for latest release"
        info "  Download the latest binary for $OS_TYPE/$ARCH_TYPE"
        info "  Install to: $INSTALL_DIR/l337-audio-server"
        info "  Configure launchd plist (user-level service, no --user flag needed)"
        info "  Config: ~/Library/Application Support/l337-audio-server/server.ini"
        info "  Logs:   ~/Library/Logs/${PLIST_LABEL}.log"
    fi
    exit 0
fi

if [ "$UNINSTALL" = true ]; then
    check_command "launchctl"
    uninstall_macos
    exit 0
fi

check_dependencies

info "Checking for latest release..."
ASSET_NAME=$(get_asset_name "$OS_TYPE" "$ARCH_TYPE")
info "Selected asset: $ASSET_NAME"

if ! select_release_with_asset "$ASSET_NAME"; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        fail "No prerelease for ${REPO} publishes ${ASSET_NAME}. Check https://github.com/${REPO}/releases."
    fi
    fail "No stable release for ${REPO} publishes ${ASSET_NAME}. Re-run with --pre-release to install a prerelease."
fi
LATEST_TAG="$SELECTED_TAG"
PUBLISHED_AT="$SELECTED_PUBLISHED"

if [ -n "$NEWEST_TAG" ] && [ "$LATEST_TAG" != "$NEWEST_TAG" ]; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        info "Newest prerelease ${NEWEST_TAG} does not publish ${ASSET_NAME}"
    else
        info "Newest stable release ${NEWEST_TAG} does not publish ${ASSET_NAME}"
    fi
    info "Installing release ${LATEST_TAG} instead, the newest one that does."
fi

info "Latest release: $LATEST_TAG (published: $PUBLISHED_AT)"

INSTALLED_VERSION=""
if [ -f "$INSTALL_DIR/l337-audio-server" ]; then
    info "Existing installation found at: $INSTALL_DIR/l337-audio-server"
    INSTALLED_VERSION=$(get_installed_version)

    if [ -n "$INSTALLED_VERSION" ]; then
        info "Installed version: $INSTALLED_VERSION"
        if version_eq "$INSTALLED_VERSION" "$LATEST_TAG"; then
            if [ "$FORCE" = true ]; then
                info "Force reinstall requested for the same version ($INSTALLED_VERSION)"
            else
                ok "Installed binary is up-to-date ($INSTALLED_VERSION)"
                echo
                echo "To reinstall anyway, run with --force:"
                echo "  $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
                exit 0
            fi
        elif version_gt "$LATEST_TAG" "$INSTALLED_VERSION"; then
            info "Update available ($INSTALLED_VERSION -> $LATEST_TAG)"
        else
            if [ "$FORCE" = true ]; then
                info "Force reinstall requested even though installed ($INSTALLED_VERSION) is newer than release ($LATEST_TAG)"
            else
                ok "Installed binary ($INSTALLED_VERSION) is newer than latest release ($LATEST_TAG)"
                echo
                echo "To reinstall anyway, run with --force:"
                echo "  $0 --force $([ "$INSTALL_PRERELEASE" = true ] && echo '--pre-release ')$([ "$DRY_RUN" = true ] && echo '--dry-run ')"
                exit 0
            fi
        fi
    else
        warn "Could not determine installed version; will reinstall"
    fi
fi

# The download URL comes from the selected release's own asset (matched by the
# API's asset name, never constructed here), so a release that publishes our
# binary can never 404 at download time.
DOWNLOAD_URL="$SELECTED_URL"
NEW_BIN=""
if mkdir -p "$INSTALL_DIR/.tmp" 2>/dev/null && [ -w "$INSTALL_DIR/.tmp" ]; then
    NEW_BIN="$INSTALL_DIR/.tmp/l337-audio-server.new"
else
    warn "Cannot write to $INSTALL_DIR/.tmp; falling back to /tmp (non-atomic copy)"
    NEW_BIN="/tmp/${ASSET_NAME}.tmp"
fi

download_binary "$DOWNLOAD_URL" "$NEW_BIN"

setup_launchd "$NEW_BIN"

rm -f "$NEW_BIN"
rm -rf "$INSTALL_DIR/.tmp"
ok "Installation complete"
echo
echo "Next steps:"
echo "  Check status:    launchctl list | grep $PLIST_LABEL"
echo "  View logs:       tail -f ~/Library/Logs/$PLIST_LABEL.log"
echo "  Configuration:   ~/Library/Application Support/l337-audio-server/server.ini"
