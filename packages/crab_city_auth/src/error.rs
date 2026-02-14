//! Error types, error codes, and machine-actionable recovery hints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RecoveryAction {
    Reconnect,
    Retry {
        retry_after_secs: u64,
    },
    ContactAdmin {
        admin_fingerprints: Vec<String>,
        reason: String,
    },
    RedeemInvite,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    #[serde(flatten)]
    pub action: RecoveryAction,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("invalid invite: {0}")]
    InvalidInvite(String),

    #[error("invalid identity proof: {0}")]
    InvalidIdentityProof(String),

    #[error("invalid signature")]
    InvalidSignature,

    #[error("not a member")]
    NotAMember,

    #[error("grant not active: {reason}")]
    GrantNotActive { reason: String },

    #[error("insufficient access: requires {required_type}:{required_action}")]
    InsufficientAccess {
        required_type: String,
        required_action: String,
    },

    #[error("blocklisted: {reason}")]
    Blocklisted { reason: String },

    #[error("handle taken")]
    HandleTaken,

    #[error("already a member")]
    AlreadyAMember,

    #[error("rate limited")]
    RateLimited { retry_after_secs: u64 },
}

impl AuthError {
    pub fn error_code(&self) -> &str {
        match self {
            Self::InvalidInvite(_) => "invalid_invite",
            Self::InvalidIdentityProof(_) => "invalid_identity_proof",
            Self::InvalidSignature => "invalid_signature",
            Self::NotAMember => "not_a_member",
            Self::GrantNotActive { .. } => "grant_not_active",
            Self::InsufficientAccess { .. } => "insufficient_access",
            Self::Blocklisted { .. } => "blocklisted",
            Self::HandleTaken => "handle_taken",
            Self::AlreadyAMember => "already_a_member",
            Self::RateLimited { .. } => "rate_limited",
        }
    }

    pub fn recovery(&self) -> Recovery {
        let action = match self {
            Self::InvalidInvite(_) => RecoveryAction::None,
            Self::InvalidIdentityProof(_) => RecoveryAction::None,
            Self::InvalidSignature => RecoveryAction::None,
            Self::NotAMember => RecoveryAction::RedeemInvite,
            Self::GrantNotActive { reason } => RecoveryAction::ContactAdmin {
                admin_fingerprints: Vec::new(),
                reason: reason.clone(),
            },
            Self::InsufficientAccess { .. } => RecoveryAction::None,
            Self::Blocklisted { reason } => RecoveryAction::ContactAdmin {
                admin_fingerprints: Vec::new(),
                reason: reason.clone(),
            },
            Self::HandleTaken => RecoveryAction::None,
            Self::AlreadyAMember => RecoveryAction::Reconnect,
            Self::RateLimited { retry_after_secs } => RecoveryAction::Retry {
                retry_after_secs: *retry_after_secs,
            },
        };
        Recovery { action }
    }
}

/// Serializable error response for iroh stream or HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    pub recovery: Recovery,
}

impl From<&AuthError> for ErrorResponse {
    fn from(err: &AuthError) -> Self {
        Self {
            error: err.error_code().to_string(),
            message: err.to_string(),
            recovery: err.recovery(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        assert_eq!(AuthError::NotAMember.error_code(), "not_a_member");
        assert_eq!(AuthError::HandleTaken.error_code(), "handle_taken");
        assert_eq!(
            AuthError::RateLimited {
                retry_after_secs: 30
            }
            .error_code(),
            "rate_limited"
        );
    }

    #[test]
    fn recovery_actions() {
        let r = AuthError::NotAMember.recovery();
        assert!(matches!(r.action, RecoveryAction::RedeemInvite));

        let r = AuthError::AlreadyAMember.recovery();
        assert!(matches!(r.action, RecoveryAction::Reconnect));

        let r = AuthError::RateLimited {
            retry_after_secs: 10,
        }
        .recovery();
        assert!(matches!(
            r.action,
            RecoveryAction::Retry {
                retry_after_secs: 10
            }
        ));
    }

    #[test]
    fn error_response_serde() {
        let err = AuthError::GrantNotActive {
            reason: "suspended".to_string(),
        };
        let resp = ErrorResponse::from(&err);
        let json = serde_json::to_string(&resp).unwrap();
        let back: ErrorResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.error, "grant_not_active");
    }
}
