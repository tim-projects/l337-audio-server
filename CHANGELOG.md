# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(although the release tag itself follows a CalVer `YYYY.MM.DD-N` scheme).

Entries under `[Unreleased]` accumulate during development and are published
under the release tag when `build.yml` runs the `publish-release` job — the
workflow extracts this section automatically and uses it as the GitHub Release
body. When a release is cut, promote the section by replacing `[Unreleased]`
with the new version heading.

## [Unreleased]

### Added
- Linux installers now detect the distribution from `/etc/os-release`
  (Debian/Ubuntu, Fedora/RHEL, Arch, Alpine) and cross to the matching package
  manager (`apt-get`, `dnf`, `pacman`, `zypper`, `apk`).
- New shared library `scripts/lib-distro.sh` centralises distro detection,
  package-manager discovery, package-name mapping, base-tool installation and
  runtime-library verification. Used by all three Linux installers.
- `install.sh` now exports `REPO` and `SCRIPT_TAG` so platform installers can
  download `lib-distro.sh` at runtime.
- `--no-install-deps` flag is now supported by all Linux installers and
  `install.sh` (previously Termux-only). Opt out of automatic system-package
  installation; missing packages are printed with their exact install command.
- Base tools (`curl`, `file`, `readelf`/`binutils`, `ldconfig`, `groupadd`,
  `useradd`) are now verified and auto-installed before first boot.
- Audio backend libraries are installed up-front via `ensure_audio_libs`
  (ALSA → `libasound2`+`libopus0`; PipeWire → `libpipewire-0.3-0`+`libopus0`+
  `libpulse0` on Debian, with equivalents on other distros).
- `CHANGELOG.md` — the release workflow now uses this file's `[Unreleased]`
  section as the GitHub Release notes body.
- `install.sh` now walks releases newest-first and picks the newest one that
  actually publishes this platform's binary — reading each release's
  `release-info.json` and comparing the recorded glibc against the running libc,
  so a partial release (a targeted CI build that ships only some targets) and a
  binary needing a newer glibc are both skipped instead of failing with
  `curl: (22) ... 404` at download time.
- `install.sh` downloads the binary itself into `TMPDIR` and hands the local path
  to the platform installer with `--binary <path>` (and `--release <tag>`).
- `--force`/`-f` flag on `install.sh`: reinstall even when the installed version
  already matches the release being installed.
- `install.sh` only trusts a local `scripts/install-*.sh` when it sits in a real
  repository checkout (a directory containing `.git`); a bare directory-relative
  path is never executed. The documented `curl ... install.sh | sudo bash` runs
  the script from stdin, so a CWD-relative path would otherwise be an
  attacker-placeable file executed as root.
- `PlayerStatus.time` — a human playhead (`"m:ss/m:ss"`, position/duration) for
  progress-bar front-ends that read exactly that shape. A front-end reading only
  the `position_sec`/`duration_sec` ints can still build it; one reading only
  `time` previously got no progress at all. Falls back to the position alone when
  no duration is known yet (a seek of a track without metadata).
- `/health` now advertises the real capabilities rather than only `yt_dlp`:
  `capabilities.codecs` lists the codecs this binary can decode (Opus only when
  compiled with `--features opus`) and `capabilities.quality` signals the G2
  bandwidth API, so a front-end can drop its "assume AAC only" stopgap and expose
  quality presets.

### Changed
- **Platform installers now enable, start and verify the service.**
  - Termux: `sv-enable` makes the service come back on its own when Termux is
    restarted, and the install verifies the service is supervised *and* that the
    server actually created its listening endpoint — the Unix socket the
    `--transport=socket` build uses — reporting the PID and the socket path.
  - Linux ALSA / PipeWire and the local-build systemd installer poll
    (`wait_until_ok`, new in `scripts/lib-distro.sh`) until the unit reports
    active instead of assuming a fixed `sleep` is enough, so a healthy service
    is no longer misreported as failed (and rolled back) on a slow host.
  - macOS loads the LaunchAgent with `launchctl bootstrap gui/<uid>` (falling
    back to `launchctl load` on older releases) and verifies the job is
    registered before declaring success; `bootout` is used on uninstall.
- `install.sh` now reports the glibc version found on a **termux-glibc** device.
  The dynamic loader in `$PREFIX/glibc/lib` answers for the system `ldd`/`getconf`
  (Bionic, silent), so glibc is detected instead of reported as absent, and the
  version comparison ignores the distro package revision (`2.44-0` compares as
  `2.44`). A Bionic-only Termux device says so instead of warning it cannot tell.
- A standalone run of a platform installer without `--binary` now fails with an
  actionable message on every installer (previously only Termux had it, and the
  others died deeper in on an empty path).

