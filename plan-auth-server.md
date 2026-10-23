# L337 Audio Server — Unified Auth: Server Checklist

## Scope

Server-side (`l337-audio-server`) implementation checklist. See `plan-auth.md` for the
infrastructure spec and `plan-auth-constraints.md` for the hard constraints.

**Principle: remove old methods with no fallbacks.** Exactly one auth path: challenge/redeem.
Socket-mode bypass is deleted. The server does not hand out the challenge in the response.

**Hard constraints:**
- One node per physical device
- One auth code per node (DeviceID)
- `/capabilities` endpoint on every node

---

## 1. `src/auth_challenge.rs`

### 1.1 `issue()` — add active-challenge reuse

**Change:** If a valid `challenge-token.txt` already exists and is not expired, return the
existing token without overwriting it.

```rust
pub async fn issue(&self) -> Result<String, ChallengeError> {
    let inner = self.inner.lock().await;

    if inner.path.exists() {
        if let Ok(metadata) = std::fs::metadata(&inner.path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed < CHALLENGE_TTL {
                        let stored = std::fs::read_to_string(&inner.path)?;
                        let stored = stored.trim();
                        if !stored.is_empty() {
                            tracing::warn!(
                                "Reusing existing auth challenge token at {} ({}s old)",
                                inner.path.display(),
                                elapsed.as_secs()
                            );
                            return Ok(stored.to_string());
                        }
                    }
                }
            }
        }
    }

    let token = generate_token();
    crate::secrets_fs::atomic_write_secret(&inner.path, &token)?;
    tracing::warn!("Issued new auth challenge token at {}", inner.path.display());
    Ok(token)
}
```

`verify_and_consume()` stays unchanged.

---

## 2. `src/api/handlers.rs`

### 2.1 Add `GET /capabilities` endpoint

```rust
pub async fn capabilities() -> impl IntoResponse {
    Json(serde_json::json!({
        "node_id": crate::device_id::get_or_create(),
        "name": crate::config::server_name(),
        "capabilities": ["output", "display", "voting"],  // from config
        "version": env!("L337_VERSION"),
        "transport": if use_socket { "socket" } else { "http" },
        "trust_group": crate::config::trust_group(),
        "challenge_ttl": crate::auth_challenge::CHALLENGE_TTL.as_secs(),
    }))
}
```

Register in `main.rs` as a public route (no auth).

### 2.2 `auth_challenge` handler

**Remove:** `axum::Extension(socket_mode): axum::Extension<bool>` parameter.

**Remove:** `if socket_mode { return {ok: true, socket_mode: true} }` short-circuit.

**Keep:** Rate limiting, `challenge_state.issue().await`.

**Return:** `{expires_in: <ttl>, device_id: "..."}` on success.

```rust
pub async fn auth_challenge(
    maybe_addr: Option<ConnectInfo<SocketAddr>>,
    axum::Extension(challenge_state): axum::Extension<Arc<crate::auth_challenge::ChallengeState>>,
    axum::Extension(rate_limiter): axum::Extension<Arc<crate::rate_limit::RateLimiter>>,
) -> impl IntoResponse {
    if let Some(ConnectInfo(addr)) = maybe_addr {
        if let Err(_) = rate_limiter.check(addr) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": "rate_limited"})),
            ).into_response();
        }
    }

    match challenge_state.issue().await {
        Ok(_challenge_token) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "expires_in": crate::auth_challenge::CHALLENGE_TTL.as_secs(),
                "device_id": crate::device_id::get_or_create(),
            })),
        ).into_response(),
        Err(e) => {
            tracing::error!("Failed to issue auth challenge: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to issue challenge"})),
            ).into_response()
        }
    }
}
```

### 2.3 `auth_redeem` handler

**Remove:** `axum::Extension(socket_mode): axum::Extension<bool>` parameter.

**Remove:** `if socket_mode { return {ok: true, socket_mode: true} }` short-circuit.

**Keep:** `X-L337-Challenge` header extraction, `challenge_state.verify_and_consume()`,
`auth_layer.add_peer()` (instead of `update_token`).

