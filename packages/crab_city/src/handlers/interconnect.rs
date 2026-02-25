//! RPC handlers for interconnect: membership, invites, and event log.
//!
//! These are standalone async functions callable from both the iroh transport
//! and the multiplexed WebSocket handler. All return `Result<ServerMessage, ServerMessage>`
//! where errors are `ServerMessage::Error`.

use std::sync::Arc;

use axum::extract::State;
use axum::{Json, http::StatusCode};
use crab_city_auth::event::EventType;
use crab_city_auth::{Capability, Invite, PublicKey};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;
use tracing::{error, info};

use crate::AppState;
use crate::auth::AuthUser;
use crate::identity::InstanceIdentity;
use crate::repository::ConversationRepository;
use crate::transport::connection_token::ConnectionToken;
use crate::ws::ServerMessage;

/// Shared context for all interconnect RPC handlers.
pub struct RpcContext {
    pub repo: ConversationRepository,
    pub identity: Arc<InstanceIdentity>,
    pub broadcast_tx: broadcast::Sender<ServerMessage>,
}

fn rpc_err(msg: impl Into<String>) -> ServerMessage {
    ServerMessage::Error {
        instance_id: None,
        message: msg.into(),
    }
}

// =============================================================================
// Hex helpers
// =============================================================================

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("hex string must have even length".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn parse_public_key(hex: &str) -> Result<PublicKey, ServerMessage> {
    let bytes = hex_to_bytes(hex).map_err(|e| rpc_err(format!("invalid public key hex: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| rpc_err("public key must be 32 bytes"))?;
    Ok(PublicKey::from_bytes(arr))
}

fn parse_nonce(hex: &str) -> Result<[u8; 16], ServerMessage> {
    let bytes = hex_to_bytes(hex).map_err(|e| rpc_err(format!("invalid nonce hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| rpc_err("nonce must be 16 bytes"))
}

fn member_to_json(m: &crate::repository::membership::Member) -> serde_json::Value {
    let pk = PublicKey::from_bytes(
        m.identity
            .public_key
            .as_slice()
            .try_into()
            .unwrap_or([0u8; 32]),
    );
    json!({
        "public_key": bytes_to_hex(&m.identity.public_key),
        "fingerprint": pk.fingerprint(),
        "display_name": m.identity.display_name,
        "handle": m.identity.handle,
        "avatar_url": m.identity.avatar_url,
        "capability": m.grant.capability,
        "state": m.grant.state,
        "created_at": m.grant.created_at,
    })
}

fn invite_state(inv: &crate::repository::invites::StoredInvite) -> &'static str {
    if inv.revoked_at.is_some() {
        return "revoked";
    }
    if inv.max_uses > 0 && inv.use_count >= inv.max_uses {
        return "exhausted";
    }
    if let Some(ref expires) = inv.expires_at {
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if *expires <= now {
            return "expired";
        }
    }
    "active"
}

fn invite_to_json(inv: &crate::repository::invites::StoredInvite) -> serde_json::Value {
    json!({
        "nonce": bytes_to_hex(&inv.nonce),
        "issuer": bytes_to_hex(&inv.issuer),
        "capability": inv.capability,
        "max_uses": inv.max_uses,
        "use_count": inv.use_count,
        "expires_at": inv.expires_at,
        "created_at": inv.created_at,
        "label": inv.label,
        "state": invite_state(inv),
    })
}

// =============================================================================
// Invite handlers
// =============================================================================

pub async fn handle_create_invite(
    ctx: &RpcContext,
    caller: &AuthUser,
    capability: &str,
    max_uses: u32,
    expires_in_secs: Option<u64>,
    label: Option<&str>,
) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "invite") else {
        return rpc_err("insufficient permissions for invite");
    };

    let cap: Capability = match capability.parse() {
        Ok(c) => c,
        Err(e) => return rpc_err(format!("invalid capability: {e}")),
    };

    // Cannot grant above caller's own capability
    if cap > caller.capability {
        return rpc_err(format!(
            "cannot create invite with capability '{}' — yours is '{}'",
            cap, caller.capability
        ));
    }

    let expires_at = expires_in_secs.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + secs
    });

    let invite = Invite::create_flat(
        ctx.identity.signing_key(),
        &ctx.identity.public_key,
        cap,
        max_uses,
        expires_at,
        &mut rand::rng(),
    );

    let nonce = invite.links[0].nonce;
    let token = bytes_to_hex(&nonce);
    let chain_blob = invite.to_bytes();

    let expires_at_str = expires_at.map(|ts| {
        chrono::DateTime::from_timestamp(ts as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default()
    });

    if let Err(e) = ctx
        .repo
        .store_invite(
            &nonce,
            caller.public_key.as_bytes(),
            capability,
            max_uses as i64,
            expires_at_str.as_deref(),
            &chain_blob,
            label,
        )
        .await
    {
        return rpc_err(format!("failed to store invite: {e}"));
    }

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::InviteCreated,
            Some(&caller.public_key),
            None,
            &json!({ "capability": capability, "max_uses": max_uses, "nonce": bytes_to_hex(&nonce) }),
            &ctx.identity.public_key,
        )
        .await
    {
        return rpc_err(format!("failed to log event: {e}"));
    }

    info!(
        caller = %caller.fingerprint,
        capability = capability,
        "invite created"
    );

    ServerMessage::InviteCreated {
        token,
        nonce: bytes_to_hex(&nonce),
        capability: capability.to_string(),
        max_uses,
        expires_at: expires_at_str,
        label: label.map(|s| s.to_string()),
    }
}

pub async fn handle_redeem_invite(
    ctx: &RpcContext,
    redeemer_pk: &PublicKey,
    token: &str,
    display_name: &str,
) -> ServerMessage {
    // Token is a hex-encoded nonce (32 hex chars = 16 bytes)
    let nonce_bytes = match hex_to_bytes(token) {
        Ok(b) if b.len() == 16 => b,
        _ => return rpc_err("invalid invite token"),
    };

    // Look up stored invite by nonce
    let stored = match ctx.repo.get_invite(&nonce_bytes).await {
        Ok(Some(s)) => s,
        Ok(None) => return rpc_err("invite not found"),
        Err(e) => return rpc_err(format!("failed to look up invite: {e}")),
    };

    // Verify the cryptographic chain from the stored blob
    let invite = match Invite::from_bytes(&stored.chain_blob) {
        Ok(i) => i,
        Err(e) => return rpc_err(format!("stored invite corrupt: {e}")),
    };

    let claims = match invite.verify() {
        Ok(c) => c,
        Err(e) => return rpc_err(format!("invite verification failed: {e}")),
    };

    // Must be for this instance
    if claims.instance != ctx.identity.public_key {
        return rpc_err("invite is for a different instance");
    };

    if !stored.is_valid() {
        return rpc_err("invite is no longer valid (revoked, expired, or exhausted)");
    }

    // Verify the root issuer has an active grant on this instance
    let root_pk_bytes = claims.root_issuer.as_bytes();
    match ctx.repo.get_active_grant(root_pk_bytes).await {
        Ok(Some(_)) => {}
        Ok(None) => return rpc_err("invite root issuer no longer has an active grant"),
        Err(e) => return rpc_err(format!("failed to check root issuer grant: {e}")),
    }

    // Check redeemer doesn't already have a grant
    match ctx.repo.get_grant(redeemer_pk.as_bytes()).await {
        Ok(Some(_)) => return rpc_err("you already have a grant on this instance"),
        Ok(None) => {}
        Err(e) => return rpc_err(format!("failed to check existing grant: {e}")),
    }

    let cap_str = claims.capability.to_string();
    let access_json = match serde_json::to_string(&claims.capability.access_rights()) {
        Ok(j) => j,
        Err(e) => return rpc_err(format!("failed to serialize access rights: {e}")),
    };

    // Create identity + grant
    if let Err(e) = ctx
        .repo
        .create_identity(redeemer_pk.as_bytes(), display_name)
        .await
    {
        return rpc_err(format!("failed to create identity: {e}"));
    }

    if let Err(e) = ctx
        .repo
        .create_grant(
            redeemer_pk.as_bytes(),
            &cap_str,
            &access_json,
            "active",
            Some(claims.leaf_issuer.as_bytes()),
            Some(&claims.nonce),
        )
        .await
    {
        return rpc_err(format!("failed to create grant: {e}"));
    }

    // Increment use count
    if let Err(e) = ctx.repo.increment_use_count(&claims.nonce).await {
        return rpc_err(format!("failed to increment invite use count: {e}"));
    }

    // Log events
    let pk_hex = bytes_to_hex(redeemer_pk.as_bytes());

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::InviteRedeemed,
            Some(redeemer_pk),
            None,
            &json!({ "nonce": bytes_to_hex(&claims.nonce), "capability": cap_str }),
            &ctx.identity.public_key,
        )
        .await
    {
        return rpc_err(format!("failed to log invite redeemed event: {e}"));
    }

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::MemberJoined,
            Some(redeemer_pk),
            Some(redeemer_pk),
            &json!({ "display_name": display_name, "capability": cap_str }),
            &ctx.identity.public_key,
        )
        .await
    {
        return rpc_err(format!("failed to log member joined event: {e}"));
    }

    // Broadcast MemberJoined
    let member_json = json!({
        "public_key": pk_hex,
        "fingerprint": redeemer_pk.fingerprint(),
        "display_name": display_name,
        "capability": cap_str,
        "state": "active",
    });
    let _ = ctx.broadcast_tx.send(ServerMessage::MemberJoined {
        member: member_json,
    });

    info!(
        redeemer = %redeemer_pk.fingerprint(),
        display_name = display_name,
        capability = %cap_str,
        "invite redeemed"
    );

    ServerMessage::InviteRedeemed {
        public_key: pk_hex,
        fingerprint: redeemer_pk.fingerprint(),
        display_name: display_name.to_string(),
        capability: cap_str,
    }
}

