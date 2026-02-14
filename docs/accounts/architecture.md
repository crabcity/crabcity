# Crab City Accounts: Architecture

## Overview

Crab City Accounts is a distributed identity and authorization system for Crab City instances. It combines **local cryptographic identity** (ed25519 keypairs) with an **optional central registry** at `crabcity.dev` that provides discovery, public profiles, OIDC-based enterprise SSO, and global moderation.

Every instance operates with full sovereignty. The registry adds value (handles, profiles, SSO, blocklists) without adding lock-in.

## Principles

1. **Identity is a keypair.** An ed25519 key pair IS the account. No usernames, no passwords, no email required at the base layer.
2. **Identity and authorization are separate concerns.** WHO you are (keypair, display name, handle) is stored independently from WHAT you can do (capability, permissions, membership state). Different update cadences, different broadcast events, different trust sources.
3. **Invites are signed capabilities.** Access control is a signed document, not a row in a central database.
4. **Instances are sovereign.** An instance can operate standalone forever. The registry is opt-in.
5. **The registry is a phonebook, not a platform.** It stores metadata and coordinates discovery. It does not mediate runtime traffic between users and instances.
6. **Enterprise features layer on, not replace.** OIDC/SSO binds to the same keypair identity. Org management is sugar over the same membership model.
7. **Every state transition is auditable.** Membership changes, capability grants, invite redemptions, and suspensions all produce structured events in an append-only log.

## System Topology

```
+----------------------------------------------------------+
|                     crabcity.dev                         |
|                   (central registry)                     |
|                                                          |
|  +--------------+  +--------------+  +----------------+  |
|  |  Accounts &  |  |   Instance   |  |   OIDC Provider|  |
|  |  Profiles    |  |   Directory  |  |   + RP         |  |
|  +--------------+  +--------------+  +----------------+  |
|  +-------------+  +--------------+                       |
|  | Blocklists  |  |  Org / Team  |                       |
|  |             |  |  Management  |                       |
|  +-------------+  +--------------+                       |
+----------+---------------+---------------+---------------+
           | heartbeat     | OIDC          | attestation
           | (pull)        | (auth flow)   | (push)
           v               v               v
+--------------+  +--------------+  +--------------+
|  Instance A  |  |  Instance B  |  |  Instance C  |
| (standalone) |  | (registered) |  | (org-managed)|
|              |  |              |  |              |
|  +--------+  |  |  +--------+  |  |  +--------+  |
|  | SQLite |  |  |  | SQLite |  |  |  | SQLite |  |
|  |identity|  |  |  |identity|  |  |  |identity|  |
|  | grants |  |  |  | grants |  |  |  | grants |  |
|  | events |  |  |  | events |  |  |  | events |  |
|  +--------+  |  |  +--------+  |  |  +--------+  |
+--------------+  +--------------+  +--------------+
```

**Instance A** uses only raw invites and local keypairs. No registry dependency.
**Instance B** registers with `crabcity.dev` for discovery and profile resolution.
**Instance C** is managed by an org; members authenticate via enterprise SSO through `crabcity.dev`.

## Identity Model

### Ed25519 Keypair (Base Layer)

Every user's identity is an ed25519 keypair. The public key is the canonical account identifier everywhere — on instances, on the registry, in invite tokens.

```
UserIdentity {
    public_key: ed25519::PublicKey,   // THE identity
    display_name: String,             // mutable, non-unique, user-chosen
    created_at: u64,
}
```

Keypairs are generated client-side. They never leave the device unless the user explicitly exports them. The registry can optionally custody a keypair for browser-only users (stored encrypted, derived from a passphrase), but this is a convenience, not a requirement.

### Key Fingerprints

Public keys are 32 bytes — too long to recognize visually. Every public key has a **fingerprint**: a human-readable short identifier for use in the TUI, logs, and admin UIs.

Format: `crab_` prefix + first 8 characters of the base32 (Crockford) encoding of the public key.

Example: `crab_2K7XM9QP`

Fingerprints are **not unique** (8 chars = 40 bits of entropy, sufficient to distinguish members within any realistic instance). They are a display convenience, never used for lookups or authentication.

### Loopback Identity

Local CLI/TUI connections via the loopback interface bypass authentication (existing behavior). These requests are attributed to a **synthetic loopback identity**: a well-known sentinel public key (all zeros, `0x00 * 32`) that cannot be used remotely.

