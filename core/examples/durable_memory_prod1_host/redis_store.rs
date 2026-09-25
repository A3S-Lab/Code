//! Redis storage layer for the `DM-PROD1` vector index.
//!
//! This module owns the keyspace, the wire encoding, the `meta` hash accounting,
//! and the record preparation rules. It knows nothing about the `VectorIndex`
//! trait; [`super::redis_index`] composes these pieces into the CAS contract.

use super::connection::RedisLease;
use a3s_memory::vector::{
    VectorBudgetResource, VectorIndexDescriptor, VectorIndexError, VectorIndexStatus, VectorMetric,
    VectorNormalization, VectorRecord, VectorResult, VectorRevision,
};
use redis::RedisError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const HISTORY_DIGEST_DOMAIN: &str = "a3s.code.dm-prod1.redis-vector-index-history.v1";
/// Bound on optimistic retries before a caller sees a conflict as an error.
pub(crate) const MAX_CAS_ATTEMPTS: usize = 64;

pub(crate) const FIELD_REVISION: &str = "revision";
pub(crate) const FIELD_HISTORY: &str = "history";
pub(crate) const FIELD_DESCRIPTOR: &str = "descriptor";
pub(crate) const FIELD_PARTITION_COUNT: &str = "partitionCount";
pub(crate) const FIELD_RECORD_COUNT: &str = "recordCount";
pub(crate) const FIELD_BYTE_COUNT: &str = "byteCount";

/// Every key this index owns, derived from one caller-supplied prefix.
pub(crate) struct Keyspace {
    pub(crate) prefix: String,
    pub(crate) meta: String,
    pub(crate) partitions: String,
}

impl Keyspace {
    pub(crate) fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            meta: format!("{prefix}:meta"),
            partitions: format!("{prefix}:partitions"),
        }
    }

    pub(crate) fn labels(&self, partition: &str) -> String {
        format!("{}:labels:{partition}", self.prefix)
    }

    pub(crate) fn vectors(&self, partition: &str) -> String {
        format!("{}:vectors:{partition}", self.prefix)
    }
}

/// Stored per-partition identifiers, labels, and accounted byte total.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StoredLabels {
    pub(crate) ids: Vec<String>,
    pub(crate) labels: Vec<BTreeMap<String, String>>,
    #[serde(rename = "byteCount")]
    pub(crate) byte_count: usize,
}

/// One decoded partition plus its raw row-major vectors.
pub(crate) struct LoadedPartition {
    pub(crate) name: String,
    pub(crate) labels: StoredLabels,
    pub(crate) vectors: Vec<f32>,
}

/// The published `meta` hash: revision, history identity, and totals.
#[derive(Clone, Debug)]
pub(crate) struct Meta {
    pub(crate) revision: VectorRevision,
    pub(crate) history: String,
    pub(crate) partition_count: usize,
    pub(crate) record_count: usize,
    pub(crate) byte_count: usize,
}

impl Meta {
    pub(crate) fn status(&self) -> VectorIndexStatus {
        VectorIndexStatus {
            revision: self.revision,
            partition_count: self.partition_count,
            record_count: self.record_count,
            byte_count: self.byte_count,
        }
    }
}

/// A prepared partition replacement ready to publish.
pub(crate) struct PreparedPartition {
    pub(crate) labels: StoredLabels,
    pub(crate) vectors: Vec<f32>,
    pub(crate) record_count: usize,
}

/// Accounting for one prospective published revision.
pub(crate) struct MutationPlan {
    pub(crate) revision: VectorRevision,
    pub(crate) partition_count: usize,
    pub(crate) record_count: usize,
    pub(crate) byte_count: usize,
}

