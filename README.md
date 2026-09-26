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

## Runtime Dependencies

- **yt-dlp** — required for YouTube URL playback. Install separately:
  - Debian/Ubuntu: `sudo apt install yt-dlp`
  - macOS: `brew install yt-dlp`
  - pip: `pip install yt-dlp`

The server does not bundle `yt-dlp`. If it is missing, YouTube URLs will return an error and the `/health` endpoint will show `yt_dlp: false`.
