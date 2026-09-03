use super::memory_vector_adapter::MemoryVectorIndexAdapter;
use super::vec_shadow_store::{
    VecShadowFailure, VecShadowSearchResult, VecShadowSnapshot, VecShadowStore,
};
use super::vector_authority::{
    search_results_equal, snapshot_from_status, PrimaryVectorIndex, VecPrimaryVectorIndex,
};
use super::vector_contract::{
    VectorIndexChangeToken, VectorIndexDescriptor, VectorIndexError, VectorIndexObservation,
    VectorIndexStatus, VectorMutationConsistency, VectorRecord, VectorResult, VectorRevision,
    VectorSearchRequest, VectorSearchResult, WorkspaceVectorIndex,
};
use super::{WorkspaceVecShadowPhase, WorkspaceVecShadowStatus, WorkspaceVectorEngine};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

const PHASE_READY: u8 = 1;
const PHASE_DEGRADED: u8 = 2;
const PHASE_CLOSED: u8 = 3;

#[derive(Debug, Default)]
struct VecShadowDiagnostics {
    phase: AtomicU8,
    revision: AtomicU64,
    record_count: AtomicU64,
    accounted_bytes: AtomicU64,
    initialization_failures: AtomicU64,
    successful_mutations: AtomicU64,
    failed_mutations: AtomicU64,
    compared_queries: AtomicU64,
    matching_queries: AtomicU64,
    mismatched_queries: AtomicU64,
    failed_queries: AtomicU64,
}

impl VecShadowDiagnostics {
    fn ready(&self, snapshot: VecShadowSnapshot) {
        self.update_snapshot(snapshot);
        self.phase.store(PHASE_READY, Ordering::Release);
    }

    fn initialization_failed(&self, failure: VecShadowFailure) {
        self.initialization_failures.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(
            failure_code = failure.code(),
            "A3S Vec workspace shadow initialization failed"
        );
    }

    fn mutation_succeeded(&self, snapshot: VecShadowSnapshot) {
        self.update_snapshot(snapshot);
        self.successful_mutations.fetch_add(1, Ordering::Relaxed);
    }

    fn mutation_failed(&self, failure: VecShadowFailure) {
        self.failed_mutations.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(
            failure_code = failure.code(),
            "A3S Vec workspace shadow mutation failed"
        );
    }

    fn query_compared(&self, matches: bool) {
        self.compared_queries.fetch_add(1, Ordering::Relaxed);
        if matches {
            self.matching_queries.fetch_add(1, Ordering::Relaxed);
        } else {
            self.mismatched_queries.fetch_add(1, Ordering::Relaxed);
            self.degrade();
            tracing::warn!("A3S Vec workspace shadow query differed from the Memory oracle");
        }
    }

    fn query_failed(&self, failure: VecShadowFailure) {
        self.failed_queries.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(
            failure_code = failure.code(),
            "A3S Vec workspace shadow query failed"
        );
    }

    fn memory_shadow_mutation_failed(&self, operation: &'static str) {
        self.failed_mutations.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(operation, "A3S Memory workspace shadow operation failed");
    }

    fn memory_shadow_query_failed(&self, operation: &'static str) {
        self.failed_queries.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(operation, "A3S Memory workspace shadow query failed");
    }

    fn primary_mutation_failed(&self, operation: &'static str) {
        self.failed_mutations.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(operation, "workspace vector primary mutation failed");
    }

    fn primary_query_failed(&self, operation: &'static str) {
        self.failed_queries.fetch_add(1, Ordering::Relaxed);
        self.degrade();
        tracing::warn!(operation, "workspace vector primary query failed");
    }

    fn close(&self) {
        self.revision.store(0, Ordering::Relaxed);
        self.record_count.store(0, Ordering::Relaxed);
        self.accounted_bytes.store(0, Ordering::Relaxed);
        self.phase.store(PHASE_CLOSED, Ordering::Release);
    }

    fn degrade(&self) {
        if self.phase.load(Ordering::Acquire) != PHASE_CLOSED {
            self.phase.store(PHASE_DEGRADED, Ordering::Release);
        }
    }

