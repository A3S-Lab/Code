//! A3S Flow-backed dynamic workflow runtime.
//!
//! `DynamicWorkflowRuntime` lets hosts run a sandboxed PTC script as an A3S
//! Flow runtime. Flow owns durable replay and step lifecycle; A3S Code's
//! existing `program` tool remains the sandbox and tool-call boundary.

use crate::execution_identity::{
    ExecutionIdentityV1, DYNAMIC_WORKFLOW_CLAIM_IDENTITY_DOMAIN_V1,
    DYNAMIC_WORKFLOW_CONTINUATION_IDENTITY_DOMAIN_V1, DYNAMIC_WORKFLOW_INPUT_IDENTITY_DOMAIN_V1,
    FLOW_STEP_IDENTITY_DOMAIN_V1,
};
use crate::llm::{ModelGenerationAdmission, ModelGenerationConcurrency};
use crate::task_scheduler::{TaskLease, TaskPriority as SchedulerTaskPriority, TaskScheduler};
use crate::tools::{
    registry_tool_invoker, Tool, ToolContext, ToolInvoker, ToolOutput, ToolRegistry, ToolResult,
};
use crate::{
    agent::AgentEvent,
    flow_graph::{
        FileFlowDecisionLedger, FlowDecisionClaimOutcome, FlowDecisionClaimState,
        FlowDecisionLedger, FlowGraphObserver, MemoryFlowDecisionLedger,
    },
    planning::{Complexity, ExecutionPlan, Task, TaskStatus},
};
use a3s_flow::{
    CancellationRequest, FanoutFlowEventObserver, FlowEngine, FlowEvent, FlowEventEnvelope,
    FlowEventObserver, FlowEventStore, FlowRuntime, InMemoryEventStore, LocalFileEventStore,
    RuntimeBuildCompatibility, RuntimeBuildId, RuntimeCommand, StepInvocation, StepStatus,
    WorkflowInvocation, WorkflowRunSnapshot, WorkflowRunStatus, WorkflowSpec,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::future::Future;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const DYNAMIC_WORKFLOW_TOOL: &str = "dynamic_workflow";
const GENERATE_OBJECT_TOOL: &str = "generate_object";
const PROGRAM_TOOL: &str = "program";
const TASK_TOOL: &str = "task";
const PARALLEL_TASK_TOOL: &str = "parallel_task";
const MAX_INLINE_RETRY_RESUMES: usize = 8;
const MAX_INLINE_RETRY_DELAY: Duration = Duration::from_secs(5);
const DEFAULT_MAX_CONCURRENT_STEPS: usize = 4;
const MAX_MAX_CONCURRENT_STEPS: usize = 32;
const MAX_FLOW_STEP_ID_BYTES: usize = 256;
const MAX_FLOW_STEP_INPUT_BYTES: usize = 64 * 1024;
const MAX_DYNAMIC_WORKFLOW_INPUT_BYTES: usize = 128 * 1024;
const DEFAULT_DYNAMIC_WORKFLOW_LEASE_MS: u64 = 30_000;
const MAX_DYNAMIC_WORKFLOW_SETTLE: Duration = Duration::from_secs(5);
const MAX_DYNAMIC_WORKFLOW_CONTROL_REASON_BYTES: usize = 4 * 1024;
const DYNAMIC_WORKFLOW_LEASE_RELATIVE_PATH: &str = "leases";

/// Runtime build used to pin newly-created dynamic workflow runs.
///
/// Deployments that need a stronger revision identity can provide an explicit
/// [`RuntimeBuildCompatibility`] through [`DynamicWorkflowTool::with_runtime_build_compatibility`].
pub const DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID: &str =
    concat!("a3s-code-core-", env!("CARGO_PKG_VERSION"));

/// Legacy marker used only while deriving an identity for an unpinned history
/// created before dynamic workflow runtime-build fencing was enabled.
const LEGACY_UNPINNED_RUNTIME_BUILD_ID: &str = "<unpinned>";

/// Project-relative directory used for durable dynamic workflow history.
pub const DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH: &str = ".a3s/workflow";

/// Cross-process serialized adapter for a local Flow event journal.
///
/// `a3s-flow::LocalFileEventStore` deliberately serializes writers only
/// inside one process. Dynamic workflows can be resumed or controlled by a
/// replacement process, so Code adds one small lock-file boundary around the
/// same append-only journal. The adapter does not project or cache workflow
/// state: Flow remains the sole event authority and its optimistic sequence
/// checks still decide whether an append is accepted.
#[derive(Debug, Clone)]
pub struct CrossProcessFlowEventStore {
    root: PathBuf,
    inner: LocalFileEventStore,
    process_lock: Arc<Mutex<()>>,
}

impl CrossProcessFlowEventStore {
    /// Create a cross-process event store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            inner: LocalFileEventStore::new(root.clone()),
            root,
            process_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Return the directory containing the journal files.
    pub fn root(&self) -> &Path {
        &self.root
    }

    async fn acquire_file_lock(&self) -> a3s_flow::Result<std::fs::File> {
        tokio::fs::create_dir_all(&self.root).await?;
        let lock_path = self.root.join(".flow-events.lock");
        match tokio::fs::symlink_metadata(&lock_path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(a3s_flow::FlowError::Store(format!(
                    "refusing to use symlinked event-store lock {}",
                    lock_path.display()
                )))
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(a3s_flow::FlowError::Store(format!(
                    "event-store lock {} exists but is not a file",
                    lock_path.display()
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(a3s_flow::FlowError::Io(error)),
        }
        tokio::task::spawn_blocking(move || {
            use fs2::FileExt;
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(a3s_flow::FlowError::Io)?;
            file.lock_exclusive().map_err(a3s_flow::FlowError::Io)?;
            Ok(file)
        })
        .await
        .map_err(|error| {
            a3s_flow::FlowError::Store(format!("event-store lock task failed: {error}"))
        })?
    }
}

#[async_trait]
impl FlowEventStore for CrossProcessFlowEventStore {
    async fn append(&self, run_id: &str, event: FlowEvent) -> a3s_flow::Result<FlowEventEnvelope> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner.append(run_id, event).await
    }

    async fn append_if_sequence(
        &self,
        run_id: &str,
        expected_sequence: u64,
        event: FlowEvent,
    ) -> a3s_flow::Result<FlowEventEnvelope> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner
            .append_if_sequence(run_id, expected_sequence, event)
            .await
    }

    async fn list(&self, run_id: &str) -> a3s_flow::Result<Vec<FlowEventEnvelope>> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner.list(run_id).await
    }

    async fn list_run_ids(&self) -> a3s_flow::Result<Vec<String>> {
        let _process_guard = self.process_lock.lock().await;
        let _file_guard = self.acquire_file_lock().await?;
        self.inner.list_run_ids().await
    }
}

/// Resolve the durable dynamic workflow history directory for a local workspace.
pub fn dynamic_workflow_store_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root
        .as_ref()
        .join(DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH)
}

/// Recover one completed step output from the exact durable workflow run.
///
/// Recovery is bound to the requested run ID and the original input query. It
/// never acts as a cross-run query cache and never promotes an incomplete step.
pub async fn recover_dynamic_workflow_step_output(
    workspace_root: impl AsRef<Path>,
    run_id: &str,
    expected_query: &str,
    step_id: &str,
) -> Result<Option<Value>> {
    if !safe_workflow_run_id(run_id) || expected_query.is_empty() || step_id.is_empty() {
        return Ok(None);
    }
    let workspace_root = workspace_root.as_ref();
    let store_root = dynamic_workflow_store_path(workspace_root);
    let log_path = store_root.join(format!("{run_id}.jsonl"));
    match tokio::fs::symlink_metadata(&log_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Ok(None)
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("inspect dynamic workflow history {}", log_path.display())
            })
        }
    }
    validate_dynamic_workflow_directory(&workspace_root.join(".a3s"), ".a3s").await?;
    validate_dynamic_workflow_directory(&store_root, ".a3s/workflow").await?;
    validate_dynamic_workflow_log(&log_path).await?;

    let events = LocalFileEventStore::new(store_root).list(run_id).await?;
    let input_matches = events.iter().any(|envelope| {
        matches!(
            &envelope.event,
            FlowEvent::RunCreated { input, .. }
                if input.get("query").and_then(Value::as_str) == Some(expected_query)
        )
    });
    if !input_matches {
        return Ok(None);
    }
    Ok(events
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.event {
            FlowEvent::StepCompleted {
                step_id: completed_step_id,
                output,
            } if completed_step_id == step_id => Some(output.clone()),
            _ => None,
        }))
}

/// Limits forwarded to the underlying PTC `program` tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWorkflowScriptLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
    /// Maximum independently session-bound model generations active at once.
    /// This orchestration limit is not forwarded to the PTC program sandbox.
    #[serde(default, skip_serializing)]
    pub max_concurrent_generations: Option<usize>,
    /// Maximum Flow step bodies that may execute concurrently for one run.
    /// This is an orchestration boundary and is never forwarded to QuickJS.
    #[serde(default, skip_serializing)]
    pub max_concurrent_steps: Option<usize>,
}

/// Point-in-time admission counters for one dynamic workflow runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWorkflowAdmissionStats {
    /// Effective per-run step concurrency limit.
    pub max_concurrent_steps: usize,
    /// Number of step bodies admitted since this runtime was created.
    pub admitted_steps: usize,
    /// Number of step bodies currently holding a local permit.
    pub active_steps: usize,
    /// Highest observed active step count.
    pub peak_active_steps: usize,
}

/// Bounded, host-facing projection of one dynamic workflow continuation.
///
/// The projection deliberately omits source, input, step arguments, outputs,
/// and worker owner tokens. Flow history remains available through the
/// explicit inspection APIs when a trusted host needs the full record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWorkflowControlSnapshot {
    /// Stable durable run identifier.
    pub run_id: String,
    /// Current materialized Flow status.
    pub status: WorkflowRunStatus,
    /// Last durable event sequence observed for the run.
    pub last_sequence: u64,
    /// Number of durable step definitions in the run.
    pub step_count: usize,
    /// Number of steps with a committed output.
    pub completed_steps: usize,
    /// Number of steps that remain actionable or in flight.
    pub open_steps: usize,
    /// Whether Flow has recorded a cleanup-aware cancellation request.
    pub cancellation_requested: bool,
    /// Digest-only identity reconstructed from immutable continuation facts.
    pub continuation_identity: ExecutionIdentityV1,
    /// Digest-only identity of the projected immutable step plan.
    pub plan_identity: ExecutionIdentityV1,
    /// Runtime build pinned by the durable run, or `None` for a legacy
    /// unpinned history.
    pub runtime_build_id: Option<String>,
    /// Redacted worker-lease state observed by the control operation.
    pub worker_lease: FlowDecisionClaimState,
}

/// A host-owned control handle for one dynamic workflow run.
///
/// The handle keeps the exact source, input, registry, and runtime policy
/// needed to replay the selected run. Mutating operations first acquire the
/// same worker lease used by [`DynamicWorkflowTool`], then ask A3S Flow to
/// append/drive its authoritative events. A live worker therefore remains the
/// sole executor; a controller retries after the redacted lease state reports
/// that the claim is busy.
#[must_use = "a dynamic workflow control handle should be used or explicitly dropped"]
#[derive(Clone)]
pub struct DynamicWorkflowControl {
    registry: Arc<ToolRegistry>,
    context: ToolContext,
    flow_event_store: Option<Arc<dyn FlowEventStore>>,
    run_id: String,
    source: Arc<str>,
    input: Value,
    allowed_tools: Vec<String>,
    limits: DynamicWorkflowScriptLimits,
    graph_observer: Option<FlowGraphObserver>,
    task_scheduler: Option<Arc<TaskScheduler>>,
    admit_steps_globally: bool,
    runtime_build_compatibility: Option<RuntimeBuildCompatibility>,
    continuation_lease_ledger: Option<Arc<dyn FlowDecisionLedger>>,
    continuation_lease_ms: u64,
    memory_continuation_lease_ledger: Arc<MemoryFlowDecisionLedger>,
}

