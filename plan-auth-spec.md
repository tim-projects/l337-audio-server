# L337 Audio System — Node Architecture Spec

## 1. Purpose

This spec defines the full system architecture for l337 as a distributed audio system.
There is no "server" and "client" in the traditional sense. There are **nodes**, each with
a stable identity, a set of capabilities, and peer-to-peer authenticated communication.

---

## 2. Node Model

### 2.1 What Is a Node

A node is any device running l337 software. It has:
- A stable identity (Device ID)
- A set of capabilities (what it can do)
- Auth: can authenticate other nodes and be authenticated by them
- Distribution: how it distributes audio/content to other nodes
- Bidirectional communication: can send commands and receive events

### 2.2 Node Capabilities

| Capability | Description | Examples |
|---|---|---|
| `output` | Can play audio | Speakers, headphones, Pi, car audio |
| `input` | Can capture audio | Mic, line-in, DJ deck |
| `control` | Can send play/queue/volume commands | DJ laptop, phone, web UI |
| `display` | Can show now-playing / queue | Screen on speaker, phone screen |
| `search` | Can search for music and resolve playback URLs | Phone, web UI, desktop |
| `playlist` | Can manage playlists and aggregate votes | Phone, web UI, DJ laptop |
| `connectivity` | Network/transport connectivity options | Bluetooth, WWAN, local, VPN, etc. |
| `schedule` | Supports alarms, sleep timers, timed playback | Bedside speaker, wake-up alarm |
| `sync` | Can participate in synchronized multi-node playback | PA system, multi-room speakers |
| `record` | Can capture and persist audio locally | DJ deck, recording node |
| `relay` | Can forward messages to other nodes | PA controller, mesh hub |

A node can have multiple capabilities. A DJ laptop might have `input + control + display + search + playlist + connectivity`.
A silent disco headphone might have `output + voting + display + connectivity`. A PA speaker might have `output + display + sync`.
A website backend might have `playlist + search`. A phone remote might have `control + display + search + playlist + connectivity`.

### 2.3 Node Configurations

These are common combinations, not fixed roles:

| Configuration | Capabilities | Examples |
|---|---|---|
| Output node | `output + display + connectivity` | PA speaker, installed speaker, Bluetooth speaker |
| Controller node | `control + display + search + playlist + connectivity` | DJ laptop, phone, web UI |
| Hybrid node | `output + control + display + search + playlist + connectivity` | Desktop, DJ laptop with local output |
| Input node | `input + control` | DJ deck, mic station |
| Voting terminal | `voting + display + connectivity` | Silent disco headphone, voting app |
| Website / Backend | `playlist + search` | Web interface managing shared playlists |
| Relay node | `relay + control + display + playlist + connectivity` | PA controller, mesh hub |
| Remote VPS | `output + relay + connectivity` | Cloud server reachable over VPN |
| Phone remote | `control + display + search + playlist + connectivity` | Phone app with Bluetooth + WWAN |
| PA System | `output + display + sync + connectivity` | Multi-speaker synchronized PA |
| Bedside Speaker | `output + display + schedule + connectivity` | Alarm clock speaker |
| Recording Node | `input + control + record + connectivity` | DJ deck or recording station |

---

## 3. Communication Model

### 3.1 Peer-to-Peer, Not Hierarchical

There is no "server" that all clients talk to. Nodes communicate directly:
- Controller → Output: "play this track"
- Output → Controller: "now playing: Song X, votes: 42"
- Voting terminal → Output: "vote: like"
- Output → Voting terminal: "current track: Song X"
- Relay → Output: "forwarded command from controller"

### 3.2 Transport

Nodes can communicate over:
- **Unix socket** (local, same host)
- **TCP/HTTPS** (local network, internet)
- **Bluetooth RFCOMM / BT PAN** (Bluetooth headphones)
- **Multicast** (local network discovery, trust group propagation)

The transport is selected based on reachability, not role.

### 3.3 Bidirectional Messaging

The server pushes events to connected clients:
- `now_playing` — track changed
- `vote_update` — vote counts changed
- `queue_update` — queue changed
- `session_update` — client connected/disconnected
- `volume_update` — volume changed

The client can poll or use long-polling / WebSocket for push events.

---

## 4. Authentication in a Node Model

### 4.1 Peer Authentication

Every node has a stable Device ID. When two nodes connect, they authenticate each other:
- Node A presents its token to Node B
- Node B validates the token against its allowlist
- Node B presents its token to Node A (optional, for mutual auth)
- Both nodes verify each other's Device ID

This is symmetric: any node can challenge any other node.

### 4.2 Challenge/Redeem (Pairing)

