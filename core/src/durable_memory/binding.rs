use super::{
    invalid, DurableMemoryMode, DurableMemoryRecallPolicy, DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1,
    DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
};
use a3s_memory::repository::{
    MemoryNamespace, MemoryRepositoryError, MEMORY_LEXICAL_QUERY_PROFILE_V1,
};
use serde::{Deserialize, Serialize};

/// Schema version for the secret-free durable-memory binding persisted with a
/// session snapshot.
pub const DURABLE_MEMORY_BINDING_SCHEMA_VERSION: u32 = 4;

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
            context_id_profile: DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2.to_string(),
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
        let (expected_retrieval_profile, expected_context_id_profile) = match self.schema_version {
            DURABLE_MEMORY_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
            ),
            LEGACY_SESSION_RUN_CONTEXT_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1,
            ),
            LEGACY_RETRIEVAL_BINDING_SCHEMA_VERSION => (
                DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
                LEGACY_HOST_CONTEXT_ID_PROFILE_V0,
            ),
            LEGACY_BASE_BINDING_SCHEMA_VERSION => (
                LEGACY_WORD_RETRIEVAL_PROFILE_V1,
                LEGACY_HOST_CONTEXT_ID_PROFILE_V0,
            ),
            _ => {
                return Err(invalid(
                    "durableMemoryBinding.schemaVersion",
                    format!(
                        "unsupported schema version {}; expected {}, legacy {}, legacy {}, or legacy {}",
                        self.schema_version,
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
