# L337 Audio Server — Unified Auth: Client Checklist

## Scope

Client-side (`l337-player`) implementation checklist. See `plan-auth.md` for the
infrastructure spec, `plan-auth-constraints.md` for the hard constraints, and
`plan-auth-server.md` for server-side changes.

**Principle: remove old methods with no fallbacks.** Exactly one auth path: challenge/redeem.
No `/setup`, no static token-file reads, no socket bypass.

**Hard constraints:**
- One node per physical device
- One auth code per node (DeviceID)
- `/capabilities` endpoint for discovery

**Multi-device list is preserved.** The client maintains `paired_devices` with one active
at a time. Switching the active server pauses the previous one. Multi-output to multiple
servers simultaneously is future scope.

---

## 1. `src/client/core/api_client.py`

### 1.1 `request_challenge()`

**No change to the HTTP call.** The server returns `{expires_in: 600, device_id: "..."}`.
The challenge value is NOT in the response.

Verify the docstring reflects this:
```python
async def request_challenge(self) -> dict[str, Any]:
    """POST /auth/challenge (no auth).

    Returns {expires_in: 600, device_id: "..."} on 202.
    The caller must obtain the challenge token from the server's out-of-band
    channel (local file for socket, operator entry for TCP) and pass it to
    redeem_challenge().
    """
```

### 1.2 Add `request_capabilities()`

```python
async def request_capabilities(self) -> dict[str, Any] | None:
    """GET /capabilities (no auth).

    Returns node info: node_id, capabilities, version, transport, trust_group.
    Used for discovery before pairing.
    """
    try:
        if _HAS_HTTPX:
            client = self._ensure_client()
            resp = await client.get("/capabilities")
            if resp.status_code == 200:
                return resp.json()
            return None
        req = urllib.request.Request(f"{self.base_url}/capabilities", method="GET")
        with urllib.request.urlopen(req, timeout=5.0) as r:
            if r.status == 200:
                raw = r.read().decode("utf-8")
                if raw:
                    return json.loads(raw)
            return None
    except Exception:
        return None
```

### 1.3 Delete `get_setup_token()`

Delete the entire method and its urllib fallback. The server has no `/setup` endpoint.

---

## 2. `src/client/core/audio_servers.py`

### 2.1 Delete these functions entirely

These are the old auth-discovery methods that bypass challenge/redeem:

| Function | Reason |
|---|---|
| `_read_cached_token()` | Reads stale dev-server token from `~/.cache/l337/l337-audio-server/server_token.txt` |
| `_read_systemd_server_token()` | Reads `/etc/l337-audio-server/server.ini` which is `0640 l337:l337` |
| `fetch_setup_token()` | Calls `GET /setup` which doesn't exist on the server |
| `_handle_remote_token_rejection()` | Replaced by challenge/redeem flow |
| `_try_discover_remote_token()` | Replaced by challenge/redeem flow |

**Keep** the multi-device list functions: `get_named_servers`, `save_named_servers`,
`get_paired_devices`, `save_paired_devices`, `get_active_paired_device`,
`set_active_paired_device`, `upsert_paired_device`, `forget_paired_device`.

Also delete associated imports (`asyncio`, `json` where no longer needed, etc.).

### 2.2 Add `_read_challenge_file()`

```python
def _read_challenge_file() -> str | None:
    """Read the server's challenge-token.txt from the local filesystem.

    Tries the systemd state dir first, then the desktop cache dir.
    Only viable for Unix-socket (same-host) connections where the client
    and server share a filesystem. The file is owned by the server user
    with 0600 permissions; the client must be the same user (or have
    equivalent access) to read it.
    """
    candidates = [
        "/var/lib/l337-audio-server/challenge-token.txt",  # systemd
        os.path.join(
            os.path.expanduser("~"), ".cache", "l337",
            "l337-audio-server", "challenge-token.txt"
        ),  # desktop/dev
    ]
    for path in candidates:
        try:
            with open(path) as f:
                token = f.read().strip()
                if token:
                    return token
        except OSError:
            continue
    return None
```

