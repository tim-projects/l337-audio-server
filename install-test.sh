#!/bin/bash
# install-test.sh — curl-only GitHub release resolver (no api.github.com).
#
# Test harness for install.sh: finds the newest installable release for this
# platform using releases.atom + release/expanded_assets HTML + releases/download
# URLs. This avoids the GitHub REST API rate limit (unauthenticated 60/hr/IP).
# GitHub web/CDN endpoints can still apply abuse controls in edge cases.
#
# Usage:
#   ./install-test.sh [--pre-release] [--dry-run|--download]
#   ./install-test.sh --help
set -euo pipefail
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin${PATH:+:$PATH}"

REPO="tim-projects/l337-audio-server"
INSTALL_PRERELEASE=false
DRY_RUN=true

fail() { echo -e "\033[1;31m[FAIL]\033[0m $*" >&2; exit 1; }
warn() { echo -e "\033[1;33m[WARN]\033[0m $*" >&2; }
info() { echo -e "\033[1;34m[INFO]\033[0m $*"; }
ok()   { echo -e "\033[1;32m[OK]\033[0m   $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --pre-release|--prerelease) INSTALL_PRERELEASE=true; shift ;;
    --dry-run) DRY_RUN=true; shift ;;
    --download) DRY_RUN=false; shift ;; # download selected binary to TMPDIR (no install)
    -h|--help)
      cat <<'EOF'
Usage: install-test.sh [--pre-release] [--dry-run|--download]

Resolves a release for this platform using only curl + github.com (no api.github.com).

Options:
  --pre-release   Resolve newest prerelease instead of newest stable
  --dry-run       Resolve only; do not download the binary (default)
  --download      Download the selected binary to TMPDIR and verify checksums
  -h, --help      Show this help
EOF
      exit 0 ;;
    *) fail "Unknown option: $1" ;;
  esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in Linux*) OS_TYPE=linux ;; Darwin*) OS_TYPE=macos ;; *) fail "Unsupported OS: $OS" ;; esac
case "$ARCH" in
  x86_64|amd64) ARCH_TYPE=x86_64 ;;
  aarch64|arm64) ARCH_TYPE=aarch64 ;;
  armv7l|armhf) ARCH_TYPE=armv7 ;;
  riscv64) ARCH_TYPE=riscv64 ;;
  *) fail "Unsupported architecture: $ARCH" ;;
esac
OS_VARIANT="$OS_TYPE"
if [ "$OS_TYPE" = linux ] && [ -n "${TERMUX_VERSION:-}" ]; then
  OS_VARIANT=termux
fi

if [ "$OS_VARIANT" = termux ]; then
  BINARY_ASSET="l337-audio-server-aarch64-android-termux"
elif [ "$OS_TYPE" = linux ] && { [ "$ARCH_TYPE" = armv7 ] || [ "$ARCH_TYPE" = riscv64 ]; }; then
  BINARY_ASSET="l337-audio-server-${ARCH_TYPE}-linux-alsa"
elif [ "$OS_TYPE" = linux ]; then
  BINARY_ASSET="l337-audio-server-${ARCH_TYPE}-linux-pipewire"
elif [ "$OS_TYPE" = macos ]; then
  BINARY_ASSET="l337-audio-server-x86_64-apple-darwin"
else
  fail "Unsupported platform"
fi
info "Platform: $OS_VARIANT/$ARCH_TYPE asset=$BINARY_ASSET (curl-only, no api.github.com)"

version_lt() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ]; }

system_glibc_version() {
  local v="" prefix loader
  if command -v ldd >/dev/null 2>&1; then
    v=$(ldd --version 2>/dev/null | head -1 | sed -n 's/.*[[:space:]]\([0-9][0-9]*\.[0-9][0-9.]*\)[[:space:]]*$/\1/p')
  fi
  if [ -z "$v" ] && command -v getconf >/dev/null 2>&1; then
    v=$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/.*glibc[[:space:]]*\([0-9][0-9]*\.[0-9][0-9.]*\).*/\1/p')
  fi
  prefix="${PREFIX:-}"
  if [ -z "$v" ] && [ -n "$prefix" ] && [ -d "$prefix/glibc/lib" ]; then
    for loader in "$prefix"/glibc/lib/ld-linux-*.so.1; do
      [ -f "$loader" ] || continue
      v=$("$loader" --version 2>/dev/null | head -1 | sed -n 's/.*version[[:space:]]\([0-9][0-9.]*\).*/\1/p')
      [ -n "$v" ] && break
    done
  fi
  printf '%s' "$v"
}

