//! Core types for the extensible tool system

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Sender for streaming tool output deltas during execution.
pub type ToolEventSender = mpsc::Sender<ToolStreamEvent>;

/// Internal acknowledgement channel used to preserve causal ordering between
/// high-level events emitted by a tool and the tool's terminal runtime event.
///
/// `broadcast::Sender::send` only confirms that an event was enqueued. A tool
/// can therefore return before the run event sink has observed its final
/// `SubagentEnd`. This barrier lets the invocation gateway wait until all
/// already-enqueued high-level events have been drained without adding an
/// internal marker to the public [`crate::agent::AgentEvent`] protocol.
#[derive(Clone)]
pub(crate) struct AgentEventBarrier {
    tx: mpsc::Sender<oneshot::Sender<()>>,
}

pub(crate) struct AgentEventBarrierReceiver {
    rx: mpsc::Receiver<oneshot::Sender<()>>,
}

impl AgentEventBarrier {
    pub(crate) fn channel(capacity: usize) -> (Self, AgentEventBarrierReceiver) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, AgentEventBarrierReceiver { rx })
    }

    pub(crate) async fn flush(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.tx.send(ack_tx).await.is_ok() {
            // A dropped acknowledgement means the owning run sink has already
            // stopped. There is no later ToolEnd in that sink to order against.
            let _ = ack_rx.await;
        }
    }
}

impl AgentEventBarrierReceiver {
    pub(crate) async fn recv(&mut self) -> Option<oneshot::Sender<()>> {
        self.rx.recv().await
    }
}

/// Events emitted by tools during execution
#[derive(Debug, Clone)]
pub enum ToolStreamEvent {
    /// Intermediate output delta (e.g., a line of stdout from bash)
    OutputDelta(String),
}

/// Primary representation produced by a tool invocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputKind {
    Text,
    Structured,
    Diff,
    Mixed,
}

/// Execution properties used by orchestrators to make safe scheduling choices.
///
/// Defaults are deliberately conservative. A tool must explicitly opt into
/// parallel-safe or resumable behavior.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCapabilities {
    pub read_only: bool,
    pub idempotent: bool,
    pub resumable: bool,
    pub cancellation_safe: bool,
    pub supports_pagination: bool,
    pub max_parallelism: usize,
    pub output_kind: ToolOutputKind,
}

impl ToolCapabilities {
    pub const fn conservative() -> Self {
        Self {
            read_only: false,
            idempotent: false,
            resumable: false,
            cancellation_safe: false,
            supports_pagination: false,
            max_parallelism: 1,
            output_kind: ToolOutputKind::Text,
        }
    }

    pub const fn read_only_paginated(max_parallelism: usize) -> Self {
        Self {
            read_only: true,
            idempotent: true,
            resumable: true,
            cancellation_safe: true,
            supports_pagination: true,
            max_parallelism,
            output_kind: ToolOutputKind::Text,
        }
    }

    pub const fn parallel_safe_read(max_parallelism: usize) -> Self {
        Self {
            read_only: true,
            idempotent: true,
            resumable: false,
            cancellation_safe: true,
            supports_pagination: false,
            max_parallelism,
            output_kind: ToolOutputKind::Text,
        }
    }

    pub fn allows_parallel_batch(self) -> bool {
        self.read_only && self.idempotent && self.cancellation_safe && self.max_parallelism > 1
    }
}

impl Default for ToolCapabilities {
    fn default() -> Self {
        Self::conservative()
    }
}

/// Governed capabilities available to a tool during an agent/session
/// invocation.
///
/// Custom meta-tools should use this facade for nested tools and model calls.
/// It preserves the caller's permission, HITL, hook, budget, timeout, queue,
/// cancellation, and sanitization scope, and cannot manufacture the trusted
/// host-direct origin.
#[derive(Clone)]
pub struct InvocationRuntime {
    context: ToolContext,
}

impl std::fmt::Debug for InvocationRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationRuntime")
            .field("session_id", &self.context.session_id)
            .field("cancelled", &self.context.is_cancelled())
            .field("has_tool_invoker", &self.context.tool_invoker.is_some())
            .field("has_llm_client", &self.context.llm_client.is_some())
            .finish()
    }
}

