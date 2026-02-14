//! Identity nouns: human-readable account references and validation.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::keys::PublicKey;

/// An identity noun — a human-readable reference to an account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "provider", content = "subject", rename_all = "lowercase")]
pub enum IdentityNoun {
    Handle(String),
    GitHub(String),
    Google(String),
    Email(String),
}

impl IdentityNoun {
    /// The provider string: `"handle"`, `"github"`, `"google"`, or `"email"`.
    pub fn provider(&self) -> &str {
        match self {
            Self::Handle(_) => "handle",
            Self::GitHub(_) => "github",
            Self::Google(_) => "google",
            Self::Email(_) => "email",
        }
    }

    /// The inner subject value.
    pub fn subject(&self) -> &str {
        match self {
            Self::Handle(s) | Self::GitHub(s) | Self::Google(s) | Self::Email(s) => s,
        }
    }

    /// Validate the noun's inner value against provider-specific rules.
    pub fn validate(&self) -> Result<(), NounError> {
        match self {
            Self::Handle(h) => validate_handle(h),
            Self::GitHub(u) => validate_github(u),
            Self::Google(e) => validate_email(e, "google"),
            Self::Email(e) => validate_email(e, "email"),
        }
    }
}

impl fmt::Display for IdentityNoun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handle(h) => write!(f, "@{h}"),
            Self::GitHub(u) => write!(f, "github:{u}"),
            Self::Google(e) => write!(f, "google:{e}"),
            Self::Email(e) => write!(f, "email:{e}"),
        }
    }
}

impl FromStr for IdentityNoun {
    type Err = NounError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let noun = if let Some(handle) = s.strip_prefix('@') {
            Self::Handle(handle.to_string())
        } else if let Some(user) = s.strip_prefix("github:") {
            Self::GitHub(user.to_string())
        } else if let Some(email) = s.strip_prefix("google:") {
            Self::Google(email.to_string())
        } else if let Some(email) = s.strip_prefix("email:") {
            Self::Email(email.to_string())
        } else {
            return Err(NounError::UnknownFormat(s.to_string()));
        };
        noun.validate()?;
        Ok(noun)
    }
}

/// A resolved noun — maps a noun to an account with cryptographic keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NounResolution {
    pub account_id: uuid::Uuid,
    pub handle: Option<String>,
    pub pubkeys: Vec<PublicKey>,
    /// Opaque signed blob from the registry attesting the resolution.
    pub attestation: Vec<u8>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum NounError {
    #[error("unknown noun format: {0}")]
    UnknownFormat(String),

    #[error("invalid handle: {0}")]
    InvalidHandle(String),

    #[error("invalid github username: {0}")]
    InvalidGitHub(String),

    #[error("invalid email for {provider}: {reason}")]
    InvalidEmail { provider: String, reason: String },
}

// --- Validation helpers ---

fn validate_handle(h: &str) -> Result<(), NounError> {
    let err = |msg: &str| NounError::InvalidHandle(msg.to_string());

    if h.len() < 3 || h.len() > 30 {
        return Err(err("must be 3-30 characters"));
    }
    if h.starts_with('-') || h.ends_with('-') {
        return Err(err("cannot start or end with hyphen"));
    }
    if !h
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(err("must be lowercase alphanumeric + hyphens"));
    }
    Ok(())
}

fn validate_github(u: &str) -> Result<(), NounError> {
    let err = |msg: &str| NounError::InvalidGitHub(msg.to_string());

    if u.is_empty() || u.len() > 39 {
        return Err(err("must be 1-39 characters"));
    }
    if u.starts_with('-') {
        return Err(err("cannot start with hyphen"));
    }
    if !u.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(err("must be alphanumeric + hyphens"));
    }
    Ok(())
}