### Fixed
- **Playback wrote silence for every track not at 48 kHz** (device P0). Proven by
  isolation: an identical 44.1 kHz WAV decoded to a healthy, running PulseAudio
  sink-input with `codec=pcm_s16le` and correct duration, but the monitor read a
  peak of exactly 0 — only the rate-conversion path differed. Two defects in one
  path: `rubato::SincFixedIn` takes a *fixed* input chunk and returns its result
  **channel-planar**, so the streaming producer fed each ~1024-frame packet
  straight into a 4096-frame chunk resampler — every call failed validation and
  its `if let Ok(...)` guard dropped the packet, so a whole 44.1 kHz stream
  reached the sink as nothing at all; and the file path appended
  `output_planar[ch]` per channel, i.e. `LLL...RRR...`, where the stream format
  needs interleaved `L R L R ...`. Both paths now share one
  `InterleavedResampler`: it buffers any input length, always advances (a chunk
  is never re-fed and never stalls the stream), and returns interleaved output,
  with `finish()` flushing the zero-padded tail so a rate change no longer
  shortens a track. Unit tests pin 44100→48000 to non-zero with length
  ≈ `input × ratio`, pin the interleave, and pin that odd-size packets
  (1024/512/2048/128) deliver the same audio as the whole buffer.
- **`stop()` never tore the stream down** (device P1), so the sink-input survived
  `POST /player/stop` and the paused writer kept appending keep-alive silence to
  it — the sink-input's `Buffer Latency` grew to ~1000 s draining only at 0.03×,
  which is the "9975 s backlog" number of the original report. `stop()` now stops
  and joins the backend stream (freeing the sink-input) instead of pausing it,
  and `ensure_stream()` reopens one at each playback entry point so a decoupled
  writer thread is never missing while the buffer fills.
- The PulseAudio writer's keep-alive silence was unpaced: it wrote a 4096-sample
  chunk every ~10 ms (≈8× real time) so a *paused* stream reproduced the
  unbounded-backlog symptom. Keep-alive writes are now paced to the device clock
  like audio, so latency stays bounded in PulseAudio rather than growing.
- **`best` quality under-selected the rendition** (BUG-4). yt-dlp's own
  `bestaudio` ranking resolved YouTube's "best" to a ~129 kbps AAC m4a (itag 140)
  even when a higher-bitrate Opus (itag 251) was available, so the default
  silently degraded everyone's audio. Rendition choice now comes from
  `yt-dlp --dump-single-json` through one policy function
  (`select_audio_format`): the **highest average bitrate** audio rendition at or
  below the ceiling (0 = best), the lowest available when nothing fits (a ceiling
  never fails playback), audio-only renditions preferred over video renditions
  that merely carry audio, and never a rendition with no audio at all. Resolution
  and download share the same call, so a play and its download can no longer pick
  different renditions. Tests cover the YouTube AAC/Opus ladder and each ceiling.
- The device Python's third-party deps (`httpx`, `pytest`, `textual`) are gone
  after a rebuild, so player-side end-to-end checks now run as a stdlib-only
  HTTP client against the Unix socket.
- The new `InterleavedResampler` originally drained its consumed prefix from
  `pending` once per chunk, which is quadratic in track length on the whole-file
  path (`load_and_play`, `seek`, `seek_and_resume` all pass the whole track in
  one call): a 4-minute 44.1 kHz track moved ~110 GB of samples before the first
  sound. It now consumes by cursor and compacts once (**measured 330 ms → 36 µs**
  for 30 s of audio).
- Rendition selection degrades instead of failing when yt-dlp's
  `--dump-single-json` output is unusable (older yt-dlp, a proxy, any shape
  change): it falls back to the classic `-f bestaudio[abr<=N]/bestaudio --
  print` pair, so a JSON-format regression costs quality reporting rather than
  playback.
- `ensure_stream()` now records a backend failure the way a writer does
  (buffer `backend_error` + `Stopped`) instead of only logging it. Without this a
  failed restart left the buffer filling with no writer to drain it, which read
  as `Playing` forever rather than a visible stop.
- `get_status` builds `PlayerStatus` once instead of in two hand-mirrored
  literals, so a field cannot land in one branch and not the other.
- **The download was never verified**, so a truncated or corrupted transfer could
  be installed silently as a valid-looking (still ELF-headed) binary. Every
  release now publishes a per-asset checksum block (`md5`, `crc32`, `size`) in
  `release-info.json`, and `install.sh` verifies the download before installing.
