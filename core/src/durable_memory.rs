//! Host-bound durable-memory integration.
//!
//! Code owns extraction and admission policy. `a3s-memory` owns the exact
//! namespace and repository integrity boundary. V2 recall is explicit and
//! admits only the current active node revision selected by final assembly.

mod binding;
mod context;
mod fusion;
mod policy;
mod semantic;
mod semantic_binding;
mod semantic_refresh;
pub use binding::{
    DurableMemoryBindingV1, DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
    DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION, DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
};
pub(crate) use context::{durable_memory_context_id, DurableMemoryRecallIdentity};
pub use context::{DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1, DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2};
pub use policy::{
    DurableMemoryMode, DurableMemoryRecallChannel, DurableMemoryRecallHit,
    DurableMemoryRecallPolicy, DurableMemoryRecallPreview,
};
pub use semantic::DurableMemorySemanticRecall;
pub use semantic_binding::{
    DurableMemorySemanticBindingV1, DurableMemorySemanticError, DurableMemorySemanticRecallPolicy,
    DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1, DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1,
};
pub use semantic_refresh::{
    DurableMemorySemanticRefreshReceipt, DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1,
};

use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, MemoryAccessEvent, MemoryChangeSet,
    MemoryNamespace, MemoryNode, MemoryNodeDraft, MemoryOperation, MemoryRepository,
    MemoryRepositoryError, MemoryStatus, MAX_IDENTIFIER_BYTES,
};
use a3s_memory::vector::VectorMutationConsistency;
use a3s_memory::{MemoryItem, MemoryType};
use chrono::{DateTime, Utc};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Explicit, evidence-backed request to activate one candidate revision.
#[derive(Debug, Clone)]
pub struct DurableMemoryActivation {
    idempotency_key: String,
    node_id: String,
    expected_revision: u64,
    decision_evidence: EvidenceRef,
    occurred_at: DateTime<Utc>,
}

impl DurableMemoryActivation {
    pub fn try_new(
        idempotency_key: impl Into<String>,
        node_id: impl Into<String>,
        expected_revision: u64,
        decision_evidence: EvidenceRef,
        occurred_at: DateTime<Utc>,
    ) -> Result<Self, MemoryRepositoryError> {
        let idempotency_key = idempotency_key.into();
        let node_id = node_id.into();
        validate_identifier("activation.idempotencyKey", &idempotency_key)?;
        validate_identifier("activation.nodeId", &node_id)?;
        if expected_revision == 0 {
            return Err(invalid(
                "activation.expectedRevision",
                "must be greater than zero",
            ));
        }
        if !matches!(
            decision_evidence.kind,
            EvidenceKind::Manual | EvidenceKind::Verification
        ) {
            return Err(invalid(
                "activation.decisionEvidence.kind",
                "must be manual or verification evidence",
            ));
        }
        if decision_evidence.occurred_at > occurred_at {
            return Err(invalid(
                "activation.decisionEvidence.occurredAt",
                "must not follow activation occurredAt",
            ));
        }
        Ok(Self {
            idempotency_key,
            node_id,
            expected_revision,
            decision_evidence,
            occurred_at,
        })
    }
}

/// Explicit observation that a caller used one exact active node revision.
#[derive(Debug, Clone)]
pub struct DurableMemoryUse {
    event_id: String,
    node_id: String,
    node_revision: u64,
    occurred_at: DateTime<Utc>,
    context_id: Option<String>,
}

impl DurableMemoryUse {
    pub fn try_new(
        event_id: impl Into<String>,
        node_id: impl Into<String>,
        node_revision: u64,
        occurred_at: DateTime<Utc>,
    ) -> Result<Self, MemoryRepositoryError> {
        let event_id = event_id.into();
        let node_id = node_id.into();
        validate_identifier("use.eventId", &event_id)?;
        validate_identifier("use.nodeId", &node_id)?;
        if node_revision == 0 {
            return Err(invalid("use.nodeRevision", "must be greater than zero"));
        }
        Ok(Self {
            event_id,
            node_id,
            node_revision,
            occurred_at,
            context_id: None,
        })
    }

    pub fn with_context_id(mut self, context_id: impl Into<String>) -> Self {
        self.context_id = Some(context_id.into());
        self
    }
}

/// Exact repository and namespace supplied by the embedding host.
#[derive(Clone)]
pub struct DurableMemorySession {
    repository: Arc<dyn MemoryRepository>,
    namespace: MemoryNamespace,
    mode: DurableMemoryMode,
    recall_policy: Option<DurableMemoryRecallPolicy>,
    semantic_recall: Option<DurableMemorySemanticRecall>,
}

impl DurableMemorySession {
    /// Create a candidate-only binding. V2 recall remains disabled.
    pub fn shadow(repository: Arc<dyn MemoryRepository>, namespace: MemoryNamespace) -> Self {
        Self {
            repository,
            namespace,
            mode: DurableMemoryMode::ShadowCandidates,
            recall_policy: None,
            semantic_recall: None,
        }
    }

