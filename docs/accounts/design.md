# Interconnect Design

## Overview

Technical design for instance-to-instance federation. Covers the tunnel
protocol, data models, authentication flow, message dispatch, and the
connection token format.

## 1. Cryptographic Primitives

### 1.1 Key Types (crab_city_auth)

All types live in the `crab_city_auth` crate:

```rust
PublicKey([u8; 32])    // ed25519 public key, Display as base64, fingerprint()
SigningKey             // wraps ed25519_dalek::SigningKey, generate/sign
Signature([u8; 64])   // ed25519 signature
```

- `PublicKey::fingerprint()` → `crab_` + first 8 chars Crockford base32
- `PublicKey::LOOPBACK` → all-zeros sentinel (`[0u8; 32]`)
- `verify(public_key, message, signature) -> Result<()>`

### 1.2 Instance Identity

Each instance generates an ed25519 keypair at first startup:

```rust
InstanceIdentity {
    signing_key: SigningKey,         // stored in config dir
    public_key: PublicKey,           // = iroh NodeId
}
```

The instance identity signs connection tokens and authenticates iroh
connections. The `iroh_secret_key()` method converts to iroh's key format
(same curve, different wrapper).

### 1.3 Identity Proofs

Cross-instance authentication uses signed identity proofs. The user signs
the host's node_id with their signing key:

```rust
// Home side: sign the host's node_id
let proof = signing_key.sign(&host_node_id);

// Host side: verify
verify(&account_key, &my_node_id, &proof)?;
```

This proves the user controls the private key corresponding to
`account_key` without revealing the key.

### 1.4 Access Rights

GNAP-inspired access rights (RFC 9635 Section 8):

```rust
AccessRight { type_: String, actions: Vec<String> }
AccessRights(Vec<AccessRight>)

// Capability presets
Capability::View        → [content:read, terminals:read]
Capability::Collaborate → View + [terminals:input, chat:send, tasks:*, instances:create]
Capability::Admin       → Collaborate + [members:*]
Capability::Owner       → Admin + [instance:manage,transfer]
```

Four algebraic operations on `AccessRights`:
- `intersect` — commutative, idempotent
- `contains(type_, action)` — authorization check
- `is_superset_of` — delegation validation
- `diff` — audit trail

Property-tested with proptest. Kani bounded model checking proves
`intersect` commutativity, preset ordering, and round-trip correctness.

## 2. Tunnel Protocol

### 2.1 Message Types

Defined in `interconnect/protocol.rs`:

```rust
enum TunnelClientMessage {
    Hello { instance_name: String },
    Authenticate {
        account_key: String,        // hex-encoded pubkey
        display_name: String,
        identity_proof: String,     // hex-encoded signature
    },
    UserMessage {
        account_key: String,
        message: ClientMessage,     // existing WS protocol
    },
    UserDisconnected {
        account_key: String,
    },
}

enum TunnelServerMessage {
    Welcome { instance_name: String },
    AuthResult {
        account_key: String,
        capability: Option<String>,
        access: Vec<AccessRight>,
        error: Option<String>,
    },
    UserMessage {
        account_key: String,
        message: ServerMessage,     // existing WS protocol
    },
    InstanceList {
        instances: Vec<...>,
    },
}
```

### 2.2 Wire Format

Length-prefixed JSON over iroh QUIC streams:

```
[4 bytes big-endian length][JSON payload]
```

Read/write helpers in `protocol.rs`:
- `write_tunnel_client_message(send, msg)`
- `write_tunnel_server_message(send, msg)`
- `read_tunnel_client_message(recv) -> Option<TunnelClientMessage>`
- `read_tunnel_server_message(recv) -> Option<TunnelServerMessage>`

### 2.3 Connection Lifecycle

```
Home Instance                          Host Instance
    │                                      │
    │  iroh QUIC connect (ALPN: crab/1)   │
    │ ──────────────────────────────────►  │
    │                                      │  accept, open bidi stream
    │                                      │
    │  Hello { instance_name }            │
    │ ──────────────────────────────────►  │
    │                                      │
    │  Welcome { instance_name }          │
    │ ◄──────────────────────────────────  │
    │                                      │
    │  Authenticate { account_key,        │
    │    display_name, identity_proof }   │
    │ ──────────────────────────────────►  │
    │                                      │  verify proof against node_id
    │                                      │  lookup federated_accounts
    │  AuthResult { capability, access }  │
    │ ◄──────────────────────────────────  │
    │                                      │
    │  UserMessage { account_key, msg }   │  per-message access checks
    │ ◄────────────────────────────────►  │
    │                                      │
    │  UserDisconnected { account_key }   │
    │ ──────────────────────────────────►  │
```

