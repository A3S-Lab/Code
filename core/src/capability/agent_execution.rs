use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{
    CapabilityEffectError, CapabilityScope, CapabilityScopeError, CapabilityScopeHandle, Run,
    ScopeCloseReport, Subtask, Turn,
};

/// Cloneable, non-owning capability boundary installed on an AgentLoop.
///
/// Root loops create Turns below the admitted Run. Delegated loops create
/// Turns below their owning Subtask, allowing the hierarchy to recurse as
/// `Run -> Turn -> Subtask -> Turn` without extending any parent lifetime.
#[derive(Clone, Debug)]
pub(crate) struct AgentCapabilityRuntime {
    parent: AgentTurnParent,
    run: CapabilityScopeHandle<Run>,
    next_turn_scope: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
enum AgentTurnParent {
    Run(CapabilityScopeHandle<Run>),
    Subtask(CapabilityScopeHandle<Subtask>),
}

/// One owned temporal slice spanning a provider response and every Tool call
/// produced by that response.
pub(crate) struct AgentCapabilityTurn {
    scope: CapabilityScope<Turn>,
    run: CapabilityScopeHandle<Run>,
}

/// Scope parents made available to a Tool invocation.
///
/// Foreground delegated work belongs to the current Turn. Explicit background
/// work is promoted to the Run boundary so closing a Turn does not silently
/// cancel a task that the Tool contract says may continue.
#[derive(Clone, Debug)]
pub(crate) struct AgentToolCapabilityContext {
    foreground: CapabilityScopeHandle<Turn>,
    background: CapabilityScopeHandle<Run>,
}

/// One delegated execution owner and the runtime used by its child AgentLoop.
pub(crate) struct AgentCapabilitySubtask {
    scope: CapabilityScope<Subtask>,
    runtime: AgentCapabilityRuntime,
}

/// Optional temporal owner for one model-side orchestration operation.
///
/// Compatibility AgentLoops without an admitted capability Run retain their
/// existing cancellation token. Scoped AgentLoops create a real Turn and must
/// close it cleanly before the orchestration result is observed.
pub(crate) struct AgentCapabilityOperation {
    turn: Option<AgentCapabilityTurn>,
    cancellation: CancellationToken,
    label: &'static str,
}

impl AgentCapabilityRuntime {
    pub(crate) fn from_run(scope: &CapabilityScope<Run>) -> Self {
        let run = scope.handle();
        Self {
            parent: AgentTurnParent::Run(run.clone()),
            run,
            next_turn_scope: Arc::new(AtomicU64::new(1)),
        }
    }

    fn from_subtask(scope: &CapabilityScope<Subtask>, run: CapabilityScopeHandle<Run>) -> Self {
        Self {
            parent: AgentTurnParent::Subtask(scope.handle()),
            run,
            next_turn_scope: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(crate) fn begin_turn(
        &self,
        logical_turn: usize,
    ) -> Result<AgentCapabilityTurn, CapabilityScopeError> {
        let sequence = self
            .next_turn_scope
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| CapabilityScopeError::ChildIdentityExhausted)?;
        let local_id = format!("turn-{logical_turn}-{sequence}");
        let scope = match &self.parent {
            AgentTurnParent::Run(parent) => parent.turn_inheriting(local_id)?,
            AgentTurnParent::Subtask(parent) => parent.turn_inheriting(local_id)?,
        };
        Ok(AgentCapabilityTurn {
            scope,
            run: self.run.clone(),
        })
    }
}

impl AgentCapabilityTurn {
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.scope.cancellation()
    }

    pub(crate) fn tool_context(&self) -> AgentToolCapabilityContext {
        AgentToolCapabilityContext {
            foreground: self.scope.handle(),
            background: self.run.clone(),
        }
    }

    pub(crate) async fn close(&self) -> Result<ScopeCloseReport, CapabilityScopeError> {
        self.scope.close().await
    }
}

impl AgentToolCapabilityContext {
    pub(crate) fn foreground_scope_id(&self) -> &str {
        self.foreground.id().as_str()
    }

    pub(crate) fn register_foreground_effect<E>(
        &self,
        effect: E,
    ) -> Result<(), CapabilityScopeError>
    where
        E: super::CapabilityEffect,
    {
        self.foreground.register_effect(effect)
    }

    pub(crate) fn spawn_foreground_task<F>(
        &self,
        name: impl Into<String>,
        task: F,
    ) -> Result<(), CapabilityScopeError>
    where
        F: Future<Output = Result<(), CapabilityEffectError>> + Send + 'static,
    {
        self.foreground.spawn_task(name, task).map(|_| ())
    }

    pub(crate) fn admit_subtask(
        &self,
        local_id: impl Into<String>,
        background: bool,
    ) -> Result<AgentCapabilitySubtask, CapabilityScopeError> {
        let scope = if background {
            // Promotion is authorized by the invoking Turn even though the
            // resulting lifetime belongs to the Run. A stale ToolContext must
            // never create new Run-owned work after its Turn has closed.
            self.foreground.cancellation()?;
            self.background.subtask_inheriting(local_id)?
        } else {
            self.foreground.subtask_inheriting(local_id)?
        };
        let runtime = AgentCapabilityRuntime::from_subtask(&scope, self.background.clone());
        Ok(AgentCapabilitySubtask { scope, runtime })
    }

    pub(crate) fn background_scope(&self) -> &CapabilityScopeHandle<Run> {
        &self.background
    }
}

impl AgentCapabilitySubtask {
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.scope.cancellation()
    }

    pub(crate) fn runtime(&self) -> AgentCapabilityRuntime {
        self.runtime.clone()
    }

    pub(crate) async fn close(&self) -> Result<ScopeCloseReport, CapabilityScopeError> {
        self.scope.close().await
    }
}

impl AgentCapabilityOperation {
    pub(crate) fn begin(
        runtime: Option<&AgentCapabilityRuntime>,
        logical_turn: usize,
        fallback_cancellation: &CancellationToken,
        label: &'static str,
    ) -> Result<Self, CapabilityScopeError> {
        let turn = runtime
            .map(|runtime| runtime.begin_turn(logical_turn))
            .transpose()?;
        let cancellation = turn
            .as_ref()
            .map(AgentCapabilityTurn::cancellation)
            .unwrap_or_else(|| fallback_cancellation.clone());
        Ok(Self {
            turn,
            cancellation,
            label,
        })
    }

    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub(crate) async fn close(&self) -> anyhow::Result<()> {
        let Some(turn) = self.turn.as_ref() else {
            return Ok(());
        };
        let report = turn.close().await?;
        if !report.is_clean() {
            anyhow::bail!(
                "Capability {} Turn close was incomplete (tasks failed: {}, tasks timed out: {}, child scopes failed: {}, child scopes timed out: {}, effects failed: {}, effects timed out: {})",
                self.label,
                report.tasks_failed,
                report.tasks_timed_out,
                report.child_scopes_failed,
                report.child_scopes_timed_out,
                report.effects_failed,
                report.effects_timed_out,
            );
        }
        Ok(())
    }
}
