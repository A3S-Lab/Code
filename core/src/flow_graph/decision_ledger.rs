use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const MAX_DECISION_LEDGER_BYTES: usize = 16 * 1024 * 1024;
const DECISION_LEDGER_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowDecisionClaimOutcome {
    Claimed { attempt: u32 },
    Completed,
    Busy { lease_expires_at_ms: u64 },
    Conflict,
}

#[async_trait]
pub trait FlowDecisionLedger: Send + Sync {
    async fn claim(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<FlowDecisionClaimOutcome>;

    async fn complete(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()>;

    async fn release(&self, decision_id: &str, request_hash: &str, owner_id: &str) -> Result<()>;

    /// Remove completed receipts older than the host's retention cutoff.
    async fn prune_completed(&self, _before_ms: u64) -> Result<usize> {
        anyhow::bail!("Flow decision ledger does not support receipt pruning")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClaimStatus {
    Pending,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClaimRecord {
    request_hash: String,
    status: ClaimStatus,
    owner_id: String,
    lease_expires_at_ms: u64,
    attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DecisionLedgerFile {
    schema_version: u32,
    records: BTreeMap<String, ClaimRecord>,
}

#[derive(Serialize)]
struct DecisionLedgerFileRef<'a> {
    schema_version: u32,
    records: &'a BTreeMap<String, ClaimRecord>,
}

#[derive(Debug, Default)]
pub struct MemoryFlowDecisionLedger {
    records: Mutex<BTreeMap<String, ClaimRecord>>,
}

impl MemoryFlowDecisionLedger {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FlowDecisionLedger for MemoryFlowDecisionLedger {
    async fn claim(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<FlowDecisionClaimOutcome> {
        let mut records = self.records.lock().await;
        claim_record(
            &mut records,
            decision_id,
            request_hash,
            owner_id,
            now_ms,
            lease_ms,
        )
    }

    async fn complete(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()> {
        let mut records = self.records.lock().await;
        complete_record(
            &mut records,
            decision_id,
            request_hash,
            owner_id,
            completed_at_ms,
        )
    }

    async fn release(&self, decision_id: &str, request_hash: &str, owner_id: &str) -> Result<()> {
        let mut records = self.records.lock().await;
        release_record(&mut records, decision_id, request_hash, owner_id)
    }

    async fn prune_completed(&self, before_ms: u64) -> Result<usize> {
        let mut records = self.records.lock().await;
        Ok(prune_records(&mut records, before_ms))
    }
}

#[derive(Debug)]
pub struct FileFlowDecisionLedger {
    root: PathBuf,
    process_lock: Mutex<()>,
}

impl FileFlowDecisionLedger {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            process_lock: Mutex::new(()),
        }
    }

    fn data_path(&self) -> PathBuf {
        self.root.join("flow-decisions.json")
    }

    async fn acquire_file_lock(&self) -> Result<std::fs::File> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .with_context(|| format!("create decision ledger `{}`", self.root.display()))?;
        let path = self.root.join(".flow-decisions.lock");
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("open decision ledger lock `{}`", path.display()))?;
            file.lock_exclusive()
                .with_context(|| format!("lock decision ledger `{}`", path.display()))?;
            Ok(file)
        })
        .await
        .context("decision ledger lock task failed")?
    }

    async fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut BTreeMap<String, ClaimRecord>) -> Result<T>,
    ) -> Result<T> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        let mut records = read_records(&self.data_path()).await?;
        let result = mutation(&mut records)?;
        write_records(&self.data_path(), &records).await?;
        Ok(result)
    }
}

