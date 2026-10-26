# l337-audio-server
A simple rust based audio server designed to handle playing music or podcasts with 3rd party plugin extensibility

## Quick Install

No clone required. One command for all platforms:

```bash
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/install.sh | sudo bash -s -- --force
```

Or with `wget`:
```bash
wget -qO- https://github.com/tim-projects/l337-audio-server/raw/main/install.sh | sudo bash -s -- --force
```

`install.sh` auto-detects your OS and audio backend (PipeWire, ALSA, or macOS), downloads the matching binary, and sets up the service.

## Install Options

| Flag | Description |
|------|-------------|
| `--force` | Force reinstall even if the same version is already installed |
| `--no-audio` | Run without audio hardware (`dummy = true`) |
| `--dry-run` | Show what would happen without making changes |
| `--uninstall` | Remove the service and installed files |
| `--remove-data` | Also remove configuration and data directories |
| `--group-access` | *(ALSA only)* Add local users to the service group so they can reach the Unix socket |

## Post-Install

### Check Status
```bash
# Linux systemd service
sudo systemctl status l337-audio-server.service

# Linux user service (PipeWire)
systemctl --user status l337-audio-server.service

# macOS launchd
launchctl list | grep com.l337.audio-server
```

### View Logs
```bash
# Linux systemd
sudo journalctl -u l337-audio-server.service -f

# Linux user service (PipeWire)
journalctl --user -u l337-audio-server.service -f

# macOS
tail -f ~/Library/Logs/com.l337.audio-server.log
```

### Configuration
- **Linux systemd**: `/etc/l337-audio-server/server.ini`
- **Linux user service**: `~/.config/l337-audio-server/server.ini`
- **macOS**: `~/Library/Application Support/l337-audio-server/server.ini`

## Uninstall

```bash
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/install.sh | sudo bash -s -- --uninstall
```

Or remove data too:
```bash
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/install.sh | sudo bash -s -- --uninstall --remove-data
```

Windows (from the checkout, or via Add/Remove Programs):
```powershell
.\install.ps1 -Uninstall
.\install.ps1 -Uninstall -RemoveData
```

On every platform `--uninstall` stops the service, removes the systemd unit /
launchd agent, the binary and the runtime directory (Unix socket and instance
lock), and refuses with the offending PID if a manually started instance is
still running — stop it and re-run. Configuration, state, cache and logs are
**kept**; add `--remove-data` to delete them. Add `--dry-run` to print the plan
without changing anything.

### Termux (Android)

Termux is a **user-space** install — there is no `sudo` and no root, and it uses
runit (`termux-services`) rather than systemd.

Install (no `sudo`; open a **new Termux session** the first time so runit is
running):
```bash
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/install.sh | bash -s -- --pre-release
```

The installer installs what it needs by itself — `termux-services` and
`pulseaudio`, and it starts the PulseAudio daemon if it is not running — so no
manual step is required. Pass `--no-install-deps` to opt out and be told what to
run instead. The only step it cannot do for you is opening a **new Termux
session** after a fresh `termux-services` install (runit only starts with a new
session).

Status, logs and config:
```bash
sv status l337-audio-server     # run: / down: / exit:
sv-enable l337-audio-server     # auto-start on Termux launch
cat ~/.cache/l337/l337.log      # server log
# config lives in the service dir, which is also the service's CWD
cat $PREFIX/var/service/l337-audio-server/server.ini
```

Uninstall:
```bash
# from a checkout
bash scripts/install-android-termux.sh --uninstall

# or via install.sh
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/install.sh | bash -s -- --uninstall
```

`--uninstall` stops the service and removes the service directory, the binary,
the socket symlink and the runtime directory. Playlists, the auth token and
logs are **kept**, as is your `server.ini` (preserved and restored on the next
install, so `dummy = true`/`false` and a custom port survive). Add
`--remove-data` to delete that data as well.

Afterwards you are asked, one package at a time, whether to also remove
`termux-services` and `pulseaudio` (both are general-purpose packages that
other software uses, so the default is to keep them). Piped through
`curl | bash` there is nobody to ask, so they are kept and the removal command
is printed instead.

Flags are shared where the platforms overlap: `--pre-release`, `--uninstall`,
`--remove-data` and `--help` work on all of them; `--force`, `--no-audio`,
`--dry-run` and `--group-access` are ALSA/systemd only. `--no-install-deps` is
Termux-only — only that installer installs its own dependencies.

## Runtime Dependencies

- **yt-dlp** — required for YouTube URL playback. Install separately:
  - Debian/Ubuntu: `sudo apt install yt-dlp`
  - macOS: `brew install yt-dlp`
  - pip: `pip install yt-dlp`

The server does not bundle `yt-dlp`. If it is missing, YouTube URLs will return an error and the `/health` endpoint will show `yt_dlp: false`.

## Troubleshooting

### Installer falls back to the wrong backend or won’t run

If `install.sh` auto-detection picks the wrong installer, or the downloaded
script fails immediately, install the platform script directly instead:

```bash
# Linux PipeWire
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/scripts/install-linux-pipewire.sh | sudo bash

# Linux ALSA
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/scripts/install-linux-alsa.sh | sudo bash

# macOS
curl -fsSL https://github.com/tim-projects/l337-audio-server/raw/main/scripts/install-macos.sh | bash
```

### Service fails to start after install

```bash
# Linux systemd
sudo systemctl status l337-audio-server.service
sudo journalctl -u l337-audio-server.service -n 50

# Linux user service (PipeWire)
systemctl --user status l337-audio-server.service
journalctl --user -u l337-audio-server.service -n 50

# macOS
tail -n 50 ~/Library/Logs/com.l337.audio-server.log
```

Common causes:
- No audio device present: use `--no-audio` to enable dummy mode.
- Port 1337 already in use: change `port` in `server.ini`.
- Missing token or bad permissions: ensure `server.ini` is owned by the service user and mode `0600`/`0640`.

### Binary or script download errors

```bash
# Verify the downloaded file is a valid binary
file /opt/l337-audio-server/.tmp/l337-audio-server.new

# Re-download with verbose output
curl -fL --retry 2 --retry-delay 2 -o /tmp/l337.new \
  https://github.com/tim-projects/l337-audio-server/releases/latest/download/l337-audio-server-x86_64-linux-pipewire
```

### Rollback left the install in a bad state

```bash
# Restore manually if needed
sudo systemctl stop l337-audio-server.service || true
sudo mv /opt/l337-audio-server/l337-audio-server.bak /opt/l337-audio-server/l337-audio-server
sudo systemctl daemon-reload
sudo systemctl start l337-audio-server.service
```