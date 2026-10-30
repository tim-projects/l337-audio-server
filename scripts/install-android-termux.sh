#!/bin/bash
# install-android-termux.sh — Termux binary installer (runit/termux-services, no systemd)
# Requirements: Termux + aarch64 + termux-services + running pulseaudio.
#
# Every release ships a release-info.json beside its binaries listing the assets
# that release actually contains. Releases are walked newest-first and the newest
# one whose release-info.json confirms the Termux binary is in it is installed, so
# a partial release (e.g. a targeted CI build that publishes only some targets)
# is skipped instead of downloading into a 404.
# Missing packages are installed automatically unless --no-install-deps is
# given; anything that needs a decision is reported as a numbered instruction.
#
# Usage:
#   bash scripts/install-android-termux.sh                            # install/update
#   bash scripts/install-android-termux.sh --pre-release              # install prerelease
#   bash scripts/install-android-termux.sh --no-install-deps          # never install packages
#   bash scripts/install-android-termux.sh --uninstall                # remove service + binary
#   bash scripts/install-android-termux.sh --uninstall --remove-data  # also delete data
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
# User data (never removed unless --remove-data):
#   DATA_DIR  playlists/app.db/playlists.db/config.ini (+ preserved server.ini)
#   CACHE_DIR auth token, socket symlink, server logs
DATA_DIR="${HOME}/.local/share/l337-player"
CACHE_DIR="${HOME}/.cache/l337"
RUNTIME_DIR="${TMPDIR}/l337-audio-server"

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

# Remove a runtime directory once, tolerating TMPDIR/$PREFIX/tmp aliasing
# (Termux exports TMPDIR=$PREFIX/tmp, so both usually resolve to the same path).
remove_runtime_dir() {
    local dir="$1"
    if [ -z "$dir" ] || [ "$dir" = "/" ]; then
        return 0
    fi
    if [ -e "$dir" ] || [ -L "$dir" ]; then
        rm -rf "$dir"
        ok "Runtime removed: $dir"
    fi
}

