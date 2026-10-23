# L337 System — Object Model

## 1. Node

The top-level object. Every device running l337 software is a node. A node has a stable
identity and a set of capabilities. Nodes communicate peer-to-peer; there is no hierarchical
server/client relationship.

```
Node
├── identity: DeviceID
├── capabilities: Set[Capability]
├── transport: Transport
├── auth: AuthLayer
├── challenge: ChallengeState
├── distribution: Distribution
├── output: Output?           # present if `output` in capabilities
├── input: Input?             # present if `input` in capabilities
├── search: Search?           # present if `search` in capabilities
├── playlist: Playlist?       # present if `playlist` in capabilities
└── [objects depending on capabilities]
```

### 1.1 DeviceID

Stable identity for a node. Generated on first boot.

- Format: Ed25519 public key hash (Syncthing-style) or UUID
- Stored in: `device_id.txt` in the state directory
- Used for: mutual authentication, trust group membership, client identity

### 1.2 Capability

A node's abilities. Determines which objects it owns and which endpoints it exposes.

| Capability | Description | Objects |
|---|---|---|
| `output` | Can play audio | PlayerEngine, Storage, AudioOutput |
| `input` | Can capture audio | AudioInput, StreamUploader |
| `control` | Can send play/queue/volume commands | APIClient, PlaybackController |
| `display` | Can show now-playing / queue | NowPlayingWidget, QueueWidget |
| `search` | Can search for music and resolve URLs | Search { panel, resolver, plugins } |
| `playlist` | Can manage playlists and aggregate votes | Playlist { manager, store, db, voting? } |
| `connectivity` | Network/transport connectivity options | Transport, ConnectivityManager |
| `relay` | Can forward messages to other nodes | MessageRelay, TrustGroupSync |

A node can have multiple capabilities. Example configurations:

| Configuration | Capabilities | Example |
|---|---|---|
| PA Speaker | `output + display + connectivity` | Installed PA speaker — no voting, Bluetooth |
| Silent Disco Headphone | `output + voting + display + connectivity` | Headphone with buttons + Bluetooth |
| DJ Laptop | `control + search + playlist + input + connectivity` | DJ controlling a PA system, WiFi + VPN |
| Desktop | `output + control + display + search + playlist + connectivity` | All-in-one desktop, local + Bluetooth |
| Phone Remote | `control + display + search + playlist + connectivity` | Phone app, WWAN + Bluetooth |
| Voting Terminal | `voting + display + connectivity` | Dedicated voting station, local WiFi |
| Website / Backend | `playlist + search` | Web interface managing shared playlists |
| PA Controller | `relay + control + display + playlist + connectivity` | Central controller, VPN + local |
| VPS / Cloud | `output + relay + connectivity` | Remote server, VPN only |
| Bluetooth Device | `output + connectivity` | Bluetooth speaker or headphone only |

---

## 2. Connectivity

How nodes communicate. The connectivity object manages available transports and selects
the best one based on reachability, not role.

```
Connectivity
├── transports: list[Transport]
├── active_transport: Transport
└── methods
    ├── select_transport(peer: NodeRef) -> Transport
    ├── available_transports() -> list[Transport]
    └── is_reachable(peer: NodeRef) -> bool
```

### 2.1 UnixSocketTransport

- Path: `/run/l337-audio-server/l337.sock` (Linux), platform-specific on others
- Permissions: `0600` by default, configurable via `socket_mode`
- Use case: local same-host communication, local network via mounted socket
- Trust: peer credentials (`SO_PEERCRED`) + challenge/redeem

### 2.2 TcpTransport

- Bind: private/local interfaces only by default (`127.0.0.1`, `192.168.x.x`, etc.)
- TLS: self-signed or configured cert
- Use case: local network, internet (opt-in)
- Trust: challenge/redeem + optional website verification

### 2.3 BluetoothTransport

- RFCOMM: mapped to Unix socket path (e.g., `/dev/rfcomm0` → `/run/l337-audio-server/l337.sock`)
- BT PAN: TCP over Bluetooth PAN (e.g., `tcp://192.168.2.1:1337`)
- Use case: Bluetooth headphones, car audio
- Trust: OS-level Bluetooth pairing + challenge/redeem (no website)

### 2.4 WwanTransport

- Cellular modem, mobile hotspot, satellite
- Use case: remote locations, mobile DJ setups
- Trust: challenge/redeem + optional website verification

### 2.5 VpnTransport

- WireGuard, OpenVPN, Tailscale, etc.
- Use case: secure remote access, multi-site PA systems
- Trust: VPN-level auth + challenge/redeem

## 3. Distribution

How this node distributes audio and content to other nodes. A node may support one or
more distribution modes; the profile tells peers what to expect.

```
Distribution
├── broadcast: bool    # can push live/realtime audio to multiple listeners
├── receive: bool      # can accept pushed raw audio from controllers
├── serve: bool        # can serve cached audio files/streams on demand
└── feed: bool         # can export current playlist/queue to subscribers
```

| Mode | Direction | Content | Example |
|---|---|---|---|
| `broadcast` | Node → many | Live/realtime audio stream | Radio station, DJ mixer |
| `receive` | Many → Node | Raw pushed audio | Desktop recording node |
| `serve` | Node → many | Cached files/streams on demand | Desktop library, NAS |
| `feed` | Node → many | Playlist/queue metadata | Website backend, DJ laptop |

A radio station might be `Distribution { broadcast: true, feed: true }`.
A desktop with a music library might be `Distribution { serve: true }`.
A recording node might be `Distribution { receive: true }`.

## 4. Output

Present on nodes with the `output` capability. Encapsulates everything needed for
audio playback.

```
Output
├── engine: PlayerEngine
├── storage: Storage
├── audio: AudioOutput
├── schedule: ScheduleProfile?    # if `schedule` capability
└── sync: SyncProfile?            # if `sync` capability
```

### 4.1 PlayerEngine (Server-side)

The playback state machine.

