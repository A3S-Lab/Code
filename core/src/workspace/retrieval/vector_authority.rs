use super::memory_vector_adapter::MemoryVectorIndexAdapter;
use super::vec_shadow_store::{VecShadowFailure, VecShadowSnapshot, VecShadowStore};
use super::vector_contract::{
    VectorBudgetResource, VectorIndexChangeToken, VectorIndexDescriptor, VectorIndexError,
    VectorIndexObservation, VectorIndexStatus, VectorMetric, VectorMutationConsistency,
    VectorNormalization, VectorRecord, VectorResult, VectorRevision, VectorSearchHit,
    VectorSearchRequest, VectorSearchResult, WorkspaceVectorIndex,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// Primary backend used by the workspace migration adapter.
pub(super) enum PrimaryVectorIndex {
    Memory(MemoryVectorIndexAdapter),
    Vec(VecPrimaryVectorIndex),
}

/// A3S Vec-backed implementation of the small vector-index contract used by
/// workspace retrieval. The store itself remains isolated behind the adapter;
/// callers never receive a dependency-owned collection handle.
pub(super) struct VecPrimaryVectorIndex {
    descriptor: VectorIndexDescriptor,
    store: VecShadowStore,
    snapshot: Mutex<VecShadowSnapshot>,
}

impl VecPrimaryVectorIndex {
    pub(super) fn new(descriptor: VectorIndexDescriptor) -> VectorResult<Self> {
        validate_descriptor(&descriptor)?;
        let (store, snapshot) =
            VecShadowStore::create(&descriptor).map_err(vector_error_from_shadow_failure)?;
        Ok(Self {
            descriptor,
            store,
            snapshot: Mutex::new(snapshot),
        })
    }

    fn status(&self) -> VectorIndexStatus {
        status_from_snapshot(*lock_unpoisoned(&self.snapshot))
    }

    fn update_snapshot(&self, snapshot: VecShadowSnapshot) {
        *lock_unpoisoned(&self.snapshot) = snapshot;
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        let partition = validate_partition(partition)?.to_owned();
        validate_records(&self.descriptor, &partition, &records)?;
        let snapshot = self
            .store
            .replace_partition(partition, records)
            .await
            .map_err(vector_error_from_shadow_failure)?;
        self.update_snapshot(snapshot);
        Ok(status_from_snapshot(snapshot))
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        let partition = validate_partition(partition)?.to_owned();
        let snapshot = self
            .store
            .remove_partition(partition)
            .await
            .map_err(vector_error_from_shadow_failure)?;
        self.update_snapshot(snapshot);
        Ok(status_from_snapshot(snapshot))
    }

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        validate_search_request(&self.descriptor, &request)?;
        let result = self
            .store
            .search(request)
            .await
            .map_err(vector_error_from_shadow_failure)?;
        self.update_snapshot(result.snapshot);
        Ok(VectorSearchResult {
            hits: result
                .hits
                .into_iter()
                .map(|hit| VectorSearchHit {
                    id: hit.id,
                    partition: hit.partition,
                    score: hit.score,
                    labels: BTreeMap::new(),
                })
                .collect(),
            status: status_from_snapshot(result.snapshot),
            searched_records: result.searched_records,
            truncated: result.truncated,
        })
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        let snapshot = self
            .store
            .clear()
            .await
            .map_err(vector_error_from_shadow_failure)?;
        self.update_snapshot(snapshot);
        Ok(status_from_snapshot(snapshot))
    }

    pub(super) async fn close(&self) -> VectorResult<()> {
        self.store
            .close()
            .await
            .map_err(vector_error_from_shadow_failure)?;
        self.update_snapshot(VecShadowSnapshot::default());
        Ok(())
    }
}