```rust
pub async fn auth_redeem(
    State(_state): State<AppState>,
    axum::Extension(challenge_state): axum::Extension<Arc<crate::auth_challenge::ChallengeState>>,
    axum::Extension(auth_layer): axum::Extension<crate::security::AuthLayer>,
    axum::Extension(rate_limiter): axum::Extension<Arc<crate::rate_limit::RateLimiter>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let presented = match headers.get("X-L337-Challenge").and_then(|v| v.to_str().ok()) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing X-L337-Challenge header"})),
            ).into_response();
        }
    };

    let node_id = headers.get("X-L337-Node-ID")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match challenge_state.verify_and_consume(presented).await {
        Ok(_token) => {
            // Store peer DeviceID in allowlist (not the challenge value as a token)
            auth_layer.add_peer(crate::security::AllowedPeer {
                node_id: node_id.to_string(),
                token: _token,  // or derive from node_id
                account_id: None,
                verified: false,
                paired_at: chrono::Utc::now().to_rfc3339(),
                expires_at: None,
                capabilities: vec![],
                name: None,
                last_seen: chrono::Utc::now().to_rfc3339(),
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({"ok": true})),
            ).into_response()
        }
        Err(crate::auth_challenge::ChallengeError::Io(e)) => {
            tracing::error!("Challenge redeem IO error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            ).into_response()
        }
        Err(_) => {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid_or_expired_challenge"})),
            ).into_response()
        }
    }
}
```

---

## 3. `src/security.rs`

### 3.1 Change `AuthLayer` from single-token to allowlist

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedPeer {
    pub node_id: String,
    pub token: String,
    pub account_id: Option<String>,
    pub verified: bool,
    pub paired_at: String,
    pub expires_at: Option<String>,
    pub capabilities: Vec<String>,
    pub name: Option<String>,
    pub last_seen: String,
}

pub struct AuthLayer {
    allowlist: Arc<Mutex<HashSet<String>>>,  // set of valid tokens/DeviceIDs
    peers: Arc<Mutex<Vec<AllowedPeer>>>,     // full peer info for management
}

