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

### Changed
- `install_verification_tools()` in `install-systemd.sh` refactored to use the
  shared library's per-distro dispatch instead of its own duplicated table.

### Fixed
- Volume control makes software gain the single authoritative volume path
  (fixes a regression where hardware-volume requests were silently dropped).
- `track_id` is now validated to prevent injection through the streaming
  endpoint.
- `install.sh` install path now picks the newest release that actually ships a
  binary for the caller's platform (was selecting a release that may omit certain
  architectures).

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