#[derive(Clone)]
struct DynamicStepAdmission {
    semaphore: Arc<Semaphore>,
    max_concurrent_steps: usize,
    admitted_steps: Arc<AtomicUsize>,
    active_steps: Arc<AtomicUsize>,
    peak_active_steps: Arc<AtomicUsize>,
}

impl DynamicStepAdmission {
    fn new(max_concurrent_steps: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent_steps)),
            max_concurrent_steps,
            admitted_steps: Arc::new(AtomicUsize::new(0)),
            active_steps: Arc::new(AtomicUsize::new(0)),
            peak_active_steps: Arc::new(AtomicUsize::new(0)),
        }
    }

    async fn acquire(
        &self,
        identity: ExecutionIdentityV1,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> a3s_flow::Result<DynamicStepLease> {
        let permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(a3s_flow::FlowError::Runtime(
                    "dynamic workflow step admission cancelled".to_string(),
                ));
            }
            permit = Arc::clone(&self.semaphore).acquire_owned() => {
                permit.map_err(|_| a3s_flow::FlowError::Runtime(
                    "dynamic workflow step admission is closed".to_string(),
                ))?
            }
        };
        if cancellation.is_cancelled() {
            drop(permit);
            return Err(a3s_flow::FlowError::Runtime(
                "dynamic workflow step admission cancelled".to_string(),
            ));
        }

        let active = self.active_steps.fetch_add(1, Ordering::AcqRel) + 1;
        self.admitted_steps.fetch_add(1, Ordering::Relaxed);
        let mut observed = self.peak_active_steps.load(Ordering::Acquire);
        while active > observed {
            match self.peak_active_steps.compare_exchange(
                observed,
                active,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => observed = current,
            }
        }
        tracing::trace!(
            execution_identity = identity.key(),
            active_steps = active,
            max_concurrent_steps = self.max_concurrent_steps,
            "dynamic workflow step admitted"
        );
        Ok(DynamicStepLease {
            _permit: permit,
            identity,
            active_steps: Arc::clone(&self.active_steps),
            task_lease: None,
        })
    }

    fn stats(&self) -> DynamicWorkflowAdmissionStats {
        DynamicWorkflowAdmissionStats {
            max_concurrent_steps: self.max_concurrent_steps,
            admitted_steps: self.admitted_steps.load(Ordering::Acquire),
            active_steps: self.active_steps.load(Ordering::Acquire),
            peak_active_steps: self.peak_active_steps.load(Ordering::Acquire),
        }
    }
}

struct DynamicStepLease {
    _permit: OwnedSemaphorePermit,
    identity: ExecutionIdentityV1,
    active_steps: Arc<AtomicUsize>,
    task_lease: Option<TaskLease>,
}

impl DynamicStepLease {
    fn with_task_lease(mut self, task_lease: TaskLease) -> Self {
        self.task_lease = Some(task_lease);
        self
    }
}

impl Drop for DynamicStepLease {
    fn drop(&mut self) {
        let active = self.active_steps.fetch_sub(1, Ordering::AcqRel) - 1;
        tracing::trace!(
            execution_identity = self.identity.key(),
            active_steps = active,
            "dynamic workflow step admission released"
        );
    }
}

/// Runs A3S Flow workflow and step invocations through a sandboxed PTC script.
#[derive(Clone)]
pub struct DynamicWorkflowRuntime {
    invoker: Arc<dyn ToolInvoker>,
    context: ToolContext,
    source: Arc<str>,
    allowed_tools: Vec<String>,
    limits: DynamicWorkflowScriptLimits,
    parallel_generation_admission: Option<ModelGenerationAdmission>,
    step_admission: DynamicStepAdmission,
    /// Optional global scheduler for runtimes created outside an AgentSession.
    /// Session-bound callers already hold the enclosing scheduler lease and
    /// should leave this unset to avoid nested single-slot deadlocks.
    task_scheduler: Option<Arc<TaskScheduler>>,
    admit_steps_globally: bool,
    continuation_lease: Option<Arc<DynamicWorkflowLease>>,
}

impl DynamicWorkflowRuntime {
    pub fn new(
        registry: Arc<ToolRegistry>,
        context: ToolContext,
        source: impl Into<String>,
    ) -> Self {
        let allowed_tools = default_allowed_tools(&registry);
        // Session/agent callers install the governed gateway in ToolContext.
        // The raw registry adapter is retained only for explicit low-level
        // callers that construct this public runtime outside an AgentSession.
        let invoker = context
            .tool_invoker()
            .unwrap_or_else(|| registry_tool_invoker(registry));
        Self {
            invoker,
            context,
            source: Arc::from(source.into()),
            allowed_tools,
            limits: DynamicWorkflowScriptLimits::default(),
            parallel_generation_admission: None,
            step_admission: DynamicStepAdmission::new(DEFAULT_MAX_CONCURRENT_STEPS),
            task_scheduler: None,
            admit_steps_globally: false,
            continuation_lease: None,
        }
    }

    pub fn with_allowed_tools(mut self, allowed_tools: impl IntoIterator<Item = String>) -> Self {
        self.allowed_tools = sanitize_allowed_tools(allowed_tools);
        self
    }

    pub fn with_limits(mut self, limits: DynamicWorkflowScriptLimits) -> Self {
        let generation_concurrency = limits.max_concurrent_generations.unwrap_or(1).clamp(1, 4);
        self.parallel_generation_admission = NonZeroUsize::new(generation_concurrency)
            .filter(|maximum| maximum.get() > 1)
            .map(|maximum| {
                ModelGenerationAdmission::new(ModelGenerationConcurrency::bounded(maximum))
            });
        let step_concurrency = limits
            .max_concurrent_steps
            .unwrap_or(DEFAULT_MAX_CONCURRENT_STEPS)
            .clamp(1, MAX_MAX_CONCURRENT_STEPS);
        self.step_admission = DynamicStepAdmission::new(step_concurrency);
        self.limits = limits;
        self
    }

    /// Attach the agent-wide scheduler when this runtime is used as a
    /// standalone host adapter.
    ///
    /// `admit_steps_globally = false` is the correct setting for a runtime
    /// invoked from an existing AgentSession operation: the parent operation
    /// already owns the global lease and Flow steps use the local bounded gate.
    pub fn with_task_scheduler(
        mut self,
        scheduler: Arc<TaskScheduler>,
        admit_steps_globally: bool,
    ) -> Self {
        self.task_scheduler = Some(scheduler);
        self.admit_steps_globally = admit_steps_globally;
        self
    }

    fn with_continuation_lease(mut self, lease: Arc<DynamicWorkflowLease>) -> Self {
        self.continuation_lease = Some(lease);
        self
    }

    /// Return local Flow-step admission counters for diagnostics and hosts.
    pub fn admission_stats(&self) -> DynamicWorkflowAdmissionStats {
        self.step_admission.stats()
    }

    async fn admit_step(&self, invocation: &StepInvocation) -> a3s_flow::Result<DynamicStepLease> {
        self.ensure_continuation_lease().await?;
        let identity = dynamic_workflow_step_identity(
            &invocation.run_id,
            &invocation.step_id,
            &invocation.step_name,
            &invocation.input,
        )
        .map_err(|error| a3s_flow::FlowError::Runtime(error.to_string()))?;
        let mut lease = self
            .step_admission
            .acquire(identity.clone(), &self.context.cancellation_token())
            .await?;

        // Host task fan-out has its own child-run scheduler boundary. Holding
        // a second global lease here would deadlock a max_active=1 scheduler
        // while the child waits for capacity, so only direct script-backed
        // steps opt into the standalone global admission.
        if self.admit_steps_globally
            && !matches!(
                invocation.step_name.as_str(),
                TASK_TOOL | PARALLEL_TASK_TOOL
            )
        {
            let Some(scheduler) = self.task_scheduler.as_ref() else {
                return Err(a3s_flow::FlowError::Runtime(
                    "global Flow-step admission requires a configured task scheduler".to_string(),
                ));
            };
            let task_lease = scheduler
                .acquire_with_identity(
                    SchedulerTaskPriority::Foreground,
                    format!(
                        "flow:{}:{}:{}",
                        invocation.run_id, invocation.step_id, invocation.step_name
                    ),
                    Some(identity),
                    &self.context.cancellation_token(),
                )
                .await
                .map_err(|error| a3s_flow::FlowError::Runtime(error.to_string()))?;
            lease = lease.with_task_lease(task_lease);
        }
        Ok(lease)
    }

    async fn ensure_continuation_lease(&self) -> a3s_flow::Result<()> {
        let Some(lease) = self.continuation_lease.as_ref() else {
            return Ok(());
        };
        if lease
            .renew()
            .await
            .map_err(|error| a3s_flow::FlowError::Runtime(error.to_string()))?
        {
            Ok(())
        } else {
            Err(a3s_flow::FlowError::Runtime(
                "dynamic workflow worker lease is no longer owned before admission".to_string(),
            ))
        }
    }

    async fn run_script(
        &self,
        payload: Value,
        context: &ToolContext,
    ) -> a3s_flow::Result<ToolResult> {
        let mut args = json!({
            "type": "script",
            "language": "javascript",
            "source": self.source.as_ref(),
            "inputs": payload,
            "allowed_tools": self.allowed_tools,
        });
        if let Some(object) = args.as_object_mut() {
            if let Ok(Value::Object(limits)) = serde_json::to_value(&self.limits) {
                if !limits.is_empty() {
                    object.insert("limits".to_string(), Value::Object(limits));
                }
            }
        }

        let result = self
            .invoker
            .invoke(
                crate::tools::ToolInvocation::runtime_internal(PROGRAM_TOOL, args),
                context,
            )
            .await;
        if result.exit_code != 0 {
            return Err(a3s_flow::FlowError::Runtime(result.output));
        }
        Ok(result)
    }

    async fn context_for_step(
        &self,
        run_id: &str,
        step_id: &str,
        step_name: &str,
    ) -> a3s_flow::Result<ToolContext> {
        if step_name != GENERATE_OBJECT_TOOL {
            return Ok(self.context.clone());
        }
        if let (Some(admission), Some(client)) = (
            self.parallel_generation_admission.as_ref(),
            self.context.llm_client(),
        ) {
            let fork_id = format!("{run_id}:{step_id}");
            if let Some(forked_client) = client.fork_for_session(&fork_id) {
                let permit = admission
                    .acquire(&self.context.cancellation_token())
                    .await
                    .map_err(|error| {
                        a3s_flow::FlowError::Runtime(format!(
                            "parallel model-generation admission failed before workflow step: {error}"
                        ))
                    })?;
                return self
                    .context
                    .clone()
                    .with_llm_client(forked_client)
                    .with_model_generation_permit(admission.clone(), Arc::new(permit))
                    .map_err(|error| {
                        a3s_flow::FlowError::Runtime(format!(
                            "bind parallel model-generation admission to workflow step: {error}"
                        ))
                    });
            }
        }
        let Some(admission) = self.context.model_generation_admission() else {
            return Ok(self.context.clone());
        };
        let permit = admission
            .acquire(&self.context.cancellation_token())
            .await
            .map_err(|error| {
                a3s_flow::FlowError::Runtime(format!(
                    "model-generation admission failed before workflow step: {error}"
                ))
            })?;
        self.context
            .clone()
            .with_model_generation_permit(admission, Arc::new(permit))
            .map_err(|error| {
                a3s_flow::FlowError::Runtime(format!(
                    "bind model-generation admission to workflow step: {error}"
                ))
            })
    }