impl InvocationRuntime {
    /// Invoke another registered tool inside the current governance scope.
    pub async fn invoke_tool(
        &self,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<super::ToolResult> {
        let invoker = self.context.tool_invoker.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "governed nested tool invocation is unavailable outside an agent/session runtime"
            )
        })?;
        Ok(invoker
            .invoke(
                super::invocation::ToolInvocation::nested(name, args),
                &self.context,
            )
            .await)
    }

    /// Return tool names visible through the current invocation gateway.
    pub fn available_tools(&self) -> Vec<String> {
        self.context
            .tool_invoker
            .as_ref()
            .map(|invoker| invoker.available_tools())
            .unwrap_or_default()
    }

    /// Make a governed model sub-call using the current budget and
    /// cancellation scope.
    pub async fn complete(
        &self,
        messages: &[crate::llm::Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
    ) -> Result<crate::llm::LlmResponse> {
        let client = self.context.llm_client.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "governed model invocation is unavailable outside an agent/session runtime"
            )
        })?;
        client.complete(messages, system, tools).await
    }
}

/// Tool execution context
///
/// Provides tools with access to workspace and other runtime information.
#[derive(Clone)]
pub struct ToolContext {
    /// Workspace root directory (sandbox boundary)
    pub workspace: PathBuf,
    /// Optional session ID for session-aware tools
    pub session_id: Option<String>,
    /// Run identity for the invocation that owns this context. This is kept
    /// separate from `session_id` so detached work from different runs cannot
    /// accidentally share an admission scope.
    run_id: Option<String>,
    /// Optional sender for streaming tool output deltas during execution
    pub event_tx: Option<ToolEventSender>,
    /// Optional agent event sender for tools that emit high-level agent events (e.g., SubagentStart)
    pub agent_event_tx: Option<broadcast::Sender<crate::agent::AgentEvent>>,
    /// Run-owned acknowledgement barrier paired with `agent_event_tx`.
    agent_event_barrier: Option<AgentEventBarrier>,
    /// Optional search configuration for web_search tool
    pub search_config: Option<Arc<crate::config::SearchConfig>>,
    /// Session-scoped search circuit state shared by cloned tool contexts.
    pub(crate) search_circuit_breaker: a3s_search::CircuitBreaker,
    search_circuit_scope: Option<String>,
    /// Agent-scoped per-engine admission shared by cloned tool contexts.
    search_bulkhead: a3s_search::Bulkhead,
    /// Agent-scoped retry allowance shared across browser requests.
    search_retry_budget: a3s_search::RetryBudget,
    /// Session-scoped identical-request flights shared by cloned tool contexts.
    search_request_coalescer: a3s_search::SearchCoalescer,
    /// Optional sandbox for routing `bash` tool execution through the configured
    /// process-isolation boundary.
    pub sandbox: Option<std::sync::Arc<dyn crate::sandbox::BashSandbox>>,
    /// Optional command environment overrides for subprocess-based tools.
    pub command_env: Option<Arc<HashMap<String, String>>>,
    /// Host-provided workspace capabilities used by built-in tools.
    pub workspace_services: Arc<crate::workspace::WorkspaceServices>,
    /// Permission boundary frozen by the agent run that owns this invocation.
    ///
    /// Child-run tools consult this value instead of the mutable session
    /// configuration they were registered with.
    pub(crate) run_permission_checker: Option<Arc<dyn crate::permissions::PermissionChecker>>,
    /// Confirmation routing frozen by the agent run that owns this invocation.
    pub(crate) run_confirmation_manager: Option<Arc<dyn crate::hitl::ConfirmationProvider>>,
    /// Distinguishes an agent run that intentionally has no governance
    /// provider from a plain low-level tool context that was never bound.
    pub(crate) run_governance_bound: bool,
    /// Scoped invocation gateway installed by the agent runtime.
    pub(crate) tool_invoker: Option<Arc<dyn super::invocation::ToolInvoker>>,
    /// Per-run governed LLM facade for tools that perform model sub-calls.
    pub(crate) llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Shared active-generation gate for this agent/session runtime.
    pub(crate) model_generation_admission: Option<crate::llm::ModelGenerationAdmission>,
    /// Optional capacity already acquired by an outer model-generation step.
    model_generation_permit: Option<Arc<ModelGenerationPermitReservation>>,
    /// Trust policy inherited by nested calls of a host-direct orchestrator.
    pub(crate) host_direct_policy: Option<super::invocation::HostDirectPolicy>,
    /// Cancellation for the invocation that owns this tool call.
    ///
    /// Session construction installs the session lifetime token. Agent runs
    /// replace it with their per-run child token before dispatch, so a host
    /// cancellation interrupts queued and directly executing tools as well as
    /// model calls.
    cancellation: CancellationToken,
    /// Weak spatial parents for effects and delegated work created by the
    /// current Agent Turn. Handles never retain a capability generation.
    capability_context: Option<crate::capability::AgentToolCapabilityContext>,
    /// Orchestrator call stack used to reject recursive `batch` / `program` calls.
    invocation_stack: Vec<String>,
    /// True while a queue worker owns this invocation scope.
    ///
    /// Nested orchestrator calls stay inside the already-admitted queue command
    /// instead of submitting back into the same lane and deadlocking a
    /// single-concurrency queue.
    inside_tool_queue: bool,
}

