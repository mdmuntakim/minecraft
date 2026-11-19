//! Ed25519 signatures over a container digest, with pinning and revocation
//! (trust on first use).

use crate::error::{ErrorCode, FabricError, Result};
use ed25519_dalek::{Signature as DalekSignature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The JSON document stored at `META-INF/fabricmod.sig`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub algorithm: String,
    /// The raw 32-byte Ed25519 public key, hex.
    #[serde(rename = "publicKey")]
    pub public_key: String,
    /// The raw 64-byte signature, hex.
    pub signature: String,
    /// The SHA-256 digest the signature covers, hex.
    pub digest: String,
}

impl Signature {
    /// Signs a digest with a 32-byte Ed25519 seed.
    pub fn sign(digest: &[u8; 32], key: &SigningKey) -> Self {
        let signature = key.sign(digest);
        Self {
            algorithm: "ed25519".into(),
            public_key: hex(&key.verifying_key().to_bytes()),
            signature: hex(&signature.to_bytes()),
            digest: hex(digest),
        }
    }

    /// The key's fingerprint: SHA-256 of the raw public key, hex.
    pub fn fingerprint(&self) -> String {
        let Ok(key) = unhex(&self.public_key, 32) else {
            return "invalid-key".into();
        };
        hex(&Sha256::digest(key))
    }

    /// Verifies the signature over `digest`.
    pub fn verify(&self, digest: &[u8; 32]) -> Result<()> {
        if self.algorithm != "ed25519" {
            return Err(FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the mod uses an unknown signature algorithm",
                self.algorithm.clone(),
            ));
        }
        if self.digest != hex(digest) {
            return Err(FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the mod's signature covers different contents",
                format!("signed {} but read {}", self.digest, hex(digest)),
            ));
        }
        let key_bytes = unhex(&self.public_key, 32).map_err(|_| {
            FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the mod's public key is malformed",
                self.public_key.clone(),
            )
        })?;
        let sig_bytes = unhex(&self.signature, 64).map_err(|_| {
            FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the mod's signature is malformed",
                self.signature.clone(),
            )
        })?;
        let key_bytes: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| FabricError::new(ErrorCode::SignatureInvalid, "bad key length"))?;
        let sig_bytes: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| FabricError::new(ErrorCode::SignatureInvalid, "bad signature length"))?;
        let key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| {
            FabricError::new(
                ErrorCode::SignatureInvalid,
                "the mod's public key is not a curve point",
            )
        })?;
        let signature = DalekSignature::from_bytes(&sig_bytes);
        key.verify(digest, &signature).map_err(|_| {
            FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the mod's signature does not verify",
                self.fingerprint(),
            )
        })
    }
}

/// What a trust store says about a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// The pinned key matches.
    Trusted,
    /// The key is pinned as revoked.
    Revoked,
    /// First time this key has been seen.
    Unknown,
}

/// A store of pinned and revoked signing keys, persisted by the launcher.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    /// Fingerprint (SHA-256 of the public key) to the key itself.
    pub pinned: BTreeMap<String, String>,
    /// Fingerprints the owner has revoked.
    pub revoked: BTreeSet<String>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evaluate(&self, signature: &Signature) -> Trust {
        let fingerprint = signature.fingerprint();
        if self.revoked.contains(&fingerprint) {
            return Trust::Revoked;
        }
        match self.pinned.get(&fingerprint) {
            Some(key) if key == &signature.public_key => Trust::Trusted,
            Some(_) => Trust::Unknown,
            None => Trust::Unknown,
        }
    }

    /// Pins a key, removing any revocation for it.
    pub fn pin(&mut self, signature: &Signature) {
        let fingerprint = signature.fingerprint();
        self.revoked.remove(&fingerprint);
        self.pinned
            .insert(fingerprint, signature.public_key.clone());
    }

    /// Revokes a key, keeping its pin for the record.
    pub fn revoke(&mut self, fingerprint: impl Into<String>) {
        self.revoked.insert(fingerprint.into());
    }

    pub fn is_pinned(&self, fingerprint: &str) -> bool {
        self.pinned.contains_key(fingerprint)
    }

    /// Reads a trust store from `path`.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::SignatureUntrusted,
                "the trust store cannot be read",
                format!("{}: {e}", path.display()),
            )
        })?;
        serde_json::from_str(&text).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the trust store is not valid",
                e.to_string(),
            )
        })
    }

    /// Writes the store, creating its folder.
    pub fn write(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                FabricError::with_detail(
                    ErrorCode::SignatureInvalid,
                    "the trust store folder cannot be created",
                    e.to_string(),
                )
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the trust store cannot be written",
                e.to_string(),
            )
        })?;
        std::fs::write(path, text).map_err(|e| {
            FabricError::with_detail(
                ErrorCode::SignatureInvalid,
                "the trust store cannot be written",
                format!("{}: {e}", path.display()),
            )
        })
    }
}

