# l337-audio-server
A simple rust based audio server designed to handle playing music or podcasts with 3rd party plugin extensibility

## Runtime dependencies

- **yt-dlp** — required for YouTube URL playback. Install separately:
  - Debian/Ubuntu: `apt install yt-dlp`
  - macOS: `brew install yt-dlp`
  - pip: `pip install yt-dlp`

The server does not bundle `yt-dlp`. If it is missing, YouTube URLs will return an error and
the `/health` endpoint will show `yt_dlp: false`.