# Uninstall the Termux install. Default keeps user data (playlists, token,
# logs and the service's server.ini, which is live config: the runit service's
# CWD *is* the service dir, so ./server.ini is that file). --remove-data
# deletes it.
#
# No requirement checks gate this path on purpose: uninstalling must work even
# when pulseaudio is broken or termux-services was removed.
uninstall_termux() {
    info "Uninstalling l337-audio-server from Termux..."

    if [ ! -d "${SERVICE_DIR}" ] && [ ! -f "${BINARY_PATH}" ] && [ ! -d "${DATA_DIR}" ]; then
        warn "Nothing to uninstall (no service, binary or data found)."
    fi

    # --- Stop the service and any manually started instance ---
    # runsvdir re-spawns a service the moment its directory exists, so the stop
    # must complete (and the process be gone) before the directory is removed.
    if [ -d "${SERVICE_DIR}" ]; then
        if command -v sv >/dev/null 2>&1; then
            sv down "${SERVICE_NAME}" >/dev/null 2>&1 || true
        else
            warn "'sv' not found — cannot stop via runit; will signal processes directly"
        fi
    fi

    if pgrep -f "${BINARY_PATH}" >/dev/null 2>&1; then
        info "Stopping running l337-audio-server instances..."
        pkill -f "${BINARY_PATH}" >/dev/null 2>&1 || true
    fi

    local _stopped=false
    for _ in $(seq 1 10); do
        pgrep -f "${BINARY_PATH}" >/dev/null 2>&1 || { _stopped=true; break; }
        sleep 1
    done
    if [ "$_stopped" != true ]; then
        local pids
        pids="$(pgrep -f "${BINARY_PATH}" | tr '\n' ' ' || true)"
        fail "l337-audio-server is still running (pid: ${pids}); cannot remove its files.
Stop it manually and re-run:  kill ${pids}"
    fi
    ok "Stopped all instances"

    # --- Preserve live config before the service dir goes away ---
    # server.ini holds the user's choice (e.g. dummy = true, a custom port).
    # Always overwrite the preserved copy: the live config is authoritative,
    # and keeping a stale one would silently revert the user's last change on
    # the next uninstall/install cycle.
    if [ -f "${SERVICE_DIR}/server.ini" ]; then
        if mkdir -p "${DATA_DIR}"; then
            if cp -p "${SERVICE_DIR}/server.ini" "${DATA_DIR}/server.ini" 2>/dev/null; then
                info "Preserved config: ${DATA_DIR}/server.ini"
            else
                warn "Could not preserve ${SERVICE_DIR}/server.ini into ${DATA_DIR}"
            fi
        fi
    fi

    # --- Service registration ---
    # `sv-enable` leaves nothing behind (it only deletes the service dir's
    # 'down' file), so removing the directory fully unregisters the service.
    if [ -d "${SERVICE_DIR}" ]; then
        rm -rf "${SERVICE_DIR}"
        ok "Service removed: ${SERVICE_DIR}"
    else
        warn "Service directory not found: ${SERVICE_DIR}"
    fi

    # --- Binary ---
    local removed_bin=false
    if [ -f "${BINARY_PATH}" ]; then
        rm -f "${BINARY_PATH}"
        removed_bin=true
        ok "Binary removed: ${BINARY_PATH}"
    else
        warn "Binary not found: ${BINARY_PATH}"
    fi
    # A download that was interrupted leaves a .tmp behind (~15 MB); it is ours
    # to delete, unlike any other foreign file in bin/.
    if [ -e "${BINARY_PATH}.tmp" ]; then
        rm -f "${BINARY_PATH}.tmp"
        removed_bin=true
        ok "Stale download removed: ${BINARY_PATH}.tmp"
    fi
    if [ "$removed_bin" = true ]; then
        # Only succeeds if bin/ contains nothing else, so foreign files (and the
        # user's own leftovers) are never deleted.
        rmdir "${INSTALL_DIR}" 2>/dev/null || true
    fi

    # --- Socket symlink + runtime dir ---
    if [ -L "${SOCKET_TARGET}" ]; then
        rm -f "${SOCKET_TARGET}"
        ok "Socket symlink removed: ${SOCKET_TARGET}"
    fi
    remove_runtime_dir "${RUNTIME_DIR}"
    if [ "${PREFIX}/tmp/l337-audio-server" != "${RUNTIME_DIR}" ]; then
        remove_runtime_dir "${PREFIX}/tmp/l337-audio-server"
    fi
    # A run started by install.sh with a different TMPDIR can also have left
    # /tmp/l337-audio-server behind on some devices.
    remove_runtime_dir "/tmp/l337-audio-server"

    # --- Data ---
    if [ "$REMOVE_DATA" = true ]; then
        for d in "${DATA_DIR}" "${CACHE_DIR}" "${PREFIX}/etc/l337-audio-server"; do
            if [ -e "$d" ]; then
                rm -rf "$d"
                ok "Data removed: $d"
            fi
        done
        ok "Data directories removed"
    else
        warn "User data retained:"
        warn "  ${DATA_DIR}    (playlists, app.db, playlists.db, config.ini, server.ini)"
        warn "  ${CACHE_DIR}   (auth token, server logs)"
        warn "Re-run with --remove-data to delete them."
    fi

    warn "Not removed (never created by this installer): a stray ./server.ini"
    warn "in \$HOME or any other working directory."
    echo
    ok "l337-audio-server removed"
    echo

    # --- Optional: remove the packages this server needed ---
    # Asked one at a time, defaulting to keep, because both are general-purpose
    # Termux packages that other software also relies on. The data was retained
    # above, so removing a package cannot destroy anything of the user's.
    info "The following packages were needed by this server:"
    remove_dependency_if_confirmed "termux-services" "runit service supervision"
    remove_dependency_if_confirmed "pulseaudio" "the PulseAudio daemon used for audio output"
    echo
    ok "Uninstall finished."
}

