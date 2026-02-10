//! Authentication module: password ops, session management, extractors, middleware, handlers.

use anyhow::Result;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

use crate::config::AuthConfig;
use crate::models::{Session, User, UserInfo};
use crate::repository::ConversationRepository;

// =============================================================================
// Password Operations
// =============================================================================

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("Failed to hash password: {}", e))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// =============================================================================
// Token Generation
// =============================================================================

pub fn generate_session_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
    hex::encode(&bytes)
}

pub fn generate_csrf_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen()).collect();
    hex::encode(&bytes)
}

// Inline hex encoding to avoid extra dependency
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// =============================================================================
// AuthUser Extractor
// =============================================================================

/// Authenticated user, extracted from the session cookie.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthUser {
    pub user_id: String,
    pub display_name: String,
    pub is_admin: bool,
    pub session_token: String,
    pub csrf_token: String,
}

/// Optional auth user (for endpoints that work with or without auth).
#[derive(Debug, Clone)]
pub struct MaybeAuthUser(pub Option<AuthUser>);

// =============================================================================
// Auth State (subset of AppState needed for auth)
// =============================================================================

#[derive(Clone)]
pub struct AuthState {
    pub repository: Arc<ConversationRepository>,
    pub auth_config: Arc<AuthConfig>,
}

// =============================================================================
// Cookie Helpers
// =============================================================================

fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for cookie in cookie_header.split(';') {
        let cookie = cookie.trim();
        if let Some(value) = cookie.strip_prefix("crab_session=") {
            return Some(value.to_string());
        }
    }
    None
}

fn make_session_cookie(token: &str, auth_config: &AuthConfig) -> String {
    let mut cookie = format!(
        "crab_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        token, auth_config.session_ttl_secs
    );
    if auth_config.https {
        cookie.push_str("; Secure");
    }
    cookie
}

fn make_clear_cookie(auth_config: &AuthConfig) -> String {
    let mut cookie = "crab_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0".to_string();
    if auth_config.https {
        cookie.push_str("; Secure");
    }
    cookie
}

// =============================================================================
// Auth Middleware
// =============================================================================

/// Auth middleware that:
/// 1. Exempts public routes (auth endpoints, health, metrics, share, static)
/// 2. Validates session cookie for protected routes
/// 3. Verifies CSRF token on mutating requests (POST/PUT/DELETE)
pub async fn auth_middleware(
    State(auth_state): State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();

    // Exempt public routes
    // All SPA pages are served under /spa/ and don't need auth middleware
    // (the SPA handles its own auth redirects client-side).
    // API auth endpoints, health checks, and static assets are also exempt.
    if path.starts_with("/api/auth/")
        || path == "/health"
        || path.starts_with("/health/")
        || path == "/metrics"
        || path.starts_with("/api/share/")
        || path.starts_with("/spa/")
        || path == "/"
        || path.ends_with(".js")
        || path.ends_with(".css")
        || path.ends_with(".png")
        || path.ends_with(".ico")
        || path.ends_with(".svg")
        || path.ends_with(".woff2")
    {
        return next.run(request).await;
    }

    // Check if this is a loopback connection (CLI clients discover daemon via local
    // PID/port files, so reaching the server on localhost already proves local trust).
    let is_loopback = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|ci| ci.0.ip().is_loopback());

    // Extract session token from cookie
    let token = match extract_session_token(request.headers()) {
        Some(t) => t,
        None if is_loopback => {
            // Loopback without a session cookie — allow through without identity
            return next.run(request).await;
        }
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Authentication required"
                })),
            )
                .into_response();
        }
    };

    // Look up session
    let (session, user) = match auth_state.repository.get_session_with_user(&token).await {
        Ok(Some(pair)) => pair,
        Ok(None) if is_loopback => {
            // Loopback with an invalid/expired session — allow through without identity
            return next.run(request).await;
        }
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Invalid or expired session"
                })),
            )
                .into_response();
        }
        Err(e) => {
            warn!("Session lookup error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Internal server error"
                })),
            )
                .into_response();
        }
    };

    // CSRF check for mutating methods
    if method == "POST" || method == "PUT" || method == "DELETE" || method == "PATCH" {
        // WebSocket upgrade requests don't need CSRF
        let is_ws = request
            .headers()
            .get("upgrade")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);

        if !is_ws {
            let csrf_header = request
                .headers()
                .get("X-CSRF-Token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            if csrf_header != session.csrf_token {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": "Invalid CSRF token"
                    })),
                )
                    .into_response();
            }
        }
    }

    // Touch session (update last_active_at) - fire and forget
    let repo = auth_state.repository.clone();
    let token_clone = token.clone();
    tokio::spawn(async move {
        if let Err(e) = repo.touch_session(&token_clone).await {
            warn!("Failed to touch session: {}", e);
        }
    });

    // Inject AuthUser into request extensions
    let auth_user = AuthUser {
        user_id: user.id,
        display_name: user.display_name,
        is_admin: user.is_admin,
        session_token: session.token,
        csrf_token: session.csrf_token,
    };
    request.extensions_mut().insert(auth_user);

    next.run(request).await
}