fn validate_email(e: &str, provider: &str) -> Result<(), NounError> {
    let err = |reason: &str| NounError::InvalidEmail {
        provider: provider.to_string(),
        reason: reason.to_string(),
    };

    if e.is_empty() {
        return Err(err("empty"));
    }
    let at_pos = e.find('@').ok_or_else(|| err("missing @"))?;
    let local = &e[..at_pos];
    let domain = &e[at_pos + 1..];
    if local.is_empty() {
        return Err(err("empty local part"));
    }
    if domain.is_empty() {
        return Err(err("empty domain"));
    }
    if !domain.contains('.') {
        return Err(err("domain must contain a dot"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Display / FromStr round-trips ---

    #[test]
    fn handle_roundtrip() {
        let noun: IdentityNoun = "@alex".parse().unwrap();
        assert_eq!(noun, IdentityNoun::Handle("alex".to_string()));
        assert_eq!(noun.to_string(), "@alex");
        assert_eq!(noun.provider(), "handle");
        assert_eq!(noun.subject(), "alex");
    }

    #[test]
    fn github_roundtrip() {
        let noun: IdentityNoun = "github:octocat".parse().unwrap();
        assert_eq!(noun, IdentityNoun::GitHub("octocat".to_string()));
        assert_eq!(noun.to_string(), "github:octocat");
        assert_eq!(noun.provider(), "github");
        assert_eq!(noun.subject(), "octocat");
    }

    #[test]
    fn google_roundtrip() {
        let noun: IdentityNoun = "google:alice@acme.com".parse().unwrap();
        assert_eq!(noun, IdentityNoun::Google("alice@acme.com".to_string()));
        assert_eq!(noun.to_string(), "google:alice@acme.com");
    }

    #[test]
    fn email_roundtrip() {
        let noun: IdentityNoun = "email:bob@bar.com".parse().unwrap();
        assert_eq!(noun, IdentityNoun::Email("bob@bar.com".to_string()));
        assert_eq!(noun.to_string(), "email:bob@bar.com");
    }

    // --- Validation: handles ---

    #[test]
    fn handle_too_short() {
        assert!("@ab".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn handle_too_long() {
        let long = "@".to_string() + &"a".repeat(31);
        assert!(long.parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn handle_leading_hyphen() {
        assert!("@-foo".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn handle_trailing_hyphen() {
        assert!("@foo-".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn handle_uppercase_rejected() {
        assert!("@Alex".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn handle_special_chars_rejected() {
        assert!("@foo_bar".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn handle_valid_with_hyphens() {
        assert!("@foo-bar".parse::<IdentityNoun>().is_ok());
    }

    #[test]
    fn handle_valid_with_digits() {
        assert!("@user42".parse::<IdentityNoun>().is_ok());
    }

    // --- Validation: github ---

    #[test]
    fn github_empty() {
        assert!("github:".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn github_too_long() {
        let long = "github:".to_string() + &"a".repeat(40);
        assert!(long.parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn github_leading_hyphen() {
        assert!("github:-foo".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn github_valid_with_hyphens() {
        assert!("github:octo-cat".parse::<IdentityNoun>().is_ok());
    }

    // --- Validation: email ---

    #[test]
    fn email_empty() {
        assert!("email:".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn email_no_at() {
        assert!("email:nope".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn email_no_domain_dot() {
        assert!("email:foo@bar".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn google_valid() {
        assert!("google:foo@gmail.com".parse::<IdentityNoun>().is_ok());
    }

    // --- Unknown format ---

    #[test]
    fn unknown_format() {
        assert!("unknown:foo".parse::<IdentityNoun>().is_err());
    }

    #[test]
    fn bare_at() {
        // "@" alone has empty handle, too short
        assert!("@".parse::<IdentityNoun>().is_err());
    }

    // --- Serde ---

    #[test]
    fn serde_roundtrip() {
        let nouns = vec![
            IdentityNoun::Handle("alex".to_string()),
            IdentityNoun::GitHub("octocat".to_string()),
            IdentityNoun::Google("a@b.com".to_string()),
            IdentityNoun::Email("c@d.com".to_string()),
        ];
        for noun in nouns {
            let json = serde_json::to_string(&noun).unwrap();
            let back: IdentityNoun = serde_json::from_str(&json).unwrap();
            assert_eq!(noun, back);
        }
    }
}
