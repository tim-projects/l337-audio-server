#!/bin/bash
# lib-distro.sh — Shared library for distro-aware package installation.
#
# Sourced by the Linux installer scripts (install-linux-alsa.sh,
# install-linux-pipewire.sh, install-linux-alsa.sh) to centralize:
#   - distro detection from /etc/os-release
#   - package-manager discovery (apt-get / dnf / pacman / zypper / apk)
#   - package-name mapping for base tools and shared libraries
#   - package search fallback
#   - base-tool and audio-library installation with --no-install-deps support
#
# NO output on source — all diagnostics happen only when functions are invoked.
# Safe to source under set -euo pipefail; every function guards commands that
# may legitimately return non-zero.

# ---------------------------------------------------------------------------
# Defaults — callers may override AUTO_INSTALL_DEPS and DRY_RUN before calling
# ensure_* functions. The installers set these from their own flag parsing.
# ---------------------------------------------------------------------------
: "${AUTO_INSTALL_DEPS:=true}"
: "${DRY_RUN:=false}"

# ---------------------------------------------------------------------------
# Distro detection
# ---------------------------------------------------------------------------

# detect_distro: read /etc/os-release and export DISTRO_ID, DISTRO_FAMILY
# (debian|rhel|arch|alpine), DISTRO_VERSION. Falls back to package-manager
# probing when /etc/os-release is absent. Silent on success; warns on failure.
detect_distro() {
    DISTRO_ID=""
    DISTRO_FAMILY=""
    DISTRO_VERSION=""

    if [ -f /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release 2>/dev/null || true
        DISTRO_ID="${ID:-}"
        DISTRO_VERSION="${VERSION_ID:-}"

# ID_LIKE lists families; ID is the concrete distro. Check both.
local like="${ID_LIKE:-} ${ID:-}"
for fam in $like; do
    case "$fam" in
        debian|ubuntu|linuxmint|pop|pureos)
            DISTRO_FAMILY="debian"; break ;;
        rhel|fedora|centos|rocky|almalinux|amzn)
            DISTRO_FAMILY="rhel"; break ;;
        arch|manjaro|endeavouros|arcolinux)
            DISTRO_FAMILY="arch"; break ;;
                alpine)
                    DISTRO_FAMILY="alpine"; break ;;
                gentoo)
                    DISTRO_FAMILY="gentoo"; break ;;
                void)
                    DISTRO_FAMILY="void"; break ;;
                nixos)
                    DISTRO_FAMILY="nixos"; break ;;
            esac
        done
    fi

# Fallback: infer family from the package manager on PATH.
if [ -z "$DISTRO_FAMILY" ]; then
    if command -v apt-get &>/dev/null; then
        DISTRO_FAMILY="debian"
    elif command -v dnf &>/dev/null; then
        DISTRO_FAMILY="rhel"
    elif command -v pacman &>/dev/null; then
        DISTRO_FAMILY="arch"
    elif command -v zypper &>/dev/null; then
        DISTRO_FAMILY="suse"
    elif command -v apk &>/dev/null; then
        DISTRO_FAMILY="alpine"
    elif command -v emerge &>/dev/null; then
        DISTRO_FAMILY="gentoo"
    elif command -v xbps-install &>/dev/null; then
        DISTRO_FAMILY="void"
    else
        warn "Could not detect the distribution or package manager."
        return 1
    fi
    fi
}

# ---------------------------------------------------------------------------
# Package manager discovery
# ---------------------------------------------------------------------------

# pkg_manager: print the first package-manager binary found on PATH.
# Returns 1 if none of apt-get/dnf/pacman/zypper/apk/emerge/xbps-install are found.
pkg_manager() {
    if command -v apt-get &>/dev/null; then echo "apt-get"
    elif command -v dnf &>/dev/null; then echo "dnf"
    elif command -v pacman &>/dev/null; then echo "pacman"
    elif command -v zypper &>/dev/null; then echo "zypper"
    elif command -v apk &>/dev/null; then echo "apk"
    elif command -v emerge &>/dev/null; then echo "emerge"
    elif command -v xbps-install &>/dev/null; then echo "xbps-install"
    else return 1
    fi
}