# Ask whether to remove a package the server needed. Default is keep. Without a
# TTY (e.g. `curl | bash`) there is nobody to ask, so the package is kept and
# the manual command is printed instead of guessing.
remove_dependency_if_confirmed() {
    local pkg="$1" purpose="$2" reply=""

    if ! dpkg -s "$pkg" >/dev/null 2>&1; then
        return 0
    fi

    if [ ! -t 0 ]; then
        warn "Kept '${pkg}' (${purpose})."
        warn "  Not removing it without confirmation — to remove it:  pkg uninstall ${pkg}"
        return 0
    fi

    printf 'Remove %s (%s)? [y/N] ' "$pkg" "$purpose"
    read -r reply || reply=""
    case "$reply" in
        [yY]|[yY][eE][sS])
            # Stop the daemon before its files are removed.
            case "$pkg" in
                pulseaudio) pulseaudio --kill >/dev/null 2>&1 || true ;;
            esac
            info "Removing ${pkg}..."
            if pkg uninstall -y "$pkg" >/dev/null 2>&1; then
                ok "Removed ${pkg}"
            else
                warn "Could not remove ${pkg} — remove it manually:  pkg uninstall ${pkg}"
            fi
            ;;
        *)
            ok "Kept ${pkg} (${purpose})."
            ;;
    esac
}

# Parse arguments
INSTALL_PRERELEASE=false
UNINSTALL=false
REMOVE_DATA=false
AUTO_INSTALL_DEPS=true
while [ $# -gt 0 ]; do
    case "$1" in
        --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
        --uninstall|-u)             UNINSTALL=true; shift ;;
        --remove-data)              REMOVE_DATA=true; shift ;;
        --no-install-deps)          AUTO_INSTALL_DEPS=false; shift ;;
        -h|--help)
            echo "Usage: $0 [--pre-release] [--uninstall] [--remove-data] [--no-install-deps]"
            echo ""
            echo "Installs l337-audio-server for Termux (android build) and"
            echo "registers it as a termux-services (runit) service."
            echo ""
            echo "Requirements (missing packages are installed automatically,"
            echo "unless --no-install-deps is given):"
            echo "  - Running in Termux (TERMUX_VERSION set)"
            echo "  - aarch64 architecture"
            echo "  - pkg install termux-services"
            echo "  - pulseaudio installed and running:"
            echo "      pkg install pulseaudio"
            echo "      pulseaudio --start --exit-idle-time=-1"
            echo "  - libopus present when built with --features opus: pkg install libopus"
            echo ""
            echo "Options:"
            echo "  --pre-release       Install latest prerelease instead of stable release"
            echo "  --uninstall, -u     Remove the service and installed binary (keeps data)"
            echo "  --remove-data       With --uninstall: also delete playlists, token and logs"
            echo "  --no-install-deps   Do not install missing packages automatically"
            echo ""
            echo "On uninstall you are asked, one package at a time, whether to also"
            echo "remove the packages this server needed. Interactively that is a"
            echo "prompt; piped (curl | bash) they are kept and the removal command"
            echo "is printed. Data is always kept unless --remove-data is given."
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

# Uninstall runs before the requirement checks on purpose: it must succeed when
# pulseaudio is broken or termux-services was removed, i.e. exactly when a user
# wants out. sv is used if present and degraded to a warning if not.
if [ "$UNINSTALL" = true ]; then
    uninstall_termux
    exit 0
fi

# Termux uses the bionic (NDK) android build, which is linked against
# libpulse.so (built with `--features pulseaudio` in CI).
ASSET="l337-audio-server-aarch64-android-termux"

# --- Requirement checks ---
# Requirements are split into two buckets so the ones that are just a missing
# package can be installed automatically, and the rest get a numbered
# instruction instead of a wall of bullets. Both arrays are rebuilt by
# check_requirements, so the check can be re-run after an auto-install.
MISSING_PKGS=()
NEEDS_USER=()
DAEMON_DOWN=false
STARTED_DAEMON=false