```
PlayerEngine
├── state: PlayerStateLabel  // Playing, Paused, Stopped
├── current_track: Option<Track>
├── volume: f32
├── speed: f32
├── pitch: f32
├── position_sec: Option<u64>
├── duration_sec: Option<u64>
├── storage: Storage
└── methods
    ├── play_track(track: Track) -> Result<(), EngineError>
    ├── pause()
    ├── resume()
    ├── stop()
    ├── trigger_next()
    ├── trigger_previous()
    ├── seek(position: u64)
    ├── set_volume(volume: f32)
    ├── set_speed_and_pitch(speed: f32, pitch: f32)
    ├── play_pushed(track_id, slot, title, artist)  // for stream uploads
    ├── get_status() -> PlayerStatus
    └── cache_next / cache_previous (background prefetch)
```

#### 4.1.1 PlayerStateLabel

```rust
pub enum PlayerStateLabel {
    Playing,
    Paused,
    Stopped,
}
```

#### 4.1.2 PlayerStatus

```json
{
  "state": "playing",
  "volume": 0.8,
  "speed": 1.0,
  "pitch": 1.0,
  "current_track": {
    "track_id": "abc123",
    "stream_url": "https://...",
    "title": "Song Name",
    "artist": "Artist",
    "duration": 240
  },
  "disk_pool_utilization_bytes": 104857600,
  "next_cached": true,
  "prev_cached": false,
  "position_sec": 42,
  "duration_sec": 240,
  "audio_available": true
}
```

#### 4.1.3 EngineError

```rust
pub enum EngineError {
    YouTube(YouTubeError),  // yt-dlp blocked / failed
    Other(String),          // generic error
}
```

### 4.2 Storage (Server-side)

Disk cache for audio files. LRU eviction with size limit.

```
Storage
├── cache_dir: Path
├── max_pool_size: u64  // max bytes
├── manifest: CacheManifest  // in-memory, protected by Mutex
├── active_slot: ActiveSlot  // current/next/prev
└── methods
    ├── get_active_slot_path(slot: str) -> Path
    ├── get_path_for_track(track_id: str) -> Path
    ├── update_access(track_id: str, size: u64)
    ├── evict_if_needed(needed: u64)
    └── download_stream(url: str, dest: Path) -> Result<(), Error>
```

#### 4.2.1 CacheManifestEntry

```json
{
  "track_id": "abc123",
  "file_size": 10485760,
  "last_accessed": 1706353200,
  "play_count": 5
}
```

#### 4.2.2 ActiveSlot

One of: `current`, `next`, `prev`. Each is a `.stream` file on disk.

- `current`: actively playing
- `next`: prefetched next track
- `prev`: prefetched previous track

### 4.3 AudioOutput (Server-side)

Plays audio to a local output device (speakers, headphones, HDMI).

```
AudioOutput
├── device: OutputDevice
├── format: AudioFormat
└── methods
    ├── start() -> Result<StreamHandle, Error>
    ├── stop()
    └── write(data: bytes)
```

### 4.4 ScheduleProfile

Supports timed playback: alarms, sleep timers, scheduled playlists.

```
ScheduleProfile
├── alarms: list[Alarm]
├── sleep_timer: SleepTimer?
└── methods
    ├── add_alarm(alarm: Alarm) -> alarm_id
    ├── remove_alarm(alarm_id: str) -> bool
    ├── set_sleep_timer(duration: u64) -> timer_id
    └── cancel_sleep_timer(timer_id: str) -> bool
```

```
Alarm
├── alarm_id: str
├── time: str           # "07:00"
├── days: list[str]     # ["mon","tue","wed","thu","fri"]
├── action: str         # "play", "pause", "stop"
├── target: Track | Playlist | None
└── enabled: bool
```

```
SleepTimer
├── timer_id: str
├── duration_sec: u64
├── action: str         # "stop", "pause", "fade_out"
└── remaining_sec: u64
```

### 4.5 SyncProfile

Supports synchronized multi-node playback. Used by PA systems to align multiple
speakers to a common timeline.

```
SyncProfile
├── mode: SyncMode
├── latency_ms: u32
├── delay_adjustment_ms: i32
├── clock_source: ClockSource
└── max_slaves: u32
```

```
SyncMode = sample-accurate | frame-accurate | best-effort
ClockSource = internal | external | PTP | wordclock
```

A PA speaker might report `SyncProfile { mode: sample-accurate, latency_ms: 2, delay_adjustment_ms: 0..50, max_slaves: 0 }`.
A Bluetooth headphone might omit `sync` entirely or report `best-effort`.

## 5. Input

Present on nodes with the `input` capability. Encapsulates everything needed for
audio capture.

```
Input
├── audio: AudioInput
├── uploader: StreamUploader
└── record: RecordProfile?        # if `record` capability
```

### 5.1 AudioInput (Server-side)

Captures audio from a local input device (mic, line-in, DJ deck).

```
AudioInput
├── device: InputDevice
├── format: AudioFormat
└── methods
    ├── start() -> Result<(), Error>
    ├── stop()
    └── read() -> bytes
```

#### 5.1.1 AudioFormat

```json
{
  "sample_rate": 48000,
  "channels": 2,
  "channel_layout": "stereo",
  "bits_per_sample": 16,
  "format": "s16le"
}
```

### 5.2 StreamUploader (Server-side)

Receives raw audio bytes from a client via HTTP streaming upload.

```
StreamUploader
├── slot: ActiveSlot  // current, next, prev
├── track_id: str
├── title: str | None
├── artist: str | None
└── methods
    └── write(chunk: bytes) -> Result<(), Error>
```

### 5.3 RecordProfile

Supports local capture and persistence of audio.

```
RecordProfile
├── format: AudioFormat
├── destination: Path
├── max_file_size_bytes: u64
└── methods
    ├── start() -> Result<RecordHandle, Error>
    ├── stop() -> Path
    └── pause() / resume()
```

A DJ deck might be `Input { audio, uploader, record: RecordProfile { format: flac, destination: / recordings } }`.
A simple mic node might be `Input { audio }`.

## 6. Search

Present on nodes with the `search` capability. Encapsulates music search and URL resolution.