/// Derive the next revision's totals, or `None` when the mutation is a no-op.
pub(crate) fn plan_mutation(
    descriptor: &VectorIndexDescriptor,
    meta: &Meta,
    existing: Option<(usize, usize)>,
    prepared: Option<&PreparedPartition>,
) -> VectorResult<Option<MutationPlan>> {
    let incoming_records = prepared.map_or(0, |prepared| prepared.record_count);
    if incoming_records == 0 && existing.is_none() {
        return Ok(None);
    }
    let (old_records, old_bytes) = existing.unwrap_or((0, 0));
    let record_count = meta
        .record_count
        .checked_sub(old_records)
        .and_then(|count| count.checked_add(incoming_records))
        .ok_or(VectorIndexError::SizeOverflow)?;
    let retained_bytes = meta
        .byte_count
        .checked_sub(old_bytes)
        .ok_or(VectorIndexError::SizeOverflow)?;
    let byte_count = if incoming_records == 0 {
        retained_bytes
    } else {
        retained_bytes
            .checked_add(prepared.map_or(0, |prepared| prepared.labels.byte_count))
            .ok_or(VectorIndexError::SizeOverflow)?
    };
    if record_count > descriptor.max_records {
        return Err(VectorIndexError::BudgetExceeded {
            resource: VectorBudgetResource::Records,
            limit: descriptor.max_records,
            required: record_count,
        });
    }
    if byte_count > descriptor.max_bytes {
        return Err(VectorIndexError::BudgetExceeded {
            resource: VectorBudgetResource::Bytes,
            limit: descriptor.max_bytes,
            required: byte_count,
        });
    }
    let partition_count = match (existing.is_some(), incoming_records > 0) {
        (true, false) => meta
            .partition_count
            .checked_sub(1)
            .ok_or(VectorIndexError::SizeOverflow)?,
        (false, true) => meta
            .partition_count
            .checked_add(1)
            .ok_or(VectorIndexError::SizeOverflow)?,
        _ => meta.partition_count,
    };
    Ok(Some(MutationPlan {
        revision: next_revision(meta.revision)?,
        partition_count,
        record_count,
        byte_count,
    }))
}

/// Validate and pack one partition's records, accounting bytes as we go.
pub(crate) fn prepare_partition(
    descriptor: &VectorIndexDescriptor,
    name: &str,
    records: Vec<VectorRecord>,
) -> VectorResult<PreparedPartition> {
    if records.len() > descriptor.max_records {
        return Err(VectorIndexError::BudgetExceeded {
            resource: VectorBudgetResource::Records,
            limit: descriptor.max_records,
            required: records.len(),
        });
    }
    let vector_bytes = descriptor
        .dimension
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or(VectorIndexError::SizeOverflow)?;
    let mut byte_count = name.len();
    let mut seen = BTreeSet::new();
    let mut ids = Vec::with_capacity(records.len());
    let mut labels = Vec::with_capacity(records.len());
    let mut vectors = Vec::with_capacity(records.len().saturating_mul(descriptor.dimension));

    for (record_index, record) in records.into_iter().enumerate() {
        if record.id.trim().is_empty() {
            return Err(VectorIndexError::InvalidRecordId {
                partition: name.to_string(),
                record_index,
            });
        }
        if !seen.insert(record.id.clone()) {
            return Err(VectorIndexError::DuplicateRecordId {
                partition: name.to_string(),
                id: record.id,
            });
        }
        if record.labels.keys().any(|key| key.trim().is_empty()) {
            return Err(VectorIndexError::InvalidLabel {
                context: format!("record '{}' in partition '{name}'", record.id),
            });
        }
        let label_bytes = record
            .labels
            .iter()
            .try_fold(0usize, |total, (key, value)| {
                total
                    .checked_add(key.len())
                    .and_then(|total| total.checked_add(value.len()))
                    .ok_or(VectorIndexError::SizeOverflow)
            })?;
        byte_count = byte_count
            .checked_add(record.id.len())
            .and_then(|total| total.checked_add(label_bytes))
            .and_then(|total| total.checked_add(vector_bytes))
            .ok_or(VectorIndexError::SizeOverflow)?;
        if byte_count > descriptor.max_bytes {
            return Err(VectorIndexError::BudgetExceeded {
                resource: VectorBudgetResource::Bytes,
                limit: descriptor.max_bytes,
                required: byte_count,
            });
        }
        let context = format!("record '{}' in partition '{name}'", record.id);
        let embedding = prepare_vector(record.embedding, descriptor, &context)?;
        ids.push(record.id);
        labels.push(record.labels);
        vectors.extend(embedding);
    }

    let record_count = ids.len();
    Ok(PreparedPartition {
        labels: StoredLabels {
            ids,
            labels,
            byte_count,
        },
        vectors,
        record_count,
    })
}

