use std::fmt;

use a3s_flow::{FlowEngine, FlowError, WorkflowSpec};

/// Immutable A3S Flow definition paired with the exact engine that can replay it.
///
/// [`WorkflowSpec::name`] is the public capability name and remains the single
/// source of truth for lookup and descriptor validation. The engine retains its
/// own event store, runtime, observer, and runtime-build compatibility policy;
/// Code does not duplicate those A3S Flow lifecycles.
#[derive(Clone)]
pub struct FlowBinding {
    spec: WorkflowSpec,
    engine: FlowEngine,
}

impl FlowBinding {
    /// Validate and bind one immutable workflow definition to its executor.
    pub fn new(spec: WorkflowSpec, engine: FlowEngine) -> a3s_flow::Result<Self> {
        spec.validate()?;
        if !engine.supports_runtime_build(spec.runtime_build_id.as_ref()) {
            let required = spec
                .runtime_build_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "<unpinned>".to_owned());
            let current = engine
                .runtime_build_compatibility()
                .map(|compatibility| compatibility.current_build_id().to_string())
                .unwrap_or_else(|| "<unfenced>".to_owned());
            return Err(FlowError::InvalidWorkflow(format!(
                "workflow '{}' requires runtime build {required}, but its engine exposes {current}",
                spec.name
            )));
        }
        Ok(Self { spec, engine })
    }

    /// Stable public name used by the Code capability descriptor and host lookup.
    pub fn public_name(&self) -> &str {
        &self.spec.name
    }

    /// Exact durable definition used for every run started through this binding.
    pub fn spec(&self) -> &WorkflowSpec {
        &self.spec
    }

    pub(crate) fn engine(&self) -> &FlowEngine {
        &self.engine
    }
}

impl fmt::Debug for FlowBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlowBinding")
            .field("public_name", &self.public_name())
            .field("version", &self.spec.version)
            .field("runtime", &self.spec.runtime)
            .field("runtime_build_id", &self.spec.runtime_build_id)
            .finish_non_exhaustive()
    }
}
