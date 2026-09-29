(99%) tim@archvm:~$ /opt/l337-audio-server/l337-audio-server --version
ALSA lib confmisc.c:855:(parse_card) [error.core] cannot find card '0'
ALSA lib conf.c:5211:(_snd_config_evaluate) [error.core] function snd_func_card_inum returned error: No such file or directory
ALSA lib confmisc.c:422:(snd_func_concat) [error.core] error evaluating strings
ALSA lib conf.c:5211:(_snd_config_evaluate) [error.core] function snd_func_concat returned error: No such file or directory
ALSA lib confmisc.c:1342:(snd_func_refer) [error.core] error evaluating name
ALSA lib conf.c:5211:(_snd_config_evaluate) [error.core] function snd_func_refer returned error: No such file or directory
ALSA lib conf.c:5734:(snd_config_expand) [error.core] Evaluate error: No such file or directory
ALSA lib pcm.c:2722:(snd_pcm_open_noupdate) [error.pcm] Unknown PCM default
2026-09-28T13:46:31.641401Z  INFO l337_audio_server: Audio output initialized successfully.
2026-09-28T13:46:31.642200Z  WARN l337_audio_server: No TLS cert configured. Auto-generated a self-signed certificate; server is available at https://0.0.0.0:1337. Configure a trusted cert in [server] tls_cert/tls_key for production.

^C
(99%) tim@archvm:~$ /opt/l337-audio-server/l337-audio-server --help
ALSA lib confmisc.c:855:(parse_card) [error.core] cannot find card '0'
ALSA lib conf.c:5211:(_snd_config_evaluate) [error.core] function snd_func_card_inum returned error: No such file or directory
ALSA lib confmisc.c:422:(snd_func_concat) [error.core] error evaluating strings
ALSA lib conf.c:5211:(_snd_config_evaluate) [error.core] function snd_func_concat returned error: No such file or directory
ALSA lib confmisc.c:1342:(snd_func_refer) [error.core] error evaluating name
ALSA lib conf.c:5211:(_snd_config_evaluate) [error.core] function snd_func_refer returned error: No such file or directory
ALSA lib conf.c:5734:(snd_config_expand) [error.core] Evaluate error: No such file or directory
ALSA lib pcm.c:2722:(snd_pcm_open_noupdate) [error.pcm] Unknown PCM default
2026-09-28T13:46:52.963255Z  INFO l337_audio_server: Audio output initialized successfully.
2026-09-28T13:46:52.965276Z  WARN l337_audio_server: No TLS cert configured. Auto-generated a self-signed certificate; server is available at https://0.0.0.0:1337. Configure a trusted cert in [server] tls_cert/tls_key for production.
^C
(99%) tim@archvm:~$ ssh hostvm
[tim@tim-laptop-mi ~]$ /opt/l337-audio-server/l337-audio-server --version
l337-audio-server 2026.09.26-6
[tim@tim-laptop-mi ~]$ /opt/l337-audio-server/l337-audio-server --help
2026-09-28T13:47:17.708512Z DEBUG l337_audio_server::platform::single_instance: Lock path /run/user/1002/instance.lock held by another instance
2026-09-28T13:47:17.708573Z ERROR l337_audio_server: Another instance is already running (lock file: /run/user/1002/instance.lock)