// =============================================================================
// Extractor implementations
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

// =============================================================================
// Auth Route Handlers
// =============================================================================

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
    pub invite_token: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Serialize)]
struct AuthResponse {
    user: UserInfo,
    csrf_token: String,
}

#[derive(Serialize)]
struct MeResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<UserInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    csrf_token: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    needs_setup: bool,
    auth_enabled: bool,
}

/// POST /api/auth/register
async fn register_handler(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Response {
    if !state.auth_config.enabled {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Auth not enabled"
            })),
        )
            .into_response();
    }

    // Validate input
    let username = req.username.trim();
    if username.len() < 2 || username.len() > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Username must be 2-64 characters"
            })),
        )
            .into_response();
    }

    if req.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Password must be at least 8 characters"
            })),
        )
            .into_response();
    }

    // Check if this is the first user (becomes admin)
    let user_count = match state.repository.user_count().await {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to count users: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let is_first_user = user_count == 0;

    // Validate server invite token (if provided) before the registration gate
    let valid_invite = if let Some(ref invite_token) = req.invite_token {
        match state.repository.get_server_invite(invite_token).await {
            Ok(Some(invite)) if invite.is_valid() => Some(invite),
            _ => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": "Invalid or expired invite"
                    })),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    // If not first user, check registration setting (DB overrides env var)
    if !is_first_user {
        let allow_reg = match state.repository.get_setting("allow_registration").await {
            Ok(Some(v)) => v != "false" && v != "0",
            _ => state.auth_config.allow_registration,
        };

        if !allow_reg && valid_invite.is_none() {
            // Only allow if caller is an authenticated admin
            let caller_is_admin = if let Some(token) = extract_session_token(&headers) {
                match state.repository.get_session_with_user(&token).await {
                    Ok(Some((_session, user))) => user.is_admin,
                    _ => false,
                }
            } else {
                false
            };

            if !caller_is_admin {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": "Registration is disabled. Contact an admin."
                    })),
                )
                    .into_response();
            }
        }
    }

    // Hash password
    let password_hash = match hash_password(&req.password) {
        Ok(h) => h,
        Err(e) => {
            warn!("Password hashing failed: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let display_name = req.display_name.unwrap_or_else(|| username.to_string());

    let now = chrono::Utc::now().timestamp();
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.to_string(),
        display_name,
        password_hash,
        is_admin: is_first_user,
        is_disabled: false,
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = state.repository.create_user(&user).await {
        if e.to_string().contains("UNIQUE") {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "Username already taken"
                })),
            )
                .into_response();
        }
        warn!("Failed to create user: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Record server invite usage if applicable
    if let Some(ref invite) = valid_invite {
        if let Err(e) = state
            .repository
            .use_server_invite(&invite.token, &user.id)
            .await
        {
            warn!("Failed to record server invite usage: {}", e);
        }
    }

    info!(
        "User registered: {} (admin: {})",
        user.username, user.is_admin
    );

    // Auto-login: create session
    let session_token = generate_session_token();
    let csrf_token = generate_csrf_token();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let session = Session {
        token: session_token.clone(),
        user_id: user.id.clone(),
        csrf_token: csrf_token.clone(),
        expires_at: now + state.auth_config.session_ttl_secs as i64,
        last_active_at: now,
        user_agent,
        ip_address: None,
    };

    if let Err(e) = state.repository.create_session(&session).await {
        warn!("Failed to create session: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let cookie = make_session_cookie(&session_token, &state.auth_config);
    let user_info: UserInfo = user.into();

    (
        StatusCode::CREATED,
        [(header::SET_COOKIE, cookie)],
        Json(AuthResponse {
            user: user_info,
            csrf_token,
        }),
    )
        .into_response()
}

/// POST /api/auth/login
async fn login_handler(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    if !state.auth_config.enabled {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Auth not enabled"
            })),
        )
            .into_response();
    }

    let user = match state.repository.get_user_by_username(&req.username).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Prevent timing attacks: still hash something
            let _ = hash_password("dummy_password_for_timing");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Invalid username or password"
                })),
            )
                .into_response();
        }
        Err(e) => {
            warn!("User lookup failed: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if user.is_disabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "Account is disabled"
            })),
        )
            .into_response();
    }

    if !verify_password(&req.password, &user.password_hash) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Invalid username or password"
            })),
        )
            .into_response();
    }

    let now = chrono::Utc::now().timestamp();
    let session_token = generate_session_token();
    let csrf_token = generate_csrf_token();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let session = Session {
        token: session_token.clone(),
        user_id: user.id.clone(),
        csrf_token: csrf_token.clone(),
        expires_at: now + state.auth_config.session_ttl_secs as i64,
        last_active_at: now,
        user_agent,
        ip_address: None,
    };

    if let Err(e) = state.repository.create_session(&session).await {
        warn!("Failed to create session: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    info!("User logged in: {}", user.username);

    let cookie = make_session_cookie(&session_token, &state.auth_config);
    let user_info: UserInfo = user.into();

    (
        [(header::SET_COOKIE, cookie)],
        Json(AuthResponse {
            user: user_info,
            csrf_token,
        }),
    )
        .into_response()
}

/// POST /api/auth/logout
async fn logout_handler(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if let Some(token) = extract_session_token(&headers) {
        if let Err(e) = state.repository.delete_session(&token).await {
            warn!("Failed to delete session on logout: {}", e);
        }
    }

    let cookie = make_clear_cookie(&state.auth_config);
    (
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({"ok": true})),
    )
        .into_response()
}

