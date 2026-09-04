//! Tokio-backed durable evaluation-result storage.
//!
//! The host owns the directory, authorization, encryption, and retention
//! policy.  This adapter only provides a provider-neutral, bounded CAS store
//! with crash-safe publication and cross-process serialization.

use super::identity::validate_digest;
use super::result::{
    EvaluationRecordV1, EvaluationResultSink, EvaluationStoreError, EvaluationWriteOutcomeV1,
};
use super::ExecutionTargetV1;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

pub const EVALUATION_RESULT_STORE_SCHEMA_V1: &str = "a3s.code.evaluation-result-store.v1";
pub const EVALUATION_RESULT_STORE_DEFAULT_MAX_RECORDS: usize = 4096;
pub const EVALUATION_RESULT_STORE_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationResultStoreFileV1 {
    schema: String,
    records: Vec<EvaluationRecordV1>,
}

/// A durable implementation of [`EvaluationResultSink`].
///
/// Each mutation reads and validates one complete generation while holding a
/// process-local mutex and an `fs2` cross-process lock, then publishes a
/// synced temporary file with an atomic replacement.  Reads are safe during a
/// replacement because they observe either the old or the new generation.
#[derive(Debug, Clone)]
pub struct FileEvaluationResultStore {
    root: PathBuf,
    max_records: usize,
    process_lock: Arc<Mutex<()>>,
}

impl FileEvaluationResultStore {
    /// Construct a store using the default bounded retention policy.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_records: EVALUATION_RESULT_STORE_DEFAULT_MAX_RECORDS,
            process_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Construct a store with an explicit positive record-retention bound.
    pub fn with_max_records(
        root: impl Into<PathBuf>,
        max_records: usize,
    ) -> Result<Self, EvaluationStoreError> {
        if max_records == 0 {
            return Err(EvaluationStoreError::InvalidField("max_records"));
        }
        Ok(Self {
            root: root.into(),
            max_records,
            process_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Open and validate an existing generation before admitting it to a
    /// host.  A missing file is a valid empty store; malformed or newer data
    /// is returned as an error immediately.
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, EvaluationStoreError> {
        let store = Self::new(root);
        store.validate_store().await?;
        Ok(store)
    }

    pub async fn open_with_max_records(
        root: impl Into<PathBuf>,
        max_records: usize,
    ) -> Result<Self, EvaluationStoreError> {
        let store = Self::with_max_records(root, max_records)?;
        store.validate_store().await?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn max_records(&self) -> usize {
        self.max_records
    }

    fn data_path(&self) -> PathBuf {
        self.root.join("evaluation-results.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(".evaluation-results.lock")
    }

    /// Validate and count the currently persisted generation.
    ///
    /// Unlike the compatibility `EvaluationResultSink::get` and
    /// `list_for_target` methods, this method returns storage errors so a host
    /// can fail closed on corruption instead of treating it as an empty store.
    pub async fn validate_store(&self) -> Result<usize, EvaluationStoreError> {
        Ok(self.read_records().await?.len())
    }

    pub async fn get_checked(
        &self,
        record_digest: &str,
    ) -> Result<Option<EvaluationRecordV1>, EvaluationStoreError> {
        validate_digest(record_digest)
            .map_err(|_| EvaluationStoreError::InvalidField("record_digest"))?;
        let records = self.read_records().await?;
        Ok(records
            .into_iter()
            .find(|record| record.record_digest == record_digest))
    }

    pub async fn list_for_target_checked(
        &self,
        target: &ExecutionTargetV1,
    ) -> Result<Vec<EvaluationRecordV1>, EvaluationStoreError> {
        target
            .validate()
            .map_err(|_| EvaluationStoreError::InvalidField("target"))?;
        let records = self.read_records().await?;
        Ok(records
            .into_iter()
            .filter(|record| record.result.target == *target)
            .collect())
    }

    async fn acquire_file_lock(&self) -> Result<std::fs::File, EvaluationStoreError> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|error| storage_error("create evaluation result directory", error))?;
        let path = self.lock_path();
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| storage_error("open evaluation result lock", error))?;
            file.lock_exclusive()
                .map_err(|error| storage_error("lock evaluation result store", error))?;
            Ok(file)
        })
        .await
        .map_err(|error| EvaluationStoreError::Storage(format!("lock task failed: {error}")))?
    }

    async fn read_records(&self) -> Result<Vec<EvaluationRecordV1>, EvaluationStoreError> {
        read_records_from_path(&self.data_path(), self.max_records).await
    }

    async fn mutate(
        &self,
        record: EvaluationRecordV1,
    ) -> Result<EvaluationWriteOutcomeV1, EvaluationStoreError> {
        record.validate()?;
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        let mut records = self.read_records().await?;
        if let Some(existing) = records
            .iter()
            .find(|existing| existing.record_digest == record.record_digest)
        {
            if existing == &record {
                return Ok(EvaluationWriteOutcomeV1 {
                    written: false,
                    replayed: true,
                });
            }
            return Err(EvaluationStoreError::Conflict);
        }
        if records.iter().any(|existing| {
            existing.result.target == record.result.target
                && existing.result.evaluator_id == record.result.evaluator_id
                && existing.result.auxiliary_run_id == record.result.auxiliary_run_id
        }) {
            return Err(EvaluationStoreError::Conflict);
        }
        records.push(record);
        while records.len() > self.max_records {
            records.remove(0);
        }
        self.publish_records(&records).await?;
        Ok(EvaluationWriteOutcomeV1 {
            written: true,
            replayed: false,
        })
    }

