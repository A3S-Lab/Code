use super::checkpoint::DurableMemorySemanticRefreshCheckpoint;
use crate::durable_memory::{
    DurableMemorySemanticRecall, DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
};
use a3s_memory::repository::{MemoryNamespaceChangeToken, MemoryNamespaceSnapshot};
use a3s_memory::vector::{
    VectorIndexChangeToken, VectorIndexObservation, VectorIndexStatus, VectorMutationConsistency,
    VectorRevision,
};
use serde::Serialize;

pub(super) struct RefreshIndexObservation<'a> {
    pub(super) consistency: VectorMutationConsistency,
    pub(super) expected_revision: Option<VectorRevision>,
    pub(super) observation: &'a VectorIndexObservation,
    pub(super) require_history_continuity: bool,
}

/// Stable identity of the verified full-snapshot refresh algorithm.
pub const DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1: &str =
    "a3s.code.memory.semantic-refresh.full-snapshot.v1";

/// Secret-free evidence that one exact Active snapshot was published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableMemorySemanticRefreshReceipt {
    pub(super) profile: String,
    pub(super) source_snapshot_profile: String,
    pub(super) source_snapshot_digest: String,
    pub(super) source_snapshot_bytes: usize,
    pub(super) source_change_token: Option<MemoryNamespaceChangeToken>,
    pub(super) semantic_binding_schema: String,
    pub(super) serving_generation_digest: String,
    pub(super) active_node_count: usize,
    pub(super) mutation_consistency: VectorMutationConsistency,
    pub(super) index_change_token: Option<VectorIndexChangeToken>,
    pub(super) index_status: VectorIndexStatus,
}

impl DurableMemorySemanticRefreshReceipt {
    /// Create persistable recovery evidence without the repository-local token.
    pub fn checkpoint(&self) -> DurableMemorySemanticRefreshCheckpoint {
        DurableMemorySemanticRefreshCheckpoint::from_receipt(self)
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn source_snapshot_profile(&self) -> &str {
        &self.source_snapshot_profile
    }

    pub fn source_snapshot_digest(&self) -> &str {
        &self.source_snapshot_digest
    }

    pub fn source_snapshot_bytes(&self) -> usize {
        self.source_snapshot_bytes
    }

    /// Optional bounded change evidence scoped to one repository history and
    /// schedule-ownership epoch.
    pub fn source_change_token(&self) -> Option<&MemoryNamespaceChangeToken> {
        self.source_change_token.as_ref()
    }

    pub fn semantic_binding_schema(&self) -> &str {
        &self.semantic_binding_schema
    }

    pub fn serving_generation_digest(&self) -> &str {
        &self.serving_generation_digest
    }

    pub fn active_node_count(&self) -> usize {
        self.active_node_count
    }

    pub fn mutation_consistency(&self) -> VectorMutationConsistency {
        self.mutation_consistency
    }

    /// Optional exact continuity evidence for one vector-index history.
    pub fn index_change_token(&self) -> Option<&VectorIndexChangeToken> {
        self.index_change_token.as_ref()
    }

    pub fn index_status(&self) -> &VectorIndexStatus {
        &self.index_status
    }

    pub(super) fn matches_current(
        &self,
        semantic: &DurableMemorySemanticRecall,
        snapshot: &MemoryNamespaceSnapshot,
        index: RefreshIndexObservation<'_>,
    ) -> bool {
        self.profile == DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1
            && self.source_snapshot_profile == snapshot.profile()
            && self.source_snapshot_digest == snapshot.digest()
            && self.source_snapshot_bytes == snapshot.byte_count()
            && self.semantic_binding_schema == DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1
            && self.serving_generation_digest == semantic.serving_generation_digest()
            && self.active_node_count == snapshot.nodes().len()
            && self.mutation_consistency == index.consistency
            && index.expected_revision == Some(self.index_status.revision)
            && self.matches_index_continuity(
                index.observation.change_token.as_ref(),
                &index.observation.status,
                index.require_history_continuity,
            )
            && self.index_status == index.observation.status
    }

    pub(super) fn matches_current_change_token(
        &self,
        semantic: &DurableMemorySemanticRecall,
        token: &MemoryNamespaceChangeToken,
        index: RefreshIndexObservation<'_>,
    ) -> bool {
        self.profile == DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1
            && self.source_change_token.as_ref() == Some(token)
            && self.semantic_binding_schema == DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1
            && self.serving_generation_digest == semantic.serving_generation_digest()
            && self.mutation_consistency == index.consistency
            && index.expected_revision == Some(self.index_status.revision)
            && self.matches_index_continuity(
                index.observation.change_token.as_ref(),
                &index.observation.status,
                index.require_history_continuity,
            )
            && self.index_status == index.observation.status
    }

    fn matches_index_continuity(
        &self,
        current: Option<&VectorIndexChangeToken>,
        current_status: &VectorIndexStatus,
        required: bool,
    ) -> bool {
        match self.index_change_token.as_ref() {
            Some(expected) => {
                current == Some(expected)
                    && expected.revision() == self.index_status.revision
                    && current.is_some_and(|token| token.revision() == current_status.revision)
            }
            None => !required,
        }
    }

    pub(super) fn with_source_change_token(
        &self,
        source_change_token: Option<MemoryNamespaceChangeToken>,
    ) -> Self {
        let mut receipt = self.clone();
        receipt.source_change_token = source_change_token;
        receipt
    }
}