    async fn run_tool_step(&self, tool_name: &str, args: Value) -> a3s_flow::Result<Value> {
        let result = self
            .invoker
            .invoke(
                self.context
                    .nested_tool_invocation(tool_name.to_string(), args),
                &self.context,
            )
            .await;
        if result.exit_code != 0 {
            return Err(a3s_flow::FlowError::Runtime(result.output));
        }
        Ok(json!({
            "tool": result.name,
            "output": result.output,
            "exit_code": result.exit_code,
            "metadata": result.metadata,
        }))
    }
}

#[async_trait]
impl FlowRuntime for DynamicWorkflowRuntime {
    async fn run_workflow(
        &self,
        invocation: WorkflowInvocation,
    ) -> a3s_flow::Result<RuntimeCommand> {
        self.ensure_continuation_lease().await?;
        let payload = invocation_payload("workflow", &invocation.run_id, &invocation.history)
            .with("input", invocation.input);
        let result = self.run_script(payload.into_value(), &self.context).await?;
        serde_json::from_value(script_result(&result)?).map_err(a3s_flow::FlowError::from)
    }

    async fn run_step(&self, invocation: StepInvocation) -> a3s_flow::Result<Value> {
        let _admission = self.admit_step(&invocation).await?;
        if matches!(
            invocation.step_name.as_str(),
            TASK_TOOL | PARALLEL_TASK_TOOL
        ) {
            return self
                .run_tool_step(&invocation.step_name, invocation.input)
                .await;
        }

        let context = self
            .context_for_step(
                &invocation.run_id,
                &invocation.step_id,
                &invocation.step_name,
            )
            .await?;
        let payload = invocation_payload("step", &invocation.run_id, &invocation.history)
            .with("step_id", invocation.step_id)
            .with("step_name", invocation.step_name)
            .with("input", invocation.input);
        let result = self.run_script(payload.into_value(), &context).await?;
        script_result(&result)
    }
}

/// Derive the stable identity for one dynamic Flow step admission.
///
/// The returned value is digest-only. The step input is included in the
/// derivation so retries with different arguments cannot share a lease, but no
/// input bytes are retained in the identity or emitted by the scheduler.
pub fn dynamic_workflow_step_identity(
    run_id: &str,
    step_id: &str,
    step_name: &str,
    input: &Value,
) -> Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    for (field, value) in [
        ("run_id", run_id),
        ("step_id", step_id),
        ("step_name", step_name),
    ] {
        if value.is_empty()
            || value.len() > MAX_FLOW_STEP_ID_BYTES
            || value.contains('\0')
            || value.lines().count() != 1
        {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField(field),
            );
        }
    }
    let encoded_input = serde_json::to_vec(input).map_err(|error| {
        crate::execution_identity::ExecutionIdentityError::Serialization(error.to_string())
    })?;
    if encoded_input.len() > MAX_FLOW_STEP_INPUT_BYTES {
        return Err(
            crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                "dynamic Flow step input exceeds {} bytes",
                MAX_FLOW_STEP_INPUT_BYTES
            )),
        );
    }
    ExecutionIdentityV1::derive(
        FLOW_STEP_IDENTITY_DOMAIN_V1,
        &json!({
            "run_id": run_id,
            "step_id": step_id,
            "step_name": step_name,
            "input": input,
        }),
    )
}

fn dynamic_workflow_input_identity(
    input: &Value,
) -> Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    let encoded = serde_json::to_vec(input).map_err(|error| {
        crate::execution_identity::ExecutionIdentityError::Serialization(error.to_string())
    })?;
    if encoded.len() > MAX_DYNAMIC_WORKFLOW_INPUT_BYTES {
        return Err(
            crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                "dynamic workflow input exceeds {} bytes",
                MAX_DYNAMIC_WORKFLOW_INPUT_BYTES
            )),
        );
    }
    ExecutionIdentityV1::derive(DYNAMIC_WORKFLOW_INPUT_IDENTITY_DOMAIN_V1, input)
}

/// Derive the stable, digest-only claim identity for one dynamic workflow.
///
/// Unlike the continuation identity, this identity intentionally excludes the
/// evolving plan and step history. It therefore remains constant while a
/// worker replays, retries, or takes over the same durable run. The complete
/// continuation identity is still validated first, so malformed or
/// mixed-generation history can never acquire a worker lease.
pub fn dynamic_workflow_claim_identity(
    run_id: &str,
    source: &str,
    input: &Value,
    runtime_build_id: &str,
    history: &[FlowEventEnvelope],
) -> std::result::Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    dynamic_workflow_continuation_identity(run_id, source, input, runtime_build_id, history)?;
    let input_identity = dynamic_workflow_input_identity(input)?;
    let effective_runtime_build_id = history
        .iter()
        .find_map(|envelope| match &envelope.event {
            FlowEvent::RunCreated { spec, .. } => Some(
                spec.runtime_build_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| LEGACY_UNPINNED_RUNTIME_BUILD_ID.to_string()),
            ),
            _ => None,
        })
        .unwrap_or_else(|| runtime_build_id.to_string());
    ExecutionIdentityV1::derive(
        DYNAMIC_WORKFLOW_CLAIM_IDENTITY_DOMAIN_V1,
        &json!({
            "run_id": run_id,
            "source_hash": source_hash(source),
            "input_identity": input_identity.digest,
            "runtime_build_id": effective_runtime_build_id,
        }),
    )
}

/// Reconstruct the immutable identity of a dynamic workflow continuation.
///
/// The Flow journal already persists the run definition and every step
/// definition. This adapter binds those facts to the current runtime build,
/// source, and initial input without introducing a second journal. Progress,
/// retries, event sequence, and step outputs are intentionally excluded, so a
/// restart observes the same continuation identity before and after replay.
/// A malformed or mixed-generation history is rejected before a step body can
/// be admitted.
pub fn dynamic_workflow_continuation_identity(
    run_id: &str,
    source: &str,
    input: &Value,
    runtime_build_id: &str,
    history: &[FlowEventEnvelope],
) -> std::result::Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    if !safe_workflow_run_id(run_id) {
        return Err(crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_id"));
    }
    if source.is_empty() {
        return Err(crate::execution_identity::ExecutionIdentityError::InvalidClaimField("source"));
    }
    RuntimeBuildId::new(runtime_build_id.to_string()).map_err(|error| {
        crate::execution_identity::ExecutionIdentityError::Serialization(format!(
            "invalid dynamic workflow runtime build id: {error}"
        ))
    })?;
    let input_identity = dynamic_workflow_input_identity(input)?;
    let expected_source_hash = source_hash(source);
    let expected_spec = WorkflowSpec::rust_embedded(
        "a3s-code.dynamic-workflow",
        expected_source_hash.as_str(),
        "ptc",
        "run",
    );
    let mut saw_run_created = false;
    let mut persisted_runtime_build: Option<String> = None;
    let mut step_identities = BTreeMap::<String, String>::new();
    let mut terminal_seen = false;

    for (index, envelope) in history.iter().enumerate() {
        if envelope.run_id != run_id {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_id"),
            );
        }
        if envelope.sequence != index as u64 + 1 {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("sequence"),
            );
        }
        if terminal_seen {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("terminal"),
            );
        }
        if index == 0 && !matches!(&envelope.event, FlowEvent::RunCreated { .. }) {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_created"),
            );
        }

        match &envelope.event {
            FlowEvent::RunCreated {
                spec,
                input: persisted_input,
            } => {
                if saw_run_created {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "run_created",
                        ),
                    );
                }
                saw_run_created = true;
                if spec.name != expected_spec.name
                    || spec.runtime != expected_spec.runtime
                    || !spec.patch_markers.is_empty()
                    || !spec.signal_names.is_empty()
                {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "workflow_spec",
                        ),
                    );
                }
                if spec.version != expected_source_hash {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "source",
                        ),
                    );
                }
                let persisted_input_identity = dynamic_workflow_input_identity(persisted_input)?;
                if persisted_input_identity != input_identity {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "input",
                        ),
                    );
                }
                if let Some(build_id) = &spec.runtime_build_id {
                    RuntimeBuildId::new(build_id.as_str().to_string()).map_err(|error| {
                        crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                            "invalid persisted dynamic workflow runtime build id: {error}"
                        ))
                    })?;
                    persisted_runtime_build = Some(build_id.as_str().to_string());
                } else {
                    persisted_runtime_build = Some(LEGACY_UNPINNED_RUNTIME_BUILD_ID.to_string());
                }
            }
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                retry,
            } => {
                let admission_identity =
                    dynamic_workflow_step_identity(run_id, step_id, step_name, input)?;
                // Retry behavior is part of Flow's immutable step definition,
                // even though it is not needed for the scheduler admission
                // lease. Bind it here so a conflicting duplicate cannot hide
                // behind the digest-only admission identity.
                let identity = ExecutionIdentityV1::derive(
                    FLOW_STEP_IDENTITY_DOMAIN_V1,
                    &json!({
                        "admission": admission_identity.digest,
                        "retry": retry,
                    }),
                )?;
                if step_identities
                    .insert(step_id.clone(), identity.digest)
                    .is_some()
                {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "step_definition",
                        ),
                    );
                }
            }
            _ => {}
        }

        terminal_seen = matches!(
            &envelope.event,
            FlowEvent::RunCompleted { .. }
                | FlowEvent::RunFailed { .. }
                | FlowEvent::RunCancelled { .. }
                | FlowEvent::RunTimedOut { .. }
                | FlowEvent::RunRetryExhausted { .. }
                | FlowEvent::RunHostShutdown { .. }
                | FlowEvent::RunContinuedAsNew { .. }
        );
    }

    if !history.is_empty() && !saw_run_created {
        return Err(
            crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_created"),
        );
    }
    let effective_runtime_build = persisted_runtime_build
        .as_deref()
        .unwrap_or(runtime_build_id);
    if effective_runtime_build != LEGACY_UNPINNED_RUNTIME_BUILD_ID {
        RuntimeBuildId::new(effective_runtime_build.to_string()).map_err(|error| {
            crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                "invalid effective dynamic workflow runtime build id: {error}"
            ))
        })?;
    }
    let plan = dynamic_workflow_execution_plan(history);
    let plan_identity = plan.definition_identity()?;
    ExecutionIdentityV1::derive(
        DYNAMIC_WORKFLOW_CONTINUATION_IDENTITY_DOMAIN_V1,
        &json!({
            "run_id": run_id,
            "source_hash": expected_source_hash,
            "input_identity": input_identity.digest,
            "runtime_build_id": effective_runtime_build,
            "plan_identity": plan_identity.digest,
            "step_identities": step_identities,
        }),
    )
}

/// Project the complete durable Flow history into Code's canonical plan model.
///
/// Flow remains the execution authority; this is a read-only adapter used by
/// progress events and metadata. Replaying the full history (rather than only
/// observing newly appended events) makes resumed runs expose the same plan as
/// fresh runs.
pub fn dynamic_workflow_execution_plan(history: &[FlowEventEnvelope]) -> ExecutionPlan {
    let mut plan = ExecutionPlan::new("dynamic workflow", Complexity::Medium);
    for envelope in history {
        match &envelope.event {
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                ..
            } => {
                let task = Task::new(
                    step_id.clone(),
                    workflow_step_description(step_id, step_name, Some(input)),
                )
                .with_tool(step_name.clone());
                plan.upsert_step(task);
            }
            FlowEvent::StepStarted { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::InProgress);
            }
            FlowEvent::StepRetrying { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::InProgress);
            }
            FlowEvent::StepCompleted { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::Completed);
            }
            FlowEvent::StepFailed { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::Failed);
            }
            FlowEvent::RunFailed { .. }
            | FlowEvent::RunTimedOut { .. }
            | FlowEvent::RunRetryExhausted { .. } => {
                for task in &mut plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Failed;
                    }
                }
            }
            FlowEvent::RunCancelled { .. } | FlowEvent::RunHostShutdown { .. } => {
                for task in &mut plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Cancelled;
                    }
                }
            }
            _ => {}
        }
    }
    plan
}