/// GET /api/auth/me
async fn me_handler(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !state.auth_config.enabled {
        return Json(MeResponse {
            user: None,
            csrf_token: None,
            needs_setup: false,
            auth_enabled: false,
        })
        .into_response();
    }

    // Check if any users exist
    let user_count = state.repository.user_count().await.unwrap_or(0);
    if user_count == 0 {
        return Json(MeResponse {
            user: None,
            csrf_token: None,
            needs_setup: true,
            auth_enabled: true,
        })
        .into_response();
    }

    // Try to get current user from session
    if let Some(token) = extract_session_token(&headers) {
        if let Ok(Some((session, user))) = state.repository.get_session_with_user(&token).await {
            return Json(MeResponse {
                user: Some(user.into()),
                csrf_token: Some(session.csrf_token),
                needs_setup: false,
                auth_enabled: true,
            })
            .into_response();
        }
    }

    Json(MeResponse {
        user: None,
        csrf_token: None,
        needs_setup: false,
        auth_enabled: true,
    })
    .into_response()
}

/// POST /api/auth/change-password
async fn change_password_handler(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Authentication required"
                })),
            )
                .into_response();
        }
    };

    let (_, user) = match state.repository.get_session_with_user(&token).await {
        Ok(Some(pair)) => pair,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Invalid session"
                })),
            )
                .into_response();
        }
    };

    if !verify_password(&req.current_password, &user.password_hash) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Current password is incorrect"
            })),
        )
            .into_response();
    }

    if req.new_password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "New password must be at least 8 characters"
            })),
        )
            .into_response();
    }

    let new_hash = match hash_password(&req.new_password) {
        Ok(h) => h,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if let Err(e) = state
        .repository
        .update_user_password(&user.id, &new_hash)
        .await
    {
        warn!("Failed to update password: {}", e);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Invalidate all other sessions (keep the current one so the user stays logged in)
    if let Err(e) = state
        .repository
        .delete_user_sessions(&user.id, Some(&token))
        .await
    {
        warn!("Failed to invalidate old sessions: {}", e);
    }

    Json(serde_json::json!({"ok": true})).into_response()
}

// =============================================================================
// Check Invite Handler
// =============================================================================

#[derive(Deserialize)]
pub struct CheckInviteParams {
    pub token: String,
}

#[derive(Serialize)]
struct CheckInviteResponse {
    valid: bool,
    label: Option<String>,
}

/// GET /api/auth/check-invite?token=...
async fn check_invite_handler(
    State(state): State<AuthState>,
    Query(params): Query<CheckInviteParams>,
) -> Json<CheckInviteResponse> {
    match state.repository.get_server_invite(&params.token).await {
        Ok(Some(invite)) if invite.is_valid() => Json(CheckInviteResponse {
            valid: true,
            label: invite.label,
        }),
        _ => Json(CheckInviteResponse {
            valid: false,
            label: None,
        }),
    }
}

// =============================================================================
// Router
// =============================================================================

pub fn auth_routes() -> Router<AuthState> {
    Router::new()
        .route("/api/auth/register", post(register_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/logout", post(logout_handler))
        .route("/api/auth/me", get(me_handler))
        .route("/api/auth/change-password", post(change_password_handler))
        .route("/api/auth/check-invite", get(check_invite_handler))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_password_and_verify() {
        let password = "secret123";
        let hash = hash_password(password).unwrap();

        // Hash should be non-empty and different from password
        assert!(!hash.is_empty());
        assert_ne!(hash, password);

        // Verification should work
        assert!(verify_password(password, &hash));
        assert!(!verify_password("wrong_password", &hash));
    }

    #[test]
    fn test_hash_password_produces_unique_hashes() {
        let password = "same_password";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();

        // Same password should produce different hashes (due to random salt)
        assert_ne!(hash1, hash2);

        // Both should verify correctly
        assert!(verify_password(password, &hash1));
        assert!(verify_password(password, &hash2));
    }

    #[test]
    fn test_verify_password_invalid_hash() {
        // Invalid hash format should return false, not panic
        assert!(!verify_password("password", "not_a_valid_hash"));
        assert!(!verify_password("password", ""));
    }

    #[test]
    fn test_generate_session_token() {
        let token1 = generate_session_token();
        let token2 = generate_session_token();

        // Tokens should be 64 hex characters (32 bytes)
        assert_eq!(token1.len(), 64);
        assert_eq!(token2.len(), 64);

        // Tokens should be unique
        assert_ne!(token1, token2);

        // Tokens should be valid hex
        assert!(token1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_csrf_token() {
        let token1 = generate_csrf_token();
        let token2 = generate_csrf_token();

        // CSRF tokens should be 64 hex characters
        assert_eq!(token1.len(), 64);

        // Tokens should be unique
        assert_ne!(token1, token2);
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex::encode(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(hex::encode(&[]), "");
        assert_eq!(hex::encode(&[0x12, 0x34]), "1234");
    }

    #[test]
    fn test_make_session_cookie_http() {
        let config = AuthConfig {
            enabled: true,
            session_ttl_secs: 3600,
            https: false,
            allow_registration: true,
        };

        let cookie = make_session_cookie("test_token", &config);

        assert!(cookie.contains("crab_session=test_token"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Path=/"));
        assert!(cookie.contains("Max-Age=3600"));
        assert!(!cookie.contains("Secure"));
    }

    #[test]
    fn test_make_session_cookie_https() {
        let config = AuthConfig {
            enabled: true,
            session_ttl_secs: 7200,
            https: true,
            allow_registration: true,
        };

        let cookie = make_session_cookie("test_token", &config);

        assert!(cookie.contains("crab_session=test_token"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("Max-Age=7200"));
    }

    #[test]
    fn test_make_clear_cookie() {
        let config = AuthConfig {
            enabled: true,
            session_ttl_secs: 3600,
            https: false,
            allow_registration: true,
        };

        let cookie = make_clear_cookie(&config);

        assert!(cookie.contains("crab_session="));
        assert!(cookie.contains("Max-Age=0"));
    }

    #[test]
    fn test_extract_session_token() {
        use axum::http::HeaderValue;

        // Test with valid cookie
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("crab_session=abc123"),
        );
        assert_eq!(extract_session_token(&headers), Some("abc123".to_string()));

        // Test with multiple cookies
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=foo; crab_session=xyz789; another=bar"),
        );
        assert_eq!(extract_session_token(&headers), Some("xyz789".to_string()));

        // Test with no matching cookie
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("other=foo"));
        assert!(extract_session_token(&headers).is_none());

        // Test with no cookie header
        let headers = HeaderMap::new();
        assert!(extract_session_token(&headers).is_none());
    }
}
