# L337 Audio Server — Auth Options

## Background: Why Syncthing Doesn't Have This Problem

Syncthing uses persistent device identities, not rotating tokens. Each device generates a keypair on first run. The Device ID is a stable hash of the public certificate. Pairing is additive: you add a new device ID to your config list. Existing device IDs remain valid. Authentication is TLS mutual auth based on the peer's stable identity, not a shared secret that rotates.

The "new pairing invalidates old clients" problem cannot happen in Syncthing because:
- Pairing is additive (add to config list)
- Authentication is based on stable identity (Device ID), not a rotating token
- The server maintains a list of authorized peers

---

## Options for l337-audio-server

### Option 1: Challenge/redeem + optional website verification

The challenge/redeem flow is the primary mechanism. During `POST /auth/redeem`, the client optionally includes a website auth token (`X-L337-Web-Token` header). The server:

1. Verifies the challenge as usual
2. If a web token is present, calls the l337 website API: `POST /api/verify-token {token: "..."}`
3. Website responds: `{valid: true, account_id: "user123", display_name: "Alice"}`
4. Server stores the pairing as: `{token: challenge_value, account_id: "user123", verified: true, paired_at: "..."}`
5. If no web token is presented, server stores: `{token: challenge_value, account_id: null, verified: false, paired_at: "..."}`

The server can enforce policy:
- **Permissive:** accept both verified and unverified clients
- **Strict:** only accept verified clients for remote/TCP; accept unverified for local/socket

**Pros:**
- Website verification is opt-in per client — no forced dependency
- Server doesn't manage auth, just delegates verification
- Adds accountability: every pairing is tied to an account ID
- Graceful degradation: if the website API is down, local pairings still work

**Cons:**
- The website API must be reachable from the server
- If the website API is down, remote verification fails

---

### Option 2: Website-verified pairing required for remote, optional for local (RECOMMENDED)

Same as Option 1, but the server enforces:
- **Socket (local):** challenge/redeem without website verification is sufficient. Pairing succeeds with `verified: false`.
- **TCP (remote):** challenge/redeem MUST include a valid website token. Server rejects redeem if `X-L337-Web-Token` is missing or invalid.

This gives remote servers the strongest security model (website-verified identity + challenge/redeem) while keeping local pairing zero-friction.

**Protocol:**

```
Client (remote)                              Server
  |                                           |
  |  POST /auth/challenge (no auth)           |
  |  ← {expires_in: 600}                      |
  |                                           |
  |  [out-of-band: operator reads file]        |
  |                                           |
  |  POST /auth/redeem                         |
  |  X-L337-Challenge: <nonce>                |
  |  X-L337-Web-Token: <website_session>      |
  |  ← {ok: true}                             |
  |                                           |  → verifies challenge
  |                                           |  → calls website API
  |                                           |  → stores {token, account_id, verified}
  |                                           |
  |  GET /player/status                       |
  |  Authorization: Bearer <token>            |
  |  ← 200 + status                           |
```

**Server-side:**
- `paired_clients.json` stores `{token, account_id, verified, paired_at}`
- `POST /auth/redeem` accepts optional `X-L337-Web-Token` header
- If present, server calls website API to verify
- If invalid/missing and URL is TCP, reject with 401
- If invalid/missing and URL is Unix socket, accept (local trust model)

**Client-side:**
- `redeem_challenge()` accepts optional `web_token` parameter
- For TCP pairings, client includes `X-L337-Web-Token: <website_session>` header
- For socket pairings, no web token needed

**Website API (new endpoint):**
```
POST /api/verify-token
Authorization: Bearer <server_api_secret>
Body: {token: "<client_session_token>"}
Response: {valid: true, account_id: "user123", display_name: "Alice"}
```

The server authenticates to the website API with a shared secret (configured in `server.ini`).

**Pros:**
- Remote pairing has the strongest security model
- Local pairing remains zero-friction
- Account IDs are stored for accountability
- Server doesn't manage auth, just delegates verification

**Cons:**
- Clients must be logged into the l337 website to pair with remote servers
- The website API must be reachable from the server for remote pairings

---

### Option 3: Website-issued pairing tokens (server never talks to website)