The loopback identity:
- Always exists as an `owner`-level grant on every instance
- Cannot be invited, suspended, or removed
- Cannot be used for remote authentication (instances reject all-zeros pubkey on non-loopback connections)
- Preserves backward compatibility: local access still "just works"

This avoids the ambiguity of auto-provisioning a real keypair for loopback users. The loopback identity is synthetic and non-portable by design.

### Registry Account (Optional Layer)

When a user registers at `crabcity.dev`, they bind their keypair(s) to a handle:

```
Account {
    id: Uuid,
    keys: Vec<AccountKey>,            // multiple devices, one identity
    handle: String,                   // @alex — unique on the registry
    display_name: String,
    avatar_url: Option<String>,
    email: Option<String>,            // for OIDC binding / recovery
    oidc_bindings: Vec<OidcBinding>,
    created_at: DateTime,
    blocked: bool,
}

AccountKey {
    public_key: ed25519::PublicKey,
    label: String,                    // "MacBook", "Phone", "YubiKey"
    created_at: DateTime,
    revoked_at: Option<DateTime>,
}
```

Multi-device is a day-one registry feature, not a late addition. Users who set up their identity on a laptop and then want to check from their phone should not be blocked. Adding a second key requires authentication with an existing key.

Instances resolve handles via the registry API (`GET /api/v1/accounts/by-handle/:handle`), but they cache the pubkey->handle mapping locally. The registry is not in the hot path for any request after initial resolution.

### OIDC Binding (Enterprise Layer)

Enterprise users authenticate via their corporate IdP. `crabcity.dev` acts as an OIDC relying party (consuming Okta/Entra/Google Workspace tokens) and an OIDC provider (issuing tokens to instances).

```
OidcBinding {
    account_id: Uuid,
    provider: String,              // "okta", "entra", "google-workspace"
    issuer: Url,                   // https://acme.okta.com
    subject: String,               // IdP-assigned user ID
    org_id: Option<Uuid>,
}
```

The double-hop OIDC flow:

```
Enterprise IdP (Okta)
       |
       |  OIDC auth code flow
       v
crabcity.dev (Relying Party)
       |
       |  maps to keypair, issues crabcity.dev OIDC token
       v
crabcity.dev (Provider)
       |
       |  OIDC auth code flow (instance is the RP now)
       v
Crab City Instance
       |
       |  extracts pubkey + org claims, creates local identity + grant
       v
Local SQLite (member_identities + member_grants)
```

Instances only need to trust one OIDC issuer: `https://crabcity.dev`. They never configure per-enterprise IdPs.

## Authorization Model

### Capabilities and Permissions

Capabilities are named presets that expand to a set of fine-grained permissions. The API surface uses capabilities (`"capability": "collaborate"`). The database stores the expanded permission set. This allows future fine-grained overrides without breaking the simple capability model.

| Capability    | Description                                    |
|---------------|------------------------------------------------|
| `view`        | Read-only access to instance content           |
| `collaborate` | Create/edit content, join terminals            |
| `admin`       | Manage instance settings, invite others, moderate |
| `owner`       | Full control, transfer ownership               |

Capabilities expand to permission bitfields:

```
Permissions (u32 bitfield):
    VIEW_CONTENT     = 0x01    // read instances, conversations, tasks
    VIEW_TERMINALS   = 0x02    // observe terminal output
    SEND_CHAT        = 0x04    // send chat messages
    EDIT_TASKS       = 0x08    // create/edit/close tasks
    TERMINAL_INPUT   = 0x10    // send terminal input
    CREATE_INSTANCE  = 0x20    // create new instances
    MANAGE_MEMBERS   = 0x40    // invite, suspend, remove, change capabilities
    MANAGE_INSTANCE  = 0x80    // instance settings, restart, shutdown

Capability presets:
    view        = VIEW_CONTENT | VIEW_TERMINALS
    collaborate = view | SEND_CHAT | EDIT_TASKS | TERMINAL_INPUT | CREATE_INSTANCE
    admin       = collaborate | MANAGE_MEMBERS
    owner       = all bits set
```

Invite tokens carry a `Capability` enum (1 byte, compact). The expansion to permissions happens at redemption time. Admins can optionally tweak individual permission bits on a grant after redemption (e.g., "collaborate but no terminal input").

### Membership State Machine

Every membership (grant) has an explicit lifecycle state:

