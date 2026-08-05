use super::parse_lane;
use a3s_code_core::hitl::{
    ConfirmationPolicy as RustConfirmationPolicy, TimeoutAction as RustTimeoutAction,
};
use a3s_code_core::permissions::{
    PermissionDecision as RustPermissionDecision, PermissionPolicy as RustPermissionPolicy,
    PermissionRule as RustPermissionRule,
};
use a3s_code_core::subagent::{
    AgentDefinition as RustAgentDefinition, ModelConfig as RustAgentModelConfig,
    WorkerAgentKind as RustWorkerAgentKind, WorkerAgentSpec as RustWorkerAgentSpec,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

// ============================================================================
// Policy and delegation configuration
// ============================================================================

/// Host-provided deterministic ID and clock configuration.
///
/// Set both fields for replay so IDs and timestamps are reproducible across
/// hosts. Omitted fields keep their system-backed default.
#[pyclass(name = "HostEnvConfig")]
#[derive(Clone)]
pub(super) struct PyHostEnvConfig {
    /// Prefix for deterministic IDs (``<prefix>-0``, ``<prefix>-1``, ...).
    #[pyo3(get, set)]
    pub(super) sequential_id_prefix: Option<String>,
    /// Fixed Unix-epoch timestamp returned by the session clock.
    #[pyo3(get, set)]
    pub(super) fixed_time_ms: Option<u64>,
}

#[pymethods]
impl PyHostEnvConfig {
    #[new]
    #[pyo3(signature = (sequential_id_prefix=None, fixed_time_ms=None))]
    fn new(sequential_id_prefix: Option<String>, fixed_time_ms: Option<u64>) -> Self {
        Self {
            sequential_id_prefix,
            fixed_time_ms,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "HostEnvConfig(sequential_id_prefix={:?}, fixed_time_ms={:?})",
            self.sequential_id_prefix, self.fixed_time_ms
        )
    }
}

/// Explicit allow/deny/ask tool permission policy.
#[pyclass(name = "PermissionPolicy")]
#[derive(Clone)]
pub(super) struct PyPermissionPolicy {
    #[pyo3(get, set)]
    deny: Vec<String>,
    #[pyo3(get, set)]
    allow: Vec<String>,
    #[pyo3(get, set)]
    ask: Vec<String>,
    #[pyo3(get, set)]
    default_decision: String,
    #[pyo3(get, set)]
    enabled: bool,
}

#[pymethods]
impl PyPermissionPolicy {
    #[new]
    #[pyo3(signature = (allow=None, deny=None, ask=None, default_decision=None, enabled=true))]
    pub(super) fn new(
        allow: Option<Vec<String>>,
        deny: Option<Vec<String>>,
        ask: Option<Vec<String>>,
        default_decision: Option<String>,
        enabled: bool,
    ) -> Self {
        Self {
            deny: deny.unwrap_or_default(),
            allow: allow.unwrap_or_default(),
            ask: ask.unwrap_or_default(),
            default_decision: default_decision.unwrap_or_else(|| "ask".to_string()),
            enabled,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PermissionPolicy(allow={}, deny={}, ask={}, default_decision={:?}, enabled={})",
            self.allow.len(),
            self.deny.len(),
            self.ask.len(),
            self.default_decision,
            self.enabled
        )
    }
}

fn parse_py_permission_decision(value: &str) -> PyResult<RustPermissionDecision> {
    match value.trim().to_ascii_lowercase().as_str() {
        "allow" => Ok(RustPermissionDecision::Allow),
        "deny" => Ok(RustPermissionDecision::Deny),
        "ask" => Ok(RustPermissionDecision::Ask),
        other => Err(PyValueError::new_err(format!(
            "default_decision must be 'allow', 'deny', or 'ask', got {other:?}"
        ))),
    }
}

pub(super) fn py_permission_policy_to_rust(
    policy: PyPermissionPolicy,
) -> PyResult<RustPermissionPolicy> {
    Ok(RustPermissionPolicy {
        deny: policy
            .deny
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        allow: policy
            .allow
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        ask: policy
            .ask
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        default_decision: parse_py_permission_decision(&policy.default_decision)?,
        enabled: policy.enabled,
    })
}

/// HITL confirmation policy configuration.
#[pyclass(name = "ConfirmationPolicy")]
#[derive(Clone)]
pub(super) struct PyConfirmationPolicy {
    #[pyo3(get, set)]
    enabled: bool,
    #[pyo3(get, set)]
    default_timeout_ms: u64,
    #[pyo3(get, set)]
    timeout_action: String,
    #[pyo3(get, set)]
    yolo_lanes: Vec<String>,
}

#[pymethods]
impl PyConfirmationPolicy {
    #[new]
    #[pyo3(signature = (enabled=false, default_timeout_ms=30000, timeout_action=None, yolo_lanes=None))]
    fn new(
        enabled: bool,
        default_timeout_ms: u64,
        timeout_action: Option<String>,
        yolo_lanes: Option<Vec<String>>,
    ) -> Self {
        Self {
            enabled,
            default_timeout_ms,
            timeout_action: timeout_action.unwrap_or_else(|| "reject".to_string()),
            yolo_lanes: yolo_lanes.unwrap_or_default(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "ConfirmationPolicy(enabled={}, default_timeout_ms={}, timeout_action={:?}, yolo_lanes={})",
            self.enabled,
            self.default_timeout_ms,
            self.timeout_action,
            self.yolo_lanes.len()
        )
    }
}

fn parse_py_timeout_action(value: &str) -> PyResult<RustTimeoutAction> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "reject" => Ok(RustTimeoutAction::Reject),
        "auto_approve" | "autoapprove" => Ok(RustTimeoutAction::AutoApprove),
        other => Err(PyValueError::new_err(format!(
            "timeout_action must be 'reject' or 'auto_approve', got {other:?}"
        ))),
    }
}

