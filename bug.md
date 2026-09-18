[1;34m[INFO][0m Platform: linux / x86_64
[1;34m[INFO][0m Checking for latest release...
[1;34m[INFO][0m Latest release: 2026.09.17-6 (published: 2026-09-17T13:45:53Z)
[1;34m[INFO][0m Detected PipeWire mode: none
[1;34m[INFO][0m No PipeWire detected; installing system-wide with ALSA.
[1;34m[INFO][0m Selected asset: l337-audio-server-x86_64-linux-alsa
[1;34m[INFO][0m Existing installation found at: /opt/l337-audio-server/l337-audio-server
[1;34m[INFO][0m Installed version: 0.1.0
[1;34m[INFO][0m Update available (0.1.0 -> 2026.09.17-6)
[1;34m[INFO][0m Downloading: https://github.com/tim-projects/l337-audio-server/releases/download/2026.09.17-6/l337-audio-server-x86_64-linux-alsa
[1;32m[OK][0m   Downloaded: /opt/l337-audio-server/.tmp/l337-audio-server.new (15M)
[1;34m[INFO][0m Configuring systemd service...
[1;34m[INFO][0m Ensuring configuration at /etc/l337-audio-server/server.ini...
[1;32m[OK][0m   Configuration ready at /etc/l337-audio-server/server.ini
[1;34m[INFO][0m Writing systemd unit: /etc/systemd/system/l337-audio-server.service
[1;34m[INFO][0m Replacing binary...
[1;32m[OK][0m   Systemd service installed and started
[1;32m[OK][0m   Installation complete

Next steps:
  Check status:    systemctl status l337-audio-server.service
  View logs:       journalctl -u l337-audio-server.service -f
  Configuration:   /etc/l337-audio-server/server.ini
