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

## Runtime Dependencies

- **yt-dlp** — required for YouTube URL playback. Install separately:
  - Debian/Ubuntu: `sudo apt install yt-dlp`
  - macOS: `brew install yt-dlp`
  - pip: `pip install yt-dlp`

The server does not bundle `yt-dlp`. If it is missing, YouTube URLs will return an error and the `/health` endpoint will show `yt_dlp: false`.
