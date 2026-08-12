//! Event-stream bridge for direct host tool invocations.

use super::*;
use tokio::sync::{broadcast, oneshot};

impl DirectToolRuntime {
    pub(in crate::agent_api) fn spawn_call_with_agent_events(
        self,
        name: String,
        args: serde_json::Value,
    ) -> (
        mpsc::Receiver<AgentEvent>,
        JoinHandle<Result<ToolCallResult>>,
    ) {
        if let Err(error) = self.ensure_open() {
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            return (rx, tokio::spawn(async move { Err(error) }));
        }
        let (agent_tx, agent_rx) = mpsc::channel::<AgentEvent>(256);
        let (runtime_tx, mut runtime_rx) = mpsc::channel::<AgentEvent>(256);
        let (broadcast_tx, mut broadcast_rx) = broadcast::channel::<AgentEvent>(256);
        let (broadcast_shutdown_tx, mut broadcast_shutdown_rx) = oneshot::channel::<()>();
        let security_provider = self.security_provider.clone();
        let forward_tx = agent_tx.clone();
        let forwarder = tokio::spawn(async move {
            let mut sanitizer = crate::security::AgentEventStreamSanitizer::new(security_provider);
            let mut runtime_open = true;
            let mut broadcast_open = true;
            let mut broadcast_drain_remaining = None;
            while runtime_open || broadcast_open {
                let event = tokio::select! {
                    biased;
                    _ = &mut broadcast_shutdown_rx,
                        if broadcast_open && broadcast_drain_remaining.is_none() =>
                    {
                        // A detached producer (notably `task` with
                        // `background: true`) may retain its sender long after
                        // the direct tool invocation returns. Snapshot the
                        // already-accepted backlog at the invocation boundary
                        // and drain only that prefix instead of waiting for all
                        // producer clones to disappear.
                        let pending = broadcast_rx.len();
                        if pending == 0 {
                            broadcast_open = false;
                        } else {
                            broadcast_drain_remaining = Some(pending);
                        }
                        None
                    }
                    event = runtime_rx.recv(), if runtime_open => {
                        match event {
                            Some(event) => Some(event),
                            None => {
                                runtime_open = false;
                                None
                            }
                        }
                    }
                    event = broadcast_rx.recv(), if broadcast_open => {
                        match event {
                            Ok(event) => {
                                if let Some(remaining) = &mut broadcast_drain_remaining {
                                    *remaining = remaining.saturating_sub(1);
                                    if *remaining == 0 {
                                        broadcast_open = false;
                                    }
                                }
                                Some(event)
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(skipped, "direct tool event bridge lagged");
                                if let Some(remaining) = &mut broadcast_drain_remaining {
                                    let skipped = usize::try_from(skipped).unwrap_or(usize::MAX);
                                    *remaining = remaining.saturating_sub(skipped);
                                    if *remaining == 0 {
                                        broadcast_open = false;
                                    }
                                }
                                None
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                broadcast_open = false;
                                None
                            }
                        }
                    }
                };
                let Some(event) = event else {
                    continue;
                };
                for event in sanitizer.push(event) {
                    if forward_tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
            for event in sanitizer.finish() {
                if forward_tx.send(event).await.is_err() {
                    return;
                }
            }
        });

        let agent_loop = self.agent_loop;
        let session_id = self.session_id;
        let cancel = self.session_cancel.child_token();
        let task_scheduler = self.task_scheduler;
        let task_priority = self.task_priority;
        let closed = self.closed;
        let mut ctx = self.tool_context;
        ctx.agent_event_tx = Some(broadcast_tx);
        let tool_name = name.clone();
        let tool_id = format!("host-{tool_name}-{}", uuid::Uuid::new_v4());
        let event_args = args.clone();
        let event_tx = Some(runtime_tx);
        let security_provider = self.security_provider;
        let handle = tokio::spawn(async move {
            let _task_lease = acquire_task_admission(
                task_scheduler.as_deref(),
                task_priority,
                &session_id,
                &tool_name,
                &cancel,
                &closed,
            )
            .await?;
            if closed.load(Ordering::Acquire) {
                return Err(crate::error::CodeError::SessionClosed {
                    session_id: session_id.clone(),
                });
            }
            let result = agent_loop
                .invoke_host_tool(
                    ToolInvocation::host_direct(tool_id.clone(), tool_name.clone(), args),
                    &session_id,
                    &event_tx,
                    &cancel,
                    &ctx,
                )
                .await;
            // Runtime deltas have an owned producer lifecycle, so closing that
            // path still guarantees all tool-stream deltas precede ToolEnd.
            // The broadcast path uses an explicit invocation boundary because
            // detached background tasks intentionally outlive this call.
            broadcast_shutdown_tx.send(()).ok();
            drop(event_tx);
            drop(ctx);
            if let Err(error) = forwarder.await {
                tracing::warn!(%error, "direct tool event bridge failed");
            }
            let end = AgentEvent::ToolEnd {
                id: tool_id,
                name: tool_name,
                args: Some(event_args),
                output: result.output.clone(),
                exit_code: result.exit_code,
                metadata: result.metadata.clone(),
                error_kind: result.error_kind.clone(),
            };
            let end = security_provider
                .as_deref()
                .map(|provider| crate::security::sanitize_agent_event(provider, &end))
                .unwrap_or(end);
            agent_tx.send(end).await.ok();
            Ok(project_tool_result(result))
        });

        (agent_rx, handle)
    }
}
