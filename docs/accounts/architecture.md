# Interconnect Architecture

## Overview

Crab City interconnect lets two people running separate Crab City instances
connect them over iroh. Alice invites Bob. Bob sees Alice's terminals, chat,
tasks — the full Crab City experience — as a switchable context in his TUI.
The transport is iroh QUIC. The protocol is the same `ClientMessage`/
`ServerMessage` the WebSocket layer already speaks. Authorization is
per-account. No registry required.

## Principles

1. **Account identity is a keypair.** An ed25519 keypair IS the account. No
   usernames, no passwords, no email at the base layer.
2. **Federation is per-user, not per-instance.** Bob invites Alice, not
   "everyone on Alice's server." Alice's teammate Dave gets nothing unless Bob
   separately invites Dave.
3. **Each direction is independent.** Bob inviting Alice does NOT mean Alice
   invited Bob. The two grants are independent with potentially different
   access levels.
4. **Same protocol everywhere.** Remote hosts speak the same `ClientMessage`/
   `ServerMessage` protocol that local WebSocket clients already speak. No new
   message types for the core experience.
5. **Instances are sovereign.** An instance operates standalone forever. The
   registry (future) is opt-in.
6. **The token is the UX.** Connection tokens are self-describing (carry
   metadata), self-verifying (signed by the instance), and self-contained (no
   network needed to read them).

## System Topology

```
Alice's TUI ──ws──► Alice's Instance ──iroh──► Bob's Instance
                    (home side)                 (host side)

Alice's Browser ──ws──► Alice's Instance ──iroh──► Bob's Instance
                        (same proxy)                (same host)
```

No iroh WASM required. Browsers stay on WebSocket. The home instance proxies
to remote hosts over iroh. Browser clients never touch iroh directly.

Local/browser connections keep session-based auth. iroh is the native CLI/TUI
and instance-to-instance transport. Both auth paths coexist.

## Identity Model

### Ed25519 Keypair

Every user's identity is an ed25519 keypair. The public key is the canonical
account identifier. Keypairs are generated client-side and never leave the
device.

```
UserIdentity {
    public_key: ed25519::PublicKey,   // THE identity
    display_name: String,             // mutable, user-chosen
    created_at: u64,
}
```

### Key Fingerprints

Public keys get a human-readable fingerprint: `crab_` prefix + first 8
characters of the Crockford base32 encoding.

Example: `crab_2K7XM9QP`

Fingerprints are display-only — never used for lookups or authentication.

### Instance Identity

Every Crab City instance has its own ed25519 keypair (= iroh NodeId). This
identity is generated at first startup and stored in the config directory.
The instance signs connection tokens and authenticates iroh connections with
this key.

### Loopback Identity

Local CLI/TUI connections via loopback bypass authentication (existing
behavior). Attributed to a synthetic all-zeros sentinel pubkey (`0x00 * 32`)
that cannot be used remotely. Always has `owner` access.

## Transport Model

### iroh for Instance-to-Instance

Ed25519 keypairs are iroh `NodeId`s — same curve, same key format.
Authentication IS the connection. The iroh handshake proves keypair ownership
and establishes end-to-end encryption.

```
Home Instance                           Host Instance
    |                                       |
    |  iroh QUIC connect (ALPN: crab/1)    |
    | ------------------------------------> |
    |                                       |  accept connection
    |  Hello { instance_name }             |
    | ------------------------------------> |
    |                                       |
    |  Welcome { instance_name }           |
    | <------------------------------------ |
    |                                       |
    |  tunnel established                  |
    |  users authenticate individually     |
```

Properties:
- **E2E encryption by construction.** QUIC authenticated encryption.
- **Connection migration.** WiFi-to-cellular transitions handled by QUIC.
- **Multiplexed streams.** One QUIC connection carries control, terminal
  output, and everything else without head-of-line blocking.

### Embedded Relay

Every instance embeds an iroh relay server (`iroh-relay` crate). This enables
NAT traversal — instances behind firewalls can still connect.

```
Local:    direct QUIC connection (same LAN)
Remote:   via relay (NAT traversal)
```

### One Tunnel, Many Users

The iroh connection is between instances (NodeId to NodeId). If both Alice
and Dave on the same home instance have grants on Bob's Crab City, their
messages travel over the same iroh connection. Each user authenticates
individually within the tunnel.

```
Alice's Instance ──iroh──► Bob's Instance
  │                             │
  │ Alice's messages ──────────►│ verify Alice's identity proof
  │                             │ lookup federated account → collaborate
  │                             │ allow
  │                             │
  │ Dave's messages ───────────►│ verify Dave's identity proof
  │                             │ lookup federated account → not found
  │                             │ reject
```

## Authorization Model

### Capabilities and Access Rights

Authorization uses GNAP-inspired access rights (RFC 9635 Section 8):

```json
{ "type": "terminals", "actions": ["read", "input"] }
```

Capabilities are named presets that expand to access right arrays:

| Capability    | Access Rights |
|---------------|---------------|
| `view`        | `content:read`, `terminals:read` |
| `collaborate` | view + `terminals:input`, `chat:send`, `tasks:read,create,edit`, `instances:create` |
| `admin`       | collaborate + `members:read,invite,suspend,reinstate,remove,update` |
| `owner`       | admin + `instance:manage,transfer` |

### Capability Algebra

Four operations in `crab_city_auth` — the only way to manipulate access
rights:

| Operation | Use case | Semantics |
|-----------|----------|-----------|
| `intersect(a, b)` | Scoped sessions | requested ∩ granted |
| `contains(type, action)` | Authorization checks | required ⊆ scope |
| `is_superset_of(other)` | Capability narrowing | invite.cap ⊆ issuer.cap |
| `diff(old, new)` | Access tweaking, audit | (added, removed) |

Property-tested: `intersect` is commutative and idempotent, preset ordering
holds, round-trip from capability to access rights and back.

### Federated Accounts

When Bob invites Alice, Bob's instance creates a **federated account** for
Alice's public key. This is a real account on Bob's server with its own
grant, linked to an identity that lives on another server.

```rust
FederatedAccount {
    account_key: PublicKey,          // Alice's ed25519 pubkey
    display_name: String,            // "Alice"
    home_node_id: Option<PublicKey>, // Alice's home instance NodeId
    home_name: Option<String>,       // "Alice's Lab"
    access: AccessRights,            // what Alice can do here
    state: GrantState,               // active, suspended
    created_by: PublicKey,           // Bob (admin who created this)
}
```

The same capability algebra applies to federated accounts as local member
grants. Access rights are enforced on every message.

### Per-User Authentication in Tunnels

Users authenticate individually within instance tunnels using ed25519
identity proofs:

```
Home → Host:  Authenticate { account_key, display_name, identity_proof }
Host → Home:  AuthResult { account_key, capability, access, error }
```

The identity proof is the user's signing key signing the host's node_id.
The host verifies the signature, looks up the federated account, and grants
or denies access.

### Membership State Machine

Federated accounts use the same state machine as local grants:

```
active ──► suspended ──► active (reinstate)
  │            │
  └──► removed └──► removed
```

| State       | Access  |
|-------------|---------|
| `active`    | Granted |
| `suspended` | Denied  |
| `removed`   | Denied (terminal) |

## The Invite Flow

### Connection Tokens

Connection tokens are self-describing signed envelopes:

```
ConnectionToken v2:
  [1B version=2]
  [32B node_id]              — host instance ed25519 pubkey
  [16B invite_nonce]         — nonce to redeem
  [1B name_len][name_len B]  — instance name (UTF-8)
  [8B inviter_fingerprint]   — for display
  [1B capability]            — 0=view, 1=collaborate, 2=admin
  [64B signature]            — instance key signs all preceding bytes
  [remaining: relay_url]     — optional
```

Alice can verify what she's joining before any network activity — the token
carries instance name, inviter identity, and capability, all signed.

### The Experience

Bob creates an invite:
```
$ crab invite --for alice --expires 1h
  Invite for Bob's Workshop
  Access:  collaborate
  24G8R3YK7V3MQ6...
  (copied to clipboard)
```

Alice joins:
```
$ crab connect 24G8R3YK7V3MQ6...
  Bob's Workshop
  Invited by: Bob (crab_2K7XM9QP)
  Access: collaborate
  Join this workspace? [Y/n] y
  Connected to Bob's Workshop
```

Every error state has a specific message with recovery action — expired,
exhausted, revoked, already a member, suspended, unreachable.

## Message Flow

After authentication, the tunnel carries the existing protocol:

**Home → Host** (tagged with user's account_key):
- `Focus`, `Input`, `Resize` — terminal interaction
- `ChatSend`, `ChatHistory` — chat
- `ListMembers` — member queries
- `TerminalVisible`, `TerminalHidden` — viewport registration

**Host → Home**:
- `InstanceList` — available terminals
- `Output`, `OutputHistory` — terminal data
- `StateChange` — instance state
- `PresenceUpdate` — who's viewing what
- `ChatMessage` — chat messages
- `TaskUpdate` — task changes

Every inbound message is checked against the user's federated grant.

## Context Switching

When viewing a remote Crab City, the entire UI context switches — instance
list, chat, tasks, presence all come from the remote host:

```
Viewing: Alice's Lab [▼]              Viewing: Bob's Workshop [▼]

┌ Instances ─────────────────┐        ┌ Instances ──────────────────┐
│ ► experiments.sh (running) │        │ ► deploy.sh     (running)   │
│   data-pipeline  (idle)    │        │   editor         (idle)     │
└────────────────────────────┘        └──────────────────────────────┘
Chat: Alice's #general                Chat: Bob's #general
Presence: Alice, Dave                 Presence: Bob, Alice
```

## What Interconnect Is NOT

- **Symmetric.** Bob inviting Alice does not mean Alice invited Bob.
- **Instance-level.** Bob invites Alice, not "everyone on Alice's server."
- **Replication.** Terminal output streams live. Nothing is replicated.
- **Transitive.** Alice→Bob and Carol→Alice does NOT give Carol→Bob.
- **Registry-dependent.** Two instances connect on a LAN with zero internet.

## Future: Registry and Enterprise

The interconnect system is designed to work standalone. Future milestones
layer additional capabilities on top:

| Future Milestone | Adds |
|-----------------|------|
| Registry Core | Handle-based profiles, instance directory, noun-based invites |
| Registry Integration | Heartbeat sync, handle resolution, blocklist distribution |
| OIDC Provider | "Sign in with Crab City" |
| Enterprise SSO | Okta/Entra/Google Workspace integration |
| Blocklists | Global and org-scoped moderation |
| LAN Discovery | `crab connect --discover` via mDNS/DHT |

None of these are required for interconnect to function. Two instances can
connect and federate users with zero external dependencies.
