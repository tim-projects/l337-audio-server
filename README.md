# l337-audio-server
A simple rust based audio server designed to handle playing music or podcasts with 3rd party plugin extensibility

## Quick Install

No clone required. Pick the command for your platform:

### Linux
```bash
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/install.sh | sudo bash -s -- --pre-release
```

### macOS
```bash
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/install.sh | bash -s -- --pre-release
```

Or with `wget`:
```bash
wget -qO- https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/install.sh | sudo bash -s -- --pre-release
```

## Install Options

| Flag | Description |
|------|-------------|
| `--pre-release` | Install the latest prerelease instead of stable |
| `--force` | Force reinstall even if the same version is already installed |
| `--no-audio` | Run without audio hardware (`dummy = true`) |
| `--dry-run` | Show what would happen without making changes |
| `--uninstall` | Remove the service and installed files |
| `--remove-data` | Also remove configuration and data directories |

## Platform-Specific Installers

If you prefer to call the platform installer directly:

### Linux PipeWire
```bash
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/scripts/install-linux-pipewire.sh | sudo bash
```

### Linux ALSA
```bash
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/scripts/install-linux-alsa.sh | sudo bash
```

### macOS
```bash
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/scripts/install-macos.sh | bash
```

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
# Linux
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/install.sh | sudo bash -s -- --uninstall

# macOS
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/install.sh | bash -s -- --uninstall
```

Or remove data too:
```bash
curl -fsSL https://raw.githubusercontent.com/tim-projects/l337-audio-server/main/install.sh | sudo bash -s -- --uninstall --remove-data
```

## Runtime Dependencies

- **yt-dlp** — required for YouTube URL playback. Install separately:
  - Debian/Ubuntu: `sudo apt install yt-dlp`
  - macOS: `brew install yt-dlp`
  - pip: `pip install yt-dlp`

The server does not bundle `yt-dlp`. If it is missing, YouTube URLs will return an error and the `/health` endpoint will show `yt_dlp: false`.