### 2.3 Add `_silent_pair()`

```python
async def _silent_pair(client: APIClient) -> str | None:
    """Run challenge/redeem silently for socket (same-host) connections.

    1. POST /auth/challenge → server writes challenge-token.txt (or reuses existing)
    2. Read challenge-token.txt from local filesystem (out-of-band)
    3. POST /auth/redeem with X-L337-Challenge → {ok: true}
    4. Return the challenge value as the paired token
    """
    try:
        challenge_data = await client.request_challenge()
        if not isinstance(challenge_data, dict):
            return None

        challenge = _read_challenge_file()
        if not challenge:
            return None

        redeem_result = await client.redeem_challenge(challenge)
        if isinstance(redeem_result, dict) and redeem_result.get("ok"):
            return challenge
    except Exception:
        pass
    return None
```

### 2.4 Rewrite `connect_audio_server()`

**Delete the 4-level token precedence Rube-Goldberg machine.** Replace with:
1. Use configured `server_token` if present (from manual override or active paired device).
2. If token missing or rejected:
   - **Socket (`unix://`):** run `_silent_pair()`, cache result, retry.
   - **TCP (`https://`):** return `token_ok: False`. Caller shows `TokenPairingScreen`.

```python
async def connect_audio_server(settings, timeout: float = 5.0) -> dict:
    """Probe a running l337-audio-server.

    Returns dict with reachable, token_ok, url, capabilities, error.

    Token precedence:
        1. server_token in settings (from manual override or active paired device)
        2. Silent challenge/redeem for unix:// URLs
    """
    url = (settings.get("server_url") or "").strip()
    token = (settings.get("server_token") or "").strip()

    if not url:
        return {
            "url": "",
            "reachable": False,
            "token_ok": None,
            "error": "no audio server configured",
            "socket_path": None,
            "capabilities": {},
        }

    client = APIClient(base_url=url, token=token or None, verify=False, timeout=timeout)
    result = {
        "url": url,
        "reachable": False,
        "token_ok": None,
        "error": None,
        "socket_path": None,
        "capabilities": {},
    }

    try:
        health_data = await client.health_data()
        if not health_data:
            result["error"] = "server not reachable"
            return result

        result["reachable"] = True
        result["capabilities"] = health_data.get("capabilities") or {}

        try:
            status = await client.get_status()
            result["token_ok"] = status is not None
        except Exception:
            result["token_ok"] = False

        # If token rejected or missing, run pairing
        if result["token_ok"] is not True:
            if url.startswith("unix://"):
                # Socket: silent background pairing
                new_token = await _silent_pair(client)
                if new_token:
                    await client.aclose()
                    client = APIClient(base_url=url, token=new_token, verify=False, timeout=timeout)
                    try:
                        status = await client.get_status()
                        result["token_ok"] = status is not None
                        if result["token_ok"]:
                            settings["server_token"] = new_token
                            save_settings(settings)
                            result["token_fallback"] = True
                    except Exception:
                        result["token_ok"] = False
            else:
                # TCP: caller handles manual pairing via TokenPairingScreen
                result["token_ok"] = False

        return result
    except Exception as e:
        result["error"] = str(e)
        return result
    finally:
        await client.aclose()
```

**Delete from `connect_audio_server`:**
- `_read_cached_token` fallback after token rejection
- `fetch_setup_token` fallback
- `_read_systemd_server_token` fallback

### 2.5 Simplify `resolve_audio_server()`

**Remove:** Token pre-population from systemd/cache reads. The token is obtained exclusively
through `connect_audio_server()`.