### 2.4 ALPN

All iroh connections use `ALPN = b"crab/1"`. The accept loop distinguishes
tunnel connections from invite connections by inspecting the first message.

## 3. Data Model

### 3.1 Host-Side: Federated Accounts

```sql
CREATE TABLE federated_accounts (
    account_key BLOB NOT NULL PRIMARY KEY,  -- 32 bytes, ed25519 pubkey
    display_name TEXT NOT NULL,
    home_node_id BLOB,                      -- 32 bytes, home instance NodeId
    home_name TEXT,                          -- "Alice's Lab"
    access TEXT NOT NULL DEFAULT '[]',       -- JSON access rights array
    state TEXT NOT NULL DEFAULT 'active',    -- 'active', 'suspended'
    created_by BLOB NOT NULL,               -- pubkey of admin who created this
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_federated_state ON federated_accounts(state);
```

### 3.2 Home-Side: Remote Crab Cities

```sql
CREATE TABLE remote_crab_cities (
    host_node_id BLOB NOT NULL,              -- 32 bytes, host instance NodeId
    account_key BLOB NOT NULL,               -- 32 bytes, local user
    host_name TEXT NOT NULL,                  -- "Bob's Workshop"
    granted_access TEXT NOT NULL DEFAULT '[]', -- what the host granted
    auto_connect INTEGER NOT NULL DEFAULT 1,  -- connect on startup?
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (host_node_id, account_key)
);

CREATE INDEX idx_remote_auto ON remote_crab_cities(auto_connect);
```

### 3.3 Repository Functions

**Federation** (`repository/federation.rs`):
```rust
create_federated_account(db, account_key, display_name, home_node_id, home_name, access, created_by)
get_federated_account(db, account_key) -> Option<FederatedAccount>
list_federated_accounts(db) -> Vec<FederatedAccount>
update_federated_access(db, account_key, access)
update_federated_state(db, account_key, state)
delete_federated_account(db, account_key)

add_remote_crab_city(db, host_node_id, account_key, host_name, granted_access)
list_remote_crab_cities(db, account_key) -> Vec<RemoteCrabCity>
list_auto_connect(db) -> Vec<RemoteCrabCity>
remove_remote_crab_city(db, host_node_id, account_key)
update_remote_access(db, host_node_id, account_key, granted_access)
```

**Membership** (`repository/membership.rs`):
```rust
create_identity(db, identity) -> MemberIdentity
get_identity(db, public_key) -> Option<MemberIdentity>
create_grant(db, grant) -> MemberGrant
get_grant(db, public_key) -> Option<MemberGrant>
get_active_grant(db, public_key) -> Option<MemberGrant>
list_members(db) -> Vec<(MemberIdentity, MemberGrant)>
update_grant_state(db, public_key, new_state)
```

**Invites** (`repository/invites.rs`):
```rust
create_invite(db, invite)
get_invite(db, nonce) -> Option<StoredInvite>
increment_invite_use_count(db, nonce)
revoke_invite(db, nonce)
list_active_invites(db) -> Vec<StoredInvite>
```

**Event Log** (`repository/event_log.rs`):
```rust
log_event(db, event) -> i64          // hash-chained append
query_events(db, filter) -> Vec<Event>
verify_chain(db, from, to) -> ChainVerification
get_chain_head(db) -> (i64, [u8; 32])
create_checkpoint(db, event_id, signing_key) -> EventCheckpoint
```

### 3.4 Event Types

Federation lifecycle events:
- `federation.granted` — new federated account created
- `federation.access_changed` — access rights updated
- `federation.suspended` — account suspended
- `federation.removed` — account removed
- `federation.authenticated` — remote user authenticated (informational)
- `federation.disconnected` — remote user disconnected (informational)

## 4. Host Handler

File: `interconnect/host.rs`

The host handler accepts tunnel connections from remote instances and
dispatches per-user messages.

### 4.1 Tunnel Context

