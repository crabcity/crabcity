//! Instance identity: a persistent ed25519 keypair used for both
//! iroh transport (QUIC endpoint identity) and crab_city_auth (event signing).

use std::path::Path;

use anyhow::{Context, Result};
use crab_city_auth::{PublicKey, Signature, SigningKey};
use tracing::info;

/// Persistent instance identity backed by a single ed25519 keypair.
///
/// The same key is used as:
/// - The iroh `SecretKey` for QUIC endpoint identity
/// - The `crab_city_auth::SigningKey` for event chain signatures
pub struct InstanceIdentity {
    signing_key: SigningKey,
    pub public_key: PublicKey,
}

const KEY_FILE: &str = "identity.key";
const KEY_LEN: usize = 32;

impl InstanceIdentity {
    /// Load from `<data_dir>/identity.key`, or generate and save a new keypair.
    pub fn load_or_generate(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(KEY_FILE);

        if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("failed to read identity key: {}", path.display()))?;
            let arr: [u8; KEY_LEN] = bytes.try_into().map_err(|v: Vec<u8>| {
                anyhow::anyhow!("identity key must be {} bytes, got {}", KEY_LEN, v.len())
            })?;
            let signing_key = SigningKey::from_bytes(arr);
            let public_key = signing_key.public_key();
            info!("Loaded instance identity: {}", public_key.fingerprint());
            Ok(Self {
                signing_key,
                public_key,
            })
        } else {
            let identity = Self::generate();
            identity.save(&path)?;
            info!(
                "Generated new instance identity: {}",
                identity.public_key.fingerprint()
            );
            Ok(identity)
        }
    }

    /// The signing key for this instance (used for invite creation, event signing).
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// Generate a fresh identity (not persisted until `save` is called).
    pub(crate) fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut rand::rng());
        let public_key = signing_key.public_key();
        Self {
            signing_key,
            public_key,
        }
    }

    /// Write the 32-byte key seed to disk with mode 0600.
    fn save(&self, path: &Path) -> Result<()> {
        let bytes = self.signing_key.to_bytes();
        std::fs::write(path, bytes)
            .with_context(|| format!("failed to write identity key: {}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to set permissions on {}", path.display()))?;
        }

        Ok(())
    }

    /// The iroh `SecretKey` for this instance's QUIC endpoint.
    pub fn iroh_secret_key(&self) -> iroh::SecretKey {
        iroh::SecretKey::from_bytes(&self.signing_key.to_bytes())
    }

    /// Sign arbitrary bytes (used for event checkpoints).
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let id1 = InstanceIdentity::load_or_generate(tmp.path()).unwrap();
        let id2 = InstanceIdentity::load_or_generate(tmp.path()).unwrap();
        assert_eq!(id1.public_key, id2.public_key);
    }

    #[test]
    fn iroh_key_matches_public_key() {
        let tmp = tempfile::tempdir().unwrap();
        let id = InstanceIdentity::load_or_generate(tmp.path()).unwrap();
        let iroh_pub = id.iroh_secret_key().public();
        // iroh PublicKey bytes should match our PublicKey bytes
        assert_eq!(iroh_pub.as_bytes(), id.public_key.as_bytes());
    }

    #[test]
    fn sign_produces_valid_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let id = InstanceIdentity::load_or_generate(tmp.path()).unwrap();
        let msg = b"test message";
        let sig = id.sign(msg);
        assert!(crab_city_auth::keys::verify(&id.public_key, msg, &sig).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn key_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let _id = InstanceIdentity::load_or_generate(tmp.path()).unwrap();
        let path = tmp.path().join(KEY_FILE);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn fingerprint_format() {
        let tmp = tempfile::tempdir().unwrap();
        let id = InstanceIdentity::load_or_generate(tmp.path()).unwrap();
        let fp = id.public_key.fingerprint();
        assert!(fp.starts_with("crab_"));
        assert_eq!(fp.len(), 13);
    }
}