```python
def resolve_audio_server(settings):
    choice = settings.get("audio_server", "local")
    if choice == "local":
        settings["audio_server"] = "local"
        from client.platforms import detect_platform
        platform = detect_platform()
        url = socket_url(platform)
        settings["server_url"] = url or ""
        settings["server_token"] = ""   # cleared; connect_audio_server handles pairing
    elif choice.startswith("named:"):
        named = resolve_named_server(settings)
        if named:
            settings["server_url"] = named.get("url", "https://localhost:1337")
            settings["server_token"] = named.get("token", "")
        else:
            settings["server_url"] = "https://localhost:1337"
            settings["server_token"] = ""
    else:
        settings["audio_server"] = "remote"
```

---

## 3. `src/client/tui/startup.py`

### 3.1 Remove `_try_discover_remote_token()`

Delete the entire function.

### 3.2 Remove `_read_systemd_server_token` import and call

In `_run_audio_server_checks()`, remove the import and the call that reads
`/etc/l337-audio-server/server.ini`.

### 3.3 Remove `_try_discover_remote_token` call

In `_run_audio_server_checks()`, remove the call that tries to discover remote tokens.

### 3.4 Resulting `_run_audio_server_checks()`

```python
def _run_audio_server_checks(ctx: StartupContext) -> None:
    settings = ctx.settings
    platform = ctx.platform
    audio_server_mode = settings.get("audio_server", "local")

    from client.core.audio_servers import (
        _check_systemd_audio_server_socket,
        resolve_audio_server,
        sync_named_servers_with_backend,
    )
    resolve_audio_server(settings)

    if audio_server_mode == "local":
        systemd_url, systemd_socket = _check_systemd_audio_server_socket()
        if systemd_url:
            settings["server_url"] = systemd_url
            # Token is no longer read here; connect_audio_server handles pairing
            _status("using systemd audio server socket")
        else:
            _status("no systemd audio server socket found")
            ctx.warnings.append(
                "No local audio server available. "
                "Install l337-audio-server or configure a remote server in settings."
            )
    else:
        _status("remote server configured; no local server needed")

    _status("checking audio server connection...")
    try:
        status = _connect_audio_server_sync(settings)
        ctx.audio_server_status = status
        if status.get("reachable"):
            _status("audio server connection: ok")
        else:
            _status(f"audio server connection: {status.get('error', 'not reachable')}")
            ctx.warnings.append(f"Audio server not reachable: {status.get('error', 'unknown')}")
    except Exception as exc:
        ctx.audio_server_status = {"reachable": False, "error": str(exc)}
        _status(f"audio server connection: error ({exc})")
        ctx.warnings.append(f"Audio server connection failed: {exc}")

    # Backend sync (unchanged)
    # ...
```

---

## 4. `src/client/tui/audio_server_manager.py`

### 4.1 Remove `_handle_remote_token_rejection`

Delete the entire function.

### 4.2 Remove `fetch_setup_token` import

```python
from client.core.audio_servers import (
    connect_audio_server,
    fetch_setup_token,  # ← REMOVE
    get_named_servers,
    save_named_servers,
)
```

### 4.3 Remove the `token_ok is False` branch in `connect()`

```python
    else:
        server_url = app.settings.get("server_url") or ""
        is_local_socket = server_url.startswith("unix://")
        logger.info("Connected to l337-audio-server at %s", status.get("url"))
        # REMOVED: if not is_local_socket and status.get("token_ok") is False:
        #     await _handle_remote_token_rejection(app, server_url)
        if status.get("token_fallback"):
            if hasattr(app, "player") and app.player is not None:
                app.player._refresh_api_client()
        if hasattr(app, "player") and app.player is not None:
            app.player.settings["server_capabilities"] = status.get("capabilities") or {}
```

---

## 5. `src/client/tui/screens/settings_screen.py`

### 5.1 Keep `TokenPairingScreen` — revise instructions

Keep the class. Revise the instructions to reflect the out-of-band model.