When two nodes haven't paired before:
1. Node A calls `POST /auth/challenge` on Node B
2. Node B writes `challenge-token.txt`, returns `{expires_in: 600, device_id: "..."}`
3. Node A obtains the challenge out-of-band (reads file for local, operator entry for remote)
4. Node A calls `POST /auth/redeem` with `X-L337-Challenge`
5. Node B verifies, adds Node A's token to its allowlist
6. Node A adds Node B's token to its allowlist (mutual pairing)

After pairing, both nodes trust each other.

### 4.3 Local Trust (Same Device)

When two nodes run on the same device (e.g., desktop with local output):
- They communicate over a Unix socket
- Trust is established by peer credentials (`SO_PEERCRED`) or a local bypass header
- No challenge/redeem needed
- This is the "server+client" case from the old model

### 4.4 Website Verification (Optional)

For internet-connected nodes, the challenge/redeem flow can include website verification:
- Node A includes `X-L337-Web-Token` (its l337 website session)
- Node B calls the website API to verify Node A's identity
- If verified, Node B stores `account_id` in the allowlist

This is optional. Local/offline nodes skip it.

### 4.5 Allowlist (Per-Node)

Each node maintains its own allowlist of trusted peers:

```json
[
  {
    "token": "paired-token-1",
    "node_id": "node-uuid-1",
    "account_id": "user123",
    "verified": true,
    "paired_at": "2026-01-01T00:00:00Z",
    "expires_at": null,
    "capabilities": ["output", "display"],
    "name": "Foyer Speaker"
  }
]
```

The allowlist is per-node, not global. Each node decides who it trusts.

---

## 5. Auth Protocol

### 5.1 Challenge/Redeem

```
Node A (controller)                     Node B (speaker)
  |                                       |
  |  POST /auth/challenge (no auth)       |
  |  ← {expires_in: 600, device_id: "B"}  |
  |                                       |  → writes challenge-token.txt
  |                                       |
  |  [out-of-band: read challenge]         |
  |                                       |
  |  POST /auth/redeem                     |
  |  X-L337-Challenge: <nonce>            |
  |  X-L337-Web-Token: <optional>         |
  |  ← {ok: true}                         |
  |                                       |  → verifies challenge
  |                                       |  → adds A to B's allowlist
  |                                       |
  |  GET /player/now-playing              |
  |  Authorization: Bearer <token>        |
  |  ← 200 + now-playing                  |
```

### 5.2 Authenticated Request

```
Node A                                     Node B
  |                                       |
  |  GET /player/now-playing              |
  |  Authorization: Bearer <token>        |
  |  ← 200 + now-playing                  |
```

### 5.3 Push Events (Server → Client)

```
Node B (speaker)                         Node A (controller)
  |                                       |
  |  POST /events (long-poll / WS)        |
  |  ← {event: "vote_update",             |
  |       data: {track: "X", likes: 42}}  |
```

Or via WebSocket:
```
Node B → Node A: {"event": "now_playing", "data": {"track": "Song X"}}
Node B → Node A: {"event": "vote_update", "data": {"track": "Song X", "likes": 42}}
```

### 5.4 Local Trust Bypass (Same Device)

```
Node A (local controller)                Node B (local output)
  |                                       |
  |  GET /player/now-playing              |
  |  X-L337-Local-Bypass: true            |
  |  ← 200 + now-playing                  |
  |                                       |  → verifies SO_PEERCRED
```

Only works over Unix socket. Never works over TCP.

---

## 6. Interactive Features

### 6.1 Voting

Nodes with the `voting` capability can send votes to nodes with the `output` capability:

```
Voting node → Output node: POST /player/vote {track_id: "X", vote: "like"}
Output node → All controllers: {event: "vote_update", data: {track_id: "X", likes: 42, dislikes: 3}}
```

Voting is per-track. The server tracks vote counts and can trigger actions:
- Skip track if dislikes > threshold
- Replay track if likes > threshold
- Show vote counts on display nodes

### 6.2 Now-Pushing / Queue

The controller node manages the queue. It sends the next track to the output node:
```
Controller → Output: POST /player/queue/add {track: "Song X"}
Output → Controller: {event: "queue_update", data: {queue: [...]}}
```

The output node can prefetch/cache the next track if it has storage.

### 6.3 Session Management

Nodes can see who else is connected:
```
Controller → Output: GET /player/session
Output → Controller: {clients: [{node_id: "A", capabilities: ["control"]}, ...]}
```

---

## 7. Trust Groups

### 7.1 Purpose

A trust group is a set of nodes that share pairing tokens. This enables:
- PA systems: temporary DJ token propagates to all speakers
- Silent disco: all headphones share event tokens
- Multi-room: tokens valid across all rooms

### 7.2 Configuration

Each node can belong to a trust group:
```ini
[server]
trust_group = "main-pa"
```

Or be standalone (no group).

### 7.3 Token Propagation

When a node pairs with another node in the same trust group, the token is propagated to all nodes in the group:

