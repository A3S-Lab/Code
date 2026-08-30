use super::{DurableMemorySemanticRefreshReceipt, DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1};
use crate::durable_memory::semantic_binding::{
    invalid, DurableMemorySemanticError, DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
};
use a3s_memory::repository::{
    MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_NODES, MEMORY_NAMESPACE_SNAPSHOT_PROFILE_V1,
};
use a3s_memory::vector::{VectorIndexChangeToken, VectorIndexStatus, VectorMutationConsistency};
use serde::{Deserialize, Serialize};

/// Stable schema of a persisted semantic-refresh recovery checkpoint.
pub const DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1: &str =
    "a3s.code.memory.semantic-refresh-checkpoint.v1";

/// Persistable, secret-free evidence used to recover a semantic refresh owner.
///
/// A checkpoint deliberately excludes the repository change token because that
/// token is comparable only inside one repository history. The first scheduled
/// run after recovery must verify a complete bounded Active snapshot and the
/// current index status before it can adopt this evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableMemorySemanticRefreshCheckpoint {
    schema: String,
    refresh_profile: String,
    source_snapshot_profile: String,
    source_snapshot_digest: String,
    source_snapshot_bytes: usize,
    semantic_binding_schema: String,
    serving_generation_digest: String,
    active_node_count: usize,
    mutation_consistency: VectorMutationConsistency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index_change_token: Option<VectorIndexChangeToken>,
    index_status: VectorIndexStatus,
}

impl DurableMemorySemanticRefreshCheckpoint {
    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn refresh_profile(&self) -> &str {
        &self.refresh_profile
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

    pub fn index_change_token(&self) -> Option<&VectorIndexChangeToken> {
        self.index_change_token.as_ref()
    }

    pub fn index_status(&self) -> &VectorIndexStatus {
        &self.index_status
    }

    /// Validate a deserialized checkpoint before installing it in a schedule.
    pub fn verify(&self) -> Result<(), DurableMemorySemanticError> {
        if self.schema != DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1 {
            return Err(invalid("checkpoint.schema", "is unsupported"));
        }
        if self.refresh_profile != DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1 {
            return Err(invalid("checkpoint.refreshProfile", "is unsupported"));
        }
        if self.source_snapshot_profile != MEMORY_NAMESPACE_SNAPSHOT_PROFILE_V1 {
            return Err(invalid(
                "checkpoint.sourceSnapshotProfile",
                "is unsupported",
            ));
        }
        if !valid_prefixed_sha256(&self.source_snapshot_digest) {
            return Err(invalid(
                "checkpoint.sourceSnapshotDigest",
                "must be canonical lowercase SHA-256",
            ));
        }
        if !(1..=MAX_SNAPSHOT_BYTES).contains(&self.source_snapshot_bytes) {
            return Err(invalid(
                "checkpoint.sourceSnapshotBytes",
                format!("must be between 1 and {MAX_SNAPSHOT_BYTES}"),
            ));
        }
        if self.semantic_binding_schema != DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1 {
            return Err(invalid(
                "checkpoint.semanticBindingSchema",
                "is unsupported",
            ));
        }
        if !valid_prefixed_sha256(&self.serving_generation_digest) {
            return Err(invalid(
                "checkpoint.servingGenerationDigest",
                "must be canonical lowercase SHA-256",
            ));
        }
        if self.active_node_count > MAX_SNAPSHOT_NODES {
            return Err(invalid(
                "checkpoint.activeNodeCount",
                format!("must not exceed {MAX_SNAPSHOT_NODES}"),
            ));
        }
        if self.mutation_consistency != VectorMutationConsistency::IndexRevisionCas {
            return Err(invalid(
                "checkpoint.mutationConsistency",
                "must be index_revision_cas",
            ));
        }
        if let Some(token) = self.index_change_token.as_ref() {
            token.verify()?;
            if token.revision() != self.index_status.revision {
                return Err(invalid(
                    "checkpoint.indexChangeToken.revision",
                    "must match indexStatus.revision",
                ));
            }
        }
        if self.active_node_count > self.index_status.record_count {
            return Err(invalid(
                "checkpoint.indexStatus.recordCount",
                "must account for every Active node",
            ));
        }
        if self.active_node_count > 0 && self.index_status.partition_count == 0 {
            return Err(invalid(
                "checkpoint.indexStatus.partitionCount",
                "must account for the Active semantic partition",
            ));
        }
        Ok(())
    }

    pub(super) fn from_receipt(receipt: &DurableMemorySemanticRefreshReceipt) -> Self {
        Self {
            schema: DURABLE_MEMORY_SEMANTIC_REFRESH_CHECKPOINT_SCHEMA_V1.to_string(),
            refresh_profile: receipt.profile.clone(),
            source_snapshot_profile: receipt.source_snapshot_profile.clone(),
            source_snapshot_digest: receipt.source_snapshot_digest.clone(),
            source_snapshot_bytes: receipt.source_snapshot_bytes,
            semantic_binding_schema: receipt.semantic_binding_schema.clone(),
            serving_generation_digest: receipt.serving_generation_digest.clone(),
            active_node_count: receipt.active_node_count,
            mutation_consistency: receipt.mutation_consistency,
            index_change_token: receipt.index_change_token.clone(),
            index_status: receipt.index_status.clone(),
        }
    }

    pub(crate) fn into_recovery_receipt(self) -> DurableMemorySemanticRefreshReceipt {
        DurableMemorySemanticRefreshReceipt {
            profile: self.refresh_profile,
            source_snapshot_profile: self.source_snapshot_profile,
            source_snapshot_digest: self.source_snapshot_digest,
            source_snapshot_bytes: self.source_snapshot_bytes,
            source_change_token: None,
            semantic_binding_schema: self.semantic_binding_schema,
            serving_generation_digest: self.serving_generation_digest,
            active_node_count: self.active_node_count,
            mutation_consistency: self.mutation_consistency,
            index_change_token: self.index_change_token,
            index_status: self.index_status,
        }
    }
}

fn valid_prefixed_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(valid_unprefixed_sha256)
}

fn valid_unprefixed_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