```
Search
├── panel: SearchPanel
├── resolver: TrackResolver
└── plugins: list[SearchPlugin]
```

Search and resolution are coupled: a node that can search should also be able to resolve
the best playback URL for a candidate. The `resolve` behavior lives inside `Search`,
not as a separate root capability.

### 6.1 SearchPanel / SearchScreen (TUI)

Textual UI for searching music.

```
SearchPanel
├── raw_results: list[SearchResult]
└── methods
    └── execute_search(query) -> None
```

### 6.2 TrackResolver

Resolves a track's best playback URL from its metadata.

```
TrackResolver
├── yt_dlp: str  # path to yt-dlp binary
├── settings: dict
└── methods
    ├── resolve(track, playlist) -> None  # mutates track in-place
    ├── get_candidates(track, progress_callback, out_entry) -> list[str]
    ├── next_fallback(track, failed_url) -> str | None
    └── resolve_playlist(tracks) -> None
```

Resolution order:
1. Local file on disk
2. KB community-validated sources (≥3 validations)
3. Existing DB URLs (url, url_cached, health-ranked sources)
4. Registered search plugins (archive.org, sftp, YouTube, etc.)
5. MusicBrainz URL-relations
6. Search cache

### 6.3 SearchSource

| Source | Description | Plugin |
|---|---|---|
| YouTube | YouTube search via yt-dlp | `youtube_search`, `youtube` |
| Archive.org | Archive.org search | `archive_org` |
| SFTP | SFTP server search | `sftp` |
| Local | Local filesystem search | `local` |
| Knowledge Base | Central KB search | `knowledge_base` |

### 6.4 SearchResult

```json
{
  "title": "Song Name",
  "channel": "Artist",
  "url": "https://...",
  "duration": "240"
}
```

## 7. Playlist

Present on nodes with the `playlist` capability. Manages the full playlist database:
tracks, metadata, history, sources, sync, and voting.

```
Playlist
├── manager: PlaylistManager
├── store: PlaylistStore
├── db: PlaylistsDB
├── voting: VotingProfile?    # if `voting` capability
└── methods
    ├── create_playlist(name) -> playlist_id
    ├── load_current_playlist() -> list[Track]
    ├── save_current_playlist(tracks)
    ├── add_track(track)
    ├── remove_track(track_id)
    ├── reorder_tracks(from_index, to_index)
    ├── undo() -> bool
    ├── redo() -> bool
    ├── sync_to_firebase(user_id)
    ├── sync_from_firebase(user_id)
    ├── backup()
    └── import_playlist(file_path) -> dict
```

Voting is a behavior of the playlist management node. A voting terminal sends votes
to the node that owns the playlist database; that node aggregates votes and decides
whether to skip, replay, or reorder. Therefore `voting` is a sub-profile of `playlist`,
not a standalone root capability.

### 7.1 VotingProfile

```
VotingProfile
├── aggregate: VoteAggregate
├── thresholds: VoteThresholds
└── methods
    ├── cast_vote(track_id, vote: like|dislike)
    ├── get_aggregate(track_id) -> VoteAggregate
    ├── should_skip(track_id) -> bool
    └── should_replay(track_id) -> bool
```

```
VoteAggregate
├── track_id: str
├── likes: u32
├── dislikes: u32
├── total_voters: u32
├── threshold_skip: u32
└── threshold_replay: u32
```

```
VoteThresholds
├── skip: u32       # dislikes needed to skip
├── replay: u32     # likes needed to replay
└── window_sec: u32 # voting window after track start
```

### 7.2 PlaylistManager

High-level playlist management. Coordinates create, edit, reorder, sync, and playback
of playlists.

```
PlaylistManager
├── store: PlaylistStore
├── db: PlaylistsDB
├── kb: KnowledgeBaseClient
├── fb: FirebaseSync
└── methods
    ├── create_playlist(name) -> playlist_id
    ├── load_current_playlist() -> list[Track]
    ├── save_current_playlist(tracks)
    ├── add_track(track)
    ├── remove_track(track_id)
    ├── reorder_tracks(from_index, to_index)
    ├── undo() -> bool
    ├── redo() -> bool
    ├── sync_to_firebase(user_id)
    ├── sync_from_firebase(user_id)
    ├── backup()
    └── import_playlist(file_path) -> dict
```

### 7.3 PlaylistStore (Client-side)

Manages playlist persistence, sync, backup, and enrichment.

```
PlaylistStore
├── db: PlaylistsDB
├── kb: KnowledgeBaseClient
├── fb: FirebaseSync
└── methods
    ├── load_playlist() -> list[Track]
    ├── save_playlist(playlist: list[Track])
    ├── save_playlist_and_sync(playlist, user_id) -> dict
    ├── sync_single_track_to_kb(track, user_id) -> dict
    ├── sync_playlists_with_firebase(user_id) -> dict
    ├── backup_playlist()
    ├── undo_playlist() -> bool
    ├── redo_playlist() -> bool
    ├── clear_duplicates(playlist) -> int
    ├── enrich_track_with_musicbrainz(track) -> Track
    ├── import_playlist_and_resolve(tracks) -> dict
    └── add_track_and_resolve(track) -> dict
```

#### 7.3.1 PlaylistsDB (Client-side)

SQLite persistence for playlists, history, likes, sources, and undo/redo.

```
PlaylistsDB
├── db_path: Path
├── conn: sqlite3.Connection
└── tables
    ├── playlists (id, name, format, is_current, created_at, updated_at)
    ├── entries (id, playlist_id, position, artist, title, album, duration, ...)
    ├── entry_ids (entry_id, key, value)
    ├── sources (entry_id, type, url, plays, fails, last_played, last_error)
    ├── history (id, title, artist, channel, url, duration, timestamp)
    ├── source_blocks (source_type, block_type, first_block_at, ...)
    └── likes (id, title, artist, channel, url, source, created_at)
```

---

## 8. AuthLayer

Per-node allowlist of trusted peers. Replaces the old single-token model.

