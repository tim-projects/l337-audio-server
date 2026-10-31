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

### Changed
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

### Fixed
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
