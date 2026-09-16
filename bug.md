vscode@pipelinepro:~/git/l337-audio-server$ sudo bash install.sh  --pre-release
[INFO] Platform: linux / x86_64
[INFO] Checking for latest release...
[INFO] Latest release: 2026.09.16-5 (published: 2026-09-16T14:35:50Z)
[INFO] Detected PipeWire mode: none
[INFO] No PipeWire detected; installing system-wide with ALSA.
[INFO] Selected asset: l337-audio-server-x86_64-linux-alsa
[INFO] Existing installation found at: /opt/l337-audio-server/l337-audio-server
[INFO] Installed version: 0.1.0
[INFO] Update available (0.1.0 -> 2026.09.16-5)
[INFO] Downloading: https://github.com/tim-projects/l337-audio-server/releases/download/2026.09.16-5/l337-audio-server-x86_64-linux-alsa
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed
  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0
100 14.4M  100 14.4M    0     0  9615k      0  0:00:01  0:00:01 --:--:-- 20.0M
[OK]   Downloaded: /opt/l337-audio-server/.tmp/l337-audio-server.new (15M)
[INFO] Configuring systemd service...
[INFO] Ensuring configuration at /etc/l337-audio-server/server.ini...
[OK]   Configuration ready at /etc/l337-audio-server/server.ini
[INFO] Writing systemd unit: /etc/systemd/system/l337-audio-server.service
[INFO] Replacing binary...
Created symlink '/etc/systemd/system/multi-user.target.wants/l337-audio-server.service' → '/etc/systemd/system/l337-audio-server.service'.
Sep 16 15:18:59 pipelinepro systemd[1]: Started l337-audio-server.service - L337 Audio Server.
Sep 16 15:18:59 pipelinepro l337-audio-server[183797]: 2026-09-16T15:18:59.438720Z DEBUG l337_audio_server::platform::single_instance: Lock path /home/l337/.cache/l337/l337-audio-server/instance.lock: cannot create parent dir: Permission denied (os error 13)
Sep 16 15:18:59 pipelinepro l337-audio-server[183797]: 2026-09-16T15:18:59.441452Z  WARN l337_audio_server: Could not create default server.ini: Permission denied (os error 13)
Sep 16 15:18:59 pipelinepro l337-audio-server[183797]: thread 'main' (183797) panicked at src/main.rs:166:40:
Sep 16 15:18:59 pipelinepro l337-audio-server[183797]: Failed to load config: missing configuration field "server"
Sep 16 15:18:59 pipelinepro l337-audio-server[183797]: note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
Sep 16 15:18:59 pipelinepro systemd[1]: l337-audio-server.service: Main process exited, code=exited, status=101/n/a
Sep 16 15:18:59 pipelinepro systemd[1]: l337-audio-server.service: Failed with result 'exit-code'.
[WARN] New binary failed to start. Rolling back...
[FAIL] Rollback failed — manual recovery required