```
AuthLayer
├── allowlist: List[AllowedPeer]
└── methods
    ├── is_allowed(token: str) -> bool
    ├── add_peer(peer: AllowedPeer)
    ├── remove_peer(token: str)
    ├── prune_expired() -> int
    └── update_token(token: str)  // for backward compat / single-token mode
```

### 3.1 AllowedPeer

```json
{
  "token": "paired-token-1",
  "node_id": "node-uuid-1",
  "account_id": "user123",
  "verified": true,
  "paired_at": "2026-01-01T00:00:00Z",
  "expires_at": null,
  "capabilities": ["output", "display"],
  "name": "Foyer Speaker",
  "last_seen": "2026-01-02T00:00:00Z"
}
```

Fields:
- `token`: bearer credential (challenge value from pairing)
- `node_id`: stable Device ID of the peer
- `account_id`: l337 website account ID (if web-verified)
- `verified`: whether website verification was performed
- `paired_at`: when pairing occurred
- `expires_at`: null = permanent, ISO timestamp = temporary with expiry
- `capabilities`: what the peer can do (informational)
- `name`: human-readable name
- `last_seen`: last time this token was used

---

## 9. ChallengeState

Manages the challenge/redeem pairing flow.

```
ChallengeState
├── path: Path  // challenge-token.txt location
├── ttl: Duration  // default 600s
└── methods
    ├── issue() -> Result<String, ChallengeError>
    │   ├── if active challenge exists and not expired: return existing token
    │   └── else: generate new token, write to file, return it
    └── verify_and_consume(presented: str) -> Result<String, ChallengeError>
        ├── read challenge-token.txt
        ├── compare with presented
        ├── if match: promote to server_token, delete challenge file, return token
        └── if mismatch/expired: return error
```

### 9.1 Challenge File

- Path: `STATE_DIRECTORY/challenge-token.txt` (systemd) or `~/.cache/l337/l337-audio-server/challenge-token.txt` (desktop)
- Permissions: `0600`
- TTL: 10 minutes (mtime check)
- Single-use: deleted after successful redeem

---

## 10. PlayerEngine (Server-side)

The playback state machine. Runs on nodes with the `output` capability.

```
PlayerEngine
├── state: PlayerStateLabel  // Playing, Paused, Stopped
├── current_track: Option<Track>
├── volume: f32
├── speed: f32
├── pitch: f32
├── position_sec: Option<u64>
├── duration_sec: Option<u64>
├── storage: Storage
└── methods
    ├── play_track(track: Track) -> Result<(), EngineError>
    ├── pause()
    ├── resume()
    ├── stop()
    ├── trigger_next()
    ├── trigger_previous()
    ├── seek(position: u64)
    ├── set_volume(volume: f32)
    ├── set_speed_and_pitch(speed: f32, pitch: f32)
    ├── play_pushed(track_id, slot, title, artist)  // for stream uploads
    ├── get_status() -> PlayerStatus
    └── cache_next / cache_previous (background prefetch)
```

### 10.1 PlayerStateLabel

```rust
pub enum PlayerStateLabel {
    Playing,
    Paused,
    Stopped,
}
```

### 10.2 PlayerStatus

```json
{
  "state": "playing",
  "volume": 0.8,
  "speed": 1.0,
  "pitch": 1.0,
  "current_track": {
    "track_id": "abc123",
    "stream_url": "https://...",
    "title": "Song Name",
    "artist": "Artist",
    "duration": 240
  },
  "disk_pool_utilization_bytes": 104857600,
  "next_cached": true,
  "prev_cached": false,
  "position_sec": 42,
  "duration_sec": 240,
  "audio_available": true
}
```

### 10.3 EngineError

```rust
pub enum EngineError {
    YouTube(YouTubeError),  // yt-dlp blocked / failed
    Other(String),          // generic error
}
```

---

## 11. Storage (Server-side)

Disk cache for audio files. LRU eviction with size limit.

```
Storage
├── cache_dir: Path
├── max_pool_size: u64  // max bytes
├── manifest: CacheManifest  // in-memory, protected by Mutex
├── active_slot: ActiveSlot  // current/next/prev
└── methods
    ├── get_active_slot_path(slot: str) -> Path
    ├── get_path_for_track(track_id: str) -> Path
    ├── update_access(track_id: str, size: u64)
    ├── evict_if_needed(needed: u64)
    └── download_stream(url: str, dest: Path) -> Result<(), Error>
```

### 11.1 CacheManifestEntry

```json
{
  "track_id": "abc123",
  "file_size": 10485760,
  "last_accessed": 1706353200,
  "play_count": 5
}
```

### 11.2 ActiveSlot

One of: `current`, `next`, `prev`. Each is a `.stream` file on disk.

- `current`: actively playing
- `next`: prefetched next track
- `prev`: prefetched previous track

---

## 12. APIClient (Client-side)

Async HTTP client for communicating with nodes. Supports Unix socket, TCP/HTTPS, and
streaming uploads.

```
APIClient
├── base_url: str
├── uds: Option<Path>  // Unix socket path
├── token: Option<String>
├── verify: bool
├── timeout: Option<f32>
└── methods
    ├── health() -> bool
    ├── health_data() -> dict
    ├── get_status() -> dict
    ├── request_challenge() -> dict  // {expires_in: 600}
    ├── redeem_challenge(token: str) -> dict  // {ok: true}
    ├── play_track(track: Track)
    ├── pause()
    ├── next_track()
    ├── previous_track()
    ├── set_volume(volume: f32)
    ├── set_speed(speed: f32)
    ├── seek(position: u64)
    ├── cache_next(track: Track)
    ├── cache_previous(track: Track)
    ├── cache_lookup(track_ids: list[str]) -> dict
    ├── upload_stream(endpoint, fileobj, track_id, title, artist)
    ├── backend_login(firebase_token: str) -> dict
    ├── backend_refresh(refresh_token: str) -> dict
    ├── list_servers() -> list[dict]
    ├── create_server(name, url, token) -> dict
    └── delete_server(server_id: str) -> bool
```

---

## 13. Player (Client-side)

Client-side playback wrapper. Manages local playback or delegates to a remote node.

