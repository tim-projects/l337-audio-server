#!/usr/bin/env bash
# Intel Linux Vorbis verification script
# Run on x86_64 Linux with libvorbis-dev libogg-dev installed

set -euo pipefail

echo "=== Building with native Vorbis ==="
CARGO_HOME=/tmp/cargo cargo build --release --features backend,vorbis

echo "=== Checking linkage ==="
readelf -d target/release/l337-audio-server | grep -E "vorbis|ogg|opus" || true

echo "=== Running tests ==="
CARGO_HOME=/tmp/cargo cargo test --features backend,vorbis

echo "=== Starting isolated server ==="
rm -rf /tmp/l337-test && mkdir -p /tmp/l337-test/run /tmp/l337-test/state /tmp/l337-test/cache
L337_RUNTIME_DIR=/tmp/l337-test/run \
L337__SERVER__TOKEN=test-token \
L337__SERVER__PORT=13370 \
RUST_LOG=l337_audio_server=debug \
./target/release/l337-audio-server --transport=socket \
> /tmp/l337-test/server.log 2>&1 &
SERVER_PID=$!

sleep 3

# Find a test OGG file
TEST_OGG="${1:-}"
if [ -z "$TEST_OGG" ]; then
    TEST_OGG=$(find /home -name "*.ogg" -o -name "*.OGG" 2>/dev/null | head -1 || true)
fi
if [ -z "$TEST_OGG" ]; then
    echo "No test OGG file found. Pass path as first argument or place one in /home."
    kill $SERVER_PID 2>/dev/null || true
    exit 1
fi

echo "=== Playing: $TEST_OGG ==="
curl --unix-socket /tmp/l337-test/run/l337.sock \
  -H "Authorization: Bearer test-token" \
  -X POST http://localhost/player/play \
  -H "Content-Type: application/json" \
  -d "{\"track_id\":\"test\",\"stream_url\":\"$TEST_OGG\",\"title\":\"test\",\"artist\":\"test\"}"

echo "=== Waiting for playback... ==="
sleep 5

# Check logs for native decode
echo "=== Checking decode path ==="
grep -E "native decode|Vorbis chunk|falling back" /tmp/l337-test/server.log | head -5

# Poll until stopped
for i in {1..60}; do
    STATUS=$(curl -s --unix-socket /tmp/l337-test/run/l337.sock \
      -H "Authorization: Bearer test-token" \
      http://localhost/player/status 2>/dev/null || echo '{}')
    STATE=$(echo "$STATUS" | jq -r '.state')
    POS=$(echo "$STATUS" | jq -r '.position_sec')
    DUR=$(echo "$STATUS" | jq -r '.duration_sec')
    echo "  state=$STATE position=$POS duration=$DUR"
    if [ "$STATE" = "stopped" ]; then
        break
    fi
    sleep 2
done

# Duration verification
echo "=== Duration verification ==="
EXPECTED=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$TEST_OGG" 2>/dev/null | cut -d. -f1)
ACTUAL=$(echo "$STATUS" | jq -r '.position_sec')
echo "Expected (ffprobe): ${EXPECTED}s"
echo "Actual (position_sec): ${ACTUAL}s"
DIFF=$((ACTUAL - EXPECTED))
if [ ${DIFF#-} -le 2 ]; then
    echo "✓ Duration match within ±2s"
else
    echo "✗ Duration mismatch >2s"
fi

# Incremental decode check
CHUNKS=$(grep -c "Vorbis chunk" /tmp/l337-test/server.log || echo 0)
echo "Chunks decoded: $CHUNKS (should be >100 for incremental)"

# Cleanup
kill $SERVER_PID 2>/dev/null || true
wait $SERVER_PID 2>/dev/null || true

echo "=== Verification complete ==="