/// Check one vector against the descriptor, normalizing when it demands it.
pub(crate) fn prepare_vector(
    mut vector: Vec<f32>,
    descriptor: &VectorIndexDescriptor,
    context: &str,
) -> VectorResult<Vec<f32>> {
    if vector.len() != descriptor.dimension {
        return Err(VectorIndexError::DimensionMismatch {
            context: context.to_string(),
            expected: descriptor.dimension,
            actual: vector.len(),
        });
    }
    if let Some(element_index) = vector.iter().position(|value| !value.is_finite()) {
        return Err(VectorIndexError::NonFiniteVector {
            context: context.to_string(),
            element_index,
        });
    }
    if descriptor.normalization == VectorNormalization::Unit {
        let norm = vector
            .iter()
            .fold(0.0f64, |sum, value| {
                let value = f64::from(*value);
                sum + value * value
            })
            .sqrt();
        if norm == 0.0 {
            return Err(VectorIndexError::ZeroVector {
                context: context.to_string(),
            });
        }
        for value in &mut vector {
            *value = (f64::from(*value) / norm) as f32;
        }
    }
    Ok(vector)
}

pub(crate) fn similarity(query: &[f32], candidate: &[f32], metric: VectorMetric) -> f32 {
    let dot = query
        .iter()
        .zip(candidate)
        .fold(0.0f64, |sum, (left, right)| {
            sum + f64::from(*left) * f64::from(*right)
        });
    match metric {
        VectorMetric::Cosine => dot.clamp(-1.0, 1.0) as f32,
        VectorMetric::DotProduct => dot as f32,
    }
}

pub(crate) fn validate_partition(partition: &str) -> VectorResult<&str> {
    let partition = partition.trim();
    if partition.is_empty() {
        Err(VectorIndexError::InvalidPartition)
    } else {
        Ok(partition)
    }
}

pub(crate) fn validate_descriptor(descriptor: &VectorIndexDescriptor) -> VectorResult<()> {
    if descriptor.dimension == 0 {
        return Err(VectorIndexError::InvalidDescriptor(
            "dimension must be greater than zero".into(),
        ));
    }
    if descriptor.metric == VectorMetric::Cosine
        && descriptor.normalization != VectorNormalization::Unit
    {
        return Err(VectorIndexError::InvalidDescriptor(
            "cosine indexes require unit normalization".into(),
        ));
    }
    Ok(())
}

pub(crate) fn canonical_descriptor(descriptor: &VectorIndexDescriptor) -> VectorResult<String> {
    serde_json::to_string(descriptor)
        .map_err(|error| VectorIndexError::InvalidDescriptor(error.to_string()))
}

pub(crate) fn next_revision(revision: VectorRevision) -> VectorResult<VectorRevision> {
    revision
        .value()
        .checked_add(1)
        .map(VectorRevision::new)
        .ok_or(VectorIndexError::RevisionExhausted)
}