struct WorkflowProgressState {
    plan: ExecutionPlan,
}

impl WorkflowProgressState {
    fn new(plan: ExecutionPlan) -> Self {
        Self { plan }
    }

    fn upsert_step(
        &mut self,
        step_id: &str,
        step_name: &str,
        input: Option<&Value>,
        status: TaskStatus,
    ) {
        let content = workflow_step_description(step_id, step_name, input);
        self.plan.upsert_step(
            Task::new(step_id.to_string(), content)
                .with_tool(step_name)
                .with_status(status),
        );
    }

    fn mark_status(&mut self, step_id: &str, status: TaskStatus) {
        self.plan.mark_status(step_id, status);
    }

    fn step_position(&self, step_id: &str) -> (usize, usize) {
        let total = self.plan.steps.len().max(1);
        let number = self
            .plan
            .steps
            .iter()
            .position(|task| task.id == step_id)
            .map(|idx| idx + 1)
            .unwrap_or(total);
        (number, total)
    }

    fn step_description(&self, step_id: &str) -> String {
        self.plan
            .steps
            .iter()
            .find(|task| task.id == step_id)
            .map(|task| task.content.clone())
            .unwrap_or_else(|| step_id.to_string())
    }

    fn tasks(&self) -> &[Task] {
        &self.plan.steps
    }

    fn snapshot(&self) -> ExecutionPlan {
        self.plan.clone()
    }
}

struct AgentEventFlowObserver {
    tx: broadcast::Sender<AgentEvent>,
    session_id: String,
    state: Mutex<WorkflowProgressState>,
}

impl AgentEventFlowObserver {
    fn new(tx: broadcast::Sender<AgentEvent>, session_id: String, plan: ExecutionPlan) -> Self {
        Self {
            tx,
            session_id,
            state: Mutex::new(WorkflowProgressState::new(plan)),
        }
    }

    fn emit_task_update(&self, tasks: &[Task]) {
        let _ = self.tx.send(AgentEvent::TaskUpdated {
            session_id: self.session_id.clone(),
            tasks: tasks.to_vec(),
        });
    }
}

#[async_trait]
impl FlowEventObserver for AgentEventFlowObserver {
    async fn observe(&self, envelope: FlowEventEnvelope) {
        match envelope.event {
            FlowEvent::RunStarted => {
                let _ = self.tx.send(AgentEvent::PlanningStart {
                    prompt: "dynamic_workflow".to_string(),
                });
                let state = self.state.lock().await;
                if !state.plan.steps.is_empty() {
                    let plan = state.snapshot();
                    let _ = self.tx.send(AgentEvent::PlanningEnd {
                        estimated_steps: plan.steps.len(),
                        plan,
                    });
                }
            }
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                ..
            } => {
                let mut state = self.state.lock().await;
                state.upsert_step(&step_id, &step_name, Some(&input), TaskStatus::Pending);
                self.emit_task_update(state.tasks());
                let plan = state.snapshot();
                let _ = self.tx.send(AgentEvent::PlanningEnd {
                    estimated_steps: plan.steps.len(),
                    plan,
                });
            }
            FlowEvent::StepStarted { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::InProgress);
                self.emit_task_update(state.tasks());
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepStart {
                    description: state.step_description(&step_id),
                    step_id,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::StepCompleted { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::Completed);
                self.emit_task_update(state.tasks());
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepEnd {
                    step_id,
                    status: TaskStatus::Completed,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::StepRetrying { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::InProgress);
                self.emit_task_update(state.tasks());
            }
            FlowEvent::StepFailed { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::Failed);
                self.emit_task_update(state.tasks());
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepEnd {
                    step_id,
                    status: TaskStatus::Failed,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::RunFailed { .. }
            | FlowEvent::RunTimedOut { .. }
            | FlowEvent::RunRetryExhausted { .. } => {
                let mut state = self.state.lock().await;
                for task in &mut state.plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Failed;
                    }
                }
                self.emit_task_update(state.tasks());
            }
            FlowEvent::RunCancelled { .. } | FlowEvent::RunHostShutdown { .. } => {
                let mut state = self.state.lock().await;
                for task in &mut state.plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Cancelled;
                    }
                }
                self.emit_task_update(state.tasks());
            }
            _ => {}
        }
    }
}