```
                +---> Active ---+---> Suspended ---+---> Active (reinstate)
                |               |                  |
    Invited ----+               +---> Removed      +---> Removed
                |
                +---> Removed (invite expired before first auth)
```

| State       | Meaning                                              | Access |
|-------------|------------------------------------------------------|--------|
| `invited`   | Invite redeemed, grant created, user hasn't completed first auth yet | Denied |
| `active`    | Normal operating state                               | Granted |
| `suspended` | Admin action, blocklist hit, or temporary hold        | Denied |
| `removed`   | Terminal state; row kept for audit trail              | Denied |

The auth middleware checks `state == active`, period. No multi-table joins against blocklists in the hot path — blocklist hits transition grants to `suspended` (with `reason` recorded in the event log), and the middleware only needs to read one column.

### Identity and Authorization: Separate Tables

Instance-local data is split into two concerns:

**`member_identities`** — WHO you are. Cached from registry or self-reported. Updated by registry resolution, user profile changes. Broadcast as `IdentityUpdate`.

**`member_grants`** — WHAT you can do. Instance-local authorization. Updated by admin actions, invite redemption, blocklist enforcement. Broadcast as `GrantUpdate`.

This separation means:
- Updating a display name doesn't touch the authorization table
- Changing a capability doesn't re-resolve identity
- The broadcast for "Alex changed their avatar" is a different message type than "Alex was promoted to admin"
- Identity resolution (slow, async, registry-dependent) is decoupled from authorization checks (fast, local, synchronous)

### Invites (Standalone Path)

An invite is a self-contained, signed capability grant:

```
Invite {
    issuer: PublicKey,             // who created this
    instance: PublicKey,           // which instance (NodeId)
    capability: Capability,
    max_uses: Option<u32>,
    expires_at: Option<u64>,
    nonce: [u8; 16],
    signature: ed25519::Signature, // issuer signs all fields above
}
```

Invites are serialized, base32-encoded, and distributed out-of-band (URL, chat, email). They require no registry involvement.

**Invite revocation semantics:** Revoking an invite revokes *unredeemed uses only*. Existing memberships created from that invite are not affected. To suspend members who joined via a specific invite, use the separate "revoke invite and suspend derived members" admin action. This requires tracing invite->member relationships via the `invited_via` field on grants.

### Invite Delegation Chains

Invites support **delegation**: a member who received an invite can sub-delegate it to others, creating a cryptographic chain of authority.

```
DelegatedInvite {
    chain: Vec<InviteLink>,        // ordered, root-to-leaf
}

InviteLink {
    issuer: PublicKey,
    capability: Capability,        // can only stay same or decrease down the chain
    max_depth: u8,                 // how many more delegations allowed (0 = leaf, no further delegation)
    max_uses: u32,
    nonce: [u8; 16],
    signature: Signature,          // signs (prev_link_hash ++ own fields)
}
```

Verification walks the chain from root to leaf:
1. Root issuer must be a member with `MANAGE_MEMBERS` on the instance
2. Each link's capability must be <= previous link's capability (capabilities can only narrow)
3. Each link's depth must be < previous link's remaining depth
4. Each signature is valid over (previous link hash + current fields)

The token is a self-contained proof of authorization — the instance can verify the entire delegation without having seen any intermediate step. This enables viral invite distribution: power users become invite distributors without admin intervention, bounded by `max_depth`.

A flat (non-delegated) invite is just a chain of length 1. The v1 invite format is a degenerate case of the delegation chain.

### Event Log (Verifiable Audit Trail)

Every state transition on an instance produces an event. Events are **hash-chained**: each event includes the SHA-256 hash of the previous event, forming a tamper-evident log.

```
Event {
    id: u64,                       // monotonic
    prev_hash: [u8; 32],          // H(previous event) — genesis event uses H(instance_node_id)
    event_type: String,            // "member.joined", "grant.capability_changed", etc.
    actor: PublicKey,               // who did it
    target: Option<PublicKey>,      // who it happened to
    payload: Json,                 // event-specific details
    created_at: DateTime,
    hash: [u8; 32],               // H(id ++ prev_hash ++ event_type ++ actor ++ target ++ payload ++ created_at)
}
```