# pkg_update_prefix: print the cache-sync command for the detected PM.
pkg_update_prefix() {
    if command -v apt-get &>/dev/null; then echo "apt-get update"
    elif command -v dnf &>/dev/null; then echo "dnf check-update || true"
    elif command -v pacman &>/dev/null; then echo "pacman -Sy"
    elif command -v zypper &>/dev/null; then echo "zypper refresh"
    elif command -v apk &>/dev/null; then echo "apk update"
    else return 1
    fi
}

# pkg_install_prefix: print the install command prefix, e.g. "apt-get install -y".
pkg_install_prefix() {
    if command -v apt-get &>/dev/null; then echo "apt-get install -y"
    elif command -v dnf &>/dev/null; then echo "dnf install -y"
    elif command -v pacman &>/dev/null; then echo "pacman -S --noconfirm"
    elif command -v zypper &>/dev/null; then echo "zypper install -y"
    elif command -v apk &>/dev/null; then echo "apk add"
    else return 1
    fi
}

# ---------------------------------------------------------------------------
# Runtime shared-library presence checks
# ---------------------------------------------------------------------------

# is_loader_provided: SONAMEs the C runtime always provides. A dynamically
# linked glibc host always has libc/libm/libpthread/libdl/librt/libresolv
# (glibc 2.34+ folds pthread/dl/rt into libc), so reporting one "missing" — or
# trying to install a package for it — is a false failure that aborted a correct
# Debian install ("No known package provides libm.so.6"). Used by every Linux
# installer's ensure_runtime_dependencies, so it lives here with the other
# shared distro logic instead of being copied into each installer.
is_loader_provided() {
    case "$1" in
        libc.so.6|libm.so.6|libpthread.so.0|libdl.so.2|librt.so.1|libresolv.so.2) return 0 ;;
        libnss_*.so.2) return 0 ;;
        *) return 1 ;;
    esac
}

# lib_present: is SONAME $1 resolvable on this host? The (refreshed) ldconfig
# cache first, then the standard library dirs: a cache that has not been
# regenerated after an earlier package install in this same run reported
# libraries that were present, and the install then failed on its own success
# (libopus0 installed, yet "Runtime library libopus.so.0 is still missing").
lib_present() {
    local soname="$1" d
    ldconfig -p 2>/dev/null | grep -q -F "$soname" && return 0
    for d in /lib /usr/lib /lib64 /usr/lib64 /lib/*-linux-gnu /usr/lib/*-linux-gnu; do
        [ -e "$d/$soname" ] && return 0
    done
    return 1
}

# ---------------------------------------------------------------------------
# Package search
# ---------------------------------------------------------------------------

# pkg_search: search the package cache for $1, print matching package names
# (one per line). Returns 1 on unsupported PM or no results.
pkg_search() {
    local keyword="$1"
    if command -v apt-get &>/dev/null; then
        apt-cache search "$keyword" 2>/dev/null | awk '{print $1}' | head -20
    elif command -v dnf &>/dev/null; then
        dnf search "$keyword" 2>/dev/null | awk '/^[^ ]/{print $1}' | head -20
    elif command -v pacman &>/dev/null; then
        pacman -Ss "$keyword" 2>/dev/null \
            | sed -n 's|^[^ ]*/\([^ ]*\) .*$|\1|p' | head -20
    elif command -v zypper &>/dev/null; then
        zypper search -x "$keyword" 2>/dev/null | tail -n +4 \
            | awk '{print $3}' | head -20
    elif command -v apk &>/dev/null; then
        apk search "$keyword" 2>/dev/null | head -20
    else
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Package name mapping
# ---------------------------------------------------------------------------

# pkg_for_lib: map a DT_NEEDED SONAME to the package that provides it on the
# detected distro. Prints the package name; returns 1 (no output) when the
# SONAME is unknown for this package manager.
pkg_for_lib() {
    local soname="$1"
    if command -v apt-get &>/dev/null; then
        case "$soname" in
            libopus.so*)          echo "libopus0" ;;
            libasound.so*)        echo "libasound2" ;;
            libpipewire-0.3.so*)  echo "libpipewire-0.3-0" ;;
            libpulse.so*)         echo "libpulse0" ;;
            libm.so*)             echo "libc6" ;;
            *) return 1 ;;
        esac
    elif command -v dnf &>/dev/null; then
        case "$soname" in
            libopus.so*)          echo "opus" ;;
            libasound.so*)        echo "alsa-lib" ;;
            libpipewire-0.3.so*)  echo "pipewire-libs" ;;
            libpulse.so*)         echo "pulseaudio-libs" ;;
            libm.so*)             echo "glibc" ;;
            *) return 1 ;;
        esac
    elif command -v pacman &>/dev/null; then
        case "$soname" in
            libopus.so*)          echo "opus" ;;
            libasound.so*)        echo "alsa-lib" ;;
            libpipewire-0.3.so*)  echo "pipewire" ;;
            libpulse.so*)         echo "libpulse" ;;
            libm.so*)             echo "glibc" ;;
            *) return 1 ;;
        esac
    elif command -v zypper &>/dev/null; then
        case "$soname" in
            libopus.so*)          echo "libopus0" ;;
            libasound.so*)        echo "libasound2" ;;
            libpipewire-0.3.so*)  echo "libpipewire-0-3" ;;
            libpulse.so*)         echo "libpulse0" ;;
            libm.so*)             echo "libc6" ;;
            *) return 1 ;;
        esac
    elif command -v apk &>/dev/null; then
        case "$soname" in
            libopus.so*)          echo "libopus" ;;
            libasound.so*)        echo "alsa-lib" ;;
            libpipewire-0.3.so*)  echo "pipewire" ;;
            libpulse.so*)         echo "pulseaudio" ;;
            libm.so*)             echo "musl" ;;
            *) return 1 ;;
        esac
    else
        return 1
    fi
}