fn validate_descriptor(descriptor: &VectorIndexDescriptor) -> VectorResult<()> {
    if descriptor.dimension == 0 {
        return Err(VectorIndexError::InvalidDescriptor(
            "dimension must be greater than zero".to_string(),
        ));
    }
    if descriptor.max_records == 0 {
        return Err(VectorIndexError::InvalidDescriptor(
            "max_records must be greater than zero".to_string(),
        ));
    }
    if descriptor.max_bytes == 0 {
        return Err(VectorIndexError::InvalidDescriptor(
            "max_bytes must be greater than zero".to_string(),
        ));
    }
    if descriptor.metric != VectorMetric::Cosine
        || descriptor.normalization != VectorNormalization::Unit
    {
        return Err(VectorIndexError::InvalidDescriptor(
            "the A3S Vec workspace adapter currently requires cosine/unit vectors".to_string(),
        ));
    }
    let vector_bytes = descriptor
        .dimension
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(VectorIndexError::SizeOverflow)?;
    if vector_bytes > descriptor.max_bytes {
        return Err(VectorIndexError::BudgetExceeded {
            resource: VectorBudgetResource::Bytes,
            limit: descriptor.max_bytes,
            required: vector_bytes,
        });
    }
    Ok(())
}

fn validate_partition(partition: &str) -> VectorResult<&str> {
    if partition.trim().is_empty() {
        Err(VectorIndexError::InvalidPartition)
    } else {
        // Preserve the caller's exact partition identity.  Trimming here
        // would make the Vec authority return a different partition than the
        // Memory contract for an otherwise valid (non-empty) identifier.
        Ok(partition)
    }
}

fn validate_records(
    descriptor: &VectorIndexDescriptor,
    partition: &str,
    records: &[VectorRecord],
) -> VectorResult<()> {
    if records.len() > descriptor.max_records {
        return Err(VectorIndexError::BudgetExceeded {
            resource: VectorBudgetResource::Records,
            limit: descriptor.max_records,
            required: records.len(),
        });
    }
    let mut ids = BTreeSet::new();
    let mut accounted = partition.len();
    for (record_index, record) in records.iter().enumerate() {
        if record.id.trim().is_empty() {
            return Err(VectorIndexError::InvalidRecordId {
                partition: partition.to_owned(),
                record_index,
            });
        }
        if !ids.insert(record.id.clone()) {
            return Err(VectorIndexError::DuplicateRecordId {
                partition: partition.to_owned(),
                id: record.id.clone(),
            });
        }
        if !record.labels.is_empty() {
            return Err(VectorIndexError::InvalidLabel {
                context: "A3S Vec workspace authority does not support labels".to_string(),
            });
        }
        if record.embedding.len() != descriptor.dimension {
            return Err(VectorIndexError::DimensionMismatch {
                context: format!("record '{}' in partition '{partition}'", record.id),
                expected: descriptor.dimension,
                actual: record.embedding.len(),
            });
        }
        if let Some(element_index) = record.embedding.iter().position(|value| !value.is_finite()) {
            return Err(VectorIndexError::NonFiniteVector {
                context: format!("record '{}' in partition '{partition}'", record.id),
                element_index,
            });
        }
        let squared_norm = record.embedding.iter().fold(0.0f64, |sum, value| {
            let value = f64::from(*value);
            sum + value * value
        });
        if squared_norm == 0.0 {
            return Err(VectorIndexError::ZeroVector {
                context: format!("record '{}' in partition '{partition}'", record.id),
            });
        }
        let vector_bytes = descriptor
            .dimension
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or(VectorIndexError::SizeOverflow)?;
        accounted = accounted
            .checked_add(record.id.len())
            .and_then(|value| value.checked_add(vector_bytes))
            .ok_or(VectorIndexError::SizeOverflow)?;
        if accounted > descriptor.max_bytes {
            return Err(VectorIndexError::BudgetExceeded {
                resource: VectorBudgetResource::Bytes,
                limit: descriptor.max_bytes,
                required: accounted,
            });
        }
    }
    Ok(())
}

fn validate_search_request(
    descriptor: &VectorIndexDescriptor,
    request: &VectorSearchRequest,
) -> VectorResult<()> {
    if request.limit == 0 {
        return Err(VectorIndexError::InvalidRequest(
            "limit must be greater than zero".to_string(),
        ));
    }
    if request.embedding.len() != descriptor.dimension {
        return Err(VectorIndexError::DimensionMismatch {
            context: "query".to_string(),
            expected: descriptor.dimension,
            actual: request.embedding.len(),
        });
    }
    if let Some(element_index) = request
        .embedding
        .iter()
        .position(|value| !value.is_finite())
    {
        return Err(VectorIndexError::NonFiniteVector {
            context: "query".to_string(),
            element_index,
        });
    }
    let squared_norm = request.embedding.iter().fold(0.0f64, |sum, value| {
        let value = f64::from(*value);
        sum + value * value
    });
    if squared_norm == 0.0 {
        return Err(VectorIndexError::ZeroVector {
            context: "query".to_string(),
        });
    }
    if request
        .partitions
        .iter()
        .any(|partition| partition.trim().is_empty())
    {
        return Err(VectorIndexError::InvalidPartition);
    }
    if !request.labels.is_empty() {
        return Err(VectorIndexError::InvalidLabel {
            context: "A3S Vec workspace authority does not support labels".to_string(),
        });
    }
    Ok(())
}