    /// Create an opt-in binding that recalls only explicitly activated nodes.
    pub fn active_recall(
        repository: Arc<dyn MemoryRepository>,
        namespace: MemoryNamespace,
        recall_policy: DurableMemoryRecallPolicy,
    ) -> Self {
        Self {
            repository,
            namespace,
            mode: DurableMemoryMode::ActiveRecall,
            recall_policy: Some(recall_policy),
            semantic_recall: None,
        }
    }

    /// Add a typed host-owned semantic generation to an Active recall binding.
    pub fn with_semantic_recall(
        mut self,
        semantic_recall: DurableMemorySemanticRecall,
    ) -> Result<Self, DurableMemorySemanticError> {
        if self.mode != DurableMemoryMode::ActiveRecall || self.recall_policy.is_none() {
            return Err(DurableMemorySemanticError::InvalidConfiguration {
                field: "mode",
                reason: "semantic recall requires an Active recall binding".to_string(),
            });
        }
        self.semantic_recall = Some(semantic_recall);
        Ok(self)
    }

    pub fn repository(&self) -> &Arc<dyn MemoryRepository> {
        &self.repository
    }

    pub fn namespace(&self) -> &MemoryNamespace {
        &self.namespace
    }

    pub fn mode(&self) -> DurableMemoryMode {
        self.mode
    }

    pub fn recall_policy(&self) -> Option<DurableMemoryRecallPolicy> {
        self.recall_policy
    }

    pub fn semantic_recall(&self) -> Option<&DurableMemorySemanticRecall> {
        self.semantic_recall.as_ref()
    }

