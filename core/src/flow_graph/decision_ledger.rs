use crate::execution_identity::{ExecutionIdentityV1, ExecutionResultReceiptV1};
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

    /// Extend a pending claim only while it is still owned by `owner_id`.
    /// Returns `false` after completion, takeover, release, or identity conflict.
    async fn renew(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool>;

    /// Claim a decision while binding its canonical execution identity. The
    /// default preserves source compatibility for host ledgers; built-in
    /// ledgers persist and fence on the identity.
    async fn claim_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<FlowDecisionClaimOutcome> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.claim(decision_id, request_hash, owner_id, now_ms, lease_ms)
            .await
    }

    /// Renew only when both the legacy request key and canonical identity
    /// still belong to the admitted worker.
    async fn renew_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.renew(decision_id, request_hash, owner_id, now_ms, lease_ms)
            .await
    }

    async fn complete(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()>;

    /// Complete a pending claim only when its canonical execution identity
    /// still matches the record that was admitted.  The default keeps custom
    /// host ledgers source-compatible; built-in ledgers persist and fence the
    /// identity together with the owner and lease checks.
    async fn complete_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.complete(decision_id, request_hash, owner_id, completed_at_ms)
            .await
    }

    /// Complete with a bounded digest-only result receipt. The default keeps
    /// third-party ledgers source-compatible but cannot persist the receipt.
    async fn complete_with_receipt(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        receipt: &ExecutionResultReceiptV1,
        completed_at_ms: u64,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        receipt.validate().map_err(|error| anyhow::anyhow!(error))?;
        if &receipt.identity != identity {
            anyhow::bail!("decision result receipt identity conflicts with its claim");
        }
        self.complete(decision_id, request_hash, owner_id, completed_at_ms)
            .await
    }

    async fn release(&self, decision_id: &str, request_hash: &str, owner_id: &str) -> Result<()>;

    /// Release only when the canonical identity also matches. Legacy ledgers
    /// fall back to their existing request/owner fence.
    async fn release_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.release(decision_id, request_hash, owner_id).await
    }

    /// Return a terminal receipt when the ledger supports result persistence.
    async fn completed_receipt(
        &self,
        _decision_id: &str,
    ) -> Result<Option<ExecutionResultReceiptV1>> {
        Ok(None)
    }

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_identity: Option<ExecutionIdentityV1>,
    status: ClaimStatus,
    owner_id: String,
    lease_expires_at_ms: u64,
    attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result_receipt: Option<ExecutionResultReceiptV1>,
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
            None,
            owner_id,
            now_ms,
            lease_ms,
        )
    }

    async fn renew(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool> {
        let mut records = self.records.lock().await;
        Ok(renew_record(
            &mut records,
            decision_id,
            request_hash,
            None,
            owner_id,
            now_ms,
            lease_ms,
        ))
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
            None,
            owner_id,
            None,
            completed_at_ms,
        )
    }

    async fn complete_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        let mut records = self.records.lock().await;
        complete_record(
            &mut records,
            decision_id,
            request_hash,
            Some(identity),
            owner_id,
            None,
            completed_at_ms,
        )
    }

    async fn release(&self, decision_id: &str, request_hash: &str, owner_id: &str) -> Result<()> {
        let mut records = self.records.lock().await;
        release_record(&mut records, decision_id, request_hash, None, owner_id)
    }

    async fn claim_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<FlowDecisionClaimOutcome> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        let mut records = self.records.lock().await;
        claim_record(
            &mut records,
            decision_id,
            request_hash,
            Some(identity),
            owner_id,
            now_ms,
            lease_ms,
        )
    }

    async fn renew_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        let mut records = self.records.lock().await;
        Ok(renew_record(
            &mut records,
            decision_id,
            request_hash,
            Some(identity),
            owner_id,
            now_ms,
            lease_ms,
        ))
    }

    async fn complete_with_receipt(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        receipt: &ExecutionResultReceiptV1,
        completed_at_ms: u64,
    ) -> Result<()> {
        validate_receipt(identity, receipt)?;
        let mut records = self.records.lock().await;
        complete_record(
            &mut records,
            decision_id,
            request_hash,
            Some(identity),
            owner_id,
            Some(receipt),
            completed_at_ms,
        )
    }

    async fn release_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        let mut records = self.records.lock().await;
        release_record(
            &mut records,
            decision_id,
            request_hash,
            Some(identity),
            owner_id,
        )
    }

    async fn completed_receipt(
        &self,
        decision_id: &str,
    ) -> Result<Option<ExecutionResultReceiptV1>> {
        let records = self.records.lock().await;
        Ok(records
            .get(decision_id)
            .and_then(|record| record.result_receipt.clone()))
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
                None,
                owner_id,
                now_ms,
                lease_ms,
            )
        })
        .await
    }

    async fn renew(
        &self,
        decision_id: &str,
        request_hash: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool> {
        self.mutate(|records| {
            Ok(renew_record(
                records,
                decision_id,
                request_hash,
                None,
                owner_id,
                now_ms,
                lease_ms,
            ))
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
                None,
                owner_id,
                None,
                completed_at_ms,
            )
        })
        .await
    }

    async fn complete_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.mutate(|records| {
            complete_record(
                records,
                decision_id,
                request_hash,
                Some(identity),
                owner_id,
                None,
                completed_at_ms,
            )
        })
        .await
    }

    async fn release(&self, decision_id: &str, request_hash: &str, owner_id: &str) -> Result<()> {
        self.mutate(|records| release_record(records, decision_id, request_hash, None, owner_id))
            .await
    }

    async fn claim_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<FlowDecisionClaimOutcome> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.mutate(|records| {
            claim_record(
                records,
                decision_id,
                request_hash,
                Some(identity),
                owner_id,
                now_ms,
                lease_ms,
            )
        })
        .await
    }

    async fn renew_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.mutate(|records| {
            Ok(renew_record(
                records,
                decision_id,
                request_hash,
                Some(identity),
                owner_id,
                now_ms,
                lease_ms,
            ))
        })
        .await
    }

    async fn complete_with_receipt(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        receipt: &ExecutionResultReceiptV1,
        completed_at_ms: u64,
    ) -> Result<()> {
        validate_receipt(identity, receipt)?;
        self.mutate(|records| {
            complete_record(
                records,
                decision_id,
                request_hash,
                Some(identity),
                owner_id,
                Some(receipt),
                completed_at_ms,
            )
        })
        .await
    }

    async fn release_with_identity(
        &self,
        decision_id: &str,
        request_hash: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
    ) -> Result<()> {
        identity
            .validate()
            .map_err(|error| anyhow::anyhow!(error))?;
        self.mutate(|records| {
            release_record(records, decision_id, request_hash, Some(identity), owner_id)
        })
        .await
    }

    async fn completed_receipt(
        &self,
        decision_id: &str,
    ) -> Result<Option<ExecutionResultReceiptV1>> {
        let records = read_records(&self.data_path()).await?;
        Ok(records
            .get(decision_id)
            .and_then(|record| record.result_receipt.clone()))
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
    execution_identity: Option<&ExecutionIdentityV1>,
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
                    execution_identity: execution_identity.cloned(),
                    status: ClaimStatus::Pending,
                    owner_id: owner_id.to_string(),
                    lease_expires_at_ms,
                    attempts: 1,
                    completed_at_ms: None,
                    result_receipt: None,
                },
            );
            Ok(FlowDecisionClaimOutcome::Claimed { attempt: 1 })
        }
        Some(record) if record.request_hash != request_hash => {
            Ok(FlowDecisionClaimOutcome::Conflict)
        }
        Some(record) if identity_conflicts(record, execution_identity) => {
            Ok(FlowDecisionClaimOutcome::Conflict)
        }
        Some(record) if record.status == ClaimStatus::Completed => {
            if record.execution_identity.is_none() {
                record.execution_identity = execution_identity.cloned();
            }
            Ok(FlowDecisionClaimOutcome::Completed)
        }
        Some(record) if record.lease_expires_at_ms > now_ms && record.owner_id != owner_id => {
            Ok(FlowDecisionClaimOutcome::Busy {
                lease_expires_at_ms: record.lease_expires_at_ms,
            })
        }
        Some(record) => {
            if record.execution_identity.is_none() {
                record.execution_identity = execution_identity.cloned();
            }
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
    execution_identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
    result_receipt: Option<&ExecutionResultReceiptV1>,
    completed_at_ms: u64,
) -> Result<()> {
    let record = records
        .get_mut(decision_id)
        .with_context(|| format!("decision claim `{decision_id}` does not exist"))?;
    if record.request_hash != request_hash {
        anyhow::bail!("decision `{decision_id}` request hash conflicts with its claim");
    }
    if identity_conflicts(record, execution_identity) {
        anyhow::bail!("decision `{decision_id}` execution identity conflicts with its claim");
    }
    if record.status == ClaimStatus::Completed {
        if let Some(receipt) = result_receipt {
            if let Some(existing) = record.result_receipt.as_ref() {
                if existing != receipt {
                    anyhow::bail!(
                        "decision `{decision_id}` result receipt conflicts with its completion"
                    );
                }
            } else {
                record.result_receipt = Some(receipt.clone());
            }
        }
        if record.execution_identity.is_none() {
            record.execution_identity = execution_identity.cloned();
        }
        return Ok(());
    }
    if record.owner_id != owner_id {
        anyhow::bail!("decision `{decision_id}` is owned by another dispatcher");
    }
    if record.lease_expires_at_ms == 0 || record.lease_expires_at_ms <= completed_at_ms {
        anyhow::bail!("decision `{decision_id}` claim lease expired before completion");
    }
    if record.execution_identity.is_none() {
        record.execution_identity = execution_identity.cloned();
    }
    record.status = ClaimStatus::Completed;
    record.lease_expires_at_ms = 0;
    record.completed_at_ms = Some(completed_at_ms);
    record.result_receipt = result_receipt.cloned();
    Ok(())
}

