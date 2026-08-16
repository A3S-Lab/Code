use super::HarnessEvidenceError;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;

const DIGEST_PREFIX: &[u8] = b"a3s-code-harness-evidence\0";

pub(super) struct DigestMeasurement {
    pub(super) digest: String,
    pub(super) bytes: u64,
}

struct DigestWriter {
    hasher: Sha256,
    bytes: u64,
}

impl DigestWriter {
    fn new(domain: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(DIGEST_PREFIX);
        hasher.update((domain.len() as u64).to_be_bytes());
        hasher.update(domain.as_bytes());
        Self { hasher, bytes: 0 }
    }

    fn finish(mut self) -> DigestMeasurement {
        self.hasher.update(self.bytes.to_be_bytes());
        DigestMeasurement {
            digest: format!("sha256:{:x}", self.hasher.finalize()),
            bytes: self.bytes,
        }
    }
}

impl Write for DigestWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(buffer);
        self.bytes = self.bytes.saturating_add(to_u64(buffer.len()));
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn measure<T: Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> Result<DigestMeasurement, HarnessEvidenceError> {
    let mut writer = DigestWriter::new(domain);
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.finish())
}

pub(super) fn require_optional_digest(
    field: &'static str,
    digest: Option<&str>,
) -> Result<(), HarnessEvidenceError> {
    if let Some(digest) = digest {
        require_digest(field, digest)?;
    }
    Ok(())
}

pub(super) fn require_digest(
    field: &'static str,
    digest: &str,
) -> Result<(), HarnessEvidenceError> {
    if digest.len() != 71
        || !digest.starts_with("sha256:")
        || !digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(HarnessEvidenceError::InvalidDigest(field));
    }
    Ok(())
}

pub(super) fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
