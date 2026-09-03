use super::vec_shadow_document::{
    normalize_unit, partition_filter, prepare_documents, EMBEDDING_FIELD, PARTITION_FIELD,
    PARTITION_KEY_FIELD, RECORD_ID_FIELD,
};
use super::vector_contract::{VectorIndexDescriptor, VectorRecord, VectorSearchRequest};
use a3s_vec::{
    Collection, CollectionOptions, CollectionResourceLimits, CollectionSchema, DataType, Doc,
    Durability, ErrorCode, FieldSchema, SearchQuery, WriteResult,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Clone, Copy, Debug)]
pub(super) enum VecShadowFailure {
    Closed,
    FileSystem,
    FilterBudget,
    InvalidContract,
    RevisionConflict { expected: u64, actual: u64 },
    Rollback,
    Unavailable,
    UnsupportedLabels,
    Vec(ErrorCode),
    Worker,
    WriteRejected,
}

impl VecShadowFailure {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::FileSystem => "file_system",
            Self::FilterBudget => "filter_budget",
            Self::InvalidContract => "invalid_contract",
            Self::RevisionConflict { .. } => "revision_conflict",
            Self::Rollback => "rollback",
            Self::Unavailable => "unavailable",
            Self::UnsupportedLabels => "unsupported_labels",
            Self::Vec(ErrorCode::NotFound) => "vec_not_found",
            Self::Vec(ErrorCode::AlreadyExists) => "vec_already_exists",
            Self::Vec(ErrorCode::InvalidArgument) => "vec_invalid_argument",
            Self::Vec(ErrorCode::PermissionDenied) => "vec_permission_denied",
            Self::Vec(ErrorCode::FailedPrecondition) => "vec_failed_precondition",
            Self::Vec(ErrorCode::ResourceExhausted) => "vec_resource_exhausted",
            Self::Vec(ErrorCode::Unavailable) => "vec_unavailable",
            Self::Vec(ErrorCode::InternalError) => "vec_internal",
            Self::Vec(ErrorCode::NotSupported) => "vec_not_supported",
            Self::Vec(ErrorCode::Unknown) => "vec_unknown",
            Self::Worker => "worker",
            Self::WriteRejected => "write_rejected",
        }
    }
}

impl From<a3s_vec::Error> for VecShadowFailure {
    fn from(error: a3s_vec::Error) -> Self {
        Self::Vec(error.code)
    }
}

pub(super) type VecShadowResult<T> = Result<T, VecShadowFailure>;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VecShadowSnapshot {
    pub(super) revision: u64,
    pub(super) partition_count: usize,
    pub(super) record_count: usize,
    pub(super) accounted_bytes: usize,
}

#[derive(Debug)]
pub(super) struct VecShadowSearchHit {
    pub(super) id: String,
    pub(super) partition: String,
    pub(super) score: f32,
}

#[derive(Debug)]
pub(super) struct VecShadowSearchResult {
    pub(super) hits: Vec<VecShadowSearchHit>,
    pub(super) snapshot: VecShadowSnapshot,
    pub(super) searched_records: usize,
    pub(super) truncated: bool,
}

#[derive(Debug)]
struct VecShadowState {
    collection: Option<Collection>,
    temp_dir: Option<TempDir>,
    partitions: BTreeMap<String, Vec<String>>,
    /// Logical revision of the adapter contract. A partition replacement may
    /// require multiple physical Vec mutations (delete then insert), but it
    /// publishes exactly one revision to callers.
    revision: u64,
}

#[derive(Debug)]
struct VecShadowInner {
    dimension: usize,
    state: Mutex<VecShadowState>,
    operation_gate: Arc<tokio::sync::RwLock<()>>,
}

#[derive(Clone, Debug)]
pub(super) struct VecShadowStore {
    inner: Arc<VecShadowInner>,
}

