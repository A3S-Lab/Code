use super::{invalid, DurableMemoryMode, DurableMemoryRecallPolicy};
use a3s_memory::repository::{MemoryNamespace, MemoryRepositoryError};
use serde::{Deserialize, Serialize};

/// Schema version for the secret-free durable-memory binding persisted with a
/// session snapshot.
pub const DURABLE_MEMORY_BINDING_SCHEMA_VERSION: u32 = 1;

/// Exact Code-visible durable-memory authority and serving policy.
///
/// The live repository is intentionally absent. Hosts own repository
/// construction and must re-inject it after restart, while Code persists this
/// descriptor to reject namespace, mode, or recall-policy drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableMemoryBindingV1 {
    schema_version: u32,
    namespace: MemoryNamespace,
    mode: DurableMemoryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recall_policy: Option<DurableMemoryRecallPolicy>,
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

    pub(crate) fn validate(&self) -> Result<(), MemoryRepositoryError> {
        if self.schema_version != DURABLE_MEMORY_BINDING_SCHEMA_VERSION {
            return Err(invalid(
                "durableMemoryBinding.schemaVersion",
                format!(
                    "unsupported schema version {}; expected {}",
                    self.schema_version, DURABLE_MEMORY_BINDING_SCHEMA_VERSION
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