#[derive(Debug)]
struct ModelGenerationPermitReservation {
    permit: Mutex<Option<Arc<crate::llm::ModelGenerationPermit>>>,
}

impl ModelGenerationPermitReservation {
    fn new(permit: Arc<crate::llm::ModelGenerationPermit>) -> Self {
        Self {
            permit: Mutex::new(Some(permit)),
        }
    }

    fn owns(&self, admission: &crate::llm::ModelGenerationAdmission) -> bool {
        self.permit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|permit| admission.owns(permit))
    }

    fn claim(
        &self,
        admission: &crate::llm::ModelGenerationAdmission,
    ) -> Option<Arc<crate::llm::ModelGenerationPermit>> {
        let mut permit = self
            .permit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if permit.as_ref().is_some_and(|permit| admission.owns(permit)) {
            permit.take()
        } else {
            None
        }
    }
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("workspace", &self.workspace)
            .field("session_id", &self.session_id)
            .field("run_id", &self.run_id)
            .field("sandbox", &self.sandbox.is_some())
            .field("workspace_services", &self.workspace_services)
            .field(
                "has_run_permission_checker",
                &self.run_permission_checker.is_some(),
            )
            .field(
                "has_run_confirmation_manager",
                &self.run_confirmation_manager.is_some(),
            )
            .field("run_governance_bound", &self.run_governance_bound)
            .field("has_tool_invoker", &self.tool_invoker.is_some())
            .field("has_llm_client", &self.llm_client.is_some())
            .field(
                "has_model_generation_admission",
                &self.model_generation_admission.is_some(),
            )
            .field(
                "has_model_generation_permit",
                &self.model_generation_permit.is_some(),
            )
            .field(
                "has_agent_event_barrier",
                &self.agent_event_barrier.is_some(),
            )
            .field("host_direct_policy", &self.host_direct_policy)
            .field("cancelled", &self.cancellation.is_cancelled())
            .field(
                "capability_scope_id",
                &self
                    .capability_context
                    .as_ref()
                    .map(crate::capability::AgentToolCapabilityContext::foreground_scope_id),
            )
            .field("invocation_stack", &self.invocation_stack)
            .field("inside_tool_queue", &self.inside_tool_queue)
            .finish()
    }
}

fn search_circuit_breaker(
    config: Option<&crate::config::SearchConfig>,
) -> a3s_search::CircuitBreaker {
    let mut policy = a3s_search::CircuitBreakerConfig::default();
    if let Some(health) = config.and_then(|config| config.health.as_ref()) {
        policy.failure_threshold = health.max_failures.max(1);
        policy.empty_threshold = health.max_failures.max(1);
        policy.transient_open_duration = std::time::Duration::from_secs(health.suspend_seconds);
    }
    a3s_search::CircuitBreaker::new(policy)
}

