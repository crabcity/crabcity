# Crab City Accounts: Design

## Overview

This document specifies the detailed technical design for the Crab City account
system. It covers data models, API contracts, cryptographic protocols, and the
interaction patterns between instances, the registry, and clients.

## 1. Cryptographic Primitives

### 1.1 Key Generation

All keypairs are ed25519 (the same curve used by iroh's `NodeId`). Keys are
generated using `ed25519-dalek` (already a transitive dependency via iroh).

Client-side key generation (browser):
- Use Web Crypto API (`Ed25519` via the `SubtleCrypto` interface)
- Store private key in IndexedDB, encrypted with a user-chosen passphrase via
  AES-256-GCM (PBKDF2-derived key)
- Export: PKCS8 (private), raw 32-byte (public)

Client-side key generation (CLI/TUI):
- Generate via `ed25519_dalek::SigningKey::generate(&mut OsRng)`
- Store in `~/.config/crabcity/identity.key` (mode 0600)

### 1.2 Key Fingerprints

Human-readable short identifiers for public keys.

Format: `crab_` + first 8 characters of Crockford base32 encoding of the 32-byte
public key.

Example: `crab_2K7XM9QP`

Properties:
- 40 bits of entropy — sufficient to distinguish members within any realistic
  instance
- Case-insensitive (Crockford base32)
- Used in TUI display, logs, admin UIs, CLI output
- Never used for lookups or authentication — display only
- Defined in `crab_city_auth` crate: `PublicKey::fingerprint() -> String`

### 1.3 iroh Authentication (All Clients)

All clients — native and browser — authenticate via the iroh handshake. The
ed25519 keypair IS the iroh `NodeId`, so proving keypair ownership is implicit
in connection establishment.

```
Client (native or browser WASM)    Instance
  |                                    |
  |  iroh connect                      |
  |  (client NodeId = ed25519 pubkey)  |
  | ---------------------------------->|
  |                                    |  extract client NodeId from handshake
  |                                    |  lookup grant by pubkey
  |                                    |  check grant state == active
  |                                    |  open bidirectional stream
  |  E2E encrypted stream              |
  | <--------------------------------->|
  |                                    |
  |  { v:1, seq:0, type:"Snapshot",   |
  |    data: { full state } }          |  initial state snapshot
  | <----------------------------------|
  |                                    |
  |  bidirectional message stream      |
  |  { v, seq, type, data }            |
  | <--------------------------------->|
```

Native clients connect via QUIC directly. Browser clients connect via iroh WASM
through the instance's embedded relay (WebSocket transport, same iroh protocol).
Both paths perform the same ed25519 handshake.

Properties:
- **Zero token management.** No session tokens, no refresh tokens, no token
  expiry, no challenge-response. The connection IS the authenticated session.
- **Immediate revocation.** When an admin suspends a user, the instance closes
  their iroh connection. No revocation set, no token expiry window.
- **Connection = session.** Disconnecting ends the session. Reconnecting
  re-authenticates via handshake.
- **E2E encrypted.** QUIC authenticated encryption. Browser clients get the
  same E2E encryption — the embedded relay cannot decrypt traffic.

The instance maintains a map of `NodeId -> active iroh connection` for all
connected clients. Presence, broadcast, and connection cleanup are driven by
connection state.

### 1.4 Invite Token Format

A flat (non-delegated) invite:

```
Invite = {
    version: u8,                    // 0x01
    instance: [u8; 32],            // instance NodeId
    chain_length: u8,              // number of InviteLinks (1 for flat invites)
    links: [InviteLink],          // ordered, root-to-leaf
}

InviteLink = {
    issuer: [u8; 32],             // ed25519 public key
    capability: u8,               // 0=view, 1=collaborate, 2=admin
    max_depth: u8,                // remaining delegation depth (0 = leaf, cannot delegate further)
    max_uses: u32,                // 0 = unlimited
    expires_at: u64,              // unix timestamp, 0 = never
    nonce: [u8; 16],              // random, for uniqueness
    signature: [u8; 64],          // signs H(prev_link) ++ instance ++ own fields (root link signs H(0x00*32) ++ instance ++ own fields)
}
```

Per-link size: 32 + 1 + 1 + 4 + 8 + 16 + 64 = **126 bytes**

Flat invite total: 1 + 32 + 1 + 126 = **160 bytes** (256 chars base32)

Delegated invite (3-hop chain): 1 + 32 + 1 + (126 * 3) = **412 bytes** (660
chars base32)

Verification (delegation chain):
1. Root link: verify `signature` over `H(0x00*32) ++ instance ++ fields`; root
   issuer must have `members:invite` access on the instance
2. Each subsequent link: verify `signature` over `H(prev_link) ++ instance ++
   fields`
3. Each link's `capability` must be <= previous link's `capability`
4. Each link's `max_depth` must be < previous link's `max_depth`
5. All links must be unexpired and within use limits

A flat invite is a chain of length 1 with `max_depth = 0`.

URL format:
```
https://<instance-host>/join#<base32-token>
```

Fragment (`#`) ensures the token never appears in server access logs or referrer
headers. The SvelteKit frontend extracts it client-side.

Registry-mediated URL format:
```
https://crabcity.dev/join/<short-code>
```

Where `short-code` is an 8-character random alphanumeric ID that maps to a
stored invite in the registry database.

### 1.5 Loopback Identity

The loopback identity is a well-known sentinel public key: 32 zero bytes (`0x00
* 32`).

Rules:
- Instances reject this pubkey on any non-loopback connection (iroh, invite
  redemption, OIDC)
- The loopback identity always has an `owner` grant with state `active`
- It is seeded during instance bootstrap (not created via invite)
- It cannot be suspended, removed, or have its capability changed

## 2. Instance-Side Data Model

### 2.1 Schema (SQLite)

```sql
-- WHO you are (identity, cached from registry or self-reported)
CREATE TABLE member_identities (
    public_key BLOB NOT NULL PRIMARY KEY,  -- 32 bytes, ed25519
    display_name TEXT NOT NULL DEFAULT '',
    handle TEXT,                            -- @alex, from registry
    avatar_url TEXT,
    registry_account_id TEXT,              -- UUID, from registry resolution
    resolved_at TEXT,                       -- when identity was last resolved from registry
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- WHAT you can do (authorization, instance-local)
CREATE TABLE member_grants (
    public_key BLOB NOT NULL PRIMARY KEY,  -- 32 bytes, ed25519
    capability TEXT NOT NULL,               -- 'view', 'collaborate', 'admin', 'owner'
    access TEXT NOT NULL DEFAULT '[]',      -- JSON array of GNAP-style access rights
    state TEXT NOT NULL DEFAULT 'invited',  -- 'invited', 'active', 'suspended', 'removed'
    org_id TEXT,                            -- UUID, from OIDC claims
    invited_by BLOB,                        -- 32 bytes, pubkey of inviter
    invited_via BLOB,                       -- 16 bytes, invite nonce (traces which invite)
    replaces BLOB,                          -- 32 bytes, pubkey of old grant (key loss recovery)
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (public_key) REFERENCES member_identities(public_key)
);

-- Invite tokens created by this instance
CREATE TABLE invites (
    nonce BLOB NOT NULL PRIMARY KEY,       -- 16 bytes
    issuer BLOB NOT NULL,                  -- 32 bytes, pubkey
    capability TEXT NOT NULL,
    max_uses INTEGER NOT NULL DEFAULT 0,   -- 0 = unlimited
    use_count INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT,                        -- ISO 8601, NULL = never
    signature BLOB NOT NULL,               -- 64 bytes
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_at TEXT,                        -- NULL if active
    FOREIGN KEY (issuer) REFERENCES member_identities(public_key)
);

-- Instance-local blocklist
CREATE TABLE blocklist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_type TEXT NOT NULL,              -- 'pubkey', 'node_id', 'ip_range'
    target_value BLOB NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    added_by BLOB NOT NULL,                -- pubkey of admin who added it
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Cached global/org blocklist from registry
CREATE TABLE blocklist_cache (
    scope TEXT NOT NULL,                   -- 'global', 'org:<uuid>'
    version INTEGER NOT NULL,
    target_type TEXT NOT NULL,
    target_value BLOB NOT NULL,
    PRIMARY KEY (scope, target_type, target_value)
);

-- Append-only, hash-chained audit trail
CREATE TABLE event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    prev_hash BLOB NOT NULL,               -- 32 bytes, SHA-256 of previous event (genesis: H(instance_node_id))
    event_type TEXT NOT NULL,              -- 'member.joined', 'grant.capability_changed', etc.
    actor BLOB,                            -- pubkey of who did it (NULL for system events)
    target BLOB,                           -- pubkey of who it happened to
    payload TEXT NOT NULL DEFAULT '{}',     -- JSON details
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    hash BLOB NOT NULL                     -- 32 bytes, H(id ++ prev_hash ++ event_type ++ actor ++ target ++ payload ++ created_at)
);

-- Signed checkpoints for tamper evidence
CREATE TABLE event_checkpoints (
    event_id INTEGER NOT NULL PRIMARY KEY, -- the event this checkpoint covers through
    chain_head_hash BLOB NOT NULL,         -- 32 bytes, hash of the event at event_id
    signature BLOB NOT NULL,               -- 64 bytes, instance NodeId signs the chain head
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (event_id) REFERENCES event_log(id)
);

CREATE INDEX idx_invites_expires ON invites(expires_at);
CREATE INDEX idx_grants_state ON member_grants(state);
CREATE INDEX idx_grants_invited_via ON member_grants(invited_via);
CREATE INDEX idx_event_log_type ON event_log(event_type);
CREATE INDEX idx_event_log_target ON event_log(target);
CREATE INDEX idx_event_log_created ON event_log(created_at);
CREATE INDEX idx_event_log_hash ON event_log(hash);
```

### 2.2 Access Rights (GNAP-Inspired)

The `access` column on `member_grants` stores the expanded access rights as a
JSON array of objects, inspired by
[GNAP (RFC 9635)](https://www.rfc-editor.org/rfc/rfc9635.html) Section 8:

```json
[
  { "type": "content", "actions": ["read"] },
  { "type": "terminals", "actions": ["read", "input"] },
  { "type": "chat", "actions": ["send"] },
  { "type": "tasks", "actions": ["read", "create", "edit"] },
  { "type": "instances", "actions": ["create"] }
]
```

Each object has a `type` (resource kind) and `actions` (permitted operations).
This is the sole authorization primitive.

Default expansion from capability:

| Capability    | Access Rights |
|---------------|---------------|
| `view`        | `content:read`, `terminals:read` |
| `collaborate` | view + `terminals:input`, `chat:send`, `tasks:read,create,edit`, `instances:create` |
| `admin`       | collaborate + `members:read,invite,suspend,reinstate,remove,update` |
| `owner`       | admin + `instance:manage,transfer` |

Admins can tweak individual access rights via `PATCH
/api/members/:public_key/access`. The `capability` field always reflects the
original preset; `access` reflects the actual enforced set (which may differ
from the preset after tweaking).

Permission checks iterate the access array looking for a matching `type` and
`action`. At this scale (4-7 objects per grant, each with 1-5 actions), this is
trivially fast — no index, no bitmask, just a linear scan.

The model is extensible: adding a new resource type or action is adding a new
object to the array. If the initial set turns out to be wrong, it can be revised
without a schema migration.

#### Capability Algebra

All access rights manipulation goes through four defined operations in
`crab_city_auth`. No code outside this module performs ad-hoc iteration over
access rights arrays.

```rust
impl AccessRights {
    /// Intersection: for scoped sessions.
    /// "what can I do with this token?" = requested ∩ granted
    fn intersect(&self, other: &AccessRights) -> AccessRights;

    /// Subset check: for authorization.
    /// "does this session allow this action?" = required ⊆ scope
    fn contains(&self, type_: &str, action: &str) -> bool;

    /// Superset check: for capability narrowing.
    /// "can this invite grant this?" = invite.cap ⊆ issuer.cap
    fn is_superset_of(&self, other: &AccessRights) -> bool;

    /// Diff: for access tweaking and audit.
    /// "what changed?" = old.diff(new) -> (added, removed)
    fn diff(&self, other: &AccessRights) -> (AccessRights, AccessRights);
}
```

Property-tested invariants:
- `intersect` is commutative: `a.intersect(b) == b.intersect(a)`
- `intersect` is idempotent: `a.intersect(a) == a`
- `intersect` narrows: `a.intersect(b).is_superset_of(c)` implies `a.is_superset_of(c) && b.is_superset_of(c)`
- Preset ordering: `Owner.access_rights().is_superset_of(Admin.access_rights())` for all adjacent presets
- Round-trip: `Capability::from_access(cap.access_rights()) == Some(cap)` for all presets

### 2.3 Instance-Side API

All authenticated user-to-instance communication goes over iroh streams. The
instance exposes no authenticated HTTP API endpoints. HTTP is used only for:
- Static asset serving (the SvelteKit app)
- Unauthenticated preview (join page activity stream)
- OIDC callbacks (enterprise SSO, M4+)
- `/metrics` (observability)

The following operations are iroh RPC messages (request/response over the iroh
stream), not HTTP endpoints:

#### `POST /api/auth/oidc/callback`

OIDC callback from crabcity.dev. Instance acts as OIDC RP. This is an HTTP
endpoint because the OIDC redirect flow requires it.

```
Query:    ?code=<auth_code>&state=<csrf_state>
Response: 302 redirect to instance UI (browser then connects via iroh WASM)
```

#### Invite Creation (iroh RPC)

Create an invite. Requires `members` access.

```
Request:  { "capability": "collaborate", "max_uses": 5, "expires_in_hours": 72 }
Response: { "token": "<base32>", "url": "https://instance/join#<base32>" }
```

#### Invite Redemption (iroh RPC)

Redeem an invite token. Sent as the first message on a new iroh connection when
the client's NodeId has no active grant. Idempotent on `(invite_nonce,
NodeId)` — if a grant already exists for this key from this invite, returns
the existing grant.

```
Request:  { "type": "RedeemInvite", "token": "<base32>", "display_name": "Alex" }
Response: { "type": "InviteRedeemed", "identity": { ... }, "grant": { ... } }
Error:    { "type": "Error", "code": "invalid_invite", ... }
```

The client's public key is implicit — it's the NodeId from the iroh handshake.
No session tokens or refresh tokens are returned; the iroh connection IS the
authenticated session.

On redemption:
1. Verify invite: walk the delegation chain root-to-leaf, verify all signatures, check capability narrowing and depth constraints
2. Verify root issuer has `members:invite` access on this instance (lookup grant)
3. Check all links: not expired, not exhausted, not revoked
4. Create `member_identities` row (or update if pubkey already known)
5. Create `member_grants` row with `state = active`, `invited_via = leaf_link.nonce`, capability from leaf link
6. Increment use count on the leaf link's nonce (stored in `invites` table)
7. Log `invite.redeemed` and `member.joined` events (payload includes full chain for auditability)
8. Send initial state snapshot over the iroh stream
9. Broadcast `MemberJoined`

#### Invite by Noun (iroh RPC)

Create an invite by noun. Requires `members:invite` access and registry
integration. Resolves the noun via the registry and either creates an immediate
invite (if resolved) or registers a pending invite.

```
Request:  { "type": "InviteByNoun", "noun": "github:foo", "capability": "collaborate" }
Response (resolved): {
    "type": "NounResolved",
    "status": "resolved",
    "invite": { "token": "<base32>", "url": "https://instance/join#<base32>" },
    "account": { "handle": "...", "fingerprint": "crab_..." }
}
Response (pending): {
    "type": "NounPending",
    "status": "pending",
    "noun": "github:foo",
    "message": "github:foo is not on crabcity yet. They'll receive the invite when they sign up."
}
```

On resolution, the instance creates a standard signed invite for the resolved
pubkey and optionally notifies the invitee via the heartbeat delivery mechanism.
On pending, the instance records the pending noun locally (for admin display)
and the registry holds the pending invite for future resolution.

#### List Pending Nouns (iroh RPC)

List pending noun invites for this instance. Requires `members:read` access.

```
Request:  { "type": "ListPendingNouns" }
Response: { "type": "PendingNouns", "pending": [{ "noun": "github:foo", "capability": "collaborate", "created_at": "...", "created_by": "crab_2K7XM9QP" }] }
```

#### Revoke Invite (iroh RPC)

Revoke an invite. Requires `members` access.

```
Request:  { "type": "RevokeInvite", "nonce": "<base64>", "suspend_derived_members": false }
Response: { "type": "InviteRevoked", "revoked": true, "members_suspended": 0 }
```

If `suspend_derived_members` is true, all grants with `invited_via = nonce` and
`state = active` are transitioned to `suspended`. Each transition produces a
`member.suspended` event.

#### List Members (iroh RPC)

List instance members. Requires `content:read` access.

```
Request:  { "type": "ListMembers" }
Response: { "type": "Members", "members": [{
    "public_key": "...",
    "fingerprint": "crab_2K7XM9QP",
    "display_name": "...",
    "handle": "@alex",
    "capability": "collaborate",
    "access": [{ "type": "content", "actions": ["read"] }, ...],
    "state": "active"
}] }
```

#### Remove Member (iroh RPC)

Remove a member. Requires `members` access. Cannot remove `owner`. Transitions
grant to `removed`.

```
Request:  { "type": "RemoveMember", "public_key": "<base64>" }
Response: { "type": "MemberRemoved", "public_key": "..." }
```

#### Update Member Capability (iroh RPC)

Update a member's capability. Requires `members` access. Cannot escalate beyond
own capability.

```
Request:  { "type": "UpdateMember", "public_key": "<base64>", "capability": "admin" }
Response: { "type": "GrantUpdated", "grant": { ... } }
```

#### Tweak Access Rights (iroh RPC)

Tweak individual access rights. Requires `members:update` access.

```
Request:  { "type": "TweakAccess", "public_key": "<base64>", "add": [{ "type": "terminals", "actions": ["input"] }], "remove": [{ "type": "chat", "actions": ["send"] }] }
Response: { "type": "AccessUpdated", "access": [...] }
```

#### Suspend Member (iroh RPC)

Suspend a member. Requires `members` access.

```
Request:  { "type": "SuspendMember", "public_key": "<base64>", "reason": "..." }
Response: { "type": "GrantUpdated", "grant": { ... } }
```

#### Reinstate Member (iroh RPC)

Reinstate a suspended member. Requires `members` access.

```
Request:  { "type": "ReinstateMember", "public_key": "<base64>" }
Response: { "type": "GrantUpdated", "grant": { ... } }
```

#### Replace Member Key (iroh RPC)

Link a new grant to an old one (key loss recovery). Requires `members` access.

```
Request:  { "type": "ReplaceMember", "public_key": "<base64>", "old_public_key": "<base64>" }
Response: { "type": "GrantUpdated", "grant": { ... } }
```

Sets `replaces = old_public_key` on the new grant, transitions old grant to
`removed`. Logs `member.replaced` event.

#### Query Events (iroh RPC)

Query event log. Requires `members` access.

```
Request:  { "type": "QueryEvents", "target": "<base64>", "event_type": "member.*", "limit": 50, "before": 123 }
Response: { "type": "Events", "events": [...], "has_more": true }
```

#### Verify Events (iroh RPC)

Verify event log integrity. Requires `members` access.

```
Request:  { "type": "VerifyEvents", "from": 1, "to": 847 }
Response: {
    "type": "EventVerification",
    "valid": true,
    "events_checked": 847,
    "chain_head": { "event_id": 847, "hash": "<hex>" },
    "checkpoints": [
        { "event_id": 100, "hash": "<hex>", "signature": "<base64>", "valid": true },
        { "event_id": 200, "hash": "<hex>", "signature": "<base64>", "valid": true }
    ]
}
```

#### Event Proof (iroh RPC)

Get an inclusion proof for a specific event. Requires `content:read` access.

```
Request:  { "type": "GetEventProof", "event_id": 42 }
Response: {
    "type": "EventProof",
    "event": { ... },
    "prev_hash": "<hex>",
    "hash": "<hex>",
    "nearest_checkpoint": { "event_id": 200, "hash": "<hex>", "signature": "<base64>" }
}
```

#### `GET /api/preview` (WebSocket)

Unauthenticated preview stream for the join page. This is the one WebSocket
endpoint that remains — it serves browsers that haven't connected via iroh yet
(they're still on the join page deciding whether to join).

```
No authentication required. Rate-limited to 5 concurrent connections per IP.

Server -> Client messages:
    { "type": "preview.activity", "data": {
        "terminal_count": 2,
        "active_cursors": [
            { "terminal_id": "...", "row": 24, "col": 80 }   // cursor position only, no content
        ],
        "user_count": 3,
        "instance_name": "Alex's Workshop",
        "uptime_secs": 84200
    }}

Sent every 2 seconds while connected. No client -> server messages accepted.
```

This is the "looking through the restaurant window" stream. It communicates
liveness and activity without leaking any instance data. Terminal content, chat
messages, task details, and user identities are never sent on this stream.

### 2.4 Auth Middleware

All authenticated requests arrive over iroh streams. The auth layer is simple:

```
1. Loopback bypass → synthetic owner identity (all-zeros pubkey, owner grant, full access)
2. iroh connection → extract NodeId from handshake → lookup grant → full grant access rights
3. No valid connection → reject
```

The middleware extracts the `NodeId` (= ed25519 pubkey) from the iroh
connection. No signature verification needed — the QUIC handshake already proved
key ownership. The grant is cached in memory from connection establishment.
Access rights are the full grant (no scoped sessions).

Revocation is immediate: when an admin suspends a user, the instance closes
their iroh connection. No revocation set, no token expiry window.

```rust
fn create_task(auth: AuthUser) -> Result<...> {
    auth.require_access("tasks", "create")?;  // checks grant via AccessRights::contains()
    // ...
}
```

Handlers are transport-unaware. `AuthUser` is populated from the iroh connection
state. The same code works for native QUIC clients and browser WASM clients
(both connect via iroh).

## 3. Registry Data Model (crabcity.dev)

### 3.1 Schema (SQLite or Postgres — SQLite is fine for the traffic level)

```sql
-- Registry accounts
CREATE TABLE accounts (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID v7
    handle TEXT NOT NULL UNIQUE,            -- lowercase, alphanumeric + hyphens
    display_name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    email TEXT,
    email_verified INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    blocked INTEGER NOT NULL DEFAULT 0,
    blocked_reason TEXT
);

-- Account keys (multi-device, day-one feature)
CREATE TABLE account_keys (
    account_id TEXT NOT NULL,
    public_key BLOB NOT NULL,              -- 32 bytes, ed25519
    label TEXT NOT NULL DEFAULT '',         -- "MacBook", "Phone", "YubiKey"
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_at TEXT,                        -- NULL if active
    PRIMARY KEY (account_id, public_key),
    FOREIGN KEY (account_id) REFERENCES accounts(id)
);

CREATE UNIQUE INDEX idx_account_keys_pubkey ON account_keys(public_key) WHERE revoked_at IS NULL;

-- OIDC bindings (enterprise SSO)
CREATE TABLE oidc_bindings (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID
    account_id TEXT NOT NULL,
    provider TEXT NOT NULL,                 -- 'okta', 'entra', 'google-workspace'
    issuer TEXT NOT NULL,                   -- https://acme.okta.com
    subject TEXT NOT NULL,                  -- IdP user ID
    org_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (account_id) REFERENCES accounts(id),
    FOREIGN KEY (org_id) REFERENCES orgs(id),
    UNIQUE (issuer, subject)
);

-- Organizations
CREATE TABLE orgs (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID
    slug TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    oidc_issuer TEXT,                       -- enterprise IdP issuer URL
    oidc_client_id TEXT,
    oidc_client_secret_encrypted BLOB,
    instance_quota INTEGER NOT NULL DEFAULT 10,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    blocked INTEGER NOT NULL DEFAULT 0
);

-- Org membership
CREATE TABLE org_members (
    org_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member',    -- 'owner', 'admin', 'member'
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (org_id, account_id),
    FOREIGN KEY (org_id) REFERENCES orgs(id),
    FOREIGN KEY (account_id) REFERENCES accounts(id)
);

-- Registered instances
CREATE TABLE instances (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID
    owner_id TEXT NOT NULL,
    node_id BLOB NOT NULL UNIQUE,          -- 32 bytes, iroh NodeId
    slug TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    visibility TEXT NOT NULL DEFAULT 'unlisted', -- 'public', 'unlisted', 'private'
    version TEXT,
    user_count INTEGER,
    last_heartbeat TEXT,
    published_at TEXT NOT NULL DEFAULT (datetime('now')),
    blocked INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (owner_id) REFERENCES accounts(id)
);

-- Instance-to-org binding (org-managed instances)
CREATE TABLE org_instances (
    org_id TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    default_capability TEXT NOT NULL DEFAULT 'collaborate',
    PRIMARY KEY (org_id, instance_id),
    FOREIGN KEY (org_id) REFERENCES orgs(id),
    FOREIGN KEY (instance_id) REFERENCES instances(id)
);

-- Global blocklist
CREATE TABLE global_blocklist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_type TEXT NOT NULL,              -- 'pubkey', 'node_id', 'ip_range'
    target_value BLOB NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    added_by TEXT NOT NULL,                 -- account UUID of admin
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    version INTEGER NOT NULL               -- monotonically increasing
);