pub async fn handle_revoke_invite(
    ctx: &RpcContext,
    caller: &AuthUser,
    nonce_hex: &str,
    suspend_derived: bool,
) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "invite") else {
        return rpc_err("insufficient permissions for invite");
    };

    let nonce_bytes = match parse_nonce(nonce_hex) {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Verify invite exists
    let stored = match ctx.repo.get_invite(&nonce_bytes).await {
        Ok(Some(s)) => s,
        Ok(None) => return rpc_err("invite not found"),
        Err(e) => return rpc_err(format!("failed to look up invite: {e}")),
    };

    if stored.revoked_at.is_some() {
        return rpc_err("invite is already revoked");
    }

    if let Err(e) = ctx.repo.revoke_invite(&nonce_bytes).await {
        return rpc_err(format!("failed to revoke invite: {e}"));
    }

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::InviteRevoked,
            Some(&caller.public_key),
            None,
            &json!({ "nonce": nonce_hex }),
            &ctx.identity.public_key,
        )
        .await
    {
        return rpc_err(format!("failed to log event: {e}"));
    }

    // Optionally suspend members who joined via this invite
    if suspend_derived {
        let grants = match ctx.repo.list_grants_by_invite(&nonce_bytes).await {
            Ok(g) => g,
            Err(e) => return rpc_err(format!("failed to list derived grants: {e}")),
        };

        for grant in &grants {
            if grant.state == "active" {
                let pk_arr: [u8; 32] = match grant.public_key.clone().try_into() {
                    Ok(arr) => arr,
                    Err(v) => {
                        error!(
                            len = v.len(),
                            nonce = nonce_hex,
                            "skipping grant with invalid public key length during revoke suspend"
                        );
                        continue;
                    }
                };

                if let Err(e) = ctx
                    .repo
                    .update_grant_state(&grant.public_key, "suspended")
                    .await
                {
                    return rpc_err(format!("failed to suspend derived member: {e}"));
                }

                let target_pk = PublicKey::from_bytes(pk_arr);
                let identity = ctx
                    .repo
                    .get_identity(&grant.public_key)
                    .await
                    .ok()
                    .flatten();
                let dn = identity
                    .map(|i| i.display_name)
                    .unwrap_or_else(|| "unknown".into());

                ctx.repo
                    .append_event(
                        EventType::MemberSuspended,
                        Some(&caller.public_key),
                        Some(&target_pk),
                        &json!({ "reason": "invite revoked", "nonce": nonce_hex }),
                        &ctx.identity.public_key,
                    )
                    .await
                    .ok();

                let suspended_pk = PublicKey::from_bytes(
                    grant.public_key.as_slice().try_into().unwrap_or([0u8; 32]),
                );
                let _ = ctx.broadcast_tx.send(ServerMessage::MemberSuspended {
                    public_key: bytes_to_hex(&grant.public_key),
                    fingerprint: suspended_pk.fingerprint(),
                    display_name: dn,
                });
            }
        }
    }

    info!(caller = %caller.fingerprint, nonce = nonce_hex, "invite revoked");

    ServerMessage::InviteRevoked {
        nonce: nonce_hex.to_string(),
    }
}