check_requirements() {
    MISSING_PKGS=()
    NEEDS_USER=()
    DAEMON_DOWN=false

    if [ -z "${TERMUX_VERSION:-}" ]; then
        NEEDS_USER+=("Not running inside Termux.
       This installer only supports Termux — run it from a Termux shell.")
    fi

    case "$(uname -m)" in
        aarch64|arm64) ;;
        *) NEEDS_USER+=("This device is '$(uname -m)', but only aarch64 builds are published.") ;;
    esac

    # `sv` is the termux-services (runit) supervision binary. This installer
    # registers a runit service, so it cannot do anything without it.
    # $PREFIX/var/service is NOT a usable signal (the openssh package ships
    # service dirs there without termux-services), so only the binary counts.
    command -v sv >/dev/null 2>&1 || MISSING_PKGS+=("termux-services")

    # The android build prefers PulseAudio because direct AAudio from a Termux
    # console process is unreliable: AAudio_createStreamBuilder returns null
    # and the server exits at startup.
    if ! command -v pulseaudio >/dev/null 2>&1; then
        MISSING_PKGS+=("pulseaudio")
    elif ! pulseaudio --check >/dev/null 2>&1; then
        # Starting the daemon needs no decision from the user, so it is a fix,
        # not an instruction — unless auto-fixing is switched off.
        DAEMON_DOWN=true
    fi

    # libopus is required at runtime when the server is built with --features
    # opus (server-side Opus decoding, G1): the binary links against it at build
    # time, i.e. it has DT_NEEDED libopus.so, and it is resolved from
    # LD_LIBRARY_PATH=$PREFIX/lib.
    LIBOPUS_PATH="${PREFIX}/lib/libopus.so"
    if [ ! -f "$LIBOPUS_PATH" ]; then
        MISSING_PKGS+=("libopus")
    fi
}

# Print what is still missing, one numbered instruction each, then exit.
report_requirements() {
    local n=0

    if [ "${#MISSING_PKGS[@]}" -gt 0 ]; then
        echo >&2
        warn "Packages still missing: ${MISSING_PKGS[*]}"
        warn "Install them with:  pkg install ${MISSING_PKGS[*]}"
    fi

    if [ "${#NEEDS_USER[@]}" -gt 0 ]; then
        echo >&2
        echo -e "\033[1;31m[FAIL]\033[0m Requirements not met:" >&2
        for item in "${NEEDS_USER[@]}"; do
            n=$((n + 1))
            printf '  %d. %s\n' "$n" "$item" >&2
        done
        echo >&2
        echo "Fix the above, then re-run this installer." >&2
        exit 1
    fi
}

check_requirements

# Missing packages need no decision from the user: install them (opt out with
# --no-install-deps). pkg pulls ~15 MB, so always say what is being installed.
if [ "${#MISSING_PKGS[@]}" -gt 0 ]; then
    if [ "$AUTO_INSTALL_DEPS" = true ]; then
        info "Installing missing packages: ${MISSING_PKGS[*]}"
        if pkg install -y "${MISSING_PKGS[@]}"; then
            ok "Installed packages: ${MISSING_PKGS[*]}"
        else
            fail "Could not install ${MISSING_PKGS[*]} — install them manually: pkg install ${MISSING_PKGS[*]}"
        fi
        # A newly installed package can change the picture (pulseaudio is now
        # present but its daemon still is not running).
        check_requirements
    else
        warn "Not installing missing packages (--no-install-deps given)."
        # libopus is a hard runtime dependency of the Opus-capable binary
        # (DT_NEEDED libopus.so): without it the server cannot load at all.
        # Fail clearly rather than leaving a broken install that later blames
        # the audio backend.
        if [ ! -f "${PREFIX}/lib/libopus.so" ]; then
            NEEDS_USER+=("libopus is missing and --no-install-deps was given. The Opus-capable binary links libopus.so at load time. Install it:  pkg install libopus")
            report_requirements
        fi
    fi
fi

