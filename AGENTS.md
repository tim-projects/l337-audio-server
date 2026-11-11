# L337 Audio Server — Agent Instructions

## Build

**Disk space is tight.** All cargo artifacts (registry, target dir, build cache) must live under `/tmp`. The in-repo `target/` must not be used for builds.

### Preferred path: fetch a prebuilt binary via GitHub Actions

The CI workflow `Build Release Binaries` produces a Linux `x86_64` binary for
every push to `main`. Prefer downloading that artifact instead of building
locally — it avoids the Rust toolchain entirely:

```bash
gh workflow run "Build Release Binaries"           # start a build for HEAD
gh run list --workflow="Build Release Binaries" --limit=3
gh run download <run-id>                           # fetch artifacts
```

If a binary is not committed to the repo and no fresh artifact is available
locally, use a `gh run download ...` to obtain the one built from the current
commit. Local builds are a last resort (see below).

### Local build (fallback only — only when explicitly requested)

Only build for **Linux** locally. All other platforms (Windows, macOS,
Android, iOS, etc.) are built and packaged exclusively by GitHub Actions
runners. Do not attempt cross-compilation or native builds for non-Linux
targets on this machine.

Never run `cargo build`, `cargo check`, or `cargo test` directly — they will
write large temp files into `./target` and exhaust disk space. Always use the
project build script, which pins `CARGO_TARGET_DIR=/tmp/l337-build`
and copies the finished binary to `bin/l337-audio-server`:

```bash
CARGO_HOME=/tmp/cargo-home ./scripts/build.sh
```

`cargo check` (which does not produce a linked binary) is acceptable when you
need to verify code compiles:

```bash
CARGO_HOME=/tmp/cargo-home cargo check
```

Do not create a `.cargo/config.toml` in the repo. The build script already
handles `CARGO_TARGET_DIR` via environment variable.

## Disk space hygiene

Before building, check available space:
```bash
df -h /tmp
```

If `/tmp` is low, clean build artifacts:
```bash
rm -rf /tmp/l337-build /tmp/cargo-home
```

Never let cargo write into `./target/`. If a `target/` directory already exists
in the repo, it should be moved to `/tmp` or removed.

## Local build targets

Only build for **Linux** locally. All other platforms (Windows, macOS,
Android, iOS, etc.) are built and packaged exclusively by GitHub Actions
runners. Do not attempt cross-compilation or native builds for non-Linux
targets on this machine.

For cross-platform syntax and type checking without producing a binary,
`cargo check` is acceptable, but it must still use the `/tmp` cargo home as
documented above.

## Development Guidelines

When implementing new features:
- Seek implementations that are both feature-complete and reduce binary size through better abstraction
- Prefer generic solutions over duplicated logic (monomorphization can share more code than expected)
- Use Cargo features to make components optionally compilable
- Regularly audit binary size with `size -a ./bin/l337-audio-server` and `cargo bloat`
- Remember our core mission: an efficient, lightweight audio server - every feature should justify its cost

## Security

The audio server must take security very seriously in all aspects:
- Authentication and authorization are required for all sensitive endpoints
- Input validation and sanitization prevent injection and traversal attacks
- Output encoding avoids XSS and injection vulnerabilities
- Secure defaults for cryptographic operations and random number generation
- Regular dependency auditing with `cargo audit`
- Minimizing attack surface by disabling unnecessary features
- Following the principle of least privilege in all components
- Regular security reviews and threat modeling

## Dummy mode (no soundcard)

This server requires a soundcard at runtime by default. Dummy mode is an
**explicit opt-in** — there is **no automatic fallback**. If no audio
backend is available, audio initialization fails and the server exits with
an error telling the operator to set `dummy = true` in server.ini.

To enable dummy mode, set `dummy = true` in the `[server]` section of
server.ini, or pass the `--dummy` CLI flag:

```bash
./bin/l337-audio-server --dummy
```

In dummy mode `PlayerEngine::new_dummy()` is used: no cpal output stream is
opened, all API endpoints still work, and `pause()`/`stop()` are no-ops.
Tests already use `PlayerEngine::new_dummy()` so `cargo test` works
headless.

## Run

```bash
./scripts/run-server.sh            # runs bin/l337-audio-server or falls back to cargo run
L337__SERVER__PORT=1337 ./bin/l337-audio-server --dummy   # env-var config override
```

## Isolated testing on a live device

When the machine already runs a live audio server (e.g. a supervised systemd service
on its default socket), run test builds as **isolated instances** that never touch
the live server's socket, port, token, or runtime dir:

1. **Build the test binary to `/tmp`** (never the repo `target/` — see Build rules):

   ```bash
   CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/l337-build \
     cargo build --release --no-default-features --features alsa,opus
   cp -f /tmp/l337-build/release/l337-audio-server ./bin/l337-audio-server
   ```

2. **Create isolated runtime dirs:**

   ```bash
   rm -rf /tmp/l337-test && mkdir -p /tmp/l337-test/run /tmp/l337-test/state /tmp/l337-test/cache
   ```

