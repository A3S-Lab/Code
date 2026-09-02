use super::vec_shadow_store::{
    VecShadowFailure, VecShadowSearchResult, VecShadowSnapshot, VecShadowStore,
};
use super::{WorkspaceVecShadowPhase, WorkspaceVecShadowStatus};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexChangeToken, VectorIndexDescriptor,
    VectorIndexObservation, VectorIndexStatus, VectorMutationConsistency, VectorRecord,
    VectorResult, VectorRevision, VectorSearchRequest, VectorSearchResult,
};
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

/// Code-owned migration adapter. Memory remains the result oracle while Vec
/// receives the exact same admitted vectors and is compared under one gate.
pub(super) struct ShadowVectorIndex {
    primary: InMemoryVectorIndex,
    shadow: Option<VecShadowStore>,
    diagnostics: Arc<VecShadowDiagnostics>,
    publication_gate: tokio::sync::RwLock<()>,
}

impl ShadowVectorIndex {
    pub(super) fn new(descriptor: VectorIndexDescriptor) -> VectorResult<Self> {
        let primary = InMemoryVectorIndex::new(descriptor.clone())?;
        let diagnostics = Arc::new(VecShadowDiagnostics::default());
        let shadow = match VecShadowStore::create(&descriptor) {
            Ok((shadow, snapshot)) => {
                diagnostics.ready(snapshot);
                Some(shadow)
            }
            Err(failure) => {
                diagnostics.initialization_failed(failure);
                None
            }
        };
        Ok(Self {
            primary,
            shadow,
            diagnostics,
            publication_gate: tokio::sync::RwLock::new(()),
        })
    }

    pub(super) fn shadow_status(&self) -> WorkspaceVecShadowStatus {
        self.diagnostics.status()
    }

    pub(super) async fn close(&self) {
        let _publication = self.publication_gate.write().await;
        let result = match &self.shadow {
            Some(shadow) => shadow.close().await,
            None => Ok(()),
        };
        if let Err(failure) = result {
            tracing::warn!(
                failure_code = failure.code(),
                "A3S Vec workspace shadow close failed"
            );
        }
        self.diagnostics.close();
    }

    async fn mirror_replace(&self, partition: &str, records: Vec<VectorRecord>) {
        let result = match &self.shadow {
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

    async fn mirror_remove(&self, partition: &str) {
        let result = match &self.shadow {
            Some(shadow) => shadow.remove_partition(partition.to_string()).await,
            None => Err(VecShadowFailure::Unavailable),
        };
        match result {
            Ok(snapshot) => self.diagnostics.mutation_succeeded(snapshot),
            Err(failure) => self.diagnostics.mutation_failed(failure),
        }
    }

    async fn shadow_search(
        &self,
        request: VectorSearchRequest,
    ) -> Result<VecShadowSearchResult, VecShadowFailure> {
        match &self.shadow {
            Some(shadow) => shadow.search(request).await,
            None => Err(VecShadowFailure::Unavailable),
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
}

#[async_trait::async_trait]
impl VectorIndex for ShadowVectorIndex {
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
        let status = self.primary.replace_partition(partition, records).await?;
        self.mirror_replace(partition, shadow_records).await;
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
        let status = self
            .primary
            .replace_partition_if_revision(partition, expected_revision, records)
            .await?;
        self.mirror_replace(partition, shadow_records).await;
        Ok(status)
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let status = self.primary.remove_partition(partition).await?;
        self.mirror_remove(partition).await;
        Ok(status)
    }

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
    ) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let status = self
            .primary
            .remove_partition_if_revision(partition, expected_revision)
            .await?;
        self.mirror_remove(partition).await;
        Ok(status)
    }

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        let _publication = self.publication_gate.read().await;
        let shadow_request = request.clone();
        let (primary, shadow) = tokio::join!(
            self.primary.search(request),
            self.shadow_search(shadow_request)
        );
        let primary = primary?;
        self.record_comparison(&primary, shadow);
        Ok(primary)
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        let _publication = self.publication_gate.write().await;
        let status = self.primary.clear().await?;
        let result = match &self.shadow {
            Some(shadow) => shadow.clear().await,
            None => Err(VecShadowFailure::Unavailable),
        };
        match result {
            Ok(snapshot) => self.diagnostics.mutation_succeeded(snapshot),
            Err(failure) => self.diagnostics.mutation_failed(failure),
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
