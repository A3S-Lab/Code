use super::{
    DurableMemorySemanticError, DurableMemorySemanticRecall, DurableMemorySession,
    SemanticRefreshEmbeddingCache, DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
};
use a3s_memory::repository::{
    MemoryNamespace, MemoryNamespaceChangeToken, MemoryNamespaceSnapshot, MemoryRepository,
    MemorySnapshotRequest, MemoryStatus, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_NODES,
};
use a3s_memory::vector::{VectorIndexChangeToken, VectorMutationConsistency};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[path = "semantic_refresh/checkpoint.rs"]
mod checkpoint;
#[path = "semantic_refresh/receipt.rs"]
mod receipt;
pub use checkpoint::{
    DurableMemorySemanticRefreshCheckpoint, DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1,
};
use receipt::VectorIndexObservation;
pub use receipt::{
    DurableMemorySemanticRefreshReceipt, DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1,
};

pub(crate) enum DurableMemorySemanticRefreshRun {
    Published {
        receipt: DurableMemorySemanticRefreshReceipt,
        embedding_cache: Option<Arc<SemanticRefreshEmbeddingCache>>,
    },
    Unchanged(DurableMemorySemanticRefreshReceipt),
}

pub(crate) struct DurableMemorySemanticRefreshAttempt {
    pub(crate) result: Result<DurableMemorySemanticRefreshRun, DurableMemorySemanticError>,
    pub(crate) work: DurableMemorySemanticRefreshWork,
    pub(crate) elapsed: Duration,
}

#[derive(Default)]
pub(crate) struct DurableMemorySemanticRefreshWork {
    pub(crate) source_change_token_requests: usize,
    pub(crate) source_change_token_observations: usize,
    pub(crate) source_snapshot_requests: usize,
    pub(crate) source_snapshot_node_reads: usize,
    pub(crate) source_snapshot_bytes: usize,
    pub(crate) embedding_cache_hits: usize,
    pub(crate) embedding_inputs: usize,
    pub(crate) embedding_input_bytes: usize,
    pub(crate) provider_requests: usize,
    pub(crate) provider_inputs: usize,
    pub(crate) provider_input_bytes: usize,
    pub(crate) publication_attempts: usize,
    pub(crate) publication_records: usize,
}

impl DurableMemorySemanticRefreshWork {
    fn observe_change_token_request(&mut self) {
        self.source_change_token_requests = self.source_change_token_requests.saturating_add(1);
    }

    fn observe_change_token(&mut self) {
        self.source_change_token_observations =
            self.source_change_token_observations.saturating_add(1);
    }

    fn observe_snapshot_request(&mut self) {
        self.source_snapshot_requests = self.source_snapshot_requests.saturating_add(1);
    }

    fn observe_snapshot(&mut self, snapshot: &MemoryNamespaceSnapshot) {
        self.source_snapshot_node_reads = self
            .source_snapshot_node_reads
            .saturating_add(snapshot.nodes().len());
        self.source_snapshot_bytes = self
            .source_snapshot_bytes
            .saturating_add(snapshot.byte_count());
    }
}

async fn read_source_change_token(
    repository: &dyn MemoryRepository,
    namespace: &MemoryNamespace,
    work: Option<&mut DurableMemorySemanticRefreshWork>,
) -> Result<Option<MemoryNamespaceChangeToken>, DurableMemorySemanticError> {
    let mut work = work;
    if let Some(work) = work.as_deref_mut() {
        work.observe_change_token_request();
    }
    let token = repository.namespace_change_token(namespace).await?;
    if let Some(token) = token.as_ref() {
        token.verify()?;
        if let Some(work) = work {
            work.observe_change_token();
        }
    }
    Ok(token)
}

fn read_index_change_token(
    semantic: &DurableMemorySemanticRecall,
) -> Result<Option<VectorIndexChangeToken>, DurableMemorySemanticError> {
    let token = semantic.index_change_token();
    if let Some(token) = token.as_ref() {
        token.verify()?;
    }
    Ok(token)
}

impl DurableMemorySemanticRefreshRun {
    pub(crate) fn into_receipt(self) -> DurableMemorySemanticRefreshReceipt {
        match self {
            Self::Published { receipt, .. } | Self::Unchanged(receipt) => receipt,
        }
    }
}

enum SemanticRefreshCacheMode<'a> {
    Disabled,
    Capture(Option<&'a SemanticRefreshEmbeddingCache>),
}

struct SemanticRefreshExecution<'a> {
    repository: &'a dyn MemoryRepository,
    namespace: &'a MemoryNamespace,
    required_consistency: VectorMutationConsistency,
    previous: Option<&'a DurableMemorySemanticRefreshReceipt>,
    previous_requires_index_continuity: bool,
    cache_mode: SemanticRefreshCacheMode<'a>,
    cancellation: CancellationToken,
}