-- Org-scoped blocklists
CREATE TABLE org_blocklist (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    org_id TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_value BLOB NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    added_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    version INTEGER NOT NULL,
    FOREIGN KEY (org_id) REFERENCES orgs(id)
);

-- Identity bindings (external identity -> account, attested by registry)
CREATE TABLE identity_bindings (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID
    account_id TEXT NOT NULL,
    provider TEXT NOT NULL,                 -- 'github', 'google', 'email'
    subject TEXT NOT NULL,                  -- username, email, etc.
    verified_at TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_at TEXT,
    FOREIGN KEY (account_id) REFERENCES accounts(id),
    UNIQUE (provider, subject)             -- one binding per external identity
);

-- Pending noun-based invites (waiting for the invitee to sign up)
CREATE TABLE pending_invites (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID
    instance_id TEXT NOT NULL,
    provider TEXT NOT NULL,                 -- 'github', 'google', 'email', 'handle'
    subject TEXT NOT NULL,                  -- the noun target (username, email, handle)
    capability TEXT NOT NULL,               -- 'view', 'collaborate', 'admin'
    created_by_fingerprint TEXT NOT NULL,   -- fingerprint of the admin who created it
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,                        -- NULL = never
    resolved_at TEXT,                       -- NULL if still pending
    resolved_account_id TEXT,              -- set when the invitee signs up and links the identity
    FOREIGN KEY (instance_id) REFERENCES instances(id),
    FOREIGN KEY (resolved_account_id) REFERENCES accounts(id)
);