/// Verifies a signature and applies trust-on-first-use.
///
/// `consent` is called with the key fingerprint the first time a key is seen;
/// returning `false` refuses the mod. An unknown key is never trusted silently.
pub fn verify_with_consent(
    store: &mut TrustStore,
    signature: &Signature,
    digest: &[u8; 32],
    mut consent: impl FnMut(&str) -> bool,
) -> Result<Trust> {
    signature.verify(digest)?;
    let fingerprint = signature.fingerprint();
    match store.evaluate(signature) {
        Trust::Trusted => Ok(Trust::Trusted),
        Trust::Revoked => Err(FabricError::with_detail(
            ErrorCode::SignatureRevoked,
            "this mod is signed by a revoked key",
            fingerprint,
        )),
        Trust::Unknown => {
            if consent(&fingerprint) {
                store.pin(signature);
                Ok(Trust::Trusted)
            } else {
                Err(FabricError::with_detail(
                    ErrorCode::SignatureUntrusted,
                    "this mod is signed by an unknown key",
                    fingerprint,
                ))
            }
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn unhex(text: &str, expected: usize) -> std::result::Result<Vec<u8>, ()> {
    if text.len() != expected * 2 || !text.is_ascii() {
        return Err(());
    }
    let mut out = Vec::with_capacity(expected);
    let bytes = text.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16).ok_or(())?;
        let lo = (pair[1] as char).to_digit(16).ok_or(())?;
        out.push((hi * 16 + lo) as u8);
    }
    Ok(out)
}

/// A key pair for tests and the CLI's `verify --sign` path.
pub fn signing_key(seed: [u8; 32]) -> SigningKey {
    SigningKey::from_bytes(&seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> [u8; 32] {
        Sha256::digest(b"a container").into()
    }

    #[test]
    fn signs_and_verifies() {
        let key = signing_key([7u8; 32]);
        let signature = Signature::sign(&digest(), &key);
        signature.verify(&digest()).unwrap();
        let mut other = digest();
        other[0] ^= 1;
        assert_eq!(
            signature.verify(&other).unwrap_err().code,
            ErrorCode::SignatureInvalid
        );
    }

    #[test]
    fn hostile_signature_never_panics() {
        for (key, sig) in [
            ("", ""),
            ("zz", "zz"),
            (&"a".repeat(64), &"b".repeat(128)),
            (&"a".repeat(64), &"b".repeat(4)),
        ] {
            let s = Signature {
                algorithm: "ed25519".into(),
                public_key: key.into(),
                signature: sig.into(),
                digest: hex(&digest()),
            };
            assert!(s.verify(&digest()).is_err());
        }
        let s = Signature {
            algorithm: "rsa".into(),
            public_key: String::new(),
            signature: String::new(),
            digest: hex(&digest()),
        };
        assert!(s.verify(&digest()).is_err());
    }

    #[test]
    fn trust_on_first_use_and_revocation() {
        let key = signing_key([3u8; 32]);
        let signature = Signature::sign(&digest(), &key);
        let mut store = TrustStore::new();
        assert_eq!(store.evaluate(&signature), Trust::Unknown);
        let err = verify_with_consent(&mut store, &signature, &digest(), |_| false).unwrap_err();
        assert_eq!(err.code, ErrorCode::SignatureUntrusted);
        assert_eq!(
            verify_with_consent(&mut store, &signature, &digest(), |_| true).unwrap(),
            Trust::Trusted
        );
        assert!(store.is_pinned(&signature.fingerprint()));
        store.revoke(signature.fingerprint());
        let err = verify_with_consent(&mut store, &signature, &digest(), |_| true).unwrap_err();
        assert_eq!(err.code, ErrorCode::SignatureRevoked);
    }
}
