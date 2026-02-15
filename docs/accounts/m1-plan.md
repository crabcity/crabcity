# Milestone 1: Instance-Local Auth — Implementation Plan

Replace the session/cookie auth system with iroh-based identity. After M1,
every client — native and browser — connects via iroh. The iroh handshake IS
authentication. No session tokens, no passwords, no cookies.

Nothing is in production. No migration path. Old auth gets deleted.

## Prerequisites

- [x] M0 crate (`crab_city_auth`) complete and passing `cargo test`
- [ ] M0 bazel verification (`bazel build //packages/crab_city_auth`)

---

## Phase 0: Spike — iroh Embedded Relay + WASM (Do This First)

**Goal:** Validate the highest-risk technical bet before writing any production
code. A throwaway proof-of-concept that answers three questions:

1. Can a Crab City instance embed an iroh relay server in-process?
2. Can a browser connect to it via iroh WASM over WebSocket?
3. What's the WASM bundle size and cold-start time?

### 0.1 Spike Binary

Create: `packages/spike_iroh_relay/` (temporary, will be deleted after spike)

```rust
// A minimal axum server that:
// 1. Starts an in-process iroh relay on a WebSocket endpoint (/relay)
// 2. Starts an iroh node that accepts incoming connections
// 3. On connection: extracts NodeId, prints fingerprint, echoes messages
// 4. Serves a static HTML page with iroh WASM that connects through the relay
```

- [ ] Add `iroh` and `iroh-relay` to workspace deps (Cargo.toml + MODULE.bazel)
- [ ] Stand up embedded relay (`iroh_relay::server::Server`) bound to
      `ws://localhost:<port>/relay`
- [ ] Create iroh endpoint that accepts connections, extracts `NodeId`
- [ ] Verify: native client (`iroh::Endpoint`) connects via QUIC, NodeId
      extracted correctly