pub async fn handle_list_invites(ctx: &RpcContext, caller: &AuthUser) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "invite") else {
        return rpc_err("insufficient permissions for invite");
    };

    let invites = match ctx.repo.list_all_invites().await {
        Ok(i) => i,
        Err(e) => return rpc_err(format!("failed to list invites: {e}")),
    };

    ServerMessage::InviteList {
        invites: invites.iter().map(invite_to_json).collect(),
    }
}

// =============================================================================
// Member handlers
// =============================================================================

pub async fn handle_list_members(ctx: &RpcContext, caller: &AuthUser) -> ServerMessage {
    let Ok(()) = caller.require_access("content", "read") else {
        return rpc_err("insufficient permissions to list members");
    };

    let members = match ctx.repo.list_members().await {
        Ok(m) => m,
        Err(e) => return rpc_err(format!("failed to list members: {e}")),
    };

    ServerMessage::MembersList {
        members: members.iter().map(member_to_json).collect(),
    }
}

pub async fn handle_update_member(
    ctx: &RpcContext,
    caller: &AuthUser,
    public_key_hex: &str,
    new_capability: Option<&str>,
    new_display_name: Option<&str>,
) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "update") else {
        return rpc_err("insufficient permissions to update member");
    };

    let target_pk = match parse_public_key(public_key_hex) {
        Ok(pk) => pk,
        Err(e) => return e,
    };

    // Cannot update owner
    let grant = match ctx.repo.get_grant(target_pk.as_bytes()).await {
        Ok(Some(g)) => g,
        Ok(None) => return rpc_err("member not found"),
        Err(e) => return rpc_err(format!("failed to look up member: {e}")),
    };

    if grant.capability == "owner" {
        return rpc_err("cannot update the owner");
    }

    // No-op guard: if nothing to change, return current state without broadcast
    if new_capability.is_none() && new_display_name.is_none() {
        let member = match ctx.repo.get_member(target_pk.as_bytes()).await {
            Ok(Some(m)) => m,
            Ok(None) => return rpc_err("member not found"),
            Err(e) => return rpc_err(format!("failed to look up member: {e}")),
        };

        return ServerMessage::MemberUpdated {
            member: member_to_json(&member),
        };
    }

    if let Some(cap_str) = new_capability {
        let cap: Capability = match cap_str.parse() {
            Ok(c) => c,
            Err(e) => return rpc_err(format!("invalid capability: {e}")),
        };

        // No escalation beyond caller's own capability
        if cap > caller.capability {
            return rpc_err(format!(
                "cannot set capability '{}' — yours is '{}'",
                cap, caller.capability
            ));
        }

        let access_json = match serde_json::to_string(&cap.access_rights()) {
            Ok(j) => j,
            Err(e) => return rpc_err(format!("failed to serialize access: {e}")),
        };

        if let Err(e) = ctx
            .repo
            .update_grant_capability(target_pk.as_bytes(), cap_str, &access_json)
            .await
        {
            return rpc_err(format!("failed to update capability: {e}"));
        }

        if let Err(e) = ctx
            .repo
            .append_event(
                EventType::GrantCapabilityChanged,
                Some(&caller.public_key),
                Some(&target_pk),
                &json!({ "old_capability": grant.capability, "new_capability": cap_str }),
                &ctx.identity.public_key,
            )
            .await
        {
            return rpc_err(format!("failed to log event: {e}"));
        }
    }

    if let Some(dn) = new_display_name {
        if let Err(e) = ctx
            .repo
            .update_identity(target_pk.as_bytes(), dn, None, None)
            .await
        {
            return rpc_err(format!("failed to update display name: {e}"));
        }

        if let Err(e) = ctx
            .repo
            .append_event(
                EventType::IdentityUpdated,
                Some(&caller.public_key),
                Some(&target_pk),
                &json!({ "display_name": dn }),
                &ctx.identity.public_key,
            )
            .await
        {
            return rpc_err(format!("failed to log event: {e}"));
        }
    }

    // Fetch updated member for broadcast
    let member_json = match ctx
        .repo
        .get_member(target_pk.as_bytes())
        .await
        .ok()
        .flatten()
    {
        Some(m) => member_to_json(&m),
        None => json!({ "public_key": public_key_hex }),
    };

    let _ = ctx.broadcast_tx.send(ServerMessage::MemberUpdated {
        member: member_json.clone(),
    });

    info!(caller = %caller.fingerprint, target = public_key_hex, "member updated");

    ServerMessage::MemberUpdated {
        member: member_json,
    }
}