impl ToolContext {
    pub fn new(workspace: PathBuf) -> Self {
        let canonical_workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());
        Self {
            workspace: canonical_workspace,
            session_id: None,
            run_id: None,
            event_tx: None,
            agent_event_tx: None,
            agent_event_barrier: None,
            search_config: None,
            search_circuit_breaker: search_circuit_breaker(None),
            search_circuit_scope: None,
            search_bulkhead: a3s_search::Bulkhead::default(),
            search_retry_budget: a3s_search::RetryBudget::default(),
            search_request_coalescer: a3s_search::SearchCoalescer::default(),
            sandbox: None,
            command_env: None,
            workspace_services: crate::workspace::WorkspaceServices::local(workspace),
            run_permission_checker: None,
            run_confirmation_manager: None,
            run_governance_bound: false,
            tool_invoker: None,
            llm_client: None,
            model_generation_admission: None,
            model_generation_permit: None,
            host_direct_policy: None,
            cancellation: CancellationToken::new(),
            capability_context: None,
            invocation_stack: Vec::new(),
            inside_tool_queue: false,
        }
    }

    /// Set the session ID for this context
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        if self.search_circuit_scope.as_deref() != Some(session_id.as_str()) {
            self.search_circuit_breaker = search_circuit_breaker(self.search_config.as_deref());
            self.search_request_coalescer = a3s_search::SearchCoalescer::default();
            self.search_circuit_scope = Some(session_id.clone());
        }
        self.session_id = Some(session_id);
        self
    }

    /// Bind the context to one agent-run identity.
    pub(crate) fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// Return the run identity bound to this invocation, when available.
    pub(crate) fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    /// Set the event sender for streaming tool output
    pub fn with_event_tx(mut self, tx: ToolEventSender) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Set the agent event sender for high-level agent events (e.g., SubagentStart/End)
    pub fn with_agent_event_tx(mut self, tx: broadcast::Sender<crate::agent::AgentEvent>) -> Self {
        self.agent_event_tx = Some(tx);
        // A barrier is paired with one specific receiver. Replacing only the
        // sender must not retain an acknowledgement path for the old channel.
        self.agent_event_barrier = None;
        self
    }

    pub(crate) fn with_agent_event_barrier(mut self, barrier: AgentEventBarrier) -> Self {
        self.agent_event_barrier = Some(barrier);
        self
    }

    /// Wait until the owning run sink has consumed every high-level agent
    /// event that the current tool enqueued before this call.
    pub(crate) async fn flush_agent_events(&self) {
        if let Some(barrier) = &self.agent_event_barrier {
            barrier.flush().await;
        }
    }

    /// Set the search configuration
    pub fn with_search_config(mut self, config: crate::config::SearchConfig) -> Self {
        self.search_circuit_breaker = search_circuit_breaker(Some(&config));
        self.search_request_coalescer = a3s_search::SearchCoalescer::default();
        self.search_config = Some(Arc::new(config));
        self
    }

    pub(crate) fn search_circuit_breaker(&self) -> a3s_search::CircuitBreaker {
        self.search_circuit_breaker.clone()
    }

    pub(crate) fn search_bulkhead(&self) -> a3s_search::Bulkhead {
        self.search_bulkhead.clone()
    }

    pub(crate) fn with_search_runtime(
        mut self,
        bulkhead: a3s_search::Bulkhead,
        retry_budget: a3s_search::RetryBudget,
    ) -> Self {
        self.search_bulkhead = bulkhead;
        self.search_retry_budget = retry_budget;
        self
    }

    pub(crate) fn search_retry_budget(&self) -> a3s_search::RetryBudget {
        self.search_retry_budget.clone()
    }

    pub(crate) fn with_search_request_coalescer(
        mut self,
        coalescer: a3s_search::SearchCoalescer,
    ) -> Self {
        self.search_request_coalescer = coalescer;
        self
    }

    pub(crate) fn search_request_coalescer(&self) -> a3s_search::SearchCoalescer {
        self.search_request_coalescer.clone()
    }

    /// Set a sandbox executor for the `bash` tool.
    pub fn with_sandbox(
        mut self,
        sandbox: std::sync::Arc<dyn crate::sandbox::BashSandbox>,
    ) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Set environment overrides for subprocess-based tools such as `bash`.
    pub fn with_command_env(mut self, env: Arc<HashMap<String, String>>) -> Self {
        self.command_env = Some(env);
        self
    }

    /// Set host-provided workspace capabilities for built-in tools.
    pub fn with_workspace_services(
        mut self,
        services: Arc<crate::workspace::WorkspaceServices>,
    ) -> Self {
        self.workspace_services = services;
        self
    }

    pub(crate) fn with_run_governance(
        mut self,
        permission_checker: Option<Arc<dyn crate::permissions::PermissionChecker>>,
        confirmation_manager: Option<Arc<dyn crate::hitl::ConfirmationProvider>>,
    ) -> Self {
        self.run_permission_checker = permission_checker;
        self.run_confirmation_manager = confirmation_manager;
        self.run_governance_bound = true;
        self
    }

    pub(crate) fn has_run_governance(&self) -> bool {
        self.run_governance_bound
    }

    pub(crate) fn run_permission_checker(
        &self,
    ) -> Option<Arc<dyn crate::permissions::PermissionChecker>> {
        self.run_permission_checker.clone()
    }

    pub(crate) fn run_confirmation_manager(
        &self,
    ) -> Option<Arc<dyn crate::hitl::ConfirmationProvider>> {
        self.run_confirmation_manager.clone()
    }

    /// Return the governed nested-invocation facade for this tool call.
    ///
    /// The facade is always constructible so tools can be tested with a plain
    /// context; individual methods return a contextual error when no agent or
    /// session runtime installed the corresponding capability.
    pub fn invocation_runtime(&self) -> InvocationRuntime {
        InvocationRuntime {
            context: self.clone(),
        }
    }

    pub(crate) fn with_tool_invoker(
        mut self,
        invoker: Arc<dyn super::invocation::ToolInvoker>,
    ) -> Self {
        self.tool_invoker = Some(invoker);
        self
    }

    /// Bind this context to the lifetime of the owning session or run.
    pub(crate) fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Cancellation token for cooperative tools and invocation gateways.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Canonical capability Turn identity for this Tool invocation.
    pub fn capability_scope_id(&self) -> Option<&str> {
        self.capability_context
            .as_ref()
            .map(crate::capability::AgentToolCapabilityContext::foreground_scope_id)
    }

    /// Transfer a reversible Tool-owned effect into the current Agent Turn.
    ///
    /// The effect closes after every Tool call produced by that model response
    /// has settled. Plain host-direct Tool contexts fail closed because they do
    /// not own an Agent Turn.
    pub fn register_capability_effect<E>(
        &self,
        effect: E,
    ) -> std::result::Result<(), crate::capability::CapabilityScopeError>
    where
        E: crate::capability::CapabilityEffect,
    {
        let context = self
            .capability_context
            .as_ref()
            .ok_or(crate::capability::CapabilityScopeError::AgentTurnScopeUnavailable)?;
        context.register_foreground_effect(effect)
    }

    pub(crate) fn with_capability_context(
        mut self,
        context: crate::capability::AgentToolCapabilityContext,
    ) -> Self {
        self.capability_context = Some(context);
        self
    }

    pub(crate) fn capability_context(
        &self,
    ) -> Option<crate::capability::AgentToolCapabilityContext> {
        self.capability_context.clone()
    }

    /// Whether the owning invocation has already been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn tool_invoker(&self) -> Option<Arc<dyn super::invocation::ToolInvoker>> {
        self.tool_invoker.clone()
    }

    pub(crate) fn with_llm_client(mut self, client: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    pub(crate) fn llm_client(&self) -> Option<Arc<dyn crate::llm::LlmClient>> {
        self.llm_client.clone()
    }

    pub(crate) fn with_model_generation_admission(
        mut self,
        admission: crate::llm::ModelGenerationAdmission,
    ) -> Self {
        let retains_permit = self
            .model_generation_permit
            .as_ref()
            .is_some_and(|reservation| reservation.owns(&admission));
        self.model_generation_admission = Some(admission);
        if !retains_permit {
            self.model_generation_permit = None;
        }
        self
    }

    pub(crate) fn model_generation_admission(
        &self,
    ) -> Option<crate::llm::ModelGenerationAdmission> {
        self.model_generation_admission.clone()
    }

    pub(crate) fn with_model_generation_permit(
        mut self,
        admission: crate::llm::ModelGenerationAdmission,
        permit: Arc<crate::llm::ModelGenerationPermit>,
    ) -> std::result::Result<Self, crate::llm::ModelGenerationAdmissionError> {
        if !admission.owns(&permit) {
            return Err(crate::llm::ModelGenerationAdmissionError::ForeignPermit);
        }
        self.model_generation_admission = Some(admission);
        self.model_generation_permit =
            Some(Arc::new(ModelGenerationPermitReservation::new(permit)));
        Ok(self)
    }

    pub(crate) fn model_generation_permit(
        &self,
        admission: &crate::llm::ModelGenerationAdmission,
    ) -> Option<Arc<crate::llm::ModelGenerationPermit>> {
        self.model_generation_permit
            .as_ref()
            .and_then(|reservation| reservation.claim(admission))
    }

    pub(crate) fn with_host_direct_policy(
        mut self,
        policy: super::invocation::HostDirectPolicy,
    ) -> Self {
        self.host_direct_policy = Some(policy);
        self
    }

    pub(crate) fn without_host_direct_policy(mut self) -> Self {
        self.host_direct_policy = None;
        self
    }

    /// Construct a nested invocation for a built-in orchestrator.
    ///
    /// Only an explicit host-direct orchestration context propagates trusted
    /// control-plane authority. Public custom tools use [`InvocationRuntime`],
    /// whose `invoke_tool` method always creates an ordinary governed nested
    /// invocation.
    pub(crate) fn nested_tool_invocation(
        &self,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> super::invocation::ToolInvocation {
        match self.host_direct_policy {
            Some(policy) => {
                super::invocation::ToolInvocation::host_direct_nested(name, args, policy)
            }
            None => super::invocation::ToolInvocation::nested(name, args),
        }
    }

    pub(crate) fn enter_tool_invocation(
        &self,
        tool_name: &str,
    ) -> std::result::Result<Self, String> {
        if matches!(tool_name, "batch" | "program")
            && self.invocation_stack.iter().any(|name| name == tool_name)
        {
            return Err(format!("recursive {tool_name} calls are not allowed"));
        }

        let mut ctx = self.clone();
        ctx.invocation_stack.push(tool_name.to_string());
        Ok(ctx)
    }

    pub(crate) fn with_tool_queue_scope(mut self) -> Self {
        self.inside_tool_queue = true;
        self
    }

    pub(crate) fn is_inside_tool_queue(&self) -> bool {
        self.inside_tool_queue
    }

    /// Normalize a user-supplied path through the configured workspace backend.
    pub fn resolve_workspace_path(&self, path: &str) -> Result<crate::workspace::WorkspacePath> {
        self.workspace_services.normalize_path(path)
    }

    /// Resolve path relative to workspace, ensuring it stays within sandbox.
    ///
    /// Deprecated: returns a host-filesystem `PathBuf` that is meaningless for
    /// virtual / DFS / browser workspace backends. New code should call
    /// [`Self::resolve_workspace_path`] and route I/O through
    /// `workspace_services.fs()` instead.
    #[deprecated(
        note = "Use resolve_workspace_path() and route I/O through workspace_services.fs() for non-local backends"
    )]
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        if self.workspace_services.local_root().is_none() {
            anyhow::bail!(
                "resolve_path is only valid for local workspaces; this session uses a non-local workspace backend, call resolve_workspace_path() instead"
            );
        }
        a3s_common::tools::resolve_path(&self.workspace, path).map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Resolve path for writing (allows non-existent files).
    ///
    /// Deprecated: see [`Self::resolve_path`].
    #[deprecated(
        note = "Use resolve_workspace_path() and route I/O through workspace_services.fs() for non-local backends"
    )]
    pub fn resolve_path_for_write(&self, path: &str) -> Result<PathBuf> {
        if self.workspace_services.local_root().is_none() {
            anyhow::bail!(
                "resolve_path_for_write is only valid for local workspaces; this session uses a non-local workspace backend, call resolve_workspace_path() instead"
            );
        }
        a3s_common::tools::resolve_path_for_write(&self.workspace, path)
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}

