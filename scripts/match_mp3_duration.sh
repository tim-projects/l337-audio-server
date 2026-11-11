#!/usr/bin/env bash
# match_mp3_duration.sh — Measure MP3 duration with ffmpeg and match
# against the L337 Audio Server /player/status endpoint.
#
# Usage:
#   ./match_mp3_duration.sh [server_port] [server_token] [mp3_path]
#
# Defaults:
#   port   = 13370
#   token  = test-token-iso
#   mp3    = /tmp/l337-test/dispatch.mp3
#
# Steps:
#   1. ffprobe the mp3 to get its true duration (seconds)
#   2. Start (or expect running) the server on $PORT with $TOKEN
#   3. POST /player/play for the mp3
#   4. Poll /player/status every 1s until state==stopped OR duration reached
#   5. Compare: final position_sec vs ffprobe duration (allow ±2s tolerance)
#   6. Report PASS/FAIL and exit code accordingly
#
# Exit codes:
#   0 = PASS (position matches ffprobe duration within tolerance)
#   1 = FAIL (mismatch, timeout, or setup error)
#   2 = ffmpeg/ffprobe not available
#   3 = server not reachable
#
set -euo pipefail

PORT="${1:-13370}"
TOKEN="${2:-test-token-iso}"
MP3="${3:-/tmp/l337-test/dispatch.mp3}"
BASE="https://127.0.0.1:${PORT}"
TOLERANCE="${TOLERANCE:-2}"   # seconds of tolerance for duration match
POLL_INTERVAL_SEC="${POLL_INTERVAL_SEC:-1}"   # seconds between polls

# --- prerequisites ---
if ! command -v ffprobe >/dev/null 2>&1; then
    echo "ERROR: ffprobe not found (install ffmpeg)" >&2
    exit 2
fi
if ! command -v curl >/dev/null 2>&1; then
    echo "ERROR: curl not found" >&2
    exit 2
fi

if [ ! -f "$MP3" ]; then
    echo "ERROR: mp3 file not found: $MP3" >&2
    exit 1
fi

# --- step 1: ffprobe duration ---
FFPROBE_DUR=$(ffprobe -v error -show_entries format=duration \
    -of default=noprint_wrappers=1:nokey=1 "$MP3" 2>/dev/null || true)

if [ -z "$FFPROBE_DUR" ]; then
    echo "ERROR: ffprobe could not read duration from $MP3" >&2
    exit 1
fi

# Round to integer seconds for comparison
FFPROBE_SEC=$(printf "%.0f" "$FFPROBE_DUR")
echo "ffprobe duration: ${FFPROBE_DUR}s (rounded: ${FFPROBE_SEC}s)"

# MAX_WAIT = track duration + buffer so we can wait for playback to reach stopped
MAX_WAIT=$((FFPROBE_SEC + 15))

# --- step 2: check server is reachable ---
if ! curl -sk -H "Authorization: Bearer ${TOKEN}" "${BASE}/version" >/dev/null 2>&1; then
    echo "ERROR: server not reachable at ${BASE} (is it running on port ${PORT} with token ${TOKEN}?)" >&2
    exit 3
fi
echo "server reachable at ${BASE}"

# --- step 3: start playback ---
PLAY_RESP=$(curl -sk -X POST "${BASE}/player/play" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Content-Type: application/json" \
    -d "{\"track_id\":\"dispatch-match\",\"stream_url\":\"${MP3}\",\"title\":\"Dispatch 2\"}" 2>/dev/null || true)

echo "play response: ${PLAY_RESP}"

if ! echo "$PLAY_RESP" | grep -q '"state":"playing"'; then
    echo "ERROR: play did not return state=playing: ${PLAY_RESP}" >&2
    exit 1
fi

# --- step 4: poll until stopped or max wait ---
echo "polling /player/status every ${POLL_INTERVAL_SEC}s (max ${MAX_WAIT}s)..."
SECONDS_ELAPSED=0
LAST_POS=0
FINAL_STATE="unknown"
FINAL_DUR="unknown"

while [ "$SECONDS_ELAPSED" -lt "$MAX_WAIT" ]; do
    STATUS=$(curl -sk -H "Authorization: Bearer ${TOKEN}" "${BASE}/player/status" 2>/dev/null || echo '{}')
    STATE=$(echo "$STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('state','?'))" 2>/dev/null || echo "?")
    POS=$(echo "$STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('position_sec',0))" 2>/dev/null || echo "0")
    DUR=$(echo "$STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('duration_sec','?'))" 2>/dev/null || echo "?")

    LAST_POS="$POS"
    FINAL_STATE="$STATE"
    FINAL_DUR="$DUR"

    echo "  [${SECONDS_ELAPSED}s] state=${STATE} pos=${POS}/${DUR}s"

    if [ "$STATE" = "stopped" ]; then
        break
    fi

    sleep "$POLL_INTERVAL_SEC"
    SECONDS_ELAPSED=$((SECONDS_ELAPSED + POLL_INTERVAL_SEC))
done

# --- step 5: compare ---
echo ""
echo "=== Result ==="
echo "ffprobe duration:  ${FFPROBE_SEC}s"
echo "server duration:   ${FINAL_DUR}s"
echo "final position:    ${LAST_POS}s"
echo "final state:       ${FINAL_STATE}"
echo "elapsed poll time: ${SECONDS_ELAPSED}s"

# Allow small floating-point tolerance: position within TOLERANCE of ffprobe
PASS=0
if [ "$FINAL_STATE" = "stopped" ]; then
    DIFF=$(python3 -c "
diff = abs(float(${LAST_POS}) - float(${FFPROBE_SEC}))
print('PASS' if diff <= ${TOLERANCE} else 'FAIL')
" 2>/dev/null || echo "FAIL")
    PASS=$([ "$DIFF" = "PASS" ] && echo 1 || echo 0)
else
    echo "WARNING: playback did not reach 'stopped' state within ${MAX_WAIT}s"
    PASS=0
fi

if [ "$PASS" -eq 1 ]; then
    echo "RESULT: PASS — playback duration matches ffprobe within ±${TOLERANCE}s"
    exit 0
else
    echo "RESULT: FAIL — playback duration does not match ffprobe"
    exit 1
fi