-- Registry-mediated invites (short-code -> invite token)
CREATE TABLE registry_invites (
    short_code TEXT NOT NULL PRIMARY KEY,   -- 8-char alphanumeric
    instance_id TEXT NOT NULL,
    invite_token BLOB NOT NULL,            -- the full signed invite blob
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    FOREIGN KEY (instance_id) REFERENCES instances(id)
);

-- Key transparency log (Merkle tree of all key binding operations)
CREATE TABLE transparency_log (
    tree_index INTEGER PRIMARY KEY AUTOINCREMENT,
    action TEXT NOT NULL,                  -- 'key_added', 'key_revoked'
    account_id TEXT NOT NULL,
    public_key BLOB NOT NULL,             -- 32 bytes
    label TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    leaf_hash BLOB NOT NULL,              -- H(action ++ account_id ++ public_key ++ created_at)
    FOREIGN KEY (account_id) REFERENCES accounts(id)
);

-- Signed tree heads (published on every mutation)
CREATE TABLE transparency_tree_heads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tree_size INTEGER NOT NULL,
    root_hash BLOB NOT NULL,              -- 32 bytes
    signature BLOB NOT NULL,              -- 64 bytes, registry signing key
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- OIDC signing keys (for crabcity.dev as OIDC provider)
CREATE TABLE signing_keys (
    kid TEXT NOT NULL PRIMARY KEY,          -- key ID
    algorithm TEXT NOT NULL DEFAULT 'EdDSA',
    private_key_encrypted BLOB NOT NULL,
    public_key BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    active INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_accounts_handle ON accounts(handle);
CREATE INDEX idx_identity_bindings_account ON identity_bindings(account_id);
CREATE INDEX idx_identity_bindings_provider_subject ON identity_bindings(provider, subject);
CREATE INDEX idx_pending_invites_provider_subject ON pending_invites(provider, subject) WHERE resolved_at IS NULL;
CREATE INDEX idx_pending_invites_instance ON pending_invites(instance_id) WHERE resolved_at IS NULL;
CREATE INDEX idx_instances_slug ON instances(slug);
CREATE INDEX idx_instances_visibility ON instances(visibility);
CREATE INDEX idx_global_blocklist_version ON global_blocklist(version);
CREATE INDEX idx_org_blocklist_version ON org_blocklist(org_id, version);
```

### 3.2 OIDC Signing Key Management

`crabcity.dev` signs OIDC tokens with ed25519 (EdDSA over Ed25519, per RFC 8037).

- JWKS endpoint: `GET /.well-known/jwks.json` — lists all active public keys
- Key rotation: every 90 days. New key is activated, old key remains valid until its expiry (180 days from creation). This means there are always 2 active keys during rotation.
- Instances cache JWKS with a 1-hour TTL (standard OIDC practice)

### 3.3 OIDC Token Claims

When `crabcity.dev` issues an ID token to an instance:

```json
{
  "iss": "https://crabcity.dev",
  "sub": "<account-uuid>",
  "aud": "<instance-node-id-hex>",
  "iat": 1739484000,
  "exp": 1739487600,
  "public_key": "<base64-ed25519-pubkey>",
  "handle": "alex",
  "display_name": "Alex",
  "org": "acme-corp",
  "org_role": "member",
  "capability": "collaborate"
}
```

The `capability` claim is set by the org admin when they bind an instance to the
org. Instances MAY override this (downgrade only, never upgrade).

## 4. Registry API (crabcity.dev)

### 4.1 Account Endpoints

#### `POST /api/v1/accounts`

Create a registry account. Links a public key to a handle.

```
Request:  { "public_key": "<base64>", "handle": "alex", "display_name": "Alex", "proof": "<base64-signature>", "key_label": "MacBook" }
Response: { "id": "<uuid>", "handle": "alex", ... }
Error:    409 if handle taken, 400 if proof invalid
```

`proof` is the signature of `"crabcity.dev:register:<handle>"` — proves the
caller controls the private key.

Creates both an `accounts` row and an `account_keys` row (the initial key).

#### `GET /api/v1/accounts/by-handle/:handle`

Public profile lookup.

```
Response: { "id": "...", "public_keys": [{ "public_key": "...", "fingerprint": "crab_...", "label": "MacBook" }], "handle": "alex", "display_name": "Alex", "avatar_url": "...", "instances": [...] }
```

Only includes instances with `visibility = 'public'`. Includes all active (non-revoked) public keys.

#### `GET /api/v1/accounts/by-key/:public_key`

Reverse lookup: public key -> account.

```
Response: { "account_id": "...", "handle": "alex", "display_name": "Alex", "public_keys": ["<key1>", "<key2>"] }
Error:    404 if not registered
```

Instances use this to resolve display names and discover that multiple keys
belong to the same account.

### 4.2 Account Key Endpoints

#### `POST /api/v1/accounts/:id/keys`

Add a new key to an account. Authenticated with an existing key.

```
Request:  { "public_key": "<base64>", "label": "Phone", "proof": "<base64-signature>" }
Response: { "public_key": "...", "label": "Phone", "created_at": "..." }
```

`proof` is the new key signing `"crabcity.dev:add-key:<account-id>"`.

#### `DELETE /api/v1/accounts/:id/keys/:public_key`

Revoke a key. Cannot revoke the last active key.

#### `GET /api/v1/accounts/:id/keys`

List active keys for an account.

### 4.2.1 Identity Binding Endpoints

#### `POST /api/v1/accounts/:id/identity-bindings`

Link an external identity to an account. Authenticated with an existing key. The
registry verifies the binding via OAuth or email verification flow, then stores
it as an attested binding.

```
Request:  { "provider": "github", "oauth_code": "<code>" }
Response: { "provider": "github", "subject": "foo", "verified_at": "..." }
```

Supported providers: `github` (OAuth), `google` (OAuth/OIDC), `email` (verification link).

#### `GET /api/v1/accounts/:id/identity-bindings`

List identity bindings for an account.

```
Response: { "bindings": [{ "provider": "github", "subject": "foo", "verified_at": "..." }, ...] }
```

#### `DELETE /api/v1/accounts/:id/identity-bindings/:binding_id`

Revoke an identity binding.

#### `GET /api/v1/accounts/by-identity`

Resolve a noun (external identity) to an account and its active pubkeys. This is
the core noun resolution endpoint used by instances for noun-based invites.

```
Query:    ?provider=github&subject=foo
Response: {
    "account_id": "...",
    "handle": "alex",
    "public_keys": [
        { "public_key": "<base64>", "fingerprint": "crab_...", "label": "MacBook" },
        { "public_key": "<base64>", "fingerprint": "crab_...", "label": "Phone" }
    ],
    "identity_bindings": [
        { "provider": "github", "subject": "foo", "verified_at": "..." }
    ],
    "attestation": "<base64>"   // registry signs (account_id ++ provider ++ subject ++ timestamp)
}
Error:    404 if no binding exists for this provider+subject
```

The `attestation` is the registry's signed statement that "account X is bound to
provider:subject at timestamp T." Instances can verify this signature to confirm
the binding was attested by the registry, not fabricated.

### 4.3 Instance Endpoints

#### `POST /api/v1/instances`

Register an instance. Authenticated by instance's NodeId keypair.

```
Request:  { "node_id": "<hex>", "slug": "alexs-workshop", "display_name": "Alex's Workshop", "visibility": "public", "proof": "<base64-sig>" }
Response: { "id": "<uuid>", "slug": "alexs-workshop", "api_token": "<base64>" }
```

Returns an API token for subsequent heartbeats.

#### `POST /api/v1/instances/heartbeat`

Periodic health check. Authenticated by the API token from registration.

```
Request:  { "version": "0.4.2", "user_count": 7 }
Response: {
    "blocklist_version": 42,
    "blocklist_deltas": {
        "global": [
            { "action": "add", "target_type": "pubkey", "target_value": "<base64>" },
            { "action": "remove", "target_type": "pubkey", "target_value": "<base64>" }
        ],
        "org:acme-corp": [
            { "action": "add", "target_type": "pubkey", "target_value": "<base64>" }
        ]
    },
    "resolved_invites": [
        {
            "pending_invite_id": "<uuid>",
            "noun": "github:foo",
            "capability": "collaborate",
            "account": {
                "id": "...",
                "handle": "alex",
                "public_keys": [{ "public_key": "<base64>", "fingerprint": "crab_...", "label": "MacBook" }]
            },
            "attestation": "<base64>",
            "created_by_fingerprint": "crab_2K7XM9QP"
        }
    ],
    "motd": null
}
```

The instance sends its current `blocklist_version` as an `If-None-Match` header.
The registry responds with entries added since that version. Blocklist deltas
are **scoped** — separate arrays for global and each org the instance is bound
to.

#### `GET /api/v1/instances`

Public directory listing.

```
Query:    ?visibility=public&sort=last_seen&limit=50&offset=0
Response: { "instances": [...], "total": 142 }
```

#### `GET /api/v1/instances/by-slug/:slug`

Single instance lookup.

### 4.4 OIDC Endpoints

Standard OIDC provider endpoints:

```
GET  /.well-known/openid-configuration    -- OIDC discovery document
GET  /.well-known/jwks.json               -- public signing keys
GET  /oidc/authorize                      -- authorization endpoint
POST /oidc/token                          -- token endpoint
GET  /oidc/userinfo                       -- userinfo endpoint
```

### 4.5 Org Endpoints

#### `POST /api/v1/orgs`

Create an org.

#### `PATCH /api/v1/orgs/:slug`

Update org settings (OIDC config, instance quota).

#### `POST /api/v1/orgs/:slug/members`

Add a member to the org.

#### `POST /api/v1/orgs/:slug/instances`

Bind an instance to the org (sets default capability for org members).

### 4.6 Invite Endpoints

#### `POST /api/v1/invites`

Register an invite at the registry (creates short-code URL).

```
Request:  { "instance_id": "<uuid>", "invite_token": "<base32>" }
Response: { "short_code": "abc12345", "url": "https://crabcity.dev/join/abc12345" }
```

#### `GET /api/v1/invites/:short_code`

Resolve short-code to invite metadata (does NOT return the raw token until the
user authenticates/creates an account).

### 4.6.1 Noun-Based Invite Endpoints

#### `POST /api/v1/invites/by-noun`

Resolve a noun and create a pending invite if the person isn't on crabcity yet.
Called by instances on behalf of admins.

```
Request:  {
    "instance_id": "<uuid>",
    "noun": "github:foo",
    "capability": "collaborate",
    "created_by_fingerprint": "crab_2K7XM9QP"
}
Response (resolved): {
    "status": "resolved",
    "account": { "id": "...", "handle": "...", "public_keys": [...] },
    "attestation": "<base64>"
}
Response (pending): {
    "status": "pending",
    "pending_invite_id": "<uuid>",
    "message": "No account bound to github:foo. Invite will be held until they sign up."
}
Error:    400 if noun format invalid, 404 if provider not supported
```

When status is `"resolved"`, the instance creates a standard keypair-based
invite for the resolved pubkey(s). When status is `"pending"`, the registry
stores the invite and will deliver it via heartbeat when the person signs up and
links the matching identity.

#### `GET /api/v1/invites/pending`

List pending noun invites for an instance. Used for admin visibility into
outstanding invites.

```
Query:    ?instance_id=<uuid>
Response: { "pending_invites": [{ "id": "...", "provider": "github", "subject": "foo", "capability": "collaborate", "created_at": "...", "expires_at": "..." }] }
```

#### `DELETE /api/v1/invites/pending/:id`

Cancel a pending noun invite.

### 4.7 Key Transparency Endpoints

#### `GET /api/v1/transparency/tree-head`

Current signed tree head.

```
Response: {
    "tree_size": 1847,
    "root_hash": "<hex>",
    "timestamp": "<iso8601>",
    "signature": "<base64>"          // registry signing key signs (tree_size ++ root_hash ++ timestamp)
}
```

#### `GET /api/v1/transparency/proof?handle=:handle`

Audit an account's key binding history with inclusion proofs.

```
Response: {
    "account_id": "...",
    "entries": [
        { "action": "key_added", "public_key": "<base64>", "fingerprint": "crab_...", "label": "MacBook", "timestamp": "...", "tree_index": 42 },
        { "action": "key_added", "public_key": "<base64>", "fingerprint": "crab_...", "label": "Phone", "timestamp": "...", "tree_index": 97 }
    ],
    "inclusion_proofs": [
        { "tree_index": 42, "proof_hashes": ["<hex>", "<hex>", ...] },
        { "tree_index": 97, "proof_hashes": ["<hex>", "<hex>", ...] }
    ],
    "tree_head": { "tree_size": 1847, "root_hash": "<hex>", "signature": "<base64>" }
}
```

#### `GET /api/v1/transparency/entries?start=N&end=M`

Raw log entries for monitors. Paginated.

```
Response: {
    "entries": [
        { "index": 42, "action": "key_added", "account_id": "...", "public_key": "<base64>", "timestamp": "..." },
        ...
    ],
    "tree_head": { ... }
}
```

Monitors (instances, public auditors) poll this endpoint to watch for
unauthorized key bindings. An instance can run a background task that
periodically fetches new entries and alerts if any of its members' registry
accounts have unexpected key additions.

### 4.8 Blocklist Endpoints

#### `GET /api/v1/blocklist`

Full global blocklist (for initial sync).

#### `GET /api/v1/blocklist/delta?since_version=N`

Delta since version N.

#### `POST /api/v1/orgs/:slug/blocklist`

Add org-scoped blocklist entry. Org admin only.

#### `GET /api/v1/orgs/:slug/blocklist`

Full org blocklist.

#### `GET /api/v1/orgs/:slug/blocklist/delta?since_version=N`

Org blocklist delta.

## 5. Client Authentication Flows

### 5.1 Flow A: Raw Invite (No Registry)

```
1. User receives invite URL: https://instance.example/join#<base32>
2. SvelteKit frontend extracts token from fragment
3. Join page renders: instance name, inviter name, capability, "Your name" input
4. If user has no keypair: generate ed25519 keypair (= iroh NodeId), store in IndexedDB
5. KEY BACKUP MODAL: blocking modal, copy/download key, "I saved my key" checkbox
6. iroh WASM connect to instance via embedded relay (ws://localhost:<port>/relay)
7. Send RedeemInvite { token, display_name } over iroh stream
8. Instance verifies invite, creates identity + grant, sends initial state snapshot
9. User's presence appears to all connected clients (MemberJoined broadcast)
```

### 5.2 Flow B: Registry Invite

```
1. User receives URL: https://crabcity.dev/join/abc12345
2. If user has no crabcity.dev account:
   a. Generate ed25519 keypair (= iroh NodeId), or import existing
   b. KEY BACKUP MODAL
   c. POST /api/v1/accounts { public_key, handle, proof, key_label }
3. Registry resolves short_code -> invite token + instance connection info (NodeId, relay URL)
4. Browser: iroh WASM connect to instance via relay
5. Send RedeemInvite { token, display_name } over iroh stream
6. Instance verifies invite, creates identity + grant, resolves handle via registry
7. Instance sends initial state snapshot
```

### 5.3 Flow C: OIDC SSO (Enterprise)

```
1. User navigates to instance, clicks "Sign in with Crab City"
2. Instance redirects to crabcity.dev/oidc/authorize
3. crabcity.dev checks if user has an active session
4. If user's org has OIDC configured:
   a. Redirect to enterprise IdP (Okta/Entra)
   b. User authenticates with corporate credentials
   c. Enterprise IdP redirects back to crabcity.dev with auth code
   d. crabcity.dev exchanges code for IdP tokens
   e. crabcity.dev maps IdP subject -> account (auto-provisioning if first login)
5. crabcity.dev issues its own OIDC id_token with crab city claims
6. Redirect back to instance with auth code
7. Instance exchanges code for id_token via HTTP callback (POST /api/auth/oidc/callback)
8. Instance extracts public_key, handle, org, capability from claims
9. Instance creates/updates identity + grant
10. Browser: 302 redirect to SvelteKit app, which connects via iroh WASM
11. iroh handshake authenticates the user (NodeId matches the OIDC-provisioned grant)
```

### 5.4 Flow D: CLI/TUI Authentication (iroh)

Native clients use iroh QUIC directly. The ed25519 keypair IS the iroh NodeId,
so authentication is implicit in the connection.

```
First run (no identity):
1. $ crabcity connect instance.example.com
2. No keypair at ~/.config/crabcity/identity.key
3. Generate keypair, save to identity.key (mode 0600)
4. Print: "Your identity: crab_2K7XM9QP (saved to ~/.config/crabcity/identity.key)"
5. Prompt: "This instance requires an invitation. Enter invite code:"
6. User pastes base32 token
7. iroh connect to instance, redeem invite over iroh stream
8. Instance creates identity + grant, sends initial state snapshot
9. Connected.

Subsequent connections:
1. Read keypair from ~/.config/crabcity/identity.key
2. iroh connect to instance (handshake proves key ownership)
3. Instance verifies NodeId has active grant
4. Instance sends initial state snapshot
5. Print: "Connected to Alex's Workshop (3 users online)"

Multi-instance:
1. Read keypair from ~/.config/crabcity/identity.key
2. iroh connect to all configured instances in parallel
3. One instance is "active" (receives input, shown in UI)
4. Background instances receive presence/chat/notifications
5. User switches active instance via keybinding (Ctrl+1/2/3...)
```

No session tokens, no refresh tokens, no token expiry. The iroh connection IS
the authenticated session. Disconnecting ends it. Reconnecting re-authenticates
via handshake.

Loopback bypass (existing) still works for local instances.

### 5.5 Flow E: Noun-Based Invite

```
Happy path (invitee already on crabcity):
1. Admin types: /invite github:foo collaborate
2. Instance: POST /api/invites/by-noun { noun: "github:foo", capability: "collaborate" }
3. Instance -> Registry: POST /api/v1/invites/by-noun (resolve noun)
4. Registry: lookup identity_bindings(provider=github, subject=foo) -> account + pubkeys
5. Registry: return { status: "resolved", account, pubkeys, attestation }
6. Instance: verify attestation signature, create signed invite for resolved pubkey
7. Instance: store invite, log invite.created event
8. Admin sees: "Invite created for github:foo (@alex, crab_2K7XM9QP)"
9. Next heartbeat: registry includes resolved invite in response to invitee's instance(s)
10. Invitee sees pending invite notification in their UI

Pending path (invitee not yet on crabcity):
1. Admin types: /invite github:foo collaborate
2. Instance -> Registry: POST /api/v1/invites/by-noun (resolve noun)
3. Registry: lookup identity_bindings(provider=github, subject=foo) -> 404
4. Registry: create pending_invites row, return { status: "pending" }
5. Instance: store pending noun locally for admin display
6. Admin sees: "github:foo is not on crabcity yet. Invite will be delivered when they sign up."
7. ... time passes ...
8. foo signs up at crabcity.dev, links their GitHub account (OAuth flow)
9. Registry: POST-registration hook checks pending_invites for (github, foo) -> match found
10. Registry: resolve pending invite, set resolved_account_id
11. Next heartbeat to the inviting instance: includes resolved_invites entry
12. Instance: receives resolved invite, creates standard signed invite for foo's pubkey
13. Instance: broadcasts notification, admin sees "github:foo has joined crabcity — invite ready"

Key loss recovery path:
1. Blake lost their key, contacts admin out-of-band
2. Blake registers new keys at crabcity.dev (same account, new device)
3. Admin types: /invite @blake collaborate
4. Registry resolves @blake -> account -> NEW pubkeys
5. Instance creates invite for the new pubkey
6. Blake redeems invite, gets new grant
7. Admin links new grant to old: POST /api/members/:new_pk/replace { old_public_key }
8. Old grant -> removed, attribution merged
```

## 6. Connection Lifecycle

There are no session tokens, refresh tokens, or revocation sets. The iroh
connection IS the session for all clients — native and browser.

### 6.1 Connection States

```
disconnected  → connecting    via: client initiates iroh connect
connecting    → connected     via: iroh handshake completes, grant lookup succeeds
connecting    → rejected      via: no active grant, or blocklisted
connected     → disconnected  via: client disconnects, network loss, or server close
```

### 6.2 Immediate Revocation

When an admin suspends a user, the instance closes their iroh connection. The
client receives a `ConnectionClosed { reason: "suspended" }` message before
disconnection. There is no revocation window — the connection is closed
immediately.

For browser clients connected via the embedded relay, the same mechanism
applies: the relay is in-process, so closing the iroh connection closes the
underlying WebSocket transport.

### 6.3 Reconnection

On reconnect, the client re-establishes the iroh connection (handshake =
re-authentication) and sends `last_seq` in the initial message. The server
replays from the ring buffer or sends a full snapshot (see section 9.2).

### 6.4 Cleanup

The instance maintains a `NodeId -> connection` map. When a connection drops,
the entry is removed and a presence update is broadcast. No background cleanup
tasks needed — connection state is ephemeral.

## 7. Key Recovery and Multi-Device

### 7.1 Multiple Keys Per Account

A registry account has multiple public keys from day one (see `account_keys`
table in section 3.1).

When a user adds a new device, they authenticate with an existing key and
register the new one. Instances learn about the key->account mapping via handle
resolution.

When resolving a public key via the registry, the response includes the
canonical account ID and all active keys. The instance can recognize that two
different keys belong to the same logical user:

```json
{
  "account_id": "...",
  "handle": "alex",
  "public_keys": [
    { "public_key": "<key1>", "label": "MacBook" },
    { "public_key": "<key2>", "label": "Phone" }
  ]
}
```

The instance stores the `registry_account_id` on `member_identities`. Different
keys for the same account share identity metadata (display name, handle) but
have separate grants (allowing different capabilities per device if desired).

### 7.2 Account Recovery

For keypair-only users (no registry): the instance admin uses the re-invite +
replace flow (see architecture doc, "Key Loss Recovery"). Old contributions are
attributed to the new key via the `replaces` link.

For registry users: if they've set an email, they can verify ownership and
register a new key. This is a high-security operation (email verification + rate
limiting + cooldown period).

There is no "forgot password" flow because there are no passwords.

## 8. Rate Limiting and Abuse Prevention

Despite "almost no traffic," certain operations need basic protection:

| Operation | Limit | Window |
|-----------|-------|--------|
| iroh connect (new) | 10 | per minute per IP |
| `RedeemInvite` | 5 | per minute per NodeId |
| `POST /api/v1/accounts` | 3 | per hour per IP |
| `POST /api/v1/accounts/:id/keys` | 5 | per hour per account |
| `POST /api/v1/instances/heartbeat` | 15 | per minute per instance |
| iroh connections | 10 | concurrent per NodeId |
| WebSocket `/api/preview` | 5 | concurrent connections per IP |
| iroh reconnect | 30 | per minute per NodeId |

Implemented as in-memory token buckets. No Redis needed. Counters reset on
restart (acceptable at this traffic level).

## 9. Wire Formats

### 9.1 HTTP API

All API communication uses JSON over HTTPS. Content-Type: `application/json`.

Public keys are encoded as unpadded base64 (URL-safe variant) in JSON payloads.

Invite tokens are encoded as Crockford base32 (no padding, case-insensitive) for
human-friendly sharing.

Signatures are encoded as unpadded base64 (URL-safe variant) in JSON payloads.

UUIDs are v7 (time-ordered) for database locality.

### 9.2 Message Protocol

All messages use envelope versioning with monotonic sequence numbers over iroh
QUIC streams:

```json
{ "v": 1, "seq": 4817, "type": "GrantUpdate", "data": { ... } }
```

The `seq` field is a per-connection monotonic counter assigned by the server.
Clients track their last-seen `seq` for reconnection/resumption.

**Framing:** Length-prefixed JSON (4-byte big-endian length + JSON bytes) over
iroh QUIC streams. Both native clients and browser WASM clients use the same
framing — the iroh library handles the transport differences (raw QUIC vs
WebSocket-to-relay). Future: msgpack for efficiency.

Server -> Client message types (auth-related):

```
GrantUpdate       { grant }           -- member capability/state changed
IdentityUpdate    { identity }        -- member display name/handle/avatar changed
MemberJoined      { identity, grant } -- new member added
MemberRemoved     { public_key }      -- member removed
ConnectionClosed  { reason }          -- server is closing the connection (suspended, blocklisted, etc.)
```

These are in addition to the existing `StateChange`, `TaskUpdate`,
`InstanceList`, `Focus`, etc.

Unauthenticated preview stream (`/api/preview`, WebSocket — the only non-iroh
endpoint):

```
PreviewActivity    { terminal_count, active_cursors, user_count, instance_name, uptime_secs }
```

Clients MUST ignore unknown message types and unknown versions. This allows the
protocol to evolve without breaking existing clients.

#### Reconnection

QUIC connection migration handles most network transitions (WiFi-to-cellular,
IP changes) transparently. If the connection is truly lost, the client
re-establishes the iroh connection (handshake = re-authentication) and sends
`last_seq` in the initial stream message. The server replays from a bounded ring
buffer (last 1000 messages or last 5 minutes, whichever is smaller). If the gap
is too large, the server sends a full state snapshot instead.

```
Client                              Instance
  |  iroh connect                    |
  |  { last_seq: 4817 }             |
  | -------------------------------->|
  |                                  |  check ring buffer
  |                                  |  4817 is within buffer
  |  { v:1, seq:4818, type:... }    |
  |  { v:1, seq:4819, type:... }    |
  |  ...                             |
  |  { v:1, seq:4825, type:... }    |  replay complete
  | <--------------------------------|
  |                                  |  resume live stream
```

If `last_seq` is too old or not provided:

```
  |  { v:1, seq:0, type:"Snapshot", |
  |    data: { full state } }       |  full state snapshot
  | <--------------------------------|
  |                                  |  resume live stream
```

#### Heartbeat Pings

The iroh connection uses QUIC keepalive (30-second interval). If the client
doesn't respond within 10 seconds, the server closes the connection and removes
the user from presence. This prevents ghost users who appear online after
disconnecting.

## 10. Protocol Reference

### 10.1 Membership State Transitions

```
invited   -> active      via: invite redemption over iroh stream (RedeemInvite)
invited   -> removed     via: invite expired before redemption, or admin action
active    -> suspended   via: admin SuspendMember, or blocklist hit (iroh connection closed)
active    -> removed     via: admin RemoveMember (iroh connection closed)
suspended -> active      via: admin ReinstateMember
suspended -> removed     via: admin RemoveMember
removed   -> (terminal)  no transitions out of removed
```

### 10.2 Event Types

| Event Type | Actor | Target | Payload |
|------------|-------|--------|---------|
| `member.joined` | redeemer pubkey | redeemer pubkey | `{ invite_nonce, capability }` |
| `member.suspended` | admin pubkey | target pubkey | `{ reason, source: "admin"\|"blocklist" }` |
| `member.reinstated` | admin pubkey | target pubkey | `{}` |
| `member.removed` | admin pubkey | target pubkey | `{}` |
| `member.replaced` | admin pubkey | new pubkey | `{ old_public_key }` |
| `grant.capability_changed` | admin pubkey | target pubkey | `{ old, new }` |
| `grant.access_changed` | admin pubkey | target pubkey | `{ added: [...], removed: [...] }` |
| `invite.created` | issuer pubkey | null | `{ nonce, capability, max_uses }` |
| `invite.redeemed` | redeemer pubkey | null | `{ nonce }` |
| `invite.revoked` | admin pubkey | null | `{ nonce, suspend_derived: bool }` |
| `invite.noun_created` | admin pubkey | null | `{ noun, capability, status: "resolved"\|"pending" }` |
| `invite.noun_resolved` | system | null | `{ noun, account_id, pending_invite_id }` |
| `identity.updated` | user pubkey | user pubkey | `{ fields_changed: [] }` |

### 10.3 Error Codes

Every error response includes a machine-actionable `recovery` field. Clients
never have to guess what to do next. Recovery actions are a closed enum:
`reconnect`, `retry`, `contact_admin`, `redeem_invite`, `none`.

Errors on the iroh stream use the same JSON envelope:

```json
{ "v": 1, "seq": N, "type": "Error", "data": { "code": "...", "message": "...", "recovery": { ... } } }
```

| Code | Meaning | Recovery |
|------|---------|----------|
| `invalid_invite` | Invite expired, exhausted, or malformed | `{ "action": "none" }` |
| `not_a_member` | No grant exists for this NodeId | `{ "action": "redeem_invite" }` |
| `grant_not_active` | Grant exists but state != active | `{ "action": "contact_admin", "admin_fingerprints": [...], "reason": "..." }` |
| `insufficient_access` | Missing required access right | `{ "action": "none", "required": { "type": "...", "action": "..." } }` |
| `blocklisted` | NodeId is on a blocklist | `{ "action": "contact_admin", "reason": "..." }` |
| `already_a_member` | NodeId already has an active grant | `{ "action": "reconnect" }` |
| `rate_limited` | Too many requests | `{ "action": "retry", "retry_after_secs": N }` |

For connection-level errors (`not_a_member`, `grant_not_active`, `blocklisted`),
the server sends the error and then closes the iroh connection. For
request-level errors (`insufficient_access`, `invalid_invite`), the connection
remains open.

Registry HTTP endpoints (section 4) use standard HTTP status codes. Only
instance-side communication uses this iroh error format.

Example:

```json
{
    "v": 1, "seq": 3, "type": "Error",
    "data": {
        "code": "grant_not_active",
        "message": "Your membership is suspended",
        "recovery": {
            "action": "contact_admin",
            "admin_fingerprints": ["crab_2K7XM9QP", "crab_9F4YN2RZ"],
            "reason": "Blocklist match (org:acme-corp)"
        }
    }
}
```

## 11. Cross-Instance Identity Proofs

Self-issued identity proofs link a user's identities across instances without
requiring the registry:

```
IdentityProof = {
    version: u8,                    // 0x01
    subject: [u8; 32],             // the key doing the proving
    instance: [u8; 32],            // which instance this identity lives on (NodeId)
    related_keys: Vec<[u8; 32]>,   // other keys belonging to the same person
    registry_handle: Option<String>, // optional, for display
    timestamp: u64,                 // unix timestamp
    signature: [u8; 64],           // subject signs all fields
}
```

Verification: check `signature` over all fields using `subject` as the public
key. The proof asserts "I, `subject`, claim that `related_keys` also belong to
me." The consuming instance decides how much weight to give this claim.

Identity proofs enable:
- **Cross-instance reputation**: "This user is a trusted admin on 3 other
  instances"
- **Portable identity display**: Instance A shows that a user is also `@alex` on
  Instance B without asking the registry
- **Offline federation**: Identity linkage works when the registry is
  unreachable

Identity proofs are **assertions, not guarantees**. They prove the subject
*claimed* the linkage (signature is valid), but not that the subject actually
has an active grant on the claimed instance. Full verification requires
contacting the instance or registry.

Proofs are exchanged during iroh connection establishment (client sends its
proofs as part of the initial message) and cached locally. They are refreshed
when the registry resolves a handle or when the user explicitly re-proves.

## 12. Invite QR Codes

A flat invite is 160 bytes (256 chars Crockford base32) — well within QR code
alphanumeric capacity (4296 chars). Delegated invites (3-hop, 660 chars) also
fit.

The invite creation response includes a `qr_data` field:

```
Response: {
    "token": "<base32>",
    "url": "https://instance/join#<base32>",
    "qr_data": "<base32>"          // alphanumeric payload optimized for QR encoding
}
```

Rendering:
- **TUI**: Unicode half-block characters (`▀▄█ `) — no external dependencies,
  works in any terminal
- **Web UI**: SVG generation (client-side, no server round-trip)
- **CLI**: `crabcity invite --qr` prints the QR code to stdout

The URL-based flow remains the default. QR is an additional distribution
channel, not a replacement.

## 13. Idempotency

Every mutation handles the "request succeeded, response lost, client retries"
failure mode. Since all mutations go over iroh streams, the iroh connection
provides reliable delivery for most cases. Idempotency matters for reconnect
scenarios where the client replays its last unacknowledged request:

| Operation | Idempotency key | Behavior on retry |
|-----------|----------------|-------------------|
| `CreateInvite` | Client-supplied `idempotency_key` field | Returns existing invite if key matches |
| `RedeemInvite` | `(invite_nonce, NodeId)` | Returns existing grant if already redeemed by this NodeId |
| `UpdateMember` | Last-write-wins | Capability change is idempotent by nature |
| `SuspendMember` | State check | No-op if already suspended |
| `ReinstateMember` | State check | No-op if already active |

Event logging is serialized within SQLite transactions: `BEGIN → read prev_hash
→ compute new hash → INSERT → COMMIT`. The hash chain requires sequential
writes; concurrent event inserts are serialized by the database.

## 14. Observability

### 14.1 Metrics (Prometheus)

Every instance and registry exposes `GET /metrics`:

**iroh Connections:**
- `crabcity_iroh_connections_active` (gauge) — all connected clients
- `crabcity_iroh_connections_total{transport}` — cumulative connections
  (`quic` = native, `relay` = browser via embedded relay)
- `crabcity_iroh_reconnections_total` — connection re-establishments
- `crabcity_iroh_auth_rejections_total{reason}` — `no_grant`, `suspended`,
  `blocklisted`
- `crabcity_replay_messages_total` — messages replayed on reconnect
- `crabcity_snapshots_total` — full state snapshots sent (reconnect gap too
  large)

**Membership:**
- `crabcity_grants_by_state{state}` (gauge) — invited, active, suspended,
  removed
- `crabcity_invites_redeemed_total{capability}`
- `crabcity_invites_active` (gauge) — unexpired, unexhausted invites
- `crabcity_noun_invites_total{status}` — resolved, pending
- `crabcity_noun_invites_pending` (gauge) — pending noun invites awaiting
  resolution

**Registry (instance-side):**
- `crabcity_registry_heartbeat_latency_seconds` (histogram)
- `crabcity_registry_heartbeat_failures_total`
- `crabcity_blocklist_sync_version{scope}` (gauge)

**Event log:**
- `crabcity_event_log_size` (gauge)
- `crabcity_event_log_append_latency_seconds` (histogram)

### 14.2 Structured Logging

Every auth decision (connection accepted or rejected) emits a structured log
line with fields: `node_id_fingerprint`, `result`, `reason`, `grant_capability`,
`transport` (`quic` or `relay`). This is the forensic trail for investigating
auth issues.

Every state transition emits a structured log line with: `event_type`,
`actor_fingerprint`, `target_fingerprint`, `payload_summary`.

### 14.3 Distributed Tracing

OpenTelemetry trace context propagation on the registry HTTP client links spans
across instance → registry → instance flows. Traces cover: handle resolution,
heartbeat round-trips, OIDC token exchange, and invite short-code resolution.