/// Structured discriminant for tool failures.
///
/// This is the SDK-facing counterpart of [`WorkspaceError`](crate::workspace::WorkspaceError)
/// (and any future typed error sources). The Rust trait surface returns
/// typed enums; this struct is what survives the trip through
/// `ToolOutput` → `ToolResult` → `ToolCallResult` → SDK boundary so JS /
/// Python callers can do `match` on the kind instead of regex-matching
/// the human-readable `output` string.
///
/// Serializes to JSON with a `type` discriminator, e.g.:
/// ```json
/// { "type": "version_conflict", "path": "doc.md", "expected": "etag-1", "actual": "etag-2" }
/// ```
///
/// `#[non_exhaustive]` so adding a new kind is a minor-version change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// Compare-and-swap write rejected by the backend because the file
    /// changed since the caller read it. Originates from
    /// `WorkspaceError::VersionConflict` on the S3 / future versioning
    /// backends.
    VersionConflict {
        path: String,
        expected: String,
        actual: Option<String>,
    },
    /// Remote git server returned a typed 409 / 422 conflict code such
    /// as `BRANCH_EXISTS` or `WORKING_TREE_DIRTY`.
    RemoteGitConflict { code: String, message: String },
    /// Operation referenced a path that does not exist.
    NotFound { path: String },
    /// Caller passed an argument the tool / backend cannot honour
    /// (malformed pattern, parent-traversal path, ...).
    InvalidArgument { message: String },
    /// A pre-tool governance hook denied the invocation. `retryable` and
    /// `retry_after_ms` let SDK callers distinguish temporary policy state
    /// from a permanent denial without parsing the rendered tool output.
    HookDenied {
        reason: String,
        retryable: bool,
        retry_after_ms: Option<u64>,
    },
    /// The backend explicitly does not support this operation
    /// (e.g. worktree on a remote-git workspace).
    Unsupported { message: String },
    /// The operation's outer timeout fired before the backend responded.
    Timeout { op: String, duration_ms: u64 },
    /// A typed network or upstream HTTP transport failure.
    Transport { op: String },
    /// The caller or owning session cancelled the operation.
    Cancelled { op: String },
    /// A collection operation completed but one or more children failed.
    PartialFailure { failed: usize, total: usize },
    /// A provider rejected the operation due to rate limiting.
    RateLimited { retry_after_ms: Option<u64> },
}

