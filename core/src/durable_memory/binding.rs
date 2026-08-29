use super::{
    invalid, DurableMemoryMode, DurableMemoryRecallPolicy, DurableMemorySemanticBindingV1,
    DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1, DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
};
use a3s_memory::repository::{
    MemoryNamespace, MemoryRepositoryError, MEMORY_LEXICAL_QUERY_PROFILE_V1,
};
use serde::{Deserialize, Serialize};

/// Schema version for the secret-free durable-memory binding persisted with a
/// session snapshot.
pub const DURABLE_MEMORY_BINDING_SCHEMA_VERSION: u32 = 4;

/// Schema version for sessions that bind an exact semantic serving generation.
pub const DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION: u32 = 5;

/// Current A3S Memory query algorithm bound into new durable-memory sessions.
pub const DURABLE_MEMORY_RETRIEVAL_PROFILE_V1: &str = MEMORY_LEXICAL_QUERY_PROFILE_V1;

const LEGACY_WORD_RETRIEVAL_PROFILE_V1: &str = "a3s.memory.lexical.word.v1";
const LEGACY_HOST_CONTEXT_ID_PROFILE_V0: &str = "a3s.code.memory.context.host-id.v0";
const LEGACY_SESSION_RUN_CONTEXT_BINDING_SCHEMA_VERSION: u32 = 3;
const LEGACY_RETRIEVAL_BINDING_SCHEMA_VERSION: u32 = 2;
const LEGACY_BASE_BINDING_SCHEMA_VERSION: u32 = 1;

fn legacy_retrieval_profile() -> String {
    LEGACY_WORD_RETRIEVAL_PROFILE_V1.to_string()
}

fn legacy_context_id_profile() -> String {
    LEGACY_HOST_CONTEXT_ID_PROFILE_V0.to_string()
}

/// Exact Code-visible durable-memory authority and serving policy.
///
/// The live repository is intentionally absent. Hosts own repository
/// construction and must re-inject it after restart, while Code persists this
/// descriptor to reject namespace, mode, recall-policy, query-semantics, or
/// admission-identity drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableMemoryBindingV1 {
    schema_version: u32,
    namespace: MemoryNamespace,
    mode: DurableMemoryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recall_policy: Option<DurableMemoryRecallPolicy>,
    #[serde(default = "legacy_retrieval_profile")]
    retrieval_profile: String,
    #[serde(default = "legacy_context_id_profile")]
    context_id_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    semantic_recall: Option<DurableMemorySemanticBindingV1>,
}

