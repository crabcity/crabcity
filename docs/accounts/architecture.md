# Crab City Accounts: Architecture

## Overview

Crab City Accounts is a distributed identity and authorization system for Crab
City instances. It combines **local cryptographic identity** (ed25519 keypairs)
with an **optional central registry** at `crabcity.dev` that provides discovery,
public profiles, OIDC-based enterprise SSO, and global moderation.

Every instance operates with full sovereignty. The registry adds value (handles,
profiles, SSO, blocklists) without adding lock-in.

## Principles

1. **Account identity is a keypair.** An ed25519 key pair IS the account. No
   usernames, no passwords, no email required at the base layer.
2. **Human identity and account identity are separate.** A person may have
   multiple accounts, linked or not. The system never infers human identity from
   account identity — linkage is always explicit and opt-in (via identity proofs,
   registry resolution, or OIDC bindings).
3. **People think in nouns, not keys.** The invite system speaks the nouns people
   already know — GitHub usernames, email addresses, crabcity handles. Noun
   resolution happens at invite time through the registry; grants are always
   keypair-based at runtime.
4. **Identity and authorization are separate concerns.** WHO your account is
   (keypair, display name, handle) is stored independently from WHAT you can do
   (capability, access rights, membership state). Different update cadences,
   different broadcast events, different trust sources.
5. **Invites are signed capabilities.** Access control is a signed document, not
   a row in a central database.
6. **Instances are sovereign.** An instance can operate standalone forever. The
   registry is opt-in.
7. **The registry is a phonebook, not a platform.** It stores metadata and
   coordinates discovery. It does not mediate runtime traffic between users and
   instances. It also serves as the noun resolver — mapping GitHub usernames,
   email addresses, and handles to pubkeys — but only at invite time, never at
   runtime.
8. **Enterprise features layer on, not replace.** OIDC/SSO binds to the same
   keypair identity. Org management is sugar over the same membership model.
9. **Every state transition is auditable.** Membership changes, capability
   grants, invite redemptions, and suspensions all produce structured events in
   an append-only log.

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
**Instance B** registers with `crabcity.dev` for discovery and profile
resolution. **Instance C** is managed by an org; members authenticate via
enterprise SSO through `crabcity.dev`.

## Identity Model

### Ed25519 Keypair (Base Layer)

Every user's identity is an ed25519 keypair. The public key is the canonical
account identifier everywhere — on instances, on the registry, in invite tokens.

```
UserIdentity {
    public_key: ed25519::PublicKey,   // THE identity
    display_name: String,             // mutable, non-unique, user-chosen
    created_at: u64,
}
```

Keypairs are generated client-side. They never leave the device unless the user
explicitly exports them. The registry can optionally custody a keypair for
browser-only users (stored encrypted, derived from a passphrase), but this is a
convenience, not a requirement.

### Key Fingerprints

Public keys are 32 bytes — too long to recognize visually. Every public key has
a **fingerprint**: a human-readable short identifier for use in the TUI, logs,
and admin UIs.

Format: `crab_` prefix + first 8 characters of the base32 (Crockford) encoding
of the public key.

Example: `crab_2K7XM9QP`

Fingerprints are **not unique** (8 chars = 40 bits of entropy, sufficient to
distinguish members within any realistic instance). They are a display
convenience, never used for lookups or authentication.

### Loopback Identity

Local CLI/TUI connections via the loopback interface bypass authentication
(existing behavior). These requests are attributed to a **synthetic loopback
identity**: a well-known sentinel public key (all zeros, `0x00 * 32`) that
cannot be used remotely.

The loopback identity:
- Always exists as an `owner`-level grant on every instance
- Cannot be invited, suspended, or removed
- Cannot be used for remote authentication (instances reject all-zeros pubkey on
  non-loopback connections)
- Preserves backward compatibility: local access still "just works"

This avoids the ambiguity of auto-provisioning a real keypair for loopback
users. The loopback identity is synthetic and non-portable by design.

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

Multi-device is a day-one registry feature, not a late addition. Users who set
up their identity on a laptop and then want to check from their phone should not
be blocked. Adding a second key requires authentication with an existing key.

Instances resolve handles via the registry API (`GET
/api/v1/accounts/by-handle/:handle`), but they cache the pubkey->handle mapping
locally. The registry is not in the hot path for any request after initial
resolution.

### Cross-Instance Identity Proofs