impl VecShadowStore {
    pub(super) fn create(
        descriptor: &VectorIndexDescriptor,
    ) -> VecShadowResult<(Self, VecShadowSnapshot)> {
        let dimension =
            u32::try_from(descriptor.dimension).map_err(|_| VecShadowFailure::InvalidContract)?;
        let max_records =
            u64::try_from(descriptor.max_records).map_err(|_| VecShadowFailure::InvalidContract)?;
        let max_bytes =
            u64::try_from(descriptor.max_bytes).map_err(|_| VecShadowFailure::InvalidContract)?;
        let schema = CollectionSchema::builder("workspace-vector-shadow")
            .add_field(FieldSchema::new(
                RECORD_ID_FIELD,
                DataType::String,
                false,
                0,
            )?)
            .add_field(FieldSchema::new(
                PARTITION_FIELD,
                DataType::String,
                false,
                0,
            )?)
            .add_field(FieldSchema::new(
                PARTITION_KEY_FIELD,
                DataType::String,
                false,
                0,
            )?)
            .add_field(FieldSchema::new(
                EMBEDDING_FIELD,
                DataType::VectorFp32,
                false,
                dimension,
            )?)
            .build()?;
        let limits = CollectionResourceLimits::new()
            .try_with_max_documents(max_records)?
            .try_with_max_accounted_bytes(max_bytes)?
            .try_with_max_query_candidates(max_records)?
            .try_with_max_write_batch_documents(max_records)?;
        let mut options = CollectionOptions::new()?;
        options.set_durability(Durability::Manual)?;
        options.set_resource_limits(limits)?;
        let temp_dir = tempfile::tempdir().map_err(|_| VecShadowFailure::FileSystem)?;
        let collection_path = temp_dir.path().join("collection");
        let collection_path = collection_path
            .to_str()
            .ok_or(VecShadowFailure::FileSystem)?;
        let collection = Collection::create(collection_path, &schema, Some(&options))?;
        let snapshot = snapshot(&collection, 0, 0)?;
        Ok((
            Self {
                inner: Arc::new(VecShadowInner {
                    dimension: descriptor.dimension,
                    state: Mutex::new(VecShadowState {
                        collection: Some(collection),
                        temp_dir: Some(temp_dir),
                        partitions: BTreeMap::new(),
                        revision: 0,
                    }),
                    operation_gate: Arc::new(tokio::sync::RwLock::new(())),
                }),
            },
            snapshot,
        ))
    }

    pub(super) async fn replace_partition(
        &self,
        partition: String,
        records: Vec<VectorRecord>,
    ) -> VecShadowResult<VecShadowSnapshot> {
        let inner = Arc::clone(&self.inner);
        let operation = Arc::clone(&inner.operation_gate).write_owned().await;
        run_blocking(move || {
            let _operation = operation;
            replace_partition(&inner, partition, records)
        })
        .await
    }

    pub(super) async fn replace_partition_if_revision(
        &self,
        partition: String,
        expected_revision: u64,
        records: Vec<VectorRecord>,
    ) -> VecShadowResult<VecShadowSnapshot> {
        let inner = Arc::clone(&self.inner);
        let operation = Arc::clone(&inner.operation_gate).write_owned().await;
        run_blocking(move || {
            let _operation = operation;
            replace_partition_if_revision(&inner, partition, expected_revision, records)
        })
        .await
    }

    pub(super) async fn remove_partition(
        &self,
        partition: String,
    ) -> VecShadowResult<VecShadowSnapshot> {
        self.replace_partition(partition, Vec::new()).await
    }

    pub(super) async fn remove_partition_if_revision(
        &self,
        partition: String,
        expected_revision: u64,
    ) -> VecShadowResult<VecShadowSnapshot> {
        self.replace_partition_if_revision(partition, expected_revision, Vec::new())
            .await
    }

    pub(super) async fn clear(&self) -> VecShadowResult<VecShadowSnapshot> {
        let inner = Arc::clone(&self.inner);
        let operation = Arc::clone(&inner.operation_gate).write_owned().await;
        run_blocking(move || {
            let _operation = operation;
            clear(&inner)
        })
        .await
    }

    pub(super) async fn search(
        &self,
        request: VectorSearchRequest,
    ) -> VecShadowResult<VecShadowSearchResult> {
        let inner = Arc::clone(&self.inner);
        let operation = Arc::clone(&inner.operation_gate).read_owned().await;
        run_blocking(move || {
            let _operation = operation;
            search(&inner, request)
        })
        .await
    }

    pub(super) async fn close(&self) -> VecShadowResult<()> {
        let inner = Arc::clone(&self.inner);
        let operation = Arc::clone(&inner.operation_gate).write_owned().await;
        run_blocking(move || {
            let _operation = operation;
            close(&inner)
        })
        .await
    }
}

