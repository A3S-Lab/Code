//! Host execution handle for one exact projected A3S Flow generation.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use a3s_flow::{FlowEventEnvelope, WorkflowRunSnapshot, WorkflowSpec};

use super::AgentSession;
use crate::capability::{
    CapabilityRuntimeError, CapabilityValue, FlowBinding, ScopeCloseReport, SessionCapabilityRun,
    UseCapabilityGeneration,
};
use crate::error::Result;

/// Non-clone host handle that retains one exact Code projection and A3S Use lease.
///
/// Every operation uses the [`FlowBinding`] selected at admission. Publishing a
/// newer catalog generation cannot replace this handle's workflow definition,
/// engine, event store, or runtime-build compatibility boundary. Call
/// [`close`](Self::close) when the host has finished its current execution or
/// inspection window.
#[must_use = "a projected Flow handle must remain alive for the complete host operation"]
pub struct ProjectedFlowHandle {
    binding: Arc<FlowBinding>,
    capability_run: SessionCapabilityRun,
}

impl ProjectedFlowHandle {
    /// Stable public name selected from the admitted capability generation.
    pub fn public_name(&self) -> &str {
        self.binding.public_name()
    }

    /// Exact durable workflow definition frozen into this handle.
    pub fn spec(&self) -> &WorkflowSpec {
        self.binding.spec()
    }

    /// Upstream A3S Use generation retained by this handle, when applicable.
    pub fn use_generation(&self) -> Option<&UseCapabilityGeneration> {
        self.capability_run.run_scope().use_generation()
    }

    /// Start a workflow with a generated durable run id and drive it to a
    /// terminal or suspended state.
    pub async fn start(&self, input: serde_json::Value) -> Result<String> {
        self.start_with_id(uuid::Uuid::new_v4().to_string(), input)
            .await
    }

    /// Start or idempotently replay a workflow using a caller-owned durable id.
    pub async fn start_with_id(
        &self,
        run_id: impl Into<String>,
        input: serde_json::Value,
    ) -> Result<String> {
        let run_id = run_id.into();
        let spec = self.binding.spec().clone();
        self.await_flow(self.binding.engine().start_with_id(run_id, spec, input))
            .await
    }

    /// Resume replay of an existing durable workflow run.
    pub async fn drive(&self, run_id: &str) -> Result<WorkflowRunSnapshot> {
        self.await_flow(self.binding.engine().drive(run_id)).await
    }

    /// Read the current event-sourced workflow snapshot.
    pub async fn snapshot(&self, run_id: &str) -> Result<WorkflowRunSnapshot> {
        self.await_flow(self.binding.engine().snapshot(run_id))
            .await
    }

    /// Read the durable event history for a workflow run.
    pub async fn history(&self, run_id: &str) -> Result<Vec<FlowEventEnvelope>> {
        self.await_flow(self.binding.engine().history(run_id)).await
    }

    /// Request durable cancellation of a workflow run.
    pub async fn cancel(&self, run_id: &str, reason: Option<String>) -> Result<()> {
        self.await_flow(self.binding.engine().cancel(run_id, reason))
            .await
    }

    /// Close the exact Code/Use capability scope retained by this handle.
    pub async fn close(self) -> Result<ScopeCloseReport> {
        self.capability_run.close().await.map_err(Into::into)
    }

    async fn await_flow<T>(&self, future: impl Future<Output = a3s_flow::Result<T>>) -> Result<T> {
        let cancellation = self.capability_run.run_scope().cancellation();
        tokio::pin!(future);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(CapabilityRuntimeError::Cancelled.into()),
            result = &mut future => result.map_err(Into::into),
        }
    }
}

impl fmt::Debug for ProjectedFlowHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectedFlowHandle")
            .field("public_name", &self.public_name())
            .field("version", &self.spec().version)
            .field("use_generation", &self.use_generation())
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    /// Admit and return one exact named Flow from the current atomic catalog.
    ///
    /// Missing names return `None` without acquiring an A3S Use lease. A found
    /// binding pins its Code generation before asynchronous Use admission, so a
    /// concurrent N+1 publication cannot alter the returned engine or spec.
    pub async fn projected_flow(&self, public_name: &str) -> Result<Option<ProjectedFlowHandle>> {
        let Some(admitted) = self
            .admit_projected_host_capability(
                crate::capability::CapabilityKind::Flow,
                public_name,
                |value| match value {
                    CapabilityValue::Flow(binding) => Some(Arc::clone(binding)),
                    _ => None,
                },
            )
            .await?
        else {
            return Ok(None);
        };
        debug_assert_eq!(admitted.value.public_name(), public_name);

        Ok(Some(ProjectedFlowHandle {
            binding: admitted.value,
            capability_run: admitted.capability_run,
        }))
    }
}