- **`md5sum` was a hard dependency**: a host without coreutils failed the install
  even though the checksum was published. Verification is now layered and never
  hard-fails on a missing tool: byte size always (needs no tool), then `md5sum`
  when present, then POSIX `cksum` (crc32 — shipped by both coreutils **and**
  busybox, so it covers hosts without md5sum). With neither tool it verifies size
  only and says exactly which check was applied. A pure-bash hash is not
  possible: bash variables cannot hold NUL bytes, and a per-byte loop measures
  ~4 KB/s (about an hour for a 15 MB binary).
- `cksum`'s byte count was read from the wrong output field
  (`CRC SIZE FILENAME`), so the length cross-check always failed on hosts without
  `md5sum`.
- `scripts/install-android-termux.sh` re-read `L337_BINARY_PATH` **after**
  argument parsing, so it overwrote the `--binary` path install.sh had just
  passed and every install ended in "No binary given" after the download.
  The environment fallback is now resolved before parsing, as in the other
  installers.
- The install-time verification probed TCP `127.0.0.1:<port>`, but the Termux
  build runs with `--transport=socket` and never opens that port, so a working
  install always reported "not answering". It now verifies the Unix socket.
- `--force` re-downloading is now stated explicitly in the output
  ("Force reinstall requested ... — re-downloading").
- **All GitHub access moved out of the platform installers into `install.sh`.**
  `install-android-termux.sh`, `install-linux-alsa.sh`, `install-linux-pipewire.sh`
  and `install-macos.sh` no longer query the GitHub API, walk releases, compare
  versions or download anything: they receive the binary path and register the
  service. The duplicated release-walk, version-comparison and download code
  (five copies with drifting behaviour) is gone; running a platform installer
  without `--binary` fails with an explanatory message, and `--uninstall` still
  works standalone.
- macOS asset selection now matches `install-macos.sh`, which installs the
  x86_64 build under Rosetta 2 on Apple Silicon: `install.sh` requires
  `l337-audio-server-x86_64-apple-darwin` rather than the aarch64 asset.
- `install_verification_tools()` in `install-systemd.sh` refactored to use the
  shared library's per-distro dispatch instead of its own duplicated table.

- **Termux/macOS installers resolved the latest release unconditionally**, so a
  release that did not publish this platform's binary died with a 404. Against
  the real API the binary download 404s when the newest release only carried
  other targets. `install.sh` now owns the walk and verifies the asset against
  the release's own `release-info.json`, so only installable releases are picked.
- A partial download left behind by an interrupted run could be mistaken for a
  good binary: `curl -f` never truncates its `-o` target on failure, so a stale
  non-empty file passed the "is it non-empty?" test and was installed as a
  complete binary (it still begins with the ELF magic, so format checks miss it).
  Success is now decided by the transfer status, and the destination is removed
  before each attempt.
- `curl ... install.sh | sudo bash` would prefer a `scripts/install-*.sh` from
  the caller's working directory: the script arrives on stdin, so `$0` is `bash`
  and a directory-relative path resolved wherever the user ran it, executing that
  file as root. Only a real repository checkout is trusted now.
- On Apple Silicon `install.sh` required the aarch64 asset while the macOS
  installer installs the x86_64 build under Rosetta 2, so the two disagreed
  about which release to use. The names now match.
- `install.sh` called `ok()` which was never defined (a `command not found`
  crash) on the version-check path.
- An unparseable `--version` output (e.g. a locally built binary reporting
  `0.0.0-dev`) compared as "newer than release" and silently blocked every
  future update; the installed version is now only compared when it is a
  well-formed `YYYY.MM.DD-N` release tag, and reinstalled otherwise.
- On Termux the installed-version check never saw a version: the Android binary
  has no `DT_RUNPATH`, so `libpulse.so` is only resolved via `LD_LIBRARY_PATH`
  and even `--version` failed without it. Both install paths now set it for the
  version probe, so the up-to-date check actually works.
- Volume control makes software gain the single authoritative volume path
  (fixes a regression where hardware-volume requests were silently dropped).
- `track_id` is now validated to prevent injection through the streaming
  endpoint.

## [2026.10.27-1] — 2026-10-27

### Security
- Accept the bare token on the `X-L337-Token` and `Authorization` headers.

### Fixed
- `wait_for_progress` used an unqualified `Path` (E0425) after refactoring.
- Address all branch-review findings (see `bugs.md`).

### Packaging
- Commit `opus-stubs/libopus.so` (was gitignored); wire runtime-dependency
  check into the Linux installers.
- Install `libclang` for bindgen in the Linux CI jobs.
- Fix dummy-mode config mismatch (see `bugs.md` host-run table).

[Unreleased]: https://github.com/tim-projects/l337-audio-server/compare/2026.10.27-1...HEAD
[2026.10.27-1]: https://github.com/tim-projects/l337-audio-server/releases/tag/2026.10.27-1