impl DurableMemoryBindingV1 {
    pub(super) fn new(
        namespace: MemoryNamespace,
        mode: DurableMemoryMode,
        recall_policy: Option<DurableMemoryRecallPolicy>,
        semantic_recall: Option<DurableMemorySemanticBindingV1>,
    ) -> Self {
        let schema_version = if semantic_recall.is_some() {
            DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION
        } else {
            DURABLE_MEMORY_BINDING_SCHEMA_VERSION
        };
        Self {
            schema_version,
            namespace,
            mode,
            recall_policy,
            retrieval_profile: DURABLE_MEMORY_RETRIEVAL_PROFILE_V1.to_string(),
            context_id_profile: DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2.to_string(),
            semantic_recall,
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
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

    pub fn retrieval_profile(&self) -> &str {
        &self.retrieval_profile
    }

    pub fn context_id_profile(&self) -> &str {
        &self.context_id_profile
    }

    pub fn semantic_recall(&self) -> Option<&DurableMemorySemanticBindingV1> {
        self.semantic_recall.as_ref()
    }

    pub(crate) fn validate(&self) -> Result<(), MemoryRepositoryError> {
        if self.retrieval_profile != DURABLE_MEMORY_RETRIEVAL_PROFILE_V1
            && self.retrieval_profile != LEGACY_WORD_RETRIEVAL_PROFILE_V1
        {
            return Err(invalid(
                "durableMemoryBinding.retrievalProfile",
                format!(
                    "unrecognized durable-memory retrieval profile `{}`",
                    self.retrieval_profile
                ),
            ));
        }
        if self.context_id_profile != DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2
            && self.context_id_profile != DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1
            && self.context_id_profile != LEGACY_HOST_CONTEXT_ID_PROFILE_V0
        {
            return Err(invalid(
                "durableMemoryBinding.contextIdProfile",
                format!(
                    "unrecognized durable-memory context identity profile `{}`",
                    self.context_id_profile
                ),
            ));
        }
        let (expected_retrieval_profile, expected_context_id_profile, expects_semantic) = match self
            .schema_version
        {
            DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
                true,
            ),
            DURABLE_MEMORY_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
                false,
            ),
            LEGACY_SESSION_RUN_CONTEXT_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1,
                false,
            ),
            LEGACY_RETRIEVAL_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                LEGACY_HOST_CONTEXT_ID_PROFILE_V0,
                false,
            ),
            LEGACY_BASE_BINDING_SCHEMA_VERSION => (
                LEGACY_WORD_RETRIEVAL_PROFILE_V1,
                LEGACY_HOST_CONTEXT_ID_PROFILE_V0,
                false,
            ),
            _ => {
                return Err(invalid(
                    "durableMemoryBinding.schemaVersion",
                    format!(
                        "unsupported schema version {}; expected {} or {}, legacy {}, legacy {}, or legacy {}",
                        self.schema_version,
                        DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION,
                        DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
                        LEGACY_SESSION_RUN_CONTEXT_BINDING_SCHEMA_VERSION,
                        LEGACY_RETRIEVAL_BINDING_SCHEMA_VERSION,
                        LEGACY_BASE_BINDING_SCHEMA_VERSION
                    ),
                ));
            }
        };
        if self.retrieval_profile != expected_retrieval_profile {
            return Err(invalid(
                "durableMemoryBinding.retrievalProfile",
                format!(
                    "retrieval profile `{}` is incompatible with schema version {}",
                    self.retrieval_profile, self.schema_version
                ),
            ));
        }
        if self.context_id_profile != expected_context_id_profile {
            return Err(invalid(
                "durableMemoryBinding.contextIdProfile",
                format!(
                    "context identity profile `{}` is incompatible with schema version {}",
                    self.context_id_profile, self.schema_version
                ),
            ));
        }
        MemoryNamespace::try_new(
            self.namespace.tenant_id(),
            self.namespace.principal_id(),
            self.namespace.scope_id(),
        )?;
        match (expects_semantic, self.semantic_recall.as_ref()) {
            (true, Some(semantic)) => {
                semantic.validate().map_err(|error| {
                    invalid("durableMemoryBinding.semanticRecall", error.to_string())
                })?;
                if self.mode != DurableMemoryMode::ActiveRecall {
                    return Err(invalid(
                        "durableMemoryBinding.mode",
                        "semantic recall requires active recall mode",
                    ));
                }
            }
            (true, None) => {
                return Err(invalid(
                    "durableMemoryBinding.semanticRecall",
                    "hybrid schema requires an exact semantic recall binding",
                ));
            }
            (false, Some(_)) => {
                return Err(invalid(
                    "durableMemoryBinding.semanticRecall",
                    "semantic recall is incompatible with this schema version",
                ));
            }
            (false, None) => {}
        }
        match (self.mode, self.recall_policy) {
            (DurableMemoryMode::ShadowCandidates, None) => Ok(()),
            (DurableMemoryMode::ActiveRecall, Some(policy)) => {
                DurableMemoryRecallPolicy::try_new(
                    policy.max_results(),
                    policy.min_lexical_score(),
                )?
                .try_with_related_lookups(policy.max_related_lookups())?;
                Ok(())
            }
            (DurableMemoryMode::ShadowCandidates, Some(_)) => Err(invalid(
                "durableMemoryBinding.recallPolicy",
                "shadow candidate mode must not carry a recall policy",
            )),
            (DurableMemoryMode::ActiveRecall, None) => Err(invalid(
                "durableMemoryBinding.recallPolicy",
                "active recall mode requires a recall policy",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_memory::{
        DurableMemorySemanticRecall, DurableMemorySemanticRecallPolicy, DurableMemorySession,
    };
    use crate::embedding::{
        EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig,
        EmbeddingNormalization, EmbeddingProvider, EmbeddingProviderDescriptor,
        EmbeddingProviderError,
    };
    use a3s_memory::repository::InMemoryRepository;
    use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct DescriptorOnlyProvider;

    #[async_trait::async_trait]
    impl EmbeddingProvider for DescriptorOnlyProvider {
        fn descriptor(&self) -> EmbeddingProviderDescriptor {
            EmbeddingProviderDescriptor::new("fixture", "semantic-binding", 2)
                .with_revision("fixture-r1")
                .with_normalization(EmbeddingNormalization::Unit)
        }

        async fn embed(
            &self,
            _request: EmbeddingBatchRequest,
            _cancellation: CancellationToken,
        ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
            Err(EmbeddingProviderError::InvalidRequest)
        }
    }

    fn hybrid_binding() -> DurableMemoryBindingV1 {
        let namespace = MemoryNamespace::try_new("tenant", "principal", "semantic").unwrap();
        let repository = Arc::new(InMemoryRepository::new());
        let index: Arc<dyn VectorIndex> =
            Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
        let semantic = DurableMemorySemanticRecall::new(
            format!("sha256:{}", "a".repeat(64)),
            Arc::new(DescriptorOnlyProvider),
            EmbeddingExecutorConfig::default(),
            index,
            DurableMemorySemanticRecallPolicy::try_new(8, 0.7).unwrap(),
        )
        .unwrap();
        DurableMemorySession::active_recall(
            repository,
            namespace,
            DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
        )
        .with_semantic_recall(semantic)
        .unwrap()
        .binding()
    }

    #[test]
    fn hybrid_schema_requires_a_valid_semantic_binding_and_active_mode() {
        let binding = hybrid_binding();
        assert_eq!(
            binding.schema_version(),
            DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION
        );
        binding.validate().unwrap();

        let encoded = serde_json::to_value(&binding).unwrap();
        let mut missing = encoded.clone();
        missing.as_object_mut().unwrap().remove("semanticRecall");
        let missing: DurableMemoryBindingV1 = serde_json::from_value(missing).unwrap();
        assert!(missing.validate().is_err());

        let mut legacy_with_semantic = encoded.clone();
        legacy_with_semantic["schemaVersion"] =
            serde_json::json!(DURABLE_MEMORY_BINDING_SCHEMA_VERSION);
        let legacy_with_semantic: DurableMemoryBindingV1 =
            serde_json::from_value(legacy_with_semantic).unwrap();
        assert!(legacy_with_semantic.validate().is_err());

        let mut shadow = encoded.clone();
        shadow["mode"] = serde_json::json!("shadow_candidates");
        let shadow: DurableMemoryBindingV1 = serde_json::from_value(shadow).unwrap();
        assert!(shadow.validate().is_err());

        let mut unsupported_fusion = encoded;
        unsupported_fusion["semanticRecall"]["fusionProfile"] =
            serde_json::json!("a3s.code.memory.hybrid.unknown.v1");
        let unsupported_fusion: DurableMemoryBindingV1 =
            serde_json::from_value(unsupported_fusion).unwrap();
        assert!(unsupported_fusion.validate().is_err());

        let mut unpinned_embedding = serde_json::to_value(&binding).unwrap();
        unpinned_embedding["semanticRecall"]["embedding"]["revision"] = serde_json::Value::Null;
        let unpinned_embedding: DurableMemoryBindingV1 =
            serde_json::from_value(unpinned_embedding).unwrap();
        assert!(unpinned_embedding.validate().is_err());

        let mut mismatched_dimension = serde_json::to_value(&binding).unwrap();
        mismatched_dimension["semanticRecall"]["vectorIndex"]["dimension"] = serde_json::json!(3);
        let mismatched_dimension: DurableMemoryBindingV1 =
            serde_json::from_value(mismatched_dimension).unwrap();
        assert!(mismatched_dimension.validate().is_err());
    }

    #[test]
    fn semantic_generation_identity_round_trips_and_detects_drift() {
        let binding = hybrid_binding();
        let encoded = serde_json::to_value(&binding).unwrap();
        let round_trip: DurableMemoryBindingV1 = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(round_trip, binding);

        let mut drifted = encoded;
        drifted["semanticRecall"]["embedding"]["model"] = serde_json::json!("semantic-binding-v2");
        let drifted: DurableMemoryBindingV1 = serde_json::from_value(drifted).unwrap();
        drifted.validate().unwrap();
        assert_ne!(drifted, binding);
    }
}