    fn update_snapshot(&self, snapshot: VecShadowSnapshot) {
        self.revision.store(snapshot.revision, Ordering::Relaxed);
        self.record_count.store(
            u64::try_from(snapshot.record_count).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.accounted_bytes.store(
            u64::try_from(snapshot.accounted_bytes).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn status(&self) -> WorkspaceVecShadowStatus {
        WorkspaceVecShadowStatus {
            phase: match self.phase.load(Ordering::Acquire) {
                PHASE_READY => WorkspaceVecShadowPhase::Ready,
                PHASE_DEGRADED => WorkspaceVecShadowPhase::Degraded,
                PHASE_CLOSED => WorkspaceVecShadowPhase::Closed,
                _ => WorkspaceVecShadowPhase::Disabled,
            },
            revision: self.revision.load(Ordering::Relaxed),
            record_count: usize::try_from(self.record_count.load(Ordering::Relaxed))
                .unwrap_or(usize::MAX),
            accounted_bytes: usize::try_from(self.accounted_bytes.load(Ordering::Relaxed))
                .unwrap_or(usize::MAX),
            initialization_failures: self.initialization_failures.load(Ordering::Relaxed),
            successful_mutations: self.successful_mutations.load(Ordering::Relaxed),
            failed_mutations: self.failed_mutations.load(Ordering::Relaxed),
            compared_queries: self.compared_queries.load(Ordering::Relaxed),
            matching_queries: self.matching_queries.load(Ordering::Relaxed),
            mismatched_queries: self.mismatched_queries.load(Ordering::Relaxed),
            failed_queries: self.failed_queries.load(Ordering::Relaxed),
        }
    }
}

/// Code-owned migration adapter for the workspace vector authority.
///
/// The compatibility default keeps A3S Memory as the serving primary and
/// mirrors every admitted mutation/query into Vec. Both engines expose
/// revision-CAS for delayed partition writers. An explicit `A3sVec`
/// selection reverses that direction while retaining a Memory differential
/// shadow. Both modes share one publication gate; a shadow mismatch degrades
/// the evidence without changing the selected serving result.
pub(super) struct ShadowVectorIndex {
    primary: PrimaryVectorIndex,
    memory_shadow: Option<MemoryVectorIndexAdapter>,
    vec_shadow: Option<VecShadowStore>,
    diagnostics: Arc<VecShadowDiagnostics>,
    publication_gate: tokio::sync::RwLock<()>,
    active_engine: WorkspaceVectorEngine,
}

impl ShadowVectorIndex {
    pub(super) fn new_with_engine(
        descriptor: VectorIndexDescriptor,
        active_engine: WorkspaceVectorEngine,
    ) -> VectorResult<Self> {
        let diagnostics = Arc::new(VecShadowDiagnostics::default());
        let (primary, memory_shadow, vec_shadow) = match active_engine {
            WorkspaceVectorEngine::A3sMemory => {
                let primary =
                    PrimaryVectorIndex::Memory(MemoryVectorIndexAdapter::new(descriptor.clone())?);
                let vec_shadow = match VecShadowStore::create(&descriptor) {
                    Ok((shadow, snapshot)) => {
                        diagnostics.ready(snapshot);
                        Some(shadow)
                    }
                    Err(failure) => {
                        diagnostics.initialization_failed(failure);
                        None
                    }
                };
                (primary, None, vec_shadow)
            }
            WorkspaceVectorEngine::A3sVec => {
                let primary =
                    PrimaryVectorIndex::Vec(VecPrimaryVectorIndex::new(descriptor.clone())?);
                diagnostics.ready(snapshot_from_status(primary.status()));
                let memory_shadow = Some(MemoryVectorIndexAdapter::new(descriptor)?);
                (primary, memory_shadow, None)
            }
        };
        Ok(Self {
            primary,
            memory_shadow,
            vec_shadow,
            diagnostics,
            publication_gate: tokio::sync::RwLock::new(()),
            active_engine,
        })
    }

    pub(super) fn shadow_status(&self) -> WorkspaceVecShadowStatus {
        self.diagnostics.status()
    }

    pub(super) fn active_engine(&self) -> WorkspaceVectorEngine {
        self.active_engine
    }

    pub(super) async fn close(&self) {
        let _publication = self.publication_gate.write().await;
        if let Some(shadow) = &self.vec_shadow {
            if let Err(failure) = shadow.close().await {
                tracing::warn!(
                    failure_code = failure.code(),
                    "A3S Vec workspace shadow close failed"
                );
            }
        }
        if let Some(shadow) = &self.memory_shadow {
            if let Err(error) = shadow.clear().await {
                tracing::warn!(error = %error, "A3S Memory workspace shadow close clear failed");
            }
        }
        if let Err(error) = self.primary.close().await {
            tracing::warn!(
                error = %error,
                "A3S Vec workspace primary close failed"
            );
        }
        self.diagnostics.close();
    }

    async fn mirror_vec_replace(&self, partition: &str, records: Vec<VectorRecord>) {
        let result = match &self.vec_shadow {
            Some(shadow) => {
                shadow
                    .replace_partition(partition.to_string(), records)
                    .await
            }
            None => Err(VecShadowFailure::Unavailable),
        };
        match result {
            Ok(snapshot) => self.diagnostics.mutation_succeeded(snapshot),
            Err(failure) => self.diagnostics.mutation_failed(failure),
        }
    }

    async fn mirror_vec_remove(&self, partition: &str) {
        let result = match &self.vec_shadow {
            Some(shadow) => shadow.remove_partition(partition.to_string()).await,
            None => Err(VecShadowFailure::Unavailable),
        };
        match result {
            Ok(snapshot) => self.diagnostics.mutation_succeeded(snapshot),
            Err(failure) => self.diagnostics.mutation_failed(failure),
        }
    }

    async fn search_vec_shadow(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VecShadowSearchResult, VecShadowFailure> {
        match &self.vec_shadow {
            Some(shadow) => shadow.search(request).await,
            None => Err(VecShadowFailure::Unavailable),
        }
    }

    async fn mirror_memory_replace(&self, partition: &str, records: Vec<VectorRecord>) {
        let Some(shadow) = &self.memory_shadow else {
            return;
        };
        if let Err(error) = shadow.replace_partition(partition, records).await {
            self.diagnostics
                .memory_shadow_mutation_failed("replace_partition");
            tracing::warn!(partition, error = %error, "A3S Memory workspace shadow mutation failed");
        }
    }

    async fn mirror_memory_remove(&self, partition: &str) {
        let Some(shadow) = &self.memory_shadow else {
            return;
        };
        if let Err(error) = shadow.remove_partition(partition).await {
            self.diagnostics
                .memory_shadow_mutation_failed("remove_partition");
            tracing::warn!(partition, error = %error, "A3S Memory workspace shadow removal failed");
        }
    }

    async fn search_memory_shadow(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VectorSearchResult, VectorIndexError> {
        match &self.memory_shadow {
            Some(shadow) => shadow.search(request).await,
            None => Err(VectorIndexError::StorageFailed(
                "Memory shadow is unavailable".to_string(),
            )),
        }
    }

    fn record_comparison(
        &self,
        primary: &VectorSearchResult,
        shadow: Result<VecShadowSearchResult, VecShadowFailure>,
    ) {
        match shadow {
            Ok(shadow) => self
                .diagnostics
                .query_compared(search_results_match(primary, &shadow)),
            Err(failure) => self.diagnostics.query_failed(failure),
        }
    }

    fn record_memory_comparison(
        &self,
        primary: &VectorSearchResult,
        shadow: Result<VectorSearchResult, VectorIndexError>,
    ) {
        match shadow {
            Ok(shadow) => self
                .diagnostics
                .query_compared(search_results_equal(primary, &shadow)),
            Err(error) => {
                self.diagnostics.memory_shadow_query_failed("search");
                tracing::warn!(error = %error, "A3S Memory workspace shadow query failed");
            }
        }
    }
}

#[async_trait::async_trait]
impl WorkspaceVectorIndex for ShadowVectorIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        self.primary.descriptor()
    }

    fn status(&self) -> VectorIndexStatus {
        self.primary.status()
    }

    fn change_token(&self) -> Option<VectorIndexChangeToken> {
        self.primary.change_token()
    }

    async fn observe(&self) -> VectorResult<VectorIndexObservation> {
        self.primary.observe().await
    }

    fn mutation_consistency(&self) -> VectorMutationConsistency {
        self.primary.mutation_consistency()
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let shadow_records = records.clone();
        let status = match self.primary.replace_partition(partition, records).await {
            Ok(status) => status,
            Err(error) => {
                self.diagnostics
                    .primary_mutation_failed("replace_partition");
                return Err(error);
            }
        };
        match self.active_engine {
            WorkspaceVectorEngine::A3sMemory => {
                self.mirror_vec_replace(partition, shadow_records).await;
            }
            WorkspaceVectorEngine::A3sVec => {
                self.mirror_memory_replace(partition, shadow_records).await;
                self.diagnostics
                    .mutation_succeeded(snapshot_from_status(status.clone()));
            }
        }
        Ok(status)
    }

    async fn replace_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let shadow_records = records.clone();
        let status = match self
            .primary
            .replace_partition_if_revision(partition, expected_revision, records)
            .await
        {
            Ok(status) => status,
            Err(error) => {
                if !matches!(
                    &error,
                    VectorIndexError::ConditionalMutationUnsupported
                        | VectorIndexError::RevisionConflict { .. }
                ) {
                    self.diagnostics
                        .primary_mutation_failed("replace_partition_if_revision");
                }
                return Err(error);
            }
        };
        match self.active_engine {
            WorkspaceVectorEngine::A3sMemory => {
                self.mirror_vec_replace(partition, shadow_records).await;
            }
            WorkspaceVectorEngine::A3sVec => {
                self.mirror_memory_replace(partition, shadow_records).await;
                self.diagnostics
                    .mutation_succeeded(snapshot_from_status(status.clone()));
            }
        }
        Ok(status)
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let status = match self.primary.remove_partition(partition).await {
            Ok(status) => status,
            Err(error) => {
                self.diagnostics.primary_mutation_failed("remove_partition");
                return Err(error);
            }
        };
        match self.active_engine {
            WorkspaceVectorEngine::A3sMemory => self.mirror_vec_remove(partition).await,
            WorkspaceVectorEngine::A3sVec => {
                self.mirror_memory_remove(partition).await;
                self.diagnostics
                    .mutation_succeeded(snapshot_from_status(status.clone()));
            }
        }
        Ok(status)
    }

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
    ) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let status = match self
            .primary
            .remove_partition_if_revision(partition, expected_revision)
            .await
        {
            Ok(status) => status,
            Err(error) => {
                if !matches!(
                    &error,
                    VectorIndexError::ConditionalMutationUnsupported
                        | VectorIndexError::RevisionConflict { .. }
                ) {
                    self.diagnostics
                        .primary_mutation_failed("remove_partition_if_revision");
                }
                return Err(error);
            }
        };
        match self.active_engine {
            WorkspaceVectorEngine::A3sMemory => self.mirror_vec_remove(partition).await,
            WorkspaceVectorEngine::A3sVec => {
                self.mirror_memory_remove(partition).await;
                self.diagnostics
                    .mutation_succeeded(snapshot_from_status(status.clone()));
            }
        }
        Ok(status)
    }

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        let _publication = self.publication_gate.read().await;
        let shadow_request = request.clone();
        let primary_request = request;
        let primary = self.primary.search(primary_request);
        let shadow = match self.active_engine {
            WorkspaceVectorEngine::A3sMemory => {
                let (primary, shadow) =
                    tokio::join!(primary, self.search_vec_shadow(shadow_request));
                let primary = match primary {
                    Ok(primary) => primary,
                    Err(error) => {
                        self.diagnostics.primary_query_failed("search");
                        return Err(error);
                    }
                };
                self.record_comparison(&primary, shadow);
                primary
            }
            WorkspaceVectorEngine::A3sVec => {
                let (primary, shadow) =
                    tokio::join!(primary, self.search_memory_shadow(shadow_request));
                let primary = match primary {
                    Ok(primary) => primary,
                    Err(error) => {
                        self.diagnostics.primary_query_failed("search");
                        return Err(error);
                    }
                };
                self.record_memory_comparison(&primary, shadow);
                primary
            }
        };
        Ok(shadow)
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let status = match self.primary.clear().await {
            Ok(status) => status,
            Err(error) => {
                self.diagnostics.primary_mutation_failed("clear");
                return Err(error);
            }
        };
        match self.active_engine {
            WorkspaceVectorEngine::A3sMemory => {
                let result = match &self.vec_shadow {
                    Some(shadow) => shadow.clear().await,
                    None => Err(VecShadowFailure::Unavailable),
                };
                match result {
                    Ok(snapshot) => self.diagnostics.mutation_succeeded(snapshot),
                    Err(failure) => self.diagnostics.mutation_failed(failure),
                }
            }
            WorkspaceVectorEngine::A3sVec => {
                if let Some(shadow) = &self.memory_shadow {
                    if let Err(error) = shadow.clear().await {
                        self.diagnostics.memory_shadow_mutation_failed("clear");
                        tracing::warn!(error = %error, "A3S Memory workspace shadow clear failed");
                    }
                }
                self.diagnostics
                    .mutation_succeeded(snapshot_from_status(status.clone()));
            }
        }
        Ok(status)
    }
}

fn search_results_match(primary: &VectorSearchResult, shadow: &VecShadowSearchResult) -> bool {
    primary.searched_records == shadow.searched_records
        && primary.truncated == shadow.truncated
        && primary.hits.len() == shadow.hits.len()
        && primary.hits.iter().zip(&shadow.hits).all(|(left, right)| {
            left.id == right.id
                && left.partition == right.partition
                && left.score.to_bits() == right.score.to_bits()
        })
}
