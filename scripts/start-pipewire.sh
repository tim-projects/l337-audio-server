#!/bin/bash
# Start a dedicated PipeWire session for the l337-audio-server service.
# This script is invoked from ExecStartPre and must return quickly;
# long-running daemons are backgrounded inside it.
set -euo pipefail

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/l337-audio-server}"
export PIPEWIRE_RUNTIME_DIR="${XDG_RUNTIME_DIR}"

# Ensure the runtime directory exists with correct ownership.
if [ ! -d "${XDG_RUNTIME_DIR}" ]; then
    install -d -m 0700 "${XDG_RUNTIME_DIR}"
    # If running as root in ExecStartPre, chown to the service user.
    if [ "$(id -u)" = "0" ]; then
        chown l337:l337 "${XDG_RUNTIME_DIR}"
    fi
fi

# Start PipeWire in the background.
# Use nohup + & so it survives after ExecStartPre returns.
if ! /usr/bin/pipewire -d >/dev/null 2>&1; then
    nohup /usr/bin/pipewire >/dev/null 2>&1 &
fi

# Start WirePlumber (session manager) in the background.
nohup /usr/bin/wireplumber >/dev/null 2>&1 &

# Brief pause so the session is up before the server opens its stream.
sleep 1

exit 0