fn workflow_step_description(step_id: &str, step_name: &str, input: Option<&Value>) -> String {
    if matches!(step_name, TASK_TOOL | PARALLEL_TASK_TOOL) {
        let count = input
            .and_then(|value| value.get("tasks"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if count > 0 {
            return bounded_workflow_description(&format!(
                "Fan out {count} parallel subagent task(s)"
            ));
        }
    }

    let description = input
        .and_then(|value| value.get("description").or_else(|| value.get("title")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if step_name == step_id {
                step_id.to_string()
            } else {
                format!("{step_name}: {step_id}")
            }
        });
    bounded_workflow_description(&description)
}

fn bounded_workflow_description(value: &str) -> String {
    const MAX_DESCRIPTION_BYTES: usize = 512;
    if value.len() <= MAX_DESCRIPTION_BYTES {
        return value.to_string();
    }
    let mut end = MAX_DESCRIPTION_BYTES.saturating_sub("…".len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[derive(Clone)]
struct DynamicWorkflowLease {
    ledger: Arc<dyn FlowDecisionLedger>,
    decision_id: String,
    request_hash: String,
    identity: ExecutionIdentityV1,
    owner_id: String,
    lease_ms: u64,
    attempt: u32,
}

impl DynamicWorkflowLease {
    async fn renew(&self) -> Result<bool> {
        self.ledger
            .renew_with_identity(
                &self.decision_id,
                &self.request_hash,
                &self.identity,
                &self.owner_id,
                dynamic_workflow_now_ms(),
                self.lease_ms,
            )
            .await
    }

    async fn complete(&self) -> Result<()> {
        self.ledger
            .complete_with_identity(
                &self.decision_id,
                &self.request_hash,
                &self.identity,
                &self.owner_id,
                dynamic_workflow_now_ms(),
            )
            .await
    }

    async fn release(&self) -> Result<()> {
        self.ledger
            .release_with_identity(
                &self.decision_id,
                &self.request_hash,
                &self.identity,
                &self.owner_id,
            )
            .await
    }
}

enum DynamicWorkflowLeaseClaim {
    Owned(DynamicWorkflowLease),
    AlreadyCompleted,
}

fn dynamic_workflow_lease_key(identity: &ExecutionIdentityV1) -> (String, String) {
    (
        format!("dynamic-workflow:{}", identity.digest),
        identity.digest.clone(),
    )
}

async fn claim_dynamic_workflow_lease(
    ledger: Arc<dyn FlowDecisionLedger>,
    identity: ExecutionIdentityV1,
    lease_ms: u64,
) -> Result<DynamicWorkflowLeaseClaim> {
    identity
        .validate()
        .map_err(|error| anyhow::anyhow!(error))?;
    let (decision_id, request_hash) = dynamic_workflow_lease_key(&identity);
    let owner_id = format!("dynamic-workflow-worker-{}", uuid::Uuid::new_v4());
    let outcome = ledger
        .claim_with_identity(
            &decision_id,
            &request_hash,
            &identity,
            &owner_id,
            dynamic_workflow_now_ms(),
            lease_ms,
        )
        .await
        .context("admit dynamic workflow worker lease")?;
    match outcome {
        FlowDecisionClaimOutcome::Claimed { attempt } => {
            Ok(DynamicWorkflowLeaseClaim::Owned(DynamicWorkflowLease {
                ledger,
                decision_id,
                request_hash,
                identity,
                owner_id,
                lease_ms,
                attempt,
            }))
        }
        FlowDecisionClaimOutcome::Completed => Ok(DynamicWorkflowLeaseClaim::AlreadyCompleted),
        FlowDecisionClaimOutcome::Busy {
            lease_expires_at_ms,
        } => anyhow::bail!("dynamic workflow worker lease is busy until {lease_expires_at_ms}"),
        FlowDecisionClaimOutcome::Conflict => {
            anyhow::bail!("dynamic workflow worker lease identity conflicts with its claim")
        }
    }
}

fn dynamic_workflow_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn is_terminal_workflow_event(event: &FlowEvent) -> bool {
    matches!(
        event,
        FlowEvent::RunCompleted { .. }
            | FlowEvent::RunFailed { .. }
            | FlowEvent::RunCancelled { .. }
            | FlowEvent::RunTimedOut { .. }
            | FlowEvent::RunRetryExhausted { .. }
            | FlowEvent::RunHostShutdown { .. }
            | FlowEvent::RunContinuedAsNew { .. }
    )
}

fn history_is_terminal(history: &[FlowEventEnvelope]) -> bool {
    history
        .last()
        .is_some_and(|envelope| is_terminal_workflow_event(&envelope.event))
}

/// Model-visible tool that executes a dynamic workflow through A3S Flow.
pub struct DynamicWorkflowTool {
    registry: DynamicWorkflowRegistry,
    flow_event_store: Option<Arc<dyn FlowEventStore>>,
    graph_observer: Option<FlowGraphObserver>,
    task_scheduler: Option<Arc<TaskScheduler>>,
    admit_steps_globally: bool,
    runtime_build_compatibility: Option<RuntimeBuildCompatibility>,
    continuation_lease_ledger: Option<Arc<dyn FlowDecisionLedger>>,
    continuation_lease_ms: u64,
    memory_continuation_lease_ledger: Arc<MemoryFlowDecisionLedger>,
}

enum DynamicWorkflowRegistry {
    Standalone(Arc<ToolRegistry>),
    RegistryBound(Weak<ToolRegistry>),
}

impl DynamicWorkflowRegistry {
    fn resolve(&self) -> Option<Arc<ToolRegistry>> {
        match self {
            Self::Standalone(registry) => Some(Arc::clone(registry)),
            Self::RegistryBound(registry) => registry.upgrade(),
        }
    }
}

impl DynamicWorkflowTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry: DynamicWorkflowRegistry::Standalone(registry),
            flow_event_store: None,
            graph_observer: None,
            task_scheduler: None,
            admit_steps_globally: false,
            runtime_build_compatibility: None,
            continuation_lease_ledger: None,
            continuation_lease_ms: DEFAULT_DYNAMIC_WORKFLOW_LEASE_MS,
            memory_continuation_lease_ledger: Arc::new(MemoryFlowDecisionLedger::new()),
        }
    }

    fn new_registry_bound(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry: DynamicWorkflowRegistry::RegistryBound(Arc::downgrade(&registry)),
            flow_event_store: None,
            graph_observer: None,
            task_scheduler: None,
            admit_steps_globally: false,
            runtime_build_compatibility: None,
            continuation_lease_ledger: None,
            continuation_lease_ms: DEFAULT_DYNAMIC_WORKFLOW_LEASE_MS,
            memory_continuation_lease_ledger: Arc::new(MemoryFlowDecisionLedger::new()),
        }
    }

    /// Project committed Flow events into an optional reactive state graph.
    /// A3S Flow remains the workflow execution source of truth.
    pub fn with_graph_observer(mut self, observer: FlowGraphObserver) -> Self {
        self.graph_observer = Some(observer);
        self
    }

    /// Use a host-owned Flow event store for workflow history and control.
    ///
    /// This is the extension point for remote or database-backed hosts. The
    /// store must provide its own durable append/sequence contract; Code does
    /// not mirror its events into a second journal or cache. When omitted,
    /// local workspaces use [`CrossProcessFlowEventStore`] and non-local
    /// contexts use a process-local in-memory store for compatibility.
    pub fn with_flow_event_store(mut self, store: Arc<dyn FlowEventStore>) -> Self {
        self.flow_event_store = Some(store);
        self
    }

    /// Configure optional global admission for direct script-backed Flow
    /// steps. Session-bound registrations should leave this disabled because
    /// the enclosing session operation already owns the global lease.
    pub fn with_task_scheduler(
        mut self,
        scheduler: Arc<TaskScheduler>,
        admit_steps_globally: bool,
    ) -> Self {
        self.task_scheduler = Some(scheduler);
        self.admit_steps_globally = admit_steps_globally;
        self
    }

    /// Fence new and resumed runs to an explicit runtime-build compatibility
    /// set. By default the tool pins new runs to
    /// [`DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID`] and temporarily accepts legacy
    /// unpinned histories; hosts that can replay older builds should add them
    /// to this compatibility set explicitly.
    pub fn with_runtime_build_compatibility(
        mut self,
        compatibility: RuntimeBuildCompatibility,
    ) -> Self {
        self.runtime_build_compatibility = Some(compatibility);
        self
    }

    /// Use a caller-owned durable lease ledger for worker admission.
    ///
    /// The ledger stores only claim metadata and digests; A3S Flow's event
    /// store remains the workflow source of truth. Hosts that run multiple
    /// workers should provide a shared ledger (for example a
    /// [`FileFlowDecisionLedger`]) and choose a lease long enough for one
    /// heartbeat interval. Without an explicit ledger, local workspaces use a
    /// sidecar file ledger and remote/in-memory contexts use a tool-scoped
    /// memory ledger.
    pub fn with_continuation_lease_ledger(
        mut self,
        ledger: Arc<dyn FlowDecisionLedger>,
        lease_ms: u64,
    ) -> Self {
        self.continuation_lease_ledger = Some(ledger);
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }

    /// Change the default worker lease while retaining automatic ledger
    /// selection.
    pub fn with_continuation_lease_ms(mut self, lease_ms: u64) -> Self {
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }

    /// Bind a host control handle to one durable run and its replay inputs.
    ///
    /// The handle is intentionally separate from the model-visible tool call:
    /// a host must provide the exact source and initial input before it can
    /// inspect, drive, or cancel a run. No operation is performed until a
    /// method on the returned handle is awaited.
    pub fn control(
        &self,
        run_id: impl Into<String>,
        source: impl Into<String>,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<DynamicWorkflowControl> {
        let registry = self
            .registry
            .resolve()
            .ok_or_else(|| anyhow::anyhow!("tool registry is closed"))?;
        let run_id = run_id.into();
        if !safe_workflow_run_id(&run_id) {
            anyhow::bail!(
                "dynamic workflow run_id must contain only ASCII letters, numbers, '-' or '_'"
            );
        }
        let source = source.into();
        if source.is_empty() {
            anyhow::bail!("dynamic workflow source must not be empty");
        }
        dynamic_workflow_input_identity(&input)
            .map_err(|error| anyhow::anyhow!("invalid dynamic workflow input: {error}"))?;
        let allowed_tools = default_allowed_tools(&registry);
        Ok(DynamicWorkflowControl {
            registry,
            context: ctx.clone(),
            flow_event_store: self.flow_event_store.clone(),
            run_id,
            source: Arc::from(source),
            input,
            allowed_tools,
            limits: DynamicWorkflowScriptLimits::default(),
            graph_observer: self.graph_observer.clone(),
            task_scheduler: self.task_scheduler.clone(),
            admit_steps_globally: self.admit_steps_globally,
            runtime_build_compatibility: self.runtime_build_compatibility.clone(),
            continuation_lease_ledger: self.continuation_lease_ledger.clone(),
            continuation_lease_ms: self.continuation_lease_ms,
            memory_continuation_lease_ledger: Arc::clone(&self.memory_continuation_lease_ledger),
        })
    }

    async fn continuation_lease_ledger_for_context(
        &self,
        ctx: &ToolContext,
    ) -> Result<Arc<dyn FlowDecisionLedger>> {
        dynamic_workflow_lease_ledger_for_context(
            self.continuation_lease_ledger.as_ref(),
            &self.memory_continuation_lease_ledger,
            ctx,
        )
        .await
    }
}

impl DynamicWorkflowControl {
    /// Return the immutable run id bound to this control handle.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Replace the set of tools available to replayed script steps.
    pub fn with_allowed_tools(mut self, allowed_tools: impl IntoIterator<Item = String>) -> Self {
        self.allowed_tools = sanitize_allowed_tools(allowed_tools);
        self
    }

    /// Replace the bounded script and orchestration limits used during replay.
    pub fn with_limits(mut self, limits: DynamicWorkflowScriptLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Project control-driven Flow events into a graph observer.
    pub fn with_graph_observer(mut self, observer: FlowGraphObserver) -> Self {
        self.graph_observer = Some(observer);
        self
    }

    /// Configure optional global admission for direct script-backed steps.
    pub fn with_task_scheduler(
        mut self,
        scheduler: Arc<TaskScheduler>,
        admit_steps_globally: bool,
    ) -> Self {
        self.task_scheduler = Some(scheduler);
        self.admit_steps_globally = admit_steps_globally;
        self
    }

    /// Set the runtime-build compatibility policy used by control replay.
    pub fn with_runtime_build_compatibility(
        mut self,
        compatibility: RuntimeBuildCompatibility,
    ) -> Self {
        self.runtime_build_compatibility = Some(compatibility);
        self
    }

    /// Use a caller-owned worker lease ledger for control operations.
    pub fn with_continuation_lease_ledger(
        mut self,
        ledger: Arc<dyn FlowDecisionLedger>,
        lease_ms: u64,
    ) -> Self {
        self.continuation_lease_ledger = Some(ledger);
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }

    /// Change the worker lease while retaining automatic ledger selection.
    pub fn with_continuation_lease_ms(mut self, lease_ms: u64) -> Self {
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }
}

async fn dynamic_workflow_lease_ledger_for_context(
    explicit: Option<&Arc<dyn FlowDecisionLedger>>,
    memory: &Arc<MemoryFlowDecisionLedger>,
    ctx: &ToolContext,
) -> Result<Arc<dyn FlowDecisionLedger>> {
    if let Some(ledger) = explicit {
        return Ok(Arc::clone(ledger));
    }
    let Some(root) = ctx.workspace_services.local_root() else {
        let ledger: Arc<dyn FlowDecisionLedger> = memory.clone();
        return Ok(ledger);
    };
    let workflow_root = dynamic_workflow_store_path(root);
    validate_dynamic_workflow_directory(&root.join(".a3s"), ".a3s").await?;
    validate_dynamic_workflow_directory(&workflow_root, ".a3s/workflow").await?;
    let lease_root = workflow_root.join(DYNAMIC_WORKFLOW_LEASE_RELATIVE_PATH);
    validate_dynamic_workflow_directory(&lease_root, ".a3s/workflow/leases").await?;
    Ok(Arc::new(FileFlowDecisionLedger::new(lease_root)))
}

#[async_trait]
impl Tool for DynamicWorkflowTool {
    fn name(&self) -> &str {
        DYNAMIC_WORKFLOW_TOOL
    }

    fn description(&self) -> &str {
        "Run a local dynamic workflow with A3S Flow. The workflow source is a sandboxed JavaScript PTC script that may call allowed ctx tools; A3S Flow records workflow and step history."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "source": {
                    "type": "string",
                    "description": "JavaScript PTC source defining async function run(ctx, inputs). For inputs.kind='workflow', return a Flow command: {type:'complete', output}, {type:'fail', error}, {type:'schedule_step', step_id, step_name, input, retry?}, or {type:'schedule_steps', steps:[...]}. For inputs.kind='step', return the step JSON output. A scheduled step with step_name='task' bypasses QuickJS and calls the host task tool directly with input as its arguments; legacy parallel_task steps remain readable."
                },
                "input": {
                    "type": "object",
                    "description": "Initial workflow input."
                },
                "run_id": {
                    "type": "string",
                    "description": "Optional durable run id. Reusing it with the same source and input is idempotent."
                },
                "allowed_tools": {
                    "type": "array",
                    "description": "Tool names the workflow script may call through ctx. Defaults to all registered tools except program, dynamic_workflow, and the legacy parallel_task alias. Direct task fan-out is blocked inside QuickJS; schedule a host task step instead. Login-registered tools such as runtime are allowed when present.",
                    "items": { "type": "string" }
                },
                "limits": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "timeoutMs": { "type": "integer", "minimum": 1 },
                        "maxToolCalls": { "type": "integer", "minimum": 1 },
                        "maxOutputBytes": { "type": "integer", "minimum": 1 },
                        "maxConcurrentGenerations": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 4,
                            "description": "Optional bounded fan-out for independently session-bound generate_object steps. Providers without session forking remain single-flight."
                        },
                        "maxConcurrentSteps": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 32,
                            "description": "Optional per-workflow limit for concurrently executing Flow step bodies."
                        }
                    }
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(registry) = self.registry.resolve() else {
            return Ok(ToolOutput::error("Tool registry is closed"));
        };
        let Some(source) = args.get("source").and_then(Value::as_str) else {
            return Ok(ToolOutput::error("dynamic_workflow requires source"));
        };
        let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
        let allowed_tools = args
            .get("allowed_tools")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| default_allowed_tools(&registry));
        let limits = args
            .get("limits")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();

        let (runtime_build_id, runtime_build_compatibility) =
            match dynamic_workflow_runtime_configuration(self.runtime_build_compatibility.as_ref())
            {
                Ok(configuration) => configuration,
                Err(error) => {
                    return Ok(ToolOutput::error(format!(
                        "invalid dynamic workflow runtime build identity: {error}"
                    )))
                }
            };
        let source_hash = source_hash(source);
        let base_spec = WorkflowSpec::rust_embedded(
            "a3s-code.dynamic-workflow",
            source_hash.as_str(),
            "ptc",
            "run",
        );

        if ctx.is_cancelled() {
            return Ok(ToolOutput::error(
                "dynamic_workflow was cancelled before admission",
            ));
        }
        let requested_run_id = args.get("run_id").and_then(Value::as_str);
        if requested_run_id.is_some_and(|run_id| !safe_workflow_run_id(run_id)) {
            return Ok(ToolOutput::error(
                "dynamic_workflow run_id must contain only ASCII letters, numbers, '-' or '_'",
            ));
        }
        let run_id = requested_run_id
            .map(ToString::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let store = match self.flow_event_store.clone() {
            Some(store) => Ok(store),
            None => flow_store_for_context(ctx, Some(&run_id)).await,
        };
        let store = match store {
            Ok(store) => store,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let prior_history = match store.list(&run_id).await {
            Ok(history) => history,
            Err(a3s_flow::FlowError::RunNotFound(_)) => Vec::new(),
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        // Preserve the immutable build pin already persisted in a resumed
        // history. A new run is pinned to this worker's current build; a
        // legacy run remains intentionally unpinned during migration. Keeping
        // the exact persisted spec is required because Flow treats the whole
        // workflow definition as the idempotent start contract.
        let (spec, effective_runtime_build_id) = prior_history
            .iter()
            .find_map(|envelope| match &envelope.event {
                FlowEvent::RunCreated { spec, .. } => Some(spec.clone()),
                _ => None,
            })
            .map(|persisted_spec| {
                let runtime_build_id = persisted_spec
                    .runtime_build_id
                    .as_ref()
                    .map(ToString::to_string);
                (persisted_spec, runtime_build_id)
            })
            .unwrap_or_else(|| {
                let spec = base_spec.with_runtime_build(runtime_build_id.clone());
                (spec, Some(runtime_build_id.to_string()))
            });
        let claim_identity = match dynamic_workflow_claim_identity(
            &run_id,
            source,
            &input,
            runtime_build_id.as_str(),
            &prior_history,
        ) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "dynamic workflow continuation identity rejected: {error}"
                )))
            }
        };
        let lease_ledger = match self.continuation_lease_ledger_for_context(ctx).await {
            Ok(ledger) => ledger,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let lease_claim = match claim_dynamic_workflow_lease(
            lease_ledger,
            claim_identity,
            self.continuation_lease_ms,
        )
        .await
        {
            Ok(claim) => claim,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let (lease, mut lease_state) = match lease_claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => (Some(lease), "claimed"),
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                if !history_is_terminal(&prior_history) {
                    return Ok(ToolOutput::error(
                        "dynamic workflow worker claim is completed but its durable run is not terminal",
                    ));
                }
                (None, "already_completed")
            }
        };
        let parent_cancellation = ctx.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = ctx.clone().with_cancellation(child_cancellation.clone());
        let mut runtime = DynamicWorkflowRuntime::new(registry, workflow_context.clone(), source)
            .with_allowed_tools(allowed_tools)
            .with_limits(limits);
        if let Some(scheduler) = &self.task_scheduler {
            runtime = runtime.with_task_scheduler(Arc::clone(scheduler), self.admit_steps_globally);
        }
        if let Some(lease) = lease.as_ref() {
            runtime = runtime.with_continuation_lease(Arc::new(lease.clone()));
        }
        let runtime = Arc::new(runtime);
        let runtime_for_metadata = Arc::clone(&runtime);
        let initial_plan = if prior_history.is_empty() {
            ExecutionPlan::new("dynamic workflow", Complexity::Medium)
        } else {
            dynamic_workflow_execution_plan(&prior_history)
        };
        let mut observers: Vec<Arc<dyn FlowEventObserver>> = Vec::new();
        if let Some(tx) = ctx.agent_event_tx.clone() {
            observers.push(Arc::new(AgentEventFlowObserver::new(
                tx,
                ctx.session_id.clone().unwrap_or_default(),
                initial_plan,
            )));
        }
        if let Some(observer) = &self.graph_observer {
            observers.push(Arc::new(observer.clone()));
        }
        let mut engine_builder = FlowEngine::builder(runtime)
            .with_store(store)
            .with_runtime_build_compatibility(runtime_build_compatibility);
        if !observers.is_empty() {
            engine_builder = engine_builder
                .with_observer(Arc::new(FanoutFlowEventObserver::from_observers(observers)));
        }
        let engine = engine_builder.build();
        let input_for_identity = input.clone();

        let execution_result = if let Some(lease) = lease.as_ref() {
            drive_dynamic_workflow_with_lease(
                &engine,
                &run_id,
                spec,
                input,
                DynamicWorkflowDriveContext {
                    source,
                    input_for_identity: &input_for_identity,
                    runtime_build_id: runtime_build_id.as_str(),
                    workflow_context: &workflow_context,
                    parent_cancellation,
                    child_cancellation,
                },
                lease,
            )
            .await
        } else {
            let started_run_id = match engine.start_with_id(&run_id, spec, input).await {
                Ok(run_id) => run_id,
                Err(error) => return Ok(ToolOutput::error(error.to_string())),
            };
            if started_run_id != run_id {
                return Ok(ToolOutput::error(format!(
                    "dynamic workflow engine returned unexpected run id `{started_run_id}` for `{run_id}`"
                )));
            }
            let snapshot = match drive_inline_retries(&engine, &run_id, &workflow_context).await {
                Ok(snapshot) => snapshot,
                Err(error) => return Ok(ToolOutput::error(error.to_string())),
            };
            collect_dynamic_workflow_execution(
                &engine,
                &run_id,
                snapshot,
                source,
                &input_for_identity,
                runtime_build_id.as_str(),
            )
            .await
        };
        let execution = match execution_result {
            Ok(execution) => execution,
            Err(err) => return Ok(ToolOutput::error(err.to_string())),
        };
        let DynamicWorkflowExecution {
            snapshot,
            history,
            continuation_identity,
        } = execution;
        if lease.is_some() {
            lease_state = if snapshot.status.is_terminal() {
                "completed"
            } else {
                "released"
            };
        }

        let output = match &snapshot.output {
            Some(output) => {
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string())
            }
            None => snapshot
                .error
                .clone()
                .unwrap_or_else(|| format!("workflow status: {:?}", snapshot.status)),
        };

        let status = snapshot.status;
        let plan = dynamic_workflow_execution_plan(&history);
        let plan_identity = match plan.definition_identity() {
            Ok(identity) => identity,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let metadata = json!({
            "dynamic_workflow": {
                "run_id": run_id,
                "status": format!("{:?}", snapshot.status),
                "last_sequence": snapshot.last_sequence,
                "source_hash": source_hash,
                "runtime_build_id": effective_runtime_build_id,
                "snapshot": snapshot,
                "history": history,
                "plan": plan,
                "plan_identity": plan_identity,
                "continuation_identity": continuation_identity,
                "worker_lease": {
                    "state": lease_state,
                    "attempt": lease.as_ref().map(|lease| lease.attempt),
                },
                "admission": runtime_for_metadata.admission_stats(),
            }
        });
        let output = match status {
            WorkflowRunStatus::Completed => ToolOutput::success(output),
            WorkflowRunStatus::Failed | WorkflowRunStatus::Cancelled => ToolOutput::error(output),
            _ => ToolOutput::error(format!(
                "dynamic_workflow ended without a terminal result: {status:?}; {output}"
            )),
        };

        Ok(output.with_metadata(metadata))
    }
}