# Same reasoning for the daemon: starting it is not a decision.
if [ "$DAEMON_DOWN" = true ]; then
    if [ "$AUTO_INSTALL_DEPS" = true ]; then
        info "Starting the PulseAudio daemon..."
        if pulseaudio --start --exit-idle-time=-1 >/dev/null 2>&1; then
            ok "PulseAudio daemon started"
            STARTED_DAEMON=true
            check_requirements
        else
            NEEDS_USER+=("Could not start the PulseAudio daemon.
       Start it manually with:  pulseaudio --start --exit-idle-time=-1")
        fi
    else
        NEEDS_USER+=("The PulseAudio daemon is not running.
       Start it with:  pulseaudio --start --exit-idle-time=-1")
    fi
fi

report_requirements

ok "sv found: $(command -v sv) (termux-services)"
ok "pulseaudio: $(command -v pulseaudio)"

# --- Determine release tag + binary URL ---
# A partial release can be published without this platform's binary (e.g. a
# targeted workflow_dispatch build: 2026.10.29-6/-7 shipped only the x86_64
# Linux assets). The newest release is therefore not always installable here,
# so releases are walked newest-first and the first one whose release-info.json
# confirms ${ASSET} is in it wins; a release without our binary is skipped, not
# turned into a 404 at download time.
RELEASE_RESPONSE=$(mktemp "${TMPDIR:-/tmp}/l337-release.XXXXXX")
HTTP_CODE=$(curl -sS -L --connect-timeout 15 --max-time 60 \
    -o "$RELEASE_RESPONSE" -w '%{http_code}' \
    "https://api.github.com/repos/${REPO}/releases?per_page=100") || {
    rm -f "$RELEASE_RESPONSE"
    fail "Failed to contact GitHub Releases API. Check network connectivity."
}
# curl exits 0 on a 403/5xx body (rate limits, outages), and an error body
# flattens to zero asset rows — which would be misreported below as "no
# release publishes our asset". Surface the API's own message instead.
if [ "$HTTP_CODE" != "200" ]; then
    API_MESSAGE=$(grep -o '"message"[[:space:]]*:[[:space:]]*"[^"]*"' "$RELEASE_RESPONSE" \
        | head -1 | sed 's/.*"\([^"]*\)"$/\1/' || true)
    rm -f "$RELEASE_RESPONSE"
    fail "GitHub returned HTTP ${HTTP_CODE} while listing releases. ${API_MESSAGE:-Check network connectivity and try again later.}"
fi

# Flatten the release list into one line per published asset:
#   <tag> <prerelease:0|1> <asset name> <browser download url>
# Every name comes from the API's own "name" field, never from a URL this script
# builds, so only an asset GitHub actually publishes in that release can be
# selected.
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

ROWS=$(release_rows "$RELEASE_RESPONSE")
rm -f "$RELEASE_RESPONSE"

# tag -> release-info.json URL, built in one pass so the walk below never has
# to re-scan every row (and spawn an awk) per candidate release.
declare -A INFO_URL=()
while IFS=$'\t' read -r r_tag r_url; do
    [ -n "$r_tag" ] && INFO_URL["$r_tag"]="$r_url"
done < <(printf '%s\n' "$ROWS" | awk -F'\t' \
    '$3 == "release-info.json" { print $1 "\t" $4 }')

TAG=""
DOWNLOAD_URL=""
SKIPPED=()
NEWEST_TAG=""
while IFS=$'\t' read -r r_tag r_pre r_name r_url; do
    # --pre-release walks prereleases newest-first; a plain install walks the
    # stable releases newest-first. Both stop at the newest one that ships our
    # binary.
    if [ "$INSTALL_PRERELEASE" = true ]; then
        [ "$r_pre" = "1" ] || continue
    else
        [ "$r_pre" = "0" ] || continue
    fi
    # Newest release this mode would use if it published every asset, so the
    # fallback below can say why an older one is installed instead.
    [ -z "$NEWEST_TAG" ] && NEWEST_TAG="$r_tag"
    [ "$r_name" = "$ASSET" ] || continue

    # The binary must also be confirmed by that release's own release-info.json:
    # it is generated from the artifacts that actually downloaded, so its asset
    # map is the authoritative record of what the release contains.
    r_info_url="${INFO_URL[$r_tag]:-}"
    if [ -z "$r_info_url" ]; then
        SKIPPED+=("$r_tag (no release-info.json asset)")
        continue
    fi
    INFO_FILE=$(mktemp "${TMPDIR:-/tmp}/l337-revinfo.XXXXXX") || \
        fail "Could not create a temporary file for a release manifest."
    # -f: a 403/404/5xx body would otherwise be saved as the "manifest" and
    # read below as "does not list the asset" — a silent skip that walks the
    # install back to an older release. A network/API failure must fail loudly.
    if ! curl -fsSL --connect-timeout 15 --max-time 30 \
            -o "$INFO_FILE" "$r_info_url"; then
        rm -f "$INFO_FILE"
        fail "Could not download release-info.json for ${r_tag} from ${r_info_url} — the release cannot be verified. Check network connectivity and try again."
    fi
    # The trailing colon anchors the key so a longer asset name that merely
    # starts with this one can never be mistaken for it.
    if ! grep -qF "\"$ASSET\":" "$INFO_FILE"; then
        SKIPPED+=("$r_tag (release-info.json does not list $ASSET)")
        rm -f "$INFO_FILE"
        continue
    fi
    rm -f "$INFO_FILE"

    TAG="$r_tag"
    DOWNLOAD_URL="$r_url"
    break
done <<<"$ROWS"

if [ "${#SKIPPED[@]}" -gt 0 ]; then
    info "Skipped releases without a Termux binary: ${SKIPPED[*]}"
fi
if [ -n "$TAG" ] && [ -n "$NEWEST_TAG" ] && [ "$TAG" != "$NEWEST_TAG" ]; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        info "Newest prerelease ${NEWEST_TAG} does not contain ${ASSET}"
    else
        info "Newest stable release ${NEWEST_TAG} does not contain ${ASSET}"
    fi
    info "Installing release ${TAG} instead, the newest one that does."
fi
if [ -z "$TAG" ]; then
    if [ "$INSTALL_PRERELEASE" = true ]; then
        fail "No prerelease for ${REPO} contains ${ASSET}. Check https://github.com/${REPO}/releases."
    fi
    fail "No stable release for ${REPO} contains ${ASSET}. Re-run with --pre-release to install a prerelease."
fi

info "Latest release: $TAG"

# --- Download binary ---
mkdir -p "${INSTALL_DIR}"

info "Downloading ${ASSET} (release ${TAG})..."
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
# stub, resolved here at runtime); when built with --features opus it also has
# DT_NEEDED libopus.so. The NDK-built binary has no DT_RUNPATH and Termux leaves
# LD_LIBRARY_PATH unset, so these shared libs are not found on their own.
# Without this export the loader fails outright (CANNOT LINK EXECUTABLE).
export LD_LIBRARY_PATH="${PREFIX:-/data/data/com.termux/files/usr}/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

exec "$BINARY" --transport=socket
SVCEOF
chmod +x "${RUN_SCRIPT}"

# Restore the config preserved by `--uninstall`. Without this the server
# recreates the built-in default on first start and silently drops any
# customization the user made (dummy = true, a non-default port,
# buffer_max_bytes), making uninstall/reinstall lossy.
PRESERVED_INI="${DATA_DIR}/server.ini"
if [ -f "${PRESERVED_INI}" ] && [ ! -f "${SERVICE_DIR}/server.ini" ]; then
    cp -p "${PRESERVED_INI}" "${SERVICE_DIR}/server.ini"
    ok "Restored preserved config: ${SERVICE_DIR}/server.ini"
fi

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
You can verify with: 'sv status sshd' (runit answers for existing services)."
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
    if [ ! -f "${PREFIX}/lib/libopus.so" ]; then
        # DT_NEEDED libopus.so is resolved before main(); dummy = true cannot
        # help a loader failure, so say the real cause.
        warn "The Opus-capable binary needs libopus.so, which is missing — the"
        warn "loader fails before the server starts (not an audio-backend issue)."
        warn "Install it and restart:  pkg install libopus && sv restart ${SERVICE_NAME}"
    else
        warn "The server is exiting on startup, usually an audio-backend failure."
        warn "On devices without a usable audio device, set dummy = true in"
        warn "${SERVICE_DIR}/server.ini and run: sv restart ${SERVICE_NAME}"
    fi
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

if [ "$STARTED_DAEMON" = true ]; then
    echo
    warn "The PulseAudio daemon was started with --exit-idle-time=-1, so it"
    warn "stays up, but Termux does not auto-start it after a reboot. Re-run:"
    warn "    pulseaudio --start --exit-idle-time=-1"
fi

echo
echo "Done. Start l337 to connect."