# pkg_for_tool: map a base tool (command name) to the package that provides it.
# Prints the package name on stdout. Returns 0 with no output when the tool is
# already present on PATH. Returns 1 when the package cannot be resolved.
# Uses the DISTRO_FAMILY lookup table; falls back to pkg_search.
pkg_for_tool() {
    local tool="$1"

    # Already present — no package needed.
    if command -v "$tool" &>/dev/null; then
        return 0
    fi

    # --- per-family lookup tables ---
    case "$DISTRO_FAMILY" in
        debian)
            case "$tool" in
                curl)            echo "curl" ;;
                file)            echo "file" ;;
                readelf)         echo "binutils" ;;
                ldconfig)        echo "libc-bin" ;;
                groupadd|useradd) echo "passwd" ;;
                systemctl)       echo "systemd" ;;
                *) _pkg_search_fallback "$tool" || return 1 ;;
            esac
            ;;
        rhel)
            case "$tool" in
                curl)            echo "curl" ;;
                file)            echo "file" ;;
                readelf)         echo "binutils" ;;
                ldconfig)        echo "glibc-common" ;;
                groupadd|useradd) echo "shadow-utils" ;;
                systemctl)       echo "systemd" ;;
                *) _pkg_search_fallback "$tool" || return 1 ;;
            esac
            ;;
        arch)
            case "$tool" in
                curl)            echo "curl" ;;
                file)            echo "file" ;;
                readelf)         echo "binutils" ;;
                ldconfig)        echo "" ;; # ships with glibc on Arch; no package needed
                groupadd|useradd) echo "shadow" ;;
                systemctl)       echo "systemd" ;;
                *) _pkg_search_fallback "$tool" || return 1 ;;
            esac
            ;;
        alpine)
            case "$tool" in
                curl)            echo "curl" ;;
                file)            echo "file" ;;
                readelf)         echo "binutils" ;;
                ldconfig)        echo "musl-utils" ;; # shortcut: on musl/Alpine ldconfig ships with musl-utils
                groupadd|useradd) echo "shadow" ;;
                systemctl)       echo "" ;; # systemd is not available on Alpine
                *) _pkg_search_fallback "$tool" || return 1 ;;
            esac
            ;;
        *)
            _pkg_search_fallback "$tool" || return 1
            ;;
    esac
}