pub(super) fn py_confirmation_policy_to_rust(
    policy: PyConfirmationPolicy,
) -> PyResult<RustConfirmationPolicy> {
    let mut rust_policy = if policy.enabled {
        RustConfirmationPolicy::enabled()
    } else {
        RustConfirmationPolicy::default()
    };

    rust_policy = rust_policy.with_timeout(
        policy.default_timeout_ms,
        parse_py_timeout_action(&policy.timeout_action)?,
    );

    let yolo_lanes = policy
        .yolo_lanes
        .iter()
        .map(|lane| parse_lane(lane))
        .collect::<PyResult<Vec<_>>>()?;
    if !yolo_lanes.is_empty() {
        rust_policy = rust_policy.with_yolo_lanes(yolo_lanes);
    }

    Ok(rust_policy)
}

/// Retention limits for large tool/program artifacts.
#[pyclass(name = "ArtifactStoreLimits")]
#[derive(Clone)]
pub(super) struct PyArtifactStoreLimits {
    /// Maximum number of artifacts retained by a session.
    #[pyo3(get, set)]
    pub(super) max_artifacts: usize,
    /// Maximum total artifact content bytes retained by a session.
    #[pyo3(get, set)]
    pub(super) max_bytes: usize,
}

#[pymethods]
impl PyArtifactStoreLimits {
    #[new]
    #[pyo3(signature = (max_artifacts=None, max_bytes=None))]
    fn new(max_artifacts: Option<usize>, max_bytes: Option<usize>) -> Self {
        let defaults = a3s_code_core::tools::ArtifactStoreLimits::default();
        Self {
            max_artifacts: max_artifacts.unwrap_or(defaults.max_artifacts),
            max_bytes: max_bytes.unwrap_or(defaults.max_bytes),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "ArtifactStoreLimits(max_artifacts={}, max_bytes={})",
            self.max_artifacts, self.max_bytes
        )
    }
}

impl From<PyArtifactStoreLimits> for a3s_code_core::tools::ArtifactStoreLimits {
    fn from(limits: PyArtifactStoreLimits) -> Self {
        Self {
            max_artifacts: limits.max_artifacts,
            max_bytes: limits.max_bytes,
        }
    }
}

/// Reproducible recipe for a disposable worker/subagent.
#[pyclass(name = "WorkerAgentSpec")]
#[derive(Clone)]
pub(super) struct PyWorkerAgentSpec {
    #[pyo3(get, set)]
    name: String,
    #[pyo3(get, set)]
    description: String,
    #[pyo3(get, set)]
    kind: String,
    #[pyo3(get, set)]
    hidden: bool,
    #[pyo3(get, set)]
    permissions: Option<PyPermissionPolicy>,
    #[pyo3(get, set)]
    model: Option<String>,
    #[pyo3(get, set)]
    prompt: Option<String>,
    #[pyo3(get, set)]
    max_steps: Option<usize>,
    #[pyo3(get, set)]
    confirmation_inheritance: Option<String>,
}