```python
class TokenPairingScreen(Dialog):
    BINDINGS = [
        Binding("escape", "dismiss(None)", "Cancel"),
        Binding("ctrl+r", "request_challenge", "Request"),
    ]

    def __init__(self, server_url: str, default_name: str | None = None, **kwargs):
        super().__init__(title="Pair with Audio Server", **kwargs)
        self._server_url = server_url
        self._state = "idle"
        self._expires_at: float = 0.0
        self._server_token: str = ""
        self._default_name = default_name or _default_device_name(server_url)
        self._countdown_interval = None

        self.name_input = Input(
            id="pair-name-input",
            value=self._default_name,
            placeholder="Living Room",
        )
        self.instructions = Static(
            "1. Tap [bold]Request Challenge[/bold] to ask the server for a pairing code.\n"
            "2. Read the code from the server host (e.g., via SSH: "
            "cat /var/lib/l337-audio-server/challenge-token.txt).\n"
            "3. Paste the code below and tap [bold]Pair and Save[/bold].",
            id="pair-instructions",
        )
        self.status_label = Static("", id="pair-status")
        self.challenge_input = Input(
            id="pair-token-input",
            placeholder="Paste the challenge code from the server",
        )
        self.request_btn = Button("Request Challenge", id="pair-request-btn", variant="primary")
        self.pair_btn = Button("Pair and Save Token", id="pair-save-btn", variant="success")
        self.cancel_btn = Button("Cancel", id="pair-cancel-btn")

        self.content_widgets = [
            Label("Device name:"),
            self.name_input,
            self.instructions,
            self.status_label,
            Label("Pasted challenge code:"),
            self.challenge_input,
        ]
        self.action_widgets = [
            self.request_btn,
            self.pair_btn,
            self.cancel_btn,
        ]
```

### 5.2 Remove `_CHALLENGE_TOKEN_RE` regex

The challenge is any non-empty string from the file. No fixed-format validation needed.

```python
# DELETE THIS LINE
_CHALLENGE_TOKEN_RE = re.compile(r"^[A-Za-z0-9]{4}(-[A-Za-z0-9]{4}){4}$")
```

### 5.3 Update `_do_request_challenge()`

The response contains only `expires_in` and `device_id`. No challenge to display.

```python
async def _do_request_challenge(self) -> None:
    self.status_label.update("[dim]Requesting challenge…[/dim]")
    self.request_btn.disabled = True
    client = self._make_client()
    try:
        try:
            data = await client.request_challenge()
        except Exception as e:
            msg = str(e).lower()
            if "429" in msg or "rate" in msg:
                self._state = "rate_limited"
            else:
                self._state = "network_error"
            self._apply_state()
            return
        expires_in = int(data.get("expires_in", 600)) if isinstance(data, dict) else 600
        self._expires_at = time.time() + max(0, expires_in)
        self._state = "challenge_issued"
        self._apply_state()
        self.set_focus(self.challenge_input)
    finally:
        await client.aclose()
```

### 5.4 Update `_do_redeem()`

Remove the `socket_mode` check in the response:

```python
async def _do_redeem(self) -> None:
    token = self.challenge_input.value.strip()
    if not token:
        self.notify("Paste the challenge code from the server first", severity="warning")
        return
    self.pair_btn.disabled = True
    self.status_label.update("[dim]Redeeming challenge…[/dim]")
    client = self._make_client()
    try:
        try:
            data = await client.redeem_challenge(token)
        except Exception as e:
            msg = str(e).lower()
            if "429" in msg or "rate" in msg:
                self._state = "rate_limited"
            elif "401" in msg or "rejected" in msg or "expired" in msg:
                self._state = "redeem_failed"
            else:
                self._state = "network_error"
            self._apply_state()
            return
        # REMOVED: if isinstance(data, dict) and data.get("socket_mode"):
        #     self.notify("Server is in unix-socket mode; no token needed.", ...)
        self._state = "redeem_success"
        self._server_token = token
        self._apply_state()
        self.notify("Token saved", severity="information")
        name = self.name_input.value.strip() or self._default_name
        self.dismiss({"url": self._server_url, "token": token, "name": name})
    finally:
        await client.aclose()
```