# _pkg_search_fallback: search the package cache for $1 and print the first
# match. Returns 1 when pkg_search finds nothing.
_pkg_search_fallback() {
    local tool="$1" result
    result=$(pkg_search "$tool" 2>/dev/null | head -1) || true
    if [ -n "$result" ]; then
        echo "$result"
        return 0
    fi
    return 1
}

# ---------------------------------------------------------------------------
# Installation wrapper
# ---------------------------------------------------------------------------

# Check if packages are already installed. Returns list of missing packages.
# Usage: check_packages_installed <pkg1> <pkg2> ...
check_packages_installed() {
    [ $# -gt 0 ] || { echo ""; return 0; }
    local pm missing=() pkg
    if ! pm=$(pkg_manager); then
        warn "No supported package manager found (apt-get, dnf, pacman, zypper, apk)."
        printf '%s\n' "$@"
        return 1
    fi
    for pkg in "$@"; do
        case "$pm" in
            apt-get) dpkg -l "$pkg" 2>/dev/null | grep -q "^ii" || missing+=("$pkg") ;;
            dnf)     rpm -q "$pkg" >/dev/null 2>&1 || missing+=("$pkg") ;;
            pacman)  pacman -Q "$pkg" >/dev/null 2>&1 || missing+=("$pkg") ;;
            zypper)  rpm -q "$pkg" >/dev/null 2>&1 || missing+=("$pkg") ;;
            apk)     apk info -e "$pkg" >/dev/null 2>&1 || missing+=("$pkg") ;;
        esac
    done
    printf '%s\n' "${missing[@]}"
}