struct DynamicWorkflowControlPreparation {
    store: Arc<dyn FlowEventStore>,
    prior_history: Vec<FlowEventEnvelope>,
    spec: WorkflowSpec,
    effective_runtime_build_id: Option<String>,
    runtime_build_id: RuntimeBuildId,
    runtime_build_compatibility: RuntimeBuildCompatibility,
    claim_identity: ExecutionIdentityV1,
    lease_ledger: Arc<dyn FlowDecisionLedger>,
}

fn dynamic_workflow_runtime_configuration(
    configured: Option<&RuntimeBuildCompatibility>,
) -> Result<(RuntimeBuildId, RuntimeBuildCompatibility)> {
    let runtime_build_id =
        match configured {
            Some(compatibility) => compatibility.current_build_id().clone(),
            None => RuntimeBuildId::new(DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID.to_string()).map_err(
                |error| anyhow::anyhow!("invalid dynamic workflow runtime build identity: {error}"),
            )?,
        };
    let compatibility = configured.cloned().unwrap_or_else(|| {
        RuntimeBuildCompatibility::new(runtime_build_id.clone()).accept_unpinned()
    });
    Ok((runtime_build_id, compatibility))
}

impl DynamicWorkflowControl {
    async fn prepare(&self, allow_missing: bool) -> Result<DynamicWorkflowControlPreparation> {
        let (runtime_build_id, runtime_build_compatibility) =
            dynamic_workflow_runtime_configuration(self.runtime_build_compatibility.as_ref())?;
        let source_hash = source_hash(self.source.as_ref());
        let base_spec = WorkflowSpec::rust_embedded(
            "a3s-code.dynamic-workflow",
            source_hash.as_str(),
            "ptc",
            "run",
        );
        let store = match self.flow_event_store.clone() {
            Some(store) => store,
            None => flow_store_for_context(&self.context, Some(&self.run_id)).await?,
        };
        let prior_history = match store.list(&self.run_id).await {
            Ok(history) => history,
            Err(a3s_flow::FlowError::RunNotFound(_)) if allow_missing => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        if !allow_missing && prior_history.is_empty() {
            return Err(a3s_flow::FlowError::RunNotFound(self.run_id.clone()).into());
        }
        let (spec, effective_runtime_build_id) = prior_history
            .iter()
            .find_map(|envelope| match &envelope.event {
                FlowEvent::RunCreated { spec, .. } => Some(spec.clone()),
                _ => None,
            })
            .map(|persisted_spec| {
                let runtime_build_id = persisted_spec
                    .runtime_build_id
                    .as_ref()
                    .map(ToString::to_string);
                (persisted_spec, runtime_build_id)
            })
            .unwrap_or_else(|| {
                let spec = base_spec.with_runtime_build(runtime_build_id.clone());
                (spec, Some(runtime_build_id.to_string()))
            });
        let claim_identity = dynamic_workflow_claim_identity(
            &self.run_id,
            self.source.as_ref(),
            &self.input,
            runtime_build_id.as_str(),
            &prior_history,
        )
        .map_err(|error| {
            anyhow::anyhow!("dynamic workflow continuation identity rejected: {error}")
        })?;
        let lease_ledger = dynamic_workflow_lease_ledger_for_context(
            self.continuation_lease_ledger.as_ref(),
            &self.memory_continuation_lease_ledger,
            &self.context,
        )
        .await?;
        Ok(DynamicWorkflowControlPreparation {
            store,
            prior_history,
            spec,
            effective_runtime_build_id,
            runtime_build_id,
            runtime_build_compatibility,
            claim_identity,
            lease_ledger,
        })
    }

    fn build_engine(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
        workflow_context: ToolContext,
        lease: Option<&DynamicWorkflowLease>,
    ) -> FlowEngine {
        let mut runtime = DynamicWorkflowRuntime::new(
            Arc::clone(&self.registry),
            workflow_context,
            self.source.as_ref(),
        )
        .with_allowed_tools(self.allowed_tools.clone())
        .with_limits(self.limits.clone());
        if let Some(scheduler) = &self.task_scheduler {
            runtime = runtime.with_task_scheduler(Arc::clone(scheduler), self.admit_steps_globally);
        }
        if let Some(lease) = lease {
            runtime = runtime.with_continuation_lease(Arc::new(lease.clone()));
        }
        let runtime = Arc::new(runtime);
        let initial_plan = if preparation.prior_history.is_empty() {
            ExecutionPlan::new("dynamic workflow", Complexity::Medium)
        } else {
            dynamic_workflow_execution_plan(&preparation.prior_history)
        };
        let mut observers: Vec<Arc<dyn FlowEventObserver>> = Vec::new();
        if let Some(tx) = self.context.agent_event_tx.clone() {
            observers.push(Arc::new(AgentEventFlowObserver::new(
                tx,
                self.context.session_id.clone().unwrap_or_default(),
                initial_plan,
            )));
        }
        if let Some(observer) = &self.graph_observer {
            observers.push(Arc::new(observer.clone()));
        }
        let mut builder = FlowEngine::builder(runtime)
            .with_store(Arc::clone(&preparation.store))
            .with_runtime_build_compatibility(preparation.runtime_build_compatibility.clone());
        if !observers.is_empty() {
            builder =
                builder.with_observer(Arc::new(FanoutFlowEventObserver::from_observers(observers)));
        }
        builder.build()
    }

    async fn read_state(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
        engine: &FlowEngine,
    ) -> Result<(WorkflowRunSnapshot, Vec<FlowEventEnvelope>)> {
        for _ in 0..3 {
            // Flow exposes history and its projection as separate reads. Read
            // the journal on both sides of the projection so a concurrent
            // append cannot be mistaken for an atomic snapshot; retry a
            // bounded number of times when the sequences disagree.
            let before_history = engine.history(&self.run_id).await?;
            let snapshot = engine.snapshot(&self.run_id).await?;
            let history = engine.history(&self.run_id).await?;
            let before_sequence = before_history
                .last()
                .map(|event| event.sequence)
                .unwrap_or(0);
            let last_sequence = history.last().map(|event| event.sequence).unwrap_or(0);
            if before_sequence == last_sequence && snapshot.last_sequence == last_sequence {
                return Ok((snapshot, history));
            }
        }
        let expected = preparation
            .prior_history
            .last()
            .map(|event| event.sequence)
            .unwrap_or(0);
        anyhow::bail!(
            "dynamic workflow state changed repeatedly while inspecting run `{}` (initial sequence {expected})",
            self.run_id
        )
    }

    async fn summarize(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
        snapshot: WorkflowRunSnapshot,
        history: Vec<FlowEventEnvelope>,
    ) -> Result<DynamicWorkflowControlSnapshot> {
        let continuation_identity = dynamic_workflow_continuation_identity(
            &self.run_id,
            self.source.as_ref(),
            &self.input,
            preparation.runtime_build_id.as_str(),
            &history,
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "dynamic workflow continuation identity rejected during inspection: {error}"
            )
        })?;
        let plan = dynamic_workflow_execution_plan(&history);
        let plan_identity = plan.definition_identity()?;
        let (decision_id, request_hash) = dynamic_workflow_lease_key(&preparation.claim_identity);
        let worker_lease = preparation
            .lease_ledger
            .inspect_with_identity(&decision_id, &request_hash, &preparation.claim_identity)
            .await?;
        let completed_steps = snapshot
            .steps
            .values()
            .filter(|step| step.status == StepStatus::Completed)
            .count();
        let open_steps = snapshot
            .steps
            .values()
            .filter(|step| {
                !matches!(
                    step.status,
                    StepStatus::Completed | StepStatus::Failed | StepStatus::Cancelled
                )
            })
            .count();
        Ok(DynamicWorkflowControlSnapshot {
            run_id: self.run_id.clone(),
            status: snapshot.status,
            last_sequence: snapshot.last_sequence,
            step_count: snapshot.steps.len(),
            completed_steps,
            open_steps,
            cancellation_requested: snapshot.cancellation.is_some(),
            continuation_identity,
            plan_identity,
            runtime_build_id: preparation.effective_runtime_build_id.clone(),
            worker_lease,
        })
    }

    async fn claim(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
    ) -> Result<DynamicWorkflowLeaseClaim> {
        claim_dynamic_workflow_lease(
            Arc::clone(&preparation.lease_ledger),
            preparation.claim_identity.clone(),
            self.continuation_lease_ms,
        )
        .await
    }

    /// Inspect the durable run and return a bounded, digest-only projection.
    pub async fn inspect(&self) -> Result<DynamicWorkflowControlSnapshot> {
        let preparation = self.prepare(false).await?;
        let engine = self.build_engine(&preparation, self.context.clone(), None);
        let (snapshot, history) = self.read_state(&preparation, &engine).await?;
        self.summarize(&preparation, snapshot, history).await
    }

    /// Read the complete durable Flow history for trusted host diagnostics.
    ///
    /// Unlike [`Self::inspect`], this method intentionally returns persisted
    /// input and step output values. Callers should apply their own redaction
    /// policy before exposing the result outside a trusted control plane.
    pub async fn history(&self) -> Result<Vec<FlowEventEnvelope>> {
        let preparation = self.prepare(false).await?;
        let history = preparation.store.list(&self.run_id).await?;
        dynamic_workflow_continuation_identity(
            &self.run_id,
            self.source.as_ref(),
            &self.input,
            preparation.runtime_build_id.as_str(),
            &history,
        )
        .map_err(|error| anyhow::anyhow!("dynamic workflow history rejected: {error}"))?;
        Ok(history)
    }

    /// Request cleanup-aware durable cancellation and settle the worker lease.
    ///
    /// A live worker keeps ownership and causes this method to return a busy
    /// error; the caller can retry after the lease expires or the worker
    /// releases it. This preserves one executor for side-effecting steps.
    pub async fn request_cancellation(
        &self,
        reason: Option<String>,
    ) -> Result<DynamicWorkflowControlSnapshot> {
        validate_dynamic_workflow_control_reason(reason.as_deref())?;
        if self.context.is_cancelled() {
            anyhow::bail!("dynamic workflow control was cancelled before admission");
        }
        let preparation = self.prepare(false).await?;
        let claim = self.claim(&preparation).await?;
        let lease = match claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => lease,
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                let engine = self.build_engine(&preparation, self.context.clone(), None);
                let (snapshot, history) = self.read_state(&preparation, &engine).await?;
                if !snapshot.status.is_terminal() {
                    anyhow::bail!(
                        "dynamic workflow worker claim is completed but its durable run is not terminal"
                    );
                }
                return self.summarize(&preparation, snapshot, history).await;
            }
        };
        let parent_cancellation = self.context.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = self
            .context
            .clone()
            .with_cancellation(child_cancellation.clone());
        let engine = self.build_engine(&preparation, workflow_context.clone(), Some(&lease));
        let future = engine.request_cancellation(&self.run_id, CancellationRequest::new(reason));
        let execution = drive_dynamic_workflow_control_future(
            &engine,
            &self.run_id,
            DynamicWorkflowDriveContext {
                source: self.source.as_ref(),
                input_for_identity: &self.input,
                runtime_build_id: preparation.runtime_build_id.as_str(),
                workflow_context: &workflow_context,
                parent_cancellation,
                child_cancellation,
            },
            &lease,
            future,
        )
        .await?;
        self.summarize(&preparation, execution.snapshot, execution.history)
            .await
    }

    /// Immediately terminate the run through Flow's durable cancellation
    /// transition while still fencing the worker claim.
    pub async fn force_cancel(
        &self,
        reason: Option<String>,
    ) -> Result<DynamicWorkflowControlSnapshot> {
        validate_dynamic_workflow_control_reason(reason.as_deref())?;
        if self.context.is_cancelled() {
            anyhow::bail!("dynamic workflow control was cancelled before admission");
        }
        let preparation = self.prepare(false).await?;
        let claim = self.claim(&preparation).await?;
        let lease = match claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => lease,
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                let engine = self.build_engine(&preparation, self.context.clone(), None);
                let (snapshot, history) = self.read_state(&preparation, &engine).await?;
                if !snapshot.status.is_terminal() {
                    anyhow::bail!(
                        "dynamic workflow worker claim is completed but its durable run is not terminal"
                    );
                }
                return self.summarize(&preparation, snapshot, history).await;
            }
        };
        let parent_cancellation = self.context.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = self
            .context
            .clone()
            .with_cancellation(child_cancellation.clone());
        let engine = self.build_engine(&preparation, workflow_context.clone(), Some(&lease));
        let future = async {
            engine.force_cancel(&self.run_id, reason).await?;
            engine.snapshot(&self.run_id).await
        };
        let execution = drive_dynamic_workflow_control_future(
            &engine,
            &self.run_id,
            DynamicWorkflowDriveContext {
                source: self.source.as_ref(),
                input_for_identity: &self.input,
                runtime_build_id: preparation.runtime_build_id.as_str(),
                workflow_context: &workflow_context,
                parent_cancellation,
                child_cancellation,
            },
            &lease,
            future,
        )
        .await?;
        self.summarize(&preparation, execution.snapshot, execution.history)
            .await
    }

    /// Resume or start the bound run under the same worker fencing contract as
    /// the model-visible dynamic workflow tool.
    pub async fn drive(&self) -> Result<DynamicWorkflowControlSnapshot> {
        if self.context.is_cancelled() {
            anyhow::bail!("dynamic workflow control was cancelled before admission");
        }
        let preparation = self.prepare(true).await?;
        let claim = self.claim(&preparation).await?;
        let lease = match claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => lease,
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                let engine = self.build_engine(&preparation, self.context.clone(), None);
                let (snapshot, history) = self.read_state(&preparation, &engine).await?;
                if !snapshot.status.is_terminal() {
                    anyhow::bail!(
                        "dynamic workflow worker claim is completed but its durable run is not terminal"
                    );
                }
                return self.summarize(&preparation, snapshot, history).await;
            }
        };
        let parent_cancellation = self.context.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = self
            .context
            .clone()
            .with_cancellation(child_cancellation.clone());
        let engine = self.build_engine(&preparation, workflow_context.clone(), Some(&lease));
        let execution = drive_dynamic_workflow_with_lease(
            &engine,
            &self.run_id,
            preparation.spec.clone(),
            self.input.clone(),
            DynamicWorkflowDriveContext {
                source: self.source.as_ref(),
                input_for_identity: &self.input,
                runtime_build_id: preparation.runtime_build_id.as_str(),
                workflow_context: &workflow_context,
                parent_cancellation,
                child_cancellation,
            },
            &lease,
        )
        .await?;
        self.summarize(&preparation, execution.snapshot, execution.history)
            .await
    }
}