fn renew_record(
    records: &mut BTreeMap<String, ClaimRecord>,
    decision_id: &str,
    request_hash: &str,
    execution_identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
    now_ms: u64,
    lease_ms: u64,
) -> bool {
    let Some(record) = records.get_mut(decision_id) else {
        return false;
    };
    if record.request_hash != request_hash
        || record.status != ClaimStatus::Pending
        || record.owner_id != owner_id
        || record.lease_expires_at_ms <= now_ms
    {
        return false;
    }
    if identity_conflicts(record, execution_identity) {
        return false;
    }
    record.lease_expires_at_ms = now_ms.saturating_add(lease_ms.max(1));
    true
}

fn release_record(
    records: &mut BTreeMap<String, ClaimRecord>,
    decision_id: &str,
    request_hash: &str,
    execution_identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
) -> Result<()> {
    let Some(record) = records.get_mut(decision_id) else {
        return Ok(());
    };
    if record.request_hash != request_hash
        || record.status == ClaimStatus::Completed
        || identity_conflicts(record, execution_identity)
    {
        return Ok(());
    }
    if record.owner_id == owner_id {
        record.owner_id.clear();
        record.lease_expires_at_ms = 0;
    }
    Ok(())
}

fn identity_conflicts(
    record: &ClaimRecord,
    execution_identity: Option<&ExecutionIdentityV1>,
) -> bool {
    matches!(
        (record.execution_identity.as_ref(), execution_identity),
        (Some(stored), Some(supplied)) if stored != supplied
    )
}