#[async_trait]
impl FlowDecisionLedger for FileFlowDecisionLedger {
    async fn claim(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<FlowDecisionClaimOutcome> {
        self.mutate(|records| {
            claim_record(
                records,
                decision_id,
                request_hash,
                owner_id,
                now_ms,
                lease_ms,
            )
        })
        .await
    }

    async fn complete(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()> {
        self.mutate(|records| {
            complete_record(
                records,
                decision_id,
                request_hash,
                owner_id,
                completed_at_ms,
            )
        })
        .await
    }

    async fn release(&self, decision_id: &str, request_hash: &str, owner_id: &str) -> Result<()> {
        self.mutate(|records| release_record(records, decision_id, request_hash, owner_id))
            .await
    }

    async fn prune_completed(&self, before_ms: u64) -> Result<usize> {
        self.mutate(|records| Ok(prune_records(records, before_ms)))
            .await
    }
}

fn claim_record(
    records: &mut BTreeMap<String, ClaimRecord>,
    decision_id: &str,
    request_hash: &str,
    owner_id: &str,
    now_ms: u64,
    lease_ms: u64,
) -> Result<FlowDecisionClaimOutcome> {
    let lease_expires_at_ms = now_ms.saturating_add(lease_ms.max(1));
    match records.get_mut(decision_id) {
        None => {
            records.insert(
                decision_id.to_string(),
                ClaimRecord {
                    request_hash: request_hash.to_string(),
                    status: ClaimStatus::Pending,
                    owner_id: owner_id.to_string(),
                    lease_expires_at_ms,
                    attempts: 1,
                    completed_at_ms: None,
                },
            );
            Ok(FlowDecisionClaimOutcome::Claimed { attempt: 1 })
        }
        Some(record) if record.request_hash != request_hash => {
            Ok(FlowDecisionClaimOutcome::Conflict)
        }
        Some(record) if record.status == ClaimStatus::Completed => {
            Ok(FlowDecisionClaimOutcome::Completed)
        }
        Some(record) if record.lease_expires_at_ms > now_ms && record.owner_id != owner_id => {
            Ok(FlowDecisionClaimOutcome::Busy {
                lease_expires_at_ms: record.lease_expires_at_ms,
            })
        }
        Some(record) => {
            record.owner_id = owner_id.to_string();
            record.lease_expires_at_ms = lease_expires_at_ms;
            record.attempts = record.attempts.saturating_add(1);
            Ok(FlowDecisionClaimOutcome::Claimed {
                attempt: record.attempts,
            })
        }
    }
}

fn complete_record(
    records: &mut BTreeMap<String, ClaimRecord>,
    decision_id: &str,
    request_hash: &str,
    owner_id: &str,
    completed_at_ms: u64,
) -> Result<()> {
    let record = records
        .get_mut(decision_id)
        .with_context(|| format!("decision claim `{decision_id}` does not exist"))?;
    if record.request_hash != request_hash {
        anyhow::bail!("decision `{decision_id}` request hash conflicts with its claim");
    }
    if record.status == ClaimStatus::Completed {
        return Ok(());
    }
    if record.owner_id != owner_id {
        anyhow::bail!("decision `{decision_id}` is owned by another dispatcher");
    }
    record.status = ClaimStatus::Completed;
    record.lease_expires_at_ms = 0;
    record.completed_at_ms = Some(completed_at_ms);
    Ok(())
}

fn release_record(
    records: &mut BTreeMap<String, ClaimRecord>,
    decision_id: &str,
    request_hash: &str,
    owner_id: &str,
) -> Result<()> {
    let Some(record) = records.get_mut(decision_id) else {
        return Ok(());
    };
    if record.request_hash != request_hash || record.status == ClaimStatus::Completed {
        return Ok(());
    }
    if record.owner_id == owner_id {
        record.owner_id.clear();
        record.lease_expires_at_ms = 0;
    }
    Ok(())
}

fn prune_records(records: &mut BTreeMap<String, ClaimRecord>, before_ms: u64) -> usize {
    let before = records.len();
    records.retain(|_, record| {
        record.status != ClaimStatus::Completed
            || record.completed_at_ms.unwrap_or(u64::MAX) >= before_ms
    });
    before - records.len()
}

async fn read_records(path: &Path) -> Result<BTreeMap<String, ClaimRecord>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error).context("read decision ledger"),
    };
    if bytes.len() > MAX_DECISION_LEDGER_BYTES {
        anyhow::bail!("decision ledger exceeds {MAX_DECISION_LEDGER_BYTES} bytes");
    }
    let ledger: DecisionLedgerFile =
        serde_json::from_slice(&bytes).context("decode decision ledger")?;
    if ledger.schema_version > DECISION_LEDGER_SCHEMA_VERSION {
        anyhow::bail!(
            "decision ledger schema {} is newer than supported schema {}",
            ledger.schema_version,
            DECISION_LEDGER_SCHEMA_VERSION
        );
    }
    Ok(ledger.records)
}

async fn write_records(path: &Path, records: &BTreeMap<String, ClaimRecord>) -> Result<()> {
    let bytes = serde_json::to_vec(&DecisionLedgerFileRef {
        schema_version: DECISION_LEDGER_SCHEMA_VERSION,
        records,
    })
    .context("encode decision ledger")?;
    if bytes.len() > MAX_DECISION_LEDGER_BYTES {
        anyhow::bail!("decision ledger exceeds {MAX_DECISION_LEDGER_BYTES} bytes");
    }
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::File::create(&temp)
            .await
            .context("create decision ledger generation")?;
        file.write_all(&bytes)
            .await
            .context("write decision ledger")?;
        file.sync_all().await.context("sync decision ledger")?;
        drop(file);
        let temp_copy = temp.clone();
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            tempfile::TempPath::try_from_path(temp_copy)?
                .persist(path)
                .map_err(|error| error.error)
        })
        .await
        .context("publish decision ledger task failed")??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temp).await;
    }
    result
}
