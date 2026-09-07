//! Secret-free observability for the run-bound model middleware (OPT-OBS1).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Read-only counters for one model-middleware observation window.
///
/// Values never retain prompts, tool plaintext, credentials, or digests of
/// private content—only stage outcomes and trust-label cardinality.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelMiddlewareHealthSnapshot {
    /// Calls that passed trust admission (completion + streaming).
    pub trust_admitted: u64,
    /// Calls rejected at trust admission before budget/provider work.
    pub trust_rejected: u64,
    /// Provider invocations started after generation admission.
    pub provider_calls: u64,
    /// Successful usage-accounting completions after a provider response.
    pub usage_recorded: u64,
    /// Non-streaming completion calls that passed trust admission.
    pub completion_calls: u64,
    /// Streaming calls that passed trust admission.
    pub streaming_calls: u64,
    /// Tool-result blocks labeled Trusted observed at admission.
    pub trusted_tool_results: u64,
    /// Tool-result blocks labeled WorkspaceData observed at admission.
    pub workspace_data_tool_results: u64,
    /// Tool-result blocks labeled External observed at admission.
    pub external_tool_results: u64,
    /// External tool-result blocks that carried redaction_reviewed.
    pub external_redaction_reviewed: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ModelMiddlewareObs {
    trust_admitted: AtomicU64,
    trust_rejected: AtomicU64,
    provider_calls: AtomicU64,
    usage_recorded: AtomicU64,
    completion_calls: AtomicU64,
    streaming_calls: AtomicU64,
    trusted_tool_results: AtomicU64,
    workspace_data_tool_results: AtomicU64,
    external_tool_results: AtomicU64,
    external_redaction_reviewed: AtomicU64,
}

impl ModelMiddlewareObs {
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn snapshot(&self) -> ModelMiddlewareHealthSnapshot {
        ModelMiddlewareHealthSnapshot {
            trust_admitted: self.trust_admitted.load(Ordering::Relaxed),
            trust_rejected: self.trust_rejected.load(Ordering::Relaxed),
            provider_calls: self.provider_calls.load(Ordering::Relaxed),
            usage_recorded: self.usage_recorded.load(Ordering::Relaxed),
            completion_calls: self.completion_calls.load(Ordering::Relaxed),
            streaming_calls: self.streaming_calls.load(Ordering::Relaxed),
            trusted_tool_results: self.trusted_tool_results.load(Ordering::Relaxed),
            workspace_data_tool_results: self.workspace_data_tool_results.load(Ordering::Relaxed),
            external_tool_results: self.external_tool_results.load(Ordering::Relaxed),
            external_redaction_reviewed: self.external_redaction_reviewed.load(Ordering::Relaxed),
        }
    }

    fn bump(counter: &AtomicU64, by: u64) {
        if by == 0 {
            return;
        }
        counter.fetch_add(by, Ordering::Relaxed);
    }

    pub(crate) fn record_trust_admitted(
        &self,
        streaming: bool,
        trusted: u32,
        workspace_data: u32,
        external: u32,
        external_reviewed: u32,
    ) {
        self.trust_admitted.fetch_add(1, Ordering::Relaxed);
        if streaming {
            self.streaming_calls.fetch_add(1, Ordering::Relaxed);
        } else {
            self.completion_calls.fetch_add(1, Ordering::Relaxed);
        }
        Self::bump(&self.trusted_tool_results, u64::from(trusted));
        Self::bump(&self.workspace_data_tool_results, u64::from(workspace_data));
        Self::bump(&self.external_tool_results, u64::from(external));
        Self::bump(
            &self.external_redaction_reviewed,
            u64::from(external_reviewed),
        );
    }

    pub(crate) fn record_trust_rejected(&self) {
        self.trust_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_provider_call(&self) {
        self.provider_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_usage(&self) {
        self.usage_recorded.fetch_add(1, Ordering::Relaxed);
    }
}