    async fn publish_records(
        &self,
        records: &[EvaluationRecordV1],
    ) -> Result<(), EvaluationStoreError> {
        let bytes = serde_json::to_vec(&EvaluationResultStoreFileV1 {
            schema: EVALUATION_RESULT_STORE_SCHEMA_V1.to_string(),
            records: records.to_vec(),
        })
        .map_err(|error| EvaluationStoreError::Serialization(error.to_string()))?;
        if bytes.len() > EVALUATION_RESULT_STORE_MAX_BYTES {
            return Err(EvaluationStoreError::SizeLimit);
        }
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|error| storage_error("create evaluation result directory", error))?;
        let path = self.data_path();
        let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
        let result = async {
            let mut file = tokio::fs::File::create(&temp)
                .await
                .map_err(|error| storage_error("create evaluation result generation", error))?;
            file.write_all(&bytes)
                .await
                .map_err(|error| storage_error("write evaluation result generation", error))?;
            file.sync_all()
                .await
                .map_err(|error| storage_error("sync evaluation result generation", error))?;
            drop(file);
            let temp_copy = temp.clone();
            let path_copy = path.clone();
            tokio::task::spawn_blocking(move || {
                tempfile::TempPath::try_from_path(temp_copy)
                    .map_err(|error| storage_error("prepare evaluation result replacement", error))?
                    .persist(path_copy)
                    .map_err(|error| {
                        storage_error("publish evaluation result generation", error.error)
                    })
            })
            .await
            .map_err(|error| {
                EvaluationStoreError::Storage(format!("publish task failed: {error}"))
            })??;
            Ok::<(), EvaluationStoreError>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        result
    }
}

#[async_trait]
impl EvaluationResultSink for FileEvaluationResultStore {
    async fn write(
        &self,
        record: EvaluationRecordV1,
    ) -> Result<EvaluationWriteOutcomeV1, EvaluationStoreError> {
        self.mutate(record).await
    }

    async fn get(&self, record_digest: &str) -> Option<EvaluationRecordV1> {
        self.get_checked(record_digest).await.ok().flatten()
    }

    async fn list_for_target(&self, target: &ExecutionTargetV1) -> Vec<EvaluationRecordV1> {
        self.list_for_target_checked(target)
            .await
            .unwrap_or_default()
    }

    async fn get_checked(
        &self,
        record_digest: &str,
    ) -> Result<Option<EvaluationRecordV1>, EvaluationStoreError> {
        FileEvaluationResultStore::get_checked(self, record_digest).await
    }

    async fn list_for_target_checked(
        &self,
        target: &ExecutionTargetV1,
    ) -> Result<Vec<EvaluationRecordV1>, EvaluationStoreError> {
        FileEvaluationResultStore::list_for_target_checked(self, target).await
    }
}

async fn read_records_from_path(
    path: &Path,
    max_records: usize,
) -> Result<Vec<EvaluationRecordV1>, EvaluationStoreError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(storage_error("read evaluation result store", error)),
    };
    if bytes.len() > EVALUATION_RESULT_STORE_MAX_BYTES {
        return Err(EvaluationStoreError::SizeLimit);
    }
    let file: EvaluationResultStoreFileV1 = serde_json::from_slice(&bytes)
        .map_err(|error| EvaluationStoreError::Corrupt(format!("decode generation: {error}")))?;
    if file.schema != EVALUATION_RESULT_STORE_SCHEMA_V1 {
        return Err(EvaluationStoreError::UnsupportedSchema);
    }
    if file.records.is_empty() {
        return Ok(file.records);
    }
    if file.records.len() > max_records {
        return Err(EvaluationStoreError::Corrupt(format!(
            "{} records exceed configured retention limit {}",
            file.records.len(),
            max_records
        )));
    }
    let mut digests = HashSet::with_capacity(file.records.len());
    let mut identities = HashSet::with_capacity(file.records.len());
    for record in &file.records {
        record.validate()?;
        if !digests.insert(record.record_digest.clone()) {
            return Err(EvaluationStoreError::Corrupt(
                "duplicate record digest".to_string(),
            ));
        }
        let identity = (
            record.result.target.clone(),
            record.result.evaluator_id.clone(),
            record.result.auxiliary_run_id.clone(),
        );
        if !identities.insert(identity) {
            return Err(EvaluationStoreError::Corrupt(
                "duplicate evaluator identity".to_string(),
            ));
        }
    }
    Ok(file.records)
}

fn storage_error(operation: &str, error: impl std::fmt::Display) -> EvaluationStoreError {
    EvaluationStoreError::Storage(format!("{operation}: {error}"))
}