### 5.5 Keep `_pair_new()` in `RemoteAudioSettingsScreen`

`_pair_new()` pushes `TokenPairingScreen`. That flow stays the same.

---

## 6. Tests

### 6.1 `tests/unit/test_token_pairing.py`

**Remove these tests:**
- `test_redeem_challenge_socket_mode_response`
- `test_request_challenge_socket_mode_response`
- `TestTokenFormatValidation` (the regex class)

**Add these tests:**
- `test_request_challenge_returns_expires_in_and_device_id` — verify response has `expires_in` and `device_id`, no `challenge` field
- `test_token_pairing_screen_shows_out_of_band_instructions` — verify instructions mention reading from server host
- `test_silent_pair_reads_local_file_and_redeems` — mock `request_challenge`, write a temp challenge file, verify `_silent_pair` reads it and redeems

### 6.2 `tests/unit/test_startup.py`

**Remove:**
- `test_systemd_socket_populates_token_from_config`
- Patches for `_read_systemd_server_token`, `_read_cached_token`, `fetch_setup_token`

**Add:**
- `test_systemd_socket_no_token_triggers_silent_pair` — verify that when systemd socket is present but no cached token, `_run_audio_server_checks` proceeds without error

### 6.3 `tests/unit/test_paired_devices.py`

**Keep:** All multi-device list tests (insert, update, forget, activate, pause previous).

**Update:**
- `TestConnectAudioServerPrecedence` — update for new `connect_audio_server` behavior:
  - Remove tests that rely on `_read_cached_token` or `_read_systemd_server_token`
  - Add: `test_unix_socket_silent_pair_on_token_rejection`
  - Add: `test_tcp_url_does_not_silent_pair`

### 6.4 `tests/unit/test_player_unit.py`

**Update:** Any comment referencing `~/.cache/l337-audio-server/server_token.txt` should reflect that the client no longer reads that file.

---

## 7. Verification Checklist

After all changes, verify:

- [ ] Client tests pass: `pytest tests/unit/test_token_pairing.py tests/unit/test_startup.py tests/unit/test_paired_devices.py -v`
- [ ] No references to `_read_cached_token`, `_read_systemd_server_token`, `fetch_setup_token`, `get_setup_token`, `_handle_remote_token_rejection`, `_try_discover_remote_token` remain in client code
- [ ] `connect_audio_server` uses active paired device for TCP and silent pair for socket
- [ ] `TokenPairingScreen` shows out-of-band instructions (no file-reading instructions)
- [ ] `_CHALLENGE_TOKEN_RE` regex is deleted
- [ ] Socket silent-pair flow works end-to-end (manual test)
- [ ] TCP manual-pair flow works end-to-end via `TokenPairingScreen` (manual test)
- [ ] Switching active paired device pauses previous device (existing behavior preserved)
- [ ] `/capabilities` endpoint returns node_id, capabilities, version (manual test)

---

## 8. What Is Deleted (No Fallbacks)

| Old method | Deleted from | Replacement |
|---|---|---|
| Static token file read | `audio_servers.py` | Challenge/redeem + DeviceID allowlist |
| Systemd config read | `audio_servers.py`, `startup.py` | Challenge/redeem + DeviceID allowlist |
| `/setup` endpoint | `api_client.py`, `audio_servers.py` | Challenge/redeem only |
| Manual paste UI (file-reading) | `settings_screen.py` | `TokenPairingScreen` revised to out-of-band instructions |
| Remote token rejection handler | `audio_server_manager.py` | Challenge/redeem only |
| Remote token discovery | `startup.py` | Challenge/redeem only |

**Preserved:** Multi-device list (`paired_devices`, `named_servers`, `active_paired_device`,
`set_active_paired_device` with pause-on-switch). These are the data model and UI for
managing which server is active. They are orthogonal to auth method.