- [ ] Build iroh WASM client (assess `iroh` crate's WASM target support)
- [ ] Serve a static HTML page that loads iroh WASM, generates a keypair,
      connects to the relay, sends a ping
- [ ] Measure: compressed WASM bundle size (target: <500KB)
- [ ] Measure: cold start to first connection (target: <2s)
- [ ] Measure: memory footprint in browser (target: <50MB)
- [ ] Verify: E2E encryption works (relay cannot read messages)

**If the spike fails** (iroh WASM too large, relay won't embed, connection
unreliable): fall back to a hybrid model where browsers use WebSocket with a
challenge-response handshake and native clients use iroh. This changes M1
significantly — stop and reassess before proceeding.

**If the spike succeeds:** delete the spike crate, carry the dependency versions
and configuration into the real implementation.

### 0.2 Version Pinning

Record the exact iroh version that works. Pin it in `Cargo.toml` and
`MODULE.bazel`. iroh's API surface is still moving; do not float versions.

```toml
# workspace Cargo.toml [workspace.dependencies]
iroh = "=0.XX.Y"
iroh-relay = { version = "=0.XX.Y", features = ["server"] }
```

### 0.3 Spike Exit Criteria

- [ ] Native client connects via QUIC, NodeId matches generated keypair
- [ ] Browser client connects via WASM relay, NodeId matches generated keypair
- [ ] Bidirectional message exchange works on both paths
- [ ] Bundle size and latency within targets
- [ ] Spike crate deleted, learnings recorded

**Estimated effort:** ~200 LOC throwaway code. This gates everything else.

---

## Phase 1: Database + Repository Layer

No iroh transport yet. Just the tables and CRUD that everything else needs.

### 1.1 Migration

New file: `packages/crab_city/migrations/NNN_interconnect_auth.sql`

```sql
-- WHO you are (identity, cached from registry or self-reported)
CREATE TABLE member_identities (
    public_key BLOB NOT NULL PRIMARY KEY,     -- 32 bytes, ed25519
    display_name TEXT NOT NULL DEFAULT '',
    handle TEXT,                               -- @alex, from registry
    avatar_url TEXT,
    registry_account_id TEXT,
    resolved_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- WHAT you can do (authorization, instance-local)
CREATE TABLE member_grants (
    public_key BLOB NOT NULL PRIMARY KEY,     -- 32 bytes, ed25519
    capability TEXT NOT NULL,                  -- 'view', 'collaborate', 'admin', 'owner'
    access TEXT NOT NULL DEFAULT '[]',         -- JSON array of GNAP-style access rights
    state TEXT NOT NULL DEFAULT 'invited',     -- 'invited', 'active', 'suspended', 'removed'
    org_id TEXT,
    invited_by BLOB,                           -- 32 bytes, pubkey of inviter
    invited_via BLOB,                          -- 16 bytes, invite nonce
    replaces BLOB,                             -- 32 bytes, pubkey of old grant
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (public_key) REFERENCES member_identities(public_key)
);

-- Invite tokens created by this instance
CREATE TABLE invites (
    nonce BLOB NOT NULL PRIMARY KEY,          -- 16 bytes
    issuer BLOB NOT NULL,                     -- 32 bytes, pubkey
    capability TEXT NOT NULL,
    max_uses INTEGER NOT NULL DEFAULT 0,
    use_count INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT,
    chain_blob BLOB NOT NULL,                 -- full serialized invite (for redistribution)
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_at TEXT,
    FOREIGN KEY (issuer) REFERENCES member_identities(public_key)
);

-- Instance-local blocklist
CREATE TABLE blocklist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_type TEXT NOT NULL,                 -- 'pubkey', 'node_id', 'ip_range'
    target_value BLOB NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    added_by BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Cached registry blocklist
CREATE TABLE blocklist_cache (
    scope TEXT NOT NULL,
    version INTEGER NOT NULL,
    target_type TEXT NOT NULL,
    target_value BLOB NOT NULL,
    PRIMARY KEY (scope, target_type, target_value)
);

-- Append-only, hash-chained audit trail
CREATE TABLE event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    prev_hash BLOB NOT NULL,
    event_type TEXT NOT NULL,
    actor BLOB,
    target BLOB,
    payload TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    hash BLOB NOT NULL
);

-- Signed checkpoints for tamper evidence
CREATE TABLE event_checkpoints (
    event_id INTEGER NOT NULL PRIMARY KEY,
    chain_head_hash BLOB NOT NULL,
    signature BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (event_id) REFERENCES event_log(id)
);

-- Seed loopback identity
INSERT INTO member_identities (public_key, display_name)
    VALUES (X'0000000000000000000000000000000000000000000000000000000000000000', 'Local');

INSERT INTO member_grants (public_key, capability, access, state)
    VALUES (
        X'0000000000000000000000000000000000000000000000000000000000000000',
        'owner',
        '[{"type":"content","actions":["read"]},{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]},{"type":"tasks","actions":["read","create","edit"]},{"type":"instances","actions":["create"]},{"type":"members","actions":["read","invite","suspend","reinstate","remove","update"]},{"type":"instance","actions":["manage","transfer"]}]',
        'active'
    );

-- Indexes
CREATE INDEX idx_invites_expires ON invites(expires_at);
CREATE INDEX idx_grants_state ON member_grants(state);
CREATE INDEX idx_grants_invited_via ON member_grants(invited_via);
CREATE INDEX idx_event_log_type ON event_log(event_type);
CREATE INDEX idx_event_log_target ON event_log(target);
CREATE INDEX idx_event_log_created ON event_log(created_at);
CREATE INDEX idx_event_log_hash ON event_log(hash);

-- Drop old auth tables (nothing in production, clean break)
DROP TABLE IF EXISTS sessions;
DROP TABLE IF EXISTS instance_permissions;
DROP TABLE IF EXISTS instance_invitations;
DROP TABLE IF EXISTS server_invites;
-- Keep 'users' table temporarily — username/display_name data may be useful
-- for seeding member_identities during development. Drop it in a later migration.
```

- [ ] Write migration file
- [ ] Run `cargo sqlx prepare` to update offline query data
- [ ] Verify: `cargo test -p crab_city` — migration applies cleanly

### 1.2 Repository: Membership

New file: `packages/crab_city/src/repository/membership.rs`

```rust
// All functions take &SqlitePool. No iroh types — just crab_city_auth types.

pub async fn create_identity(db, public_key, display_name) -> Result<()>
pub async fn get_identity(db, public_key) -> Result<Option<MemberIdentity>>
pub async fn update_identity(db, public_key, display_name, handle, avatar_url) -> Result<()>

pub async fn create_grant(db, public_key, capability, access, state, invited_by, invited_via) -> Result<()>
pub async fn get_grant(db, public_key) -> Result<Option<MemberGrant>>
pub async fn get_active_grant(db, public_key) -> Result<Option<MemberGrant>>
pub async fn list_members(db) -> Result<Vec<Member>>  // joined identity + grant
pub async fn update_grant_state(db, public_key, new_state) -> Result<()>
pub async fn update_grant_capability(db, public_key, capability, access) -> Result<()>
pub async fn update_grant_access(db, public_key, access) -> Result<()>
pub async fn replace_grant(db, new_pubkey, old_pubkey) -> Result<()>
pub async fn list_grants_by_invite(db, invite_nonce) -> Result<Vec<MemberGrant>>
```

Struct types (defined here, not in `crab_city_auth` — these are DB row types
with sqlx derives):

```rust
pub struct MemberIdentity {
    pub public_key: Vec<u8>,        // 32 bytes
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub registry_account_id: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct MemberGrant {
    pub public_key: Vec<u8>,
    pub capability: String,
    pub access: String,             // JSON
    pub state: String,
    pub org_id: Option<String>,
    pub invited_by: Option<Vec<u8>>,
    pub invited_via: Option<Vec<u8>>,
    pub replaces: Option<Vec<u8>>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct Member {
    pub identity: MemberIdentity,
    pub grant: MemberGrant,
}
```

- [ ] Implement all functions
- [ ] Unit tests: CRUD round-trips, state transitions, replace flow
- [ ] Verify: `cargo test -p crab_city`

### 1.3 Repository: Invites

New file: `packages/crab_city/src/repository/invites.rs`

```rust
pub async fn store_invite(db, nonce, issuer, capability, max_uses, expires_at, chain_blob) -> Result<()>
pub async fn get_invite(db, nonce) -> Result<Option<StoredInvite>>
pub async fn increment_use_count(db, nonce) -> Result<()>
pub async fn revoke_invite(db, nonce) -> Result<()>
pub async fn list_active_invites(db) -> Result<Vec<StoredInvite>>
```

- [ ] Implement all functions
- [ ] Unit tests
- [ ] Verify: `cargo test -p crab_city`

### 1.4 Repository: Event Log

New file: `packages/crab_city/src/repository/event_log.rs`

```rust
pub async fn append_event(db, event_type, actor, target, payload, instance_key) -> Result<i64>
    // Inside a transaction:
    // 1. Read prev event hash (or genesis hash if first)
    // 2. Compute new hash
    // 3. INSERT
    // Returns event id

pub async fn query_events(db, target, event_type_prefix, limit, before_id) -> Result<Vec<Event>>
pub async fn verify_chain(db, from_id, to_id) -> Result<ChainVerification>
pub async fn get_chain_head(db) -> Result<Option<(i64, Vec<u8>)>>
pub async fn create_checkpoint(db, event_id, chain_head_hash, signing_key) -> Result<()>
pub async fn get_event_proof(db, event_id) -> Result<EventProof>
```

- [ ] Implement all functions with hash chaining
- [ ] Test: append 100 events, verify chain, tamper one, verify detects break
- [ ] Test: checkpoint signature round-trip
- [ ] Verify: `cargo test -p crab_city`

### Phase 1 Checklist

- [ ] Migration applies cleanly
- [ ] All three repository modules pass tests
- [ ] `cargo check -p crab_city` passes (existing code still compiles even
      though old auth tables are gone — we'll rip out old auth code in Phase 3)

---

## Phase 2: iroh Transport Layer

The core of M1. Stand up iroh as the transport, accept connections, extract
identity from handshake.

### 2.1 Dependencies

Add to workspace `Cargo.toml`:
```toml
[workspace.dependencies]
iroh = "=X.Y.Z"           # pin exact version from spike
iroh-relay = { version = "=X.Y.Z", features = ["server"] }
```

Add to `packages/crab_city/Cargo.toml`:
```toml
[dependencies]
iroh = { workspace = true }
iroh-relay = { workspace = true }
crab_city_auth = { path = "../crab_city_auth" }
```

Add to `MODULE.bazel`:
```python
crate_index.spec(name = "iroh", version = "X.Y.Z")
crate_index.spec(name = "iroh-relay", version = "X.Y.Z", features = ["server"])
```

- [ ] Add deps
- [ ] `cargo check -p crab_city` passes
- [ ] `crab_city_auth` added as a path dependency of `crab_city`

### 2.2 Instance Identity

New file: `packages/crab_city/src/identity.rs`

Each instance has a persistent ed25519 keypair (= iroh NodeId). Generated on
first startup, stored on disk.

```rust
use crab_city_auth::{SigningKey, PublicKey};

pub struct InstanceIdentity {
    signing_key: SigningKey,
    pub public_key: PublicKey,
}

impl InstanceIdentity {
    /// Load from disk or generate + save.
    /// Path: <data_dir>/identity.key (mode 0600)
    pub fn load_or_generate(data_dir: &Path) -> Result<Self>;

    /// The iroh NodeId for this instance.
    pub fn node_id(&self) -> iroh::NodeId;

    /// Sign arbitrary bytes (used for event checkpoints).
    pub fn sign(&self, message: &[u8]) -> Signature;
}
```

- [ ] Implement load/save (32-byte raw key file, mode 0600)
- [ ] Wire into server startup: load identity before starting iroh endpoint
- [ ] Log fingerprint on startup: `"Instance identity: crab_XXXXXXXX"`
- [ ] Unit test: generate → save → load → same pubkey

### 2.3 Embedded Relay

New file: `packages/crab_city/src/transport/relay.rs`

Start an in-process iroh relay server. This is how browser clients connect.

```rust
pub struct EmbeddedRelay {
    // ...relay server handle...
}

impl EmbeddedRelay {
    /// Start the relay, binding to the given address.
    /// Returns the relay URL for browser clients.
    pub async fn start(bind_addr: SocketAddr) -> Result<(Self, Url)>;

    /// Graceful shutdown.
    pub async fn shutdown(self);
}
```

The relay is wired into the axum server as a route (likely `/relay`), or runs
on a separate port. Configuration determines which:

```toml
[transport]
relay_bind = "127.0.0.1:4434"    # separate port (simpler)
# OR
# relay_path = "/relay"          # same port, path-based (requires TLS sharing)
```

Start with separate port. Same-port can be added later if needed.

- [ ] Implement relay startup with `iroh_relay::server`
- [ ] Wire into server startup (start alongside axum)
- [ ] Verify: can connect to relay from native iroh client
- [ ] Shutdown on server stop

### 2.4 iroh Endpoint + Connection Acceptor

New file: `packages/crab_city/src/transport/iroh_transport.rs`

The iroh endpoint accepts connections and routes them to the auth layer.

```rust
pub struct IrohTransport {
    endpoint: iroh::Endpoint,
    relay_url: Url,
    connections: Arc<DashMap<PublicKey, ConnectionHandle>>,
}

struct ConnectionHandle {
    conn: iroh::endpoint::Connection,
    grant: MemberGrant,
    cancel: CancellationToken,
}

impl IrohTransport {
    pub async fn start(
        identity: &InstanceIdentity,
        relay_url: Url,
        db: SqlitePool,
        broadcast_tx: broadcast::Sender<ServerMessage>,
    ) -> Result<Self>;

    /// Close a specific client's connection (for suspension/removal).
    pub async fn disconnect(&self, public_key: &PublicKey, reason: &str);

    /// Broadcast a message to all connected clients.
    pub async fn broadcast(&self, msg: &ServerMessage);

    /// Number of connected clients.
    pub fn connection_count(&self) -> usize;
}
```

The accept loop (spawned as a tokio task):

```
loop {
    conn = endpoint.accept().await
    node_id = conn.remote_node_id()      // ed25519 pubkey from handshake
    pubkey = PublicKey::from(node_id)

    if pubkey.is_loopback() && !is_loopback_addr(conn) {
        send error, close
        continue
    }

    grant = get_active_grant(db, pubkey).await

    match grant {
        Some(grant) if grant.state == "active" => {
            // Authenticated. Open bidirectional stream.
            spawn connection_handler(conn, pubkey, grant, ...)
        }
        Some(grant) => {
            // Grant exists but not active (suspended, removed, etc.)
            send Error { code: "grant_not_active", recovery: contact_admin }
            close
        }
        None => {
            // No grant. Accept the connection tentatively —
            // the first message must be RedeemInvite.
            spawn invite_handler(conn, pubkey, ...)
        }
    }
}
```

- [ ] Implement accept loop
- [ ] Implement connection handler (bidirectional stream, message routing)
- [ ] Implement invite handler (accept RedeemInvite as first message)
- [ ] Implement `disconnect()` (close connection, remove from map, broadcast
      presence update)
- [ ] Implement `broadcast()` (fan-out to all connections)
- [ ] Wire into server startup

### 2.5 Message Framing

New file: `packages/crab_city/src/transport/framing.rs`

Length-prefixed JSON over iroh QUIC streams. Same format for native and browser.

```rust
/// Write a message to a QUIC send stream.
pub async fn write_message(stream: &mut SendStream, msg: &ServerMessage, seq: &mut u64) -> Result<()> {
    let envelope = json!({ "v": 1, "seq": *seq, "type": variant_name(msg), "data": msg });
    let bytes = serde_json::to_vec(&envelope)?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    *seq += 1;
    Ok(())
}

/// Read a message from a QUIC recv stream.
pub async fn read_message(stream: &mut RecvStream) -> Result<ClientMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE { return Err(...) }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    let envelope: Envelope = serde_json::from_slice(&buf)?;
    // ignore unknown versions/types (forward compat)
    Ok(envelope.into_client_message()?)
}
```

- [ ] Implement write/read with length-prefixed framing
- [ ] Define `Envelope` struct with `v`, `seq`, `type`, `data`
- [ ] Handle unknown message types gracefully (log + skip)
- [ ] Unit test: round-trip serialization for all message types
- [ ] Test: oversized message rejected

### 2.6 Reconnection + Ring Buffer

New file: `packages/crab_city/src/transport/replay_buffer.rs`

Server-side bounded ring buffer for reconnection replay.

```rust
pub struct ReplayBuffer {
    buffer: VecDeque<(u64, Vec<u8>)>,  // (seq, serialized message)
    max_entries: usize,                 // default 1000
    max_age: Duration,                  // default 5 min
}

impl ReplayBuffer {
    pub fn push(&mut self, seq: u64, msg: &[u8]);
    pub fn replay_since(&self, last_seq: u64) -> Option<Vec<&[u8]>>;
        // Returns None if last_seq is too old (caller should send snapshot)
    pub fn evict_expired(&mut self);
}
```

On reconnect:
1. Client sends `{ "last_seq": N }` in initial stream message
2. If N is in the buffer: replay all messages since N, then resume live
3. If N is too old or missing: send full state snapshot, then resume live

- [ ] Implement ring buffer with seq tracking
- [ ] Wire into connection handler: check `last_seq` on new connections
- [ ] Snapshot generation: reuse existing `InstanceList` + state logic
- [ ] Test: replay works, too-old falls back to snapshot

### 2.7 Keepalive + Ghost Cleanup

In the connection handler:

```rust
// Server sends keepalive ping every 30s via QUIC keepalive config
// If client doesn't respond within 10s, connection is closed
// On connection drop: remove from connections map, broadcast presence update
```

- [ ] Configure QUIC keepalive on the iroh endpoint (30s interval, 10s timeout)
- [ ] On connection drop: clean up presence, broadcast `PresenceUpdate`

### Phase 2 Checklist

- [ ] iroh endpoint starts and accepts native QUIC connections
- [ ] Embedded relay starts and accepts browser connections (via iroh WASM)
- [ ] NodeId extracted from handshake, grant looked up
- [ ] Connections with active grants receive state snapshot
- [ ] Connections without grants can redeem invites
- [ ] Messages flow bidirectionally with envelope framing
- [ ] Reconnection replays from ring buffer or falls back to snapshot
- [ ] Ghost users cleaned up after keepalive timeout
- [ ] `cargo check -p crab_city` passes
- [ ] `cargo test -p crab_city` passes

---

## Phase 3: Auth Middleware + Old Auth Removal

### 3.1 New Auth Extractor

Replace the session-based `AuthUser` with an iroh-backed one.

Modify: `packages/crab_city/src/auth.rs`

Delete everything related to sessions, cookies, passwords, CSRF. Replace with:

```rust
/// Populated from the iroh connection state.
pub struct AuthUser {
    pub public_key: PublicKey,
    pub fingerprint: String,
    pub display_name: String,
    pub capability: Capability,
    pub access: AccessRights,
}

impl AuthUser {
    /// Check if the user has a specific access right.
    pub fn require_access(&self, type_: &str, action: &str) -> Result<(), AuthError> {
        if self.access.contains(type_, action) {
            Ok(())
        } else {
            Err(AuthError::InsufficientAccess {
                required_type: type_.into(),
                required_action: action.into(),
            })
        }
    }
}

/// For loopback connections.
impl AuthUser {
    pub fn loopback() -> Self {
        AuthUser {
            public_key: PublicKey::LOOPBACK,
            fingerprint: PublicKey::LOOPBACK.fingerprint(),
            display_name: "Local".into(),
            capability: Capability::Owner,
            access: Capability::Owner.access_rights(),
        }
    }
}
```

For the transition period (while HTTP handlers still exist for static assets,
health checks, and metrics), the loopback bypass stays:

```
1. Loopback connection → AuthUser::loopback()
2. iroh connection → AuthUser from grant
3. HTTP request to public route → no auth required
4. HTTP request to non-public route → 401 (must use iroh)
```

- [ ] Rewrite `AuthUser` to use `PublicKey` + `AccessRights`
- [ ] Implement `require_access()` using capability algebra
- [ ] Delete: `hash_password`, `verify_password`, `generate_session_token`,
      `generate_csrf_token`, session middleware, CSRF checking, cookie helpers,
      all `/api/auth/*` handlers
- [ ] Delete: `packages/crab_city/src/repository/auth.rs` (sessions, users,
      invitations, permissions)
- [ ] Delete: `packages/crab_city/src/cli/auth.rs` (enable/disable/status
      commands — replaced by keypair-based identity)
- [ ] Remove deps: `argon2`, `password-hash`, `rpassword`
- [ ] Update all handlers that extract `AuthUser` — they now get it from the
      iroh connection context, not from cookie middleware
- [ ] Verify: `cargo check -p crab_city` (every handler that used old AuthUser
      now compiles with new AuthUser)

### 3.2 Handler Migration

Every handler that currently uses `AuthUser` or `MaybeAuthUser` needs to work
with the new iroh-backed version. The handlers themselves are
transport-unaware — they take `AuthUser` and do their thing.

The routing changes: instead of axum routes with cookie middleware, handlers
are dispatched from the iroh RPC message loop.

Existing handlers that can be called as iroh RPC:
- Task CRUD (already exists in `handlers/tasks.rs`)
- Chat send/history (already exists in `ws/handler.rs`)
- Instance management (already exists)
- Terminal input/resize (already exists)

These keep their logic but get called from the iroh message dispatcher instead
of the axum router or WebSocket handler.

- [ ] Create RPC dispatcher in `transport/iroh_transport.rs` that routes
      `ClientMessage` variants to existing handler logic
- [ ] Verify each handler works with new `AuthUser`
- [ ] Add `require_access()` checks where the design doc specifies them (e.g.,
      `ListMembers` requires `content:read`, `UpdateMember` requires
      `members:update`)

### Phase 3 Checklist

- [ ] Old auth code fully deleted
- [ ] All handlers use `require_access()` for authorization
- [ ] `cargo check -p crab_city` passes with zero references to sessions,
      cookies, passwords
- [ ] `cargo test -p crab_city` passes

---

## Phase 4: Invite + Membership RPC Handlers

The new operations that don't exist yet.

### 4.1 Invite Operations

New file: `packages/crab_city/src/handlers/invites.rs`

These are iroh RPC handlers (called from the message dispatcher).

**CreateInvite:**
```
Auth: require_access("members", "invite")
Input: { capability, max_uses, expires_in_hours, max_depth, idempotency_key }
Logic:
  1. Verify requested capability <= caller's capability
  2. Generate nonce (16 random bytes)
  3. Sign invite with instance identity key (root link, issuer = caller)
  4. Store in invites table
  5. Log invite.created event
  6. Return { token (base32), url, qr_data }
Idempotency: if idempotency_key matches existing invite, return that invite
```

**RedeemInvite:**
```
Auth: none (this is how you GET auth)
Input: { token (base32), display_name }
Caller identity: implicit from iroh NodeId
Logic:
  1. Parse invite from base32
  2. Verify invite (signature chain, capability narrowing, depth)
  3. Verify root issuer has members:invite access on this instance
  4. Check invite: not expired, not exhausted, not revoked
  5. Idempotency check: if (nonce, NodeId) already has a grant, return it
  6. Create member_identities row
  7. Create member_grants row (state = active, capability from leaf link)
  8. Increment use count
  9. Log invite.redeemed + member.joined events
  10. Send full state snapshot
  11. Broadcast MemberJoined to all other connections
```

**RevokeInvite:**
```
Auth: require_access("members", "invite")
Input: { nonce, suspend_derived_members }
Logic:
  1. Revoke invite (set revoked_at)
  2. If suspend_derived_members: transition all grants with invited_via=nonce
     to suspended, close their iroh connections
  3. Log invite.revoked event
```

- [ ] Implement CreateInvite
- [ ] Implement RedeemInvite (with full chain verification)
- [ ] Implement RevokeInvite
- [ ] Wire into RPC dispatcher
- [ ] Test: create invite → redeem → verify grant created
- [ ] Test: redeem with wrong signature → rejected
- [ ] Test: redeem expired invite → rejected
- [ ] Test: redeem exhausted invite → rejected
- [ ] Test: idempotent redemption → same grant returned
- [ ] Test: delegation chain → capability narrowing enforced

### 4.2 Member Management Operations

New file: `packages/crab_city/src/handlers/members.rs`

**ListMembers:**
```
Auth: require_access("content", "read")
Returns: all members with identity + grant data
```

**UpdateMember:**
```
Auth: require_access("members", "update")
Input: { public_key, capability }
Constraint: cannot escalate beyond caller's own capability
Logic: update grant, expand access rights from new capability, log event, broadcast
```

**TweakAccess:**
```
Auth: require_access("members", "update")
Input: { public_key, add: [...], remove: [...] }
Logic: apply diff to access rights, log event, broadcast
```

**SuspendMember:**
```
Auth: require_access("members", "suspend")
Input: { public_key, reason }
Logic: transition grant to suspended, close their iroh connection, log event, broadcast
```

**ReinstateMember:**
```
Auth: require_access("members", "reinstate")
Input: { public_key }
Logic: transition grant to active, log event, broadcast
```

**RemoveMember:**
```
Auth: require_access("members", "remove")
Input: { public_key }
Constraint: cannot remove owner
Logic: transition grant to removed, close iroh connection, log event, broadcast
```

**ReplaceMember:**
```
Auth: require_access("members", "update")
Input: { new_public_key, old_public_key }
Logic: set replaces on new grant, transition old to removed, log event, broadcast
```

**QueryEvents:**
```
Auth: require_access("members", "read")
Input: { target, event_type, limit, before_id }
Returns: paginated events
```

**VerifyEvents:**
```
Auth: require_access("members", "read")
Input: { from_id, to_id }
Returns: chain verification result + checkpoint details
```

**GetEventProof:**
```
Auth: require_access("content", "read")
Input: { event_id }
Returns: event + prev_hash + hash + nearest checkpoint
```

- [ ] Implement all handlers
- [ ] Wire into RPC dispatcher
- [ ] Test: full lifecycle (create invite → redeem → promote → suspend →
      reinstate → remove)
- [ ] Test: capability escalation blocked
- [ ] Test: owner cannot be removed
- [ ] Test: suspension closes iroh connection immediately
- [ ] Test: event log records all transitions
- [ ] Test: chain verification detects tampering

### Phase 4 Checklist

- [ ] All invite operations work end-to-end
- [ ] All member management operations work end-to-end
- [ ] Event log records every state transition with correct hash chaining
- [ ] Access rights enforced on every operation
- [ ] `cargo test -p crab_city` passes

---

## Phase 5: Wire Format + Protocol Updates

### 5.1 Envelope Versioning

Wrap all existing `ServerMessage` and `ClientMessage` variants in the envelope
format. No backwards compatibility — just change the format.

Modify: `packages/crab_city/src/ws/protocol.rs`

Add new variants for auth-related messages:

```rust
// New ServerMessage variants
GrantUpdate { grant: MemberGrantInfo },
IdentityUpdate { identity: MemberIdentityInfo },
MemberJoined { identity: MemberIdentityInfo, grant: MemberGrantInfo },
MemberRemoved { public_key: String },
ConnectionClosed { reason: String },

// New ClientMessage variants (RPC requests)
CreateInvite { capability: String, max_uses: u32, expires_in_hours: Option<u32>, max_depth: Option<u8>, idempotency_key: Option<String> },
RedeemInvite { token: String, display_name: String },
RevokeInvite { nonce: String, suspend_derived_members: bool },
ListMembers,
UpdateMember { public_key: String, capability: String },
TweakAccess { public_key: String, add: Vec<AccessRight>, remove: Vec<AccessRight> },
SuspendMember { public_key: String, reason: String },
ReinstateMember { public_key: String },
RemoveMember { public_key: String },
ReplaceMember { new_public_key: String, old_public_key: String },
QueryEvents { target: Option<String>, event_type: Option<String>, limit: u32, before_id: Option<i64> },
VerifyEvents { from_id: i64, to_id: i64 },
GetEventProof { event_id: i64 },
```

Response types for RPC (request-response, not broadcast):

```rust
// RPC responses sent back on the same stream
InviteCreated { token: String, url: String, qr_data: String },
InviteRedeemed { identity: MemberIdentityInfo, grant: MemberGrantInfo },
InviteRevoked { revoked: bool, members_suspended: u32 },
MembersList { members: Vec<MemberInfo> },
GrantUpdated { grant: MemberGrantInfo },
AccessUpdated { access: Vec<AccessRight> },
EventsResponse { events: Vec<EventInfo>, has_more: bool },
EventVerification { valid: bool, events_checked: u64, chain_head: ChainHeadInfo, checkpoints: Vec<CheckpointInfo> },
EventProofResponse { event: EventInfo, prev_hash: String, hash: String, nearest_checkpoint: Option<CheckpointInfo> },
```

- [ ] Add new variants to `ServerMessage` and `ClientMessage`
- [ ] Add RPC response types
- [ ] All messages serialized with envelope: `{ v, seq, type, data }`
- [ ] Update all existing message serialization to use envelope format
- [ ] Clients must ignore unknown `type` values (forward compat)

### 5.2 Preview WebSocket

New file: `packages/crab_city/src/handlers/preview.rs`

The one remaining WebSocket endpoint — unauthenticated, for the join page.

```rust
// GET /api/preview (WebSocket upgrade)
// No auth required. Rate-limited to 5 concurrent per IP.
// Server sends PreviewActivity every 2 seconds:
//   { terminal_count, active_cursors: [{terminal_id, row, col}],
//     user_count, instance_name, uptime_secs }
// No client -> server messages accepted.
```

- [ ] Implement preview WebSocket handler
- [ ] Rate limit: 5 concurrent per IP (in-memory counter)
- [ ] Strict allowlist: only non-content signals (no terminal text, no chat, no
      task details, no user names)
- [ ] Wire into axum router (public route, no auth)

### Phase 5 Checklist

- [ ] All messages use envelope format
- [ ] New auth message types compile and serialize correctly
- [ ] Preview WebSocket works without auth
- [ ] `cargo test -p crab_city` passes

---

## Phase 6: Frontend

### 6.1 iroh WASM Integration

Modify: `packages/crab_city_ui/`

**New files:**
- `src/lib/iroh/client.ts` — iroh WASM wrapper
- `src/lib/iroh/keypair.ts` — ed25519 key generation + IndexedDB storage
- `src/lib/iroh/framing.ts` — envelope encode/decode

```typescript
// client.ts
export class IrohClient {
    private endpoint: IrohEndpoint;  // from iroh WASM
    private connection: IrohConnection | null;
    private seq: number = 0;

    static async init(keypair: Ed25519Keypair): Promise<IrohClient>;

    async connect(relayUrl: string): Promise<void>;
    async disconnect(): Promise<void>;

    async send(msg: ClientMessage): Promise<void>;
    onMessage(handler: (msg: ServerMessage) => void): void;

    get nodeId(): string;  // hex-encoded public key
    get fingerprint(): string;  // crab_XXXXXXXX

    // Reconnection
    private lastSeq: number = 0;
    private async reconnect(): Promise<void>;
}
```

```typescript
// keypair.ts
export class KeyManager {
    // Store keypair in IndexedDB, encrypted with a passphrase via AES-256-GCM
    static async generate(): Promise<Ed25519Keypair>;
    static async load(): Promise<Ed25519Keypair | null>;
    static async save(keypair: Ed25519Keypair): Promise<void>;
    static async export(keypair: Ed25519Keypair): Promise<Uint8Array>;
    static async import(bytes: Uint8Array): Promise<Ed25519Keypair>;
}
```

- [ ] Assess iroh WASM npm package (or compile from source)
- [ ] Implement `IrohClient` with connect/disconnect/send/receive
- [ ] Implement `KeyManager` with IndexedDB storage
- [ ] Replace existing WebSocket connection with `IrohClient`
- [ ] Update `ws-handlers.ts` to handle envelope-formatted messages
- [ ] Handle reconnection: track `lastSeq`, replay on reconnect
- [ ] Test: client connects, receives state snapshot

### 6.2 Join Page

New route: `src/routes/join/+page.svelte`

```
URL: /join#<base32-invite-token>

Layout:
  ┌────────────────────────────────────────┐
  │  Instance name                         │
  │  Invited by crab_XXXXXXXX              │
  │  You're being invited to collaborate   │
  │                                        │
  │  ┌──────────────────────┐              │
  │  │ Preview panel        │  3 users     │
  │  │ (cursor movement,    │  online      │
  │  │  no content)         │              │
  │  └──────────────────────┘              │
  │                                        │
  │  Your name: [______________]           │
  │                                        │
  │  [ Join ]                              │
  │                                        │
  │  This will create a cryptographic      │
  │  identity on your device.              │
  └────────────────────────────────────────┘
```

On "Join":
1. If no keypair in IndexedDB: generate one → show key backup modal (blocking)
2. Connect to instance via iroh WASM (through embedded relay)
3. Send `RedeemInvite { token, display_name }` as first message
4. On success: redirect to main app view
5. On error: show structured error with recovery action

- [ ] Implement join page
- [ ] Extract invite token from URL fragment
- [ ] Parse invite (client-side) to display instance name, inviter, capability
- [ ] Connect to preview WebSocket for live activity display
- [ ] Key generation via `KeyManager`
- [ ] Key backup modal (blocking, checkbox required)
- [ ] Invite redemption flow
- [ ] Error handling with recovery actions
- [ ] Test: full join flow from link click to authenticated session

### 6.3 Key Backup Modal

New component: `src/lib/components/KeyBackupModal.svelte`

Blocking modal that cannot be dismissed without confirming:

```
┌────────────────────────────────────────────┐
│  Save Your Identity Key                    │
│                                            │
│  This key is the only proof of your        │
│  identity. If you lose it, you'll need     │
│  an admin to re-invite you.                │
│                                            │
│  [ Copy to clipboard ]  [ Download .key ]  │
│                                            │
│  ☐ I saved my key somewhere safe           │
│                                            │
│  [ Continue ]  (grayed out until checked)  │
└────────────────────────────────────────────┘
```

- [ ] Implement modal component
- [ ] Copy-to-clipboard (base64-encoded private key)
- [ ] Download as `.key` file
- [ ] Checkbox gates the Continue button
- [ ] Cannot be dismissed by clicking outside or pressing Escape

### 6.4 Member Management UI

New component: `src/lib/components/MemberPanel.svelte`

For admins: list members, show state, manage capabilities.

- [ ] Member list with fingerprints, display names, capabilities, state badges
- [ ] Invite creation form (capability picker, max uses, expiry)
- [ ] QR code rendering for invites (client-side SVG)
- [ ] Suspend/reinstate/remove actions
- [ ] Capability change dropdown

### 6.5 Identity Display

- [ ] Show current user fingerprint in header/sidebar
- [ ] Show `crab_XXXXXXXX` next to display names in chat, presence, tasks

### Phase 6 Checklist

- [ ] Browser connects via iroh WASM through embedded relay
- [ ] Join page works end-to-end (link → preview → name → keygen → backup →
      redeem → connected)
- [ ] Key backup modal is blocking and functional
- [ ] Member panel shows real data
- [ ] Invite QR codes render
- [ ] Fingerprints visible throughout the UI
- [ ] `pnpm test` passes
- [ ] `pnpm build` produces a working build

---

## Phase 7: Instance Bootstrap + CLI

### 7.1 Instance First-Run

On first startup (no members except loopback):

```
$ crab-city start

Instance identity: crab_2K7XM9QP
No members yet. Creating owner invite...

Owner invite: CRJKF8EH3VMQ7G6Y...
Join URL: http://localhost:3000/join#CRJKF8EH3VMQ7G6Y...

Or scan:
█▀▀▀▀▀█ █ ▄█▀ █▀▀▀▀▀█
█ ███ █ ▀█▀▄  █ ███ █
...
```

- [ ] Detect first-run (only loopback member exists)
- [ ] Generate owner invite (capability = owner, max_uses = 1)
- [ ] Print invite token + URL + QR code to stdout
- [ ] Store invite in database

### 7.2 CLI Identity

The CLI/TUI uses a persistent keypair at `~/.config/crabcity/identity.key`.

Modify: CLI startup code

```
1. Check ~/.config/crabcity/identity.key
2. If missing: generate, save (mode 0600), print fingerprint
3. Connect to instance via iroh QUIC
4. If no grant: prompt for invite token, send RedeemInvite
5. If grant active: connected, show instance
```

- [ ] Key file load/generate on CLI startup
- [ ] Invite redemption prompt when no grant exists
- [ ] Print connection status with fingerprint

### 7.3 TUI QR Code Rendering

For `/invite` command output in the TUI.

```rust
// Render invite as QR code using Unicode half-block characters
// ▀▄█  — no external dependencies, works in any terminal
fn render_qr(data: &str) -> String;
```

- [ ] Implement QR code renderer (half-block encoding)
- [ ] Wire into `/invite` TUI command

### Phase 7 Checklist

- [ ] First-run prints owner invite with QR code
- [ ] CLI generates and persists keypair
- [ ] CLI can redeem invite and connect
- [ ] TUI `/invite` command creates and displays invite with QR

---

## Phase 8: Observability

### 8.1 Prometheus Metrics

Add to `GET /metrics`:

```
# iroh connections
crabcity_iroh_connections_active{transport="quic|relay"} gauge
crabcity_iroh_connections_total{transport="quic|relay"} counter
crabcity_iroh_reconnections_total counter
crabcity_iroh_auth_rejections_total{reason="no_grant|suspended|blocklisted"} counter
crabcity_replay_messages_total counter
crabcity_snapshots_total counter

# Membership
crabcity_grants_by_state{state="invited|active|suspended|removed"} gauge
crabcity_invites_redeemed_total{capability="view|collaborate|admin"} counter
crabcity_invites_active gauge

# Event log
crabcity_event_log_size gauge
crabcity_event_log_append_latency_seconds histogram
```

- [ ] Add metrics crate dependency (`metrics`, `metrics-exporter-prometheus`)
- [ ] Instrument connection accept/reject
- [ ] Instrument invite redemption
- [ ] Instrument grant state transitions
- [ ] Instrument event log append latency

### 8.2 Structured Logging

Every auth decision emits a structured log line:

```
level=INFO msg="connection accepted" node_id_fingerprint=crab_2K7XM9QP transport=quic capability=collaborate
level=WARN msg="connection rejected" node_id_fingerprint=crab_7F3XM9QP reason=no_grant transport=relay
level=INFO msg="member suspended" actor=crab_2K7XM9QP target=crab_7F3XM9QP reason="admin action"
```

- [ ] Add structured fields to all auth log lines
- [ ] Add structured fields to all state transition log lines

### Phase 8 Checklist

- [ ] `/metrics` endpoint returns all iroh + membership counters
- [ ] Auth decisions produce structured log lines
- [ ] State transitions produce structured log lines

---

## Phase 9: Integration Tests

New file: `packages/crab_city/tests/interconnect_integration.rs`

End-to-end tests that spin up a real instance with an iroh endpoint. All tests
use iroh connections.

### 9.1 Connection + Auth

```rust
#[tokio::test]
async fn connect_with_active_grant()
    // Generate keypair → seed grant → connect via iroh → receive snapshot

#[tokio::test]
async fn connect_without_grant_rejected()
    // Generate keypair → connect → receive Error { not_a_member, recovery: redeem_invite }

#[tokio::test]
async fn connect_redeem_invite()
    // Create invite → new keypair → connect → RedeemInvite → grant created → snapshot received

#[tokio::test]
async fn suspend_closes_connection()
    // Connect → admin suspends → connection closed with reason

#[tokio::test]
async fn reconnect_replays_from_buffer()
    // Connect → receive messages → disconnect → reconnect with last_seq → verify replay

#[tokio::test]
async fn reconnect_old_seq_gets_snapshot()
    // Connect → disconnect → wait → reconnect with very old seq → verify snapshot
```

### 9.2 Invites + Delegation

```rust
#[tokio::test]
async fn create_and_redeem_flat_invite()

#[tokio::test]
async fn delegated_invite_chain_verified()
    // Admin creates invite with max_depth=2 → member delegates → sub-delegate redeems

#[tokio::test]
async fn delegation_forgery_rejected()
    // Tamper with a link in a delegation chain → redemption rejected

#[tokio::test]
async fn capability_narrowing_enforced()
    // Admin invite → delegate with lower cap → verify grant has lower cap

#[tokio::test]
async fn expired_invite_rejected()

#[tokio::test]
async fn exhausted_invite_rejected()

#[tokio::test]
async fn idempotent_redemption()
    // Redeem same invite with same NodeId twice → same grant returned

#[tokio::test]
async fn revoke_invite_unredeemed_fail()
    // Revoke → attempt redeem → rejected

#[tokio::test]
async fn revoke_invite_suspend_derived()
    // Revoke with suspend_derived → all members from that invite suspended
```

### 9.3 Member Management

```rust
#[tokio::test]
async fn full_lifecycle()
    // invite → redeem → promote → suspend → reinstate → remove

#[tokio::test]
async fn capability_escalation_blocked()
    // collaborate user cannot promote to admin

#[tokio::test]
async fn owner_cannot_be_removed()

#[tokio::test]
async fn access_rights_enforced()
    // view user cannot send chat, collaborate user cannot manage members

#[tokio::test]
async fn replace_member_key()
    // New key replaces old, old grant removed, replaces link set
```

### 9.4 Event Log

```rust
#[tokio::test]
async fn event_chain_integrity()
    // Perform operations → verify chain → tamper → verify detects

#[tokio::test]
async fn checkpoint_signature_valid()

#[tokio::test]
async fn all_transitions_logged()
    // Every state transition produces correct event type
```

### 9.5 Preview + Errors

```rust
#[tokio::test]
async fn preview_websocket_no_content_leak()
    // Connect to /api/preview → verify only non-content signals received

#[tokio::test]
async fn all_errors_have_recovery()
    // Trigger each error type → verify recovery field present with valid action
```

### 9.6 Loopback

```rust
#[tokio::test]
async fn loopback_gets_owner_access()

#[tokio::test]
async fn loopback_pubkey_rejected_remotely()
    // All-zeros pubkey on non-loopback connection → rejected
```

- [ ] Implement all integration tests
- [ ] All tests pass with `cargo test -p crab_city`

### Phase 9 Checklist

- [ ] All integration tests pass
- [ ] Code coverage: every RPC handler exercised
- [ ] Every error path tested
- [ ] Every state transition tested

---

## File Layout (New + Modified)

```
packages/crab_city/
  migrations/
    NNN_interconnect_auth.sql           NEW

  src/
    auth.rs                             REWRITE (delete old, new AuthUser)
    identity.rs                         NEW (instance keypair)
    config.rs                           MODIFY (add [transport] section)

    transport/
      mod.rs                            NEW
      iroh_transport.rs                 NEW (endpoint, accept loop, connection map)
      relay.rs                          NEW (embedded relay)
      framing.rs                        NEW (length-prefixed envelope)
      replay_buffer.rs                  NEW (reconnection ring buffer)

    repository/
      auth.rs                           DELETE
      membership.rs                     NEW
      invites.rs                        NEW
      event_log.rs                      NEW

    handlers/
      invites.rs                        NEW (create, redeem, revoke RPC)
      members.rs                        NEW (list, update, suspend, etc.)
      preview.rs                        NEW (unauthenticated preview WS)

    cli/
      auth.rs                           DELETE (replaced by keypair identity)

    ws/
      protocol.rs                       MODIFY (add auth message variants, envelope)
      handler.rs                        MODIFY (route from iroh dispatcher)

  tests/
    interconnect_integration.rs         NEW

packages/crab_city_ui/
  src/
    lib/
      iroh/
        client.ts                       NEW
        keypair.ts                      NEW
        framing.ts                      NEW
      components/
        KeyBackupModal.svelte           NEW
        MemberPanel.svelte              NEW
    routes/
      join/
        +page.svelte                    NEW
    stores/
      ws-handlers.ts                    MODIFY (envelope format)
```

---

## Dependency Order

```
Phase 0: Spike (iroh WASM + relay)     ← GATES EVERYTHING
  │
  ├── Phase 1: Database + repository    ← no iroh, just SQL + Rust
  │     │
  │     └── Phase 4: RPC handlers       ← uses repository layer
  │           │
  │           └── Phase 9: Integration tests
  │
  ├── Phase 2: iroh transport           ← uses iroh deps from spike
  │     │
  │     ├── Phase 3: Auth rewrite       ← replaces old auth with iroh-backed
  │     │
  │     └── Phase 5: Wire format        ← envelope framing over iroh streams
  │           │
  │           └── Phase 6: Frontend     ← iroh WASM, join page, member UI
  │                 │
  │                 └── Phase 7: Bootstrap + CLI
  │
  └── Phase 8: Observability            ← can be done alongside any phase
```

Phases 1 and 2 can run in parallel (no dependency between them).
Phase 8 can be done alongside any phase after Phase 2.
Phase 9 should be incremental — add tests as each phase completes.

## Estimated Sizes

| Phase | Rust LOC | Frontend LOC | Test LOC |
|-------|----------|-------------|----------|
| 0: Spike | ~200 | ~100 | — |
| 1: Database + repo | ~600 | — | ~200 |
| 2: iroh transport | ~800 | — | ~100 |
| 3: Auth rewrite | ~150 (+delete ~1500) | — | ~50 |
| 4: RPC handlers | ~600 | — | ~200 |
| 5: Wire format | ~200 | — | ~50 |
| 6: Frontend | — | ~1400 | ~200 |
| 7: Bootstrap + CLI | ~200 | — | ~50 |
| 8: Observability | ~150 | — | — |
| 9: Integration tests | — | — | ~600 |
| **Total** | **~2900** (+delete ~1500) | **~1500** | **~1450** |

Net lines: ~2900 new Rust + ~1500 new frontend - ~1500 deleted old auth
≈ **4400 new lines + 1450 test lines**

## Done Criteria

- [ ] No references to sessions, cookies, passwords, or CSRF in the codebase
- [ ] Native client connects via iroh QUIC, authenticates via handshake
- [ ] Browser client connects via iroh WASM through embedded relay
- [ ] Invite creation, redemption, and revocation work end-to-end
- [ ] Delegation chains verified and capability narrowing enforced
- [ ] Member lifecycle: invite → join → promote → suspend → reinstate → remove
- [ ] Access rights enforced on every operation
- [ ] Event log records every state transition with hash chaining
- [ ] Reconnection replays from ring buffer or sends snapshot
- [ ] Ghost users cleaned up by keepalive timeout
- [ ] Join page with live preview, key backup modal, invite redemption
- [ ] Member management UI for admins
- [ ] QR code rendering for invites (TUI + web)
- [ ] Prometheus metrics for connections, grants, events
- [ ] Structured logging for all auth decisions
- [ ] All integration tests pass
- [ ] `cargo check -p crab_city` passes
- [ ] `cargo test -p crab_city` passes
- [ ] `pnpm build` produces a working frontend
- [ ] `pnpm test` passes
- [ ] `bazel test //...` passes
