//! Agent loop construction for a session.
//!
//! The public session facade should not know how hooks, live tool definitions,
//! and optional queue adapters are threaded into an `AgentLoop`. This module is
//! the runtime seam for constructing the executable loop from session state.

use super::AgentSession;
use crate::agent::AgentLoop;
use crate::capability::{
    CapabilityKind, CapabilityProjection, CapabilityRuntimeError, CapabilityValue,
    SessionCapabilityRun,
};
use crate::commands::{CommandRegistry, CommandRegistrySnapshotError};
use crate::context::SkillCatalogContextProvider;
use crate::hooks::{HookEngine, HookEngineSnapshotError, HookExecutor};
use crate::skills::{SkillRegistry, SkillRegistrySnapshotError};
use crate::subagent::{AgentRegistry, AgentRegistrySnapshotError};
use crate::tools::ToolExecutor;
use std::sync::Arc;

const MAX_RUNTIME_VALIDATION_MESSAGE_BYTES: usize = 1_024;

pub(super) struct PinnedRuntimeProjection {
    skill_registry: Arc<SkillRegistry>,
    agent_registry: Arc<AgentRegistry>,
    command_registry: Arc<CommandRegistry>,
    hook_engine: Arc<HookEngine>,
    tool_executor: Arc<ToolExecutor>,
    // Only accepted, generation-bound projected Tools may enter delegated Tasks.
    task_tools: Vec<Arc<dyn crate::tools::Tool>>,
    mcp_bindings: Vec<Arc<crate::mcp::McpBinding>>,
    projected_mcp_authoritative: bool,
    knowledge: Option<Arc<crate::cognitive_context::CognitiveContextSession>>,
    projected_knowledge_authoritative: bool,
    context_providers: Vec<Arc<dyn crate::context::ContextProvider>>,
}

impl PinnedRuntimeProjection {
    pub(super) fn command_registry(&self) -> &CommandRegistry {
        &self.command_registry
    }

    #[cfg(test)]
    pub(super) fn skill_registry(&self) -> Arc<SkillRegistry> {
        Arc::clone(&self.skill_registry)
    }

    #[allow(dead_code)]
    pub(super) fn tool_executor_handle(&self) -> Arc<ToolExecutor> {
        Arc::clone(&self.tool_executor)
    }

    pub(super) fn tool_executor(&self) -> &ToolExecutor {
        &self.tool_executor
    }
}

pub(super) fn build_agent_loop(session: &AgentSession) -> AgentLoop {
    let tool_executor = Arc::clone(&session.tool_executor);
    let mut config = live_config(session);

    // Compatibility APIs remain mutable until CAP-GA1. A host-direct runtime
    // observes their latest definitions when it is constructed.
    config.tools = tool_executor.definitions();
    finish_agent_loop(session, tool_executor, config)
}

pub(super) async fn build_pinned_agent_loop(
    session: &AgentSession,
    cancellation: tokio_util::sync::CancellationToken,
) -> crate::error::Result<(AgentLoop, SessionCapabilityRun)> {
    let (runtime_projection, capability_run, checkpoint_capability_binding) =
        pin_and_admit_runtime_projection_with_cancellation(session, cancellation).await?;
    let run_hook_executor = match super::run_hook_executor::RunHookExecutor::new(
        session.hook_executor.clone(),
        Arc::clone(&runtime_projection.hook_engine),
        capability_run.task_spawner(),
    ) {
        Ok(executor) => executor as Arc<dyn HookExecutor>,
        Err(message) => {
            if let Err(error) = capability_run.close().await {
                tracing::warn!(error = %error, "Capability Run close failed after Hook executor assembly failed");
            }
            return Err(CapabilityRuntimeError::RuntimeValueInvalid {
                kind: CapabilityKind::Hook,
                public_name: "hook-registry".to_owned(),
                message: message.to_owned(),
            }
            .into());
        }
    };
    let inputs = LoopAssemblyInputs::from_session(session, live_config(session));
    let agent_loop =
        assemble_pinned_agent_loop(inputs, runtime_projection, run_hook_executor, || {
            session.parent_run_context()
        });
    let capability_runtime =
        crate::capability::AgentCapabilityRuntime::from_run(capability_run.run_scope());
    Ok((
        agent_loop
            .with_checkpoint_capability_binding(checkpoint_capability_binding)
            .with_capability_runtime(capability_runtime),
        capability_run,
    ))
}