#[pymethods]
impl PyWorkerAgentSpec {
    #[new]
    #[pyo3(signature = (name, description, kind=None))]
    fn new(name: String, description: String, kind: Option<String>) -> Self {
        Self {
            name,
            description,
            kind: kind.unwrap_or_else(|| "custom".to_string()),
            hidden: false,
            permissions: None,
            model: None,
            prompt: None,
            max_steps: None,
            confirmation_inheritance: None,
        }
    }

    #[staticmethod]
    fn read_only(name: String, description: String) -> Self {
        Self::new(name, description, Some("read_only".to_string()))
    }

    #[staticmethod]
    fn planner(name: String, description: String) -> Self {
        Self::new(name, description, Some("planner".to_string()))
    }

    #[staticmethod]
    fn implementer(name: String, description: String) -> Self {
        Self::new(name, description, Some("implementer".to_string()))
    }

    #[staticmethod]
    fn verifier(name: String, description: String) -> Self {
        Self::new(name, description, Some("verifier".to_string()))
    }

    #[staticmethod]
    fn reviewer(name: String, description: String) -> Self {
        Self::new(name, description, Some("reviewer".to_string()))
    }

    #[staticmethod]
    fn custom(name: String, description: String) -> Self {
        Self::new(name, description, Some("custom".to_string()))
    }

    fn __repr__(&self) -> String {
        format!(
            "WorkerAgentSpec(name={:?}, kind={:?}, max_steps={:?})",
            self.name, self.kind, self.max_steps
        )
    }
}

/// Compiled agent definition returned after registering a worker.
#[pyclass(name = "AgentDefinition")]
#[derive(Clone)]
pub(super) struct PyAgentDefinition {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    description: String,
    #[pyo3(get)]
    native: bool,
    #[pyo3(get)]
    hidden: bool,
    #[pyo3(get)]
    model: Option<String>,
    #[pyo3(get)]
    prompt: Option<String>,
    #[pyo3(get)]
    max_steps: Option<usize>,
    #[pyo3(get)]
    confirmation_inheritance: Option<String>,
}

#[pymethods]
impl PyAgentDefinition {
    fn __repr__(&self) -> String {
        format!(
            "AgentDefinition(name={:?}, native={}, hidden={})",
            self.name, self.native, self.hidden
        )
    }
}

