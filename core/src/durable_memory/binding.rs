use super::{invalid, DurableMemoryMode, DurableMemoryRecallPolicy};
use a3s_memory::repository::{
    MemoryNamespace, MemoryRepositoryError, MEMORY_LEXICAL_QUERY_PROFILE_V1,
};
use serde::{Deserialize, Serialize};

/// Schema version for the secret-free durable-memory binding persisted with a
/// session snapshot.
pub const DURABLE_MEMORY_BINDING_SCHEMA_VERSION: u32 = 2;

/// Current A3S Memory query algorithm bound into new durable-memory sessions.
pub const DURABLE_MEMORY_RETRIEVAL_PROFILE_V1: &str = MEMORY_LEXICAL_QUERY_PROFILE_V1;

const LEGACY_WORD_RETRIEVAL_PROFILE_V1: &str = "a3s.memory.lexical.word.v1";
const LEGACY_DURABLE_MEMORY_BINDING_SCHEMA_VERSION: u32 = 1;

fn legacy_retrieval_profile() -> String {
    LEGACY_WORD_RETRIEVAL_PROFILE_V1.to_string()
}

/// Exact Code-visible durable-memory authority and serving policy.
///
/// The live repository is intentionally absent. Hosts own repository
/// construction and must re-inject it after restart, while Code persists this
/// descriptor to reject namespace, mode, recall-policy, or query-semantics
/// drift.
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
}

impl DurableMemoryBindingV1 {
    pub(super) fn new(
        namespace: MemoryNamespace,
        mode: DurableMemoryMode,
        recall_policy: Option<DurableMemoryRecallPolicy>,
    ) -> Self {
        Self {
            schema_version: DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
            namespace,
            mode,
            recall_policy,
            retrieval_profile: DURABLE_MEMORY_RETRIEVAL_PROFILE_V1.to_string(),
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

    pub(crate) fn validate(&self) -> Result<(), MemoryRepositoryError> {
        if self.schema_version != DURABLE_MEMORY_BINDING_SCHEMA_VERSION
            && self.schema_version != LEGACY_DURABLE_MEMORY_BINDING_SCHEMA_VERSION
        {
            return Err(invalid(
                "durableMemoryBinding.schemaVersion",
                format!(
                    "unsupported schema version {}; expected {} or legacy {}",
                    self.schema_version,
                    DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
                    LEGACY_DURABLE_MEMORY_BINDING_SCHEMA_VERSION
                ),
            ));
        }
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
        let current_pair = self.schema_version == DURABLE_MEMORY_BINDING_SCHEMA_VERSION
            && self.retrieval_profile == DURABLE_MEMORY_RETRIEVAL_PROFILE_V1;
        let legacy_pair = self.schema_version == LEGACY_DURABLE_MEMORY_BINDING_SCHEMA_VERSION
            && self.retrieval_profile == LEGACY_WORD_RETRIEVAL_PROFILE_V1;
        if !current_pair && !legacy_pair {
            return Err(invalid(
                "durableMemoryBinding.retrievalProfile",
                format!(
                    "retrieval profile `{}` is incompatible with schema version {}",
                    self.retrieval_profile, self.schema_version
                ),
            ));
        }
        MemoryNamespace::try_new(
            self.namespace.tenant_id(),
            self.namespace.principal_id(),
            self.namespace.scope_id(),
        )?;
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
