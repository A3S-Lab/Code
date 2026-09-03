use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::capability::{CapabilityEffectError, SupervisedTaskSpawner};
use crate::hooks::{
    HookEngine, HookEvent, HookExecutor, HookOutcome, HookResult, HookTaskDispatcher,
    HookTaskFuture,
};

struct CapabilityHookTaskDispatcher {
    tasks: SupervisedTaskSpawner,
}

impl HookTaskDispatcher for CapabilityHookTaskDispatcher {
    fn dispatch(&self, name: &'static str, task: HookTaskFuture) -> Result<(), String> {
        self.tasks
            .spawn_task(name, async move {
                task.await;
                Ok::<(), CapabilityEffectError>(())
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Run-frozen Hook executor composed over an optional Session-static host seam.
///
/// The external executor remains the outer policy authority. The immutable
/// in-process snapshot then evaluates compatibility and projected Hook
/// bindings. Observational work is registered with the capability supervisor
/// before dispatch returns.
pub(super) struct RunHookExecutor {
    external: Option<Arc<dyn HookExecutor>>,
    snapshot: Arc<HookEngine>,
    tasks: SupervisedTaskSpawner,
}

impl RunHookExecutor {
    pub(super) fn new(
        external: Option<Arc<dyn HookExecutor>>,
        snapshot: Arc<HookEngine>,
        tasks: SupervisedTaskSpawner,
    ) -> Result<Arc<Self>, &'static str> {
        let dispatcher: Arc<dyn HookTaskDispatcher> = Arc::new(CapabilityHookTaskDispatcher {
            tasks: tasks.clone(),
        });
        snapshot
            .attach_task_dispatcher(dispatcher)
            .map_err(|_| "Run Hook task dispatcher was already attached")?;
        Ok(Arc::new(Self {
            external,
            snapshot,
            tasks,
        }))
    }

    async fn fire_composed(&self, event: &HookEvent, inline_snapshot: bool) -> HookOutcome {
        let external_modified = match &self.external {
            Some(external) => match external.fire_outcome(event).await {
                HookOutcome::Continue(modified) => modified,
                // Skip is scoped to the external executor's own chain. It
                // cannot bypass package-projected policy in the next layer.
                HookOutcome::Skip => None,
                terminal => return terminal,
            },
            None => None,
        };

        let snapshot_outcome = if inline_snapshot {
            self.snapshot.fire_outcome_inline_observers(event).await
        } else {
            self.snapshot.fire_outcome(event).await
        };
        match snapshot_outcome {
            HookOutcome::Continue(modified) => {
                HookOutcome::Continue(modified.or(external_modified))
            }
            HookOutcome::Skip => HookOutcome::Continue(external_modified),
            terminal => terminal,
        }
    }
}

impl fmt::Debug for RunHookExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunHookExecutor")
            .field("has_external", &self.external.is_some())
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl HookExecutor for RunHookExecutor {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        self.fire_composed(event, false).await.into()
    }

    async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        self.fire_composed(event, false).await
    }

    fn dispatch_observational(self: Arc<Self>, event: HookEvent) {
        let tasks = self.tasks.clone();
        let event_type = event.event_type();
        if let Err(error) = tasks.spawn_task("hook.observation", async move {
            let _ = self.fire_composed(&event, true).await;
            Ok(())
        }) {
            tracing::warn!(
                event_type = %event_type,
                failure = %error,
                "Run-scoped observational Hook could not be supervised"
            );
        }
    }

    async fn record_agent_event(
        &self,
        event: &crate::agent::AgentEvent,
        run_id: &str,
        session_id: &str,
    ) {
        if let Some(external) = &self.external {
            external.record_agent_event(event, run_id, session_id).await;
        }
    }

    async fn record_run_cancelled(&self, run_id: &str, session_id: &str, reason: Option<&str>) {
        if let Some(external) = &self.external {
            external
                .record_run_cancelled(run_id, session_id, reason)
                .await;
        }
    }

    async fn before_run_control(
        &self,
        request: &crate::run_control::RunControlRequest,
    ) -> HookOutcome {
        if let Some(external) = &self.external {
            match external.before_run_control(request).await {
                HookOutcome::Continue(_) => {}
                terminal => return terminal,
            }
        }
        // Evaluate the immutable projected compatibility registry at the same
        // gating boundary. This keeps package policy and host policy
        // composable without allowing a Session mutation to affect a live
        // Run.
        let event = HookEvent::PreRunControl(crate::hooks::PreRunControlEvent {
            session_id: request.session_id.clone().unwrap_or_default(),
            run_id: request.run_id.clone(),
            request_id: request.request_id.clone(),
            operation: request.command.operation(),
            command: request.command.clone(),
            expected_turn_id: request.expected_turn_id.clone(),
            expected_turn_revision: request.expected_turn_revision,
            deadline_ms: request.deadline_ms,
        });
        self.snapshot.fire_outcome(&event).await
    }

    async fn record_run_control(
        &self,
        request: &crate::run_control::RunControlRequest,
        receipt: &crate::run_control::RunControlReceipt,
    ) {
        if let Some(external) = &self.external {
            external.record_run_control(request, receipt).await;
        }
        let event = HookEvent::PostRunControl(crate::hooks::PostRunControlEvent {
            session_id: receipt.session_id.clone(),
            run_id: receipt.run_id.clone(),
            request_id: receipt.request_id.clone(),
            operation: receipt.operation,
            state: receipt.state,
            sequence: receipt.sequence,
            turn_id: receipt.turn_id.clone(),
            turn_revision: receipt.turn_revision,
            accepted_at_ms: receipt.accepted_at_ms,
            applied_at_ms: receipt.applied_at_ms,
            error: receipt.error.clone(),
        });
        let _ = self.snapshot.fire_outcome(&event).await;
    }
}