// This supplier is consumed synchronously on this stack. It preserves the
// legacy getter's exact lazy position; owned callers supply an already captured
// value. It is not stored, spawned, or used as a job/future execution port.
fn assemble_pinned_agent_loop(
    inputs: LoopAssemblyInputs,
    runtime_projection: PinnedRuntimeProjection,
    run_hook_executor: Arc<dyn HookExecutor>,
    parent_context: impl FnOnce() -> crate::child_run::ChildRunContext,
) -> AgentLoop {
    let PinnedRuntimeProjection {
        skill_registry,
        agent_registry,
        command_registry: _,
        hook_engine: _,
        tool_executor,
        task_tools,
        mcp_bindings,
        projected_mcp_authoritative,
        knowledge,
        projected_knowledge_authoritative,
        context_providers,
    } = runtime_projection;
    let mut config = inputs.config;
    config.hook_engine = Some(run_hook_executor);
    config.skill_registry = Some(Arc::clone(&skill_registry));
    config.agent_registry = Some(Arc::clone(&agent_registry));
    config
        .context_providers
        .retain(|provider| provider.name() != "skills_catalog");
    config
        .context_providers
        .push(Arc::new(SkillCatalogContextProvider::new(Arc::clone(
            &skill_registry,
        ))));
    if projected_knowledge_authoritative {
        config
            .context_providers
            .retain(|provider| provider.cognitive_package_binding().is_none());
    }
    if let Some(knowledge) = knowledge {
        // A projected Knowledge value is the exact cognitive authority for
        // this Run. The Session-static provider is a durable compatibility or
        // resume seed and must not remain visible beside the projected value.
        config
            .context_providers
            .retain(|provider| provider.cognitive_package_binding().is_none());
        config.context_providers.push(knowledge);
    }
    config.context_providers.extend(context_providers);
    config.tools = tool_executor.definitions();

    // The Skill and search_skills built-ins capture a registry and executor.
    // Rebind them inside the Run snapshot so nested Skill execution cannot
    // resolve through the mutable Session-latest compatibility registries.
    crate::tools::register_skill(
        tool_executor.registry(),
        Arc::clone(&inputs.client),
        Arc::clone(&skill_registry),
        Arc::clone(&tool_executor),
        config.clone(),
    );

    // Delegation Tools capture an AgentRegistry when they are constructed.
    // Rebind them to this Run-owned snapshot so a later Session registration
    // or projected generation cannot change child-Agent selection in flight.
    if config.auto_delegation.allow_manual_delegation {
        let mut parent_context = parent_context();
        parent_context.skill_registry = Some(skill_registry);
        crate::tools::register_task_with_mcp_sources_and_scheduler(
            tool_executor.registry(),
            Arc::clone(&inputs.client),
            agent_registry,
            inputs.workspace.display().to_string(),
            if projected_mcp_authoritative {
                Vec::new()
            } else {
                inputs.mcp_managers
            },
            mcp_bindings,
            task_tools,
            Some(parent_context),
            Some(Arc::clone(&inputs.subagent_tasks)),
            Arc::clone(&inputs.task_scheduler),
        );
    }
    config.tools = tool_executor.definitions();

    finish_agent_loop_from_inputs(
        inputs.client,
        tool_executor,
        inputs.tool_context,
        config,
        inputs.model_admission,
        inputs.middleware,
        inputs.queue,
    )
}

/// Inputs for the one synchronous assembly implementation. None owns a Session
/// borrow, a checkpoint sender, or a new admission authority.
struct LoopAssemblyInputs {
    config: crate::agent::AgentConfig,
    client: Arc<dyn crate::llm::LlmClient>,
    tool_context: crate::tools::ToolContext,
    model_admission: crate::llm::ModelGenerationAdmission,
    middleware: Arc<crate::agent::ModelMiddlewareObs>,
    queue: Option<Arc<crate::session_lane_queue::SessionLaneQueue>>,
    external_hooks: Option<Arc<dyn HookExecutor>>,
    mcp_managers: Vec<Arc<crate::mcp::McpManager>>,
    subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
    task_scheduler: Arc<crate::task_scheduler::TaskScheduler>,
    workspace: std::path::PathBuf,
}

impl LoopAssemblyInputs {
    fn from_session(session: &AgentSession, config: crate::agent::AgentConfig) -> Self {
        Self {
            config,
            client: Arc::clone(&session.llm_client),
            tool_context: session.tool_context.clone(),
            model_admission: session.model_generation_admission.clone(),
            middleware: Arc::clone(&session.middleware_obs),
            queue: session.command_queue.clone(),
            external_hooks: session.hook_executor.clone(),
            mcp_managers: session.mcp_managers.clone(),
            subagent_tasks: Arc::clone(&session.subagent_tasks),
            task_scheduler: Arc::clone(&session.task_scheduler),
            workspace: session.workspace.clone(),
        }
    }
}