```rust
struct TunnelContext {
    send: SendStream,
    repo: ConversationRepository,
    state_manager: Arc<GlobalStateManager>,
    instance_manager: Arc<InstanceManager>,
    instance_name: String,
    node_id: [u8; 32],
    authenticated_users: HashMap<String, AuthenticatedUser>,
}

struct AuthenticatedUser {
    account_key: [u8; 32],
    display_name: String,
    access: Vec<AccessRight>,
}
```

### 4.2 Message Dispatch

The host handler's message loop:

1. Read `TunnelClientMessage` from the stream
2. Match on message type:
   - `Hello` → respond with `Welcome`
   - `Authenticate` → verify proof, lookup federated account, respond with
     `AuthResult`
   - `UserMessage` → check user is authenticated, check access rights,
     dispatch through the existing `dispatch_client_message` (same code
     path as local WebSocket clients)
   - `UserDisconnected` → clean up user state

### 4.3 Authentication Flow

```rust
fn handle_authenticate(ctx, account_key, display_name, identity_proof):
    // 1. Decode hex account_key to [u8; 32]
    // 2. Decode hex identity_proof to [u8; 64]
    // 3. Verify: verify(&account_key, &ctx.node_id, &identity_proof)
    // 4. Lookup: repo.get_federated_account(&account_key)
    // 5. Check state == active
    // 6. Send AuthResult with granted access
```

### 4.4 RPC Context

For dispatching user messages, the host constructs an `RpcContext` that
mirrors the local WebSocket handler's context:

```rust
struct RpcContext {
    repo: ConversationRepository,
    state_manager: Arc<GlobalStateManager>,
    instance_manager: Arc<InstanceManager>,
    account_key: [u8; 32],
    access: Vec<AccessRight>,
}
```

This allows federated users to go through the same `dispatch_client_message`
code path as local users, with the same access checks.

## 5. Connection Manager

File: `interconnect/manager.rs`

The connection manager maintains outbound iroh tunnels to remote hosts on
behalf of local users.

### 5.1 Structure

```rust
struct ConnectionManager {
    tunnels: HashMap<[u8; 32], InstanceTunnel>,
    repo: ConversationRepository,
    identity: Arc<InstanceIdentity>,
    endpoint: Endpoint,
}

struct InstanceTunnel {
    host_node_id: [u8; 32],
    host_name: String,
    send: SendStream,
    recv: RecvStream,
    authenticated_users: HashSet<String>,
}
```

### 5.2 Operations

- **connect** — establish iroh tunnel to host, exchange Hello/Welcome
- **authenticate_user** — send Authenticate for a specific user, cache result
- **forward_message** — wrap ClientMessage in UserMessage, send over tunnel
- **disconnect** — send UserDisconnected, close tunnel

### 5.3 Message Routing

When a local user is viewing a remote Crab City:
1. Local WebSocket handler receives `ClientMessage`
2. Check `CrabCityContext` — if `Remote`, route to `ConnectionManager`
3. Manager wraps in `TunnelClientMessage::UserMessage { account_key, message }`
4. Manager sends over the appropriate tunnel
5. Response arrives as `TunnelServerMessage::UserMessage`
6. Manager forwards to the local user's WebSocket

## 6. Connection Token v2

File: `transport/connection_token.rs`

### 6.1 Format

```
[1B version=2]
[32B node_id]              — host instance ed25519 pubkey
[16B invite_nonce]         — nonce to redeem
[1B name_len]              — length of instance name
[name_len B instance_name] — UTF-8
[8B inviter_fingerprint]   — first 8 bytes of inviter pubkey hash
[1B capability]            — 0=view, 1=collaborate, 2=admin
[64B signature]            — ed25519 over all preceding bytes
[remaining: relay_url]     — optional
```

Typical size: 143 bytes (20-char name, no relay) → ~229 base32 chars.
QR-code compatible.

### 6.2 Backward Compatibility

`from_base32` checks the version byte. v1 tokens (no metadata) still parse.
v2 tokens are only generated by instances that support them.

## 7. CLI Commands

### 7.1 `crab invite`

File: `cli/invite.rs`

```
crab invite [--for <label>] [--access <capability>] [--expires <duration>]
```

Creates a connection invite token. Displays the token, QR code, and copies
to clipboard. The token carries v2 metadata (instance name, inviter
fingerprint, capability).

### 7.2 `crab connect`

File: `cli/connect.rs`

```
crab connect <token> [--yes]
```