When a user is a member of multiple instances, each instance has a separate
identity row. The registry links them via `account_id`, but this requires the
registry to be reachable. For S-tier interconnect, identity linkage must work
without the registry.

**Self-issued identity proofs** are signed statements linking a user's
identities across instances:

```
IdentityProof {
    subject: PublicKey,              // the key doing the proving
    instance: NodeId,                // which instance this identity lives on
    related_keys: Vec<PublicKey>,    // other keys belonging to the same person
    registry_handle: Option<String>, // optional, for display
    timestamp: u64,
    signature: Signature,            // subject signs all fields
}
```

These proofs are verifiable by anyone with the subject's public key. They don't
require the registry. They enable:

- **Cross-instance reputation**: "This user is a trusted admin on 3 other
  instances"
- **Portable identity display**: Instance A shows that a user is also `@alex` on
  Instance B, without asking the registry
- **Offline federation**: Two instances can verify identity relationships even
  if the registry is unreachable

Identity proofs are **assertions, not guarantees**. An instance receiving a
proof can verify the signature (the subject really did claim this linkage) but
cannot verify that the subject actually has an active grant on the claimed
instance without contacting that instance or the registry. The proof is "I claim
these keys are mine" — the consuming instance decides how much weight to give
that claim.

### Identity Layers and the Noun Model

The system has three distinct identity layers. Understanding their relationships
is critical for the invite and authorization model.

```
Layer 3: Human Identity (implicit, never modeled)
    ↕  one human → many external accounts, many keypairs
Layer 2: External Identities (GitHub, Google Workspace, OIDC, email)
    ↕  many-to-many with keypairs (via registry identity bindings)
Layer 1: Account Identity (ed25519 keypairs)
    ↕  grants are always here
Layer 0: Instance Membership (grants, access rights, state)
```

**Layer 1 (keypairs)** is the only layer that touches authorization. Grants,
sessions, and access rights are always keypair-based. This is inviolable.

**Layer 2 (external identities)** exists only in the registry, as metadata
about keypairs. The registry maintains **identity bindings**: signed assertions
that "pubkey A is bound to github:foo" or "pubkey B is bound to
google:alice@acme.com". These bindings are established when a user links an
external account (OAuth flow or OIDC) and are attested by the registry's
signature.

**Layer 3 (human identity)** is never modeled explicitly. A person may have
multiple keypairs, multiple GitHub accounts, multiple email addresses — the
system doesn't try to unify them. The DAG between keypairs and external
accounts is the closest the system gets to representing "a person," and it's
always opt-in.

The **noun model** bridges layers 1 and 2 for the invite system. People think
in nouns — `@alex`, `github:foo`, `google:alice@acme.com`, `email:bob@bar.com`
— not in base32-encoded public keys. The invite system resolves nouns to
keypairs at invite time through the registry, then issues a standard
keypair-based invite. At runtime, nouns don't exist — only keys.

Noun vocabulary:

| Noun format | Example | Resolution |
|-------------|---------|------------|
| `@handle` | `@alex` | Registry handle → account → active pubkeys |
| `github:<username>` | `github:foo` | Registry identity binding → account → active pubkeys |
| `google:<email>` | `google:alice@acme.com` | Registry identity binding → account → active pubkeys |
| `email:<address>` | `email:bob@bar.com` | Registry identity binding → account → active pubkeys |

All noun resolution goes through the registry. If the registry is unavailable,
noun-based invites cannot be created (but raw keypair invites always work).

### OIDC Binding (Enterprise Layer)

Enterprise users authenticate via their corporate IdP. `crabcity.dev` acts as an
OIDC relying party (consuming Okta/Entra/Google Workspace tokens) and an OIDC
provider (issuing tokens to instances).

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

Instances only need to trust one OIDC issuer: `https://crabcity.dev`. They never
configure per-enterprise IdPs.

## Authorization Model

### Capabilities and Access Rights

