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

### 1.2 Challenge-Response Authentication

When a user authenticates to an instance with their keypair:

```
Client                              Instance
  │                                    │
  │  POST /api/auth/challenge          │
  │  { public_key }                    │
  │ ──────────────────────────────────>│
  │                                    │  generate 32-byte random nonce
  │                                    │  store (nonce, pubkey, expires=60s)
  │  { nonce }                         │
  │ <──────────────────────────────────│
  │                                    │
  │  sign(nonce ++ instance_node_id)   │
  │                                    │
  │  POST /api/auth/verify             │
  │  { public_key, nonce, signature }  │
  │ ──────────────────────────────────>│
  │                                    │  verify signature
  │                                    │  check membership
  │                                    │  create session
  │  { session_token, expires_at }     │
  │ <──────────────────────────────────│
```

The signed payload includes the instance's `NodeId` to prevent replay attacks across instances. The nonce is single-use and expires after 60 seconds.

### 1.3 Invite Token Format

```
Invite = {
    version: u8,                    // 0x01
    issuer: [u8; 32],              // ed25519 public key
    instance: [u8; 32],            // instance NodeId
    capability: u8,                // 0=view, 1=collaborate, 2=admin
    max_uses: u32,                 // 0 = unlimited
    expires_at: u64,               // unix timestamp, 0 = never
    nonce: [u8; 16],               // random, for uniqueness
    signature: [u8; 64],           // ed25519 signature over all preceding fields
}
```

Total: 1 + 32 + 32 + 1 + 4 + 8 + 16 + 64 = **158 bytes**

Encoded as base32 (Crockford, no padding): **254 characters**

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

## 2. Instance-Side Data Model

### 2.1 Schema (SQLite)

```sql
-- User memberships on this instance
CREATE TABLE memberships (
    public_key BLOB NOT NULL PRIMARY KEY,  -- 32 bytes, ed25519
    display_name TEXT NOT NULL DEFAULT '',
    handle TEXT,                            -- @alex, from registry
    capability TEXT NOT NULL,               -- 'view', 'collaborate', 'admin', 'owner'
    org_id TEXT,                            -- UUID, from OIDC claims
    invited_by BLOB,                        -- 32 bytes, pubkey of inviter
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
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
    FOREIGN KEY (issuer) REFERENCES memberships(public_key)
);

-- Active sessions
CREATE TABLE sessions (
    token BLOB NOT NULL PRIMARY KEY,       -- 32 bytes, random
    public_key BLOB NOT NULL,              -- 32 bytes
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (public_key) REFERENCES memberships(public_key)
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

CREATE INDEX idx_sessions_expires ON sessions(expires_at);
CREATE INDEX idx_sessions_pubkey ON sessions(public_key);
CREATE INDEX idx_invites_expires ON invites(expires_at);
```

### 2.2 Instance-Side API

#### `POST /api/auth/challenge`

Request identity challenge.

```
Request:  { "public_key": "<base64>" }
Response: { "nonce": "<base64>", "expires_at": "<iso8601>" }
```

#### `POST /api/auth/verify`

Complete challenge-response, get session.

```
Request:  { "public_key": "<base64>", "nonce": "<base64>", "signature": "<base64>" }
Response: { "session_token": "<base64>", "expires_at": "<iso8601>", "capability": "collaborate" }
Error:    401 if signature invalid, 403 if no membership
```

#### `POST /api/auth/oidc/callback`

OIDC callback from crabcity.dev. Instance acts as OIDC RP.

```
Query:    ?code=<auth_code>&state=<csrf_state>
Response: 302 redirect to instance UI with session cookie set
```

#### `POST /api/invites`

Create an invite. Requires `admin` or `owner` capability.

```
Request:  { "capability": "collaborate", "max_uses": 5, "expires_in_hours": 72 }
Response: { "token": "<base32>", "url": "https://instance/join#<base32>" }
```

#### `POST /api/invites/redeem`

Redeem an invite token.

```
Request:  { "token": "<base32>", "public_key": "<base64>", "display_name": "Alex" }
Response: { "membership": { ... }, "session_token": "<base64>" }
Error:    400 if expired/exhausted, 403 if issuer revoked or blocklisted
```

#### `GET /api/members`

