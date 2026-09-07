//! Writer lease fencing for session stores (KRN-6 / STORE-LEASE1).
//!
//! A durable lease epoch fences cross-process writers: after a takeover
//! bumps the epoch, a stale holder cannot commit a newer snapshot generation.
//! The lease never stores session plaintext — only holder identity and epoch.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const SESSION_STORE_WRITER_LEASE_SCHEMA_V1: &str = "a3s.code.session-store-writer-lease.v1";

/// Durable writer lease published by a session store that advertises
/// [`super::SessionStoreCapabilities::lease_fencing`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStoreWriterLeaseV1 {
    pub schema: String,
    /// Monotonic epoch; each successful acquire bumps by one.
    pub epoch: u64,
    /// Caller-supplied holder identity (process, host, or worker id).
    pub holder_id: String,
    pub acquired_at_ms: u64,
}

impl SessionStoreWriterLeaseV1 {
    pub fn new(epoch: u64, holder_id: impl Into<String>, acquired_at_ms: u64) -> Result<Self> {
        let lease = Self {
            schema: SESSION_STORE_WRITER_LEASE_SCHEMA_V1.to_owned(),
            epoch,
            holder_id: holder_id.into(),
            acquired_at_ms,
        };
        lease.validate()?;
        Ok(lease)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SESSION_STORE_WRITER_LEASE_SCHEMA_V1 {
            bail!("session store writer lease schema is unsupported");
        }
        if self.epoch == 0 {
            bail!("session store writer lease epoch must be one-based");
        }
        if self.holder_id.trim().is_empty() {
            bail!("session store writer lease holder_id must be non-empty");
        }
        Ok(())
    }

    pub fn matches_holder(&self, other: &Self) -> bool {
        self.epoch == other.epoch && self.holder_id == other.holder_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_lease_rejects_empty_holder_and_zero_epoch() {
        assert!(SessionStoreWriterLeaseV1::new(0, "a", 1).is_err());
        assert!(SessionStoreWriterLeaseV1::new(1, "  ", 1).is_err());
        let ok = SessionStoreWriterLeaseV1::new(1, "worker-a", 10).unwrap();
        assert_eq!(ok.epoch, 1);
        assert!(ok.matches_holder(&ok));
    }
}
