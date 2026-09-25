//! Durable remote `VectorIndex` backed by Redis with index-revision CAS.
//!
//! Every content mutation rewrites the `meta` hash, so `WATCH meta` before a
//! `MULTI`/`EXEC` publishes a partition at one linearization point. A delayed
//! writer that captured an older revision is rejected instead of overwriting a
//! newer generation, which is what `VectorMutationConsistency::IndexRevisionCas`
//! promises.
//!
//! The history digest is minted once per keyspace and read back on reopen, so
//! revision equality stays meaningful across restarts and rotates whenever the
//! keyspace is independently recreated.
//!
//! The keyspace, wire encoding, and accounting rules live in
//! [`super::redis_store`].

use super::connection::{is_connection_loss, RedisLease, RedisSocket, MAX_RECONNECT_ATTEMPTS};
use super::redis_store::{
    canonical_descriptor, decode_vectors, delete_keyspace, encode_vectors, extract_fatal, fatal,
    initialize_meta, plan_mutation, prepare_partition, prepare_vector, read_meta,
    read_partition_totals, similarity, storage_failed, unwatch, validate_descriptor,
    validate_partition, watch, Keyspace, LoadedPartition, Meta, PreparedPartition, StoredLabels,
    FIELD_BYTE_COUNT, FIELD_DESCRIPTOR, FIELD_PARTITION_COUNT, FIELD_RECORD_COUNT, FIELD_REVISION,
    MAX_CAS_ATTEMPTS,
};
use a3s_memory::vector::{
    VectorIndex, VectorIndexChangeToken, VectorIndexDescriptor, VectorIndexError,
    VectorIndexObservation, VectorIndexStatus, VectorMutationConsistency, VectorRecord,
    VectorResult, VectorRevision, VectorSearchHit, VectorSearchRequest, VectorSearchResult,
};
use redis::{FromRedisValue, RedisError, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Run an awaited Redis sequence with an exclusive socket bound to `$lease`,
/// re-dialling once per connection drop and surfacing contract failures as-is.
///
/// This is a macro rather than a higher-order function because the body must
/// borrow from the caller's scope while the lease itself is created inside the
/// retry loop; a `for<'a>`-quantified closure cannot express that.
macro_rules! with_socket {
    ($socket:expr, $lease:ident, $body:expr) => {{
        let socket: &RedisSocket = $socket;
        let mut settled = None;
        let mut last: Option<RedisError> = None;
        for _ in 0..MAX_RECONNECT_ATTEMPTS {
            #[allow(unused_mut)]
            let mut $lease = match socket.lease().await {
                Ok(lease) => lease,
                Err(error) => {
                    if !is_connection_loss(&error) {
                        return Err(storage_failed(error));
                    }
                    last = Some(error);
                    continue;
                }
            };
            match $body {
                Ok(value) => {
                    settled = Some(value);
                    break;
                }
                Err(error) => {
                    if let Some(typed) = extract_fatal(&error) {
                        return Err(typed);
                    }
                    if !is_connection_loss(&error) {
                        return Err(storage_failed(error));
                    }
                    $lease.invalidate();
                    last = Some(error);
                }
            }
        }
        match settled {
            Some(value) => value,
            None => {
                return Err(storage_failed(last.unwrap_or_else(|| {
                    RedisError::from((
                        redis::ErrorKind::IoError,
                        "redis operation exhausted retries",
                    ))
                })));
            }
        }
    }};
}

/// Redis vector index scoped to one caller-owned key prefix.
pub struct RedisVectorIndex {
    socket: Arc<RedisSocket>,
    keys: Keyspace,
    descriptor: VectorIndexDescriptor,
    history_digest: String,
    cached_status: Mutex<VectorIndexStatus>,
    cas_conflicts: AtomicU64,
    cas_aborts: AtomicU64,
}

impl std::fmt::Debug for RedisVectorIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisVectorIndex")
            .field("prefix", &self.keys.prefix)
            .field("descriptor", &self.descriptor)
            .field("historyDigest", &self.history_digest)
            .finish_non_exhaustive()
    }
}

