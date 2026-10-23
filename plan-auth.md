# L337 Audio Server — Unified Auth: Infrastructure Spec

## 1. Goal

Challenge/redeem is the single auth method for both Unix-socket and HTTPS transports.
No static token discovery, no `/setup`, no socket bypass.

## 2. Hard Constraints

- **One node per physical device.** No virtualization, no multi-instance.
- **One auth code per node.** Each node has exactly one stable credential (DeviceID).
  Pairing adds the peer's DeviceID to the allowlist. The same DeviceID is used
  across all pairings.
- **`/capabilities` endpoint.** Every node exposes `GET /capabilities` (public, no auth)
  so peers can discover abilities before pairing.

## 3. Design Constraints

- **One method, no fallbacks.** Exactly one auth path in the codebase: challenge/redeem.
- **Socket = silent.** Local servers pair automatically at first connect. No user interaction.
- **TCP = manual.** Remote servers use `TokenPairingScreen`. Operator reads challenge from
  server host and enters it in the client.
- **Secure by design.** Challenge is never handed out over the network. Obtained exclusively
  through an out-of-band channel (server's local filesystem for socket, operator console/SSH
  for TCP). Mirrors Bluetooth-style pairing.
- **Multi-device list preserved.** The client maintains a list of paired servers. One is active
  at a time. Multi-output to multiple servers simultaneously is future scope, not part of this
  plan. Switching the active server pauses the previous one.

---

## 4. Protocol

```
Node A (controller)                     Node B (speaker)
  |                                       |
  |  GET /capabilities (no auth)          |
  |  ← {node_id, capabilities, ...}       |
  |                                       |
  |  POST /auth/challenge (no auth)       |
  |  ← {expires_in: 600, device_id: "B"}  |
  |                                       |  → generates nonce
  |                                       |  → writes challenge-token.txt
  |                                       |  → does NOT return the nonce in the response
  |                                       |
  |  [out-of-band: read challenge]         |
  |                                       |
  |  POST /auth/redeem                     |
  |  X-L337-Challenge: <nonce>            |
  |  X-L337-Node-ID: "A"                  |
  |  ← {ok: true}                         |
  |                                       |  → verifies nonce against challenge-token.txt
  |                                       |  → adds A's DeviceID to allowlist
  |                                       |  → deletes challenge-token.txt
  |                                       |
  |  GET /player/status                   |
  |  Authorization: Bearer <node_A_token> |
  |  ← 200 + status                       |
```

**The challenge value is never returned in the HTTP response.** The response contains only
`{expires_in: 600, device_id: "..."}`. The challenge value is obtained exclusively through
the out-of-band channel.

After successful redeem, the peer's DeviceID is stored in the allowlist. The bearer token
is a stable credential derived from the node's identity (not the one-time challenge value).

### 4.1 Server behavior on `/auth/challenge`

- If `challenge-token.txt` exists and is **not expired** (mtime < 10 minutes old), return
  `{expires_in: <remaining>}` **without generating a new challenge**. This prevents the
  challenge from being overwritten while the operator is in the middle of pairing.
- If `challenge-token.txt` does not exist or is expired, generate a fresh nonce, write it,
  and return `{expires_in: 600}`.

This is the critical behavior that makes the pairing flow robust: the operator can request
a challenge, read it, and be confident it won't be invalidated by a second request.

---

## 5. Challenge File Location

Server writes `challenge-token.txt` to the same directory used for `server_token.txt`:
- `STATE_DIRECTORY` (systemd: `/var/lib/l337-audio-server/`) when available
- `~/.cache/l337/l337-audio-server/` (desktop/dev) otherwise

File permissions: `0600` (owner read/write only).

---

## 6. /capabilities Endpoint

Every node exposes `GET /capabilities` (public, no auth required):

```json
{
  "node_id": "MFZWI3D-BONSGYC-YLTMRWG-C43ENR5...",
  "name": "Foyer Speaker",
  "capabilities": ["output", "display", "voting"],
  "version": "0.1.0",
  "transport": "socket",
  "trust_group": "main-pa",
  "challenge_ttl": 600
}
```

This allows nodes to discover each other's abilities before pairing.

---

## 7. Socket vs TCP

| Property | Socket (`unix://`) | TCP (`https://`) |
|---|---|---|
| Transport | Unix domain socket | HTTPS (self-signed or configured) |
| Challenge delivery | Client reads local `challenge-token.txt` | Operator reads file on server host (SSH/console) |
| Pairing UX | Silent (background, no UI) | `TokenPairingScreen` — user enters challenge |
| Retry on token reject | Silent auto-redeem | Silent auto-redeem after initial manual pair |
| Token cache | `~/.config/l337-player/` | `~/.config/l337-player/` |

The protocol is identical. The UX differs.

---

## 8. Socket Silent-Pair Flow

1. Client queries `GET /capabilities` → learns server's DeviceID and capabilities.
2. Client resolves `unix:///run/l337-audio-server/l337.sock`.
3. `connect_audio_server()` probes `/health` → reachable.
4. `get_status()` with cached DeviceID token → 401 if token missing/rejected.
5. `_silent_pair()`:
   a. `POST /auth/challenge` → server writes `challenge-token.txt` (or reuses existing),
      returns `{expires_in: 600, device_id: "B"}`.
   b. Client reads `challenge-token.txt` from local filesystem (out-of-band).
   c. `POST /auth/redeem` with `X-L337-Challenge` and `X-L337-Node-ID: "A"` → `{ok: true}`.
   d. Server adds client's DeviceID to its allowlist.
   e. Client caches the pairing (peer DeviceID + derived token) in `~/.config/l337-player/`.