struct DynamicWorkflowExecution {
    snapshot: WorkflowRunSnapshot,
    history: Vec<FlowEventEnvelope>,
    continuation_identity: ExecutionIdentityV1,
}

struct DynamicWorkflowDriveContext<'a> {
    source: &'a str,
    input_for_identity: &'a Value,
    runtime_build_id: &'a str,
    workflow_context: &'a ToolContext,
    parent_cancellation: CancellationToken,
    child_cancellation: CancellationToken,
}

async fn collect_dynamic_workflow_execution(
    engine: &FlowEngine,
    run_id: &str,
    snapshot: WorkflowRunSnapshot,
    source: &str,
    input: &Value,
    runtime_build_id: &str,
) -> Result<DynamicWorkflowExecution> {
    let history = engine.history(run_id).await?;
    let continuation_identity =
        dynamic_workflow_continuation_identity(run_id, source, input, runtime_build_id, &history)
            .map_err(|error| {
            anyhow::anyhow!("dynamic workflow continuation identity rejected after replay: {error}")
        })?;
    Ok(DynamicWorkflowExecution {
        snapshot,
        history,
        continuation_identity,
    })
}

fn validate_dynamic_workflow_control_reason(reason: Option<&str>) -> Result<()> {
    let Some(reason) = reason else {
        return Ok(());
    };
    if reason.len() > MAX_DYNAMIC_WORKFLOW_CONTROL_REASON_BYTES {
        anyhow::bail!(
            "dynamic workflow cancellation reason exceeds {} bytes",
            MAX_DYNAMIC_WORKFLOW_CONTROL_REASON_BYTES
        );
    }
    if reason.contains('\0') {
        anyhow::bail!("dynamic workflow cancellation reason contains a NUL byte");
    }
    Ok(())
}

async fn drive_dynamic_workflow_control_future<F>(
    engine: &FlowEngine,
    run_id: &str,
    drive_context: DynamicWorkflowDriveContext<'_>,
    lease: &DynamicWorkflowLease,
    future: F,
) -> Result<DynamicWorkflowExecution>
where
    F: Future<Output = a3s_flow::Result<WorkflowRunSnapshot>>,
{
    let DynamicWorkflowDriveContext {
        source,
        input_for_identity,
        runtime_build_id,
        workflow_context: _workflow_context,
        parent_cancellation,
        child_cancellation,
    } = drive_context;
    tokio::pin!(future);

    async fn collect_result(
        engine: &FlowEngine,
        run_id: &str,
        result: a3s_flow::Result<WorkflowRunSnapshot>,
        source: &str,
        input: &Value,
        runtime_build_id: &str,
    ) -> Result<DynamicWorkflowExecution> {
        let snapshot = result?;
        collect_dynamic_workflow_execution(
            engine,
            run_id,
            snapshot,
            source,
            input,
            runtime_build_id,
        )
        .await
    }

    let heartbeat_period = Duration::from_millis((lease.lease_ms / 3).max(1));
    let first_heartbeat = tokio::time::Instant::now() + heartbeat_period;
    let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_period);
    loop {
        tokio::select! {
            biased;
            result = &mut future => {
                let result = collect_result(
                    engine,
                    run_id,
                    result,
                    source,
                    input_for_identity,
                    runtime_build_id,
                ).await;
                return settle_dynamic_workflow_lease(lease, result).await;
            }
            _ = parent_cancellation.cancelled() => {
                child_cancellation.cancel();
                match tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut future).await {
                    Ok(result) => {
                        let result = collect_result(
                            engine,
                            run_id,
                            result,
                            source,
                            input_for_identity,
                            runtime_build_id,
                        ).await;
                        match settle_dynamic_workflow_lease(lease, result).await {
                            Ok(execution) if execution.snapshot.status.is_terminal() => {
                                return Ok(execution)
                            }
                            Ok(_) => anyhow::bail!("dynamic workflow control cancelled by its parent"),
                            Err(error) => return Err(error).context("settle dynamic workflow control cancellation"),
                        }
                    }
                    Err(_) => anyhow::bail!(
                        "dynamic workflow control cancellation did not settle within {} seconds; worker lease remains fenced until expiry",
                        MAX_DYNAMIC_WORKFLOW_SETTLE.as_secs()
                    ),
                }
            }
            _ = heartbeat.tick() => {
                match lease.renew().await {
                    Ok(true) => {}
                    Ok(false) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut future)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        anyhow::bail!("dynamic workflow control worker lease was lost before completion");
                    }
                    Err(error) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut future)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        return Err(error).context("renew dynamic workflow control worker lease");
                    }
                }
            }
        }
    }
}

