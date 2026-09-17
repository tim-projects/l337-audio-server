[1;34m[INFO][0m Platform: linux / x86_64
[1;34m[INFO][0m Checking for latest release...
[1;34m[INFO][0m Latest release: 2026.09.16-7 (published: 2026-09-16T18:01:52Z)
[1;34m[INFO][0m Detected PipeWire mode: none
[1;34m[INFO][0m No PipeWire detected; installing system-wide with ALSA.
[1;34m[INFO][0m Selected asset: l337-audio-server-x86_64-linux-alsa
[1;34m[INFO][0m Existing installation found at: /opt/l337-audio-server/l337-audio-server
[1;34m[INFO][0m Installed version: 0.1.0
[1;34m[INFO][0m Update available (0.1.0 -> 2026.09.16-7)
[1;34m[INFO][0m Downloading: https://github.com/tim-projects/l337-audio-server/releases/download/2026.09.16-7/l337-audio-server-x86_64-linux-alsa
[1;32m[OK][0m   Downloaded: /opt/l337-audio-server/.tmp/l337-audio-server.new (15M)
[1;34m[INFO][0m Configuring systemd service...
[1;34m[INFO][0m Ensuring configuration at /etc/l337-audio-server/server.ini...
[1;32m[OK][0m   Configuration ready at /etc/l337-audio-server/server.ini
[1;34m[INFO][0m Writing systemd unit: /etc/systemd/system/l337-audio-server.service
[1;34m[INFO][0m Replacing binary...
Sep 17 01:24:03 pipelinepro systemd[1]: Started l337-audio-server.service - L337 Audio Server.
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: 2026-09-17T01:24:03.293723Z  WARN l337_audio_server: Could not create default server.ini: Permission denied (os error 13)
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib confmisc.c:855:(parse_card) cannot find card '0'
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib conf.c:5205:(_snd_config_evaluate) function snd_func_card_inum returned error: No such file or directory
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib confmisc.c:422:(snd_func_concat) error evaluating strings
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib conf.c:5205:(_snd_config_evaluate) function snd_func_concat returned error: No such file or directory
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib confmisc.c:1342:(snd_func_refer) error evaluating name
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib conf.c:5205:(_snd_config_evaluate) function snd_func_refer returned error: No such file or directory
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib conf.c:5728:(snd_config_expand) Evaluate error: No such file or directory
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: ALSA lib pcm.c:2722:(snd_pcm_open_noupdate) Unknown PCM default
Sep 17 01:24:03 pipelinepro l337-audio-server[128154]: 2026-09-17T01:24:03.303399Z ERROR l337_audio_server: Failed to initialize audio device: snd_pcm_open failed: No such file or directory
Sep 17 01:24:03 pipelinepro systemd[1]: l337-audio-server.service: Main process exited, code=exited, status=1/FAILURE
Sep 17 01:24:03 pipelinepro systemd[1]: l337-audio-server.service: Failed with result 'exit-code'.
Sep 17 01:24:05 pipelinepro systemd[1]: l337-audio-server.service: Scheduled restart job, restart counter is at 1.
Sep 17 01:24:05 pipelinepro systemd[1]: Started l337-audio-server.service - L337 Audio Server.
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: 2026-09-17T01:24:05.816560Z  WARN l337_audio_server: Could not create default server.ini: Permission denied (os error 13)
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib confmisc.c:855:(parse_card) cannot find card '0'
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib conf.c:5205:(_snd_config_evaluate) function snd_func_card_inum returned error: No such file or directory
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib confmisc.c:422:(snd_func_concat) error evaluating strings
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib conf.c:5205:(_snd_config_evaluate) function snd_func_concat returned error: No such file or directory
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib confmisc.c:1342:(snd_func_refer) error evaluating name
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib conf.c:5205:(_snd_config_evaluate) function snd_func_refer returned error: No such file or directory
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib conf.c:5728:(snd_config_expand) Evaluate error: No such file or directory
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: ALSA lib pcm.c:2722:(snd_pcm_open_noupdate) Unknown PCM default
Sep 17 01:24:05 pipelinepro l337-audio-server[128173]: 2026-09-17T01:24:05.842602Z ERROR l337_audio_server: Failed to initialize audio device: snd_pcm_open failed: No such file or directory
Sep 17 01:24:05 pipelinepro systemd[1]: l337-audio-server.service: Main process exited, code=exited, status=1/FAILURE
Sep 17 01:24:05 pipelinepro systemd[1]: l337-audio-server.service: Failed with result 'exit-code'.