/// Returns `(response, Option<target_pk>)` — caller should disconnect target_pk if Some.
pub async fn handle_suspend_member(
    ctx: &RpcContext,
    caller: &AuthUser,
    public_key_hex: &str,
) -> (ServerMessage, Option<PublicKey>) {
    let Ok(()) = caller.require_access("members", "suspend") else {
        return (rpc_err("insufficient permissions to suspend member"), None);
    };

    let target_pk = match parse_public_key(public_key_hex) {
        Ok(pk) => pk,
        Err(e) => return (e, None),
    };

    let grant = match ctx.repo.get_grant(target_pk.as_bytes()).await {
        Ok(Some(g)) => g,
        Ok(None) => return (rpc_err("member not found"), None),
        Err(e) => return (rpc_err(format!("failed to look up member: {e}")), None),
    };

    if grant.capability == "owner" {
        return (rpc_err("cannot suspend the owner"), None);
    }

    if grant.state != "active" {
        return (
            rpc_err(format!(
                "member is not active (current state: {})",
                grant.state
            )),
            None,
        );
    }

    if let Err(e) = ctx
        .repo
        .update_grant_state(target_pk.as_bytes(), "suspended")
        .await
    {
        return (rpc_err(format!("failed to suspend member: {e}")), None);
    }

    let identity = ctx
        .repo
        .get_identity(target_pk.as_bytes())
        .await
        .ok()
        .flatten();
    let display_name = identity
        .map(|i| i.display_name)
        .unwrap_or_else(|| "unknown".into());

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::MemberSuspended,
            Some(&caller.public_key),
            Some(&target_pk),
            &json!({}),
            &ctx.identity.public_key,
        )
        .await
    {
        return (rpc_err(format!("failed to log event: {e}")), None);
    }

    let msg = ServerMessage::MemberSuspended {
        public_key: public_key_hex.to_string(),
        fingerprint: target_pk.fingerprint(),
        display_name: display_name.clone(),
    };
    let _ = ctx.broadcast_tx.send(msg.clone());

    info!(caller = %caller.fingerprint, target = public_key_hex, "member suspended");

    (msg, Some(target_pk))
}

pub async fn handle_reinstate_member(
    ctx: &RpcContext,
    caller: &AuthUser,
    public_key_hex: &str,
) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "reinstate") else {
        return rpc_err("insufficient permissions to reinstate member");
    };

    let target_pk = match parse_public_key(public_key_hex) {
        Ok(pk) => pk,
        Err(e) => return e,
    };

    let grant = match ctx.repo.get_grant(target_pk.as_bytes()).await {
        Ok(Some(g)) => g,
        Ok(None) => return rpc_err("member not found"),
        Err(e) => return rpc_err(format!("failed to look up member: {e}")),
    };

    if grant.state != "suspended" {
        return rpc_err(format!(
            "member is not suspended (current state: {})",
            grant.state
        ));
    }

    if let Err(e) = ctx
        .repo
        .update_grant_state(target_pk.as_bytes(), "active")
        .await
    {
        return rpc_err(format!("failed to reinstate member: {e}"));
    }

    let identity = ctx
        .repo
        .get_identity(target_pk.as_bytes())
        .await
        .ok()
        .flatten();
    let display_name = identity
        .map(|i| i.display_name)
        .unwrap_or_else(|| "unknown".into());

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::MemberReinstated,
            Some(&caller.public_key),
            Some(&target_pk),
            &json!({}),
            &ctx.identity.public_key,
        )
        .await
    {
        return rpc_err(format!("failed to log event: {e}"));
    }

    let msg = ServerMessage::MemberReinstated {
        public_key: public_key_hex.to_string(),
        fingerprint: target_pk.fingerprint(),
        display_name,
    };
    let _ = ctx.broadcast_tx.send(msg.clone());

    info!(caller = %caller.fingerprint, target = public_key_hex, "member reinstated");

    msg
}

/// Returns `(response, Option<target_pk>)` — caller should disconnect target_pk if Some.
pub async fn handle_remove_member(
    ctx: &RpcContext,
    caller: &AuthUser,
    public_key_hex: &str,
) -> (ServerMessage, Option<PublicKey>) {
    let Ok(()) = caller.require_access("members", "remove") else {
        return (rpc_err("insufficient permissions to remove member"), None);
    };

    let target_pk = match parse_public_key(public_key_hex) {
        Ok(pk) => pk,
        Err(e) => return (e, None),
    };

    let grant = match ctx.repo.get_grant(target_pk.as_bytes()).await {
        Ok(Some(g)) => g,
        Ok(None) => return (rpc_err("member not found"), None),
        Err(e) => return (rpc_err(format!("failed to look up member: {e}")), None),
    };

    if grant.capability == "owner" {
        return (rpc_err("cannot remove the owner"), None);
    }

    if let Err(e) = ctx
        .repo
        .update_grant_state(target_pk.as_bytes(), "removed")
        .await
    {
        return (rpc_err(format!("failed to remove member: {e}")), None);
    }

    let identity = ctx
        .repo
        .get_identity(target_pk.as_bytes())
        .await
        .ok()
        .flatten();
    let display_name = identity
        .map(|i| i.display_name)
        .unwrap_or_else(|| "unknown".into());

    if let Err(e) = ctx
        .repo
        .append_event(
            EventType::MemberRemoved,
            Some(&caller.public_key),
            Some(&target_pk),
            &json!({}),
            &ctx.identity.public_key,
        )
        .await
    {
        return (rpc_err(format!("failed to log event: {e}")), None);
    }

    let msg = ServerMessage::MemberRemoved {
        public_key: public_key_hex.to_string(),
        fingerprint: target_pk.fingerprint(),
        display_name,
    };
    let _ = ctx.broadcast_tx.send(msg.clone());

    info!(caller = %caller.fingerprint, target = public_key_hex, "member removed");

    (msg, Some(target_pk))
}

// =============================================================================
// Event log handlers
// =============================================================================