```
Node A pairs with Node B (group: "main-pa")
Node B → Node C (same group): {token: "new-token", expires_at: "+24h"}
Node B → Node D (same group): {token: "new-token", expires_at: "+24h"}
```

Propagation can be:
- Direct HTTP between nodes
- Multicast on local network
- Manual config (for offline groups)

### 7.4 Group-Wide Expiry

When a temporary token expires, all nodes in the group remove it simultaneously. This is enforced by each node independently (no central authority).

---

## 8. Scenario Coverage

### 8.1 Home Audio

- 1-5 nodes: desktop (hybrid), speaker (output), phone (controller)
- Pairing: permanent, per-node
- Trust: local bypass for desktop→local speaker, challenge/redeem for phone→speaker
- No website verification needed (optional)

### 8.2 Installed PA System

- 10-50 nodes: speakers (output + display), PA controller (relay + control)
- Pairing: permanent for installer, temporary for DJ
- Trust group: `main-pa`
- Physical pairing mode: button press / QR scan on speakers
- DJ gets 24h temporary token, propagated to all speakers

### 8.3 Silent Disco

- 100-500 nodes: headphones (output + voting + display)
- 1 controller node: DJ laptop (control)
- Pairing: pre-provisioned before event
- Trust group: `event-2026-09-27`
- Hierarchical trust: shared event secret + per-headphone tokens
- No internet: venue WiFi only

### 8.4 Bluetooth Headphones

- 2 nodes: headphones (output + voting), phone (control + input)
- Transport: Bluetooth RFCOMM or BT PAN
- Pairing: challenge/redeem over BT, no website
- Trust: OS-level Bluetooth pairing + l337 challenge/redeem

### 8.5 Remote VPS / Cloud

- 2+ nodes: server (output), client (control)
- Transport: TCP/HTTPS over internet
- Pairing: challenge/redeem + website verification
- Trust: website API confirms client identity

---

## 9. Security Model

### 9.1 Trust Levels

| Trust Level | Description | Use Case |
|---|---|---|
| **Local bypass** | Same device, Unix socket, peer credentials | Hybrid nodes on same device |
| **Unverified local** | Same host, different user, Unix socket | Multi-user desktop |
| **Unverified network** | Same network, no website verification | BT, local WiFi, offline |
| **Verified remote** | Internet, website verification | VPS, cloud |
| **Physical pairing** | Button press / QR required | PA, silent disco |

### 9.2 Attack Mitigations

| Attack | Mitigation |
|---|---|
| Attacker on same WiFi requests challenge | Physical pairing mode (button press) |
| Attacker intercepts challenge | Single-use + time-limited + rate-limited |
| Attacker brute-forces challenge | Rate limit (5/60s per IP) |
| Attacker reuses old token | Allowlist + expiry enforcement |
| Attacker impersonates node | Device ID verification |
| Attacker compromises website | Fallback to unverified mode |
| Attacker pairs and never uses token | Expiry enforcement removes unused tokens |

### 9.3 Network Binding

Nodes bind only to private/local interfaces by default:
- `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
- `169.254.0.0/16`, `fe80::/10`, `fc00::/7`

Internet-facing binding is opt-in (`bind_public = true`).

---

## 10. Implementation Phases

### Phase 1: Core Auth
- Challenge/redeem as single auth method
- Allowlist with expiry
- Socket silent-pair + TCP manual pair
- Website verification optional
- Remove old methods

### Phase 2: Bidirectional Communication
- Push events (now-playing, vote updates)
- Session management
- Voting endpoints

### Phase 3: Trust Groups
- Trust group configuration
- Token propagation between group members
- Group-wide expiry enforcement

### Phase 4: Silent Disco / Bulk Provisioning
- Hierarchical trust (shared secret + per-device pairing)
- Bulk provisioning CLI
- QR/NFC provisioning

### Phase 5: Physical Pairing UX
- Button press, QR, NFC pairing modes
- Device identity display and verification

---

## 11. Open Questions

1. **Device identity format:** Ed25519 keypair (Syncthing-style) or simple UUID? Keypair enables mutual auth but adds complexity.
2. **Trust group propagation protocol:** HTTP between nodes? Multicast? Manual config?
3. **Voting system:** simple like/dislike, or weighted (DJ has more weight)? Per-session or per-track?
4. **Website API scope:** does the website need to know which node the client is pairing with? Can the same website token be used for multiple nodes?
5. **Silent disco provisioning:** how are initial shared secrets loaded onto headphones at the factory? Secure provisioning protocol?
6. **Silent disco revocation:** if the event secret is compromised, can we rotate it without re-pairing all 500 headphones?
7. **Local bypass security:** is `SO_PEERCRED` sufficient, or do we need an additional local token?
8. **Multi-output sync:** when a node streams to multiple output nodes simultaneously, how is sync handled? Does the controller fan out, or do output nodes pull from the controller?
