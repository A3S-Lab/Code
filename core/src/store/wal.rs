//! Append-only session-store write-ahead log (KRN-6 / STORE-WAL1).
//!
//! The file adapter records an intent before replacing a session snapshot and
//! a commit after the atomic replace succeeds. Reopen recovers by sealing any
//! intent whose durable snapshot already matches, so a crash between rename
//! and commit acknowledgement cannot lose the generation or invent a second
//! one. The log never stores session plaintext payloads — only digests and
//! identities.

use crate::evaluation::{digest_json, validate_digest};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const SESSION_STORE_WAL_ENTRY_SCHEMA_V1: &str = "a3s.code.session-store-wal-entry.v1";
const SESSION_STORE_WAL_DIGEST_DOMAIN: &str = "a3s.code.session-store-wal-entry.identity.v1";
const SESSION_STORE_SNAPSHOT_DIGEST_DOMAIN: &str = "a3s.code.session-store-snapshot.identity.v1";

/// Lifecycle phase of one WAL record for a session snapshot commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStoreWalPhaseV1 {
    Intent,
    Committed,
}

/// One append-only WAL record for a FileSessionStore snapshot commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStoreWalEntryV1 {
    pub schema: String,
    pub sequence: u64,
    pub session_id: String,
    pub snapshot_digest: String,
    pub phase: SessionStoreWalPhaseV1,
    pub recorded_at_ms: u64,
    pub entry_digest: String,
}

impl SessionStoreWalEntryV1 {
    pub fn new(
        sequence: u64,
        session_id: impl Into<String>,
        snapshot_digest: impl Into<String>,
        phase: SessionStoreWalPhaseV1,
        recorded_at_ms: u64,
    ) -> Result<Self> {
        let mut entry = Self {
            schema: SESSION_STORE_WAL_ENTRY_SCHEMA_V1.to_owned(),
            sequence,
            session_id: session_id.into(),
            snapshot_digest: snapshot_digest.into(),
            phase,
            recorded_at_ms,
            entry_digest: String::new(),
        };
        entry.validate_without_digest()?;
        entry.entry_digest = entry.expected_digest()?;
        Ok(entry)
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_without_digest()?;
        validate_digest(&self.entry_digest)
            .map_err(|_| anyhow::anyhow!("session store WAL entryDigest is invalid"))?;
        if self.entry_digest != self.expected_digest()? {
            bail!("session store WAL entryDigest does not match contents");
        }
        Ok(())
    }

    fn validate_without_digest(&self) -> Result<()> {
        if self.schema != SESSION_STORE_WAL_ENTRY_SCHEMA_V1 {
            bail!("session store WAL schema is unsupported");
        }
        if self.sequence == 0 {
            bail!("session store WAL sequence must be one-based");
        }
        if self.session_id.is_empty()
            || self.session_id.contains('\0')
            || self.session_id.contains(['\r', '\n'])
        {
            bail!("session store WAL sessionId is invalid");
        }
        validate_digest(&self.snapshot_digest)
            .map_err(|_| anyhow::anyhow!("session store WAL snapshotDigest is invalid"))?;
        Ok(())
    }

    fn expected_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            sequence: u64,
            session_id: &'a str,
            snapshot_digest: &'a str,
            phase: SessionStoreWalPhaseV1,
            recorded_at_ms: u64,
        }
        digest_json(
            SESSION_STORE_WAL_DIGEST_DOMAIN,
            &Identity {
                schema: &self.schema,
                sequence: self.sequence,
                session_id: &self.session_id,
                snapshot_digest: &self.snapshot_digest,
                phase: self.phase,
                recorded_at_ms: self.recorded_at_ms,
            },
        )
        .context("failed to digest session store WAL entry")
    }
}

/// Digest a complete session snapshot for WAL identity fencing.
pub fn snapshot_content_digest<T: Serialize>(snapshot: &T) -> Result<String> {
    digest_json(SESSION_STORE_SNAPSHOT_DIGEST_DOMAIN, snapshot)
        .context("failed to digest session snapshot for WAL")
}

/// Durable append-only JSONL WAL under a FileSessionStore root.
pub struct FileSessionStoreWal {
    path: PathBuf,
}