impl RedisVectorIndex {
    /// Open or initialize the index rooted at `prefix`.
    ///
    /// An existing keyspace must agree on the descriptor and keeps its original
    /// history digest, so a reopened handle continues the same revision history.
    pub async fn open(
        socket: Arc<RedisSocket>,
        prefix: impl Into<String>,
        descriptor: VectorIndexDescriptor,
    ) -> VectorResult<Self> {
        validate_descriptor(&descriptor)?;
        let keys = Keyspace::new(&prefix.into());
        let descriptor_json = canonical_descriptor(&descriptor)?;

        let meta = with_socket!(
            &socket,
            lease,
            initialize_meta(&mut lease, &keys, &descriptor_json).await
        )
        .ok_or_else(|| {
            VectorIndexError::StorageFailed("index initialization exhausted CAS attempts".into())
        })?;

        let stored_descriptor = with_socket!(
            &socket,
            lease,
            redis::cmd("HGET")
                .arg(&keys.meta)
                .arg(FIELD_DESCRIPTOR)
                .query_async::<Option<String>>(lease.connection())
                .await
        )
        .ok_or_else(|| VectorIndexError::StorageCorrupted("meta hash has no descriptor".into()))?;
        if stored_descriptor != descriptor_json {
            return Err(VectorIndexError::DescriptorMismatch);
        }
        VectorIndexChangeToken::try_new(meta.history.clone(), meta.revision)?;

        Ok(Self {
            socket,
            keys,
            descriptor,
            history_digest: meta.history.clone(),
            cached_status: Mutex::new(meta.status()),
            cas_conflicts: AtomicU64::new(0),
            cas_aborts: AtomicU64::new(0),
        })
    }

    /// Durable history identity minted when this keyspace was created.
    pub fn history_digest(&self) -> &str {
        &self.history_digest
    }

    /// Rejected conditional mutations caused by a newer published revision.
    pub fn cas_conflicts(&self) -> u64 {
        self.cas_conflicts.load(Ordering::SeqCst)
    }

    /// Optimistic `EXEC` aborts caused by a concurrent writer touching `meta`.
    pub fn cas_aborts(&self) -> u64 {
        self.cas_aborts.load(Ordering::SeqCst)
    }

    /// Sockets replaced after a connection drop.
    pub fn reconnects(&self) -> u64 {
        self.socket.reconnects()
    }

    /// Remove every key owned by this index, simulating independent loss.
    ///
    /// Scoped to this prefix so unrelated keys in the same database survive.
    pub async fn destroy_keyspace(&self) -> VectorResult<usize> {
        Ok(with_socket!(
            &self.socket,
            lease,
            delete_keyspace(&mut lease, &self.keys).await
        ))
    }

    fn cache_status(&self, status: VectorIndexStatus) {
        let mut cached = self
            .cached_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if status.revision >= cached.revision {
            *cached = status;
        }
    }

    /// Read the exact published meta hash in one atomic command.
    async fn read_observation(&self) -> VectorResult<VectorIndexObservation> {
        let meta = with_socket!(
            &self.socket,
            lease,
            read_meta(lease.connection(), &self.keys.meta).await
        )
        .ok_or_else(|| VectorIndexError::StorageCorrupted("meta hash disappeared".into()))?;
        self.verify_history(&meta)?;
        let status = meta.status();
        self.cache_status(status.clone());
        let observation = VectorIndexObservation {
            status,
            change_token: Some(VectorIndexChangeToken::try_new(
                meta.history,
                meta.revision,
            )?),
        };
        observation.verify()?;
        Ok(observation)
    }

    fn verify_history(&self, meta: &Meta) -> VectorResult<()> {
        if meta.history == self.history_digest {
            Ok(())
        } else {
            Err(VectorIndexError::StorageCorrupted(
                "index history identity rotated underneath an open handle".into(),
            ))
        }
    }