impl ToolErrorKind {
    /// Map a [`WorkspaceError`](crate::workspace::WorkspaceError) into the
    /// corresponding SDK-visible kind. Backend variants that don't fit a
    /// dedicated [`ToolErrorKind`] (currently `Backend(_)`) return `None`;
    /// the caller then surfaces only the human-readable message.
    pub fn from_workspace_error(err: &crate::workspace::WorkspaceError) -> Option<Self> {
        use crate::workspace::WorkspaceError as WE;
        match err {
            WE::NotFound { path } => Some(Self::NotFound { path: path.clone() }),
            WE::VersionConflict(c) => Some(Self::VersionConflict {
                path: c.path.clone(),
                expected: c.expected.clone(),
                actual: c.actual.clone(),
            }),
            WE::RemoteGitConflict(c) => Some(Self::RemoteGitConflict {
                code: c.code.clone(),
                message: c.message.clone(),
            }),
            WE::InvalidArgument { message } => Some(Self::InvalidArgument {
                message: message.clone(),
            }),
            WE::Unsupported(message) => Some(Self::Unsupported {
                message: message.clone(),
            }),
            WE::Timeout { op, duration } => Some(Self::Timeout {
                op: op.clone(),
                duration_ms: duration.as_millis() as u64,
            }),
            WE::Backend(_) => None,
        }
    }
}