fn replace_partition(
    inner: &VecShadowInner,
    partition: String,
    records: Vec<VectorRecord>,
) -> VecShadowResult<VecShadowSnapshot> {
    replace_partition_checked(inner, partition, None, records)
}

fn replace_partition_if_revision(
    inner: &VecShadowInner,
    partition: String,
    expected_revision: u64,
    records: Vec<VectorRecord>,
) -> VecShadowResult<VecShadowSnapshot> {
    replace_partition_checked(inner, partition, Some(expected_revision), records)
}

fn replace_partition_checked(
    inner: &VecShadowInner,
    partition: String,
    expected_revision: Option<u64>,
    records: Vec<VectorRecord>,
) -> VecShadowResult<VecShadowSnapshot> {
    let (new_keys, new_docs) = prepare_documents(&partition, records, inner.dimension)?;
    let mut state = lock_unpoisoned(&inner.state);
    let collection = state
        .collection
        .as_ref()
        .cloned()
        .ok_or(VecShadowFailure::Closed)?;
    let actual_revision = state.revision;
    if let Some(expected_revision) = expected_revision {
        if actual_revision != expected_revision {
            return Err(VecShadowFailure::RevisionConflict {
                expected: expected_revision,
                actual: actual_revision,
            });
        }
    }
    let old_keys = state
        .partitions
        .get(&partition)
        .cloned()
        .unwrap_or_default();
    let changed = !(new_keys.is_empty() && old_keys.is_empty());
    let next_revision = if changed {
        state
            .revision
            .checked_add(1)
            .ok_or(VecShadowFailure::InvalidContract)?
    } else {
        state.revision
    };
    let old_docs = fetch_documents(&collection, &old_keys)?;

    delete_keys(&collection, &old_keys)?;
    if let Err(failure) = insert_documents(&collection, &new_docs) {
        if rollback_replacement(&collection, &new_keys, &old_docs).is_err() {
            return Err(VecShadowFailure::Rollback);
        }
        return Err(failure);
    }

    if new_keys.is_empty() {
        state.partitions.remove(&partition);
    } else {
        state.partitions.insert(partition, new_keys);
    }
    state.revision = next_revision;
    snapshot(&collection, state.partitions.len(), next_revision)
}

fn clear(inner: &VecShadowInner) -> VecShadowResult<VecShadowSnapshot> {
    let mut state = lock_unpoisoned(&inner.state);
    let collection = state
        .collection
        .as_ref()
        .cloned()
        .ok_or(VecShadowFailure::Closed)?;
    let keys = state
        .partitions
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let next_revision = if keys.is_empty() {
        state.revision
    } else {
        state
            .revision
            .checked_add(1)
            .ok_or(VecShadowFailure::InvalidContract)?
    };
    delete_keys(&collection, &keys)?;
    state.partitions.clear();
    state.revision = next_revision;
    snapshot(&collection, 0, next_revision)
}