    /// Publish one prepared partition (or removal) under `WATCH`/`MULTI`/`EXEC`.
    async fn publish(
        &self,
        partition: String,
        prepared: Option<PreparedPartition>,
        expected_revision: Option<VectorRevision>,
    ) -> VectorResult<VectorIndexStatus> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let outcome = with_socket!(
                &self.socket,
                lease,
                self.publish_once(&mut lease, &partition, prepared.as_ref(), expected_revision)
                    .await
            );
            match outcome {
                PublishOutcome::Committed(status) => {
                    self.cache_status(status.clone());
                    return Ok(status);
                }
                PublishOutcome::Aborted => {
                    self.cas_aborts.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        Err(VectorIndexError::StorageFailed(
            "partition publication exhausted CAS attempts".into(),
        ))
    }

    async fn publish_once(
        &self,
        lease: &mut RedisLease<'_>,
        partition: &str,
        prepared: Option<&PreparedPartition>,
        expected_revision: Option<VectorRevision>,
    ) -> Result<PublishOutcome, RedisError> {
        let connection = lease.connection();
        watch(connection, &self.keys.meta).await?;
        let meta = match read_meta(connection, &self.keys.meta).await? {
            Some(meta) => meta,
            None => {
                unwatch(connection).await?;
                return Err(fatal(VectorIndexError::StorageCorrupted(
                    "meta hash disappeared".into(),
                )));
            }
        };
        if let Err(error) = self.verify_history(&meta) {
            unwatch(connection).await?;
            return Err(fatal(error));
        }
        if let Some(expected) = expected_revision {
            if meta.revision != expected {
                unwatch(connection).await?;
                self.cas_conflicts.fetch_add(1, Ordering::SeqCst);
                return Err(fatal(VectorIndexError::RevisionConflict {
                    expected,
                    actual: meta.revision,
                }));
            }
        }

        let existing = read_partition_totals(connection, &self.keys, partition).await?;
        let plan = match plan_mutation(&self.descriptor, &meta, existing, prepared) {
            Ok(Some(plan)) => plan,
            Ok(None) => {
                unwatch(connection).await?;
                return Ok(PublishOutcome::Committed(meta.status()));
            }
            Err(error) => {
                unwatch(connection).await?;
                return Err(fatal(error));
            }
        };

        let mut pipeline = redis::pipe();
        pipeline.atomic();
        pipeline
            .cmd("HSET")
            .arg(&self.keys.meta)
            .arg(FIELD_REVISION)
            .arg(plan.revision.value())
            .arg(FIELD_PARTITION_COUNT)
            .arg(plan.partition_count)
            .arg(FIELD_RECORD_COUNT)
            .arg(plan.record_count)
            .arg(FIELD_BYTE_COUNT)
            .arg(plan.byte_count)
            .ignore();
        match prepared {
            Some(prepared) if prepared.record_count > 0 => {
                let labels = serde_json::to_string(&prepared.labels)
                    .map_err(|error| fatal(VectorIndexError::StorageFailed(error.to_string())))?;
                pipeline
                    .cmd("SET")
                    .arg(self.keys.labels(partition))
                    .arg(labels)
                    .ignore()
                    .cmd("SET")
                    .arg(self.keys.vectors(partition))
                    .arg(encode_vectors(&prepared.vectors))
                    .ignore()
                    .cmd("SADD")
                    .arg(&self.keys.partitions)
                    .arg(partition)
                    .ignore();
            }
            _ => {
                pipeline
                    .cmd("DEL")
                    .arg(self.keys.labels(partition))
                    .arg(self.keys.vectors(partition))
                    .ignore()
                    .cmd("SREM")
                    .arg(&self.keys.partitions)
                    .arg(partition)
                    .ignore();
            }
        }
        let committed: Option<()> = pipeline.query_async(lease.connection()).await?;
        Ok(match committed {
            Some(()) => PublishOutcome::Committed(VectorIndexStatus {
                revision: plan.revision,
                partition_count: plan.partition_count,
                record_count: plan.record_count,
                byte_count: plan.byte_count,
            }),
            None => PublishOutcome::Aborted,
        })
    }

    /// Read one self-consistent snapshot of the partitions a query may touch.
    async fn load_snapshot(
        &self,
        selected: &BTreeSet<String>,
    ) -> VectorResult<(Meta, Vec<LoadedPartition>)> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let outcome = with_socket!(
                &self.socket,
                lease,
                self.load_snapshot_once(&mut lease, selected).await
            );
            if let Some(snapshot) = outcome {
                self.cache_status(snapshot.0.status());
                return Ok(snapshot);
            }
            self.cas_aborts.fetch_add(1, Ordering::SeqCst);
        }
        Err(VectorIndexError::StorageFailed(
            "snapshot read exhausted CAS attempts".into(),
        ))
    }

    async fn load_snapshot_once(
        &self,
        lease: &mut RedisLease<'_>,
        selected: &BTreeSet<String>,
    ) -> Result<Option<(Meta, Vec<LoadedPartition>)>, RedisError> {
        let connection = lease.connection();
        watch(connection, &self.keys.meta).await?;
        let meta = match read_meta(connection, &self.keys.meta).await? {
            Some(meta) => meta,
            None => {
                unwatch(connection).await?;
                return Err(fatal(VectorIndexError::StorageCorrupted(
                    "meta hash disappeared".into(),
                )));
            }
        };
        if let Err(error) = self.verify_history(&meta) {
            unwatch(connection).await?;
            return Err(fatal(error));
        }
        let mut names: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&self.keys.partitions)
            .query_async(connection)
            .await?;
        if !selected.is_empty() {
            names.retain(|name| selected.contains(name));
        }
        names.sort();
        if names.is_empty() {
            unwatch(connection).await?;
            return Ok(Some((meta, Vec::new())));
        }

        let mut pipeline = redis::pipe();
        pipeline.atomic();
        for name in &names {
            pipeline
                .cmd("GET")
                .arg(self.keys.labels(name))
                .cmd("GET")
                .arg(self.keys.vectors(name));
        }
        let values: Option<Vec<Value>> = pipeline.query_async(lease.connection()).await?;
        let Some(values) = values else {
            return Ok(None);
        };
        if values.len() != names.len() * 2 {
            return Err(fatal(VectorIndexError::StorageCorrupted(
                "partition read returned an unexpected reply count".into(),
            )));
        }

        let mut partitions = Vec::with_capacity(names.len());
        for (index, name) in names.into_iter().enumerate() {
            let labels: Option<String> =
                FromRedisValue::from_owned_redis_value(values[index * 2].clone())?;
            let raw: Option<Vec<u8>> =
                FromRedisValue::from_owned_redis_value(values[index * 2 + 1].clone())?;
            let (Some(labels), Some(raw)) = (labels, raw) else {
                return Err(fatal(VectorIndexError::StorageCorrupted(format!(
                    "partition '{name}' is listed but its payload is missing"
                ))));
            };
            let labels: StoredLabels = serde_json::from_str(&labels).map_err(|error| {
                fatal(VectorIndexError::StorageCorrupted(format!(
                    "partition '{name}' labels are unreadable: {error}"
                )))
            })?;
            let vectors = decode_vectors(&raw).map_err(fatal)?;
            if vectors.len() != labels.ids.len() * self.descriptor.dimension {
                return Err(fatal(VectorIndexError::StorageCorrupted(format!(
                    "partition '{name}' vector payload does not match its identifiers"
                ))));
            }
            partitions.push(LoadedPartition {
                name,
                labels,
                vectors,
            });
        }
        Ok(Some((meta, partitions)))
    }
}