The hash chain provides:
- **Tamper evidence** — modifying or deleting any event breaks the chain. A sequential scan can verify integrity.
- **Signed checkpoints** — every N events (configurable, default 100), the instance signs the current chain head hash with its NodeId key. This means even the instance operator cannot silently tamper with the log.
- **Cross-instance audit** — a signed checkpoint is a portable proof of log integrity. An admin can present a signed chain head to prove their instance's event history is untampered.
- **Merkle proofs** — users can request inclusion proofs that a specific event (e.g., their `member.joined`) exists in the chain.

Event types:
- `member.joined` — new identity + grant created (invite redemption, OIDC, loopback)
- `member.suspended` — grant state -> suspended (admin action or blocklist)
- `member.reinstated` — grant state -> active (admin action)
- `member.removed` — grant state -> removed
- `member.replaced` — new grant linked to old one (key loss recovery)
- `grant.capability_changed` — capability or permissions updated
- `grant.permissions_tweaked` — individual permission bits changed
- `invite.created` — new invite issued
- `invite.redeemed` — invite used (links to member.joined)
- `invite.revoked` — invite revoked
- `identity.updated` — display name, handle, or avatar changed

The event log is append-only, never mutated. It serves as:
1. **Verifiable audit trail** — "who invited who, when was someone promoted" — with cryptographic tamper evidence
2. **Debug tool** — trace the provenance of any membership
3. **Undo mechanism** — admins can review and reverse actions
4. **Future: activity feed** — surface meaningful events in the UI

## Instance Registry

### Publication

Instance operators opt into publication by registering at `crabcity.dev`:

```
Instance {
    id: Uuid,
    owner: AccountId,
    node_id: iroh::NodeId,
    slug: String,                  // "alexs-workshop"
    display_name: String,
    description: String,
    visibility: Visibility,        // Public | Unlisted | Private
    published_at: DateTime,
    last_seen: DateTime,
    blocked: bool,
}
```

`Public` instances appear in the directory. `Unlisted` instances are accessible by direct link. `Private` instances are only visible to org members.

### Heartbeat Protocol

Registered instances send a periodic heartbeat (every 5 minutes):

```
POST /api/v1/instances/heartbeat
-> { node_id, version, user_count, public_metadata }
<- {
     blocklist_version: 42,
     blocklist_deltas: {
       "global": [...],
       "org:acme-corp": [...],
       "org:widgets-inc": [...]
     },
     motd: null
   }
```

Blocklist deltas are **scoped** — the response includes separate delta arrays for the global blocklist and each org the instance is bound to. This prevents ambiguity when an instance belongs to multiple orgs.

The heartbeat serves three purposes:
1. **Liveness** — the registry marks instances as offline after missed heartbeats
2. **Blocklist sync** — scoped delta-encoded blocklist updates piggybacked on the heartbeat response
3. **Announcements** — registry-to-instance communication channel (MOTD, deprecation notices)

This is the ONLY protocol between instance and registry during steady-state operation.

**Known property:** Blocklist enforcement has a propagation window of up to 5 minutes (one heartbeat interval). If a user is blocked globally, instances that haven't heartbeated yet will still allow access until their next heartbeat. This is acceptable at the expected traffic level and is documented as an explicit design property, not a bug.

## Blocklists

Three scopes:

| Scope    | Maintained by       | Enforced by         | Distribution          |
|----------|---------------------|---------------------|-----------------------|
| Global   | crabcity.dev admins | opt-in by instances | heartbeat delta       |
| Org      | org admins          | org instances       | heartbeat delta       |
| Instance | instance admins     | that instance       | local (no sync)       |

Blocklist entries target either a `PublicKey` (user) or `NodeId` (instance) or `IpRange`.

Instances opt into global blocklist enforcement. This is a social contract, not a technical lock: a rogue instance can ignore the blocklist, but it can be delisted from the directory.

When a blocklist entry hits an active member, the instance transitions that member's grant to `suspended` and logs a `member.suspended` event with the blocklist scope and reason. This is a state transition, not a runtime check — the auth middleware only needs `state == active`.

## Wire Format Versioning

All WebSocket messages use envelope versioning:

```json
{ "v": 1, "type": "GrantUpdate", "data": { ... } }
```

Clients ignore messages with versions they don't understand. This allows individual message types to evolve independently without breaking connected clients. The envelope version (`v`) is the protocol version, not a per-message-type version — all messages in protocol v1 share the same contract.