SYSTEM_GLIBC=""
if [ "$OS_TYPE" = linux ]; then
  SYSTEM_GLIBC=$(system_glibc_version)
  if [ -n "$SYSTEM_GLIBC" ]; then
    info "System glibc: $SYSTEM_GLIBC"
  fi
fi

# Tags newest-first from Atom feed (no REST API).
fetch_tags() {
  local atom http
  atom=$(mktemp "${TMPDIR:-/tmp}/l337-atom.XXXXXX") || fail "mktemp failed"
  http=$(curl -sS -L --connect-timeout 15 --max-time 60 -o "$atom" -w '%{http_code}' \
    "https://github.com/${REPO}/releases.atom") || { rm -f "$atom"; fail "Failed to fetch releases.atom"; }
  if [ "$http" != "200" ]; then
    rm -f "$atom"
    fail "releases.atom HTTP $http"
  fi
  grep -oE '/releases/tag/[^"<]+' "$atom" | sed 's#.*/releases/tag/##' | awk '!seen[$0]++'
  rm -f "$atom"
}

# Best-effort prerelease detection from tag page HTML (avoids API).
is_prerelease_tag() {
  local tag="$1" page http
  page=$(mktemp "${TMPDIR:-/tmp}/l337-tag.XXXXXX") || return 1
  http=$(curl -sS -L --connect-timeout 15 --max-time 30 -o "$page" -w '%{http_code}' \
    "https://github.com/${REPO}/releases/tag/${tag}") || { rm -f "$page"; return 1; }
  if [ "$http" != "200" ]; then rm -f "$page"; return 1; fi
  if grep -qiE 'Pre-release|Label--warning[^>]*>[^<]*Pre|class="[^"]*prerelease' "$page"; then
    rm -f "$page"; return 0
  fi
  rm -f "$page"; return 1
}

tag_has_asset() {
  local tag="$1" asset="$2" html http
  html=$(mktemp "${TMPDIR:-/tmp}/l337-assets.XXXXXX") || return 1
  http=$(curl -sS -L --connect-timeout 15 --max-time 60 -o "$html" -w '%{http_code}' \
    "https://github.com/${REPO}/releases/expanded_assets/${tag}") || { rm -f "$html"; return 1; }
  if [ "$http" != "200" ]; then rm -f "$html"; return 1; fi
  if grep -qF "/releases/download/${tag}/${asset}" "$html"; then
    rm -f "$html"; return 0
  fi
  rm -f "$html"; return 1
}

release_info_glibc() {
  local info_file="$1" asset="$2" line value
  [ -f "$info_file" ] || return 1
  line=$(grep -F "\"$asset\":" "$info_file" 2>/dev/null | head -1) || true
  [ -z "$line" ] && return 1
  case "$line" in *null*) echo none; return 0 ;; esac
  value=$(printf '%s' "$line" | sed -n 's/.*:[[:space:]]*"\{0,1\}\([0-9][0-9.]*\)"\{0,1\}.*/\1/p')
  [ -z "$value" ] && return 1
  printf '%s' "$value"
}

release_info_checksum() {
  local info_file="$1" asset="$2" key="$3" section block
  [ -f "$info_file" ] || return 1
  section=$(sed -n '/"checksums"[[:space:]]*:[[:space:]]*{/,/^  }/p' "$info_file" 2>/dev/null) || true
  [ -z "$section" ] && return 1
  block=$(printf '%s\n' "$section" | sed -n "/\"$asset\"[[:space:]]*:[[:space:]]*{/,/^    }/p")
  [ -z "$block" ] && return 1
  printf '%s\n' "$block" | grep -o "\"$key\"[[:space:]]*:[[:space:]]*[^,]*" | head -1 \
    | sed 's/.*:[[:space:]]*//; s/^"//; s/,$//; s/"$//'
}

TAG=""
DOWNLOAD_URL=""
SELECTED_MD5=""
SELECTED_CRC32=""
SELECTED_SIZE=""