enum PublishOutcome {
    Committed(VectorIndexStatus),
    Aborted,
}

#[async_trait::async_trait]
impl VectorIndex for RedisVectorIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        &self.descriptor
    }

    fn status(&self) -> VectorIndexStatus {
        self.cached_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn change_token(&self) -> Option<VectorIndexChangeToken> {
        VectorIndexChangeToken::try_new(self.history_digest.clone(), self.status().revision).ok()
    }

    async fn observe(&self) -> VectorResult<VectorIndexObservation> {
        self.read_observation().await
    }

    fn mutation_consistency(&self) -> VectorMutationConsistency {
        VectorMutationConsistency::IndexRevisionCas
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        let partition = validate_partition(partition)?.to_string();
        let prepared = prepare_partition(&self.descriptor, &partition, records)?;
        self.publish(partition, Some(prepared), None).await
    }

    async fn replace_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        let partition = validate_partition(partition)?.to_string();
        let prepared = prepare_partition(&self.descriptor, &partition, records)?;
        self.publish(partition, Some(prepared), Some(expected_revision))
            .await
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        let partition = validate_partition(partition)?.to_string();
        self.publish(partition, None, None).await
    }

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
    ) -> VectorResult<VectorIndexStatus> {
        let partition = validate_partition(partition)?.to_string();
        self.publish(partition, None, Some(expected_revision)).await
    }

    async fn search(&self, mut request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        if request.limit == 0 {
            return Err(VectorIndexError::InvalidRequest(
                "limit must be greater than zero".to_string(),
            ));
        }
        if request
            .partitions
            .iter()
            .any(|partition| partition.trim().is_empty())
        {
            return Err(VectorIndexError::InvalidPartition);
        }
        if request.labels.keys().any(|key| key.trim().is_empty()) {
            return Err(VectorIndexError::InvalidLabel {
                context: "query filter".to_string(),
            });
        }
        let query = prepare_vector(
            std::mem::take(&mut request.embedding),
            &self.descriptor,
            "query",
        )?;
        let (meta, partitions) = self.load_snapshot(&request.partitions).await?;

        let mut hits: Vec<VectorSearchHit> = Vec::new();
        let mut searched_records = 0usize;
        for partition in &partitions {
            for (index, id) in partition.labels.ids.iter().enumerate() {
                let labels = partition.labels.labels.get(index).cloned().ok_or_else(|| {
                    VectorIndexError::StorageCorrupted(format!(
                        "partition '{}' has no labels for record '{id}'",
                        partition.name
                    ))
                })?;
                if !request
                    .labels
                    .iter()
                    .all(|(key, value)| labels.get(key) == Some(value))
                {
                    continue;
                }
                searched_records = searched_records.saturating_add(1);
                let start = index * self.descriptor.dimension;
                let vector = &partition.vectors[start..start + self.descriptor.dimension];
                let score = similarity(&query, vector, self.descriptor.metric);
                if !score.is_finite() {
                    return Err(VectorIndexError::ScoreOverflow {
                        partition: partition.name.clone(),
                        id: id.clone(),
                    });
                }
                hits.push(VectorSearchHit {
                    id: id.clone(),
                    partition: partition.name.clone(),
                    score,
                    labels,
                });
            }
        }
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.partition.cmp(&right.partition))
                .then_with(|| left.id.cmp(&right.id))
        });
        let truncated = hits.len() > request.limit;
        hits.truncate(request.limit);
        Ok(VectorSearchResult {
            hits,
            status: meta.status(),
            searched_records,
            truncated,
        })
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        let observation = self.read_observation().await?;
        let (_, partitions) = self.load_snapshot(&BTreeSet::new()).await?;
        if partitions.is_empty() {
            return Ok(observation.status);
        }
        // Each removal is its own CAS publication, so clearing is only atomic
        // per partition. Callers that need one-revision clearing should use the
        // in-memory or SQLite backends.
        let mut status = observation.status;
        for partition in partitions {
            status = self.publish(partition.name, None, None).await?;
        }
        Ok(status)
    }
}