```
Player
├── engine: PlayerEngine  // local engine (optional)
├── api_client: APIClient  // remote node client
├── playlist: Playlist  // current playlist
├── settings: dict
├── local_mode: bool
└── methods
    ├── play()
    ├── pause()
    ├── stop()
    ├── next()
    ├── previous()
    ├── seek(position: u64)
    ├── set_volume(volume: f32)
    ├── load_playlist(playlist: list[Track])
    ├── get_status() -> dict
    └── resolve_and_play(track: Track)
```

---

## 14. Playlist (Client-side)

Ordered list of tracks with metadata. The primary data structure for the user's music.

```
Playlist = list[Track]
```

### 14.1 Track (Universal)

The universal track representation used throughout the system. Flat dict.

```json
{
  "entry_id": 42,
  "title": "Song Name",
  "artist": "Artist Name",
  "channel": "Uploader",
  "url": "https://youtube.com/watch?v=...",
  "url_cached": "https://...",
  "local_file": "/home/user/Music/song.mp3",
  "source": "youtube",
  "track_resolved": true,
  "album": "Album Name",
  "duration": "240",
  "duration_seconds": 240,
  "track_id": "mbid-recording-id",
  "mbartistid": "mbid-artist-id",
  "mbrecid": "mbid-recording-id",
  "mbtrackid": "mbid-track-id",
  "mbreleaseid": "mbid-release-id",
  "mbreleasegroupid": "mbid-rg-id",
  "mbartistid_uri": "https://musicbrainz.org/artist/...",
  "mbrecid_uri": "https://musicbrainz.org/recording/...",
  "mbtrackid_uri": "https://musicbrainz.org/track/...",
  "mbreleaseid_uri": "https://musicbrainz.org/release/...",
  "mbreleasegroupid_uri": "https://musicbrainz.org/release-group/...",
  "acoustid": "acoustid-id",
  "filepath": "/path/to/file.mp3",
  "ids": {
    "mbrecid": "mbid-recording-id",
    "mbartistid": "mbid-artist-id",
    "uri": "https://youtube.com/watch?v=..."
  },
  "sources": [
    {
      "type": "youtube",
      "url": "https://...",
      "plays": 5,
      "fails": 0,
      "last_played": 1706353200,
      "last_error": null
    }
  ],
  "synced_to_firestore": false,
  "firestore_doc_id": null,
  "last_synced_at": null,
  "discovery_source": "local",
  "confidence_score": 1.0,
  "mb_checked_at": 1706353200,
  "backend_checked_at": 1706353200,
  "offline": false
}
```

---

## 15. PlaylistManager

High-level playlist management. Coordinates create, edit, reorder, sync, and playback
of playlists. Exists on nodes with the `playlist` capability.

```
PlaylistManager
├── store: PlaylistStore
├── db: PlaylistsDB
├── kb: KnowledgeBaseClient
├── fb: FirebaseSync
└── methods
    ├── create_playlist(name) -> playlist_id
    ├── load_current_playlist() -> list[Track]
    ├── save_current_playlist(tracks)
    ├── add_track(track)
    ├── remove_track(track_id)
    ├── reorder_tracks(from_index, to_index)
    ├── undo() -> bool
    ├── redo() -> bool
    ├── sync_to_firebase(user_id)
    ├── sync_from_firebase(user_id)
    ├── backup()
    └── import_playlist(file_path) -> dict
```

A node with `playlist` capability can manage playlists. This may be:
- A controller node (DJ laptop, phone) where the user builds the queue
- A website/backend node that manages shared playlists for many output nodes
- An output node that maintains its own playlist queue

---

## 16. PlaylistStore (Client-side)

Manages playlist persistence, sync, backup, and enrichment.

```
PlaylistStore
├── db: PlaylistsDB
├── kb: KnowledgeBaseClient
├── fb: FirebaseSync
└── methods
    ├── load_playlist() -> list[Track]
    ├── save_playlist(playlist: list[Track])
    ├── save_playlist_and_sync(playlist, user_id) -> dict
    ├── sync_single_track_to_kb(track, user_id) -> dict
    ├── sync_playlists_with_firebase(user_id) -> dict
    ├── backup_playlist()
    ├── undo_playlist() -> bool
    ├── redo_playlist() -> bool
    ├── clear_duplicates(playlist) -> int
    ├── enrich_track_with_musicbrainz(track) -> Track
    ├── import_playlist_and_resolve(tracks) -> dict
    └── add_track_and_resolve(track) -> dict
```

### 16.1 PlaylistsDB (Client-side)

SQLite persistence for playlists, history, likes, sources, and undo/redo.

```
PlaylistsDB
├── db_path: Path
├── conn: sqlite3.Connection
└── tables
    ├── playlists (id, name, format, is_current, created_at, updated_at)
    ├── entries (id, playlist_id, position, artist, title, album, duration, ...)
    ├── entry_ids (entry_id, key, value)
    ├── sources (entry_id, type, url, plays, fails, last_played, last_error)
    ├── history (id, title, artist, channel, url, duration, timestamp)
    ├── source_blocks (source_type, block_type, first_block_at, ...)
    └── likes (id, title, artist, channel, url, source, created_at)
```

---

## 17. UPL (Universal Playlist Format)

The canonical playlist format. Bridges between flat track dicts and structured UPL 2.0.

```
UPL
├── format: "UPL2"
├── playlists: list[UPLPlaylist]
└── methods
    ├── load_upl(path) -> list[UPLPlaylist]
    ├── save_upl(path, upl)
    ├── legacy_track_to_upl(track) -> UPLTrack
    ├── upl_track_to_legacy(entry) -> Track
    ├── upsert_source(entry, type, url) -> Source
    ├── best_source(entry) -> Source | None
    ├── sorted_sources(entry) -> list[Source]
    ├── source_score(source) -> tuple
    └── record_play_result(entry, type, url, success, error)
```

### 17.1 UPLPlaylist

```json
{
  "format": "UPL2",
  "name": "My Playlist",
  "id": "uuid",
  "entries": [ /* UPLTrack[] */ ]
}
```

### 17.2 UPLTrack

