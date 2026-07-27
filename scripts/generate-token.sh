#!/bin/bash
# generate-token.sh — Generate a secure random token for L337 Audio Server
#
# Usage:
#   ./scripts/generate-token.sh                  # 32-char token
#   ./scripts/generate-token.sh --length 64      # custom length
set -uo pipefail

LENGTH="32"
while [ $# -gt 0 ]; do
    case "$1" in
        --length) LENGTH="$2"; shift 2 ;;
        *) shift ;;
    esac
done

chars="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
token=""

for ((i=0; i<LENGTH; i++)); do
    idx=$(( RANDOM % ${#chars} ))
    token="${token}${chars:$idx:1}"
done

echo "$token"