fn validate_receipt(
    identity: &ExecutionIdentityV1,
    receipt: &ExecutionResultReceiptV1,
) -> Result<()> {
    identity
        .validate()
        .map_err(|error| anyhow::anyhow!(error))?;
    receipt.validate().map_err(|error| anyhow::anyhow!(error))?;
    if &receipt.identity != identity {
        anyhow::bail!("decision result receipt identity conflicts with its claim");
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
    for (decision_id, record) in &ledger.records {
        if let Some(identity) = record.execution_identity.as_ref() {
            identity
                .validate()
                .with_context(|| format!("validate identity for decision `{decision_id}`"))?;
        }
        if let Some(receipt) = record.result_receipt.as_ref() {
            receipt
                .validate()
                .with_context(|| format!("validate result receipt for decision `{decision_id}`"))?;
            if record.status != ClaimStatus::Completed {
                anyhow::bail!("decision `{decision_id}` has a result receipt before completion");
            }
            let Some(identity) = record.execution_identity.as_ref() else {
                anyhow::bail!(
                    "decision `{decision_id}` result receipt has no execution identity binding"
                );
            };
            if &receipt.identity != identity {
                anyhow::bail!(
                    "decision `{decision_id}` result receipt identity conflicts with its claim"
                );
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluation::digest_bytes;

    fn identity(tag: &str) -> ExecutionIdentityV1 {
        ExecutionIdentityV1::derive("a3s.test.flow-decision", &serde_json::json!({ "tag": tag }))
            .unwrap()
    }

    fn receipt(identity: ExecutionIdentityV1) -> ExecutionResultReceiptV1 {
        ExecutionResultReceiptV1::new(
            identity,
            digest_bytes("a3s.test.flow-evidence", b"event"),
            crate::execution_identity::ExecutionResultOutcomeV1::Succeeded,
            Some(digest_bytes("a3s.test.flow-result", b"result")),
            6,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn identity_fences_claim_renew_release_and_completion() {
        let ledger = MemoryFlowDecisionLedger::new();
        let first = identity("first");
        let other = identity("other");
        assert_eq!(
            ledger
                .claim_with_identity("decision", "hash", &first, "owner", 100, 50)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 1 }
        );
        assert!(!ledger
            .renew_with_identity("decision", "hash", &other, "owner", 110, 50)
            .await
            .unwrap());
        ledger
            .release_with_identity("decision", "hash", &other, "owner")
            .await
            .unwrap();
        assert!(ledger
            .renew_with_identity("decision", "hash", &first, "owner", 110, 50)
            .await
            .unwrap());
        let mismatched_receipt = receipt(other);
        assert!(ledger
            .complete_with_receipt(
                "decision",
                "hash",
                &first,
                "owner",
                &mismatched_receipt,
                120,
            )
            .await
            .is_err());
        let result_receipt = receipt(first.clone());
        ledger
            .complete_with_receipt("decision", "hash", &first, "owner", &result_receipt, 121)
            .await
            .unwrap();
        assert_eq!(
            ledger.completed_receipt("decision").await.unwrap(),
            Some(result_receipt)
        );
        assert!(!ledger
            .renew_with_identity("decision", "hash", &first, "owner", 130, 50)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn file_ledger_persists_identity_and_result_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = FileFlowDecisionLedger::new(directory.path());
        let execution_identity = identity("persisted");
        let result_receipt = receipt(execution_identity.clone());
        ledger
            .claim_with_identity("decision", "hash", &execution_identity, "owner", 100, 50)
            .await
            .unwrap();
        ledger
            .complete_with_receipt(
                "decision",
                "hash",
                &execution_identity,
                "owner",
                &result_receipt,
                120,
            )
            .await
            .unwrap();
        let reopened = FileFlowDecisionLedger::new(directory.path());
        assert_eq!(
            reopened.completed_receipt("decision").await.unwrap(),
            Some(result_receipt)
        );
        assert_eq!(
            reopened
                .claim_with_identity("decision", "hash", &execution_identity, "other", 130, 50,)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Completed
        );
    }

    #[tokio::test]
    async fn expired_worker_cannot_complete_after_takeover() {
        let ledger = MemoryFlowDecisionLedger::new();
        let first = identity("expired-first");
        // A retry of the same logical decision keeps the identity and changes
        // only the worker owner; the old owner must still be fenced.
        let second = first.clone();
        ledger
            .claim_with_identity("decision", "hash", &first, "first-owner", 100, 20)
            .await
            .unwrap();
        let first_receipt = receipt(first.clone());
        assert!(ledger
            .complete_with_receipt(
                "decision",
                "hash",
                &first,
                "first-owner",
                &first_receipt,
                121,
            )
            .await
            .is_err());
        assert!(ledger
            .complete_with_identity("decision", "hash", &first, "other", 120)
            .await
            .is_err());
        assert_eq!(
            ledger
                .claim_with_identity("decision", "hash", &second, "second-owner", 121, 20)
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 2 }
        );
        assert!(ledger
            .complete_with_receipt(
                "decision",
                "hash",
                &first,
                "first-owner",
                &first_receipt,
                122,
            )
            .await
            .is_err());
        let second_receipt = receipt(second.clone());
        ledger
            .complete_with_identity("decision", "hash", &second, "second-owner", 123)
            .await
            .unwrap();
        ledger
            .complete_with_receipt(
                "decision",
                "hash",
                &second,
                "second-owner",
                &second_receipt,
                123,
            )
            .await
            .unwrap();
        assert_eq!(
            ledger.completed_receipt("decision").await.unwrap(),
            Some(second_receipt)
        );
    }

    #[tokio::test]
    async fn legacy_file_record_without_identity_remains_claimable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("flow-decisions.json");
        let legacy = serde_json::json!({
            "schema_version": 1,
            "records": {
                "decision": {
                    "request_hash": "hash",
                    "status": "pending",
                    "owner_id": "old-owner",
                    "lease_expires_at_ms": 0,
                    "attempts": 1
                }
            }
        });
        tokio::fs::write(&path, serde_json::to_vec(&legacy).unwrap())
            .await
            .unwrap();
        let ledger = FileFlowDecisionLedger::new(directory.path());
        let execution_identity = identity("legacy-upgrade");
        assert_eq!(
            ledger
                .claim_with_identity(
                    "decision",
                    "hash",
                    &execution_identity,
                    "new-owner",
                    100,
                    50,
                )
                .await
                .unwrap(),
            FlowDecisionClaimOutcome::Claimed { attempt: 2 }
        );
        let result_receipt = receipt(execution_identity.clone());
        ledger
            .complete_with_receipt(
                "decision",
                "hash",
                &execution_identity,
                "new-owner",
                &result_receipt,
                120,
            )
            .await
            .unwrap();
        assert_eq!(
            ledger.completed_receipt("decision").await.unwrap(),
            Some(result_receipt)
        );
    }
}
