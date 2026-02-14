# Crab City Accounts: Design

## Overview

This document specifies the detailed technical design for the Crab City account system. It covers data models, API contracts, cryptographic protocols, and the interaction patterns between instances, the registry, and clients.

## 1. Cryptographic Primitives

### 1.1 Key Generation

All keypairs are ed25519 (the same curve used by iroh's `NodeId`). Keys are generated using `ed25519-dalek` (already a transitive dependency via iroh).

Client-side key generation (browser):
- Use Web Crypto API (`Ed25519` via the `SubtleCrypto` interface)
- Store private key in IndexedDB, encrypted with a user-chosen passphrase via AES-256-GCM (PBKDF2-derived key)
- Export: PKCS8 (private), raw 32-byte (public)

Client-side key generation (CLI/TUI):
- Generate via `ed25519_dalek::SigningKey::generate(&mut OsRng)`
- Store in `~/.config/crabcity/identity.key` (mode 0600)

### 1.2 Key Fingerprints

Human-readable short identifiers for public keys.

Format: `crab_` + first 8 characters of Crockford base32 encoding of the 32-byte public key.

Example: `crab_2K7XM9QP`

Properties:
- 40 bits of entropy — sufficient to distinguish members within any realistic instance
- Case-insensitive (Crockford base32)
- Used in TUI display, logs, admin UIs, CLI output
- Never used for lookups or authentication — display only
- Defined in `crab_city_auth` crate: `PublicKey::fingerprint() -> String`

### 1.3 Challenge-Response Authentication

When a user authenticates to an instance with their keypair:

```
Client                              Instance
  |                                    |
  |  POST /api/auth/challenge          |
  |  { public_key, timestamp }         |
  | ---------------------------------->|
  |                                    |  generate 32-byte random nonce
  |                                    |  store in-memory (nonce, pubkey, expires=60s)
  |  { nonce }                         |
  | <----------------------------------|
  |                                    |
  |  sign("crabcity:auth:v1:"         |
  |    ++ nonce                        |
  |    ++ instance_node_id             |
  |    ++ client_timestamp)            |
  |                                    |
  |  POST /api/auth/verify             |
  |  { public_key, nonce,              |
  |    signature, timestamp }          |
  | ---------------------------------->|
  |                                    |  verify signature
  |                                    |  check timestamp +-30s
  |                                    |  check grant state == active
  |                                    |  create session
  |  { session_token, expires_at }     |
  | <----------------------------------|
```

The signed payload is structured and self-documenting:
- `crabcity:auth:v1:` prefix prevents cross-protocol confusion if keypairs sign other things
- `nonce` prevents replay of the same challenge (single-use, 60s expiry)
- `instance_node_id` prevents cross-instance replay
- `client_timestamp` narrows the replay window (server checks +-30s)

**Pending challenge storage:** In-memory only (`DashMap<Nonce, PendingChallenge>`), not SQLite. Challenges are ephemeral — single-use, 60-second TTL, swept lazily on access. Note: multi-process deployments require sticky sessions or a shared store for pending challenges.

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

Delegated invite (3-hop chain): 1 + 32 + 1 + (126 * 3) = **412 bytes** (660 chars base32)

Verification (delegation chain):
1. Root link: verify `signature` over `H(0x00*32) ++ instance ++ fields`; root issuer must have `members:invite` access on the instance
2. Each subsequent link: verify `signature` over `H(prev_link) ++ instance ++ fields`
3. Each link's `capability` must be <= previous link's `capability`
4. Each link's `max_depth` must be < previous link's `max_depth`
5. All links must be unexpired and within use limits

A flat invite is a chain of length 1 with `max_depth = 0`.

URL format:
```
https://<instance-host>/join#<base32-token>
```

Fragment (`#`) ensures the token never appears in server access logs or referrer headers. The SvelteKit frontend extracts it client-side.

Registry-mediated URL format:
```
https://crabcity.dev/join/<short-code>
```

Where `short-code` is an 8-character random alphanumeric ID that maps to a stored invite in the registry database.

### 1.5 Loopback Identity

The loopback identity is a well-known sentinel public key: 32 zero bytes (`0x00 * 32`).

Rules:
- Instances reject this pubkey on any non-loopback connection (challenge-response, invite redemption, OIDC)
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

-- Active sessions (scoped: session access <= grant access)
CREATE TABLE sessions (
    token_hash BLOB NOT NULL PRIMARY KEY,  -- SHA-256 of 32-byte random token
    public_key BLOB NOT NULL,              -- 32 bytes
    scope TEXT NOT NULL DEFAULT '[]',      -- JSON array, intersection of requested access and grant access rights
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (public_key) REFERENCES member_identities(public_key)
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

CREATE INDEX idx_sessions_expires ON sessions(expires_at);
CREATE INDEX idx_sessions_pubkey ON sessions(public_key);
CREATE INDEX idx_invites_expires ON invites(expires_at);
CREATE INDEX idx_grants_state ON member_grants(state);
CREATE INDEX idx_grants_invited_via ON member_grants(invited_via);
CREATE INDEX idx_event_log_type ON event_log(event_type);
CREATE INDEX idx_event_log_target ON event_log(target);
CREATE INDEX idx_event_log_created ON event_log(created_at);
CREATE INDEX idx_event_log_hash ON event_log(hash);
```

### 2.2 Access Rights (GNAP-Inspired)

The `access` column on `member_grants` stores the expanded access rights as a JSON array of objects, inspired by [GNAP (RFC 9635)](https://www.rfc-editor.org/rfc/rfc9635.html) Section 8:

```json
[
  { "type": "content", "actions": ["read"] },
  { "type": "terminals", "actions": ["read", "input"] },
  { "type": "chat", "actions": ["send"] },
  { "type": "tasks", "actions": ["read", "create", "edit"] },
  { "type": "instances", "actions": ["create"] }
]
```

Each object has a `type` (resource kind) and `actions` (permitted operations). This is the sole authorization primitive.

Default expansion from capability:

| Capability    | Access Rights |
|---------------|---------------|
| `view`        | `content:read`, `terminals:read` |
| `collaborate` | view + `terminals:input`, `chat:send`, `tasks:read,create,edit`, `instances:create` |
| `admin`       | collaborate + `members:read,invite,suspend,reinstate,remove,update` |
| `owner`       | admin + `instance:manage,transfer` |

Admins can tweak individual access rights via `PATCH /api/members/:public_key/access`. The `capability` field always reflects the original preset; `access` reflects the actual enforced set (which may differ from the preset after tweaking).

Permission checks iterate the access array looking for a matching `type` and `action`. At this scale (4-7 objects per grant, each with 1-5 actions), this is trivially fast — no index, no bitmask, just a linear scan.

The model is extensible: adding a new resource type or action is adding a new object to the array. If the initial set turns out to be wrong, it can be revised without a schema migration.

### 2.3 Instance-Side API

#### `POST /api/auth/challenge`

Request identity challenge. Optionally request a scoped session.

```
Request:  { "public_key": "<base64>", "timestamp": "<iso8601>", "scope": [{ "type": "content", "actions": ["read"] }, { "type": "chat", "actions": ["send"] }] }
Response: { "nonce": "<base64>", "expires_at": "<iso8601>" }
```

`scope` is optional. If omitted, the session will have the full access rights of the underlying grant. If provided, the session scope is the intersection of the requested scope and the grant's access rights. This implements the principle of least privilege: a CLI tool that only reads tasks can request a `content:read`-only session, limiting blast radius if the token leaks.

#### `POST /api/auth/verify`

Complete challenge-response, get scoped session.

```
Request:  { "public_key": "<base64>", "nonce": "<base64>", "signature": "<base64>", "timestamp": "<iso8601>" }
Response: { "session_token": "<base64>", "expires_at": "<iso8601>", "capability": "collaborate", "access": [...], "scope": [...] }
Error:    401 if signature invalid, 403 if no grant or state != active
```

`scope` in the response is the actual enforced access rights for this session (may be a subset of `access` if a scope was requested during challenge).

#### `POST /api/auth/oidc/callback`

OIDC callback from crabcity.dev. Instance acts as OIDC RP.

```
Query:    ?code=<auth_code>&state=<csrf_state>
Response: 302 redirect to instance UI with session cookie set
```

#### `POST /api/invites`

Create an invite. Requires `members` access.

```
Request:  { "capability": "collaborate", "max_uses": 5, "expires_in_hours": 72 }
Response: { "token": "<base32>", "url": "https://instance/join#<base32>" }
```

#### `POST /api/invites/redeem`

Redeem an invite token.

```
Request:  { "token": "<base32>", "public_key": "<base64>", "display_name": "Alex" }
Response: { "identity": { ... }, "grant": { ... }, "session_token": "<base64>" }
Error:    400 if expired/exhausted, 403 if issuer revoked or blocklisted
```

On redemption:
1. Verify invite: walk the delegation chain root-to-leaf, verify all signatures, check capability narrowing and depth constraints
2. Verify root issuer has `members:invite` access on this instance (lookup grant)
3. Check all links: not expired, not exhausted, not revoked
4. Create `member_identities` row (or update if pubkey already known)
5. Create `member_grants` row with `state = active`, `invited_via = leaf_link.nonce`, capability from leaf link
6. Increment use count on the leaf link's nonce (stored in `invites` table)
7. Log `invite.redeemed` and `member.joined` events (payload includes full chain for auditability)
8. Create session (scoped to full grant access rights by default)
9. Broadcast `MemberJoined`

#### `POST /api/invites/revoke`

Revoke an invite. Requires `members` access.

```
Request:  { "nonce": "<base64>", "suspend_derived_members": false }
Response: { "revoked": true, "members_suspended": 0 }
```

If `suspend_derived_members` is true, all grants with `invited_via = nonce` and `state = active` are transitioned to `suspended`. Each transition produces a `member.suspended` event.

#### `GET /api/members`

List instance members. Requires `content:read` access.

```
Response: { "members": [{
    "public_key": "...",
    "fingerprint": "crab_2K7XM9QP",
    "display_name": "...",
    "handle": "@alex",
    "capability": "collaborate",
    "access": [{ "type": "content", "actions": ["read"] }, ...],
    "state": "active"
}] }
```

#### `DELETE /api/members/:public_key`

Remove a member. Requires `members` access. Cannot remove `owner`. Transitions grant to `removed`.

#### `PATCH /api/members/:public_key`

Update a member's capability. Requires `members` access. Cannot escalate beyond own capability.

```
Request:  { "capability": "admin" }
Response: { "grant": { ... } }
```

#### `PATCH /api/members/:public_key/access`

Tweak individual access rights. Requires `members:update` access.

```
Request:  { "add": [{ "type": "terminals", "actions": ["input"] }], "remove": [{ "type": "chat", "actions": ["send"] }] }
Response: { "access": [...] }
```

#### `POST /api/members/:public_key/suspend`

Suspend a member. Requires `members` access.

```
Request:  { "reason": "..." }
Response: { "grant": { ... } }
```

#### `POST /api/members/:public_key/reinstate`

Reinstate a suspended member. Requires `members` access.

#### `POST /api/members/:public_key/replace`

Link a new grant to an old one (key loss recovery). Requires `members` access.

```
Request:  { "old_public_key": "<base64>" }
Response: { "grant": { ... } }
```

Sets `replaces = old_public_key` on the new grant, transitions old grant to `removed`. Logs `member.replaced` event.

#### `GET /api/events`

Query event log. Requires `members` access.

```
Query:    ?target=<base64>&event_type=member.*&limit=50&before=<id>
Response: { "events": [...], "has_more": true }
```

#### `GET /api/events/verify`

Verify event log integrity. Requires `members` access.

```
Query:    ?from=<id>&to=<id>
Response: {
    "valid": true,
    "events_checked": 847,
    "chain_head": { "event_id": 847, "hash": "<hex>" },
    "checkpoints": [
        { "event_id": 100, "hash": "<hex>", "signature": "<base64>", "valid": true },
        { "event_id": 200, "hash": "<hex>", "signature": "<base64>", "valid": true }
    ]
}
Error:    409 if chain is broken (includes the break point)
```

#### `GET /api/events/proof/:event_id`

Get an inclusion proof for a specific event. Requires `content:read` access.

```
Response: {
    "event": { ... },
    "prev_hash": "<hex>",
    "hash": "<hex>",
    "nearest_checkpoint": { "event_id": 200, "hash": "<hex>", "signature": "<base64>" }
}
```

#### `WebSocket /api/preview`

Unauthenticated preview stream for the join page. Provides activity signals without content.

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

This is the "looking through the restaurant window" stream. It communicates liveness and activity without leaking any instance data. Terminal content, chat messages, task details, and user identities are never sent on this stream.

### 2.4 Auth Middleware Changes

The existing auth middleware gains a new check in the chain:

```
1. Loopback bypass → synthetic owner identity (all-zeros pubkey)
2. Session token in Authorization header → hash, lookup sessions table → check grant state == active
3. Cookie-based session (for browser clients) → same lookup
4. No credentials → 401
```

Session tokens are 32 random bytes, base64-encoded, stored hashed (SHA-256) in the sessions table. Default TTL: 24 hours, configurable.

The middleware extracts the identity, the grant, and the **session scope** into the request context. Access checks use the session scope (not the grant access directly), enforcing least privilege:

```rust
// In a handler:
fn create_task(auth: AuthUser) -> Result<...> {
    auth.require_access("tasks", "create")?;  // checks session.scope, not grant.access
    // ...
}
```

This means a session requested with `scope: [{ "type": "content", "actions": ["read"] }]` will fail the `tasks:create` check even if the underlying grant has `collaborate` capability. The full grant access rights are available via `auth.grant_access()` for display purposes (e.g., showing what the user *could* do with a full-scope session).

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

The `capability` claim is set by the org admin when they bind an instance to the org. Instances MAY override this (downgrade only, never upgrade).

## 4. Registry API (crabcity.dev)

### 4.1 Account Endpoints

#### `POST /api/v1/accounts`

Create a registry account. Links a public key to a handle.

```
Request:  { "public_key": "<base64>", "handle": "alex", "display_name": "Alex", "proof": "<base64-signature>", "key_label": "MacBook" }
Response: { "id": "<uuid>", "handle": "alex", ... }
Error:    409 if handle taken, 400 if proof invalid
```

`proof` is the signature of `"crabcity.dev:register:<handle>"` — proves the caller controls the private key.

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

Instances use this to resolve display names and discover that multiple keys belong to the same account.

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
    "motd": null
}
```

The instance sends its current `blocklist_version` as an `If-None-Match` header. The registry responds with entries added since that version. Blocklist deltas are **scoped** — separate arrays for global and each org the instance is bound to.

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

Resolve short-code to invite metadata (does NOT return the raw token until the user authenticates/creates an account).

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

Monitors (instances, public auditors) poll this endpoint to watch for unauthorized key bindings. An instance can run a background task that periodically fetches new entries and alerts if any of its members' registry accounts have unexpected key additions.

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
4. If user has no keypair: generate one, store in IndexedDB
5. KEY BACKUP MODAL: blocking modal, copy/download key, "I saved my key" checkbox
6. POST /api/invites/redeem { token, public_key, display_name }
7. Instance verifies invite, creates identity + grant, returns session token
8. Client stores session token as cookie, redirects to instance UI
9. User's presence appears to all connected clients (GrantUpdate broadcast)
```

### 5.2 Flow B: Registry Invite

```
1. User receives URL: https://crabcity.dev/join/abc12345
2. If user has no crabcity.dev account:
   a. Generate keypair (or import existing)
   b. KEY BACKUP MODAL
   c. POST /api/v1/accounts { public_key, handle, proof, key_label }
3. Registry resolves short_code -> invite token + instance URL
4. Registry redirects to instance with the invite token
5. Instance redeems invite (same as Flow A, step 6-9)
6. Instance also resolves handle via registry API for display
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
7. Instance exchanges code for id_token
8. Instance extracts public_key, handle, org, capability from claims
9. Instance creates/updates identity + grant, creates session
```

### 5.4 Flow D: CLI/TUI Authentication

```
First run (no identity):
1. $ crabcity connect instance.example.com
2. No keypair at ~/.config/crabcity/identity.key
3. Generate keypair, save to identity.key (mode 0600)
4. Print: "Your identity: crab_2K7XM9QP (saved to ~/.config/crabcity/identity.key)"
5. Prompt: "This instance requires an invitation. Enter invite code:"
6. User pastes base32 token
7. POST /api/invites/redeem { token, public_key, display_name }
8. Store session token in ~/.config/crabcity/sessions/<instance-id>
9. Connected.

Subsequent connections:
1. Read keypair from ~/.config/crabcity/identity.key
2. Check for cached session in ~/.config/crabcity/sessions/<instance-id>
3. If session expired: POST /api/auth/challenge { public_key, timestamp }
4. Sign nonce with private key
5. POST /api/auth/verify { public_key, nonce, signature, timestamp }
6. Cache new session token
7. Print: "Authenticated as crab_2K7XM9QP"
8. Print: "Connected to Alex's Workshop (3 users online)"
```

Loopback bypass (existing) still works for local instances.

## 6. Session Management

| Property | Value |
|----------|-------|
| Token size | 32 bytes (256 bits), random |
| Storage | SHA-256 hash stored server-side |
| Default TTL | 24 hours |
| Renewal | Sliding window — activity extends expiry |
| Revocation | DELETE /api/auth/session (logout) |
| Scope | Intersection of requested scope and grant access rights. Omit for full grant. |
| Transport | `Authorization: Bearer <base64>` header or `__crab_session` cookie (HttpOnly, Secure, SameSite=Strict) |

Session cleanup: a background task sweeps expired sessions every hour (or lazily on access — either is fine at this scale).

## 7. Key Recovery and Multi-Device

### 7.1 Multiple Keys Per Account

A registry account has multiple public keys from day one (see `account_keys` table in section 3.1).

When a user adds a new device, they authenticate with an existing key and register the new one. Instances learn about the key->account mapping via handle resolution.

When resolving a public key via the registry, the response includes the canonical account ID and all active keys. The instance can recognize that two different keys belong to the same logical user:

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

The instance stores the `registry_account_id` on `member_identities`. Different keys for the same account share identity metadata (display name, handle) but have separate grants (allowing different capabilities per device if desired).

### 7.2 Account Recovery

For keypair-only users (no registry): the instance admin uses the re-invite + replace flow (see architecture doc, "Key Loss Recovery"). Old contributions are attributed to the new key via the `replaces` link.

For registry users: if they've set an email, they can verify ownership and register a new key. This is a high-security operation (email verification + rate limiting + cooldown period).

There is no "forgot password" flow because there are no passwords.

## 8. Rate Limiting and Abuse Prevention

Despite "almost no traffic," certain endpoints need basic protection:

| Endpoint | Limit | Window |
|----------|-------|--------|
| `POST /api/auth/challenge` | 10 | per minute per IP |
| `POST /api/invites/redeem` | 5 | per minute per IP |
| `POST /api/v1/accounts` | 3 | per hour per IP |
| `POST /api/v1/accounts/:id/keys` | 5 | per hour per account |
| `POST /api/v1/instances/heartbeat` | 15 | per minute per instance |

Implemented as in-memory token buckets. No Redis needed.

## 9. Wire Formats

### 9.1 HTTP API

All API communication uses JSON over HTTPS. Content-Type: `application/json`.

Public keys are encoded as unpadded base64 (URL-safe variant) in JSON payloads.

Invite tokens are encoded as Crockford base32 (no padding, case-insensitive) for human-friendly sharing.

Signatures are encoded as unpadded base64 (URL-safe variant) in JSON payloads.

UUIDs are v7 (time-ordered) for database locality.

### 9.2 WebSocket Protocol

All WebSocket messages use envelope versioning:

```json
{ "v": 1, "type": "GrantUpdate", "data": { ... } }
```

Server -> Client message types (auth-related):

```
GrantUpdate       { grant }           -- member capability/state changed
IdentityUpdate    { identity }        -- member display name/handle/avatar changed
MemberJoined      { identity, grant } -- new member added
MemberRemoved     { public_key }      -- member removed
```

These are in addition to the existing `StateChange`, `TaskUpdate`, `InstanceList`, `Focus`, etc.

Unauthenticated preview stream (`/api/preview`):

```
PreviewActivity    { terminal_count, active_cursors, user_count, instance_name, uptime_secs }
```

Clients MUST ignore unknown message types and unknown versions. This allows the protocol to evolve without breaking existing clients.

## 10. Protocol Reference

### 10.1 Membership State Transitions

```
invited   -> active      via: first successful auth (challenge-response or session)
invited   -> removed     via: invite expired before first auth, or admin action
active    -> suspended   via: admin POST /api/members/:pk/suspend, or blocklist hit
active    -> removed     via: admin DELETE /api/members/:pk
suspended -> active      via: admin POST /api/members/:pk/reinstate
suspended -> removed     via: admin DELETE /api/members/:pk
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
| `identity.updated` | user pubkey | user pubkey | `{ fields_changed: [] }` |

### 10.3 Error Codes

| HTTP Status | Code | Meaning |
|-------------|------|---------|
| 400 | `invalid_invite` | Invite expired, exhausted, or malformed |
| 400 | `invalid_signature` | Signature verification failed |
| 400 | `invalid_timestamp` | Client timestamp too far from server time |
| 401 | `no_credentials` | No session token or cookie provided |
| 401 | `session_expired` | Session token expired |
| 403 | `not_a_member` | No grant exists for this public key |
| 403 | `grant_not_active` | Grant exists but state != active |
| 403 | `insufficient_access` | Missing required access right |
| 403 | `blocklisted` | Public key is on a blocklist |
| 409 | `handle_taken` | Registry handle already in use |
| 409 | `already_a_member` | Public key already has an active grant |
| 429 | `rate_limited` | Too many requests |