pub(super) struct CapturedRuntimeProjection {
    projection: crate::capability::CapabilityProjectionLease,
    ceiling: crate::capability::CapabilityCeiling,
    runtime: PinnedRuntimeProjection,
    binding: crate::capability::RunCapabilityBindingV1,
}

pub(super) struct OwnedLoopConstruction {
    inputs: LoopAssemblyInputs,
    parent: crate::child_run::ChildRunContext,
}

pub(super) struct OwnedCheckpointInputs {
    pub(super) store: Option<Arc<dyn crate::store::SessionStore>>,
    pub(super) export: Option<Arc<dyn crate::session_checkpoint::SessionCheckpointExportSink>>,
    pub(super) persistence: super::session_persistence::SessionPersistenceContext,
}

/// Nonactivated construction values; T004 must supply the original worker
/// handoff before any asynchronous admission. This value is never a guardian
/// registration, capability-close witness, or no-effects proof.
pub(super) struct OwnedPreparationCapture {
    pub(super) fact: CapturedRuntimeProjection,
    pub(super) command: Option<CapturedRuntimeProjection>,
    pub(super) construction: OwnedLoopConstruction,
    pub(super) checkpoint: OwnedCheckpointInputs,
    pub(super) permission: crate::permissions::PermissionPolicy,
    pub(super) cognitive_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
    pub(super) cancellation: tokio_util::sync::CancellationToken,
    pub(super) session_id: String,
    pub(super) workspace: std::path::PathBuf,
    pub(super) boundary: super::session_close::SessionAdmissionBoundary,
}

/// Every failure here follows an attempted original admission. A missing
/// returned guard is always unproved, never NotAttempted. Private fields and
/// constructors retain the original error independently of the close outcome.
type OriginalPreparationClose = (
    SessionCapabilityRun,
    Result<crate::capability::ScopeCloseReport, CapabilityRuntimeError>,
);

#[derive(Debug)]
pub(super) struct OwnedPreparationFailure {
    primary: crate::error::CodeError,
    acquired: Option<OriginalPreparationClose>,
}

impl OwnedPreparationFailure {
    fn admission_unproved(primary: crate::error::CodeError) -> Self {
        Self {
            primary,
            acquired: None,
        }
    }

    async fn close_acquired(primary: crate::error::CodeError, guard: SessionCapabilityRun) -> Self {
        let actual = guard.close().await;
        Self {
            primary,
            acquired: Some((guard, actual)),
        }
    }

    #[cfg(test)]
    pub(super) fn primary_error(&self) -> &crate::error::CodeError {
        &self.primary
    }

    /// None means admission was attempted without a returned guard. It does
    /// not authorize release. Some retains the same guard and actual result.
    pub(super) fn into_original_observation(
        self,
    ) -> (crate::error::CodeError, Option<OriginalPreparationClose>) {
        (self.primary, self.acquired)
    }
}

pub(super) fn capture_owned_preparation(
    session: &AgentSession,
    cancellation: tokio_util::sync::CancellationToken,
    command_capable: bool,
) -> crate::error::Result<OwnedPreparationCapture> {
    let _admission = session
        .close_handle
        .immediate_extension_mutation
        .lock()
        .map_err(|_| CapabilityRuntimeError::SessionClosed)?;
    // Do not call closed_under_original_boundary while already holding this
    // exact non-reentrant mutex.
    if session.is_closed() {
        return Err(CapabilityRuntimeError::SessionClosed.into());
    }
    // Direct compatibility registry guards bypass the outer mutation mutex.
    // Hold this ONE original guard over both real pins and both snapshots.
    let commands = session.command_registry.lock().map_err(|_| {
        CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Command,
            public_name: "command-registry".to_owned(),
            message: "Original command registry lock is poisoned".to_owned(),
        }
    })?;
    let parent = session.parent_run_context();
    let mut config = session.config.clone();
    config.durable_source = session.saved_durable_source();
    config.hook_engine = Some(match &session.hook_executor {
        Some(executor) => Arc::clone(executor),
        None => Arc::clone(&session.hook_engine) as Arc<dyn HookExecutor>,
    });
    config.budget_guard = parent.budget_guard.clone();
    apply_non_budget_runtime_overrides(session, &mut config);

    let capture_projection = || -> crate::error::Result<CapturedRuntimeProjection> {
        let projection = session.capability_catalog.pin();
        let ceiling = session.capability_run_ceiling_from_captured_budget(
            projection.projection().set(),
            projection.allowed_capabilities(),
            parent.budget_guard.as_ref(),
        )?;
        let binding = crate::capability::RunCapabilityBindingV1::from_set_and_ceiling(
            projection.projection().set(),
            &ceiling,
        )
        .map_err(|error| crate::error::CodeError::Internal(anyhow::anyhow!(error)))?;
        let runtime = pin_runtime_projection_with_command_registry(
            session,
            projection.projection(),
            &commands,
            Some(&ceiling),
        )?;
        Ok(CapturedRuntimeProjection {
            projection,
            ceiling,
            runtime,
            binding,
        })
    };
    let command = if command_capable {
        Some(capture_projection()?)
    } else {
        None
    };
    let fact = capture_projection()?;
    let cognitive_binding = cognitive_binding_for_projection(session, fact.projection.projection());
    let checkpoint = OwnedCheckpointInputs {
        store: session.session_store.clone(),
        export: session
            .runtime_session_checkpoint_export_sink
            .lock()
            .map_err(|_| {
                crate::error::CodeError::Session(
                    "Original checkpoint export sink lock is poisoned".to_owned(),
                )
            })?
            .clone(),
        persistence: super::session_persistence::SessionPersistenceContext::from_session(session),
    };
    let permission = session
        .runtime_permission_policy()
        .or_else(|| session.config.permission_policy.clone())
        .unwrap_or_default();
    let inputs = LoopAssemblyInputs::from_session(session, config);
    let workspace = inputs.workspace.clone();
    Ok(OwnedPreparationCapture {
        fact,
        command,
        construction: OwnedLoopConstruction { inputs, parent },
        checkpoint,
        permission,
        cognitive_binding,
        cancellation,
        session_id: session.session_id.clone(),
        workspace,
        boundary: session.close_handle.execution_admission_boundary(),
    })
}