6. Client retries `get_status()` with derived token → 200.
7. User sees no UI. Flow completes in ~100ms.

---

## 9. TCP Manual-Pair Flow

1. User queries `GET /capabilities` on the remote node → learns DeviceID and capabilities.
2. User opens Settings → Audio Server → "Remote".
3. User taps "+ Pair New Device", enters `https://<host>:1337`.
4. Client calls `POST /auth/challenge` → server writes `challenge-token.txt` (or reuses
   existing), returns `{expires_in: 600, device_id: "B"}`.
5. `TokenPairingScreen` shows: "Read the challenge code from the server host (e.g., via SSH:
   `cat /var/lib/l337-audio-server/challenge-token.txt`) and paste it below."
6. Operator reads challenge from server host (out-of-band), enters it in UI.
7. Client calls `POST /auth/redeem` with `X-L337-Challenge` and `X-L337-Node-ID: "A"` → `{ok: true}`.
8. Server adds client's DeviceID to its allowlist. Client saves paired device (peer DeviceID + derived token). Shows "Paired!" notification.
9. Subsequent connects use derived token. On rejection, silent re-pair or user re-pair.

---

## 10. Multi-Device Switching (Preserved)

The client maintains a list of paired DeviceIDs in `paired_devices`. Exactly one is active
at a time (`active_paired_device`). Each entry is a peer DeviceID with a derived token.

When the user switches the active server:
- The client calls `/player/pause` on the previous server (best-effort, errors swallowed).
- The new server's DeviceID is used for subsequent requests.

This is unchanged from the current behavior. The auth per device is still challenge/redeem;
the only difference is that pairing results in a DeviceID binding, not a one-time token.

Future scope: multi-output to multiple servers simultaneously. Not part of this plan.

---

## 11. Trust Groups (One Pairing = Access to All)

A trust group is a set of nodes that share pairing allowlists. When a node pairs with
one member of a group, its DeviceID is propagated to all members. This enables:
- PA systems: DJ pairs with controller, gets access to all 15 speakers
- Silent disco: controller pairs once, all headphones accept it
- Multi-room: one pairing grants access to all rooms in the group

Trust group membership is configured in `server.ini`:
```ini
[server]
trust_group = "main-pa"
```

Or left empty for standalone nodes.

When Node A pairs with Node B (both in group "main-pa"):
1. Node B adds A's DeviceID to its allowlist
2. Node B propagates A's DeviceID to all other nodes in "main-pa"
3. All nodes in the group now accept requests from A

Propagation can be direct HTTP, multicast, or manual config depending on the deployment.

---

## 12. Security Properties

| Property | How |
|---|---|
| No static password | Each pairing uses a unique, single-use, time-limited nonce. Server's `server_token` never leaves the server. |
| Challenge not exposed over network | Server returns only `{expires_in: 600, device_id: "..."}`. Challenge value is never transmitted server→client over HTTP. |
| Out-of-band channel required | Socket: filesystem access to server state dir. TCP: operator SSH/console access. Attacker with only network access cannot obtain challenge. |
| Active challenge not overwritten | Server reuses existing challenge if still valid. Prevents operator's challenge from being clobbered mid-pairing. |
| Single-use | Server deletes `challenge-token.txt` after successful redeem. Replay is impossible. |
| Time-boxed | 10-minute TTL via file mtime check. |
| Rate-limited | 5 requests per 60s per IP on `/auth/challenge`. Redeem implicitly limited by single-use. |
| One auth code per node | Each node has exactly one stable DeviceID. Pairing adds DeviceIDs to allowlists. |
| Socket transport | Unix socket `0600` (server user only). Challenge file readable only by server user. |
| TCP transport | Challenge travels over HTTPS. TLS protects redeem request in transit. Challenge itself never transmitted server→client. |
| Client token storage | Token stored in client's own `~/.config/l337-player/`, owned by client user. |
| No token leakage via files | Client no longer reads `/etc/l337-audio-server/server.ini` or `~/.cache/l337/l337-audio-server/server_token.txt`. |

---

## 13. Migration

### Existing users

| Existing config | New behavior |
|---|---|
| `server_token` in `config.ini` | Preserved. Used as first attempt. If it matches server's current token, no pairing needed. |
| `paired_devices` / `named_servers` | Preserved. DeviceIDs in entries are used as cached credentials. |
| `/etc/l337-audio-server/server.ini` token | No longer read by client. Server still uses it as its initial token. |
| `~/.cache/l337/l337-audio-server/server_token.txt` | No longer read by client. Safe to delete. |

### First-connect after upgrade

1. Client tries cached DeviceID token → if it matches, succeeds.
2. If cached token doesn't match:
   - **Socket:** client silently re-pairs via challenge/redeem → new DeviceID cached → succeeds.
   - **TCP:** client surfaces "token rejected"; user re-pairs via `TokenPairingScreen`.

---

## 14. Open Decisions

1. **Multi-output:** When implemented, each server in the multi-output list will need its own
   active pairing. The `paired_devices` data model already supports this; only the playback
   fan-out logic needs to change.

2. **Server should not overwrite active challenge:** Implemented in this plan. If a valid
   `challenge-token.txt` exists, return existing `expires_in` instead of generating a new
   token. Prevents attacker (or accidental double-request) from invalidating an in-progress
   pairing.

3. **DeviceID format:** Ed25519 keypair (Syncthing-style) or UUID? Keypair enables mutual
   auth but adds complexity. UUID is simpler but less secure.

4. **Bearer token derivation:** How is the stable node token derived from the DeviceID?
   Options: direct DeviceID as token, HMAC(DeviceID, shared_secret), or keypair signature.
