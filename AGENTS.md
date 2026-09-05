# L337 Audio Server — Agent Instructions

## Build

**Disk space is tight.** All cargo artifacts (registry, target dir, build cache) must live under `/tmp`. The in-repo `target/` must not be used for builds. Do not run `cargo build`, `cargo check`, or `cargo test` directly — they will write large temp files into `./target` and exhaust disk space.

Always use the project build script. Do not background builds:

```bash
CARGO_HOME=/tmp/cargo-home ./scripts/build.sh
```

The script sets `CARGO_TARGET_DIR=/tmp/l337-build` and copies the finished binary to `bin/l337-audio-server` for deployment. `cargo check` is acceptable because it does not produce a linked binary and uses far less disk.

If you need to verify code compiles without producing a binary, use:
```bash
CARGO_HOME=/tmp/cargo-home cargo check
```

Do not create a `.cargo/config.toml` in the repo. The build script already handles `CARGO_TARGET_DIR` via environment variable.

## Disk space hygiene

Before building, check available space:
```bash
df -h /tmp
```

If `/tmp` is low, clean build artifacts:
```bash
rm -rf /tmp/l337-build /tmp/cargo-home
```

Never let cargo write into `./target/`. If a `target/` directory already exists in the repo, it should be moved to `/tmp` or removed.

## Local build targets

Only build for **Linux** locally. All other platforms (Windows, macOS, Android, iOS, etc.) are built and packaged exclusively by GitHub Actions runners. Do not attempt cross-compilation or native builds for non-Linux targets on this machine.

For cross-platform syntax and type checking without producing a binary, `cargo check` is acceptable, but it must still use the `/tmp` cargo home as documented above.

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