```json
{
  "artist": "Artist Name",
  "title": "Track Title",
  "uploader": "Uploader",
  "ids": {
    "mbrecid": "mbid-recording-id",
    "mbartistid": "mbid-artist-id",
    "uri": "https://youtube.com/watch?v=..."
  },
  "sources": [
    {
      "type": "youtube",
      "url": "https://...",
      "plays": 5,
      "fails": 0,
      "last_played": 1706353200,
      "last_error": null
    }
  ],
  "album": "Album Name",
  "duration": 240.0,
  "source": "youtube",
  "track_resolved": true,
  "local_file": "/path/to/file.mp3",
  "duration_seconds": 240
}
```

---

## 18. History

Playback history. List of previously played tracks.

```
History
├── entries: list[HistoryEntry]
└── methods
    ├── save_history(track, duration)
    ├── get_history(limit) -> list[dict]
    └── get_last_track() -> dict | None
```

### 18.1 HistoryEntry

```json
{
  "title": "Song Name",
  "artist": "Artist",
  "channel": "Uploader",
  "url": "https://...",
  "duration": 240.0,
  "timestamp": 1706353200.0
}
```

---

## 20. TrackResolver

Resolves a track's best playback URL from its metadata. Client-side only.

```
TrackResolver
├── yt_dlp: str  // path to yt-dlp binary
├── settings: dict
└── methods
    ├── resolve(track, playlist) -> None  // mutates track in-place
    ├── get_candidates(track, progress_callback, out_entry) -> list[str]
    ├── next_fallback(track, failed_url) -> str | None
    └── resolve_playlist(tracks) -> None
```

Resolution order:
1. Local file on disk
2. KB community-validated sources (≥3 validations)
3. Existing DB URLs (url, url_cached, health-ranked sources)
4. Registered search plugins (archive.org, sftp, YouTube, etc.)
5. MusicBrainz URL-relations
6. Search cache

---

## 21. KnowledgeBaseClient

Central PostgreSQL knowledge base for MusicBrainz-tagged content. Client-side only.

```
KnowledgeBaseClient
├── base_url: str
├── token_provider: Callable[[], str | None]  // Firebase token
├── cache: dict  // conditional request cache
├── cache_etags: dict
└── methods
    ├── lookup_recording(mbid) -> dict | None
    ├── search_recordings(query, limit, offset) -> list[dict]
    ├── lookup_artist(mbid) -> dict | None
    ├── submit_discovery(mbid, entity_type, user_id, annotations, context) -> dict
    ├── store_custom_entry(user_id, entry) -> dict
    ├── get_custom_entries(user_id, limit) -> list[dict]
    ├── get_custom_upl_entry(mbid, entity_type, user_id) -> dict | None
    ├── sync_playlist(user_id, playlist_id, entries) -> dict
    ├── get_recording_confidence(mbid) -> dict
    ├── submit_source_validation(mbid, entity_type, user_id, source_type, url, valid) -> dict
    ├── get_validated_sources(mbid, entity_type) -> list[dict]
    └── get_community_validated_sources(mbid, entity_type, min_validations) -> list[dict]
```

---

## 22. FirebaseAuth

Google Sign-In via browser OAuth flow. Client-side only.

```
FirebaseAuth
├── api_key: str
├── project_id: str
├── auth_domain: str
├── cached_token: str | None
├── cached_user: dict | None
├── token_expires_at: float
└── methods
    ├── start_auth_flow() -> str  // returns URL for browser
    ├── complete_auth_flow(callback_url) -> dict  // extracts token from callback
    ├── verify_token(id_token) -> dict  // verifies with Firebase, returns user info
    ├── get_cached_token() -> str | None
    ├── get_cached_user() -> dict | None
    └── clear_cache()
```

---

## 23. Settings

Configuration management. Both client and server.

```
Settings
├── config_dir: Path
├── config_file: Path
├── data_dir: Path
└── methods
    ├── load_settings() -> dict
    ├── save_settings(settings)
    └── validate_settings(settings)
```

### 23.1 Client Settings

```ini
[SETTINGS]
audio_server = "local"
server_url = "https://localhost:1337"
server_token = ""
web_token = ""
cache_dir = "/home/user/.cache/l337-player"
data_dir = "/home/user/.config/l337-player"
download_folder = "/home/user/Downloads"
music_download_format = "mp3"
music_download_bitrate = "192K"
transcode = "True"
remote_transcode = "True"
max_cache_size_mb = "256"
shuffle_enabled = "False"
last_playlist_index = "-1"
last_playing_track_id = ""
```

### 23.2 Server Settings

```ini
[server]
transport = "socket"
socket_path = "/run/l337-audio-server/l337.sock"
host = "127.0.0.1"
port = 1337
bind_mode = "private"
trust_group = "main-pa"
temporary_pairing_expiry = "24h"
challenge_ttl = 600
challenge_rate_limit = 5
require_web_verification = false
website_api_url = "https://l337.audio/api/verify-token"
website_api_secret = "..."
pairing_mode = "button"
pairing_timeout = 300
token_file = "server_token.txt"
paired_clients_file = "paired_clients.json"
```

---

## 24. TransientDB

Undo/redo history for playlists. Client-side only.

```
TransientDB
├── db_path: Path
└── tables
    ├── undo_history (id, playlist_id, data, created_at)
    └── redo_history (id, playlist_id, data, created_at)
└── methods
    ├── push_undo(playlist_id, tracks) -> bool
    ├── pop_undo(playlist_id) -> (data, row_id) | None
    ├── push_redo(playlist_id, tracks)
    ├── pop_redo(playlist_id) -> (data, row_id) | None
    ├── clear_redo(playlist_id)
    └── clear_history(playlist_id)
```

---

## 25. DownloadManager

Downloads tracks via yt-dlp. Client-side only.

```
DownloadManager
├── yt_dlp: str  // path to yt-dlp binary
├── folder: Path
└── methods
    └── download(app, url, folder, track) -> None
```

Content types:
- Music: MP3 192kbps (configurable)
- Podcast: MP3 64kbps (configurable)
- Auto-detect: heuristic based on title/channel

---