Authorization is modeled as **access rights** inspired by
[GNAP (RFC 9635)](https://www.rfc-editor.org/rfc/rfc9635.html) Section 8. An
access right describes what a member can do:

```json
{ "type": "terminals", "actions": ["read", "input"] }
```

`type` identifies the resource kind. `actions` lists permitted operations on
that resource. A grant is a JSON array of access rights — this is the sole
authorization primitive and the source of truth.

Capabilities are named presets that expand to well-known access right arrays:

| Capability    | Access Rights |
|---------------|---------------|
| `view`        | `content:read`, `terminals:read` |
| `collaborate` | view + `terminals:input`, `chat:send`, `tasks:read,create,edit`, `instances:create` |
| `admin`       | collaborate + `members:read,invite,suspend,reinstate,remove,update` |
| `owner`       | admin + `instance:manage,transfer` |

The API surface uses capabilities (`"capability": "collaborate"`) for
simplicity. The database stores the expanded access rights array. Admins can
tweak individual access rights on a grant after redemption (e.g., "collaborate
but no terminal input" → remove the `terminals:input` action).

This model is extensible: adding a new resource type or action means adding a
new object to the array, not defining a new bit position. If the initial set of
access rights turns out to be wrong, they can be revised without a schema
migration — the JSON array is the source of truth, and capabilities are just
presets that happen to expand to it.

Invite tokens carry a `Capability` enum (1 byte, compact). The expansion to
access rights happens at redemption time.

### Capability Algebra

Access rights support a formal algebra — four operations that are the **only**
way to manipulate access rights throughout the codebase:

| Operation | Use case | Semantics |
|-----------|----------|-----------|
| `intersect(a, b)` | Scoped sessions | "what can I do with this token?" = requested ∩ granted |
| `contains(type, action)` | Authorization checks | "does this session allow this action?" = required ⊆ scope |
| `is_superset_of(other)` | Capability narrowing | "can this invite grant this?" = invite.cap ⊆ issuer.cap |
| `diff(old, new)` | Access tweaking, audit | "what changed?" = (added, removed) |

These operations are property-tested:
- `intersect` is commutative and idempotent
- `intersect(a, b).is_superset_of(c)` implies `a.is_superset_of(c) && b.is_superset_of(c)`
- `Owner.access_rights().is_superset_of(Admin.access_rights())` for all preset orderings

No code outside `crab_city_auth` performs ad-hoc iteration over access rights
arrays. Authorization checks go through `contains()`, scoping goes through
`intersect()`, delegation validation goes through `is_superset_of()`. This
eliminates a class of bugs where different code paths implement the same logic
differently.

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

The auth middleware checks `state == active`, period. No multi-table joins
against blocklists in the hot path — blocklist hits transition grants to
`suspended` (with `reason` recorded in the event log), and the middleware only
needs to read one column.

### Identity and Authorization: Separate Tables

Instance-local data is split into two concerns:

**`member_identities`** — WHO you are. Cached from registry or self-reported.
Updated by registry resolution, user profile changes. Broadcast as
`IdentityUpdate`.

**`member_grants`** — WHAT you can do. Instance-local authorization. Updated by
admin actions, invite redemption, blocklist enforcement. Broadcast as
`GrantUpdate`.

This separation means:
- Updating a display name doesn't touch the authorization table
- Changing a capability doesn't re-resolve identity
- The broadcast for "Alex changed their avatar" is a different message type than
  "Alex was promoted to admin"
- Identity resolution (slow, async, registry-dependent) is decoupled from
  authorization checks (fast, local, synchronous)

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

Invites are serialized, base32-encoded, and distributed out-of-band (URL, chat,
email). They require no registry involvement.

**Invite revocation semantics:** Revoking an invite revokes *unredeemed uses
only*. Existing memberships created from that invite are not affected. To
suspend members who joined via a specific invite, use the separate "revoke
invite and suspend derived members" admin action. This requires tracing
invite->member relationships via the `invited_via` field on grants.

### Invite Delegation Chains

Invites support **delegation**: a member who received an invite can sub-delegate
it to others, creating a cryptographic chain of authority.

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
1. Root issuer must be a member with `members:invite` access on the instance
2. Each link's capability must be <= previous link's capability (capabilities
   can only narrow)
3. Each link's depth must be < previous link's remaining depth
4. Each signature is valid over (previous link hash + current fields)

The token is a self-contained proof of authorization — the instance can verify
the entire delegation without having seen any intermediate step. This enables
viral invite distribution: power users become invite distributors without admin
intervention, bounded by `max_depth`.

A flat (non-delegated) invite is just a chain of length 1. The v1 invite format
is a degenerate case of the delegation chain.

### Noun-Based Invites

Raw invites require the inviter to know the invitee's public key (or share a
link out-of-band). Noun-based invites let the inviter speak in terms they
already know: "invite my friend `github:foo`" or "add `@blake`."

The flow:

```
Instance Admin                     Registry                    Invitee
     |                                |                           |
     |  POST /api/invites/by-noun     |                           |
     |  { noun: "github:foo",         |                           |
     |    capability: "collaborate" }  |                           |
     | -----------------------------> |                           |
     |                                |  resolve github:foo       |
     |                                |  → account + pubkeys      |
     |                                |                           |
     |  (A) Resolved: account exists  |                           |
     |  { status: "resolved",         |                           |
     |    account, pubkeys,           |                           |
     |    attestation }               |                           |
     | <-----------------------------  |                           |
     |                                |                           |
     |  create invite for pubkey      |                           |
     |  broadcast invite notification |                           |
     |  via heartbeat                 |                           |
     |                                |  heartbeat response:      |
     |                                |  resolved_invites: [...]  |
     |                                | -----------------------> |
     |                                |                           |  invitee sees
     |                                |                           |  pending invite
```

```
     |  (B) Pending: no account yet   |                           |
     |  { status: "pending" }         |                           |
     | <-----------------------------  |                           |
     |                                |                           |
     |  instance stores pending noun  |                           |
     |                                |                           |
     |              ... time passes, invitee signs up ...         |
     |                                |                           |
     |                                |  invitee creates account  |
     |                                |  links github:foo         |
     |                                |  registry resolves        |
     |                                |  pending invites          |
     |                                |                           |
     |  heartbeat response:           |                           |
     |  resolved_invites: [...]       |                           |
     | <-----------------------------  |                           |
     |                                |                           |
     |  create invite for resolved    |                           |
     |  pubkey, notify invitee        |                           |
```

**Design constraints:**

1. **Grants stay pubkey-only.** Noun resolution happens once, at invite time.
   The resulting grant is bound to a pubkey. If the invitee rotates keys, the
   grant follows the key (not the noun).

2. **The registry is the phonebook.** It resolves nouns to accounts and
   pubkeys. It attests identity bindings with its signature. Instances trust
   the registry's attestation at invite time but do not depend on it at
   runtime.

3. **Pending invites live at the registry.** When the noun doesn't resolve
   (person not yet on crabcity), the registry holds a pending invite record.
   When the person signs up and links the matching external identity, the
   registry resolves the pending invite and delivers it via the next heartbeat.

4. **Key loss recovery is re-invite.** If someone loses their key, the admin
   re-invites the same noun. The registry resolves to the person's new pubkey
   (they've registered new keys). The old grant is replaced via the existing
   `replace` flow. The noun is the stable human-friendly identifier; the key
   is the ephemeral (but authoritative) runtime identity.

### Event Log (Verifiable Audit Trail)

Every state transition on an instance produces an event. Events are
**hash-chained**: each event includes the SHA-256 hash of the previous event,
forming a tamper-evident log.

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
- **Tamper evidence** — modifying or deleting any event breaks the chain. A
  sequential scan can verify integrity.
- **Signed checkpoints** — every N events (configurable, default 100), the
  instance signs the current chain head hash with its NodeId key. This means
  even the instance operator cannot silently tamper with the log.
- **Cross-instance audit** — a signed checkpoint is a portable proof of log
  integrity. An admin can present a signed chain head to prove their instance's
  event history is untampered.
- **Merkle proofs** — users can request inclusion proofs that a specific event
  (e.g., their `member.joined`) exists in the chain.

Event types:
- `member.joined` — new identity + grant created (invite redemption, OIDC, loopback)
- `member.suspended` — grant state -> suspended (admin action or blocklist)
- `member.reinstated` — grant state -> active (admin action)
- `member.removed` — grant state -> removed
- `member.replaced` — new grant linked to old one (key loss recovery)
- `grant.capability_changed` — capability or access rights updated
- `grant.access_changed` — individual access rights modified
- `invite.created` — new invite issued
- `invite.redeemed` — invite used (links to member.joined)
- `invite.revoked` — invite revoked
- `invite.noun_created` — noun-based invite created (resolved or pending)
- `invite.noun_resolved` — pending noun invite resolved (person signed up)
- `identity.updated` — display name, handle, or avatar changed

The event log is append-only, never mutated. It serves as:
1. **Verifiable audit trail** — "who invited who, when was someone promoted" —
   with cryptographic tamper evidence
2. **Debug tool** — trace the provenance of any membership
3. **Undo mechanism** — admins can review and reverse actions
4. **Future: activity feed** — surface meaningful events in the UI

### Formal State Machine Verification

The membership state machine is small enough to verify exhaustively with a model
checker (TLA+ or Alloy):

```
States: {invited, active, suspended, removed}
Transitions: {join, suspend, reinstate, remove, replace, blocklist_hit, blocklist_lift}
Invariants:
  - removed is terminal (no transitions out)
  - suspend/reinstate only from active/suspended respectively
  - blocklist_lift only restores if original suspension was blocklist-sourced
  - capability can only be changed in active state
  - replace creates new grant, transitions old to removed
```

The model proves that no sequence of transitions violates the invariants. Test
cases are generated from the model to ensure the Rust implementation matches.
The state machine is the correctness kernel — if it's wrong, everything built on
top is wrong.

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

`Public` instances appear in the directory. `Unlisted` instances are accessible
by direct link. `Private` instances are only visible to org members.

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
     resolved_invites: [
       {
         noun: "github:foo",
         account_id: "...",
         pubkeys: ["<base64>", ...],
         attestation: "<base64>"
       }
     ],
     motd: null
   }
```

Blocklist deltas are **scoped** — the response includes separate delta arrays
for the global blocklist and each org the instance is bound to. This prevents
ambiguity when an instance belongs to multiple orgs.

The heartbeat serves four purposes:
1. **Liveness** — the registry marks instances as offline after missed
   heartbeats
2. **Blocklist sync** — scoped delta-encoded blocklist updates piggybacked on
   the heartbeat response
3. **Noun invite resolution** — when a pending noun invite resolves (the
   person signed up and linked the matching external identity), the resolved
   account and pubkeys are delivered in the heartbeat response
4. **Announcements** — registry-to-instance communication channel (MOTD,
   deprecation notices)

This is the ONLY protocol between instance and registry during steady-state
operation.

**Known property:** Blocklist enforcement has a propagation window of up to 5
minutes (one heartbeat interval). If a user is blocked globally, instances that
haven't heartbeated yet will still allow access until their next heartbeat. This
is acceptable at the expected traffic level and is documented as an explicit
design property, not a bug.

## Blocklists

Three scopes:

| Scope    | Maintained by       | Enforced by         | Distribution          |
|----------|---------------------|---------------------|-----------------------|
| Global   | crabcity.dev admins | opt-in by instances | heartbeat delta       |
| Org      | org admins          | org instances       | heartbeat delta       |
| Instance | instance admins     | that instance       | local (no sync)       |

Blocklist entries target either a `PublicKey` (user) or `NodeId` (instance) or
`IpRange`.

Instances opt into global blocklist enforcement. This is a social contract, not
a technical lock: a rogue instance can ignore the blocklist, but it can be
delisted from the directory.

When a blocklist entry hits an active member, the instance transitions that
member's grant to `suspended` and logs a `member.suspended` event with the
blocklist scope and reason. This is a state transition, not a runtime check —
the auth middleware only needs `state == active`.

## Wire Format Versioning

All WebSocket messages use envelope versioning with monotonic sequence numbers:

```json { "v": 1, "seq": 4817, "type": "GrantUpdate", "data": { ... } } ```

The `seq` field is a per-connection monotonic counter assigned by the server.
Clients track their last-seen `seq` for reconnection (see below).

Clients ignore messages with versions they don't understand. This allows
individual message types to evolve independently without breaking connected
clients. The envelope version (`v`) is the protocol version, not a
per-message-type version — all messages in protocol v1 share the same contract.

HTTP API versioning uses URL path prefixes (`/api/v1/...` for registry, no
prefix for instance-local APIs in M1). Breaking changes increment the path
version. Non-breaking additions (new optional fields) don't require a version
bump.

### Reconnection Protocol

WebSocket connections drop constantly: WiFi-to-cellular transitions, laptop lid
close/open, network hiccups. Reconnection must be invisible to users.

On reconnect, the client sends its last-seen `seq` in the WebSocket handshake
(query parameter: `?last_seq=4817`). The server replays missed messages from a
bounded per-connection ring buffer (last 1000 messages or last 5 minutes,
whichever is smaller). If the gap is too large, the server sends a full state
snapshot instead (same payload as initial connection).

The server sends WebSocket ping frames every 30 seconds. If the client doesn't
respond within 10 seconds, the server closes the connection and cleans up
presence state. Without this, ghost users appear online for minutes after
they've actually disconnected.

### Heartbeat and Presence Cleanup

| Timer | Interval | Action on miss |
|-------|----------|----------------|
| WS ping | 30s | Close connection, remove presence |
| Session token expiry | 15 min | Client must refresh |
| Refresh token expiry | 24 hours | Client must re-authenticate |
| Registry heartbeat | 5 min | Instance marked offline after 3 misses |

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

Orgs group accounts and instances. When an org has OIDC configured, new members
are auto-provisioned when they first SSO through `crabcity.dev`. The org admin
controls which instances their members have access to and at what capability
level.

## Security Boundaries

| Boundary | Trust model |
|----------|-------------|
| User <-> Instance | Challenge-response (user proves keypair ownership) or session token |
| Instance <-> Registry | Instance authenticates via its NodeId keypair; registry authenticates via TLS + OIDC signing key |
| User <-> Registry | OIDC tokens (for SSO) or challenge-response (for keypair-native users) |
| Instance <-> Instance | No direct trust required; iroh handles transport encryption |

### Challenge-Response Protocol (Stateless)

The challenge-response is **fully stateless** on the server side. No pending
challenges are stored in memory or in a database.

The server generates a **signed challenge token** that encodes all challenge
state:

```
challenge_token = instance_sign("crabcity:challenge:v1:" ++ nonce ++ client_pubkey ++ scope_hash ++ issued_at ++ expires_at)
```

The client receives `{ nonce, challenge_token, expires_at }`, signs the
structured payload, and sends back `{ public_key, nonce, challenge_token,
signature }`. The server verifies:

1. Its own signature on `challenge_token` (proves it issued this challenge)
2. The client's signature on `"crabcity:auth:v1:" ++ nonce ++ instance_node_id
   ++ client_timestamp` (proves key ownership)
3. `expires_at` hasn't passed
4. `public_key` matches the one encoded in the challenge token

Properties:
- **Zero server-side state.** No `DashMap`, no cleanup sweeps, no TTL
  management.
- **Survives restarts.** Outstanding challenges remain valid across instance
  restarts (as long as the instance signing key is the same).
- **Horizontally scalable.** No sticky sessions or shared stores needed for
  multi-process deployments.
- **Replay-safe.** The nonce is embedded in both the challenge token and the
  client signature. Session creation is idempotent on `(pubkey, nonce)`, so
  replaying the same verify request is harmless (returns the same session).

The `crabcity:auth:v1:` prefix in the client's signed payload prevents
cross-protocol confusion. The `instance_node_id` prevents cross-instance replay.
The challenge token TTL (60 seconds) bounds the window.

### Key Rotation

- **User keypairs**: Users can register multiple keys to the same registry
  account. Old keys can be deprecated without losing access.
- **Registry OIDC signing keys**: Published via JWKS endpoint
  (`/.well-known/jwks.json`). Support multiple active keys. Rotate on a fixed
  schedule (90 days). Instances cache JWKS with a TTL.
- **Instance NodeId keys**: Tied to iroh identity. Rotation requires
  re-registration at the registry.

### Key Transparency

The registry maintains a **verifiable log** of all key binding operations
(account creation, key addition, key revocation). This log is a Merkle tree —
any entry's inclusion can be proven with a compact proof.

Properties:
- The tree head is signed by the registry and published periodically (on every
  mutation)
- **Monitors** (instances, public auditors, or the users themselves) can watch
  the log for unauthorized key bindings
- Users can audit their own account at any time: "show me every key that has
  ever been bound to `@alex`"
- If the registry is compromised and a rogue key is bound to an account, any
  monitor watching the log will detect it

This makes the "registry is a phonebook, not a platform" principle
*cryptographically enforceable*. Enterprise customers don't have to trust the
registry operator — their instances can independently verify that key bindings
haven't been tampered with.

### Sessions as Signed Capabilities

Session tokens are **self-contained signed documents**, not database row
references. The instance signs the session with its own key; the middleware
verifies the signature on every request. No database lookup on the hot path.

```
SessionToken = instance_sign({
    public_key: PublicKey,
    scope: Vec<AccessRight>,       // intersection of requested and granted
    capability: Capability,        // for display
    grant_version: u64,            // monotonic counter on the grant row
    issued_at: DateTime,
    expires_at: DateTime,          // short-lived: 15 minutes
})
```

A separate **refresh token** (32 random bytes, stored hashed in SQLite) is used
to mint new session tokens. The refresh endpoint checks grant state, so
revocation takes effect within one refresh cycle (at most 15 minutes). This is
the same order of magnitude as blocklist propagation delay — an acceptable and
documented property.

For **immediate revocation** (admin suspends a user mid-session), the instance
maintains a small in-memory revocation set of `(pubkey, grant_version)` pairs.
The middleware checks this set (O(1) hash lookup, no IO). The set is populated
from the broadcast channel: when a `GrantUpdate` with `state=suspended` fires,
the entry is added. Entries expire when the corresponding session tokens expire,
so the set never grows large.

Per-request auth cost: ed25519 signature verification (~60μs) + revocation set
lookup (O(1)). No SQLite on the hot path.

Sessions carry an explicit **access scope** — the intersection of what the
client requested and what the underlying grant allows. A CLI tool that only
needs to read tasks can request a `content:read`-only session, limiting blast
radius if the token leaks.

Scoped sessions are backward-compatible: omit the `scope` parameter in the
challenge request and you get the full grant access rights.

## Join Experience

The zero-to-collaborating flow is a first-class design artifact, not an
implementation afterthought:

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
- **Live preview**: a small, blurred/abstracted terminal window showing
  real-time activity (cursor movement, user count, terminal dimensions — no
  content). This communicates "this is an active, living place" before the user
  commits to joining.
- Number of users currently online (live-updating via WebSocket)
- Single input: "Your name" (pre-filled if they have a registry account)
- Single button: "Join"
- Below the fold: "This will create a cryptographic identity on your device.
  [Learn more]"

If Blake already has a keypair in IndexedDB for this instance: skip the name
prompt, show "Welcome back, Blake. [Rejoin]"

The live preview uses a dedicated `preview` WebSocket stream that requires no
authentication. It delivers only: terminal dimensions, cursor position (no
content), user count, and instance uptime. This is the "looking through the
restaurant window" experience — enough to see activity, not enough to see data.

### Key Backup (Blocking Modal)

After first keypair generation, the user sees a **blocking modal** (not a
dismissable toast):

- "Save your identity key" — explanation that this is the only copy
- Copy-to-clipboard button (base64-encoded private key)
- Download `.key` file button
- "I saved my key" checkbox required to proceed

This is modeled after TOTP recovery code flows. The modal cannot be dismissed
without confirming. The UX is deliberately inconvenient because the consequence
of key loss (identity loss) is severe.

### Invite QR Codes

A flat invite is 160 bytes (256 chars base32) — well within QR code capacity (up
to 4296 alphanumeric chars). Delegated invites (3-hop chain, 412 bytes, 660
chars) also fit. QR codes are the highest-bandwidth in-person sharing mechanism:
workshops, conferences, pairing sessions, meetups.

The TUI renders invites as QR codes using Unicode block characters (half-block
encoding, no external dependencies). The web UI renders them as SVG images. The
invite creation response includes a `qr_data` field with the pre-encoded
alphanumeric payload.

The viral distribution path (Alice gives Bob a delegated invite at a meetup) is
exactly the scenario where QR codes shine — no copy-paste, no URL sharing, no
registry involvement.

### Key Loss Recovery

When a known user loses their key and can't access the instance:

1. User contacts admin out-of-band ("I lost my key")
2. Admin re-invites by noun: `/invite github:foo` or `/invite @blake`
3. Registry resolves the noun to the user's new pubkey (they've registered new
keys on their new device)
4. User receives the invite, redeems it → new keypair, new grant
5. Admin **links** the new grant to the old one ("This is the same person")
6. The old grant transitions to `removed(replaced_by=new_pubkey)`
7. The UI merges attribution: chat messages, task assignments, and terminal
history from the old key are displayed under the new identity

The noun is the stable, human-friendly identifier that survives key loss. The
admin doesn't need to know or care about the new pubkey — they just re-invite
the same noun, and the registry resolves it to whatever keys the person
currently has.

For instances without registry integration, the raw re-invite + replace flow
still works (admin sends a new invite link out-of-band).

The `replaces` field on `member_grants` enables this linking. The event log
records a `member.replaced` event for auditability.

### Iroh-Native Invite Exchange

Invites are transport-agnostic signed blobs. In addition to URL-based
distribution, invites can be exchanged **directly via iroh** — no URL, no
side-channel, no registry.

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
1. Alice's instance publishes a short-lived iroh document with the invite token,
   discoverable via mDNS or iroh's DHT
2. Bob's client discovers it, presents it to the user for confirmation
3. Bob's client redeems the invite directly via iroh transport (no HTTP needed)
4. The invite document self-destructs after redemption or timeout

This is the **zero-infrastructure invite path**: two devices on the same network
(or reachable via iroh relay) and you're in. It's a pure client feature — the
invite token format is the same, only the transport differs.

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
| Noun-based invite (resolved) | Admin -> Instance -> Registry -> Instance | Once per invite |
| Noun-based invite (pending) | Admin -> Instance -> Registry ... Registry -> Instance (heartbeat) | Once per invite, deferred |
| OIDC SSO login | User -> Enterprise IdP -> Registry -> Instance | Once per session |
| Instance heartbeat | Instance -> Registry | Every 5 min |
| Handle/profile resolution | Instance -> Registry | Cached, infrequent |
| Noun resolution | Instance -> Registry | At invite time only |
| Blocklist sync | Registry -> Instance (via heartbeat) | Every 5 min |
| Resolved invite delivery | Registry -> Instance (via heartbeat) | When pending invites resolve |

## Idempotency

Every mutation endpoint handles "request succeeded, response lost, client
retries." This is the most common distributed failure mode.

| Endpoint | Idempotency key | Behavior on retry |
|----------|----------------|-------------------|
| `POST /api/invites/redeem` | `(invite_nonce, public_key)` | Returns existing grant if already redeemed by this key |
| `POST /api/auth/challenge` | Stateless | Each call returns a new signed challenge token; no server state |
| `POST /api/auth/verify` | `(pubkey, nonce)` | Returns existing session if already verified with this nonce |
| `POST /api/invites` | Client-supplied idempotency key | Returns existing invite if key matches |
| `POST /api/auth/refresh` | `(refresh_token)` | Returns new session token (idempotent by nature — old session tokens remain valid until expiry) |

Event logging is serialized via SQLite transactions: `BEGIN → read prev_hash →
compute new hash → INSERT → COMMIT`. No concurrent writers to the hash chain.

## Observability

### Metrics (Prometheus)

Every instance and registry exposes a `/metrics` endpoint:

- `crabcity_auth_challenges_total{result}` — success, invalid_sig, expired,
  no_grant
- `crabcity_sessions_active` (gauge)
- `crabcity_sessions_created_total`, `crabcity_sessions_expired_total`
- `crabcity_invites_redeemed_total{capability}` — view, collaborate, admin
- `crabcity_grants_by_state{state}` (gauge) — invited, active, suspended,
  removed
- `crabcity_event_log_size` (gauge)
- `crabcity_registry_heartbeat_latency_seconds` (histogram)
- `crabcity_registry_heartbeat_failures_total`
- `crabcity_ws_connections_active` (gauge)
- `crabcity_ws_reconnections_total`
- `crabcity_blocklist_sync_version{scope}` (gauge)

### Structured Logging

Every auth decision (success or failure) emits a structured log line:
`public_key_fingerprint`, `endpoint`, `result`, `reason`, `session_scope`,
`duration_ms`. This is the forensic trail when something goes wrong.

### Distributed Tracing

When a request hits the instance, gets forwarded to the registry for handle
resolution, and comes back — that's a distributed trace. OpenTelemetry trace
context propagation on the registry HTTP client links the spans.

## Structured Error Recovery

Every error response includes a machine-actionable `recovery` field. Clients
never have to guess what to do next:

```json
{
    "error": "session_expired",
    "message": "Session token expired",
    "recovery": { "action": "refresh", "refresh_url": "/api/auth/refresh" }
}
```

```json
{
    "error": "rate_limited",
    "message": "Too many requests",
    "recovery": { "action": "retry", "retry_after_secs": 42 }
}
```

```json
{
    "error": "grant_not_active",
    "message": "Your membership is suspended",
    "recovery": {
        "action": "contact_admin",
        "admin_fingerprints": ["crab_2K7XM9QP"],
        "reason": "Blocklist match (org:acme-corp)"
    }
}
```

Recovery actions are a closed enum: `refresh`, `reauthenticate`, `retry`,
`contact_admin`, `redeem_invite`. The client SDK parses these into typed
actions. The frontend handles every action with a specific UI flow. No generic
"something went wrong" screens.

## Non-Goals

- **Federated social graph.** The registry is not ActivityPub.
- **Runtime traffic proxying.** The registry never proxies user<->instance
  traffic.
- **Central message storage.** Chat, terminal sessions, tasks — all
  instance-local.
- **Universal search.** You cannot search across instances from the registry.
- **Payment processing.** Billing, if it ever exists, is a separate concern.