#[async_trait::async_trait]
impl WorkspaceVectorIndex for PrimaryVectorIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        match self {
            Self::Memory(index) => index.descriptor(),
            Self::Vec(index) => &index.descriptor,
        }
    }

    fn status(&self) -> VectorIndexStatus {
        match self {
            Self::Memory(index) => index.status(),
            Self::Vec(index) => index.status(),
        }
    }

    fn change_token(&self) -> Option<VectorIndexChangeToken> {
        match self {
            Self::Memory(index) => index.change_token(),
            Self::Vec(_) => None,
        }
    }

    async fn observe(&self) -> VectorResult<VectorIndexObservation> {
        match self {
            Self::Memory(index) => index.observe().await,
            Self::Vec(index) => {
                let observation = VectorIndexObservation {
                    status: index.status(),
                    change_token: None,
                };
                observation.verify()?;
                Ok(observation)
            }
        }
    }

    fn mutation_consistency(&self) -> VectorMutationConsistency {
        match self {
            Self::Memory(index) => index.mutation_consistency(),
            Self::Vec(_) => VectorMutationConsistency::PartitionAtomic,
        }
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        match self {
            Self::Memory(index) => index.replace_partition(partition, records).await,
            Self::Vec(index) => index.replace_partition(partition, records).await,
        }
    }

    async fn replace_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        match self {
            Self::Memory(index) => {
                index
                    .replace_partition_if_revision(partition, expected_revision, records)
                    .await
            }
            Self::Vec(_) => Err(VectorIndexError::ConditionalMutationUnsupported),
        }
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        match self {
            Self::Memory(index) => index.remove_partition(partition).await,
            Self::Vec(index) => index.remove_partition(partition).await,
        }
    }

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
    ) -> VectorResult<VectorIndexStatus> {
        match self {
            Self::Memory(index) => {
                index
                    .remove_partition_if_revision(partition, expected_revision)
                    .await
            }
            Self::Vec(_) => Err(VectorIndexError::ConditionalMutationUnsupported),
        }
    }

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        match self {
            Self::Memory(index) => index.search(request).await,
            Self::Vec(index) => index.search(request).await,
        }
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        match self {
            Self::Memory(index) => index.clear().await,
            Self::Vec(index) => index.clear().await,
        }
    }
}

impl PrimaryVectorIndex {
    pub(super) async fn close(&self) -> VectorResult<()> {
        if let Self::Vec(index) = self {
            index.close().await?;
        }
        Ok(())
    }
}

pub(super) fn search_results_equal(left: &VectorSearchResult, right: &VectorSearchResult) -> bool {
    left.searched_records == right.searched_records
        && left.truncated == right.truncated
        && left.hits.len() == right.hits.len()
        && left.hits.iter().zip(&right.hits).all(|(left, right)| {
            left.id == right.id
                && left.partition == right.partition
                && left.score.to_bits() == right.score.to_bits()
        })
}

pub(super) fn status_from_snapshot(snapshot: VecShadowSnapshot) -> VectorIndexStatus {
    VectorIndexStatus {
        revision: VectorRevision::new(snapshot.revision),
        partition_count: snapshot.partition_count,
        record_count: snapshot.record_count,
        byte_count: snapshot.accounted_bytes,
    }
}

pub(super) fn snapshot_from_status(status: VectorIndexStatus) -> VecShadowSnapshot {
    VecShadowSnapshot {
        revision: status.revision.value(),
        partition_count: status.partition_count,
        record_count: status.record_count,
        accounted_bytes: status.byte_count,
    }
}

pub(super) fn vector_error_from_shadow_failure(failure: VecShadowFailure) -> VectorIndexError {
    VectorIndexError::StorageFailed(format!(
        "A3S Vec workspace operation failed: {}",
        failure.code()
    ))
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