fn parse_py_worker_agent_kind(kind: &str) -> PyResult<RustWorkerAgentKind> {
    kind.parse::<RustWorkerAgentKind>()
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

pub(super) fn py_worker_agent_spec_to_rust(
    spec: PyWorkerAgentSpec,
) -> PyResult<RustWorkerAgentSpec> {
    if spec.name.trim().is_empty() {
        return Err(PyValueError::new_err("worker agent name is required"));
    }
    if spec.description.trim().is_empty() {
        return Err(PyValueError::new_err(
            "worker agent description is required",
        ));
    }

    let mut worker = RustWorkerAgentSpec::new(
        parse_py_worker_agent_kind(&spec.kind)?,
        spec.name,
        spec.description,
    )
    .hidden(spec.hidden);
    if let Some(policy) = spec.permissions {
        worker = worker.with_permissions(py_permission_policy_to_rust(policy)?);
    }
    if let Some(model) = spec.model {
        worker = worker.with_model(RustAgentModelConfig::from_model_ref(model));
    }
    if let Some(prompt) = spec.prompt {
        worker = worker.with_prompt(prompt);
    }
    if let Some(max_steps) = spec.max_steps {
        worker = worker.with_max_steps(max_steps);
    }
    if let Some(ci) = spec.confirmation_inheritance {
        worker = worker.with_confirmation(parse_py_confirmation_inheritance(&ci)?);
    }
    Ok(worker)
}

fn parse_py_confirmation_inheritance(
    value: &str,
) -> PyResult<a3s_code_core::subagent::ConfirmationInheritance> {
    use a3s_code_core::subagent::ConfirmationInheritance;
    match value {
        "auto_approve" => Ok(ConfirmationInheritance::AutoApprove),
        "deny_on_ask" => Ok(ConfirmationInheritance::DenyOnAsk),
        "inherit_parent" => Ok(ConfirmationInheritance::InheritParent),
        other => Err(PyValueError::new_err(format!(
            "invalid confirmation_inheritance: '{}' (expected: auto_approve, deny_on_ask, inherit_parent)",
            other
        ))),
    }
}

fn confirmation_inheritance_to_py(ci: &a3s_code_core::subagent::ConfirmationInheritance) -> String {
    use a3s_code_core::subagent::ConfirmationInheritance;
    match ci {
        ConfirmationInheritance::AutoApprove => "auto_approve".to_string(),
        ConfirmationInheritance::DenyOnAsk => "deny_on_ask".to_string(),
        ConfirmationInheritance::InheritParent => "inherit_parent".to_string(),
    }
}

pub(super) fn rust_agent_definition_to_py(def: RustAgentDefinition) -> PyAgentDefinition {
    PyAgentDefinition {
        name: def.name,
        description: def.description,
        native: def.native,
        hidden: def.hidden,
        model: def.model.map(|model| model.model_ref()),
        prompt: def.prompt,
        max_steps: def.max_steps,
        confirmation_inheritance: def
            .confirmation_inheritance
            .as_ref()
            .map(confirmation_inheritance_to_py),
    }
}

/// Automatic child-agent delegation controls.
#[pyclass(name = "AutoDelegationConfig")]
#[derive(Clone)]
pub(super) struct PyAutoDelegationConfig {
    enabled: bool,
    auto_parallel: bool,
    min_confidence: f32,
    max_tasks: usize,
}

impl From<PyAutoDelegationConfig> for a3s_code_core::AutoDelegationConfig {
    fn from(config: PyAutoDelegationConfig) -> Self {
        Self {
            enabled: config.enabled,
            auto_parallel: config.auto_parallel,
            min_confidence: config.min_confidence.clamp(0.0, 1.0),
            max_tasks: config.max_tasks.max(1),
            // Core fields not exposed by the Python SDK (e.g.
            // `allow_manual_delegation`) take their core defaults, so adding a
            // field to `AutoDelegationConfig` no longer breaks this wheel build.
            ..Default::default()
        }
    }
}

#[pymethods]
impl PyAutoDelegationConfig {
    #[new]
    #[pyo3(signature = (enabled=false, auto_parallel=true, min_confidence=0.72, max_tasks=4))]
    pub(super) fn new(
        enabled: bool,
        auto_parallel: bool,
        min_confidence: f32,
        max_tasks: usize,
    ) -> Self {
        Self {
            enabled,
            auto_parallel,
            min_confidence: min_confidence.clamp(0.0, 1.0),
            max_tasks: max_tasks.max(1),
        }
    }

    /// Enable runtime-driven automatic child-agent delegation.
    #[getter]
    fn get_enabled(&self) -> bool {
        self.enabled
    }

    #[setter]
    fn set_enabled(&mut self, value: bool) {
        self.enabled = value;
    }

    /// Allow automatic delegation to launch multiple child agents in parallel.
    ///
    /// Manual ``task`` fan-out and legacy ``parallel_task`` calls remain
    /// available when this is false.
    #[getter]
    fn get_auto_parallel(&self) -> bool {
        self.auto_parallel
    }

    #[setter]
    fn set_auto_parallel(&mut self, value: bool) {
        self.auto_parallel = value;
    }

    /// Minimum local confidence required to auto-delegate a child task.
    #[getter]
    fn get_min_confidence(&self) -> f32 {
        self.min_confidence
    }

    #[setter]
    fn set_min_confidence(&mut self, value: f32) {
        self.min_confidence = value.clamp(0.0, 1.0);
    }

    /// Maximum number of automatic child tasks per user request.
    #[getter]
    fn get_max_tasks(&self) -> usize {
        self.max_tasks
    }

    #[setter]
    fn set_max_tasks(&mut self, value: usize) {
        self.max_tasks = value.max(1);
    }

    fn __repr__(&self) -> String {
        format!(
            "AutoDelegationConfig(enabled={}, auto_parallel={}, min_confidence={}, max_tasks={})",
            self.enabled, self.auto_parallel, self.min_confidence, self.max_tasks
        )
    }
}