Two-phase flow:
1. **Parse phase** — decode token, display metadata (instance name, inviter,
   access level), ask for confirmation
2. **Connect phase** — establish iroh connection, exchange Hello/Welcome,
   authenticate, display available terminals

The connect command also handles saving the remote to `remote_crab_cities`
for future auto-connect.

## 8. WebSocket Dispatch Integration

File: `ws/dispatch.rs`

The unified message dispatch handles both local and remote contexts:

```rust
async fn dispatch_client_message(msg: ClientMessage, context: &DispatchContext) {
    match context {
        DispatchContext::Local { ... } => {
            // existing local handling
        }
        DispatchContext::Remote { host_node_id, account_key, ... } => {
            // forward via ConnectionManager
        }
    }
}
```

Local WebSocket clients that are viewing a remote Crab City have their
messages forwarded through the tunnel. The client-side protocol is unchanged
— the dispatch layer handles the routing transparently.

## 9. Reconnection

If the iroh connection to a remote host drops:

1. Remote instances show as "(disconnected)" in the switcher
2. Home instance attempts reconnection with exponential backoff
3. On reconnect: re-authenticate all users, replay from ring buffer
4. The user sees a brief interruption, then normal service resumes

The replay buffer (`transport/replay_buffer.rs`) handles message replay on
reconnection.

## 10. Security Boundaries

| Boundary | Trust model |
|----------|-------------|
| Local user ↔ Instance | Loopback bypass (owner) or session auth |
| Instance ↔ Instance | iroh QUIC handshake (ed25519 + QUIC encryption) |
| Remote user ↔ Host | Identity proof (ed25519 signature of host node_id) |
| Federated access | Per-user access rights checked on every message |

### Key Properties

- **No shared secrets.** Authentication is asymmetric (ed25519 signatures).
- **No token management.** The iroh connection IS the session.
- **Immediate revocation.** Suspending a federated account takes effect on
  the next message — no token expiry window.
- **Independent grants.** Each direction is a separate federated account
  with its own access rights.

## 11. Invite Token Cryptography

### 11.1 Invite Chain Format

```rust
Invite {
    version: u8,           // 0x01
    instance: PublicKey,   // NodeId
    links: Vec<InviteLink>,
}

InviteLink {
    issuer: PublicKey,
    capability: Capability,
    max_depth: u8,         // 0 = leaf, no delegation
    max_uses: u32,
    expires_at: Option<u64>,
    nonce: [u8; 16],
    signature: Signature,  // signs H(prev_link) ++ instance ++ fields
}
```

Flat invite: chain of length 1, `max_depth = 0`, 160 bytes.
Delegated invite (3-hop): 412 bytes.

Verification walks root-to-leaf: check signatures, capability narrowing
(each link ≤ previous), depth constraints, expiry.

### 11.2 Kani Verification

The invite parser has Kani bounded model checking proof harnesses:
- `link_from_bytes_no_panic` — proves `from_bytes` never panics
- `invite_from_bytes_flat_no_panic` — flat invite parsing safety
- `chain_depth_bound_enforced` — depth limits enforced

## 12. Hash-Chained Event Log

### 12.1 Structure

```rust
Event {
    id: u64,
    prev_hash: [u8; 32],      // H(previous event)
    event_type: EventType,
    actor: Option<PublicKey>,
    target: Option<PublicKey>,
    payload: serde_json::Value,
    created_at: String,
    hash: [u8; 32],            // H(id ++ prev_hash ++ ... ++ created_at)
}
```

Genesis event uses `H(instance_node_id)` as `prev_hash`.

### 12.2 Tamper Evidence

- Modifying any event breaks the hash chain
- Signed checkpoints (every N events) provide tamper-proof anchors
- `verify_chain(from, to)` performs sequential integrity scan

### 12.3 Property-Tested Invariants

- Build chain of N events → verify succeeds
- Tamper any event → verify detects break
- Delete event from middle → verify detects break
- Checkpoint signature round-trips correctly

## 13. Error Recovery

Every error includes a machine-actionable recovery:

| Error | Recovery |
|-------|----------|
| `no federated account` | `redeem_invite` |
| `verification failed` | `none` (bad proof) |
| `suspended` | `contact_admin` |
| `insufficient_access` | `none` (need higher capability) |

Error types are defined in `crab_city_auth::AuthError` with a `recovery()`
method that maps to `RecoveryAction`.
