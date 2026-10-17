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
use crate::task_scheduler::{
    TaskLease, TaskPriority as SchedulerTaskPriority, TaskScheduler, TaskSchedulerQuota,
};
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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

mod control;
mod drive;
mod identity;
mod lease;
mod observer;
mod store;
mod tool;
use control::*;
use drive::*;
use identity::*;
pub use identity::{
    dynamic_workflow_claim_identity, dynamic_workflow_continuation_identity,
    dynamic_workflow_execution_plan, dynamic_workflow_step_identity,
};
use lease::*;
use observer::*;
pub use store::{
    dynamic_workflow_store_path, recover_dynamic_workflow_step_output, CrossProcessFlowEventStore,
};
pub use tool::DynamicWorkflowTool;
use tool::*;

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
/// Soft threshold: below this, step input is inlined into the identity
/// derivation JSON. Above it, only a SHA-256 digest is mixed in so identity
/// stays digest-only without rejecting legitimate DeepResearch selector
/// packets (up to 96 KiB prompts plus schema wrappers).
const MAX_FLOW_STEP_INPUT_INLINE_BYTES: usize = 64 * 1024;
/// Hard ceiling for hashing step input during identity derivation (DoS bound).
/// Execution/tool limits remain separately enforced by workflow `limits`.
const MAX_FLOW_STEP_INPUT_BYTES: usize = 512 * 1024;
const MAX_DYNAMIC_WORKFLOW_INPUT_BYTES: usize = 128 * 1024;
const DEFAULT_DYNAMIC_WORKFLOW_LEASE_MS: u64 = 30_000;
const MAX_DYNAMIC_WORKFLOW_SETTLE: Duration = Duration::from_secs(5);
const MAX_DYNAMIC_WORKFLOW_CONTROL_REASON_BYTES: usize = 4 * 1024;
const DYNAMIC_WORKFLOW_LEASE_RELATIVE_PATH: &str = "leases";

fn dynamic_workflow_scheduler_quota_limit(limits: &DynamicWorkflowScriptLimits) -> usize {
    limits
        .max_concurrent_steps
        .unwrap_or(DEFAULT_MAX_CONCURRENT_STEPS)
        .clamp(1, MAX_MAX_CONCURRENT_STEPS)
}

fn provider_quota_for_context(
    context: &ToolContext,
) -> Option<crate::task_scheduler::TaskSchedulerQuota> {
    if context
        .model_generation_admission()
        .is_some_and(|admission| admission.has_scheduler_quota())
    {
        // The session-owned generation gate already reserves this exact pool
        // through the shared scheduler. Holding it again on the enclosing Flow
        // step would make a single-flight provider recursively wait on itself.
        return None;
    }
    let client = context.llm_client()?;
    let pool = client.model_generation_pool()?;
    crate::task_scheduler::TaskSchedulerQuota::new(
        pool.identity.clone(),
        pool.max_concurrency().get(),
    )
    .ok()
}

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

/// Bounded cumulative worker-claim diagnostics for one dynamic-workflow tool
/// instance.  The counters retain no run ids, labels, source, input, output,
/// or owner tokens; the durable Flow ledger remains the authority for the
/// per-run attempt number and lease state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWorkflowHealthSnapshot {
    /// Number of worker-claim attempts observed by this process.
    pub claim_attempts: u64,
    /// Number of claims that acquired a worker lease, including takeovers.
    pub claims: u64,
    /// Number of claims whose durable attempt number was greater than one.
    pub takeovers: u64,
    /// Number of attempts that found an already-completed claim.
    pub already_completed: u64,
    /// Number of attempts rejected because another worker is live.
    pub busy: u64,
    /// Number of identity/hash conflicts.
    pub conflicts: u64,
    /// Successful lease heartbeats.
    pub renewals: u64,
    /// Lease renewals or terminal operations fenced by a lost lease.
    pub lease_lost: u64,
    /// Successful terminal claim completions.
    pub completions: u64,
    /// Successful releases of non-terminal claims.
    pub releases: u64,
    /// Parent/control cancellation observations.
    pub cancellations: u64,
    /// Ledger or execution-boundary failures.
    pub failures: u64,
    /// Claims currently waiting on a ledger operation.
    pub in_flight: u64,
}

#[derive(Default)]
struct DynamicWorkflowMetrics {
    claim_attempts: AtomicU64,
    claims: AtomicU64,
    takeovers: AtomicU64,
    already_completed: AtomicU64,
    busy: AtomicU64,
    conflicts: AtomicU64,
    renewals: AtomicU64,
    lease_lost: AtomicU64,
    completions: AtomicU64,
    releases: AtomicU64,
    cancellations: AtomicU64,
    failures: AtomicU64,
    in_flight: AtomicU64,
}

impl DynamicWorkflowMetrics {
    fn snapshot(&self) -> DynamicWorkflowHealthSnapshot {
        DynamicWorkflowHealthSnapshot {
            claim_attempts: self.claim_attempts.load(Ordering::Relaxed),
            claims: self.claims.load(Ordering::Relaxed),
            takeovers: self.takeovers.load(Ordering::Relaxed),
            already_completed: self.already_completed.load(Ordering::Relaxed),
            busy: self.busy.load(Ordering::Relaxed),
            conflicts: self.conflicts.load(Ordering::Relaxed),
            renewals: self.renewals.load(Ordering::Relaxed),
            lease_lost: self.lease_lost.load(Ordering::Relaxed),
            completions: self.completions.load(Ordering::Relaxed),
            releases: self.releases.load(Ordering::Relaxed),
            cancellations: self.cancellations.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            in_flight: self.in_flight.load(Ordering::Relaxed),
        }
    }