/// Tool execution output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Output content (text or base64 for binary)
    pub content: String,
    /// Whether execution was successful
    pub success: bool,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Optional image attachments from tool execution (e.g., screenshots).
    ///
    /// When present, these are included in the tool result message sent to
    /// the LLM as multi-modal content blocks alongside the text content.
    #[serde(skip)]
    pub images: Vec<crate::llm::Attachment>,
    /// Optional structured discriminant for tool failures. Populated by
    /// tools that can map their failure into a typed [`ToolErrorKind`]
    /// (e.g. `edit` / `patch` on a `WorkspaceError::VersionConflict`).
    /// Surfaced through `ToolResult` and `ToolCallResult` so SDK callers
    /// can react programmatically without parsing the `content` string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ToolErrorKind>,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            success: true,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            success: false,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Attach images to the tool output.
    ///
    /// These will be included as multi-modal content blocks in the tool
    /// result message sent to the LLM.
    pub fn with_images(mut self, images: Vec<crate::llm::Attachment>) -> Self {
        self.images = images;
        self
    }

    /// Attach a typed error kind. Used by built-in tools when they can
    /// map a backend failure (e.g. `WorkspaceError::VersionConflict`)
    /// into a programmatically actionable [`ToolErrorKind`].
    pub fn with_error_kind(mut self, kind: ToolErrorKind) -> Self {
        self.error_kind = Some(kind);
        self
    }
}