List instance members. Requires `view` or higher.

```
Response: { "members": [{ "public_key": "...", "display_name": "...", "handle": "@alex", "capability": "collaborate" }] }
```

#### `DELETE /api/members/:public_key`

Remove a member. Requires `admin` or `owner`. Cannot remove `owner`.

#### `PATCH /api/members/:public_key`

Update a member's capability. Requires `admin` or `owner`. Cannot escalate beyond own capability.

### 2.3 Auth Middleware Changes

The existing auth middleware gains a new check in the chain:

```
1. Loopback bypass (existing) → allow
2. Session token in Authorization header → lookup sessions table → allow/deny
3. Cookie-based session (for browser clients) → lookup sessions table → allow/deny
4. No credentials → 401
```

Session tokens are 32 random bytes, base64-encoded, stored hashed (SHA-256) in the sessions table. Default TTL: 24 hours, configurable.

## 3. Registry Data Model (crabcity.dev)

### 3.1 Schema (SQLite or Postgres — SQLite is fine for the traffic level)

```sql
-- Registry accounts
CREATE TABLE accounts (
    id TEXT NOT NULL PRIMARY KEY,           -- UUID
    public_key BLOB NOT NULL UNIQUE,        -- 32 bytes, ed25519
    handle TEXT NOT NULL UNIQUE,            -- lowercase, alphanumeric + hyphens
    display_name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    email TEXT,
    email_verified INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    blocked INTEGER NOT NULL DEFAULT 0,
    blocked_reason TEXT
);

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

-- Registry-mediated invites (short-code → invite token)
CREATE TABLE registry_invites (
    short_code TEXT NOT NULL PRIMARY KEY,   -- 8-char alphanumeric
    instance_id TEXT NOT NULL,
    invite_token BLOB NOT NULL,            -- the full signed invite blob
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    FOREIGN KEY (instance_id) REFERENCES instances(id)
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
Request:  { "public_key": "<base64>", "handle": "alex", "display_name": "Alex", "proof": "<base64-signature>" }
Response: { "id": "<uuid>", "handle": "alex", ... }
Error:    409 if handle taken, 400 if proof invalid
```

`proof` is the signature of `"crabcity.dev:register:<handle>"` — proves the caller controls the private key.

#### `GET /api/v1/accounts/by-handle/:handle`

Public profile lookup.

```
Response: { "id": "...", "public_key": "...", "handle": "alex", "display_name": "Alex", "avatar_url": "...", "instances": [...] }
```

Only includes instances with `visibility = 'public'`.

#### `GET /api/v1/accounts/by-key/:public_key`

Reverse lookup: public key → handle.

```
Response: { "handle": "alex", "display_name": "Alex" }
Error:    404 if not registered
```

Instances use this to resolve display names.

### 4.2 Instance Endpoints

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
    "blocklist_delta": [
        { "action": "add", "target_type": "pubkey", "target_value": "<base64>" },
        { "action": "remove", "target_type": "pubkey", "target_value": "<base64>" }
    ],
    "motd": null
}
```

The instance sends its current `blocklist_version` as an `If-None-Match` header. The registry responds with entries added since that version.

#### `GET /api/v1/instances`

Public directory listing.

```
Query:    ?visibility=public&sort=last_seen&limit=50&offset=0
Response: { "instances": [...], "total": 142 }
```

#### `GET /api/v1/instances/by-slug/:slug`

Single instance lookup.

### 4.3 OIDC Endpoints

Standard OIDC provider endpoints:

```
GET  /.well-known/openid-configuration    — OIDC discovery document
GET  /.well-known/jwks.json               — public signing keys
GET  /oidc/authorize                      — authorization endpoint
POST /oidc/token                          — token endpoint
GET  /oidc/userinfo                       — userinfo endpoint
```

### 4.4 Org Endpoints

#### `POST /api/v1/orgs`

Create an org.

#### `PATCH /api/v1/orgs/:slug`

Update org settings (OIDC config, instance quota).

#### `POST /api/v1/orgs/:slug/members`

Add a member to the org.

#### `POST /api/v1/orgs/:slug/instances`

Bind an instance to the org (sets default capability for org members).

### 4.5 Invite Endpoints

#### `POST /api/v1/invites`

Register an invite at the registry (creates short-code URL).

```
Request:  { "instance_id": "<uuid>", "invite_token": "<base32>" }
Response: { "short_code": "abc12345", "url": "https://crabcity.dev/join/abc12345" }
```

#### `GET /api/v1/invites/:short_code`

Resolve short-code to invite metadata (does NOT return the raw token until the user authenticates/creates an account).

### 4.6 Blocklist Endpoints

#### `GET /api/v1/blocklist`

Full global blocklist (for initial sync).

#### `GET /api/v1/blocklist/delta?since_version=N`

Delta since version N.

## 5. Client Authentication Flows

### 5.1 Flow A: Raw Invite (No Registry)

```
1. User receives invite URL: https://instance.example/join#<base32>
2. SvelteKit frontend extracts token from fragment
3. If user has no keypair: generate one, store in IndexedDB
4. POST /api/invites/redeem { token, public_key, display_name }
5. Instance verifies invite, creates membership, returns session token
6. Client stores session token, redirects to instance UI
```

### 5.2 Flow B: Registry Invite

```
1. User receives URL: https://crabcity.dev/join/abc12345
2. If user has no crabcity.dev account:
   a. Generate keypair (or import existing)
   b. POST /api/v1/accounts { public_key, handle, proof }