pub(super) async fn admit_captured_runtime_projection(
    capture: CapturedRuntimeProjection,
    original_boundary: &super::session_close::SessionAdmissionBoundary,
    cancellation: tokio_util::sync::CancellationToken,
    workspace: &std::path::Path,
) -> Result<
    (
        PinnedRuntimeProjection,
        SessionCapabilityRun,
        crate::capability::RunCapabilityBindingV1,
    ),
    OwnedPreparationFailure,
> {
    let CapturedRuntimeProjection {
        projection,
        ceiling,
        runtime,
        binding,
    } = capture;
    // The eventual original worker must record possible entry before calling
    // this helper; cancellation of this future cannot publish a return value.
    let guard = SessionCapabilityRun::admit(
        projection,
        "active",
        "active",
        ceiling,
        cancellation,
        workspace,
    )
    .await
    .map_err(|error| OwnedPreparationFailure::admission_unproved(error.into()))?;
    if original_boundary.closed_under_original_boundary() {
        return Err(OwnedPreparationFailure::close_acquired(
            CapabilityRuntimeError::SessionClosed.into(),
            guard,
        )
        .await);
    }
    Ok((runtime, guard, binding))
}

pub(super) async fn build_owned_pinned_agent_loop(
    capture: CapturedRuntimeProjection,
    construction: OwnedLoopConstruction,
    original_boundary: super::session_close::SessionAdmissionBoundary,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<(AgentLoop, SessionCapabilityRun), OwnedPreparationFailure> {
    let OwnedLoopConstruction { inputs, parent } = construction;
    let (runtime, guard, binding) = admit_captured_runtime_projection(
        capture,
        &original_boundary,
        cancellation,
        &inputs.workspace,
    )
    .await?;
    assemble_owned_admitted_agent_loop(runtime, guard, binding, inputs, parent).await
}

async fn assemble_owned_admitted_agent_loop(
    runtime: PinnedRuntimeProjection,
    guard: SessionCapabilityRun,
    binding: crate::capability::RunCapabilityBindingV1,
    inputs: LoopAssemblyInputs,
    parent: crate::child_run::ChildRunContext,
) -> Result<(AgentLoop, SessionCapabilityRun), OwnedPreparationFailure> {
    let hooks = match super::run_hook_executor::RunHookExecutor::new(
        inputs.external_hooks.clone(),
        Arc::clone(&runtime.hook_engine),
        guard.task_spawner(),
    ) {
        Ok(hooks) => hooks as Arc<dyn HookExecutor>,
        Err(message) => {
            return Err(OwnedPreparationFailure::close_acquired(
                CapabilityRuntimeError::RuntimeValueInvalid {
                    kind: CapabilityKind::Hook,
                    public_name: "hook-registry".to_owned(),
                    message: message.to_owned(),
                }
                .into(),
                guard,
            )
            .await)
        }
    };
    // No Session reference or legacy lazy getter is captured by this supplier.
    let agent_loop = assemble_pinned_agent_loop(inputs, runtime, hooks, move || parent)
        .with_checkpoint_capability_binding(binding)
        .with_capability_runtime(crate::capability::AgentCapabilityRuntime::from_run(
            guard.run_scope(),
        ));
    Ok((agent_loop, guard))
}

#[cfg(test)]
pub(super) async fn pin_and_admit_runtime_projection(
    session: &AgentSession,
) -> crate::error::Result<(PinnedRuntimeProjection, SessionCapabilityRun)> {
    let (runtime, run, _) = pin_and_admit_runtime_projection_with_cancellation(
        session,
        session.session_cancel.child_token(),
    )
    .await?;
    Ok((runtime, run))
}

async fn pin_and_admit_runtime_projection_with_cancellation(
    session: &AgentSession,
    cancellation: tokio_util::sync::CancellationToken,
) -> crate::error::Result<(
    PinnedRuntimeProjection,
    SessionCapabilityRun,
    crate::capability::RunCapabilityBindingV1,
)> {
    // Pin the Code generation and compatibility registries under the same
    // mutation boundary. A Run therefore linearizes either before or after a
    // host mutation and never combines a capability projection with a newer
    // compatibility name map.
    let (projection, ceiling, runtime_projection, checkpoint_capability_binding) = {
        let _admission = session
            .close_handle
            .immediate_extension_mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session.is_closed() {
            return Err(CapabilityRuntimeError::SessionClosed.into());
        }
        let projection = session.capability_catalog.pin();
        let ceiling = session.capability_run_ceiling(
            projection.projection().set(),
            projection.allowed_capabilities(),
        )?;
        let checkpoint_capability_binding =
            crate::capability::RunCapabilityBindingV1::from_set_and_ceiling(
                projection.projection().set(),
                &ceiling,
            )
            .map_err(|error| crate::error::CodeError::Internal(anyhow::anyhow!(error)))?;
        let runtime_projection =
            pin_runtime_projection(session, projection.projection(), &ceiling)?;
        (
            projection,
            ceiling,
            runtime_projection,
            checkpoint_capability_binding,
        )
    };

    let capability_run = SessionCapabilityRun::admit(
        projection,
        "active",
        "active",
        ceiling,
        cancellation,
        &session.workspace,
    )
    .await?;
    let closed_during_admission = {
        let _admission = session
            .close_handle
            .immediate_extension_mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session.is_closed()
    };
    if closed_during_admission {
        if let Err(error) = capability_run.close().await {
            tracing::warn!(error = %error, "Capability Run close failed after Session close won admission");
        }
        return Err(CapabilityRuntimeError::SessionClosed.into());
    }
    Ok((
        runtime_projection,
        capability_run,
        checkpoint_capability_binding,
    ))
}

pub(super) fn validate_capability_projection_runtime(
    session: &AgentSession,
    projection: &CapabilityProjection,
    command_registry: &CommandRegistry,
) -> Result<(), CapabilityRuntimeError> {
    pin_runtime_projection_with_command_registry(session, projection, command_registry, None)
        .map(|_| ())
}

/// Compare a persisted Run binding with the complete catalog and authority
/// ceiling currently visible to a new Run. The short admission mutex makes the
/// comparison one atomic read of scoped and compatibility-owned state.
pub(super) fn validate_run_capability_binding(
    session: &AgentSession,
    expected: &crate::capability::RunCapabilityBindingV1,
) -> Result<(), crate::capability::RunCapabilityBindingError> {
    let _admission = session
        .close_handle
        .immediate_extension_mutation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let projection = session.capability_catalog.pin();
    let ceiling = session
        .capability_run_ceiling(
            projection.projection().set(),
            projection.allowed_capabilities(),
        )
        .map_err(|error| {
            crate::capability::RunCapabilityBindingError::Encoding(error.to_string())
        })?;
    expected.ensure_matches(projection.projection().set(), &ceiling)
}

/// Validate stateful Knowledge cutover rules that depend on both the current
/// and target catalog generations.
pub(super) fn validate_capability_projection_transition(
    session: &AgentSession,
    current: &CapabilityProjection,
    target: &CapabilityProjection,
) -> Result<(), CapabilityRuntimeError> {
    let Some(static_knowledge) = session.cognitive_context.as_ref() else {
        return Ok(());
    };
    let current_knowledge = projected_knowledge(current);
    let target_knowledge = projected_knowledge(target);

    match (current_knowledge, target_knowledge) {
        (None, Some(target)) if target.binding() != static_knowledge.binding() => {
            Err(CapabilityRuntimeError::RuntimeValueInvalid {
                kind: CapabilityKind::Knowledge,
                public_name: target.provider_name().to_owned(),
                message: "a Session-static or resumed cognitive binding must bootstrap the atomic Knowledge catalog with the same exact binding before a later generation can cut over"
                    .to_owned(),
            })
        }
        (Some(current), None) => Err(CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Knowledge,
            public_name: current.provider_name().to_owned(),
            message: "removing projected Knowledge would reveal a stale Session-static cognitive provider"
                .to_owned(),
        }),
        _ => Ok(()),
    }
}

