//! Host-bound durable-memory integration.
//!
//! Code owns extraction and admission policy. `a3s-memory` owns the exact
//! namespace and repository integrity boundary. The initial integration is a
//! candidate-only shadow mode and never contributes V2 nodes to model context.

use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, MemoryChangeSet, MemoryNamespace, MemoryNode,
    MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryRepositoryError, MemoryStatus,
};
use a3s_memory::{MemoryItem, MemoryType};
use chrono::{DateTime, Utc};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Runtime behavior enabled for one durable-memory binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DurableMemoryMode {
    /// Mirror successful V1 extractions as evidence-backed V2 candidates.
    ShadowCandidates,
}

/// Exact repository and namespace supplied by the embedding host.
#[derive(Clone)]
pub struct DurableMemorySession {
    repository: Arc<dyn MemoryRepository>,
    namespace: MemoryNamespace,
    mode: DurableMemoryMode,
}

impl DurableMemorySession {
    /// Create a candidate-only binding. V2 recall remains disabled.
    pub fn shadow(repository: Arc<dyn MemoryRepository>, namespace: MemoryNamespace) -> Self {
        Self {
            repository,
            namespace,
            mode: DurableMemoryMode::ShadowCandidates,
        }
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

impl std::fmt::Debug for DurableMemorySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableMemorySession")
            .field("namespace", &self.namespace)
            .field("mode", &self.mode)
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
mod tests {
    use super::*;
    use a3s_memory::repository::{InMemoryRepository, MemoryQuery};

    #[tokio::test]
    async fn shadow_write_is_evidence_backed_candidate_and_never_active() {
        let repository = Arc::new(InMemoryRepository::new());
        let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
        let binding = DurableMemorySession::shadow(repository.clone(), namespace.clone());
        let occurred_at = DateTime::from_timestamp_millis(1_777_000_000_000).unwrap();
        let evidence = DurableTurnEvidence::try_new(
            "session/one",
            "turn one",
            "remember this",
            "done",
            "user: remember this",
            occurred_at,
        )
        .unwrap();
        let item = MemoryItem::new("The repository requires focused crate tests")
            .with_type(MemoryType::Procedural)
            .with_importance(0.9)
            .with_metadata("confidence", "0.88")
            .with_metadata("source", "workflow")
            .with_metadata("scope", "workspace")
            .with_metadata("reason", "This prevents invalid root workspace builds")
            .with_metadata("schema", "a3s.memory.durable.v1");

        let node = binding
            .store_shadow_candidate(&item, &evidence)
            .await
            .unwrap();
        assert_eq!(node.status, MemoryStatus::Candidate);
        assert_eq!(node.evidence.len(), 1);
        assert!(node.evidence[0].uri.contains("session%2Fone"));
        assert!(!node.evidence[0].uri.contains("remember this"));
        assert_eq!(node.confidence, 0.88);
        assert!(repository
            .query(MemoryQuery::new(namespace.clone()))
            .await
            .unwrap()
            .hits
            .is_empty());
        assert_eq!(
            repository
                .query(
                    MemoryQuery::new(namespace.clone())
                        .with_statuses([MemoryStatus::Candidate])
                        .with_text("focused crate"),
                )
                .await
                .unwrap()
                .hits
                .len(),
            1
        );

        let replay = binding
            .store_shadow_candidate(&item, &evidence)
            .await
            .unwrap();
        assert_eq!(replay, node);
        assert_eq!(
            repository
                .query(
                    MemoryQuery::new(namespace)
                        .with_statuses([MemoryStatus::Candidate])
                        .with_text("focused crate"),
                )
                .await
                .unwrap()
                .hits
                .len(),
            1
        );
    }
}
