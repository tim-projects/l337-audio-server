#!/bin/bash
# check-glibc.sh — assert a built binary does not require a glibc newer than the
# project's declared floor.
#
# Release 2026.10.26-4 required GLIBC_2.43 because it was linked natively on an
# ubuntu-latest runner whose toolchain had moved on (GCC 16 defaulting to C23
# mode made glibc's headers emit __isoc23_sscanf@GLIBC_2.38 and
# atan2f@GLIBC_2.43), and so failed to start on any system with an older glibc.
# Linux builds are pinned to a Debian 12 (bookworm) container image
# (build.yml: rust:1.99.1-bookworm, glibc 2.36); this script is the guard that
# catches it if the pin is ever dropped or the image is re-tagged to something
# newer.
#
# Usage: scripts/check-glibc.sh <binary> [max-version]   (default max: 2.36)
# Exit:  0 = binary is within the floor, 1 = binary requires a newer glibc,
#        2 = usage / missing tooling.
set -uo pipefail

BIN="${1:?usage: check-glibc.sh <binary> [max-version]}"
MAX="${2:-2.36}"

if [ ! -f "$BIN" ]; then
  echo "[FAIL] binary not found: $BIN" >&2
  exit 2
fi

if ! command -v readelf >/dev/null 2>&1; then
  echo "[FAIL] readelf not found (install binutils)" >&2
  exit 2
fi

# Highest GLIBC_x.y version appearing in any DT_VERNEED entry of the binary.
# A binary normally requires many GLIBC_2.2.5-era symbols; only the maximum
# matters for forward compatibility.
required=$(readelf -V "$BIN" 2>/dev/null \
  | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' \
  | sed 's/^GLIBC_//' \
  | sort -V | tail -1)

if [ -z "$required" ]; then
  echo "[OK]   $BIN: no glibc version requirement found"
  exit 0
fi

echo "[INFO] $BIN requires GLIBC_${required}; floor is GLIBC_${MAX}"

# With equal versions sort -V keeps the floor as tail -1, so only a strictly
# newer requirement can displace it.
if [ "$(printf '%s\n%s\n' "$required" "$MAX" | sort -V | tail -1)" != "$MAX" ]; then
  echo "[FAIL] $BIN requires GLIBC_${required}, newer than the supported floor GLIBC_${MAX}." >&2
  echo "       The Linux build must stay on the pinned image (build.yml: rust:1.99.1-bookworm)." >&2
  exit 1
fi

echo "[OK]   $BIN is compatible with glibc <= ${MAX}"