/// Return the exact cognitive binding visible to a Run over `projection`.
pub(super) fn cognitive_binding_for_projection(
    session: &AgentSession,
    projection: &CapabilityProjection,
) -> Option<crate::cognitive_context::CognitivePackageBindingV1> {
    projected_knowledge(projection)
        .map(|knowledge| knowledge.binding().clone())
        .or_else(|| {
            session
                .cognitive_context
                .as_ref()
                .map(|knowledge| knowledge.binding().clone())
        })
}

fn pin_runtime_projection(
    session: &AgentSession,
    projection: &CapabilityProjection,
    ceiling: &crate::capability::CapabilityCeiling,
) -> Result<PinnedRuntimeProjection, CapabilityRuntimeError> {
    let command_registry = session
        .command_registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pin_runtime_projection_with_command_registry(
        session,
        projection,
        &command_registry,
        Some(ceiling),
    )
}

pub(super) fn pin_runtime_projection_with_command_registry(
    session: &AgentSession,
    projection: &CapabilityProjection,
    compatibility_commands: &CommandRegistry,
    ceiling: Option<&crate::capability::CapabilityCeiling>,
) -> Result<PinnedRuntimeProjection, CapabilityRuntimeError> {
    let mut projected_tools = Vec::new();
    let mut projected_skills = Vec::new();
    let mut projected_agents = Vec::new();
    let mut projected_commands = Vec::new();
    let mut projected_hooks = Vec::new();
    let mut projected_mcp = Vec::new();
    let mut projected_mcp_tools = Vec::new();
    let projected_mcp_authoritative = projection
        .iter()
        .any(|(_, value)| matches!(value, CapabilityValue::Mcp(_)));
    let mut projected_knowledge = None;
    let mut projected_context = Vec::new();
    for (id, value) in projection.iter() {
        if ceiling.is_some_and(|ceiling| !ceiling.allows(id)) {
            continue;
        }
        match value {
            CapabilityValue::Tool(tool) => projected_tools.push(if ceiling.is_some() {
                crate::capability::ScopedProjectedTool::bind(
                    Arc::clone(tool),
                    id.clone(),
                    projection.set().digest().clone(),
                )
            } else {
                Arc::clone(tool)
            }),
            CapabilityValue::Skill(skill) => projected_skills.push(Arc::clone(skill)),
            CapabilityValue::Agent(agent) => projected_agents.push(Arc::clone(agent)),
            CapabilityValue::Command(command) => projected_commands.push(Arc::clone(command)),
            CapabilityValue::Hook(hook) => {
                hook.validate_run_scope().map_err(|message| {
                    CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: CapabilityKind::Hook,
                        public_name: hook.hook().id.clone(),
                        message: message.to_owned(),
                    }
                })?;
                projected_hooks.push(Arc::clone(hook));
            }
            CapabilityValue::Mcp(binding) => {
                binding.validate_run_scope().map_err(|error| {
                    CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: CapabilityKind::Mcp,
                        public_name: binding.server_name().to_owned(),
                        message: truncate_utf8(
                            error.to_string(),
                            MAX_RUNTIME_VALIDATION_MESSAGE_BYTES,
                        ),
                    }
                })?;
                let compatibility_owns_name = session
                    .close_handle
                    .mcp_tool_ownership
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains_server(binding.server_name());
                if compatibility_owns_name {
                    return Err(CapabilityRuntimeError::RuntimeNameConflict {
                        kind: CapabilityKind::Mcp,
                        public_name: binding.server_name().to_owned(),
                    });
                }
                for tool in binding.projected_tools() {
                    projected_mcp_tools.push(if ceiling.is_some() {
                        crate::capability::ScopedProjectedTool::bind(
                            tool,
                            id.clone(),
                            projection.set().digest().clone(),
                        )
                    } else {
                        tool
                    });
                }
                projected_mcp.push(Arc::clone(binding));
            }
            CapabilityValue::Context(provider) => {
                if provider.cognitive_package_binding().is_some() {
                    return Err(CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: CapabilityKind::Context,
                        public_name: provider.name().to_owned(),
                        message: "a projected Context cannot carry a cognitive package binding; install exact cognitive authority through the persisted Knowledge/session boundary"
                            .to_owned(),
                    });
                }
                if session
                    .config
                    .context_providers
                    .iter()
                    .any(|current| current.name() == provider.name())
                {
                    return Err(CapabilityRuntimeError::RuntimeNameConflict {
                        kind: CapabilityKind::Context,
                        public_name: provider.name().to_owned(),
                    });
                }
                projected_context.push(Arc::clone(provider));
            }
            CapabilityValue::Knowledge(knowledge) => {
                knowledge.binding().validate().map_err(|error| {
                    CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: CapabilityKind::Knowledge,
                        public_name: knowledge.provider_name().to_owned(),
                        message: truncate_utf8(
                            error.to_string(),
                            MAX_RUNTIME_VALIDATION_MESSAGE_BYTES,
                        ),
                    }
                })?;
                if projected_knowledge.is_some() {
                    return Err(CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: CapabilityKind::Knowledge,
                        public_name: knowledge.provider_name().to_owned(),
                        message: "a Run must have exactly one cognitive Knowledge authority"
                            .to_owned(),
                    });
                }
                projected_knowledge = Some(Arc::clone(knowledge));
            }
            // KnowledgeSurface is multi-instance readiness evidence. It is
            // intentionally not installed as Run cognitive context.
            CapabilityValue::KnowledgeSurface(_) => {}
            // Flow is consumed through AgentSession::projected_flow. The
            // admitted Agent Run still pins the same catalog generation, but
            // Flow definitions are intentionally not model-visible tools.
            #[cfg(feature = "dynamic-workflow")]
            CapabilityValue::Flow(_) => {}
            // UI is consumed through AgentSession::projected_ui. Core retains
            // its exact document and dependencies without choosing a renderer
            // or making it model-visible.
            CapabilityValue::Ui(_) => {}
        }
    }

    if let Some(knowledge) = &projected_knowledge {
        if !projected_context.is_empty() {
            return Err(CapabilityRuntimeError::RuntimeValueInvalid {
                kind: CapabilityKind::Knowledge,
                public_name: knowledge.provider_name().to_owned(),
                message:
                    "exact Knowledge cannot accompany projected general-purpose Context providers"
                        .to_owned(),
            });
        }
        if !session.host_context_provider_names.is_empty() {
            return Err(CapabilityRuntimeError::RuntimeValueInvalid {
                kind: CapabilityKind::Knowledge,
                public_name: knowledge.provider_name().to_owned(),
                message: "exact Knowledge cannot accompany Session-static general-purpose Context providers"
                    .to_owned(),
            });
        }
    }
    if session.cognitive_context.is_some() && !projected_context.is_empty() {
        return Err(CapabilityRuntimeError::RuntimeValueInvalid {
            kind: CapabilityKind::Context,
            public_name: projected_context[0].name().to_owned(),
            message: "general-purpose Context cannot accompany an exact Session cognitive binding"
                .to_owned(),
        });
    }

    // Directory discovery is optional. Classify on a fork so a later tool or
    // MCP conflict still leaves the live compatibility registry unchanged.
    // A host-protected name stays in the admit list and still fails the session.
    // A discovered file that still owns the name is withheld: neither body is
    // admitted. A live registration that replaced that file still conflicts.
    let admission = session.close_handle.skill_registry.fork();
    let mut admitted_skills = Vec::with_capacity(projected_skills.len());
    let mut withheld_skills = Vec::new();
    for skill in projected_skills {
        let name = skill.name.clone();
        if admission.protects(&name) {
            admitted_skills.push(skill);
            continue;
        }
        if let Some(discovered) = admission.take_discovered(&name) {
            let sentence = SkillRegistry::withhold_sentence(&name);
            tracing::error!(
                skill = %name,
                discovered = %discovered.display(),
                projected = "admitted generation",
                sentence = %sentence,
                "discovered skill withheld"
            );
            withheld_skills.push((name, sentence));
            continue;
        }
        admitted_skills.push(skill);
    }
    let skill_registry = admission
        .snapshot_with_external_skills(admitted_skills)
        .map_err(|error| match error {
            SkillRegistrySnapshotError::NameConflict { name } => {
                CapabilityRuntimeError::RuntimeNameConflict {
                    kind: CapabilityKind::Skill,
                    public_name: name,
                }
            }
            SkillRegistrySnapshotError::Validation { name, message } => {
                CapabilityRuntimeError::RuntimeValueInvalid {
                    kind: CapabilityKind::Skill,
                    public_name: name,
                    message: truncate_utf8(message, MAX_RUNTIME_VALIDATION_MESSAGE_BYTES),
                }
            }
        })?;
    for (name, sentence) in withheld_skills {
        skill_registry.remember_withheld(&name, sentence);
    }
    let skill_registry = Arc::new(skill_registry);
    let agent_registry = Arc::new(
        session
            .agent_registry
            .snapshot_with_external_agents(projected_agents)
            .map_err(|error: AgentRegistrySnapshotError| {
                CapabilityRuntimeError::RuntimeNameConflict {
                    kind: CapabilityKind::Agent,
                    public_name: error.name().to_owned(),
                }
            })?,
    );
    let command_registry = Arc::new(
        compatibility_commands
            .snapshot_with_external_commands(projected_commands)
            .map_err(|error: CommandRegistrySnapshotError| {
                CapabilityRuntimeError::RuntimeNameConflict {
                    kind: CapabilityKind::Command,
                    public_name: error.name().to_owned(),
                }
            })?,
    );
    let hook_engine = Arc::new(
        session
            .hook_engine
            .snapshot_with_external_hooks(projected_hooks, session.hook_executor.is_none())
            .map_err(|error: HookEngineSnapshotError| {
                CapabilityRuntimeError::RuntimeNameConflict {
                    kind: CapabilityKind::Hook,
                    public_name: error.name().to_owned(),
                }
            })?,
    );
    // Reuse the accepted wrappers, never rebuild children from the live Session.
    let task_tools = projected_tools.clone();
    let tool_executor = session
        .tool_executor
        .snapshot_with_external_tools(projected_tools)
        .map_err(|error| CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Tool,
            public_name: error.name().to_owned(),
        })?;
    let tool_executor = Arc::new(
        tool_executor
            .snapshot_with_external_tools(projected_mcp_tools)
            .map_err(|error| CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Mcp,
                public_name: error.name().to_owned(),
            })?,
    );
    Ok(PinnedRuntimeProjection {
        skill_registry,
        agent_registry,
        command_registry,
        hook_engine,
        tool_executor,
        task_tools,
        mcp_bindings: projected_mcp,
        projected_mcp_authoritative,
        knowledge: projected_knowledge,
        projected_knowledge_authoritative: projection
            .iter()
            .any(|(_, value)| matches!(value, CapabilityValue::Knowledge(_))),
        context_providers: projected_context,
    })
}

