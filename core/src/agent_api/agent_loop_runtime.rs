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
}

impl PinnedRuntimeProjection {
    pub(super) fn command_registry(&self) -> &CommandRegistry {
        &self.command_registry
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
) -> crate::error::Result<(AgentLoop, SessionCapabilityRun)> {
    let (runtime_projection, capability_run) = pin_and_admit_runtime_projection(session).await?;
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
    let PinnedRuntimeProjection {
        skill_registry,
        agent_registry,
        command_registry: _,
        hook_engine: _,
        tool_executor,
    } = runtime_projection;
    let mut config = live_config(session);
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
    config.tools = tool_executor.definitions();

    // The Skill and search_skills built-ins capture a registry and executor.
    // Rebind them inside the Run snapshot so nested Skill execution cannot
    // resolve through the mutable Session-latest compatibility registries.
    crate::tools::register_skill(
        tool_executor.registry(),
        Arc::clone(&session.llm_client),
        Arc::clone(&skill_registry),
        Arc::clone(&tool_executor),
        config.clone(),
    );

    // Delegation Tools capture an AgentRegistry when they are constructed.
    // Rebind them to this Run-owned snapshot so a later Session registration
    // or projected generation cannot change child-Agent selection in flight.
    if config.auto_delegation.allow_manual_delegation {
        let mut parent_context = session.parent_run_context();
        parent_context.skill_registry = Some(skill_registry);
        crate::tools::register_task_with_mcp_managers_and_scheduler(
            tool_executor.registry(),
            Arc::clone(&session.llm_client),
            agent_registry,
            session.workspace.display().to_string(),
            session.mcp_managers.clone(),
            Some(parent_context),
            Some(Arc::clone(&session.subagent_tasks)),
            Arc::clone(&session.task_scheduler),
        );
    }
    config.tools = tool_executor.definitions();

    Ok((
        finish_agent_loop(session, tool_executor, config),
        capability_run,
    ))
}

pub(super) async fn pin_and_admit_runtime_projection(
    session: &AgentSession,
) -> crate::error::Result<(PinnedRuntimeProjection, SessionCapabilityRun)> {
    // Pin the Code generation and compatibility registries under the same
    // mutation boundary. A Run therefore linearizes either before or after a
    // host mutation and never combines a capability projection with a newer
    // compatibility name map.
    let (projection, ceiling, runtime_projection) = {
        let _admission = session
            .close_handle
            .immediate_extension_mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session.is_closed() {
            return Err(CapabilityRuntimeError::SessionClosed.into());
        }
        let projection = session.capability_catalog.pin();
        let ceiling = session.capability_run_ceiling(projection.projection().set())?;
        let runtime_projection = pin_runtime_projection(session, projection.projection())?;
        (projection, ceiling, runtime_projection)
    };

    let capability_run = SessionCapabilityRun::admit(
        projection,
        "active",
        "active",
        ceiling,
        session.session_cancel.child_token(),
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
    Ok((runtime_projection, capability_run))
}

pub(super) fn validate_capability_projection_runtime(
    session: &AgentSession,
    projection: &CapabilityProjection,
    command_registry: &CommandRegistry,
) -> Result<(), CapabilityRuntimeError> {
    pin_runtime_projection_with_command_registry(session, projection, command_registry).map(|_| ())
}

fn pin_runtime_projection(
    session: &AgentSession,
    projection: &CapabilityProjection,
) -> Result<PinnedRuntimeProjection, CapabilityRuntimeError> {
    let command_registry = session
        .command_registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pin_runtime_projection_with_command_registry(session, projection, &command_registry)
}

fn pin_runtime_projection_with_command_registry(
    session: &AgentSession,
    projection: &CapabilityProjection,
    compatibility_commands: &CommandRegistry,
) -> Result<PinnedRuntimeProjection, CapabilityRuntimeError> {
    let mut projected_tools = Vec::new();
    let mut projected_skills = Vec::new();
    let mut projected_agents = Vec::new();
    let mut projected_commands = Vec::new();
    let mut projected_hooks = Vec::new();
    for (_, value) in projection.iter() {
        match value {
            CapabilityValue::Tool(tool) => projected_tools.push(Arc::clone(tool)),
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
            _ => return Err(CapabilityRuntimeError::UnsupportedSessionKind { kind: value.kind() }),
        }
    }

    let skill_registry = Arc::new(
        session
            .close_handle
            .skill_registry
            .snapshot_with_external_skills(projected_skills)
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
            })?,
    );
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
    let tool_executor = Arc::new(
        session
            .tool_executor
            .snapshot_with_external_tools(projected_tools)
            .map_err(|error| CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Tool,
                public_name: error.name().to_owned(),
            })?,
    );
    Ok(PinnedRuntimeProjection {
        skill_registry,
        agent_registry,
        command_registry,
        hook_engine,
        tool_executor,
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
    config
}

fn finish_agent_loop(
    session: &AgentSession,
    tool_executor: Arc<crate::tools::ToolExecutor>,
    config: crate::agent::AgentConfig,
) -> AgentLoop {
    let mut agent_loop = AgentLoop::new(
        session.llm_client.clone(),
        tool_executor,
        session.tool_context.clone(),
        config,
    )
    .with_model_generation_admission(session.model_generation_admission.clone());
    if let Some(queue) = &session.command_queue {
        agent_loop = agent_loop.with_queue(Arc::clone(queue));
    }
    // Wire per-tool-round checkpointing when the session has a store.
    // The run id is bound later by the caller via
    // `AgentLoop::set_checkpoint_run` once `start_run` returns.
    if let Some(store) = &session.session_store {
        let sink = std::sync::Arc::new(crate::loop_checkpoint::SessionStoreCheckpointSink::new(
            std::sync::Arc::clone(store),
        ));
        agent_loop = agent_loop.with_checkpoint_sink(sink);
    }
    agent_loop
}