fn new_history_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(HISTORY_DIGEST_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

pub(crate) fn encode_vectors(vectors: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vectors));
    for value in vectors {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(crate) fn decode_vectors(bytes: &[u8]) -> VectorResult<Vec<f32>> {
    let (chunks, remainder) = bytes.as_chunks::<{ std::mem::size_of::<f32>() }>();
    if !remainder.is_empty() {
        return Err(VectorIndexError::StorageCorrupted(
            "vector payload is not a whole number of f32 values".into(),
        ));
    }
    Ok(chunks.iter().copied().map(f32::from_le_bytes).collect())
}

pub(crate) async fn watch(
    connection: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Result<(), RedisError> {
    redis::cmd("WATCH").arg(key).exec_async(connection).await
}

pub(crate) async fn unwatch(
    connection: &mut redis::aio::MultiplexedConnection,
) -> Result<(), RedisError> {
    redis::cmd("UNWATCH").exec_async(connection).await
}

pub(crate) async fn read_meta(
    connection: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Result<Option<Meta>, RedisError> {
    let fields: BTreeMap<String, String> = redis::cmd("HGETALL")
        .arg(key)
        .query_async(connection)
        .await?;
    if fields.is_empty() {
        return Ok(None);
    }
    let number = |field: &str| -> Result<usize, RedisError> {
        fields
            .get(field)
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                fatal(VectorIndexError::StorageCorrupted(format!(
                    "meta field '{field}' is missing or unreadable"
                )))
            })
    };
    let history = fields.get(FIELD_HISTORY).cloned().ok_or_else(|| {
        fatal(VectorIndexError::StorageCorrupted(
            "meta field 'history' is missing".into(),
        ))
    })?;
    Ok(Some(Meta {
        revision: VectorRevision::new(number(FIELD_REVISION)? as u64),
        history,
        partition_count: number(FIELD_PARTITION_COUNT)?,
        record_count: number(FIELD_RECORD_COUNT)?,
        byte_count: number(FIELD_BYTE_COUNT)?,
    }))
}

/// Existing `(record_count, byte_count)` for one partition, if it is published.
pub(crate) async fn read_partition_totals(
    connection: &mut redis::aio::MultiplexedConnection,
    keys: &Keyspace,
    partition: &str,
) -> Result<Option<(usize, usize)>, RedisError> {
    let labels: Option<String> = redis::cmd("GET")
        .arg(keys.labels(partition))
        .query_async(connection)
        .await?;
    let Some(labels) = labels else {
        return Ok(None);
    };
    let labels: StoredLabels = serde_json::from_str(&labels).map_err(|error| {
        fatal(VectorIndexError::StorageCorrupted(format!(
            "partition '{partition}' labels are unreadable: {error}"
        )))
    })?;
    Ok(Some((labels.ids.len(), labels.byte_count)))
}

/// Read the meta hash, minting a fresh history identity for a new keyspace.
pub(crate) async fn initialize_meta(
    lease: &mut RedisLease<'_>,
    keys: &Keyspace,
    descriptor_json: &str,
) -> Result<Option<Meta>, RedisError> {
    for _ in 0..MAX_CAS_ATTEMPTS {
        let connection = lease.connection();
        watch(connection, &keys.meta).await?;
        if let Some(meta) = read_meta(connection, &keys.meta).await? {
            unwatch(connection).await?;
            return Ok(Some(meta));
        }
        let history = new_history_digest();
        let committed: Option<()> = redis::pipe()
            .atomic()
            .cmd("HSET")
            .arg(&keys.meta)
            .arg(FIELD_REVISION)
            .arg(0u64)
            .arg(FIELD_HISTORY)
            .arg(&history)
            .arg(FIELD_DESCRIPTOR)
            .arg(descriptor_json)
            .arg(FIELD_PARTITION_COUNT)
            .arg(0u64)
            .arg(FIELD_RECORD_COUNT)
            .arg(0u64)
            .arg(FIELD_BYTE_COUNT)
            .arg(0u64)
            .ignore()
            .query_async(lease.connection())
            .await?;
        if committed.is_some() {
            return Ok(Some(Meta {
                revision: VectorRevision::default(),
                history,
                partition_count: 0,
                record_count: 0,
                byte_count: 0,
            }));
        }
    }
    Ok(None)
}

/// Delete only the keys this index owns; no database is flushed.
pub(crate) async fn delete_keyspace(
    lease: &mut RedisLease<'_>,
    keys: &Keyspace,
) -> Result<usize, RedisError> {
    let connection = lease.connection();
    let names: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&keys.partitions)
        .query_async(connection)
        .await?;
    let mut command = redis::cmd("DEL");
    command.arg(&keys.meta).arg(&keys.partitions);
    for name in &names {
        command.arg(keys.labels(name)).arg(keys.vectors(name));
    }
    command.query_async(connection).await
}

const FATAL_PREFIX: &str = "a3s-dm-prod1-fatal: ";

/// Smuggle a typed [`VectorIndexError`] through `redis`'s error channel so the
/// retry wrapper can distinguish contract failures from connection loss.
pub(crate) fn fatal(error: VectorIndexError) -> RedisError {
    RedisError::from((
        redis::ErrorKind::ClientError,
        "vector index contract failure",
        format!("{FATAL_PREFIX}{error}"),
    ))
}

pub(crate) fn extract_fatal(error: &RedisError) -> Option<VectorIndexError> {
    let detail = error.detail()?;
    let message = detail.strip_prefix(FATAL_PREFIX)?;
    Some(classify_fatal(message))
}

fn classify_fatal(message: &str) -> VectorIndexError {
    if let Some(rest) = message.strip_prefix("vector index revision conflict: expected ") {
        let mut parts = rest.split(", actual ");
        if let (Some(expected), Some(actual)) = (parts.next(), parts.next()) {
            if let (Ok(expected), Ok(actual)) = (expected.parse::<u64>(), actual.parse::<u64>()) {
                return VectorIndexError::RevisionConflict {
                    expected: VectorRevision::new(expected),
                    actual: VectorRevision::new(actual),
                };
            }
        }
    }
    if message.contains("is corrupted") {
        return VectorIndexError::StorageCorrupted(message.to_string());
    }
    VectorIndexError::StorageFailed(message.to_string())
}

pub(crate) fn storage_failed(error: RedisError) -> VectorIndexError {
    VectorIndexError::StorageFailed(error.to_string())
}