async fn settle_dynamic_workflow_lease(
    lease: &DynamicWorkflowLease,
    result: Result<DynamicWorkflowExecution>,
) -> Result<DynamicWorkflowExecution> {
    match result {
        Ok(execution) if execution.snapshot.status.is_terminal() => {
            lease
                .complete()
                .await
                .context("complete dynamic workflow worker lease")?;
            Ok(execution)
        }
        Ok(execution) => {
            lease
                .release()
                .await
                .context("release suspended dynamic workflow worker lease")?;
            Ok(execution)
        }
        Err(error) => {
            if let Err(release_error) = lease.release().await {
                tracing::warn!(
                    error = %release_error,
                    "failed to release dynamic workflow worker lease after execution error"
                );
            }
            Err(error)
        }
    }
}

async fn drive_dynamic_workflow_with_lease(
    engine: &FlowEngine,
    run_id: &str,
    spec: WorkflowSpec,
    input: Value,
    drive_context: DynamicWorkflowDriveContext<'_>,
    lease: &DynamicWorkflowLease,
) -> Result<DynamicWorkflowExecution> {
    let DynamicWorkflowDriveContext {
        source,
        input_for_identity,
        runtime_build_id,
        workflow_context,
        parent_cancellation,
        child_cancellation,
    } = drive_context;
    let execution = async {
        let started_run_id = engine.start_with_id(run_id, spec, input).await?;
        if started_run_id != run_id {
            anyhow::bail!(
                "dynamic workflow engine returned unexpected run id `{started_run_id}` for `{run_id}`"
            );
        }
        let snapshot = drive_inline_retries(engine, run_id, workflow_context).await?;
        collect_dynamic_workflow_execution(
            engine,
            run_id,
            snapshot,
            source,
            input_for_identity,
            runtime_build_id,
        )
        .await
    };
    tokio::pin!(execution);

    let heartbeat_period = Duration::from_millis((lease.lease_ms / 3).max(1));
    let first_heartbeat = tokio::time::Instant::now() + heartbeat_period;
    let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_period);
    loop {
        tokio::select! {
            biased;
            result = &mut execution => {
                return settle_dynamic_workflow_lease(lease, result).await;
            }
            _ = parent_cancellation.cancelled() => {
                child_cancellation.cancel();
                match tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut execution).await {
                    Ok(result) => {
                        match settle_dynamic_workflow_lease(lease, result).await {
                            Ok(execution) if execution.snapshot.status.is_terminal() => {
                                return Ok(execution)
                            }
                            Ok(_) => anyhow::bail!("dynamic workflow cancelled by its parent"),
                            Err(error) => return Err(error).context("settle dynamic workflow cancellation"),
                        }
                    }
                    Err(_) => anyhow::bail!(
                        "dynamic workflow cancellation did not settle within {} seconds; worker lease remains fenced until expiry",
                        MAX_DYNAMIC_WORKFLOW_SETTLE.as_secs()
                    ),
                }
            }
            _ = heartbeat.tick() => {
                match lease.renew().await {
                    Ok(true) => {}
                    Ok(false) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut execution)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        anyhow::bail!("dynamic workflow worker lease was lost before execution completed");
                    }
                    Err(error) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut execution)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        return Err(error).context("renew dynamic workflow worker lease");
                    }
                }
            }
        }
    }
}

/// Drive short, persisted step retries inside the originating tool call.
///
/// A3S Flow deliberately suspends at a delayed retry boundary. Interactive
/// waits and hooks must remain suspended for an external host, but a bounded
/// retry delay is ordinary fault recovery: returning it as a terminal tool
/// error forces every caller to reimplement the scheduler and previously made
/// DeepResearch abandon its event-sourced run. Retry attempts and their delay
/// remain authoritative in the Flow journal; this helper only waits for the
/// due time and asks the engine to replay the same run.
async fn drive_inline_retries(
    engine: &FlowEngine,
    run_id: &str,
    ctx: &ToolContext,
) -> Result<WorkflowRunSnapshot> {
    for _ in 0..MAX_INLINE_RETRY_RESUMES {
        let snapshot = engine.snapshot(run_id).await?;
        if snapshot.status.is_terminal() {
            return Ok(snapshot);
        }
        let Some(retry_after) = snapshot
            .steps
            .values()
            .filter(|step| step.status == StepStatus::Pending)
            .filter_map(|step| step.retry_after)
            .min()
        else {
            return Ok(snapshot);
        };
        let delay = retry_after
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or_default();
        if delay > MAX_INLINE_RETRY_DELAY {
            return Ok(snapshot);
        }
        let cancellation = ctx.cancellation_token();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                anyhow::bail!("dynamic_workflow cancelled while waiting for a scheduled retry");
            }
            _ = tokio::time::sleep(delay) => {}
        }
        engine.drive(run_id).await?;
    }
    engine.snapshot(run_id).await.map_err(Into::into)
}

pub fn register_dynamic_workflow(registry: &Arc<ToolRegistry>) {
    registry.register(Arc::new(DynamicWorkflowTool::new_registry_bound(
        Arc::clone(registry),
    )));
}

/// Register a dynamic workflow Tool against a host-owned Flow event store.
///
/// The registry-bound Tool keeps only a weak registry reference, so adding a
/// durable store does not create a registry/tool ownership cycle. Use this for
/// database-backed or remote adapters that must be shared by model-visible
/// execution and [`DynamicWorkflowTool::control`].
pub fn register_dynamic_workflow_with_event_store(
    registry: &Arc<ToolRegistry>,
    store: Arc<dyn FlowEventStore>,
) {
    registry.register(Arc::new(
        DynamicWorkflowTool::new_registry_bound(Arc::clone(registry)).with_flow_event_store(store),
    ));
}

/// Register a dynamic workflow tool with an explicit scheduler policy.
///
/// This is intended for hosts that construct a Flow runtime outside the
/// normal AgentSession operation boundary. AgentSession registrations should
/// use [`register_dynamic_workflow`] so the enclosing run lease remains the
/// single global admission boundary.
pub fn register_dynamic_workflow_with_scheduler(
    registry: &Arc<ToolRegistry>,
    scheduler: Arc<TaskScheduler>,
    admit_steps_globally: bool,
) {
    registry.register(Arc::new(
        DynamicWorkflowTool::new_registry_bound(Arc::clone(registry))
            .with_task_scheduler(scheduler, admit_steps_globally),
    ));
}

async fn flow_store_for_context(
    ctx: &ToolContext,
    requested_run_id: Option<&str>,
) -> Result<Arc<dyn FlowEventStore>> {
    match ctx.workspace_services.local_root() {
        Some(root) => {
            let store = dynamic_workflow_store_path(root);
            validate_dynamic_workflow_directory(&root.join(".a3s"), ".a3s").await?;
            validate_dynamic_workflow_directory(&store, ".a3s/workflow").await?;
            if let Some(run_id) = requested_run_id.filter(|run_id| safe_workflow_run_id(run_id)) {
                validate_dynamic_workflow_log(&store.join(format!("{run_id}.jsonl"))).await?;
            }
            Ok(Arc::new(CrossProcessFlowEventStore::new(store)))
        }
        None => Ok(Arc::new(InMemoryEventStore::new())),
    }
}

async fn validate_dynamic_workflow_directory(path: &Path, label: &str) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("refusing to use symlinked dynamic workflow directory {label}")
        }
        Ok(metadata) if !metadata.is_dir() => {
            anyhow::bail!("dynamic workflow path {label} exists but is not a directory")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect dynamic workflow path {label}")),
    }
}

async fn validate_dynamic_workflow_log(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => anyhow::bail!(
            "refusing to read or append symlinked dynamic workflow history {}",
            path.display()
        ),
        Ok(metadata) if !metadata.is_file() => anyhow::bail!(
            "dynamic workflow history path {} exists but is not a file",
            path.display()
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("inspect dynamic workflow history {}", path.display())),
    }
}

fn safe_workflow_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

struct PayloadBuilder {
    value: Map<String, Value>,
}

impl PayloadBuilder {
    fn with(mut self, key: &str, value: impl Serialize) -> Self {
        self.value.insert(
            key.to_string(),
            serde_json::to_value(value).unwrap_or(Value::Null),
        );
        self
    }

    fn into_value(self) -> Value {
        Value::Object(self.value)
    }
}

fn invocation_payload(kind: &str, run_id: &str, history: &[FlowEventEnvelope]) -> PayloadBuilder {
    let mut value = Map::new();
    value.insert("kind".to_string(), json!(kind));
    value.insert("run_id".to_string(), json!(run_id));
    value.insert("history".to_string(), json!(history));
    value.insert("step_outputs".to_string(), completed_step_outputs(history));
    value.insert("step_failures".to_string(), failed_step_outputs(history));
    PayloadBuilder { value }
}

fn completed_step_outputs(history: &[FlowEventEnvelope]) -> Value {
    let mut outputs = Map::new();
    for envelope in history {
        if let FlowEvent::StepCompleted { step_id, output } = &envelope.event {
            outputs.insert(step_id.clone(), output.clone());
        }
    }
    Value::Object(outputs)
}

fn failed_step_outputs(history: &[FlowEventEnvelope]) -> Value {
    let mut outputs = Map::new();
    for envelope in history {
        if let FlowEvent::StepFailed {
            step_id,
            attempt,
            error,
        } = &envelope.event
        {
            outputs.insert(
                step_id.clone(),
                json!({
                    "attempt": attempt,
                    "error": error,
                }),
            );
        }
    }
    Value::Object(outputs)
}

fn script_result(result: &ToolResult) -> a3s_flow::Result<Value> {
    result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("script_result"))
        .cloned()
        .ok_or_else(|| {
            a3s_flow::FlowError::Runtime(
                "PTC program result did not include script_result metadata".to_string(),
            )
        })
}

fn default_allowed_tools(registry: &ToolRegistry) -> Vec<String> {
    sanitize_allowed_tools(registry.list())
}

fn sanitize_allowed_tools(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut tools = items.into_iter().collect::<BTreeSet<_>>();
    tools.remove(PROGRAM_TOOL);
    tools.remove(DYNAMIC_WORKFLOW_TOOL);
    tools.remove(PARALLEL_TASK_TOOL);
    tools.into_iter().collect()
}

fn source_hash(source: &str) -> String {
    sha256::digest(source.as_bytes())
}

#[cfg(test)]
#[path = "dynamic_workflow/tests.rs"]
mod tests;