while IFS= read -r tag; do
  [ -n "$tag" ] || continue
  if [ "$INSTALL_PRERELEASE" = true ]; then
    is_prerelease_tag "$tag" || { info "skip $tag (not prerelease)"; continue; }
  else
    if is_prerelease_tag "$tag"; then
      info "skip $tag (prerelease)"
      continue
    fi
  fi

  tag_has_asset "$tag" "$BINARY_ASSET" || { info "skip $tag (no $BINARY_ASSET)"; continue; }
  tag_has_asset "$tag" "release-info.json" || { info "skip $tag (no release-info.json)"; continue; }

  info_file=$(mktemp "${TMPDIR:-/tmp}/l337-info.XXXXXX") || continue
  http=$(curl -sS -L --connect-timeout 15 --max-time 30 -o "$info_file" -w '%{http_code}' \
    "https://github.com/${REPO}/releases/download/${tag}/release-info.json") || { rm -f "$info_file"; continue; }
  if [ "$http" != "200" ]; then
    rm -f "$info_file"
    info "skip $tag (release-info.json HTTP $http)"
    continue
  fi

  req=$(release_info_glibc "$info_file" "$BINARY_ASSET") || { rm -f "$info_file"; info "skip $tag (asset not in manifest)"; continue; }
  SELECTED_MD5=$(release_info_checksum "$info_file" "$BINARY_ASSET" md5 || true)
  SELECTED_CRC32=$(release_info_checksum "$info_file" "$BINARY_ASSET" crc32 || true)
  SELECTED_SIZE=$(release_info_checksum "$info_file" "$BINARY_ASSET" size || true)
  rm -f "$info_file"

  if [ "$req" != "none" ] && [ -n "$SYSTEM_GLIBC" ] && version_lt "${SYSTEM_GLIBC%%-*}" "${req%%-*}"; then
    info "skip $tag (needs glibc $req, have $SYSTEM_GLIBC)"
    continue
  fi

  TAG="$tag"
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${tag}/${BINARY_ASSET}"
  break
done < <(fetch_tags)

[ -n "$TAG" ] || fail "No installable release found for $BINARY_ASSET (curl-only). Try --pre-release."

ok "Selected: $TAG"
info "URL: $DOWNLOAD_URL"
[ -n "$SELECTED_MD5" ] && info "manifest md5=$SELECTED_MD5 size=${SELECTED_SIZE:-?} crc32=${SELECTED_CRC32:-?}"

if [ "$DRY_RUN" = false ]; then
  out="${TMPDIR:-/tmp}/l337-audio-server-download-test"
  rm -f "$out"
  info "Downloading to $out ..."
  http=$(curl -fL --connect-timeout 15 --max-time 300 -o "$out" -w '%{http_code}' "$DOWNLOAD_URL") || http="${http:-000}"
  case "$http" in 2??) ok "Downloaded $(wc -c <"$out" | tr -d ' ') bytes" ;; *) rm -f "$out"; fail "Download HTTP $http" ;; esac

  if [ -n "$SELECTED_MD5" ] && command -v md5sum >/dev/null 2>&1; then
    got=$(md5sum "$out" | awk '{print $1}')
    if [ "$got" != "$SELECTED_MD5" ]; then rm -f "$out"; fail "md5 mismatch expected=$SELECTED_MD5 got=$got"; fi
    ok "md5 verified"
  elif [ -n "$SELECTED_CRC32" ] && command -v cksum >/dev/null 2>&1; then
    cksum_out=$(cksum "$out" 2>/dev/null) || cksum_out=""
    read -r ck_crc ck_len _ <<<"$cksum_out"
    if [ -n "$ck_crc" ] && [ "$ck_crc" != "$SELECTED_CRC32" ]; then rm -f "$out"; fail "crc32 mismatch expected=$SELECTED_CRC32 got=$ck_crc"; fi
    if [ -n "$SELECTED_SIZE" ] && [ -n "$ck_len" ] && [ "$ck_len" != "$SELECTED_SIZE" ]; then rm -f "$out"; fail "size mismatch expected=$SELECTED_SIZE got=$ck_len"; fi
    ok "crc32/size verified"
  elif [ -n "$SELECTED_SIZE" ]; then
    got=$(wc -c <"$out" | tr -d ' ')
    if [ "$got" != "$SELECTED_SIZE" ]; then rm -f "$out"; fail "size mismatch expected=$SELECTED_SIZE got=$got"; fi
    ok "size verified"
  else
    warn "No checksum/size in manifest; downloaded file not hash-verified"
  fi
fi
