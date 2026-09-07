//! At-rest encryption for file session-store documents (KRN-6 / STORE-ENCRYPT1).
//!
//! Sealed files use a binary envelope so on-disk bytes are not JSON plaintext.
//! The digest-only WAL remains unencrypted by design (it never stores session
//! payloads). Hosts supply a 32-byte key; Code does not invent key management.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Nonce};
use anyhow::{bail, Context, Result};

const ENVELOPE_MAGIC: &[u8; 4] = b"A3SE";
const ENVELOPE_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 4 + 1 + NONCE_LEN;

/// AES-256-GCM cipher used by [`super::FileSessionStore`] when constructed
/// with an encryption key.
#[derive(Clone)]
pub struct SessionStoreAtRestCipher {
    cipher: Aes256Gcm,
}

impl SessionStoreAtRestCipher {
    pub fn new(key: &[u8; 32]) -> Result<Self> {
        let cipher = Aes256Gcm::new_from_slice(key)
            .context("session store at-rest cipher key must be 32 bytes")?;
        Ok(Self { cipher })
    }

    pub fn is_sealed(bytes: &[u8]) -> bool {
        bytes.len() > HEADER_LEN
            && bytes.starts_with(ENVELOPE_MAGIC)
            && bytes[4] == ENVELOPE_VERSION
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| anyhow::anyhow!("session store at-rest encryption failed"))?;
        let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        out.extend_from_slice(ENVELOPE_MAGIC);
        out.push(ENVELOPE_VERSION);
        out.extend_from_slice(nonce.as_slice());
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        if !Self::is_sealed(sealed) {
            bail!("session store document is not an at-rest sealed envelope");
        }
        let nonce = Nonce::from_slice(&sealed[5..5 + NONCE_LEN]);
        self.cipher
            .decrypt(nonce, &sealed[HEADER_LEN..])
            .map_err(|_| anyhow::anyhow!("session store at-rest decryption failed"))
    }

    /// Decrypt a sealed envelope, or return plaintext bytes unchanged for
    /// migration of pre-encryption documents.
    pub fn open_or_plaintext(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        if Self::is_sealed(bytes) {
            self.open(bytes)
        } else {
            Ok(bytes.to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_round_trip_and_wrong_key_fail_closed() {
        let a = SessionStoreAtRestCipher::new(&[7u8; 32]).unwrap();
        let b = SessionStoreAtRestCipher::new(&[8u8; 32]).unwrap();
        let sealed = a.seal(b"{\"ok\":true}").unwrap();
        assert!(SessionStoreAtRestCipher::is_sealed(&sealed));
        assert_ne!(&sealed[..], b"{\"ok\":true}");
        assert_eq!(a.open(&sealed).unwrap(), b"{\"ok\":true}");
        assert!(b.open(&sealed).is_err());
        assert_eq!(a.open_or_plaintext(b"plain").unwrap(), b"plain");
    }
}