fn search(
    inner: &VecShadowInner,
    mut request: VectorSearchRequest,
) -> VecShadowResult<VecShadowSearchResult> {
    if !request.labels.is_empty() {
        return Err(VecShadowFailure::UnsupportedLabels);
    }
    let (collection, filter, searched_records) = {
        let state = lock_unpoisoned(&inner.state);
        let collection = state
            .collection
            .as_ref()
            .cloned()
            .ok_or(VecShadowFailure::Closed)?;
        let selected = state
            .partitions
            .iter()
            .filter(|(partition, _)| {
                request.partitions.is_empty() || request.partitions.contains(*partition)
            })
            .collect::<Vec<_>>();
        let searched_records = selected
            .iter()
            .try_fold(0usize, |total, (_, keys)| total.checked_add(keys.len()))
            .ok_or(VecShadowFailure::InvalidContract)?;
        let all_selected = selected.len() == state.partitions.len();
        let filter = (!all_selected)
            .then(|| partition_filter(selected.iter().map(|(partition, _)| partition.as_str())))
            .transpose()?;
        (collection, filter, searched_records)
    };

    normalize_unit(&mut request.embedding, inner.dimension)?;
    let topk = i32::try_from(request.limit).map_err(|_| VecShadowFailure::InvalidContract)?;
    let mut query = SearchQuery::new(EMBEDDING_FIELD, &request.embedding, topk)?;
    query
        .params
        .insert("metric".to_string(), Value::String("ip".to_string()));
    query.set_output_fields(&[RECORD_ID_FIELD, PARTITION_FIELD])?;
    if let Some(filter) = filter {
        query.set_filter(&filter)?;
    }
    let docs = collection.query(&query)?;
    let hits = docs
        .into_iter()
        .map(|doc| {
            Ok(VecShadowSearchHit {
                id: doc
                    .get_string(RECORD_ID_FIELD)?
                    .ok_or(VecShadowFailure::InvalidContract)?,
                partition: doc
                    .get_string(PARTITION_FIELD)?
                    .ok_or(VecShadowFailure::InvalidContract)?,
                score: doc.get_score(),
            })
        })
        .collect::<VecShadowResult<Vec<_>>>()?;
    let (partition_count, revision) = state_partition_metadata(&inner.state);
    let snapshot = snapshot(&collection, partition_count, revision)?;
    Ok(VecShadowSearchResult {
        truncated: searched_records > hits.len(),
        hits,
        snapshot,
        searched_records,
    })
}

fn state_partition_metadata(state: &Mutex<VecShadowState>) -> (usize, u64) {
    let state = lock_unpoisoned(state);
    (state.partitions.len(), state.revision)
}

fn close(inner: &VecShadowInner) -> VecShadowResult<()> {
    let (collection, temp_dir) = {
        let mut state = lock_unpoisoned(&inner.state);
        state.partitions.clear();
        (state.collection.take(), state.temp_dir.take())
    };
    let collection_result = collection.map_or(Ok(()), Collection::close);
    let temp_result = temp_dir.map_or(Ok(()), TempDir::close);
    collection_result.map_err(VecShadowFailure::from)?;
    temp_result.map_err(|_| VecShadowFailure::FileSystem)
}

fn fetch_documents(collection: &Collection, keys: &[String]) -> VecShadowResult<Vec<Doc>> {
    let refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
    let docs = collection.fetch(&refs)?;
    if docs.len() != keys.len() {
        return Err(VecShadowFailure::InvalidContract);
    }
    Ok(docs)
}

fn insert_documents(collection: &Collection, docs: &[Doc]) -> VecShadowResult<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let refs = docs.iter().collect::<Vec<_>>();
    checked_write(collection.insert(&refs)?)
}

fn upsert_documents(collection: &Collection, docs: &[Doc]) -> VecShadowResult<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let refs = docs.iter().collect::<Vec<_>>();
    checked_write(collection.upsert(&refs)?)
}

fn delete_keys(collection: &Collection, keys: &[String]) -> VecShadowResult<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
    checked_write(collection.delete(&refs)?)
}

fn rollback_replacement(
    collection: &Collection,
    new_keys: &[String],
    old_docs: &[Doc],
) -> VecShadowResult<()> {
    let existing_new = fetch_existing_keys(collection, new_keys)?;
    delete_keys(collection, &existing_new)?;
    upsert_documents(collection, old_docs)
}

fn fetch_existing_keys(collection: &Collection, keys: &[String]) -> VecShadowResult<Vec<String>> {
    let refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(collection
        .fetch(&refs)?
        .into_iter()
        .filter_map(|doc| doc.get_pk().map(str::to_string))
        .collect())
}

fn checked_write(result: WriteResult) -> VecShadowResult<()> {
    if result.error_count == 0 {
        Ok(())
    } else {
        Err(VecShadowFailure::WriteRejected)
    }
}

fn snapshot(
    collection: &Collection,
    partition_count: usize,
    revision: u64,
) -> VecShadowResult<VecShadowSnapshot> {
    let stats = collection.stats()?;
    Ok(VecShadowSnapshot {
        revision,
        partition_count,
        record_count: usize::try_from(stats.doc_count).unwrap_or(usize::MAX),
        accounted_bytes: usize::try_from(stats.accounted_bytes).unwrap_or(usize::MAX),
    })
}

async fn run_blocking<T, F>(operation: F) -> VecShadowResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> VecShadowResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| VecShadowFailure::Worker)?
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