fn projected_knowledge(
    projection: &CapabilityProjection,
) -> Option<&Arc<crate::cognitive_context::CognitiveContextSession>> {
    projection.iter().find_map(|(_, value)| match value {
        CapabilityValue::Knowledge(knowledge) => Some(knowledge),
        _ => None,
    })
}

fn truncate_utf8(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut boundary = max;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

fn live_config(session: &AgentSession) -> crate::agent::AgentConfig {
    let mut config = session.config.clone();
    config.durable_source = session.saved_durable_source();
    config.hook_engine = Some(match &session.hook_executor {
        Some(executor) => executor.clone(),
        None => Arc::clone(&session.hook_engine) as Arc<dyn crate::hooks::HookExecutor>,
    });

    // Runtime budget-guard override (set via AgentSession::set_budget_guard)
    // takes precedence over the value baked in at session-build time.
    // Used by Node SDK where the JS callable cannot live inside
    // value-typed SessionOptions.
    if let Some(runtime_guard) = session.budget_guard() {
        config.budget_guard = Some(runtime_guard);
    }
    apply_non_budget_runtime_overrides(session, &mut config);
    config
}

fn apply_non_budget_runtime_overrides(
    session: &AgentSession,
    config: &mut crate::agent::AgentConfig,
) {
    if let Some(ledger) = session.outcome_ledger_override() {
        config.outcome_ledger = ledger;
    }
    if let Some(style) = session.runtime_agent_style_override() {
        config.prompt_slots.style = style;
    }
    if let Some(mode) = session.runtime_planning_mode_override() {
        config.planning_mode = mode;
    }
    if let Some(language) = session.runtime_output_language_override() {
        config.prompt_slots.output_language = language;
    }
}

fn finish_agent_loop(
    session: &AgentSession,
    tool_executor: Arc<crate::tools::ToolExecutor>,
    config: crate::agent::AgentConfig,
) -> AgentLoop {
    finish_agent_loop_from_inputs(
        Arc::clone(&session.llm_client),
        tool_executor,
        session.tool_context.clone(),
        config,
        session.model_generation_admission.clone(),
        Arc::clone(&session.middleware_obs),
        session.command_queue.clone(),
    )
}

fn finish_agent_loop_from_inputs(
    client: Arc<dyn crate::llm::LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    tool_context: crate::tools::ToolContext,
    config: crate::agent::AgentConfig,
    model_admission: crate::llm::ModelGenerationAdmission,
    middleware: Arc<crate::agent::ModelMiddlewareObs>,
    queue: Option<Arc<crate::session_lane_queue::SessionLaneQueue>>,
) -> AgentLoop {
    let mut agent_loop = AgentLoop::new(client, tool_executor, tool_context, config)
        .with_model_generation_admission(model_admission)
        .with_model_middleware_obs(middleware);
    if let Some(queue) = queue {
        agent_loop = agent_loop.with_queue(queue);
    }
    agent_loop
}

#[cfg(test)]
mod owned_preparation_tests;