HTTP API versioning uses URL path prefixes (`/api/v1/...` for registry, no prefix for instance-local APIs in M1). Breaking changes increment the path version. Non-breaking additions (new optional fields) don't require a version bump.

## Org / Team Management

```
Org {
    id: Uuid,
    slug: String,                  // "acme-corp"
    display_name: String,
    oidc_config: Option<OidcConfig>,
    instance_quota: u32,
    members: Vec<OrgMember>,
}

OrgMember {
    account_id: Uuid,
    org_role: OrgRole,             // Owner | Admin | Member
}
```

Orgs group accounts and instances. When an org has OIDC configured, new members are auto-provisioned when they first SSO through `crabcity.dev`. The org admin controls which instances their members have access to and at what capability level.

## Security Boundaries

| Boundary | Trust model |
|----------|-------------|
| User <-> Instance | Challenge-response (user proves keypair ownership) or session token |
| Instance <-> Registry | Instance authenticates via its NodeId keypair; registry authenticates via TLS + OIDC signing key |
| User <-> Registry | OIDC tokens (for SSO) or challenge-response (for keypair-native users) |
| Instance <-> Instance | No direct trust required; iroh handles transport encryption |

### Challenge-Response Protocol

The challenge-response signs a structured, self-documenting payload:

```
sign("crabcity:auth:v1:" ++ nonce ++ instance_node_id ++ client_timestamp)
```

- `crabcity:auth:v1:` prefix prevents cross-protocol confusion if keypairs are used for other signatures
- `nonce` prevents replay of the same challenge
- `instance_node_id` prevents cross-instance replay
- `client_timestamp` (wall clock, checked +-30s of server time) narrows the replay window

Pending challenges are stored **in-memory** (e.g., `DashMap<Nonce, PendingChallenge>`), not in SQLite. They are single-use and expire after 60 seconds. Note: if the instance is ever deployed behind a load balancer with multiple processes, pending challenges require sticky sessions or a shared store.

### Key Rotation

- **User keypairs**: Users can register multiple keys to the same registry account. Old keys can be deprecated without losing access.
- **Registry OIDC signing keys**: Published via JWKS endpoint (`/.well-known/jwks.json`). Support multiple active keys. Rotate on a fixed schedule (90 days). Instances cache JWKS with a TTL.
- **Instance NodeId keys**: Tied to iroh identity. Rotation requires re-registration at the registry.

### Key Transparency

The registry maintains a **verifiable log** of all key binding operations (account creation, key addition, key revocation). This log is a Merkle tree — any entry's inclusion can be proven with a compact proof.

Properties:
- The tree head is signed by the registry and published periodically (on every mutation)
- **Monitors** (instances, public auditors, or the users themselves) can watch the log for unauthorized key bindings
- Users can audit their own account at any time: "show me every key that has ever been bound to `@alex`"
- If the registry is compromised and a rogue key is bound to an account, any monitor watching the log will detect it

This makes the "registry is a phonebook, not a platform" principle *cryptographically enforceable*. Enterprise customers don't have to trust the registry operator — their instances can independently verify that key bindings haven't been tampered with.

### Scoped Sessions

Session tokens carry an explicit **permission scope** that is the intersection of what the client requested and what the underlying grant allows:

```
Session {
    token_hash: [u8; 32],
    public_key: PublicKey,
    scope: Permissions,            // <= grant.permissions
    expires_at: DateTime,
}
```

A CLI tool that only needs to read tasks can request a `VIEW_CONTENT`-only session. If the token leaks, the blast radius is limited to the requested scope, not the full grant. This is the principle of least privilege applied to session tokens.

Scoped sessions are backward-compatible: omit the `scope` parameter in the challenge request and you get the full grant permissions, same as an unscoped session.

## Join Experience

The zero-to-collaborating flow is a first-class design artifact, not an implementation afterthought:

```
1. Alex creates an instance, starts it locally
2. Alex clicks "Invite" in the UI -> gets a link
3. Alex sends the link to Blake in Slack
4. Blake clicks the link -> sees the join page
5. Blake enters a display name -> "Join"
6. Behind the scenes: keypair generated, invite redeemed, session created
7. Blake sees the instance. Terminals, chat, tasks.
8. Blake's presence appears in Alex's UI immediately (broadcast)
```

### The Join Page

What Blake sees at `https://instance/join#<token>`:

- Instance name + inviter's display name (extracted from the signed invite)
- Capability being granted ("You're being invited to **collaborate**")
- **Live preview**: a small, blurred/abstracted terminal window showing real-time activity (cursor movement, user count, terminal dimensions — no content). This communicates "this is an active, living place" before the user commits to joining.
- Number of users currently online (live-updating via WebSocket)
- Single input: "Your name" (pre-filled if they have a registry account)
- Single button: "Join"
- Below the fold: "This will create a cryptographic identity on your device. [Learn more]"

If Blake already has a keypair in IndexedDB for this instance: skip the name prompt, show "Welcome back, Blake. [Rejoin]"

The live preview uses a dedicated `preview` WebSocket stream that requires no authentication. It delivers only: terminal dimensions, cursor position (no content), user count, and instance uptime. This is the "looking through the restaurant window" experience — enough to see activity, not enough to see data.

### Key Backup (Blocking Modal)

After first keypair generation, the user sees a **blocking modal** (not a dismissable toast):

- "Save your identity key" — explanation that this is the only copy
- Copy-to-clipboard button (base64-encoded private key)
- Download `.key` file button
- "I saved my key" checkbox required to proceed

This is modeled after TOTP recovery code flows. The modal cannot be dismissed without confirming. The UX is deliberately inconvenient because the consequence of key loss (identity loss) is severe.

### Key Loss Recovery

When a known user loses their key and can't access the instance:

1. User contacts admin out-of-band ("I lost my key")
2. Admin goes to member list -> finds the user -> clicks "Re-invite"
3. Admin sends new invite link
4. User clicks link -> new keypair generated -> new grant created
5. Admin optionally **links** the new grant to the old one ("This is the same person")
6. The old grant transitions to `removed(replaced_by=new_pubkey)`
7. The UI merges attribution: chat messages, task assignments, and terminal history from the old key are displayed under the new identity

The `replaces` field on `member_grants` enables this linking. The event log records a `member.replaced` event for auditability.

### Iroh-Native Invite Exchange

Invites are transport-agnostic signed blobs. In addition to URL-based distribution, invites can be exchanged **directly via iroh** — no URL, no side-channel, no registry.

```
Alice's TUI:                              Bob's TUI:
  > /invite --discover                      > crabcity join --discover
  Advertising invite via iroh...            Found: "Alice's Workshop"
  Found: "Bob's MacBook" (crab_7F3X...)     Join as collaborate? [y/n]
  Accept? [y/n] > y                         > y
  Invite sent. Bob is joining...            Generating keypair... crab_7F3XM9QP
                                            Connected.
```

Under the hood:
1. Alice's instance publishes a short-lived iroh document with the invite token, discoverable via mDNS or iroh's DHT
2. Bob's client discovers it, presents it to the user for confirmation
3. Bob's client redeems the invite directly via iroh transport (no HTTP needed)
4. The invite document self-destructs after redemption or timeout

This is the **zero-infrastructure invite path**: two devices on the same network (or reachable via iroh relay) and you're in. It's a pure client feature — the invite token format is the same, only the transport differs.

### CLI/TUI First-Run

```
$ crabcity connect instance.example.com
No identity found. Generating keypair...
Your identity: crab_2K7XM9QP (saved to ~/.config/crabcity/identity.key)

This instance requires an invitation. Enter invite code:
> [paste base32 token]

Joined as "collaborate" member. Welcome!
```

Subsequent connections:

```
$ crabcity connect instance.example.com
Authenticated as crab_2K7XM9QP
Connected to Alex's Workshop (3 users online)
```

## Data Flow Summary

| Flow | Path | Frequency |
|------|------|-----------|
| Raw invite redemption | User -> Instance | Once per invite |
| Registry invite redemption | User -> Registry -> Instance | Once per invite |
| OIDC SSO login | User -> Enterprise IdP -> Registry -> Instance | Once per session |
| Instance heartbeat | Instance -> Registry | Every 5 min |
| Handle/profile resolution | Instance -> Registry | Cached, infrequent |
| Blocklist sync | Registry -> Instance (via heartbeat) | Every 5 min |

## Non-Goals

- **Federated social graph.** The registry is not ActivityPub.
- **Runtime traffic proxying.** The registry never proxies user<->instance traffic.
- **Central message storage.** Chat, terminal sessions, tasks — all instance-local.
- **Universal search.** You cannot search across instances from the registry.
- **Payment processing.** Billing, if it ever exists, is a separate concern.