3. **Start an isolated server on a unique socket/port with its own token:**

   ```bash
   setsid env \
     L337_RUNTIME_DIR=/tmp/l337-test/run \
     L337__SERVER__TOKEN=test-token-iso \
     L337__SERVER__PORT=13370 \
     L337__SERVER__DUMMY=true \
     RUST_LOG=l337_audio_server=debug \
     ./bin/l337-audio-server --transport=socket \
     > /tmp/l337-test/server.log 2>&1 < /dev/null &
   ```

   - `L337_RUNTIME_DIR` gives it its own socket path and instance lock
   - `L337__SERVER__PORT=13370` avoids colliding with the live port
   - `L337__SERVER__TOKEN` gives it its own auth token
   - `--dummy` means no audio hardware is touched
   - Everything lives under `/tmp/l337-test`, so no files conflict with the live install

4. **Test against it** (socket path is under your isolated runtime dir):

   ```bash
   curl --unix-socket /tmp/l337-test/run/l337.sock \
     -H "Authorization: Bearer test-token-iso" \
     http://localhost/player/status

   # or via HTTP on the isolated port
   curl -sk -H "Authorization: Bearer test-token-iso" \
     https://127.0.0.1:13370/version
   ```

5. **Stop when done:**

   ```bash
   pkill -f l337-audio-server
   rm -rf /tmp/l337-test
   ```

To test with real audio output instead of dummy mode, drop `--dummy` and set
`PIPEWIRE_RUNTIME_DIR`/`PULSE_SERVER` to your live user session's sockets — the
socket/port/token isolation above still keeps it separate from the running server
instance.

## Tests

```bash
cargo test
```

## Changelog / Release Notes

The file `CHANGELOG.md` at the repository root is the single source of truth
for release notes. Every code-changing change — including installer refactors
in `scripts/` — must add an entry under the `[Unreleased]` heading, grouped
under `Added`, `Changed`, `Removed`, or `Fixed`.

The `build.yml` `publish-release` job automatically extracts the
`[Unreleased]` section and passes it to `gh release create --notes-file`,
so any entry you add becomes the body of the next GitHub Release without
manual copy-paste. When a release is actually cut, promote the section by
renaming the heading from `## [Unreleased]` to `## [YYYY.MM.DD-N]`
(the CalVer tag scheme) and update the comparison links at the bottom of
the file.

Do not edit the auto-generated `release-notes.md` artifact directly; it is
created at build time from `CHANGELOG.md`.

## Install (systemd)

```bash
sudo ./install.sh --dry-run   # preview
sudo ./install.sh             # install
```

## Playback Duration Match Test (Critical)

Before considering playback fixes complete, the MP3 playback duration must match
ffprobe to the nearest second. Use `scripts/match_mp3_duration.sh`:

```bash
# Build and start server on isolated port 13370
CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/l337-build \
  cargo build --release --features backend,opus
cp -f /tmp/l337-build/release/l337-audio-server bin/l337-audio-server

rm -rf /tmp/l337-test && mkdir -p /tmp/l337-test/run /tmp/l337-test/state /tmp/l337-test/cache
cp "/path/to/test.mp3" /tmp/l337-test/dispatch.mp3

# For faster testing, cut test files to 5s / 30s / 120 seconds:
# ffmpeg -i test.mp3 -t 5  -c copy test_5.mp3
# ffmpeg -i test.mp3 -t 30 -c copy test_30.mp3
# ffmpeg -i test.mp3 -t 120 -c copy test_120.mp3

L337_RUNTIME_DIR=/tmp/l337-test/run \
L337__SERVER__TOKEN=test-token-iso \
L337__SERVER__PORT=13370 \
L337__SERVER__DUMMY=false \
./bin/l337-audio-server --transport=http &

# Run duration match test — start with smallest file:
./scripts/match_mp3_duration.sh 13370 test-token-iso /tmp/l337-test/dispatch_5.mp3

# Then 30s, then 120s:
./scripts/match_mp3_duration.sh 13370 test-token-iso /tmp/l337-test/dispatch_30.mp3
./scripts/match_mp3_duration.sh 13370 test-token-iso /tmp/l337-test/dispatch_120.mp3
```

**Pass criteria:** `position_sec` at `stopped` state equals ffprobe duration within
±2 seconds. Any choppiness, buffer issues, or wrong playback speed will be
revealed by a mismatch.
```

## MPD position-tracking reference

MPD (https://github.com/MusicPlayerDaemon/MPD) does NOT derive playback position
from buffer consumption (`read_pos` / `consumed_total`). Instead, each decoder
plugin reports elapsed time per decoded chunk (`MadDecoder::elapsed_time` via
`mad_timer_add` on `frame.header.duration`), and `MultipleOutputs::CheckPipe()`
reads `chunk->time` when consuming each chunk to update `elapsed_time`. Position =
decoder-reported time per consumed chunk, not buffer bytes / sample rate. This is
more accurate because the decoder knows exactly how much audio each chunk
represents — over-decoding trailing container garbage doesn't inflate position.
Consider adopting this pattern if `position_sec` derived from
`consumed_total + read_pos` continues to drift past `duration_sec`.