    /// Rebuild and verify this binding's semantic partition from one complete
    /// current Active repository snapshot.
    pub async fn refresh_semantic_recall(
        &self,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshReceipt, DurableMemorySemanticError> {
        self.refresh_semantic_recall_requiring(
            VectorMutationConsistency::PartitionAtomic,
            cancellation,
        )
        .await
    }

    /// Rebuild semantic recall while requiring at least the selected vector
    /// mutation ordering. An unsupported requirement fails before repository
    /// snapshot or embedding work begins.
    pub async fn refresh_semantic_recall_requiring(
        &self,
        required_consistency: VectorMutationConsistency,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshReceipt, DurableMemorySemanticError> {
        let semantic = self.semantic_recall.as_ref().ok_or_else(|| {
            DurableMemorySemanticError::InvalidConfiguration {
                field: "semanticRecall",
                reason: "refresh requires an attached semantic recall generation".to_string(),
            }
        })?;
        semantic
            .refresh_repository_namespace(
                self.repository.as_ref(),
                &self.namespace,
                required_consistency,
                cancellation,
            )
            .await
    }

    /// Return the secret-free identity that must remain exact when a
    /// persisted session is resumed.
    pub fn binding(&self) -> DurableMemoryBindingV1 {
        DurableMemoryBindingV1::new(
            self.namespace.clone(),
            self.mode,
            self.recall_policy,
            self.semantic_recall
                .as_ref()
                .map(|semantic| semantic.binding().clone()),
        )
    }

    /// Activate one exact candidate revision with independent decision evidence.
    pub async fn activate_candidate(
        &self,
        activation: DurableMemoryActivation,
    ) -> Result<MemoryNode, MemoryRepositoryError> {
        let result = self
            .repository
            .apply(MemoryChangeSet::new(
                activation.idempotency_key,
                self.namespace.clone(),
                activation.occurred_at,
                vec![MemoryOperation::Activate {
                    node_id: activation.node_id.clone(),
                    expected_revision: activation.expected_revision,
                    evidence: vec![activation.decision_evidence],
                }],
            ))
            .await?;
        result
            .nodes
            .into_iter()
            .find(|node| node.id == activation.node_id && node.status == MemoryStatus::Active)
            .ok_or_else(|| MemoryRepositoryError::InvariantViolation {
                message: "activation change returned no active target node".into(),
            })
    }

    /// Record an explicit use without widening this binding's namespace.
    pub async fn record_use(&self, usage: DurableMemoryUse) -> Result<(), MemoryRepositoryError> {
        let mut event = MemoryAccessEvent::new(
            usage.event_id,
            self.namespace.clone(),
            usage.node_id,
            usage.node_revision,
            usage.occurred_at,
        );
        if let Some(context_id) = usage.context_id {
            event = event.with_context_id(context_id);
        }
        self.repository.record_use(event).await
    }

    pub(crate) async fn store_shadow_candidate(
        &self,
        item: &MemoryItem,
        evidence: &DurableTurnEvidence,
    ) -> Result<MemoryNode, MemoryRepositoryError> {
        let kind = match item.memory_type {
            MemoryType::Episodic => DurableMemoryKind::Episodic,
            MemoryType::Semantic => DurableMemoryKind::Semantic,
            MemoryType::Procedural => DurableMemoryKind::Procedural,
            MemoryType::Working => {
                return Err(MemoryRepositoryError::InvalidInput {
                    field: "candidate.memoryType".into(),
                    message: "working memory is not durable".into(),
                });
            }
        };
        let confidence = item
            .metadata
            .get("confidence")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .unwrap_or(0.0);
        let mut draft = MemoryNodeDraft::new(
            "content-addressed-after-normalization",
            self.namespace.clone(),
            kind,
            MemoryStatus::Candidate,
            &item.content,
            vec![evidence.reference.clone()],
            evidence.occurred_at,
        )
        .with_confidence(confidence)
        .with_importance(item.importance)
        .with_label("a3s.origin", "code.llm_extraction");
        for (source, label) in [
            ("source", "a3s.extraction.source"),
            ("scope", "a3s.extraction.scope"),
            ("reason", "a3s.extraction.reason"),
            ("schema", "a3s.extraction.schema"),
        ] {
            if let Some(value) = item.metadata.get(source) {
                draft = draft.with_label(label, value);
            }
        }
        if !item.tags.is_empty() {
            let tags = serde_json::to_string(&item.tags).map_err(|error| {
                MemoryRepositoryError::InvalidInput {
                    field: "candidate.tags".into(),
                    message: error.to_string(),
                }
            })?;
            draft = draft.with_label("a3s.extraction.tags", tags);
        }
        draft.id = candidate_id(&draft)?;

        let result = self
            .repository
            .apply(MemoryChangeSet::new(
                draft.id.clone(),
                self.namespace.clone(),
                evidence.occurred_at,
                vec![MemoryOperation::Create { node: draft }],
            ))
            .await?;
        result
            .nodes
            .into_iter()
            .next()
            .ok_or_else(|| MemoryRepositoryError::InvariantViolation {
                message: "candidate change returned no node".into(),
            })
    }
}

fn invalid(field: &str, message: impl Into<String>) -> MemoryRepositoryError {
    MemoryRepositoryError::InvalidInput {
        field: field.into(),
        message: message.into(),
    }
}

fn validate_identifier(field: &str, value: &str) -> Result<(), MemoryRepositoryError> {
    if value.trim().is_empty() {
        return Err(invalid(field, "must not be empty or whitespace"));
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(invalid(
            field,
            format!("must not exceed {MAX_IDENTIFIER_BYTES} bytes"),
        ));
    }
    Ok(())
}

impl std::fmt::Debug for DurableMemorySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableMemorySession")
            .field("namespace", &self.namespace)
            .field("mode", &self.mode)
            .field("recall_policy", &self.recall_policy)
            .field(
                "semantic_recall",
                &self
                    .semantic_recall
                    .as_ref()
                    .map(DurableMemorySemanticRecall::binding),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DurableTurnEvidence {
    reference: EvidenceRef,
    occurred_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnEvidencePayload<'a> {
    schema: &'static str,
    session_id: &'a str,
    prompt: &'a str,
    response: &'a str,
    transcript: &'a str,
}

impl DurableTurnEvidence {
    pub(crate) fn try_new(
        session_id: &str,
        turn_id: &str,
        prompt: &str,
        response: &str,
        transcript: &str,
        occurred_at: DateTime<Utc>,
    ) -> Result<Self, MemoryRepositoryError> {
        let payload = TurnEvidencePayload {
            schema: "a3s.code.memory.turn-evidence.v1",
            session_id,
            prompt,
            response,
            transcript,
        };
        let encoded =
            serde_json::to_vec(&payload).map_err(|error| MemoryRepositoryError::InvalidInput {
                field: "turnEvidence".into(),
                message: error.to_string(),
            })?;
        let digest = format!("sha256:{:x}", Sha256::digest(encoded));
        let session = utf8_percent_encode(session_id, NON_ALPHANUMERIC);
        let turn = utf8_percent_encode(turn_id, NON_ALPHANUMERIC);
        let reference = EvidenceRef::try_new(
            format!("a3s://session/{session}/turn/{turn}"),
            digest,
            EvidenceKind::SessionTurn,
            occurred_at,
        )?;
        Ok(Self {
            reference,
            occurred_at,
        })
    }
}

fn candidate_id(draft: &MemoryNodeDraft) -> Result<String, MemoryRepositoryError> {
    let mut identity = draft.clone();
    identity.id.clear();
    let encoded =
        serde_json::to_vec(&identity).map_err(|error| MemoryRepositoryError::InvalidInput {
            field: "candidate".into(),
            message: error.to_string(),
        })?;
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.code.memory.candidate.v1\0");
    hasher.update(encoded);
    Ok(format!("a3s-code-candidate-{:x}", hasher.finalize()))
}

#[cfg(test)]
#[path = "durable_memory/tests.rs"]
mod tests;