    fn increment(counter: &AtomicU64) {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_add(1))
            })
            .ok();
    }
}

/// Host-facing aggregate view over dynamic-workflow claim counters and the
/// optional agent-wide scheduler.  It is a read-only composition of local
/// metrics; it does not introduce a second workflow or event authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWorkflowControlDiagnostics {
    pub workflow: DynamicWorkflowHealthSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<crate::task_scheduler::TaskSchedulerHealthSnapshot>,
    /// Live owner-quota occupancy for this workflow when direct Flow steps
    /// use the shared scheduler. The projection is absent when the runtime is
    /// session-bound or has no scheduler.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler_quota: Option<crate::task_scheduler::TaskSchedulerQuotaSnapshot>,
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
    metrics: Arc<DynamicWorkflowMetrics>,
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
    /// Optional owner quota for direct script-backed Flow steps. The quota is
    /// enforced by the same scheduler actor as global capacity.
    scheduler_quota: Option<crate::task_scheduler::TaskSchedulerQuota>,
    /// Provider/model capacity projected into the same scheduler actor as
    /// workflow-step admission.
    provider_quota: Option<crate::task_scheduler::TaskSchedulerQuota>,
    continuation_lease: Option<Arc<DynamicWorkflowLease>>,
}

impl DynamicWorkflowRuntime {
    pub fn new(
        registry: Arc<ToolRegistry>,
        context: ToolContext,
        source: impl Into<String>,
    ) -> Self {
        let provider_quota = provider_quota_for_context(&context);
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
            scheduler_quota: None,
            provider_quota,
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
                let admission =
                    ModelGenerationAdmission::new(ModelGenerationConcurrency::bounded(maximum));
                match self.context.model_generation_admission() {
                    Some(session_admission) => match admission.clone().with_scheduler_quota_from(
                        &session_admission,
                        "dynamic-workflow-model-generation",
                    ) {
                        Ok(admission) => admission,
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                "failed to project session provider quota into dynamic workflow admission"
                            );
                            admission
                        }
                    },
                    None => admission,
                }
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

    /// Bind a digest-only owner quota to scheduler-backed Flow admissions.
    ///
    /// This does not create another queue or workflow authority; it only tells
    /// the existing scheduler how many slots this runtime may own at once.
    pub fn with_task_scheduler_quota(
        mut self,
        quota: crate::task_scheduler::TaskSchedulerQuota,
    ) -> Self {
        self.scheduler_quota = Some(quota);
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

    /// Return the live owner-quota projection for this runtime, when it is
    /// layered on an agent-wide scheduler. The projection contains only the
    /// digest identity and current occupancy; Flow history and worker leases
    /// remain independent authorities.
    pub async fn scheduler_quota_snapshot(
        &self,
    ) -> std::result::Result<
        Option<crate::task_scheduler::TaskSchedulerQuotaSnapshot>,
        crate::task_scheduler::TaskSchedulerError,
    > {
        match (&self.task_scheduler, &self.scheduler_quota) {
            (Some(scheduler), Some(quota)) => scheduler.quota_snapshot(quota).await.map(Some),
            _ => Ok(None),
        }
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
            let label = format!(
                "flow:{}:{}:{}",
                invocation.run_id, invocation.step_id, invocation.step_name
            );
            let mut quotas = Vec::with_capacity(2);
            if let Some(quota) = self.scheduler_quota.as_ref() {
                quotas.push(quota.clone());
            }
            if let Some(quota) = self.provider_quota.as_ref() {
                if !quotas
                    .iter()
                    .any(|candidate| candidate.identity == quota.identity)
                {
                    quotas.push(quota.clone());
                }
            }
            let task_lease = if quotas.is_empty() {
                scheduler
                    .acquire_with_identity(
                        SchedulerTaskPriority::Foreground,
                        label,
                        Some(identity),
                        &self.context.cancellation_token(),
                    )
                    .await
            } else {
                scheduler
                    .acquire_with_quotas(
                        SchedulerTaskPriority::Foreground,
                        label,
                        &quotas,
                        Some(identity),
                        &self.context.cancellation_token(),
                    )
                    .await
            }
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
            return Ok(self.context.clone().with_run_id(run_id.to_string()));
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
                    .with_run_id(run_id.to_string())
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
            return Ok(self.context.clone().with_run_id(run_id.to_string()));
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
            .with_run_id(run_id.to_string())
            .with_model_generation_permit(admission, Arc::new(permit))
            .map_err(|error| {
                a3s_flow::FlowError::Runtime(format!(
                    "bind model-generation admission to workflow step: {error}"
                ))
            })
    }

    async fn run_tool_step(
        &self,
        run_id: &str,
        tool_name: &str,
        args: Value,
    ) -> a3s_flow::Result<Value> {
        let context = self.context.clone().with_run_id(run_id.to_string());
        let result = self
            .invoker
            .invoke(
                self.context
                    .nested_tool_invocation(tool_name.to_string(), args),
                &context,
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
        let context = self
            .context
            .clone()
            .with_run_id(invocation.run_id.to_string());
        let result = self.run_script(payload.into_value(), &context).await?;
        serde_json::from_value(script_result(&result)?).map_err(a3s_flow::FlowError::from)
    }

    async fn run_step(&self, invocation: StepInvocation) -> a3s_flow::Result<Value> {
        let _admission = self.admit_step(&invocation).await?;
        if matches!(
            invocation.step_name.as_str(),
            TASK_TOOL | PARALLEL_TASK_TOOL
        ) {
            return self
                .run_tool_step(&invocation.run_id, &invocation.step_name, invocation.input)
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
