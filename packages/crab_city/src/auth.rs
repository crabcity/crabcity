//! Authentication: keypair-based identity extracted from iroh connections.
//!
//! This replaces the previous password/session/CSRF auth system.
//! Identity is now derived from Ed25519 public keys via iroh QUIC handshakes.
//!
//! For HTTP routes (health, metrics, static assets), the loopback bypass
//! grants Owner-level access to local connections.

use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use crab_city_auth::{AccessRights, Capability, PublicKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::config::AuthConfig;
use crate::repository::ConversationRepository;

// =============================================================================
// AuthUser
// =============================================================================

/// Authenticated user, populated from the iroh connection state or loopback bypass.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// The user's Ed25519 public key (identity).
    pub public_key: PublicKey,
    /// Short fingerprint like `crab_XXXXXXXX`.
    pub fingerprint: String,
    /// Human-readable display name.
    pub display_name: String,
    /// The user's capability level (View < Collaborate < Admin < Owner).
    pub capability: Capability,
    /// Fine-grained access rights derived from capability.
    pub access: AccessRights,
}

impl AuthUser {
    /// Create an AuthUser for loopback (local CLI/TUI) connections.
    /// Loopback gets Owner-level access.
    pub fn loopback() -> Self {
        AuthUser {
            public_key: PublicKey::LOOPBACK,
            fingerprint: PublicKey::LOOPBACK.fingerprint(),
            display_name: "Local".into(),
            capability: Capability::Owner,
            access: Capability::Owner.access_rights(),
        }
    }

    /// Create an AuthUser from a grant lookup.
    pub fn from_grant(public_key: PublicKey, display_name: String, capability: Capability) -> Self {
        AuthUser {
            fingerprint: public_key.fingerprint(),
            access: capability.access_rights(),
            public_key,
            display_name,
            capability,
        }
    }

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

    /// Convenience: is this user at least Admin capability?
    pub fn is_admin(&self) -> bool {
        self.capability >= Capability::Admin
    }

    /// The stable string identifier for this user (fingerprint).
    /// This replaces the old `user_id` field.
    pub fn user_id(&self) -> &str {
        &self.fingerprint
    }
}

/// Optional auth user (for endpoints that work with or without auth).
#[derive(Debug, Clone)]
pub struct MaybeAuthUser(pub Option<AuthUser>);

// =============================================================================
// Auth Errors
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("insufficient access: requires {required_type}:{required_action}")]
    InsufficientAccess {
        required_type: String,
        required_action: String,
    },
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AuthError::InsufficientAccess { .. } => (StatusCode::FORBIDDEN, self.to_string()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

// =============================================================================
// Auth State (shared across middleware and handlers)
// =============================================================================

#[derive(Clone)]
pub struct AuthState {
    pub repository: Arc<ConversationRepository>,
    pub auth_config: Arc<AuthConfig>,
}

// =============================================================================
// Auth Middleware
// =============================================================================

/// Auth middleware for HTTP routes.
///
/// During the transition to iroh-only transport:
/// 1. Loopback connections → `AuthUser::loopback()` (Owner access)
/// 2. Public routes (health, static assets) → pass through
/// 3. Everything else → 401 (must use iroh transport)
pub async fn auth_middleware(
    State(_auth_state): State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Exempt public routes
    if is_public_route(&path) {
        return next.run(request).await;
    }

    // Check if this is a loopback connection
    let is_loopback = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|ci| ci.0.ip().is_loopback());

    if is_loopback {
        // Local connections get Owner access
        request.extensions_mut().insert(AuthUser::loopback());
        return next.run(request).await;
    }

    // Non-loopback HTTP requests are rejected — must use iroh transport
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": "Authentication required. Use iroh transport for remote access."
        })),
    )
        .into_response()
}

fn is_public_route(path: &str) -> bool {
    path == "/health"
        || path.starts_with("/health/")
        || path == "/metrics"
        || path.starts_with("/spa/")
        || path == "/"
        || path.ends_with(".js")
        || path.ends_with(".css")
        || path.ends_with(".png")
        || path.ends_with(".ico")
        || path.ends_with(".svg")
        || path.ends_with(".woff2")
}

// =============================================================================
// Axum Extractors
// =============================================================================

/// Extract AuthUser from request extensions (set by middleware).
/// Returns 401 if not present.
impl<S> axum::extract::FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        parts.extensions.get::<AuthUser>().cloned().ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Authentication required"})),
            )
        })
    }
}

/// Extract optional AuthUser from request extensions.
impl<S> axum::extract::FromRequestParts<S> for MaybeAuthUser
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        Ok(MaybeAuthUser(parts.extensions.get::<AuthUser>().cloned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_user() {
        let user = AuthUser::loopback();
        assert!(user.public_key.is_loopback());
        assert_eq!(user.capability, Capability::Owner);
        assert!(user.is_admin());
        assert!(user.fingerprint.starts_with("crab_"));
    }

    #[test]
    fn from_grant() {
        let pk = PublicKey::from_bytes([42u8; 32]);
        let user = AuthUser::from_grant(pk, "Alice".into(), Capability::Collaborate);
        assert_eq!(user.display_name, "Alice");
        assert_eq!(user.capability, Capability::Collaborate);
        assert!(!user.is_admin());
        assert!(user.fingerprint.starts_with("crab_"));
    }

    #[test]
    fn require_access_owner() {
        let user = AuthUser::loopback();
        // Owner has all access
        assert!(user.require_access("content", "read").is_ok());
        assert!(user.require_access("members", "update").is_ok());
    }

    #[test]
    fn require_access_view_denied() {
        let pk = PublicKey::from_bytes([1u8; 32]);
        let user = AuthUser::from_grant(pk, "Viewer".into(), Capability::View);
        // View can read but not write
        assert!(user.require_access("content", "read").is_ok());
        assert!(user.require_access("content", "write").is_err());
    }

    #[test]
    fn user_id_is_fingerprint() {
        let pk = PublicKey::from_bytes([42u8; 32]);
        let user = AuthUser::from_grant(pk, "Test".into(), Capability::Owner);
        assert_eq!(user.user_id(), &user.fingerprint);
    }

    #[test]
    fn public_routes() {
        assert!(is_public_route("/health"));
        assert!(is_public_route("/health/live"));
        assert!(is_public_route("/metrics"));
        assert!(is_public_route("/spa/index.html"));
        assert!(is_public_route("/"));
        assert!(is_public_route("/bundle.js"));
        assert!(is_public_route("/style.css"));
        assert!(!is_public_route("/api/instances"));
        assert!(!is_public_route("/api/tasks"));
    }
}