## 26. SearchPanel / SearchScreen (TUI)

Textual UI for searching music. Client-side only.

```
SearchPanel
├── raw_results: list[SearchResult]
└── methods
    └── execute_search(query) -> None

SearchScreen (ModalScreen)
└── contains: SearchPanel
```

---

## 27. Plugin System

Extensible plugin architecture for search, playlist import/export, and content sources.

```
PluginRegistry
├── _plugins: dict[str, BasePlugin]
├── _plugin_classes: dict[str, type]
└── methods
    ├── register_plugin(name, class)
    ├── create_plugin(name, config) -> BasePlugin | None
    ├── get_plugin(name) -> BasePlugin | None
    ├── shutdown_all()
    ├── list_plugins() -> list[str]
    ├── list_loaded_plugins() -> list[str]
    ├── get_search_plugins() -> dict[str, BasePlugin]
    └── get_search_plugin_classes() -> dict[str, type]

BasePlugin (ABC)
├── name: str
├── rate_limit: int
└── methods
    ├── initialize(config) -> bool
    ├── shutdown()
    ├── get_commands() -> dict
    ├── get_settings() -> list[dict]
    ├── get_info() -> dict
    └── search(query, max_results) -> list[dict]
```

### 27.1 Built-in Search Plugins

| Plugin | Source | Description |
|---|---|---|
| `youtube_search` | YouTube | YouTube search via youtube-search-python |
| `youtube` | YouTube | YouTube via API key or yt-dlp |
| `archive_org` | Archive.org | Archive.org search |
| `sftp` | SFTP | SFTP server search |
| `local` | Local filesystem | Local file search |

### 27.2 Playlist Plugins

| Plugin | Description |
|---|---|
| `direct` | In-memory playlist |
| `json_file` | JSON file playlist |
| `upl` | UPL 2.0 format |

### 27.3 Import/Export Plugins

| Plugin | Format |
|---|---|
| `m3u` | M3U playlist |
| `pls` | PLS playlist |
| `xspf` | XSPF playlist |
| `asx` | ASX playlist |
| `wpl` | WPL playlist |
| `mpd` | MPD playlist |
| `upl` | UPL format |

---

## 28. TrustGroup

A set of nodes that share pairing tokens. Enables one-pair-many-nodes for PA systems.

```
TrustGroup
├── name: str
├── nodes: list[NodeRef]
├── token_propagation: bool
└── methods
    ├── add_node(node: NodeRef)
    ├── remove_node(node_id: str)
    ├── propagate_token(token, expires_at, issuer)
    └── prune_expired()
```

### 28.1 NodeRef

```json
{
  "node_id": "node-uuid-1",
  "url": "https://192.168.1.100:1337",
  "capabilities": ["output", "display"],
  "name": "Foyer Speaker",
  "last_seen": "2026-01-02T00:00:00Z"
}
```

---

## 29. Vote

A vote from a voting node on a currently playing track.

```
Vote
├── track_id: str
├── node_id: str
├── vote: str  // "like" or "dislike"
├── timestamp: float
└── [server aggregates votes]
```

### 29.1 VoteAggregate

```json
{
  "track_id": "abc123",
  "likes": 42,
  "dislikes": 3,
  "total_voters": 45,
  "threshold_skip": 10,
  "threshold_replay": 20
}
```

---

## 30. Session

Connected clients on an output node.

```
Session
├── clients: list[SessionClient]
└── methods
    ├── add_client(client: SessionClient)
    ├── remove_client(node_id: str)
    └── get_clients() -> list[SessionClient]
```

### 30.1 SessionClient

```json
{
  "node_id": "node-uuid-1",
  "capabilities": ["control", "display"],
  "name": "DJ Laptop",
  "connected_at": "2026-01-01T00:00:00Z",
  "last_seen": "2026-01-02T00:00:00Z"
}
```

---

## 31. SourceBlock

Tracks blocked/unavailable sources (e.g., YouTube IP block).

```
SourceBlock
├── source_type: str  // "youtube", "archive_org", etc.
├── block_type: str  // "ip_blocked", "rate_limit", "captcha", etc.
├── first_block_at: float
├── last_block_at: float
├── last_error_at: float
└── cooldown: float  // seconds before retrying
```

---

## 32. RateLimiter

Token-bucket rate limiter for auth endpoints. Server-side.

```
RateLimiter
├── max_attempts: int  // default 5
├── window: Duration  // default 60s
└── methods
    └── check(addr: SocketAddr) -> Result<(), RateLimitError>
```

---

## 33. AppState (Server-side)

Shared application state wrapper. Makes `PlayerEngine` safely shareable across tasks.

```
AppState = Arc<SendableEngine>

SendableEngine
└── engine: Mutex<PlayerEngine>
```

---

## 34. Node Relationships

### 31.1 Controller → Output

```
Controller Node                     Output Node
  |                                   |
  | POST /auth/challenge              |
  | ← {expires_in: 600, device_id}    |
  |                                   |
  | [out-of-band: read challenge]     |
  |                                   |
  | POST /auth/redeem                 |
  | X-L337-Challenge: <nonce>        |
  | ← {ok: true}                     |
  |                                   |  → adds controller to allowlist
  |                                   |
  | POST /player/play                 |
  | Authorization: Bearer <token>     |
  | ← {ok: true}                     |
  |                                   |  → starts playback
  |                                   |
  | GET /player/status                |  ← push: now_playing event
  | ← {state, track, ...}            |
  |                                   |
  | POST /player/vote                 |
  | {track_id, vote: "like"}         |
  | ← {ok: true}                     |
  |                                   |  → aggregates votes
```

### 31.2 Output → Controller (Push Events)

```
Output Node                         Controller Node
  |                                   |
  | POST /events (long-poll / WS)     |
  | ← {event: "now_playing",          |
  |      data: {track: "X"}}          |
  |                                   |
  | POST /events                      |
  | ← {event: "vote_update",          |
  |      data: {likes: 42}}           |
```

### 31.3 Voting Terminal → Output

