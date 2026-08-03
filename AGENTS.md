# L337 Audio Server — Agent Instructions

## Build

Disk space is tight. All cargo artifacts (registry, target dir, build cache) must live under `/tmp` to keep the in-repo `target/` small. Do not let cargo write into `./target`.

Always use the project build script. Do not run cargo directly and do not background builds:

```bash
CARGO_HOME=/tmp/cargo-home ./scripts/build.sh
```

The script sets `CARGO_TARGET_DIR=/tmp/l337-build` and copies the finished binary to `bin/l337-audio-server` for deployment.

## Development Guidelines

When implementing new features:
- Seek implementations that are both feature-complete and reduce binary size through better abstraction
- Prefer generic solutions over duplicated logic (monomorphization can share more code than expected)
- Use Cargo features to make components optionally compilable
- Regularly audit binary size with `size -a ./bin/l337-audio-server` and `cargo bloat`
- Remember our core mission: an efficient, lightweight audio server - every feature should justify its cost

## Dummy mode (no soundcard)

This server requires a soundcard at runtime. On headless / CI / dev boxes without audio hardware, run with `--dummy`:

```bash
./bin/l337-audio-server --dummy
```

In dummy mode `PlayerEngine::new_dummy()` is used: no cpal output stream is opened, all API endpoints still work, and `pause()`/`stop()` are no-ops. Tests already use `PlayerEngine::new_dummy()` so `cargo test` works headless.

## Run

```bash
./scripts/run-server.sh            # runs bin/l337-audio-server or falls back to cargo run
L337__SERVER__PORT=1337 ./bin/l337-audio-server --dummy   # env-var config override
```

## Tests

```bash
cargo test
```

## Install (systemd)

```bash
sudo ./install.sh --dry-run   # preview
sudo ./install.sh             # install
```
