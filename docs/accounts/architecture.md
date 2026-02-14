# Crab City Accounts: Architecture

## Overview

Crab City Accounts is a distributed identity and authorization system for Crab City instances. It combines **local cryptographic identity** (ed25519 keypairs) with an **optional central registry** at `crabcity.dev` that provides discovery, public profiles, OIDC-based enterprise SSO, and global moderation.

Every instance operates with full sovereignty. The registry adds value (handles, profiles, SSO, blocklists) without adding lock-in.

## Principles

1. **Identity is a keypair.** An ed25519 key pair IS the account. No usernames, no passwords, no email required at the base layer.
2. **Invites are signed capabilities.** Access control is a signed document, not a row in a central database.
3. **Instances are sovereign.** An instance can operate standalone forever. The registry is opt-in.
4. **The registry is a phonebook, not a platform.** It stores metadata and coordinates discovery. It does not mediate runtime traffic between users and instances.
5. **Enterprise features layer on, not replace.** OIDC/SSO binds to the same keypair identity. Org management is sugar over the same membership model.

## System Topology

```
┌──────────────────────────────────────────────────────────┐
│                     crabcity.dev                         │
│                   (central registry)                     │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │  Accounts &  │  │   Instance   │  │   OIDC Provider│  │
│  │  Profiles    │  │   Directory  │  │   + RP         │  │
│  └──────────────┘  └──────────────┘  └────────────────┘  │
│  ┌─────────────┐  ┌──────────────┐                       │
│  │ Blocklists  │  │  Org / Team  │                       │
│  │             │  │  Management  │                       │
│  └─────────────┘  └──────────────┘                       │
└──────────┬───────────────┬───────────────┬───────────────┘
           │ heartbeat     │ OIDC          │ attestation
           │ (pull)        │ (auth flow)   │ (push)
           ▼               ▼               ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  Instance A  │  │  Instance B  │  │  Instance C  │
│ (standalone) │  │ (registered) │  │ (org-managed)│
│              │  │              │  │              │
│  ┌────────┐  │  │  ┌────────┐  │  │  ┌────────┐  │
│  │ SQLite │  │  │  │ SQLite │  │  │  │ SQLite │  │
│  │members │  │  │  │members │  │  │  │members │  │
│  └────────┘  │  │  └────────┘  │  │  └────────┘  │
└──────────────┘  └──────────────┘  └──────────────┘
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

### Registry Account (Optional Layer)

When a user registers at `crabcity.dev`, they bind their keypair to a handle:

```
Account {
    id: Uuid,
    public_key: ed25519::PublicKey,
    handle: String,                   // @alex — unique on the registry
    display_name: String,
    avatar_url: Option<String>,
    email: Option<String>,            // for OIDC binding / recovery
    oidc_bindings: Vec<OidcBinding>,
    created_at: DateTime,
    blocked: bool,
}
```

Instances resolve handles via the registry API (`GET /api/v1/accounts/by-handle/:handle`), but they cache the pubkey→handle mapping locally. The registry is not in the hot path for any request after initial resolution.

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
       │
       │  OIDC auth code flow
       ▼
crabcity.dev (Relying Party)
       │
       │  maps to keypair, issues crabcity.dev OIDC token
       ▼
crabcity.dev (Provider)
       │
       │  OIDC auth code flow (instance is the RP now)
       ▼
Crab City Instance
       │
       │  extracts pubkey + org claims, creates local membership
       ▼
Local SQLite membership table
```

Instances only need to trust one OIDC issuer: `https://crabcity.dev`. They never configure per-enterprise IdPs.

## Authorization Model

### Capabilities

A capability is what a user can do on an instance. Capabilities are granted via invites or OIDC claims:

| Capability    | Description                                    |
|---------------|------------------------------------------------|
| `view`        | Read-only access to instance content           |
| `collaborate` | Create/edit content, join terminals            |
| `admin`       | Manage instance settings, invite others, moderate |
| `owner`       | Full control, transfer ownership               |

Capabilities are hierarchical: `owner` ⊃ `admin` ⊃ `collaborate` ⊃ `view`.

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

### Memberships (Instance-Local)

Regardless of how a user authenticated (raw invite, registry invite, OIDC), the instance stores a uniform membership record:

```
Membership {
    public_key: PublicKey,         // user identity
    capability: Capability,
    display_name: String,
    handle: Option<String>,        // from registry, if available
    org_id: Option<Uuid>,          // from OIDC claims, if available
    invited_by: Option<PublicKey>,
    created_at: DateTime,
}
```

The membership table is the single source of truth for authorization on each instance. The auth middleware checks this table, not the registry.

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
→ { node_id, version, user_count, public_metadata }
← { blocklist_version, blocklist_delta, motd }
```

The heartbeat serves three purposes:
1. **Liveness** — the registry marks instances as offline after missed heartbeats
2. **Blocklist sync** — delta-encoded blocklist updates piggybacked on the heartbeat response
3. **Announcements** — registry-to-instance communication channel (MOTD, deprecation notices)

This is the ONLY protocol between instance and registry during steady-state operation.

## Blocklists

Three scopes:

| Scope    | Maintained by       | Enforced by         | Distribution          |
|----------|---------------------|---------------------|-----------------------|
| Global   | crabcity.dev admins | opt-in by instances | heartbeat delta       |
| Org      | org admins          | org instances       | heartbeat delta       |
| Instance | instance admins     | that instance       | local (no sync)       |

Blocklist entries target either a `PublicKey` (user) or `NodeId` (instance) or `IpRange`.

Instances opt into global blocklist enforcement. This is a social contract, not a technical lock: a rogue instance can ignore the blocklist, but it can be delisted from the directory.

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
| User ↔ Instance | Challenge-response (user proves keypair ownership) or session token |
| Instance ↔ Registry | Instance authenticates via its NodeId keypair; registry authenticates via TLS + OIDC signing key |
| User ↔ Registry | OIDC tokens (for SSO) or challenge-response (for keypair-native users) |
| Instance ↔ Instance | No direct trust required; iroh handles transport encryption |

### Key Rotation

- **User keypairs**: Users can register multiple keys to the same registry account. Old keys can be deprecated without losing access.
- **Registry OIDC signing keys**: Published via JWKS endpoint (`/.well-known/jwks.json`). Support multiple active keys. Rotate on a fixed schedule (90 days). Instances cache JWKS with a TTL.
- **Instance NodeId keys**: Tied to iroh identity. Rotation requires re-registration at the registry.

## Data Flow Summary

| Flow | Path | Frequency |
|------|------|-----------|
| Raw invite redemption | User → Instance | Once per invite |
| Registry invite redemption | User → Registry → Instance | Once per invite |
| OIDC SSO login | User → Enterprise IdP → Registry → Instance | Once per session |
| Instance heartbeat | Instance → Registry | Every 5 min |
| Handle/profile resolution | Instance → Registry | Cached, infrequent |
| Blocklist sync | Registry → Instance (via heartbeat) | Every 5 min |

## Non-Goals

- **Federated social graph.** The registry is not ActivityPub.
- **Runtime traffic proxying.** The registry never proxies user↔instance traffic.
- **Central message storage.** Chat, terminal sessions, tasks — all instance-local.
- **Universal search.** You cannot search across instances from the registry.
- **Payment processing.** Billing, if it ever exists, is a separate concern.