impl AuthLayer {
    pub fn new(initial_token: Option<String>) -> Self {
        let mut allowlist = HashSet::new();
        if let Some(token) = initial_token {
            allowlist.insert(token);
        }
        Self {
            allowlist: Arc::new(Mutex::new(allowlist)),
            peers: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn is_allowed(&self, token: &str) -> bool {
        self.allowlist.lock().unwrap().contains(token)
    }

    pub fn add_peer(&self, peer: AllowedPeer) {
        let token = peer.token.clone();
        self.peers.lock().unwrap().push(peer);
        self.allowlist.lock().unwrap().insert(token);
    }

    pub fn remove_peer(&self, node_id: &str) {
        let mut peers = self.peers.lock().unwrap();
        if let Some(idx) = peers.iter().position(|p| p.node_id == node_id) {
            let peer = peers.remove(idx);
            self.allowlist.lock().unwrap().remove(&peer.token);
        }
    }

    pub fn prune_expired(&self) -> usize {
        let mut removed = 0;
        let mut peers = self.peers.lock().unwrap();
        let mut allowlist = self.allowlist.lock().unwrap();
        peers.retain(|peer| {
            if let Some(expires_at) = &peer.expires_at {
                if chrono::DateTime::parse_from_rfc3339(expires_at).ok() < Some(chrono::Utc::now().into()) {
                    allowlist.remove(&peer.token);
                    removed += 1;
                    return false;
                }
            }
            true
        });
        removed
    }

    // Keep for backward compat / single-token mode
    pub fn update_token(&self, token: String) {
        self.allowlist.lock().unwrap().clear();
        self.allowlist.lock().unwrap().insert(token);
    }
}
```

### 3.2 Update `AuthMiddleware`

Change token validation to use `is_allowed()`:

```rust
pub async fn auth_middleware(
    axum::Extension(auth_layer): axum::Extension<Arc<AuthLayer>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_token(request.headers())?;
    if auth_layer.is_allowed(&token) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
```

---

## 4. `src/main.rs`

### 4.1 Remove `socket_mode` Extension

```rust
// Before
let socket_mode = use_socket;
// ...
.layer(Extension(socket_mode))

// After
// removed entirely
```

### 4.2 Add `/capabilities` route

```rust
let mut app = Router::new()
    .route("/capabilities", get(capabilities))
    // ... existing routes ...
```

### 4.3 DeviceID module

Create `src/device_id.rs`:
```rust
use std::fs;
use std::path::PathBuf;
use ed25519_dalek::{Keypair, PublicKey, SecretKey};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

pub fn get_or_create() -> String {
    let path = device_id_path();
    if let Ok(data) = fs::read(&path) {
        if let Ok(keypair) = Keypair::from_bytes(&data) {
            return device_id_from_public(&keypair.public);
        }
    }
    let keypair = Keypair::generate(&mut rand::thread_rng());
    fs::write(path, keypair.to_bytes()).ok();
    device_id_from_public(&keypair.public)
}

pub fn device_id_from_public(public: &PublicKey) -> String {
    let hash = sha2::Sha256::digest(public.as_bytes());
    BASE64.encode(hash)
}

fn device_id_path() -> PathBuf {
    // STATE_DIRECTORY/device_id.pem or ~/.cache/l337/l337-audio-server/device_id.pem
}
```

---

## 5. `src/api/models.rs`

No changes needed for `AuthChallengeResponse`. Keep `{expires_in: u64}`.

---

## 6. Tests

### 6.1 Server-side tests

Add:
- `test_capabilities_endpoint_public` — verify `GET /capabilities` returns node_id, capabilities, no auth required
- `test_challenge_response_contains_device_id` — verify `/auth/challenge` returns `device_id`
- `test_auth_redeem_adds_peer_to_allowlist` — verify `auth_layer.add_peer()` is called with peer DeviceID
- `test_auth_layer_allows_multiple_peers` — verify allowlist accepts multiple DeviceIDs simultaneously
- `test_auth_layer_prune_expired` — verify expired peers are removed
- `test_challenge_reused_when_active` — call `/auth/challenge` twice within TTL; verify same token returned

Update:
- Any test that mocks `socket_mode` or expects `{ok: true, socket_mode: true}` — remove those expectations.

### 6.2 Integration test

End-to-end test:
1. Start server in dummy mode with `--transport=socket`.
2. `GET /capabilities` → returns `{node_id, capabilities, ...}`
3. `POST /auth/challenge` → `{expires_in: 600, device_id: "..."}`
4. `cat` the `challenge-token.txt` from the state dir → note the value
5. `POST /auth/redeem` with `X-L337-Challenge` and `X-L337-Node-ID` → `{ok: true}`
6. `GET /player/status` with `Authorization: Bearer <derived_token>` → 200

Repeat over TCP:
1. Start server: `L337__SERVER__PORT=1337 ./bin/l337-audio-server --dummy --transport=http`
2. Steps 2-6 above.

---

## 7. Verification

```bash
CARGO_HOME=/tmp/cargo-home cargo check
CARGO_HOME=/tmp/cargo-home cargo test
```

Manual verification:
1. `GET /capabilities` → returns node_id, capabilities, version
2. `POST /auth/challenge` → `{"expires_in":600, "device_id":"..."}`
3. `POST /auth/challenge` again within 10 minutes → same `expires_in`, file unchanged
4. `cat` the `challenge-token.txt` → note the value
5. `POST /auth/redeem` with `X-L337-Challenge` and `X-L337-Node-ID` → `{"ok":true}`
6. `GET /player/status` with `Authorization: Bearer <derived_token>` → 200
7. Pair a second node → both DeviceIDs are in the allowlist → both can authenticate

---

## 8. What Is Deleted (No Fallbacks)

| Old method | Deleted from | Replacement |
|---|---|---|
| Socket bypass | `handlers.rs`, `main.rs` | Challenge/redeem runs over both transports |
| Single-token model | `security.rs` | Allowlist of DeviceIDs |
| Challenge overwrite | `auth_challenge.rs::issue()` | Reuse active challenge if still valid |