impl FileSessionStoreWal {
    pub fn new(store_root: impl AsRef<Path>) -> Self {
        Self {
            path: store_root
                .as_ref()
                .join("v1")
                .join("wal")
                .join("session-store.ndjson"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load every validated entry and return the next one-based sequence.
    pub async fn load_entries(&self) -> Result<(Vec<SessionStoreWalEntryV1>, u64)> {
        if !self.path.exists() {
            return Ok((Vec::new(), 1));
        }
        let file = File::open(&self.path).await.with_context(|| {
            format!("Failed to open session store WAL: {}", self.path.display())
        })?;
        let mut lines = BufReader::new(file).lines();
        let mut entries = Vec::new();
        let mut seen_sequences = BTreeMap::new();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let entry: SessionStoreWalEntryV1 = serde_json::from_str(&line).with_context(|| {
                format!(
                    "Failed to decode session store WAL line in {}",
                    self.path.display()
                )
            })?;
            entry.validate()?;
            if let Some(previous) = seen_sequences.insert(entry.sequence, entry.phase) {
                // Intent then Committed for the same sequence is the only
                // allowed reuse; any other collision fails closed.
                let allowed = matches!(previous, SessionStoreWalPhaseV1::Intent)
                    && matches!(entry.phase, SessionStoreWalPhaseV1::Committed);
                if !allowed {
                    bail!(
                        "session store WAL sequence {} conflicts with a retained entry",
                        entry.sequence
                    );
                }
            }
            entries.push(entry);
        }
        let next = entries
            .iter()
            .map(|entry| entry.sequence)
            .max()
            .map(|max| max + 1)
            .unwrap_or(1);
        Ok((entries, next))
    }

    /// Append one validated entry with a data sync so reopen can observe it.
    pub async fn append(&self, entry: &SessionStoreWalEntryV1) -> Result<()> {
        entry.validate()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create WAL directory: {}", parent.display()))?;
        }
        let mut encoded = serde_json::to_vec(entry).context("Failed to encode WAL entry")?;
        encoded.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .with_context(|| format!("Failed to open WAL for append: {}", self.path.display()))?;
        file.write_all(&encoded)
            .await
            .with_context(|| format!("Failed to append WAL entry to {}", self.path.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("Failed to sync WAL entry to {}", self.path.display()))?;
        Ok(())
    }

    /// Seal open intents whose durable snapshot already matches.
    ///
    /// `snapshot_digest_for` returns the digest of the currently durable
    /// snapshot for a session, if any. Matching intents are acknowledged with
    /// a Committed record so crash recovery does not leave a generation
    /// half-published.
    pub async fn recover<F, Fut>(&self, mut snapshot_digest_for: F) -> Result<u64>
    where
        F: FnMut(&str) -> Fut,
        Fut: std::future::Future<Output = Result<Option<String>>>,
    {
        let (entries, mut next_sequence) = self.load_entries().await?;
        let mut open_intents: BTreeMap<u64, SessionStoreWalEntryV1> = BTreeMap::new();
        for entry in entries {
            match entry.phase {
                SessionStoreWalPhaseV1::Intent => {
                    open_intents.insert(entry.sequence, entry);
                }
                SessionStoreWalPhaseV1::Committed => {
                    open_intents.remove(&entry.sequence);
                }
            }
        }
        for (_, intent) in open_intents {
            let Some(current) = snapshot_digest_for(&intent.session_id).await? else {
                continue;
            };
            if current != intent.snapshot_digest {
                continue;
            }
            let committed = SessionStoreWalEntryV1::new(
                intent.sequence,
                intent.session_id,
                intent.snapshot_digest,
                SessionStoreWalPhaseV1::Committed,
                intent.recorded_at_ms.saturating_add(1),
            )?;
            self.append(&committed).await?;
            next_sequence = next_sequence.max(committed.sequence + 1);
        }
        Ok(next_sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    #[test]
    fn wal_entry_is_digest_bound_and_rejects_zero_sequence() {
        let entry = SessionStoreWalEntryV1::new(
            1,
            "session-1",
            digest('a'),
            SessionStoreWalPhaseV1::Intent,
            10,
        )
        .unwrap();
        entry.validate().unwrap();
        assert!(SessionStoreWalEntryV1::new(
            0,
            "session-1",
            digest('a'),
            SessionStoreWalPhaseV1::Intent,
            10,
        )
        .is_err());
    }
}
