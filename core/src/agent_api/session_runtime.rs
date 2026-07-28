//! Session runtime wiring.
//!
//! Capabilities describe what the agent can do. This module wires the per-session
//! runtime channels and adapters that control how those capabilities execute.

use super::SessionOptions;
use crate::agent::AgentEvent;
use crate::config::CodeConfig;
use crate::error::{CodeError, Result, SessionBuildResource};
use crate::hitl::ConfirmationProvider;
use crate::session_lane_queue::SessionLaneQueue;
use crate::tools::{ToolContext, ToolExecutor};
use std::sync::Arc;
use tokio::sync::broadcast;

pub(super) struct SessionRuntimeInput<'a> {
    pub(super) code_config: &'a CodeConfig,
    pub(super) search_bulkhead: &'a a3s_search::Bulkhead,
    pub(super) search_retry_budget: &'a a3s_search::RetryBudget,
    pub(super) session_id: &'a str,
    pub(super) opts: &'a SessionOptions,
    pub(super) tool_executor: Arc<ToolExecutor>,
}

pub(super) struct SessionRuntime {
    pub(super) confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
    pub(super) command_queue: Option<Arc<SessionLaneQueue>>,
    pub(super) tool_context: ToolContext,
}

pub(super) async fn build_session_runtime(
    input: SessionRuntimeInput<'_>,
) -> Result<SessionRuntime> {
    let (agent_event_tx, _) = broadcast::channel::<AgentEvent>(2048);

    let confirmation_manager = build_confirmation_manager(input.opts, agent_event_tx.clone());
    let command_queue =
        build_command_queue(input.opts, input.session_id, agent_event_tx.clone()).await?;
    let tool_context = build_tool_context(&input, agent_event_tx);

    Ok(SessionRuntime {
        confirmation_manager,
        command_queue,
        tool_context,
    })
}

pub(super) fn build_session_runtime_sync(input: SessionRuntimeInput<'_>) -> SessionRuntime {
    let (agent_event_tx, _) = broadcast::channel::<AgentEvent>(2048);
    let confirmation_manager = build_confirmation_manager(input.opts, agent_event_tx.clone());
    let tool_context = build_tool_context(&input, agent_event_tx);
    SessionRuntime {
        confirmation_manager,
        command_queue: None,
        tool_context,
    }
}

fn build_confirmation_manager(
    opts: &SessionOptions,
    agent_event_tx: broadcast::Sender<AgentEvent>,
) -> Option<Arc<dyn ConfirmationProvider>> {
    if opts.confirmation_manager.is_some() {
        opts.confirmation_manager.clone()
    } else if let Some(policy) = &opts.confirmation_policy {
        let manager = Arc::new(crate::hitl::ConfirmationManager::new(
            policy.clone(),
            agent_event_tx,
        ));
        Some(manager as Arc<dyn ConfirmationProvider>)
    } else {
        None
    }
}

async fn build_command_queue(
    opts: &SessionOptions,
    session_id: &str,
    agent_event_tx: broadcast::Sender<AgentEvent>,
) -> Result<Option<Arc<SessionLaneQueue>>> {
    let Some(queue_config) = opts.queue_config.as_ref() else {
        return Ok(None);
    };

    let queue = SessionLaneQueue::new(session_id, queue_config.clone(), agent_event_tx)
        .await
        .map_err(|error| CodeError::SessionInitialization {
            resource: SessionBuildResource::Queue,
            message: format!("session '{session_id}': {error:#}"),
        })?;
    let queue = Arc::new(queue);
    queue
        .start()
        .await
        .map_err(|error| CodeError::SessionInitialization {
            resource: SessionBuildResource::Queue,
            message: format!("session '{session_id}': {error:#}"),
        })?;
    Ok(Some(queue))
}

fn build_tool_context(
    input: &SessionRuntimeInput<'_>,
    agent_event_tx: broadcast::Sender<AgentEvent>,
) -> ToolContext {
    let mut tool_context = input.tool_executor.registry().context();
    tool_context = tool_context.with_search_runtime(
        input.search_bulkhead.clone(),
        input.search_retry_budget.clone(),
    );
    tool_context = tool_context.with_session_id(input.session_id);
    if let Some(ref search_config) = input.code_config.search {
        tool_context = tool_context.with_search_config(search_config.clone());
    }
    tool_context = tool_context.with_agent_event_tx(agent_event_tx);

    if let Some(handle) = input.opts.sandbox_handle.clone() {
        input
            .tool_executor
            .registry()
            .set_sandbox(Arc::clone(&handle));
        tool_context = tool_context.with_sandbox(handle);
    }

    tool_context
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SearchConfig, SearchHealthConfig};
    use a3s_search::{BulkheadConfig, BulkheadRejection, EngineFailure, RetryBudgetConfig};
    use std::collections::HashMap;
    use std::time::Duration;

    #[tokio::test]
    async fn session_contexts_share_agent_search_limits_but_isolate_circuits() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let tool_executor = Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().to_string(),
        ));
        let code_config = CodeConfig {
            search: Some(SearchConfig {
                timeout: 10,
                health: Some(SearchHealthConfig {
                    max_failures: 1,
                    suspend_seconds: 60,
                }),
                engines: HashMap::new(),
                headless: None,
            }),
            ..CodeConfig::default()
        };
        let bulkhead = a3s_search::Bulkhead::new(BulkheadConfig {
            max_concurrent: 1,
            max_queued: 0,
            max_queue_wait: Duration::ZERO,
        });
        let retry_budget = a3s_search::RetryBudget::new(RetryBudgetConfig {
            capacity: 1,
            retry_cost: 1,
            success_credit: 0,
        });
        let opts = SessionOptions::default();
        let (first_tx, _) = broadcast::channel(1);
        let first = build_tool_context(
            &SessionRuntimeInput {
                code_config: &code_config,
                search_bulkhead: &bulkhead,
                search_retry_budget: &retry_budget,
                session_id: "session-a",
                opts: &opts,
                tool_executor: Arc::clone(&tool_executor),
            },
            first_tx,
        );
        let (second_tx, _) = broadcast::channel(1);
        let second = build_tool_context(
            &SessionRuntimeInput {
                code_config: &code_config,
                search_bulkhead: &bulkhead,
                search_retry_budget: &retry_budget,
                session_id: "session-b",
                opts: &opts,
                tool_executor,
            },
            second_tx,
        );

        let _permit = first
            .search_bulkhead()
            .acquire("shared-engine")
            .await
            .expect("first session should acquire shared capacity");
        assert_eq!(
            second
                .search_bulkhead()
                .acquire("shared-engine")
                .await
                .expect_err("second session must observe the occupied capacity"),
            BulkheadRejection::Saturated
        );

        assert!(first.search_retry_budget().try_acquire_retry());
        assert!(
            !second.search_retry_budget().try_acquire_retry(),
            "retry consumption in one session must be visible to its siblings"
        );

        first
            .search_circuit_breaker()
            .acquire("failing-engine")
            .expect("first probe")
            .record_failure(&EngineFailure::new(
                "Failing Engine",
                "provider_unavailable",
                "synthetic failure",
            ));
        assert!(first
            .search_circuit_breaker()
            .acquire("failing-engine")
            .is_err());
        assert!(
            second
                .search_circuit_breaker()
                .acquire("failing-engine")
                .is_ok(),
            "one session's upstream history must not suppress another session"
        );
    }
}