pub async fn handle_query_events(
    ctx: &RpcContext,
    caller: &AuthUser,
    target_hex: Option<&str>,
    event_type_prefix: Option<&str>,
    limit: u32,
    before_id: Option<i64>,
) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "read") else {
        return rpc_err("insufficient permissions to query events");
    };

    let target_bytes = match target_hex {
        Some(hex) => match hex_to_bytes(hex) {
            Ok(bytes) => Some(bytes),
            Err(e) => return rpc_err(format!("invalid target hex: {e}")),
        },
        None => None,
    };

    let events = match ctx
        .repo
        .query_events(target_bytes.as_deref(), event_type_prefix, limit, before_id)
        .await
    {
        Ok(e) => e,
        Err(e) => return rpc_err(format!("failed to query events: {e}")),
    };

    let event_values: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "event_type": e.event_type.to_string(),
                "actor": e.actor.as_ref().map(|pk| bytes_to_hex(pk.as_bytes())),
                "target": e.target.as_ref().map(|pk| bytes_to_hex(pk.as_bytes())),
                "payload": e.payload,
                "created_at": e.created_at,
            })
        })
        .collect();

    ServerMessage::EventsResponse {
        events: event_values,
    }
}

pub async fn handle_verify_events(
    ctx: &RpcContext,
    caller: &AuthUser,
    from_id: i64,
    to_id: i64,
) -> ServerMessage {
    let Ok(()) = caller.require_access("members", "read") else {
        return rpc_err("insufficient permissions to verify events");
    };

    let verification = match ctx
        .repo
        .verify_chain(from_id, to_id, &ctx.identity.public_key)
        .await
    {
        Ok(v) => v,
        Err(e) => return rpc_err(format!("failed to verify chain: {e}")),
    };

    ServerMessage::EventVerification {
        valid: verification.valid,
        events_checked: verification.events_checked,
        error: verification.error,
    }
}

pub async fn handle_get_event_proof(
    ctx: &RpcContext,
    caller: &AuthUser,
    event_id: i64,
) -> ServerMessage {
    let Ok(()) = caller.require_access("content", "read") else {
        return rpc_err("insufficient permissions to get event proof");
    };

    let proof = match ctx.repo.get_event_proof(event_id).await {
        Ok(p) => p,
        Err(e) => return rpc_err(format!("failed to get event proof: {e}")),
    };

    let event_json = json!({
        "id": proof.event.id,
        "event_type": proof.event.event_type.to_string(),
        "actor": proof.event.actor.as_ref().map(|pk| bytes_to_hex(pk.as_bytes())),
        "target": proof.event.target.as_ref().map(|pk| bytes_to_hex(pk.as_bytes())),
        "payload": proof.event.payload,
        "created_at": proof.event.created_at,
        "hash": bytes_to_hex(&proof.event.hash),
        "prev_hash": bytes_to_hex(&proof.event.prev_hash),
    });

    let checkpoint_json = proof.nearest_checkpoint.map(|cp| {
        json!({
            "event_id": cp.event_id,
            "chain_head_hash": bytes_to_hex(&cp.chain_head_hash),
            "signature": bytes_to_hex(&cp.signature),
            "created_at": cp.created_at,
        })
    });

    ServerMessage::EventProofResponse {
        event: event_json,
        nearest_checkpoint: checkpoint_json,
    }
}

// =============================================================================
// HTTP invite endpoints (wrap the RPC handlers for REST access)
// =============================================================================

#[derive(Deserialize)]
pub struct CreateInviteRequest {
    #[serde(default = "default_capability")]
    pub capability: String,
    #[serde(default = "default_max_uses")]
    pub max_uses: u32,
    #[serde(default)]
    pub expires_in_secs: Option<u64>,
    #[serde(default)]
    pub label: Option<String>,
}

fn default_capability() -> String {
    "collaborate".to_string()
}

fn default_max_uses() -> u32 {
    1
}