# install_packages: install a list of resolved package names using the detected
# PM. Runs pkg_update_prefix before pkg_install_prefix. Respects AUTO_INSTALL_DEPS
# (prints instructions instead of installing when false), DRY_RUN (prints what
# would be installed without running anything), and FORCE_INSTALL (skips
# already-installed check). Returns 1 on failure.
install_packages() {
    [ $# -gt 0 ] || return 0

    local pkgs=("$@")
    local pm install_cmd update_cmd

    if ! pm=$(pkg_manager); then
        warn "No supported package manager found (apt-get, dnf, pacman, zypper, apk)."
        warn "Install packages manually: ${pkgs[*]}"
        return 1
    fi

    install_cmd=$(pkg_install_prefix) || {
        warn "Cannot determine install command for $pm."
        return 1
    }

    # Check already-installed unless FORCE_INSTALL=true
    if [ "${FORCE_INSTALL:-false}" != "true" ]; then
        local missing
        missing=$(check_packages_installed "${pkgs[@]}")
        if [ -z "$missing" ]; then
            ok "Already installed: ${pkgs[*]}"
            return 0
        fi
        pkgs=($missing)
    fi

    if [ "$AUTO_INSTALL_DEPS" = false ]; then
        echo "The following packages are required:" >&2
        local n=0
        for pkg in "${pkgs[@]}"; do
            n=$((n + 1))
            printf '  %d. %s %s\n' "$n" "$install_cmd" "$pkg" >&2
        done
        echo "Install them manually and re-run the installer." >&2
        return 1
    fi

    if [ "$DRY_RUN" = true ]; then
        info "Dry-run: would install: ${pkgs[*]}"
        return 0
    fi

    update_cmd=$(pkg_update_prefix) || update_cmd=""
    if [ -n "$update_cmd" ]; then
        info "Updating package cache..."
        # shellcheck disable=SC2086
        $update_cmd || warn "Package cache update failed; continuing..."
    fi

    info "Installing: ${pkgs[*]}"
    # shellcheck disable=SC2086
    if $install_cmd "${pkgs[@]}" >/dev/null 2>&1; then
        ok "Installed: ${pkgs[*]}"
        return 0
    else
        warn "Could not install: ${pkgs[*]}"
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Base tool and audio library installation
# ---------------------------------------------------------------------------

# ensure_base_tools: verify that each named command is present on PATH via
# command -v. Missing tools are resolved to packages via pkg_for_tool and
# installed via install_packages. Returns 0 when all tools are available or
# successfully installed; returns 1 when AUTO_INSTALL_DEPS is false or
# installation fails and a tool remains missing.
#
# Usage: ensure_base_tools curl file readelf ldconfig groupadd useradd [systemctl]
ensure_base_tools() {
    [ $# -gt 0 ] || return 0

    local missing_pkgs=()
    local tool pkg

    for tool in "$@"; do
        if command -v "$tool" &>/dev/null; then
            continue
        fi
        info "Required tool missing: $tool"
        pkg=$(pkg_for_tool "$tool") || pkg=""
        if [ -n "$pkg" ]; then
            missing_pkgs+=("$pkg")
        elif [ "$AUTO_INSTALL_DEPS" = false ]; then
            warn "No known package provides '$tool' — install it manually."
        else
            fail "No known package provides '$tool' on this distribution. Cannot continue."
        fi
    done

    if [ "${#missing_pkgs[@]}" -gt 0 ]; then
        install_packages "${missing_pkgs[@]}"
    else
        ok "All required base tools are present."
    fi
}

# ensure_audio_libs: install the system audio libraries for the given backend
# (alsa or pipewire) on the detected distro. Uses a per-family table of package
# names. Returns 1 when packages cannot be installed, but does NOT exit — the
# caller should continue to ensure_runtime_dependencies which performs the
# final gate.
#
# Usage: ensure_audio_libs alsa|pipewire
ensure_audio_libs() {
    local backend="$1"
    local pkgs=()

    case "$backend" in
        alsa)
            if   command -v apt-get &>/dev/null; then pkgs=("libasound2" "libopus0")
            elif command -v dnf &>/dev/null;      then pkgs=("alsa-lib" "opus")
            elif command -v pacman &>/dev/null;   then pkgs=("alsa-lib" "opus")
            elif command -v zypper &>/dev/null;   then pkgs=("libasound2" "libopus0")
            elif command -v apk &>/dev/null;      then pkgs=("alsa-lib" "libopus")
            fi
            ;;
        pipewire)
            if   command -v apt-get &>/dev/null; then pkgs=("libpipewire-0.3-0" "libopus0" "libpulse0")
            elif command -v dnf &>/dev/null;      then pkgs=("pipewire-libs" "opus" "pulseaudio-libs")
            elif command -v pacman &>/dev/null;   then pkgs=("pipewire" "pipewire-alsa" "opus" "libpulse")
            elif command -v zypper &>/dev/null;   then pkgs=("libpipewire-0-3" "libopus0" "libpulse0")
            elif command -v apk &>/dev/null;      then pkgs=("pipewire" "libopus" "pulseaudio")
            fi
            ;;
        *)
            warn "Unknown audio backend: $backend"
            return 1
            ;;
    esac

    if [ "${#pkgs[@]}" -eq 0 ]; then
        warn "No supported package manager to install audio libraries for backend: $backend"
        return 1
    fi

    info "Ensuring audio libraries for backend: $backend"
    install_packages "${pkgs[@]}"
}

# ---------------------------------------------------------------------------
# wait_until_ok() — poll a predicate until it succeeds.
#
#   wait_until_ok <seconds> <command> [args...]
#
# Returns 0 as soon as the command succeeds, 1 after the timeout. A single
# `sleep 1` is a guess: systemd can need several seconds to bring a unit up after
# exec start, and a too-short wait turns a healthy service into a false failure
# (and a needless binary rollback).
# ---------------------------------------------------------------------------
wait_until_ok() {
    local timeout="$1"; shift
    local deadline=$(( SECONDS + timeout ))
    while ! "$@" >/dev/null 2>&1; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            return 1
        fi
        sleep 1
    done
    return 0
}

# ---------------------------------------------------------------------------
# Auto-detect distro when the library is sourced (silent on success).
# Re-detection happens if the vars are unset (e.g. function called in a subshell).
# ---------------------------------------------------------------------------
if [ -z "${DISTRO_FAMILY:-}" ]; then
    detect_distro || true
fi