/// Tool trait - the core abstraction for all tools
///
/// Implement this trait to create custom tools that can be registered
/// with the ToolRegistry.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must be unique within registry)
    fn name(&self) -> &str;

    /// Human-readable description for LLM
    fn description(&self) -> &str;

    /// JSON Schema for tool parameters
    fn parameters(&self) -> serde_json::Value;

    /// Complete model-facing definition for this tool.
    ///
    /// Most tools have static metadata and use this default. Registry-backed
    /// tools can override it to build a definition from live runtime state
    /// without leaking owned strings through `description()`.
    fn definition(&self) -> crate::llm::ToolDefinition {
        crate::llm::ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }

    /// Whether this tool definition should be sent to the model.
    ///
    /// Hidden tools remain registered and executable for host integrations,
    /// persisted calls, and compatibility aliases. This separates the compact
    /// model-facing surface from the larger runtime capability surface.
    fn is_model_visible(&self) -> bool {
        true
    }

    /// Scheduling and result capabilities for this specific invocation.
    fn capabilities(&self, _args: &serde_json::Value) -> ToolCapabilities {
        ToolCapabilities::conservative()
    }

    /// Whether tool-owned metadata requires this invocation to pass through
    /// HITL even when the host permission policy would otherwise allow it.
    ///
    /// This is an escalation-only hint. The invocation gateway still applies
    /// the host permission policy first, so a tool can never use this hook to
    /// weaken an `Ask` or `Deny` decision.
    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        false
    }

    /// Execute the tool with given arguments
    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_circuit_state_is_shared_by_context_clones_but_isolated_by_session() {
        let first = ToolContext::new(PathBuf::from("/tmp")).with_session_id("session-a");
        first
            .search_circuit_breaker()
            .acquire("quota-api")
            .unwrap()
            .record_failure(&a3s_search::EngineFailure::new(
                "Quota API",
                "provider_quota",
                "quota exhausted",
            ));

        let same_session = first.clone();
        assert!(same_session
            .search_circuit_breaker()
            .acquire("quota-api")
            .is_err());

        let other_session = first.with_session_id("session-b");
        assert!(other_session
            .search_circuit_breaker()
            .acquire("quota-api")
            .is_ok());
    }

    struct RecordingInvoker {
        origin: Arc<std::sync::Mutex<Option<super::super::invocation::InvocationOrigin>>>,
    }

    #[async_trait]
    impl super::super::invocation::ToolInvoker for RecordingInvoker {
        async fn invoke(
            &self,
            invocation: super::super::invocation::ToolInvocation,
            _ctx: &ToolContext,
        ) -> super::super::ToolResult {
            *self.origin.lock().unwrap() = Some(invocation.origin);
            super::super::ToolResult::success(&invocation.name, "nested result".to_string())
        }

        fn available_tools(&self) -> Vec<String> {
            vec!["child".to_string()]
        }
    }

    #[test]
    #[allow(deprecated)]
    fn test_tool_context_resolve_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file
        let test_file = temp_dir.path().join("file.txt");
        std::fs::write(&test_file, "test").unwrap();

        // Relative path to existing file
        let resolved = ctx.resolve_path("file.txt");
        assert!(resolved.is_ok());

        // Non-existent file should return error
        let resolved = ctx.resolve_path("nonexistent.txt");
        assert!(resolved.is_err());
    }

    #[test]
    fn tool_context_rejects_recursive_orchestrator_invocations() {
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let program_ctx = ctx.enter_tool_invocation("program").unwrap();
        assert_eq!(
            program_ctx.enter_tool_invocation("program").unwrap_err(),
            "recursive program calls are not allowed"
        );

        let batch_ctx = ctx.enter_tool_invocation("batch").unwrap();
        assert_eq!(
            batch_ctx.enter_tool_invocation("batch").unwrap_err(),
            "recursive batch calls are not allowed"
        );
    }

    #[tokio::test]
    async fn public_invocation_runtime_forces_nested_origin() {
        let origin = Arc::new(std::sync::Mutex::new(None));
        let ctx =
            ToolContext::new(PathBuf::from("/tmp")).with_tool_invoker(Arc::new(RecordingInvoker {
                origin: Arc::clone(&origin),
            }));
        let runtime = ctx.invocation_runtime();

        assert_eq!(runtime.available_tools(), vec!["child"]);
        let result = runtime
            .invoke_tool("child", serde_json::json!({"value": 1}))
            .await
            .unwrap();
        assert_eq!(result.output, "nested result");
        assert_eq!(
            *origin.lock().unwrap(),
            Some(super::super::invocation::InvocationOrigin::Nested)
        );
    }

    #[tokio::test]
    async fn invocation_runtime_fails_closed_without_a_governed_gateway() {
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let error = ctx
            .invocation_runtime()
            .invoke_tool("read", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unavailable outside"));
    }

    #[tokio::test]
    async fn tool_context_rejects_a_model_permit_from_another_admission_gate() {
        let first = crate::llm::ModelGenerationAdmission::default();
        let second = crate::llm::ModelGenerationAdmission::default();
        let permit = first
            .acquire(&CancellationToken::new())
            .await
            .expect("first admission permit");

        let error = ToolContext::new(PathBuf::from("/tmp"))
            .with_model_generation_permit(second, Arc::new(permit))
            .expect_err("a foreign permit must not bypass the configured admission gate");

        assert_eq!(
            error,
            crate::llm::ModelGenerationAdmissionError::ForeignPermit
        );
    }

    #[tokio::test]
    async fn tool_context_preacquired_model_permit_can_only_be_claimed_once() {
        let admission = crate::llm::ModelGenerationAdmission::default();
        let permit = admission
            .acquire(&CancellationToken::new())
            .await
            .expect("admission permit");
        let ctx = ToolContext::new(PathBuf::from("/tmp"))
            .with_model_generation_permit(admission.clone(), Arc::new(permit))
            .expect("bind matching permit");

        let claimed = ctx
            .model_generation_permit(&admission)
            .expect("first claim");
        assert!(
            ctx.model_generation_permit(&admission).is_none(),
            "one pre-admission permit must not authorize concurrent nested generations"
        );
        drop(claimed);

        let replacement = admission
            .acquire(&CancellationToken::new())
            .await
            .expect("claimed permit should release normally");
        drop(replacement);
    }

    #[test]
    fn test_tool_output_success() {
        let output = ToolOutput::success("Hello");
        assert!(output.success);
        assert_eq!(output.content, "Hello");
    }

    #[test]
    fn test_tool_output_error() {
        let output = ToolOutput::error("Failed");
        assert!(!output.success);
        assert_eq!(output.content, "Failed");
    }

    #[test]
    fn test_tool_output_images_default_empty() {
        let output = ToolOutput::success("ok");
        assert!(output.images.is_empty());
    }

    #[test]
    fn test_tool_output_with_images() {
        let images = vec![crate::llm::Attachment::png(vec![1, 2, 3])];
        let output = ToolOutput::success("screenshot taken").with_images(images);
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].media_type, "image/png");
    }

    #[test]
    fn test_tool_output_with_metadata_and_images() {
        let output = ToolOutput::success("done")
            .with_metadata(serde_json::json!({"key": "val"}))
            .with_images(vec![crate::llm::Attachment::jpeg(vec![0xFF])]);
        assert!(output.metadata.is_some());
        assert_eq!(output.images.len(), 1);
    }
}
