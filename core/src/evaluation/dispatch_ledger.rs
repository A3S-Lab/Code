//! Durable, provider-neutral dispatch claims for auxiliary evaluations.
//!
//! The ledger records only an evaluator dispatch identity, its lease, and an
//! optional digest-only terminal result receipt. It does not record prompts,
//! rubric decisions, tenant data, or business audit state. A host may use the
//! in-memory implementation for a process-scoped supervisor or the file
//! implementation when replay protection must survive a restart.

use super::identity::validate_digest;
use crate::execution_identity::{ExecutionIdentityV1, ExecutionResultReceiptV1};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

pub const EVALUATION_DISPATCH_LEDGER_SCHEMA_V1: &str = "a3s.code.evaluation-dispatch-ledger.v1";
pub const EVALUATION_DISPATCH_LEDGER_DEFAULT_MAX_RECORDS: usize = 4096;
pub const EVALUATION_DISPATCH_LEDGER_MAX_BYTES: usize = 16 * 1024 * 1024;
pub const EVALUATION_DISPATCH_MIN_LEASE_MS: u64 = 60 * 1000;
pub const EVALUATION_DISPATCH_LEASE_GRACE_MS: u64 = 30 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationDispatchClaimOutcome {
    Claimed { attempt: u32 },
    Completed,
    Busy { lease_expires_at_ms: u64 },
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvaluationDispatchLedgerError {
    #[error("evaluation dispatch ledger field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("evaluation dispatch ledger schema is unsupported")]
    UnsupportedSchema,
    #[error("evaluation dispatch ledger claim conflicts with an existing identity")]
    Conflict,
    #[error("evaluation dispatch ledger storage failed: {0}")]
    Storage(String),
    #[error("evaluation dispatch ledger is corrupt: {0}")]
    Corrupt(String),
    #[error("evaluation dispatch ledger exceeds its configured size limit")]
    SizeLimit,
}

#[async_trait]
pub trait EvaluationDispatchLedger: Send + Sync {
    async fn claim(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError>;

    /// Claim a dispatch while binding its canonical execution identity. The
    /// default keeps third-party ledgers source-compatible; built-in ledgers
    /// persist and fence on the identity.
    async fn claim_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError> {
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        self.claim(dispatch_id, request_digest, owner_id, now_ms, lease_ms)
            .await
    }

    async fn renew(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool, EvaluationDispatchLedgerError>;

    /// Renew a claim only when both its legacy ledger key and canonical
    /// identity still match the admitted worker.
    async fn renew_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool, EvaluationDispatchLedgerError> {
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        self.renew(dispatch_id, request_digest, owner_id, now_ms, lease_ms)
            .await
    }

    async fn complete(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<(), EvaluationDispatchLedgerError>;

    /// Complete a claim with a bounded digest-only result receipt.
    async fn complete_with_receipt(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        receipt: &ExecutionResultReceiptV1,
        completed_at_ms: u64,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        receipt
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("result_receipt"))?;
        if &receipt.identity != identity {
            return Err(EvaluationDispatchLedgerError::Conflict);
        }
        self.complete(dispatch_id, request_digest, owner_id, completed_at_ms)
            .await
    }

    async fn release(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
    ) -> Result<(), EvaluationDispatchLedgerError>;

    /// Release a claim only when its canonical identity also matches. Legacy
    /// implementations fall back to the existing request/owner fence.
    async fn release_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        self.release(dispatch_id, request_digest, owner_id).await
    }

    /// Read the terminal receipt when the ledger supports result persistence.
    async fn completed_receipt(
        &self,
        _dispatch_id: &str,
    ) -> Result<Option<ExecutionResultReceiptV1>, EvaluationDispatchLedgerError> {
        Ok(None)
    }

    async fn prune_completed(&self, before_ms: u64)
        -> Result<usize, EvaluationDispatchLedgerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DispatchClaimStatus {
    Pending,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchClaimRecord {
    request_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_identity: Option<ExecutionIdentityV1>,
    status: DispatchClaimStatus,
    owner_id: String,
    lease_expires_at_ms: u64,
    attempts: u32,
    created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result_receipt: Option<ExecutionResultReceiptV1>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchLedgerFileV1 {
    schema: String,
    records: BTreeMap<String, DispatchClaimRecord>,
}

#[derive(Debug, Default)]
pub struct MemoryEvaluationDispatchLedger {
    records: Mutex<BTreeMap<String, DispatchClaimRecord>>,
}

impl MemoryEvaluationDispatchLedger {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EvaluationDispatchLedger for MemoryEvaluationDispatchLedger {
    async fn claim(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        let mut records = self.records.lock().await;
        claim_record(
            &mut records,
            dispatch_id,
            request_digest,
            None,
            owner_id,
            now_ms,
            lease_ms,
        )
    }

    async fn claim_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        let mut records = self.records.lock().await;
        claim_record(
            &mut records,
            dispatch_id,
            request_digest,
            Some(identity),
            owner_id,
            now_ms,
            lease_ms,
        )
    }

    async fn renew(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        let mut records = self.records.lock().await;
        Ok(renew_record(
            &mut records,
            dispatch_id,
            request_digest,
            None,
            owner_id,
            now_ms,
            lease_ms,
        ))
    }

    async fn renew_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        let mut records = self.records.lock().await;
        Ok(renew_record(
            &mut records,
            dispatch_id,
            request_digest,
            Some(identity),
            owner_id,
            now_ms,
            lease_ms,
        ))
    }

    async fn complete(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        let mut records = self.records.lock().await;
        complete_record(
            &mut records,
            dispatch_id,
            request_digest,
            None,
            owner_id,
            None,
            completed_at_ms,
        )
    }

    async fn complete_with_receipt(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        receipt: &ExecutionResultReceiptV1,
        completed_at_ms: u64,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        receipt
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("result_receipt"))?;
        if &receipt.identity != identity {
            return Err(EvaluationDispatchLedgerError::Conflict);
        }
        let mut records = self.records.lock().await;
        complete_record(
            &mut records,
            dispatch_id,
            request_digest,
            Some(identity),
            owner_id,
            Some(receipt),
            completed_at_ms,
        )
    }

    async fn release(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        let mut records = self.records.lock().await;
        release_record(&mut records, dispatch_id, request_digest, None, owner_id)
    }

    async fn release_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        let mut records = self.records.lock().await;
        release_record(
            &mut records,
            dispatch_id,
            request_digest,
            Some(identity),
            owner_id,
        )
    }

    async fn completed_receipt(
        &self,
        dispatch_id: &str,
    ) -> Result<Option<ExecutionResultReceiptV1>, EvaluationDispatchLedgerError> {
        let records = self.records.lock().await;
        Ok(records
            .get(dispatch_id)
            .and_then(|record| record.result_receipt.clone()))
    }

    async fn prune_completed(
        &self,
        before_ms: u64,
    ) -> Result<usize, EvaluationDispatchLedgerError> {
        let mut records = self.records.lock().await;
        Ok(prune_records(&mut records, before_ms))
    }
}

#[derive(Debug, Clone)]
pub struct FileEvaluationDispatchLedger {
    root: PathBuf,
    max_records: usize,
    process_lock: Arc<Mutex<()>>,
}

impl FileEvaluationDispatchLedger {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_records: EVALUATION_DISPATCH_LEDGER_DEFAULT_MAX_RECORDS,
            process_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn with_max_records(
        root: impl Into<PathBuf>,
        max_records: usize,
    ) -> Result<Self, EvaluationDispatchLedgerError> {
        if max_records == 0 {
            return Err(EvaluationDispatchLedgerError::InvalidField("max_records"));
        }
        Ok(Self {
            root: root.into(),
            max_records,
            process_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Open and validate the persisted claim generation before use.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, EvaluationDispatchLedgerError> {
        let ledger = Self::new(root);
        ledger.validate_store().await?;
        Ok(ledger)
    }

    pub async fn open_with_max_records(
        root: impl Into<PathBuf>,
        max_records: usize,
    ) -> Result<Self, EvaluationDispatchLedgerError> {
        let ledger = Self::with_max_records(root, max_records)?;
        ledger.validate_store().await?;
        Ok(ledger)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn max_records(&self) -> usize {
        self.max_records
    }

    pub async fn validate_store(&self) -> Result<usize, EvaluationDispatchLedgerError> {
        Ok(self.read_records().await?.len())
    }

    fn data_path(&self) -> PathBuf {
        self.root.join("evaluation-dispatch-ledger.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(".evaluation-dispatch-ledger.lock")
    }

    async fn acquire_file_lock(&self) -> Result<std::fs::File, EvaluationDispatchLedgerError> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|error| storage_error("create dispatch ledger directory", error))?;
        let path = self.lock_path();
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| storage_error("open dispatch ledger lock", error))?;
            file.lock_exclusive()
                .map_err(|error| storage_error("lock dispatch ledger", error))?;
            Ok(file)
        })
        .await
        .map_err(|error| {
            EvaluationDispatchLedgerError::Storage(format!("lock task failed: {error}"))
        })?
    }

    async fn read_records(
        &self,
    ) -> Result<BTreeMap<String, DispatchClaimRecord>, EvaluationDispatchLedgerError> {
        read_records_from_path(&self.data_path(), self.max_records).await
    }

    async fn mutate<T>(
        &self,
        mutation: impl FnOnce(
            &mut BTreeMap<String, DispatchClaimRecord>,
        ) -> Result<T, EvaluationDispatchLedgerError>,
    ) -> Result<T, EvaluationDispatchLedgerError> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        let mut records = self.read_records().await?;
        let value = mutation(&mut records)?;
        enforce_retention(&mut records, self.max_records)?;
        self.publish_records(&records).await?;
        Ok(value)
    }

    async fn publish_records(
        &self,
        records: &BTreeMap<String, DispatchClaimRecord>,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        let bytes = serde_json::to_vec(&DispatchLedgerFileV1 {
            schema: EVALUATION_DISPATCH_LEDGER_SCHEMA_V1.to_string(),
            records: records.clone(),
        })
        .map_err(|error| {
            EvaluationDispatchLedgerError::Storage(format!("encode ledger: {error}"))
        })?;
        if bytes.len() > EVALUATION_DISPATCH_LEDGER_MAX_BYTES {
            return Err(EvaluationDispatchLedgerError::SizeLimit);
        }
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|error| storage_error("create dispatch ledger directory", error))?;
        let path = self.data_path();
        let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
        let result = async {
            let mut file = tokio::fs::File::create(&temp)
                .await
                .map_err(|error| storage_error("create dispatch ledger generation", error))?;
            file.write_all(&bytes)
                .await
                .map_err(|error| storage_error("write dispatch ledger generation", error))?;
            file.sync_all()
                .await
                .map_err(|error| storage_error("sync dispatch ledger generation", error))?;
            drop(file);
            let temp_copy = temp.clone();
            let path_copy = path.clone();
            tokio::task::spawn_blocking(move || {
                tempfile::TempPath::try_from_path(temp_copy)
                    .map_err(|error| storage_error("prepare dispatch ledger replacement", error))?
                    .persist(path_copy)
                    .map_err(|error| {
                        storage_error("publish dispatch ledger generation", error.error)
                    })
            })
            .await
            .map_err(|error| {
                EvaluationDispatchLedgerError::Storage(format!("publish task failed: {error}"))
            })??;
            Ok::<(), EvaluationDispatchLedgerError>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        result
    }
}

#[async_trait]
impl EvaluationDispatchLedger for FileEvaluationDispatchLedger {
    async fn claim(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        self.mutate(|records| {
            claim_record(
                records,
                dispatch_id,
                request_digest,
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
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        self.mutate(|records| {
            Ok(renew_record(
                records,
                dispatch_id,
                request_digest,
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
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
        completed_at_ms: u64,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        self.mutate(|records| {
            complete_record(
                records,
                dispatch_id,
                request_digest,
                None,
                owner_id,
                None,
                completed_at_ms,
            )
        })
        .await
    }

    async fn release(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        owner_id: &str,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        self.mutate(|records| release_record(records, dispatch_id, request_digest, None, owner_id))
            .await
    }

    async fn claim_with_identity(
        &self,
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        self.mutate(|records| {
            claim_record(
                records,
                dispatch_id,
                request_digest,
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
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<bool, EvaluationDispatchLedgerError> {
        validate_claim_args(dispatch_id, request_digest, owner_id, now_ms, lease_ms)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        self.mutate(|records| {
            Ok(renew_record(
                records,
                dispatch_id,
                request_digest,
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
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
        receipt: &ExecutionResultReceiptV1,
        completed_at_ms: u64,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        receipt
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("result_receipt"))?;
        if &receipt.identity != identity {
            return Err(EvaluationDispatchLedgerError::Conflict);
        }
        self.mutate(|records| {
            complete_record(
                records,
                dispatch_id,
                request_digest,
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
        dispatch_id: &str,
        request_digest: &str,
        identity: &ExecutionIdentityV1,
        owner_id: &str,
    ) -> Result<(), EvaluationDispatchLedgerError> {
        validate_identity_args(dispatch_id, request_digest, owner_id)?;
        identity
            .validate()
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("execution_identity"))?;
        self.mutate(|records| {
            release_record(
                records,
                dispatch_id,
                request_digest,
                Some(identity),
                owner_id,
            )
        })
        .await
    }

    async fn completed_receipt(
        &self,
        dispatch_id: &str,
    ) -> Result<Option<ExecutionResultReceiptV1>, EvaluationDispatchLedgerError> {
        Ok(self
            .read_records()
            .await?
            .get(dispatch_id)
            .and_then(|record| record.result_receipt.clone()))
    }

    async fn prune_completed(
        &self,
        before_ms: u64,
    ) -> Result<usize, EvaluationDispatchLedgerError> {
        self.mutate(|records| Ok(prune_records(records, before_ms)))
            .await
    }
}

fn validate_claim_args(
    dispatch_id: &str,
    request_digest: &str,
    owner_id: &str,
    now_ms: u64,
    lease_ms: u64,
) -> Result<(), EvaluationDispatchLedgerError> {
    validate_identity_args(dispatch_id, request_digest, owner_id)?;
    if now_ms == 0 {
        return Err(EvaluationDispatchLedgerError::InvalidField("now_ms"));
    }
    if lease_ms == 0 {
        return Err(EvaluationDispatchLedgerError::InvalidField("lease_ms"));
    }
    Ok(())
}

fn validate_identity_args(
    dispatch_id: &str,
    request_digest: &str,
    owner_id: &str,
) -> Result<(), EvaluationDispatchLedgerError> {
    validate_text("dispatch_id", dispatch_id)?;
    validate_text("owner_id", owner_id)?;
    validate_digest(request_digest)
        .map_err(|_| EvaluationDispatchLedgerError::InvalidField("request_digest"))
}

fn validate_text(field: &'static str, value: &str) -> Result<(), EvaluationDispatchLedgerError> {
    if value.is_empty() || value.len() > 512 || value.contains('\0') || value.lines().count() != 1 {
        return Err(EvaluationDispatchLedgerError::InvalidField(field));
    }
    Ok(())
}

fn claim_record(
    records: &mut BTreeMap<String, DispatchClaimRecord>,
    dispatch_id: &str,
    request_digest: &str,
    identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
    now_ms: u64,
    lease_ms: u64,
) -> Result<EvaluationDispatchClaimOutcome, EvaluationDispatchLedgerError> {
    let lease_expires_at_ms = now_ms.saturating_add(lease_ms.max(1));
    match records.get_mut(dispatch_id) {
        None => {
            records.insert(
                dispatch_id.to_string(),
                DispatchClaimRecord {
                    request_digest: request_digest.to_string(),
                    execution_identity: identity.cloned(),
                    status: DispatchClaimStatus::Pending,
                    owner_id: owner_id.to_string(),
                    lease_expires_at_ms,
                    attempts: 1,
                    created_at_ms: now_ms,
                    completed_at_ms: None,
                    result_receipt: None,
                },
            );
            Ok(EvaluationDispatchClaimOutcome::Claimed { attempt: 1 })
        }
        Some(record) if record.request_digest != request_digest => {
            Ok(EvaluationDispatchClaimOutcome::Conflict)
        }
        Some(record)
            if identity.is_some_and(|identity| {
                record
                    .execution_identity
                    .as_ref()
                    .is_some_and(|record_identity| record_identity != identity)
            }) =>
        {
            Ok(EvaluationDispatchClaimOutcome::Conflict)
        }
        Some(record) if record.status == DispatchClaimStatus::Completed => {
            Ok(EvaluationDispatchClaimOutcome::Completed)
        }
        Some(record) if record.lease_expires_at_ms > now_ms && record.owner_id != owner_id => {
            Ok(EvaluationDispatchClaimOutcome::Busy {
                lease_expires_at_ms: record.lease_expires_at_ms,
            })
        }
        Some(record) => {
            if record.execution_identity.is_none() {
                record.execution_identity = identity.cloned();
            }
            record.owner_id = owner_id.to_string();
            record.lease_expires_at_ms = lease_expires_at_ms;
            record.attempts = record.attempts.saturating_add(1);
            Ok(EvaluationDispatchClaimOutcome::Claimed {
                attempt: record.attempts,
            })
        }
    }
}

fn renew_record(
    records: &mut BTreeMap<String, DispatchClaimRecord>,
    dispatch_id: &str,
    request_digest: &str,
    identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
    now_ms: u64,
    lease_ms: u64,
) -> bool {
    let Some(record) = records.get_mut(dispatch_id) else {
        return false;
    };
    if record.request_digest != request_digest
        || identity.is_some_and(|identity| {
            record
                .execution_identity
                .as_ref()
                .is_some_and(|record_identity| record_identity != identity)
        })
        || record.status != DispatchClaimStatus::Pending
        || record.owner_id != owner_id
        || record.lease_expires_at_ms <= now_ms
    {
        return false;
    }
    record.lease_expires_at_ms = now_ms.saturating_add(lease_ms.max(1));
    true
}

fn complete_record(
    records: &mut BTreeMap<String, DispatchClaimRecord>,
    dispatch_id: &str,
    request_digest: &str,
    identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
    result_receipt: Option<&ExecutionResultReceiptV1>,
    completed_at_ms: u64,
) -> Result<(), EvaluationDispatchLedgerError> {
    let record = records
        .get_mut(dispatch_id)
        .ok_or(EvaluationDispatchLedgerError::Conflict)?;
    if record.request_digest != request_digest {
        return Err(EvaluationDispatchLedgerError::Conflict);
    }
    if identity.is_some_and(|identity| {
        record
            .execution_identity
            .as_ref()
            .is_some_and(|record_identity| record_identity != identity)
    }) {
        return Err(EvaluationDispatchLedgerError::Conflict);
    }
    if record.status == DispatchClaimStatus::Completed {
        if let (Some(expected), Some(actual)) = (record.result_receipt.as_ref(), result_receipt) {
            if expected != actual {
                return Err(EvaluationDispatchLedgerError::Conflict);
            }
        }
        return Ok(());
    }
    if record.owner_id != owner_id {
        return Err(EvaluationDispatchLedgerError::Conflict);
    }
    if completed_at_ms < record.created_at_ms {
        return Err(EvaluationDispatchLedgerError::InvalidField(
            "completed_at_ms",
        ));
    }
    // A completion after lease expiry is stale. Without this fence, a
    // suspended worker could publish a terminal receipt after another worker
    // is entitled to take over the dispatch.
    if record.lease_expires_at_ms <= completed_at_ms {
        return Err(EvaluationDispatchLedgerError::Conflict);
    }
    if let Some(identity) = identity {
        record.execution_identity = Some(identity.clone());
    }
    record.result_receipt = result_receipt.cloned();
    record.status = DispatchClaimStatus::Completed;
    record.lease_expires_at_ms = 0;
    record.completed_at_ms = Some(completed_at_ms);
    Ok(())
}

fn release_record(
    records: &mut BTreeMap<String, DispatchClaimRecord>,
    dispatch_id: &str,
    request_digest: &str,
    identity: Option<&ExecutionIdentityV1>,
    owner_id: &str,
) -> Result<(), EvaluationDispatchLedgerError> {
    let Some(record) = records.get_mut(dispatch_id) else {
        return Ok(());
    };
    if record.request_digest != request_digest
        || identity.is_some_and(|identity| {
            record
                .execution_identity
                .as_ref()
                .is_some_and(|record_identity| record_identity != identity)
        })
        || record.status == DispatchClaimStatus::Completed
    {
        return Ok(());
    }
    if record.owner_id == owner_id {
        record.owner_id.clear();
        record.lease_expires_at_ms = 0;
    }
    Ok(())
}

fn prune_records(records: &mut BTreeMap<String, DispatchClaimRecord>, before_ms: u64) -> usize {
    let before = records.len();
    records.retain(|_, record| {
        record.status != DispatchClaimStatus::Completed
            || record.completed_at_ms.unwrap_or(u64::MAX) >= before_ms
    });
    before - records.len()
}

fn enforce_retention(
    records: &mut BTreeMap<String, DispatchClaimRecord>,
    max_records: usize,
) -> Result<(), EvaluationDispatchLedgerError> {
    while records.len() > max_records {
        let candidate = records
            .iter()
            .filter(|(_, record)| record.status == DispatchClaimStatus::Completed)
            .min_by_key(|(id, record)| (record.completed_at_ms.unwrap_or(u64::MAX), id.as_str()))
            .map(|(id, _)| id.clone());
        let Some(candidate) = candidate else {
            return Err(EvaluationDispatchLedgerError::SizeLimit);
        };
        records.remove(&candidate);
    }
    Ok(())
}

async fn read_records_from_path(
    path: &Path,
    max_records: usize,
) -> Result<BTreeMap<String, DispatchClaimRecord>, EvaluationDispatchLedgerError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(storage_error("read dispatch ledger", error)),
    };
    if bytes.len() > EVALUATION_DISPATCH_LEDGER_MAX_BYTES {
        return Err(EvaluationDispatchLedgerError::SizeLimit);
    }
    let file: DispatchLedgerFileV1 = serde_json::from_slice(&bytes).map_err(|error| {
        EvaluationDispatchLedgerError::Corrupt(format!("decode ledger: {error}"))
    })?;
    if file.schema != EVALUATION_DISPATCH_LEDGER_SCHEMA_V1 {
        return Err(EvaluationDispatchLedgerError::UnsupportedSchema);
    }
    if file.records.len() > max_records {
        return Err(EvaluationDispatchLedgerError::Corrupt(format!(
            "{} records exceed configured retention limit {}",
            file.records.len(),
            max_records
        )));
    }
    for (dispatch_id, record) in &file.records {
        validate_text("dispatch_id", dispatch_id)?;
        validate_digest(&record.request_digest)
            .map_err(|_| EvaluationDispatchLedgerError::InvalidField("request_digest"))?;
        if let Some(identity) = &record.execution_identity {
            identity.validate().map_err(|_| {
                EvaluationDispatchLedgerError::Corrupt("execution identity is invalid".to_string())
            })?;
        }
        if let Some(receipt) = &record.result_receipt {
            receipt.validate().map_err(|_| {
                EvaluationDispatchLedgerError::Corrupt("result receipt is invalid".to_string())
            })?;
            if record
                .execution_identity
                .as_ref()
                .is_some_and(|identity| identity != &receipt.identity)
            {
                return Err(EvaluationDispatchLedgerError::Corrupt(
                    "result receipt identity does not match claim".to_string(),
                ));
            }
        }
        if !record.owner_id.is_empty() {
            validate_text("owner_id", &record.owner_id)?;
        }
        if record.created_at_ms == 0
            || record.attempts == 0
            || record
                .completed_at_ms
                .is_some_and(|completed| completed < record.created_at_ms)
        {
            return Err(EvaluationDispatchLedgerError::Corrupt(
                "claim record timestamps or attempts are invalid".to_string(),
            ));
        }
        match record.status {
            DispatchClaimStatus::Pending
                if record.completed_at_ms.is_some()
                    || record.result_receipt.is_some()
                    || (record.owner_id.is_empty() && record.lease_expires_at_ms != 0)
                    || (!record.owner_id.is_empty() && record.lease_expires_at_ms == 0) =>
            {
                return Err(EvaluationDispatchLedgerError::Corrupt(
                    "pending claim fields are inconsistent".to_string(),
                ));
            }
            DispatchClaimStatus::Completed
                if record.completed_at_ms.is_none()
                    || record.lease_expires_at_ms != 0
                    || record.owner_id.is_empty() =>
            {
                return Err(EvaluationDispatchLedgerError::Corrupt(
                    "completed claim fields are inconsistent".to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(file.records)
}

fn storage_error(operation: &str, error: impl std::fmt::Display) -> EvaluationDispatchLedgerError {
    EvaluationDispatchLedgerError::Storage(format!("{operation}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_identity::{ExecutionResultOutcomeV1, EXECUTION_RESULT_RECEIPT_SCHEMA_V1};

    fn identity(domain: &str) -> ExecutionIdentityV1 {
        ExecutionIdentityV1::derive(domain, &serde_json::json!({"request": "bounded"})).unwrap()
    }

    fn receipt(identity: ExecutionIdentityV1) -> ExecutionResultReceiptV1 {
        ExecutionResultReceiptV1 {
            schema: EXECUTION_RESULT_RECEIPT_SCHEMA_V1.to_string(),
            identity,
            evidence_digest:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            outcome: ExecutionResultOutcomeV1::Succeeded,
            result_digest: Some(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
            ),
            result_bytes: 1,
        }
    }

    #[test]
    fn legacy_claim_record_deserializes_without_identity_or_receipt() {
        let value = serde_json::json!({
            "request_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "status": "pending",
            "owner_id": "owner-1",
            "lease_expires_at_ms": 101,
            "attempts": 1,
            "created_at_ms": 1
        });
        let record: DispatchClaimRecord = serde_json::from_value(value).unwrap();
        assert!(record.execution_identity.is_none());
        assert!(record.result_receipt.is_none());
    }

    #[tokio::test]
    async fn identity_fences_claim_renewal_and_terminal_receipt() {
        let ledger = MemoryEvaluationDispatchLedger::new();
        let canonical = identity("a3s.test.identity");
        let wrong = identity("a3s.test.other");
        let request_digest =
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

        assert!(matches!(
            ledger
                .claim_with_identity("dispatch-1", request_digest, &canonical, "owner-1", 1, 100)
                .await
                .unwrap(),
            EvaluationDispatchClaimOutcome::Claimed { attempt: 1 }
        ));
        assert!(!ledger
            .renew_with_identity("dispatch-1", request_digest, &wrong, "owner-1", 2, 100)
            .await
            .unwrap());
        assert!(ledger
            .renew_with_identity("dispatch-1", request_digest, &canonical, "owner-1", 2, 100)
            .await
            .unwrap());

        let result = receipt(canonical.clone());
        ledger
            .complete_with_receipt(
                "dispatch-1",
                request_digest,
                &canonical,
                "owner-1",
                &result,
                3,
            )
            .await
            .unwrap();
        assert_eq!(
            ledger.completed_receipt("dispatch-1").await.unwrap(),
            Some(result)
        );
        assert!(matches!(
            ledger
                .complete_with_receipt(
                    "dispatch-1",
                    request_digest,
                    &wrong,
                    "owner-1",
                    &receipt(wrong.clone()),
                    4,
                )
                .await,
            Err(EvaluationDispatchLedgerError::Conflict)
        ));
    }
}
