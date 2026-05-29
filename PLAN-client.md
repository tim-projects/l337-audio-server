# Refactor: Split YTPlay into L337 Player Client-Rust Architecture

## Goal
Refactor the Python workspace to act as the single stateful orchestrator. The Python application manages the user interface (Textual TUI), maintains a local SQLite database for playlist indexing, persistence, and navigation history, and coordinates external scraper plugins. It passes stateless download, caching, and stream execution instructions to the local Rust backend server running on port `1337`.

## Track Model with Plugin & Storage Resolution

```python
class Track(BaseModel):
    # Core Primary Key
    track_id: Optional[str]     # Unique hash or ID
    
    # Resolved streaming assets 
    stream_url: Optional[str]       # Populated by Python plugins; passed to Rust
    local_filename: Optional[str]   # Local file path if downloaded on disk
    
    # Metadata Graph (Populated by local SQLite or synced web)
    artist: Optional[str]
    track: Optional[str]
    album: Optional[str]
    duration: Optional[int]
    play_count: int = 0
    last_played: Optional[int]
    
    # Legacy support (during migration)
    yt_url: Optional[str]
    yt_url_cached: Optional[str]
    yt_title: Optional[str]
    yt_channel: Optional[str]
```

## Search & Resolution Priority

When a track is chosen, playback resolution uses this cascading priority:

1. **Local Hard Drive Check**: Look up `local_filename` exists (bypasses network)
2. **Third-Party Plugin Resolution**: Trigger plugin to extract `stream_url`
3. **Rust Transmission**: Hand asset to Rust engine over port `1337`

## Search Flow (Client-side)

The search endpoint uses this priority chain:
1. **Local Folders** (fuzzy search) - search configured music folders
2. **Music Mode**: `music.youtube.com` 
3. **Standard Mode**: `www.youtube.com`
4. **Future Sources** (placeholders):
   - Apple Podcasts (podcast mode)
   - SoundCloud (music mode)  
   - Bandcamp (music mode)

Supports search types: `all` (default), `artist`, `track`

## Proposed Directory Structure

```
src/
├── client/
│   ├── __init__.py
│   ├── core/
│   │   ├── __init__.py
│   │   ├── api_client.py   # HTTP layer to Rust Server (Port 1337)
│   │   ├── database.py     # Local SQLite Engine (~/.config/l337player/library.db)
│   │   └── models.py       # Pydantic models (Track, PlayerStatus, SearchResult, SearchQuery)
│   ├── plugins/
│   │   ├── __init__.py
│   │   ├── base.py         # Abstract Base Class for plugins
│   │   ├── youtube.py      # YouTube/YT-Music resolution
│   │   ├── bandcamp.py     # Placeholder
│   │   ├── apple_podcasts.py # Placeholder  
│   │   └── soundcloud.py   # Placeholder
│   └── linux/
│       └── tui/
│           ├── __init__.py
│           ├── app.py        # Textual L337PlayerApp
│           ├── widgets/
│           │   ├── __init__.py
│           │   ├── player_bar.py
│           │   └── playlist_view.py
│           └── screens/
│               ├── __init__.py
│               ├── search_screen.py
│               └── settings_screen.py
├── layouts_tui.py          # TUI Layout configuration
├── l337player.tcss         # Textual CSS (renamed from ytplay.tcss)
└── run_tests.py            # Client tests
```

## Client-to-Rust API Mapping (Port 1337)

| Action Event | HTTP Request | Client-Side State (SQLite) |
|--------------|--------------|---------------------------|
| User selects track | `POST /player/play` (Track JSON) | Wipe history buffer; mark current item |
| Background cache next | `POST /player/cache/next` | Scan queue; resolve next via plugins |
| Background cache prev | `POST /player/cache/previous` | Scan history; resolve prev via plugins |
| Skip forward | `POST /player/next` | Shift active index forward |
| Back button | `POST /player/previous` | Shift active index backward |
| Toggle pause | `POST /player/pause` | None |
| Set speed | `POST /player/speed` ({"speed": f32}) | Save to local settings |
| Status poll | `GET /player/status` | Sync UI progress bar |

## Rust Server API Endpoints

| Method | Endpoint | Body | Description |
|--------|----------|------|-------------|
| POST | /player/play | Track | Play track now |
| POST | /player/cache/next | Track | Pre-cache next track |
| POST | /player/cache/previous | Track | Pre-cache previous track |
| POST | /player/pause | - | Toggle pause |
| POST | /player/next | - | Next track |
| POST | /player/previous | - | Previous track |
| POST | /player/speed | {"speed": f32} | Set playback speed |
| GET | /player/status | - | Get PlayerStatus |
| POST | /download | {"url": str, "track": Track} | Download to local_filename |

## Migration Steps

### Phase 1: Local Data Engine & State Foundation
1. Create `src/client/` directory structure
2. Create `src/client/core/models.py` with Track/SearchResult/SearchQuery Pydantic models
3. Create `src/client/core/database.py` with SQLite for playlists/history
4. Create `src/client/core/api_client.py` with httpx targeting `http://localhost:1337`
5. Migrate config to `~/.config/l337player/`

### Phase 2: Plugin Architecture Construction
1. Create `src/client/plugins/base.py` with AbstractSearchPlugin, AbstractStreamPlugin
2. Move YouTube logic to `src/client/plugins/youtube.py`
3. Create placeholder plugins: `bandcamp.py`, `apple_podcasts.py`, `soundcloud.py`
4. Implement plugin registry with priority chain (local → YT-Music → YouTube)
5. Add local folder fuzzy search module

### Phase 3: Textual UI Refactoring
1. Create `src/client/linux/tui/app.py` as L337PlayerApp
2. Replace direct player calls with API client calls to Rust server
3. Move widgets to `src/client/linux/tui/widgets/`
4. Move screens to `src/client/linux/tui/screens/`
5. Move `layouts_tui.py` to `src/layouts_tui.py`
6. Rename CSS to `l337player.tcss`
7. Update search to support search_type (all/artist/track)
8. Implement local-first search with fuzzy matching

### Phase 4: Testing & Polish
1. Write tests for database operations
2. Write tests for plugin system
3. Update existing tests for new imports
4. Add pyproject.toml entry point: `l337`
5. Update README.md with architecture docs
6. Add plugin hook documentation

## Technical Decisions

1. **State Ownership**: Python owns playlists, settings, queues, history via SQLite. Rust is stateless.
2. **Plugin Decoupling**: Third-party extraction modules isolated in Python layer.
3. **Local-First Search**: Check local folders before hitting YouTube.
4. **Extensible Sources**: Plugin architecture allows adding more sources.
5. **Configuration**: `~/.config/l337player/config.ini` and `library.db`
6. **Cross-Platform**: Python client works on macOS, Windows, Linux.