impl DurableMemorySession {
    pub(crate) async fn refresh_semantic_recall_scheduled(
        &self,
        previous: Option<&DurableMemorySemanticRefreshReceipt>,
        previous_cache: Option<&SemanticRefreshEmbeddingCache>,
        previous_requires_index_continuity: bool,
        cancellation: CancellationToken,
    ) -> DurableMemorySemanticRefreshAttempt {
        let started = Instant::now();
        let mut work = DurableMemorySemanticRefreshWork::default();
        let result = match self.semantic_recall.as_ref() {
            Some(semantic) => {
                semantic
                    .refresh_repository_namespace_if_stale(
                        SemanticRefreshExecution {
                            repository: self.repository.as_ref(),
                            namespace: &self.namespace,
                            required_consistency: VectorMutationConsistency::IndexRevisionCas,
                            previous,
                            previous_requires_index_continuity,
                            cache_mode: SemanticRefreshCacheMode::Capture(previous_cache),
                            cancellation,
                        },
                        Some(&mut work),
                    )
                    .await
            }
            None => Err(DurableMemorySemanticError::InvalidConfiguration {
                field: "semanticRecall",
                reason: "refresh requires an attached semantic recall generation".to_string(),
            }),
        };
        DurableMemorySemanticRefreshAttempt {
            result,
            work,
            elapsed: started.elapsed(),
        }
    }
}