3. Registry resolves short_code → invite token + instance URL
4. Registry redirects to instance with the invite token
5. Instance redeems invite (same as Flow A, step 4-6)
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
   e. crabcity.dev maps IdP subject → account (auto-provisioning if first login)
5. crabcity.dev issues its own OIDC id_token with crab city claims
6. Redirect back to instance with auth code
7. Instance exchanges code for id_token
8. Instance extracts public_key, handle, org, capability from claims
9. Instance creates/updates membership, creates session
```

### 5.4 Flow D: CLI/TUI Authentication

```
1. CLI reads keypair from ~/.config/crabcity/identity.key
2. POST /api/auth/challenge { public_key }
3. Sign nonce with private key
4. POST /api/auth/verify { public_key, nonce, signature }
5. Store session token in ~/.config/crabcity/sessions/<instance-id>
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
| Transport | `Authorization: Bearer <base64>` header or `__crab_session` cookie (HttpOnly, Secure, SameSite=Strict) |

Session cleanup: a background task sweeps expired sessions every hour (or lazily on access — either is fine at this scale).

## 7. Key Recovery and Multi-Device

### 7.1 Multiple Keys Per Account

A registry account can have multiple public keys:

```sql
CREATE TABLE account_keys (
    account_id TEXT NOT NULL,
    public_key BLOB NOT NULL,
    label TEXT NOT NULL DEFAULT '',         -- "MacBook", "Phone", "YubiKey"
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_at TEXT,
    PRIMARY KEY (account_id, public_key),
    FOREIGN KEY (account_id) REFERENCES accounts(id)
);
```

When a user adds a new device, they authenticate with an existing key and register the new one. Instances learn about the key→account mapping via handle resolution.

### 7.2 Account Recovery

For keypair-only users (no registry): the instance admin re-invites them. Old contributions stay attributed to the old key.

For registry users: if they've set an email, they can verify ownership and register a new key. This is a high-security operation (email verification + rate limiting + cooldown period).

There is no "forgot password" flow because there are no passwords.

## 8. Rate Limiting and Abuse Prevention

Despite "almost no traffic," certain endpoints need basic protection:

| Endpoint | Limit | Window |
|----------|-------|--------|
| `POST /api/auth/challenge` | 10 | per minute per IP |
| `POST /api/invites/redeem` | 5 | per minute per IP |
| `POST /api/v1/accounts` | 3 | per hour per IP |
| `POST /api/v1/instances/heartbeat` | 15 | per minute per instance |

Implemented as in-memory token buckets. No Redis needed.

## 9. Wire Formats

All API communication uses JSON over HTTPS. Content-Type: `application/json`.

Public keys are encoded as unpadded base64 (URL-safe variant) in JSON payloads.

Invite tokens are encoded as Crockford base32 (no padding, case-insensitive) for human-friendly sharing.

Signatures are encoded as unpadded base64 (URL-safe variant) in JSON payloads.

UUIDs are v7 (time-ordered) for database locality.