The server never calls the website API. Instead:
1. Client logs into the l337 website
2. Client requests a pairing token from the website: `POST /api/pairing-token {server_id: "xxx"}`
3. Website generates a short-lived signed token (e.g., JWT) containing `{account_id, server_id, exp: 5m}`
4. Client presents this signed token to the server during `POST /auth/redeem`
5. Server verifies the signature using the website's public key (embedded in server config at install time)
6. If valid, server pairs the client

The server only needs the website's public key — it never makes an API call.

**Pros:**
- Server doesn't need internet access or the website API to be up
- Verification is offline (just signature checking)
- Website controls which clients can pair with which servers

**Cons:**
- More complex: requires JWT or similar signing infrastructure on the website
- The server needs the website's public key in its config
- Revocation is harder (website can't tell the server to revoke a client)

---

### Option 4: Full Syncthing-style persistent identity

Each client has a persistent keypair. The client's "device ID" is derived from its public key. Pairing adds the client's device ID to the server's allowlist. The server doesn't need a website at all — identity is cryptographic.

**Pros:**
- No website dependency
- Stable identity independent of network
- No rotating tokens

**Cons:**
- More complex client-side (keypair generation and storage)
- No account-ID linkage to l337 website
- Doesn't match the "website-verified identity" requirement

---

## Offline / Embedded Devices

### The Problem

Some servers run on devices with no internet connection:
- Bluetooth headphones with an embedded l337 server
- A Raspberry Pi on a local network with no WAN
- A car audio system
- An airplane in-flight entertainment system

These devices cannot call the website API to verify client identity. They need a pairing method that works entirely offline.

### Key Insight: Transport Is Not Trust

The trust model should be based on **reachability and out-of-band access**, not transport type:
- A Bluetooth audio server is a local/offline server, regardless of whether it exposes a Unix socket or TCP over BT PAN
- A remote VPS is an internet server
- A local systemd server is a local server

Website verification is an **optional enhancement for internet-connected servers**, not a requirement for pairing.

### Bluetooth Pairing Model

For embedded devices like Bluetooth headphones:

1. **The device exposes a l337 audio server** over a local transport:
   - Bluetooth RFCOMM mapped to a Unix socket path (e.g., `/dev/rfcomm0` → `/run/l337-audio-server/l337.sock`)
   - Or TCP over BT PAN (e.g., `tcp://192.168.2.1:1337`)
   
2. **The OS-level Bluetooth pairing is the lower trust anchor.** The user has already paired their phone with the headphones via Bluetooth (PIN, SSP, or Just Works). This establishes that the phone is physically near the headphones and authorized at the OS level.

3. **l337's challenge/redeem runs on top of the Bluetooth transport.** The client connects to the l337 server over the Bluetooth link and runs the same challenge/redeem flow as any local server. Since the device has no internet, website verification is skipped.

4. **Pairing is classified as `verified: false`** (no website verification). The server may optionally require the client to be on the same Bluetooth link (peer credential check) in addition to challenge/redeem.

### Revised Trust Model

| Scenario | Transport | Website verification | Trust anchor |
|---|---|---|---|
| Local systemd server | Unix socket | Skipped | Filesystem access to challenge file |
| Bluetooth headphones | Unix socket or BT PAN TCP | Skipped (no internet) | OS-level Bluetooth pairing + challenge/redeem |
| Local dev server | Unix socket | Skipped | Filesystem access to challenge file |
| Remote VPS / cloud | TCP/HTTPS | Required | Website API + challenge/redeem |
| LAN server with internet | TCP/HTTPS | Optional | Website API + challenge/redeem |
| Airplane IFE | Local TCP (no internet) | Skipped | Local network access + challenge/redeem |

### Protocol for Offline Devices

```
Client (Bluetooth phone)                Server (headphones, no internet)
  |                                       |
  |  POST /auth/challenge (no auth)       |
  |  ← {expires_in: 600}                  |
  |                                       |  → writes challenge-token.txt
  |                                       |  → returns {expires_in} (no challenge in response)
  |                                       |
  |  [out-of-band: client reads file or   |
  |   operator reads on server host]       |
  |                                       |
  |  POST /auth/redeem                     |
  |  X-L337-Challenge: <nonce>            |
  |  ← {ok: true}                         |
  |                                       |  → verifies challenge
  |                                       |  → stores {token, account_id: null, verified: false}
  |                                       |
  |  GET /player/status                   |
  |  Authorization: Bearer <token>        |
  |  ← 200 + status                       |
```

The protocol is identical to the local socket flow. No website call is made. The pairing is stored with `verified: false`.

### Server-Side: Skip Website Verification When Unreachable

The server should gracefully handle website API unavailability:

```python
# Pseudocode for redeem handler
async def auth_redeem(request):
    challenge = request.headers.get("X-L337-Challenge")
    web_token = request.headers.get("X-L337-Web-Token")
    
    # Verify challenge (always required)
    token = verify_and_consume(challenge)
    
    # Try website verification if web token is present
    account_id = None
    verified = False
    if web_token:
        try:
            account_info = await call_website_api(web_token)
            account_id = account_info["account_id"]
            verified = True
        except WebsiteAPIError:
            # Website is unreachable — reject if policy requires verification
            if request.is_tcp and policy.require_web_verification:
                return 401, {"error": "website_verification_required"}
            # Otherwise accept without verification
            verified = False
    
    # Store pairing
    add_to_allowlist(token, account_id=account_id, verified=verified)
    return 200, {"ok": true}
```

Policy options:
- **Permissive (default):** accept pairings without website verification. Log whether verified or not.
- **Strict:** require website verification for TCP pairings. Reject if website is unreachable.
- **Offline mode:** skip website verification entirely (for known-offline devices).

### Client-Side: Include Web Token When Available

The client includes `X-L337-Web-Token` only when it has a valid l337 website session:

```python
async def redeem_challenge(client, challenge, web_token=None):
    headers = {"X-L337-Challenge": challenge}
    if web_token:
        headers["X-L337-Web-Token"] = web_token
    return await client.post("/auth/redeem", headers=headers)
```

For offline devices (Bluetooth, local Pi), the client doesn't have a web token and doesn't include the header. The server accepts without verification.

### Bluetooth-Specific Considerations

If the l337 server runs on a Bluetooth audio device:

1. **Transport binding:** The server binds to a Unix socket backed by RFCOMM, or to a TCP port on the BT PAN interface. The challenge/redeem flow works identically over either.
2. **Peer credential check (optional):** The server can check `SO_PEERCRED` on the Unix socket to verify the client process is running as the same user who paired via Bluetooth. This adds a second factor without user interaction.
3. **No website dependency:** The server never attempts website verification. The pairing is `verified: false`.
4. **Challenge file is local:** The operator can read the challenge from the device's local filesystem (if accessible) or via a local management interface. For Bluetooth headphones, the challenge might be displayed on the phone screen via the l337 app (the app connects to the server, requests a challenge, and displays it locally — no file reading needed because the server can return the challenge in the response for the Bluetooth case).

Wait — that last point is important. For Bluetooth, the client IS connected to the server over the Bluetooth link. The server could return the challenge in the response for this specific case, because the Bluetooth link is the trust anchor (OS-level pairing already happened). This would simplify the UX: no file reading needed.

But that's a transport-specific exception. The general rule is: challenge is never returned in the response. For Bluetooth specifically, we could add a flag like `X-L337-Trusted-Transport: bluetooth` that tells the server "I'm on a trusted link, return the challenge in the response."

This is getting into implementation details. The key point for the plan is:

> **The challenge/redeem protocol is universal. Website verification is optional. Bluetooth and offline devices use the same protocol without website verification. No separate pairing method is needed.**

---

## PA System / Multi-Speaker Scenarios

### The Problem with Per-Speaker Pairing

A permanent installed PA system (e.g., 15 speakers in a venue) works well with per-speaker pairing: the installer pairs once with each speaker, and those pairings last forever.

But a **temporary user** like a DJ at a club needs different behavior:
- The DJ shouldn't have to pair with 15 speakers individually
- The DJ's access should be time-limited (e.g., 24 hours for the event)
- After the event, the DJ's access automatically expires on all speakers

This requires two pairing modes:

### Pairing Mode 1: Permanent Pairing (Installed Systems)

- **Scope:** one client ↔ one speaker
- **Expiry:** none (or very long, e.g., 1 year)
- **Use case:** permanently installed PA systems, home audio
- **Pairing flow:** operator physically accesses the speaker, enters pairing mode (button press / QR scan), client pairs
- **Revocation:** manual only (operator removes client from allowlist)

### Pairing Mode 2: Temporary / Event Pairing (DJ, Guests)

- **Scope:** one client ↔ PA system (all speakers)
- **Expiry:** configurable, default 24 hours
- **Use case:** DJ at a club, guest at a party, event
- **Pairing flow:** DJ pairs once with the PA controller (or any speaker in the system), gets a time-limited token
- **Token propagation:** the paired speaker propagates the temporary token to all other speakers in the system
- **Auto-expiry:** after `expires_at`, the token is automatically invalidated on all speakers
- **Revocation:** automatic (expiry) or manual (operator revokes early)

### How Temporary Pairing Works

The PA system has a concept of a **trust group** — a set of speakers that share the same pairing tokens. When a client pairs with one speaker in the group with a temporary token, that speaker propagates the token to all other speakers in the group.

```
DJ laptop                           Speaker 1 (PA controller)        Speaker 2 ... Speaker 15
  |                                    |                              |
  | POST /auth/challenge               |                              |
  | ← {expires_in: 600}               |                              |
  |                                    |                              |
  | POST /auth/redeem                  |                              |
  | X-L337-Challenge: <nonce>         |                              |
  | X-L337-Web-Token: <dj_session>    |                              |
  | ← {ok: true, token: "temp-abc",   |                              |
  |        expires_at: "+24h",         |                              |
  |        group: "main-pa"}          |                              |
  |                                    | → propagates to Speakers 2-15 |
  |                                    | ← {ok} x15                   |
  | GET /player/status (Speaker 1)    | ← 200                        |
  | GET /player/status (Speaker 2)    | ← 200                        |
  | ...                                | ...                          |
```

After 24 hours, all speakers automatically remove the temporary token from their allowlists. The DJ loses access to all speakers simultaneously.

### Allowlist Structure with Expiry

```json
[
  {
    "token": "permanent-token-1",
    "client_id": "installer-uuid",
    "account_id": "user123",
    "verified": true,
    "paired_at": "2026-01-01T00:00:00Z",
    "expires_at": null,
    "group": "main-pa",
    "last_seen": "2026-01-02T00:00:00Z"
  },
  {
    "token": "temp-token-dj",
    "client_id": "dj-laptop-uuid",
    "account_id": "user456",
    "verified": true,
    "paired_at": "2026-09-27T20:00:00Z",
    "expires_at": "2026-09-28T20:00:00Z",
    "group": "main-pa",
    "last_seen": "2026-09-27T21:00:00Z"
  }
]
```

When validating a token, the server checks:
1. Is the token in the allowlist?
2. Is `expires_at` null (permanent) or in the future (not expired)?
3. If expired, reject and remove from allowlist.

### Pairing Modes in the Protocol

The server's `/auth/challenge` response includes the pairing mode:

```
POST /auth/challenge → {expires_in: 600, pairing_mode: "permanent"|"temporary", default_expires_at: null|"+24h"}
```

The client UI reflects the mode:
- **Permanent:** "This pairing will not expire. Use for installed systems."
- **Temporary:** "This pairing expires in 24 hours. Use for guests/events."

The operator can also configure the default expiry per server or per group:
```ini
[server]
# Default expiry for temporary pairings. Null = permanent only.
temporary_pairing_expiry = "24h"
```

### Security Properties for PA Systems

| Property | How |
|---|---|
| Physical pairing mode | Speaker requires button press / QR scan to enter pairing mode. Attacker needs physical access. |
| Per-speaker identity | Each speaker has a stable Device ID. Client validates server identity. |
| Client identity | Each client has a stable identity. Server stores who paired, not just what token. |
| Temporary token expiry | DJ's access auto-expires after 24 hours. No manual cleanup needed. |
| Token propagation | Temporary token is shared across all speakers in the trust group. One pairing = access to all. |
| Rate limiting | Prevent brute-force on challenge/redeem. |

### DJ Scenario Flow

**Note:** In practice, a DJ at a club typically uses a physical cable (AES/EBU, analog XLR, or USB) into the mixer, not a streaming network. The network scenario is more relevant for:
- Wireless distribution to stage monitors
- Remote zone control (different rooms, different levels)
- Backup/fallback audio path

For the cable scenario, the DJ's laptop is directly connected — no network auth needed. The l337 audio server on the DJ's laptop would be in dummy mode or outputting to the cable interface.

For the wireless scenario:
1. DJ arrives at club, opens l337-player on laptop
2. DJ scans QR code on the PA controller (or enters URL manually): `https://192.168.1.100:1337`
3. Client runs challenge/redeem with website verification (DJ is logged into l337 website)
4. Server issues temporary token with `expires_at: +24h`, `group: "main-pa"`
5. Server propagates token to all 15 speakers
6. DJ controls all speakers for 24 hours
7. After 24 hours, all speakers automatically revoke the token
8. DJ leaves, no manual cleanup needed

### Installed PA Scenario Flow

1. Installer sets up 15 speakers, configures trust group `main-pa`
2. Installer pairs once with each speaker (permanent pairing, no expiry)
3. Installer's tokens are stored on each speaker with `expires_at: null`
4. Installer's client can control any speaker at any time
5. If the DJ later pairs temporarily, the DJ gets a separate token with `expires_at: +24h`
6. Both tokens coexist in the allowlist. Installer's permanent token remains valid.

---

## Silent Disco / Headphone System Scenario

### The Scenario

A silent disco event:
- One client (DJ/controller)
- 100-500 pairs of headphones (each pair is its own l337 server)
- WiFi network (local, no internet)
- Each headphone has voting buttons (interactive, not just broadcast)
- Before the concert, every set of headphones is pre-paired to the client

This is the extreme case of the PA system problem:
- **Scale:** hundreds of devices
- **Pre-pairing:** all pairing happens before the event, not during
- **No internet:** venue WiFi is local only
- **Interactive:** headphones send voting data back, so they're not just receivers
- **Bluetooth won't work:** too many devices, range, interference

### Why This Is Different

The DJ/PA scenario assumes pairing happens during or before the event, with one or two clients. The silent disco scenario is:
- **Bulk provisioning:** 100-500 devices need to be paired at once
- **Pre-event setup:** pairing happens in a staging area, not during the concert
- **No real-time pairing during the event:** everything must work before doors open
- **Device identity matters:** the client needs to address individual headphones (for channel selection, voting, etc.)

### Design Requirements for Silent Disco

1. **Bulk provisioning flow:** client generates tokens for all headphones in one operation
2. **Pre-shared token loading:** tokens are loaded onto headphones before the event (not via real-time pairing)
3. **Per-headphone identity:** each headphone has a stable Device ID so the client can address it individually
4. **Group membership:** all headphones belong to the same event/session
5. **No website dependency:** venue has no internet
6. **Scalable auth:** the client authenticates to 500 headphones without 500 individual pairing operations

### Options for Silent Disco Provisioning

#### Option A: Pre-generated token list (simplest)

1. Client generates N tokens ahead of time (one per headphone)
2. Tokens are printed as QR codes or stored on NFC tags
3. During staging, each headphone is scanned and its token is loaded
4. Headphones accept any token in their pre-loaded list
5. No real-time pairing during the event

**Pros:**
- Simple, no network needed during provisioning
- QR/NFC is familiar UX
- Tokens can be loaded in parallel (multiple staff members)

**Cons:**
- Physical QR/NFC tags are extra hardware
- If a headphone is lost, its token must be manually revoked

#### Option B: Bulk challenge/redeem provisioning

1. Client connects to each headphone individually (one-time setup)
2. For each headphone: client calls `POST /auth/challenge`, reads challenge file, calls `POST /auth/redeem`
3. This is "pairing" but it's done in bulk before the event
4. The client can parallelize: connect to 10 headphones at once
5. After provisioning, headphones accept the paired tokens

**Pros:**
- Uses the standard challenge/redeem flow
- No extra hardware (QR/NFC) needed
- Tokens are generated by the server, not the client

**Cons:**
- Requires the client to be in range of each headphone during setup
- 500 individual pairing operations take time (but can be parallelized)

#### Option C: Shared secret provisioning (most practical for silent disco)

1. All headphones are manufactured with the same shared secret (pre-provisioned at factory)
2. The client and headphones use the shared secret to derive per-headphone tokens
3. No real-time pairing needed — the shared secret is the trust anchor
4. The client can address individual headphones by deriving their unique token from the shared secret + headphone ID

**How it works:**
```
Shared secret: "silent-disco-event-2026"
Headphone Device ID: "headphone-042"

Derived token: HMAC-SHA256(shared_secret, headphone_device_id)
```

The client can derive the token for any headphone on the fly. The headphone can verify the token independently (it knows its own Device ID and the shared secret).

**Pros:**
- No pairing needed at all — tokens are derived, not exchanged
- Client can address any headphone by its Device ID
- Works offline, no network needed
- Scalable to thousands of devices
- If a headphone is lost, its token is useless without the shared secret

**Cons:**
- All headphones share the same secret — if the secret is compromised, all headphones are compromised
- Requires secure provisioning of the shared secret at manufacture time
- Revoking a single headphone requires changing the shared secret (revokes all)

#### Option D: Hierarchical trust (shared secret + per-headphone token)

Combines Option C with per-headphone tokens:
1. All headphones share a common "event secret" (provisioned at manufacture)
2. During staging, the client pairs with each headphone individually (challenge/redeem)
3. Each headphone gets a unique token derived from the event secret + its Device ID
4. The client stores the mapping: `{device_id: derived_token}`
5. If a headphone is lost, the client revokes just that device ID

**Pros:**
- Best of both worlds: scalable addressing + per-device revocation
- No real-time pairing during the event
- Headphones can verify tokens without contacting the client

**Cons:**
- More complex: requires both shared secret and per-device pairing
- Staging time is longer (must pair each headphone)

### Recommended: Option D for Silent Disco

Hierarchical trust:
1. **Event secret** is pre-provisioned on all headphones (factory or staging)
2. **Per-headphone pairing** happens during staging (challenge/redeem, can be parallelized)
3. **Client stores** `{device_id: token}` mapping
4. **During the event:** client derives tokens on the fly or looks them up; headphones verify independently
5. **Revocation:** client can revoke individual headphones by Device ID

This gives you:
- **Scalability:** 500 headphones, one client
- **Pre-event pairing:** everything is set up before doors open
- **No internet dependency:** works on local WiFi
- **Interactive:** each headphone is individually addressable for voting
- **Revocation:** individual headphones can be kicked out

---

## Comparison Matrix

| Option | Multiple clients | Offline devices | Website dependency | Complexity |
|---|---|---|---|---|
| 1: Optional web verify | Allowlist | Works (unverified) | Optional | Low |
| 2: Web verify for remote | Allowlist | Works (unverified) | Required for TCP | Low |
| 3: Website-issued tokens | Allowlist | Needs website | Website issues tokens | Medium |
| 4: Persistent identity | Allowlist | Works | None | High |

All options support the allowlist model (multiple simultaneous clients). The difference is how identity is established and verified.

---

## Scenario Summary

| Scenario | Scale | Connectivity | Pairing mode | Recommended approach |
|---|---|---|---|---|
| Home audio | 1-5 devices | Internet optional | Permanent, per-device | Option 2, permanent pairing |
| Installed PA | 10-50 speakers | Local network | Permanent, per-device | Option 2 + physical pairing mode |
| Club DJ (cable) | 1 client, 1 mixer | None (physical) | N/A | No network auth needed |
| Club DJ (wireless) | 1 client, 10-50 speakers | Local WiFi | Temporary, 24h | Option 2, temporary pairing + trust group |
| Silent disco | 1 client, 100-500 headphones | Local WiFi only | Pre-provisioned, per-device | Hierarchical trust (Option D) |
| Bluetooth headphones | 1 client, 1 device | Bluetooth | Local, no website | Challenge/redeem over BT, no web verify |
| Remote VPS | 1-N clients | Internet | Remote, verified | Option 2, website verification required |

---

## Open Questions

1. **Trust group protocol:** how do speakers propagate temporary tokens? Multicast? HTTP between speakers? Manual config?
2. **Speaker identity verification:** does the client validate each speaker's Device ID? For the DJ scenario, the client pairs once and trusts the controller to propagate the token — but should the client verify each speaker individually?
3. **Physical pairing mode UX:** button press, QR code, NFC, or something else?
4. **Website API design:** what does `/api/verify-token` look like? Does the website need to know which server the client is pairing with?
5. **Silent disco provisioning:** how are the initial shared secrets loaded onto headphones at the factory? Is there a secure provisioning protocol?
6. **Silent disco revocation:** if the event secret is compromised, can we rotate it without re-pairing all 500 headphones?