impl DurableMemorySemanticRecall {
    pub(super) async fn refresh_repository_namespace(
        &self,
        repository: &dyn MemoryRepository,
        namespace: &MemoryNamespace,
        required_consistency: VectorMutationConsistency,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshReceipt, DurableMemorySemanticError> {
        self.refresh_repository_namespace_if_stale(
            SemanticRefreshExecution {
                repository,
                namespace,
                required_consistency,
                previous: None,
                previous_requires_index_continuity: false,
                cache_mode: SemanticRefreshCacheMode::Disabled,
                cancellation,
            },
            None,
        )
        .await
        .map(DurableMemorySemanticRefreshRun::into_receipt)
    }

    async fn refresh_repository_namespace_if_stale(
        &self,
        execution: SemanticRefreshExecution<'_>,
        mut work: Option<&mut DurableMemorySemanticRefreshWork>,
    ) -> Result<DurableMemorySemanticRefreshRun, DurableMemorySemanticError> {
        let SemanticRefreshExecution {
            repository,
            namespace,
            required_consistency,
            previous,
            previous_requires_index_continuity,
            cache_mode,
            cancellation,
        } = execution;
        if cancellation.is_cancelled() {
            return Err(crate::embedding::EmbeddingError::Cancelled.into());
        }
        let refresh_lock = self.refresh_lock();
        let _refresh_guard = tokio::select! {
            guard = refresh_lock.lock() => guard,
            _ = cancellation.cancelled() => {
                return Err(crate::embedding::EmbeddingError::Cancelled.into());
            }
        };
        let publication = self.begin_index_publication(required_consistency)?;
        let request = MemorySnapshotRequest::new(
            namespace.clone(),
            self.refresh_node_limit()?.min(MAX_SNAPSHOT_NODES),
            self.refresh_snapshot_byte_limit().min(MAX_SNAPSHOT_BYTES),
        )
        .with_statuses([MemoryStatus::Active]);
        let source_change_token_before = tokio::select! {
            result = read_source_change_token(repository, namespace, work.as_deref_mut()) => result?,
            _ = cancellation.cancelled() => {
                return Err(crate::embedding::EmbeddingError::Cancelled.into());
            }
        };
        if let (Some(previous), Some(token)) = (previous, source_change_token_before.as_ref()) {
            // CAS captures the index revision before the token read, and this
            // status read observes it afterward. The schedule keeps receipts
            // inside one repository-history ownership epoch, so exact token
            // equality proves the source snapshot identity is still current.
            let current_index_change_token = read_index_change_token(self)?;
            let current_index_status = self.index_status();
            if previous.matches_current_change_token(
                self,
                token,
                VectorIndexObservation {
                    consistency: publication.consistency(),
                    expected_revision: publication.expected_revision(),
                    change_token: current_index_change_token.as_ref(),
                    status: &current_index_status,
                    require_history_continuity: false,
                },
            ) {
                return Ok(DurableMemorySemanticRefreshRun::Unchanged(previous.clone()));
            }
        }
        if let Some(work) = work.as_deref_mut() {
            work.observe_snapshot_request();
        }
        let before = tokio::select! {
            result = repository.snapshot_namespace(request.clone()) => result?,
            _ = cancellation.cancelled() => {
                return Err(crate::embedding::EmbeddingError::Cancelled.into());
            }
        };
        if let Some(work) = work.as_deref_mut() {
            work.observe_snapshot(&before);
        }
        before.verify(&request)?;
        let stable_source_change_token = match source_change_token_before {
            Some(expected) => {
                let observed = tokio::select! {
                    result = read_source_change_token(repository, namespace, work.as_deref_mut()) => result?,
                    _ = cancellation.cancelled() => {
                        return Err(crate::embedding::EmbeddingError::Cancelled.into());
                    }
                };
                match observed {
                    Some(actual) if actual == expected => Some(expected),
                    Some(_) => {
                        return Err(DurableMemorySemanticError::RepositoryChangedDuringRefresh);
                    }
                    None => None,
                }
            }
            None => None,
        };
        if let Some(previous) = previous {
            // The verified snapshot remains the compatibility proof when a
            // backend does not expose an exact change token. It also advances
            // a receipt's token after a namespace-only change left the Active
            // projection unchanged.
            let current_index_change_token = read_index_change_token(self)?;
            let current_index_status = self.index_status();
            if previous.matches_current(
                self,
                &before,
                VectorIndexObservation {
                    consistency: publication.consistency(),
                    expected_revision: publication.expected_revision(),
                    change_token: current_index_change_token.as_ref(),
                    status: &current_index_status,
                    require_history_continuity: previous_requires_index_continuity,
                },
            ) {
                return Ok(DurableMemorySemanticRefreshRun::Unchanged(
                    previous.with_source_change_token(stable_source_change_token),
                ));
            }
        }
        let source_snapshot_profile = before.profile().to_string();
        let source_snapshot_digest = before.digest().to_string();
        let source_snapshot_bytes = before.byte_count();
        let active_node_count = before.nodes().len();

        let (index_status, embedding_cache) = match cache_mode {
            SemanticRefreshCacheMode::Disabled => (
                self.replace_namespace_locked(
                    namespace,
                    before.into_nodes(),
                    cancellation,
                    publication,
                )
                .await?,
                None,
            ),
            SemanticRefreshCacheMode::Capture(previous_cache) => {
                let replacement_attempt = self
                    .replace_namespace_locked_reusing(
                        namespace,
                        before.into_nodes(),
                        previous_cache,
                        cancellation,
                        publication,
                    )
                    .await;
                if let Some(work) = work.as_deref_mut() {
                    work.embedding_cache_hits = replacement_attempt.work.embedding_cache_hits;
                    work.embedding_inputs = replacement_attempt.work.embedding_inputs;
                    work.embedding_input_bytes = replacement_attempt.work.embedding_input_bytes;
                    work.provider_requests = replacement_attempt.work.provider_requests;
                    work.provider_inputs = replacement_attempt.work.provider_inputs;
                    work.provider_input_bytes = replacement_attempt.work.provider_input_bytes;
                    work.publication_attempts = replacement_attempt.work.publication_attempts;
                    work.publication_records = replacement_attempt.work.publication_records;
                }
                let replacement = replacement_attempt.result?;
                (
                    replacement.status,
                    Some(Arc::new(replacement.embedding_cache)),
                )
            }
        };
        let cleanup_publication = publication.after_publication(index_status.revision);

        // Publication is the commit point. Finish source verification even if
        // the caller cancels after the atomic index replacement completed. A
        // stable exact token avoids rereading the full namespace; capability
        // loss falls back to the original verified snapshot proof.
        let mut source_change_token = None;
        let require_snapshot_verification =
            match stable_source_change_token {
                Some(expected) => {
                    let observed =
                        match read_source_change_token(repository, namespace, work.as_deref_mut())
                            .await
                        {
                            Ok(observed) => observed,
                            Err(error) => {
                                self.invalidate_namespace(namespace, cleanup_publication)
                                    .await?;
                                return Err(error);
                            }
                        };
                    match observed {
                        Some(actual) if actual == expected => {
                            source_change_token = Some(expected);
                            false
                        }
                        Some(_) => {
                            self.invalidate_namespace(namespace, cleanup_publication)
                                .await?;
                            return Err(DurableMemorySemanticError::RepositoryChangedDuringRefresh);
                        }
                        None => true,
                    }
                }
                None => true,
            };
        if require_snapshot_verification {
            if let Some(work) = work.as_deref_mut() {
                work.observe_snapshot_request();
            }
            let after = match repository.snapshot_namespace(request.clone()).await {
                Ok(after) => after,
                Err(error) => {
                    self.invalidate_namespace(namespace, cleanup_publication)
                        .await?;
                    return Err(error.into());
                }
            };
            if let Some(work) = work {
                work.observe_snapshot(&after);
            }
            if let Err(error) = after.verify(&request) {
                self.invalidate_namespace(namespace, cleanup_publication)
                    .await?;
                return Err(error.into());
            }
            if after.digest() != source_snapshot_digest {
                self.invalidate_namespace(namespace, cleanup_publication)
                    .await?;
                return Err(DurableMemorySemanticError::RepositoryChangedDuringRefresh);
            }
        }
        let index_change_token = read_index_change_token(self)?;
        let current_index_status = self.index_status();
        if current_index_status != index_status
            || index_change_token
                .as_ref()
                .is_some_and(|token| token.revision() != index_status.revision)
        {
            return Err(DurableMemorySemanticError::IndexRevisionChanged);
        }

        Ok(DurableMemorySemanticRefreshRun::Published {
            receipt: DurableMemorySemanticRefreshReceipt {
                profile: DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1.to_string(),
                source_snapshot_profile,
                source_snapshot_digest,
                source_snapshot_bytes,
                source_change_token,
                semantic_binding_schema: DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1.to_string(),
                serving_generation_digest: self.serving_generation_digest().to_string(),
                active_node_count,
                mutation_consistency: publication.consistency(),
                index_change_token,
                index_status,
            },
            embedding_cache,
        })
    }
}