```
Voting Node                         Output Node
  |                                   |
  | POST /player/vote                 |
  | {track_id, vote: "like"}         |
  | ← {ok: true}                     |
  |                                   |  → updates vote count
  |                                   |  → pushes vote_update to controllers
```

### 31.4 Relay / Trust Group Propagation

```
Controller Node                     Speaker 1 (Relay)                   Speaker 2 ... Speaker N
  |                                   |                                   |
  | POST /auth/challenge              |                                   |
  | ← {expires_in: 600}              |                                   |
  |                                   |                                   |
  | POST /auth/redeem                 |                                   |
  | X-L337-Challenge: <nonce>        |                                   |
  | ← {ok: true, token: "temp",      |                                   |
  |        expires_at: "+24h",        |                                   |
  |        group: "main-pa"}         |                                   |
  |                                   | → POST /auth/redeem (propagate)   |
  |                                   | → Speaker 2 ... Speaker N         |
  |                                   | ← {ok} x N-1                    |
```

---

## 35. Data Flow

### 32.1 Playback Flow

```
User selects track on Controller
  → Controller sends POST /player/play to Output
    → Output resolves track (TrackResolver)
      → Output downloads/prefetches audio (Storage)
        → Output plays audio (PlayerEngine → AudioOutput)
          → Output pushes now_playing event to Controllers
            → Controllers update UI
```

### 32.2 Voting Flow

```
User presses "like" on Voting Node
  → Voting Node sends POST /player/vote to Playlist Node
    → Playlist Node aggregates vote (VoteAggregate)
      → Playlist Node pushes vote_update event to Controllers
        → Controllers update UI
        → Playlist Node may trigger skip/replay on Output
```

### 32.3 Search Flow

```
User searches on Controller
  → Controller queries registered SearchPlugins
    → Results aggregated and scored (SearchResultScorer)
      → User selects track
        → Controller sends track to Output for playback
```

### 32.4 Pairing Flow

```
Controller calls POST /auth/challenge on Output
  → Output generates challenge, writes challenge-token.txt
    → Output returns {expires_in: 600, device_id: "..."}
      → Controller obtains challenge out-of-band (file for local, operator entry for remote)
        → Controller calls POST /auth/redeem with X-L337-Challenge
          → Output verifies, adds controller to allowlist
            → Both nodes can now communicate freely
```

---

## 36. Summary: Objects by Capability

### Objects on Output Nodes (PA Speaker, Headphone)

| Object | Description |
|---|---|
| `Node` | Identity, capabilities, transport, distribution |
| `AuthLayer` | Allowlist of trusted controllers |
| `ChallengeState` | Challenge/redeem state |
| `Output` | Root output object |
| `PlayerEngine` | Playback state machine |
| `Storage` | Disk cache for audio |
| `AudioOutput` | Audio playback |
| `ScheduleProfile` | Alarms, sleep timers (if `schedule` capability) |
| `SyncProfile` | Multi-node sync (if `sync` capability) |
| `VoteReceiver` | Receives votes from voting nodes |
| `Session` | Connected clients |
| `NowPlayingWidget` | Push now-playing events |

### Objects on Controller Nodes (DJ Laptop, Phone)

| Object | Description |
|---|---|
| `Node` | Identity, capabilities, transport, distribution |
| `AuthLayer` | Allowlist of trusted outputs |
| `ChallengeState` | Challenge/redeem state |
| `Search` | Root search object |
| `SearchPanel` | Search UI |
| `TrackResolver` | Resolve track URLs |
| `Playlist` | Root playlist object |
| `PlaylistManager` | High-level playlist coordination |
| `PlaylistStore` | Save/load/sync playlists |
| `PlaylistsDB` | SQLite persistence |
| `VotingProfile` | Vote aggregation (if `voting` capability) |
| `APIClient` | HTTP client for communicating with outputs |
| `Player` | Playback wrapper (delegates to remote or local) |
| `FirebaseAuth` | Website authentication |
| `KnowledgeBaseClient` | Central KB client |
| `History` | Playback history |
| `QueueWidget` | Display queue |

### Objects on Hybrid Nodes (Desktop, DJ Laptop with Local Output)

| Object | Description |
|---|---|
| All output node objects | Local playback |
| `Search` | Root search object |
| `Playlist` | Root playlist object |
| `Input` | Root input object |
| `APIClient` | HTTP client for remote control |
| `Player` | Local/remote playback wrapper |
| `LocalBypass` | Trusted bypass for local client → local server |
| `AudioInput` | Optional local audio capture |
| `StreamUploader` | Push raw audio to local server |
| `RecordProfile` | Local capture and persistence (if `record` capability) |

### Objects on Silent Disco Headphones

| Object | Description |
|---|---|
| `Node` | Identity, capabilities |
| `AuthLayer` | Allowlist of trusted controller |
| `Output` | Root output object |
| `PlayerEngine` | Playback |
| `Storage` | Small cache (maybe none, streaming only) |
| `AudioOutput` | Audio to headphone driver |
| `VoteSender` | Buttons send votes |
| `VoteReceiver` | Receive vote confirmations |
| `Display` | Show now-playing / vote counts |

### Objects on Voting Terminals

| Object | Description |
|---|---|
| `Node` | Identity, capabilities |
| `AuthLayer` | Allowlist of trusted playlist node |
| `VoteSender` | Send votes to playlist management node |
| `Display` | Show current track / vote buttons |

---

## 37. Plugin Objects

| Plugin | Type | Description |
|---|---|---|
| `track_resolver` | Search + Resolve | Resolves track URLs, multi-source |
| `youtube_search` | Search | YouTube search via youtube-search-python |
| `youtube` | Search + Resolve | YouTube via API key |
| `archive_org` | Search | Archive.org search |
| `sftp` | Search | SFTP server search |
| `local` | Search | Local filesystem search |
| `mediabrainz` | Resolve | MusicBrainz metadata |
| `tunefinder` | Search | Tunefinder search |
| `l337_account` | Sync | Firebase sync |
| `youtube_blocks` | Utility | YouTube block detection |
| `search_scorer` | Utility | Search result scoring |
| `metadata_resolver` | Resolve | Metadata resolution |
