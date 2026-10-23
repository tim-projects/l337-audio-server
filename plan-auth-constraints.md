# L337 System — Hard Constraints

## 1. One Node Per Physical Device

Each physical device runs exactly one l337 node. There is no virtualization,
no container-per-service, no multi-instance on one device.

```
Physical Device
└── Node (exactly one)
    ├── DeviceID (stable, unique)
    ├── capabilities: Set[Capability]
    ├── transport: Transport
    ├── auth: AuthLayer
    └── [objects based on capabilities]
```

## 2. One Auth Code Per Node

Each node has exactly one stable credential. This is the node's identity.

- The auth code is derived from the node's DeviceID (keypair public key hash, or UUID).
- It does not change between pairings.
- It does not change when the node pairs with different peers.
- It is the same credential the node presents to every peer.

Pairing means: the remote node adds this DeviceID to its allowlist.
The node does not get a different token per pairing.

## 3. /capabilities Endpoint

Every node exposes `GET /capabilities` (public, no auth required).

Response:
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

## 4. Implications

### 4.1 Auth Flow

```
Node A (controller)                     Node B (speaker)
  |                                       |
  |  GET /capabilities (no auth)          |
  |  ← {node_id, capabilities, ...}       |
  |                                       |
  |  POST /auth/challenge (no auth)       |
  |  ← {expires_in: 600, device_id: "B"}  |
  |                                       |  → writes challenge-token.txt
  |                                       |
  |  [out-of-band: read challenge]         |
  |                                       |
  |  POST /auth/redeem                     |
  |  X-L337-Challenge: <nonce>            |
  |  X-L337-Node-ID: "A"                  |
  |  ← {ok: true}                         |
  |                                       |  → verifies challenge
  |                                       |  → adds A's DeviceID to allowlist
  |                                       |
  |  Future requests:                      |
  |  Authorization: Bearer <node_A_token>  |
  |  ← 200                                 |
```

The `X-L337-Node-ID` header identifies the pairing node. The server stores
the DeviceID in its allowlist. The bearer token can be:
- A stable token derived from the node's keypair (recommended)
- The challenge value itself (simpler, but changes per pairing)

### 4.2 Allowlist Stores DeviceIDs

```json
[
  {
    "node_id": "MFZWI3D-BONSGYC-YLTMRWG-C43ENR5...",
    "name": "DJ Laptop",
    "capabilities": ["control", "search", "display"],
    "verified": true,
    "account_id": "user123",
    "paired_at": "2026-01-01T00:00:00Z",
    "expires_at": null,
    "last_seen": "2026-01-02T00:00:00Z"
  }
]
```

The allowlist is a list of trusted DeviceIDs. Token validation is: "does this
request come from a DeviceID in my allowlist?"

### 4.3 Trust Group Propagation

When a node pairs with a trust group member, the new node's DeviceID is
propagated to all nodes in the group. One pairing = access to all.

```
DJ Laptop (DeviceID: A)                PA Controller (DeviceID: B)              Speakers C, D, E...
  |                                       |                                       |
  | POST /auth/challenge                   |                                       |
  | ← {expires_in: 600}                   |                                       |
  |                                       |                                       |
  | POST /auth/redeem                      |                                       |
  | X-L337-Challenge: <nonce>             |                                       |
  | ← {ok: true, group: "main-pa"}        |                                       |
  |                                       | → propagate A to C, D, E...           |
  |                                       | ← {ok} x N                            |
```

All speakers now accept requests from DeviceID A.

### 4.4 Per-Device Identity

Each physical device has exactly one DeviceID. This means:
- A DJ laptop has one identity, used to control all speakers
- A speaker has one identity, used to authenticate controllers
- A silent disco headphone has one identity, used to authenticate the controller
- No per-user tokens, no per-app tokens, no per-pairing tokens

### 4.5 Capability Discovery

Before pairing, nodes can query each other's capabilities:

```
Node A → Node B: GET /capabilities
Node B → Node A: {node_id, capabilities, version, transport, trust_group}
```

This allows:
- Controllers to discover which speakers are available
- DJ laptop to find all speakers in a trust group
- Voting terminals to find the output node
- Clients to validate server identity before pairing
