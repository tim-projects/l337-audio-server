# Bug Report: l337-player cannot connect to systemd-managed l337-audio-server

## Status: Resolved

### Root Causes
There were two separate issues:

1. **Systemd permission isolation** (points 13–27, 30–33): The systemd unit
   enforced `RuntimeDirectoryMode=0700` and `User=l337`, blocking the client
   user (`vscode`) from accessing the Unix socket and token file.

2. **Missing `/setup` endpoint** (points 15, 30, 51–52): The player's
   `connect_audio_server()` calls `GET /setup` as a fallback to discover the
   token when it cannot read the config or cache files. The server had no
   `/setup` route (it was removed in `cab0245` when the challenge/redeem auth
   flow replaced the insecure token-leaking `/setup` endpoint).

### Fix: `--group-access` installer option + socket-mode-restricted `/setup`

**Permission fix** — commit `755c78d` added the `--group-access` installer flag.
Running the installer with this flag:

- Sets `RuntimeDirectoryMode=0750` (group-traversable)
- Writes `socket_mode = "660"` in `server.ini` (group-writable socket)
- Adds the installing user to the `l337` group

```bash
sudo ./scripts/install-linux-alsa.sh --group-access --force
# then: log out and back in, or run: newgrp l337
```

**Token discovery fix** — commit `f175d51` re-introduced `/setup` **safely**:

- `/setup` returns `{"token": "...", "socket_mode": true}` only when the server
  is listening on a **Unix socket** (`socket_mode = true`).
- When `socket_mode = false` (TCP/HTTPS), `/setup` returns **404**.
- The endpoint is public (no auth required) — this is safe because Unix socket
  access is already gated by filesystem permissions.

### Token Discovery Mechanism

When `l337-player` connects to a local (`unix://`) audio server, it resolves the
token in this order:

1. **`server_token` setting** — saved in the player's config from a previous
   successful connection.
2. **`/etc/l337-audio-server/server.ini`** — the installer writes the token
   here with `0640 l337:l337` permissions; client users in the `l337` group
   can read it.
3. **Cached token file** — `<XDG_CACHE_DIR>/l337/l337-audio-server/server_token.txt`,
   written by the server with `0o600` permissions.
4. **`/setup` endpoint** — last-resort fallback via HTTP GET over the Unix
   socket. Returns the server's current token as JSON.

If the player cannot reach the token via any of these methods, it reports
"server not reachable."

## Environment
- **Player**: l337-player
- **Audio server**: l337-audio-server v2026.09.26-2, installed at `/opt/l337-audio-server/l337-audio-server`
- **Server management**: systemd unit `/etc/systemd/system/l337-audio-server.service` (enabled, active)
- **Client user**: `vscode` (uid 1001)
- **Server user**: `l337` (User=l337, Group=l337)
- **Transport**: Unix socket `/run/l337-audio-server/l337.sock`; TCP port `5355` (listening)

## Observed behavior
- Startup detects no usable local audio server.
- Systemd socket exists but client cannot connect: `PermissionError [Errno 13] Permission denied`.
- `connect_audio_server()` returns `reachable: False, error: "server not reachable"`.
- `/setup` endpoint returns `None` (server accepts TCP but does not respond to HTTP).
- Systemd token file `/etc/l337-audio-server/server.ini` is unreadable by `vscode` (perms `0640 l337:l337`).
- Cached token file exists but belongs to a stale vscode-spawned v0.1.0 server.

## Root cause
The systemd unit enforces user isolation:
- `User=l337`, `Group=l337`
- `RuntimeDirectoryMode=0700` — `/run/l337-audio-server/` is owner-only (`l337`)
- Socket `/run/l337-audio-server/l337.sock` is `srwx------ l337:l337`
- Config `/etc/l337-audio-server/server.ini` is `0640 l337:l337`

The client user (`vscode`) cannot access either the socket or the token file. Adding `vscode` to the `l337` group would not be sufficient because `RuntimeDirectoryMode=0700` blocks group traversal.

## Impact
- Player cannot connect to the systemd audio server.
- `connect_audio_server()` cannot auto-read the token from systemd, cached file, or `/setup`.
- Startup prompts: "1) Install local audio server" or "2) Configure remote audio server"
- Pairing flow is disabled for local Unix sockets (only triggered for remote TCP).

## Steps to reproduce
1. Install `l337-audio-server` via systemd as user `l337`.
2. Run `l337-player` as user `vscode`.
3. Observe: `no systemd audio server socket found` or `server not reachable`.

## Expected behavior
- Player should be able to connect to the systemd audio server.
- Token should be auto-discoverable, or pairing should be available for local sockets.

## Player-Side Changes (implemented in l337-player)

The server-side fix (socket-mode-restricted `/setup`) changed the player's token
discovery landscape. The player was updated to handle both transports:

### Local Unix socket
`connect_audio_server()` calls `fetch_setup_token(url)` on `unix://` URLs as a
fallback, which now succeeds against the server's socket-mode `/setup`. When
that endpoint is unavailable, the token stays empty and the auth probe reports
it as missing rather than hiding the failure.

### Remote TCP server
`/setup` returns 404 here by design. `APIClient.get_setup_token()` now raises
`SetupEndpointUnavailable` on 404 so callers can distinguish "the server
disabled the endpoint" from a transient error, and never retry discovery.

Changes:

1. **`api_client.py`**: `get_setup_token()` raises `SetupEndpointUnavailable`
   on 404; `fetch_setup_token()` propagates it and always closes the client.
2. **`audio_server_manager.py`**: `_handle_token_rejection()` (renamed from
   `_handle_remote_token_rejection()`) now runs for **both** transports. Remote
   servers try `/setup` once; on 404, or for local sockets, it falls through to
   manual entry. `AcceptTokenScreen` gained a `password` input so the user can
   paste a token when none could be auto-discovered.
3. **`startup.py`**: `_try_discover_remote_token()` surfaces a clear
   "enter `server_token` manually" warning on 404. Local mode gained
   `_warn_local_token_rejected()`, which reports the socket path and the
   `--group-access` hint instead of offering to install a second local server.

## Related code
- `src/client/core/audio_servers.py:_check_systemd_audio_server_socket()`
- `src/client/core/audio_servers.py:_read_systemd_server_token()`
- `src/client/core/audio_servers.py:connect_audio_server()`
- `src/client/core/audio_servers.py:fetch_setup_token()`
- `src/client/core/audio_servers.py:set_active_paired_device()`
- `src/client/tui/startup.py:_run_audio_server_checks()`
- `src/client/tui/startup.py:_warn_local_token_rejected()`
- `src/client/tui/audio_server_manager.py:_handle_token_rejection()`
- `src/client/core/api_client.py:APIClient.get_setup_token()`

## Notes
- Port discrepancy: The bug report shows port `5355`, but the current installer
  writes `port = 1337` in `server.ini`. Port `5355` was likely stale config from
  an older installation. Reinstall with `--force` to get the correct port.
- TCP endpoint behavior: When `transport = "auto"` on Linux, the server listens
  **only** on a Unix socket. TCP port 1337 is not bound by this server — any
  service responding on that port is a different process. Use `--transport=http`
  to serve over TCP.
- Pairing flow: For remote TCP servers, the player uses the challenge/redeem
  flow (`POST /auth/challenge` → read challenge file → `POST /auth/redeem`).
  For local Unix sockets, the `/setup` endpoint (added in `f175d51`) provides
  the token directly, since filesystem permissions already gate access.
- Startup now prompts the user to choose: install local or configure remote
  (instead of silently failing).