/// POST /api/invites — create an invite and return a connection token.
pub async fn create_invite_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateInviteRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let identity = state
        .identity
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let node_id = state.iroh_node_id.ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let rpc_ctx = RpcContext {
        repo: state.repository.as_ref().clone(),
        identity: identity.clone(),
        broadcast_tx: state.global_state_manager.lifecycle_sender(),
    };

    // Loopback caller gets Owner access
    let caller = AuthUser::from_grant(
        identity.public_key,
        "Instance Owner".into(),
        Capability::Owner,
    );

    let result = handle_create_invite(
        &rpc_ctx,
        &caller,
        &req.capability,
        req.max_uses,
        req.expires_in_secs,
        req.label.as_deref(),
    )
    .await;

    match result {
        ServerMessage::InviteCreated {
            nonce,
            capability,
            max_uses,
            expires_at,
            label,
            ..
        } => {
            // Parse nonce hex back to bytes for the connection token
            let nonce_bytes =
                hex_to_bytes(&nonce).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let nonce_arr: [u8; 16] = nonce_bytes
                .try_into()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            // Map capability string to v2 byte encoding
            let cap_byte = match capability.as_str() {
                "view" => 0u8,
                "collaborate" => 1,
                "admin" => 2,
                "owner" => 3,
                _ => 1, // default to collaborate
            };

            let inviter_fp = ConnectionToken::compute_fingerprint(identity.public_key.as_bytes());

            let mut token = ConnectionToken {
                node_id,
                invite_nonce: nonce_arr,
                relay_url: None,
                instance_name: Some(state.instance_name.clone()),
                inviter_fingerprint: Some(inviter_fp),
                capability: Some(cap_byte),
                signature: None,
            };
            token.sign(identity.signing_key());

            Ok(Json(json!({
                "token": token.to_base32(),
                "nonce": nonce,
                "capability": capability,
                "max_uses": max_uses,
                "expires_at": expires_at,
                "label": label,
                "instance_name": state.instance_name,
            })))
        }
        ServerMessage::Error { message, .. } => {
            info!("create invite failed: {}", message);
            Ok(Json(json!({ "error": message })))
        }
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// GET /api/invites — list all invites.
pub async fn list_invites_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let identity = state
        .identity
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let rpc_ctx = RpcContext {
        repo: state.repository.as_ref().clone(),
        identity: identity.clone(),
        broadcast_tx: state.global_state_manager.lifecycle_sender(),
    };

    let caller = AuthUser::from_grant(
        identity.public_key,
        "Instance Owner".into(),
        Capability::Owner,
    );

    let result = handle_list_invites(&rpc_ctx, &caller).await;

    match result {
        ServerMessage::InviteList { invites } => Ok(Json(json!({ "invites": invites }))),
        ServerMessage::Error { message, .. } => Ok(Json(json!({ "error": message }))),
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// DELETE /api/invites/{nonce} — revoke an invite.
pub async fn revoke_invite_handler(
    State(state): State<AppState>,
    axum::extract::Path(nonce): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let identity = state
        .identity
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let rpc_ctx = RpcContext {
        repo: state.repository.as_ref().clone(),
        identity: identity.clone(),
        broadcast_tx: state.global_state_manager.lifecycle_sender(),
    };

    let caller = AuthUser::from_grant(
        identity.public_key,
        "Instance Owner".into(),
        Capability::Owner,
    );

    let result = handle_revoke_invite(&rpc_ctx, &caller, &nonce, false).await;

    match result {
        ServerMessage::InviteRevoked { nonce } => Ok(Json(json!({ "revoked": nonce }))),
        ServerMessage::Error { message, .. } => Ok(Json(json!({ "error": message }))),
        _ => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// =============================================================================
// Federation connection management HTTP endpoints
// =============================================================================

/// GET /api/federation/connections — list all federation connections.
pub async fn list_connections_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn_mgr = state
        .connection_manager
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let connections = conn_mgr.list_connections().await;
    let result: Vec<serde_json::Value> = connections
        .iter()
        .map(|c| {
            let state_str = match &c.state {
                crate::interconnect::manager::ConnectionState::Connected => "connected",
                crate::interconnect::manager::ConnectionState::Disconnected { .. } => {
                    "disconnected"
                }
                crate::interconnect::manager::ConnectionState::Reconnecting { .. } => {
                    "reconnecting"
                }
            };
            json!({
                "host_node_id": bytes_to_hex(&c.host_node_id),
                "host_name": c.host_name,
                "state": state_str,
                "authenticated_users": c.authenticated_users,
            })
        })
        .collect();

    Ok(Json(json!(result)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_helpers;

    async fn test_ctx() -> (RpcContext, AuthUser) {
        let repo = test_helpers::test_repository().await;
        let identity = Arc::new(InstanceIdentity::generate());
        let (broadcast_tx, _rx) = broadcast::channel(16);

        // Seed the owner identity + grant (the loopback user)
        let owner_pk = identity.public_key;
        repo.create_identity(owner_pk.as_bytes(), "Instance Owner")
            .await
            .unwrap();
        let access_json = serde_json::to_string(&Capability::Owner.access_rights()).unwrap();
        repo.create_grant(
            owner_pk.as_bytes(),
            "owner",
            &access_json,
            "active",
            None,
            None,
        )
        .await
        .unwrap();

        let ctx = RpcContext {
            repo,
            identity: identity.clone(),
            broadcast_tx,
        };

        let caller = AuthUser::from_grant(owner_pk, "Instance Owner".into(), Capability::Owner);

        (ctx, caller)
    }

    fn assert_not_error(msg: &ServerMessage) {
        assert!(
            !matches!(msg, ServerMessage::Error { .. }),
            "expected success but got: {msg:?}"
        );
    }

    fn assert_is_error(msg: &ServerMessage) {
        assert!(
            matches!(msg, ServerMessage::Error { .. }),
            "expected error but got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn create_and_list_invites() {
        let (ctx, caller) = test_ctx().await;

        let result = handle_create_invite(&ctx, &caller, "collaborate", 5, None, None).await;
        assert_not_error(&result);

        match &result {
            ServerMessage::InviteCreated {
                token,
                nonce,
                capability,
                max_uses,
                ..
            } => {
                assert!(!token.is_empty());
                assert!(!nonce.is_empty());
                assert_eq!(capability, "collaborate");
                assert_eq!(*max_uses, 5);
            }
            _ => panic!("Expected InviteCreated"),
        }

        // List should return the invite
        let list = handle_list_invites(&ctx, &caller).await;
        assert_not_error(&list);
        match list {
            ServerMessage::InviteList { invites } => {
                assert_eq!(invites.len(), 1);
                assert_eq!(invites[0]["capability"], "collaborate");
            }
            _ => panic!("Expected InviteList"),
        }
    }

    #[tokio::test]
    async fn create_invite_no_escalation() {
        let (ctx, _owner) = test_ctx().await;

        // Create a Collaborate-level user
        let collab_pk = PublicKey::from_bytes([77u8; 32]);
        ctx.repo
            .create_identity(collab_pk.as_bytes(), "Collab")
            .await
            .unwrap();
        let access = serde_json::to_string(&Capability::Collaborate.access_rights()).unwrap();
        ctx.repo
            .create_grant(
                collab_pk.as_bytes(),
                "collaborate",
                &access,
                "active",
                None,
                None,
            )
            .await
            .unwrap();

        let collab_user = AuthUser::from_grant(collab_pk, "Collab".into(), Capability::Collaborate);

        // Collaborator cannot invite with Admin capability
        let result = handle_create_invite(&ctx, &collab_user, "admin", 1, None, None).await;
        assert_is_error(&result);
    }

    #[tokio::test]
    async fn redeem_invite_flow() {
        let (ctx, caller) = test_ctx().await;

        // Create invite
        let result = handle_create_invite(&ctx, &caller, "collaborate", 1, None, None).await;
        assert_not_error(&result);
        let token = match &result {
            ServerMessage::InviteCreated { token, .. } => token.clone(),
            _ => panic!("Expected InviteCreated"),
        };

        // Redeem with a new key
        let new_pk = PublicKey::from_bytes([88u8; 32]);
        let redeem = handle_redeem_invite(&ctx, &new_pk, &token, "New User").await;
        assert_not_error(&redeem);

        match &redeem {
            ServerMessage::InviteRedeemed {
                display_name,
                capability,
                ..
            } => {
                assert_eq!(display_name, "New User");
                assert_eq!(capability, "collaborate");
            }
            _ => panic!("Expected InviteRedeemed"),
        }

        // Verify grant exists
        let grant = ctx
            .repo
            .get_active_grant(new_pk.as_bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(grant.capability, "collaborate");

        // Verify member in list
        let members = ctx.repo.list_members().await.unwrap();
        assert!(
            members
                .iter()
                .any(|m| m.identity.display_name == "New User")
        );
    }

    #[tokio::test]
    async fn redeem_invite_exhausted() {
        let (ctx, caller) = test_ctx().await;

        let result = handle_create_invite(&ctx, &caller, "view", 1, None, None).await;
        assert_not_error(&result);
        let token = match &result {
            ServerMessage::InviteCreated { token, .. } => token.clone(),
            _ => panic!("Expected InviteCreated"),
        };

        // First redemption succeeds
        let pk1 = PublicKey::from_bytes([91u8; 32]);
        let r = handle_redeem_invite(&ctx, &pk1, &token, "User1").await;
        assert_not_error(&r);

        // Second redemption fails (max_uses = 1)
        let pk2 = PublicKey::from_bytes([92u8; 32]);
        let result = handle_redeem_invite(&ctx, &pk2, &token, "User2").await;
        assert_is_error(&result);
    }

    #[tokio::test]
    async fn revoke_then_redeem_fails() {
        let (ctx, caller) = test_ctx().await;

        let result = handle_create_invite(&ctx, &caller, "collaborate", 0, None, None).await;
        assert_not_error(&result);
        let (token, nonce) = match &result {
            ServerMessage::InviteCreated { token, nonce, .. } => (token.clone(), nonce.clone()),
            _ => panic!("Expected InviteCreated"),
        };

        // Revoke
        let r = handle_revoke_invite(&ctx, &caller, &nonce, false).await;
        assert_not_error(&r);

        // Attempt redeem
        let new_pk = PublicKey::from_bytes([99u8; 32]);
        let result = handle_redeem_invite(&ctx, &new_pk, &token, "Rejected").await;
        assert_is_error(&result);
    }

    #[tokio::test]
    async fn suspend_and_reinstate_member() {
        let (ctx, caller) = test_ctx().await;

        // Create a member
        let member_pk = PublicKey::from_bytes([55u8; 32]);
        ctx.repo
            .create_identity(member_pk.as_bytes(), "Suspendee")
            .await
            .unwrap();
        ctx.repo
            .create_grant(
                member_pk.as_bytes(),
                "collaborate",
                "[]",
                "active",
                None,
                None,
            )
            .await
            .unwrap();

        let pk_hex = bytes_to_hex(member_pk.as_bytes());

        // Suspend
        let (msg, disconnect_pk) = handle_suspend_member(&ctx, &caller, &pk_hex).await;
        assert_not_error(&msg);
        assert!(matches!(msg, ServerMessage::MemberSuspended { .. }));
        assert!(disconnect_pk.is_some());

        // Verify state
        let grant = ctx
            .repo
            .get_grant(member_pk.as_bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(grant.state, "suspended");

        // Reinstate
        let msg = handle_reinstate_member(&ctx, &caller, &pk_hex).await;
        assert_not_error(&msg);
        assert!(matches!(msg, ServerMessage::MemberReinstated { .. }));

        // Verify active
        let grant = ctx
            .repo
            .get_active_grant(member_pk.as_bytes())
            .await
            .unwrap();
        assert!(grant.is_some());
    }

    #[tokio::test]
    async fn remove_member() {
        let (ctx, caller) = test_ctx().await;

        let member_pk = PublicKey::from_bytes([66u8; 32]);
        ctx.repo
            .create_identity(member_pk.as_bytes(), "Removee")
            .await
            .unwrap();
        ctx.repo
            .create_grant(member_pk.as_bytes(), "view", "[]", "active", None, None)
            .await
            .unwrap();

        let pk_hex = bytes_to_hex(member_pk.as_bytes());
        let (msg, disconnect_pk) = handle_remove_member(&ctx, &caller, &pk_hex).await;
        assert_not_error(&msg);
        assert!(matches!(msg, ServerMessage::MemberRemoved { .. }));
        assert!(disconnect_pk.is_some());

        let grant = ctx
            .repo
            .get_grant(member_pk.as_bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(grant.state, "removed");
    }

    #[tokio::test]
    async fn cannot_suspend_owner() {
        let (ctx, caller) = test_ctx().await;
        let owner_hex = bytes_to_hex(ctx.identity.public_key.as_bytes());

        let (msg, _) = handle_suspend_member(&ctx, &caller, &owner_hex).await;
        assert_is_error(&msg);
    }

    #[tokio::test]
    async fn cannot_remove_owner() {
        let (ctx, caller) = test_ctx().await;
        let owner_hex = bytes_to_hex(ctx.identity.public_key.as_bytes());

        let (msg, _) = handle_remove_member(&ctx, &caller, &owner_hex).await;
        assert_is_error(&msg);
    }

    #[tokio::test]
    async fn update_member_capability() {
        let (ctx, caller) = test_ctx().await;

        let member_pk = PublicKey::from_bytes([44u8; 32]);
        ctx.repo
            .create_identity(member_pk.as_bytes(), "Upgradee")
            .await
            .unwrap();
        ctx.repo
            .create_grant(member_pk.as_bytes(), "view", "[]", "active", None, None)
            .await
            .unwrap();

        let pk_hex = bytes_to_hex(member_pk.as_bytes());
        let msg = handle_update_member(&ctx, &caller, &pk_hex, Some("collaborate"), None).await;
        assert_not_error(&msg);

        match msg {
            ServerMessage::MemberUpdated { member } => {
                assert_eq!(member["capability"], "collaborate");
            }
            _ => panic!("Expected MemberUpdated"),
        }
    }

    #[tokio::test]
    async fn update_member_no_escalation() {
        let (ctx, _owner) = test_ctx().await;

        // Admin caller
        let admin_pk = PublicKey::from_bytes([33u8; 32]);
        ctx.repo
            .create_identity(admin_pk.as_bytes(), "Admin")
            .await
            .unwrap();
        let access = serde_json::to_string(&Capability::Admin.access_rights()).unwrap();
        ctx.repo
            .create_grant(admin_pk.as_bytes(), "admin", &access, "active", None, None)
            .await
            .unwrap();
        let admin_user = AuthUser::from_grant(admin_pk, "Admin".into(), Capability::Admin);

        // Target
        let target_pk = PublicKey::from_bytes([34u8; 32]);
        ctx.repo
            .create_identity(target_pk.as_bytes(), "Target")
            .await
            .unwrap();
        ctx.repo
            .create_grant(target_pk.as_bytes(), "view", "[]", "active", None, None)
            .await
            .unwrap();

        let pk_hex = bytes_to_hex(target_pk.as_bytes());

        // Admin cannot promote to Owner
        let result = handle_update_member(&ctx, &admin_user, &pk_hex, Some("owner"), None).await;
        assert_is_error(&result);
    }

    #[tokio::test]
    async fn update_member_noop_no_broadcast() {
        let (ctx, caller) = test_ctx().await;

        let member_pk = PublicKey::from_bytes([45u8; 32]);
        ctx.repo
            .create_identity(member_pk.as_bytes(), "NoOp")
            .await
            .unwrap();
        ctx.repo
            .create_grant(member_pk.as_bytes(), "view", "[]", "active", None, None)
            .await
            .unwrap();

        let pk_hex = bytes_to_hex(member_pk.as_bytes());

        // Subscribe to broadcast *before* the call
        let mut rx = ctx.broadcast_tx.subscribe();

        // Both fields None → should return current state without broadcasting
        let msg = handle_update_member(&ctx, &caller, &pk_hex, None, None).await;
        assert_not_error(&msg);

        match msg {
            ServerMessage::MemberUpdated { member } => {
                assert_eq!(member["display_name"], "NoOp");
                assert_eq!(member["capability"], "view");
            }
            _ => panic!("Expected MemberUpdated"),
        }

        // No broadcast should have been sent
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn query_and_verify_events() {
        let (ctx, caller) = test_ctx().await;

        // Create some events via invite flow
        let r = handle_create_invite(&ctx, &caller, "view", 0, None, None).await;
        assert_not_error(&r);
        let r = handle_create_invite(&ctx, &caller, "collaborate", 0, None, None).await;
        assert_not_error(&r);

        let events = handle_query_events(&ctx, &caller, None, Some("invite."), 10, None).await;
        assert_not_error(&events);
        match events {
            ServerMessage::EventsResponse { events } => {
                assert_eq!(events.len(), 2);
            }
            _ => panic!("Expected EventsResponse"),
        }

        // Verify chain
        let verify = handle_verify_events(&ctx, &caller, 1, 2).await;
        assert_not_error(&verify);
        match verify {
            ServerMessage::EventVerification {
                valid,
                events_checked,
                ..
            } => {
                assert!(valid);
                assert_eq!(events_checked, 2);
            }
            _ => panic!("Expected EventVerification"),
        }
    }

    #[tokio::test]
    async fn event_proof() {
        let (ctx, caller) = test_ctx().await;

        let r = handle_create_invite(&ctx, &caller, "view", 0, None, None).await;
        assert_not_error(&r);

        let proof = handle_get_event_proof(&ctx, &caller, 1).await;
        assert_not_error(&proof);
        match proof {
            ServerMessage::EventProofResponse { event, .. } => {
                assert_eq!(event["id"], 1);
                assert!(event["hash"].is_string());
            }
            _ => panic!("Expected EventProofResponse"),
        }
    }

    #[tokio::test]
    async fn list_members_includes_all() {
        let (ctx, caller) = test_ctx().await;

        // Owner is already there, add one more
        let pk = PublicKey::from_bytes([22u8; 32]);
        ctx.repo
            .create_identity(pk.as_bytes(), "Extra")
            .await
            .unwrap();
        ctx.repo
            .create_grant(pk.as_bytes(), "view", "[]", "active", None, None)
            .await
            .unwrap();

        let result = handle_list_members(&ctx, &caller).await;
        assert_not_error(&result);
        match result {
            ServerMessage::MembersList { members } => {
                // Owner + loopback (seeded) + Extra = at least 2 from our ctx
                assert!(members.len() >= 2);
            }
            _ => panic!("Expected MembersList"),
        }
    }

    #[test]
    fn hex_roundtrip() {
        let original = vec![0xde, 0xad, 0xbe, 0xef];
        let hex = bytes_to_hex(&original);
        assert_eq!(hex, "deadbeef");
        let decoded = hex_to_bytes(&hex).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn hex_invalid() {
        assert!(hex_to_bytes("xyz").is_err());
        assert!(hex_to_bytes("abc").is_err()); // odd length
    }

    #[test]
    fn parse_public_key_valid() {
        let hex = "aa".repeat(32);
        let pk = parse_public_key(&hex).unwrap();
        assert_eq!(pk.as_bytes(), &[0xaa; 32]);
    }

    #[test]
    fn parse_public_key_invalid_length() {
        let hex = "aa".repeat(16);
        assert!(parse_public_key(&hex).is_err());
    }

    #[test]
    fn parse_nonce_valid() {
        let hex = "bb".repeat(16);
        let nonce = parse_nonce(&hex).unwrap();
        assert_eq!(nonce, [0xbb; 16]);
    }
}
