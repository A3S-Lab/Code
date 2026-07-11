//! Runtime event tracking for agent runs.
//!
//! This module owns the contract from `AgentEvent` to run records, hook
//! forwarding, and active-tool state. Run orchestration can start workers without
//! knowing which events mutate tracking state.

use super::{session_clock, AgentSession};
use crate::agent::AgentEvent;
use crate::tools::{AgentEventBarrier, AgentEventBarrierReceiver};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub(super) struct ActiveToolState {
    pub(super) tool_name: String,
    pub(super) started_at_ms: u64,
}

type ActiveToolMap = Arc<tokio::sync::RwLock<HashMap<String, ActiveToolState>>>;

pub(super) struct RunAgentEventReceiver {
    events: broadcast::Receiver<AgentEvent>,
    barriers: AgentEventBarrierReceiver,
}

pub(super) fn run_agent_event_channel(
    capacity: usize,
) -> (
    broadcast::Sender<AgentEvent>,
    AgentEventBarrier,
    RunAgentEventReceiver,
) {
    let (event_tx, event_rx) = broadcast::channel(capacity);
    let (barrier, barriers) = AgentEventBarrier::channel(32);
    (
        event_tx,
        barrier,
        RunAgentEventReceiver {
            events: event_rx,
            barriers,
        },
    )
}

pub(super) async fn active_tool_snapshots(
    active_tools: &ActiveToolMap,
) -> Vec<crate::run::ActiveToolSnapshot> {
    let mut snapshots = active_tools
        .read()
        .await
        .iter()
        .map(|(id, tool)| crate::run::ActiveToolSnapshot {
            id: id.clone(),
            name: tool.tool_name.clone(),
            started_at_ms: tool.started_at_ms,
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|a, b| {
        a.started_at_ms
            .cmp(&b.started_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    snapshots
}

#[derive(Clone)]
pub(super) struct RuntimeEventSink {
    run_store: Arc<crate::run::InMemoryRunStore>,
    run_id: String,
    session_id: String,
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
    persistence_state: Arc<std::sync::RwLock<super::session_persistence::SessionPersistenceState>>,
    active_tools: ActiveToolMap,
    subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
}

struct RuntimeEventSinkConfig {
    run_store: Arc<crate::run::InMemoryRunStore>,
    run_id: String,
    session_id: String,
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
    persistence_state: Arc<std::sync::RwLock<super::session_persistence::SessionPersistenceState>>,
    active_tools: ActiveToolMap,
    subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
}

impl RuntimeEventSink {
    pub(super) fn from_session(session: &AgentSession, run_id: &str) -> Self {
        Self::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&session.run_store),
            run_id: run_id.to_string(),
            session_id: session.session_id.clone(),
            hook_executor: session.hook_executor.clone(),
            security_provider: session.config.security_provider.clone(),
            persistence_state: Arc::clone(&session.persistence_state),
            active_tools: Arc::clone(&session.active_tools),
            subagent_tasks: Arc::clone(&session.subagent_tasks),
        })
    }

    fn new(config: RuntimeEventSinkConfig) -> Self {
        let RuntimeEventSinkConfig {
            run_store,
            run_id,
            session_id,
            hook_executor,
            security_provider,
            persistence_state,
            active_tools,
            subagent_tasks,
        } = config;
        Self {
            run_store,
            run_id,
            session_id,
            hook_executor,
            security_provider,
            persistence_state,
            active_tools,
            subagent_tasks,
        }
    }

    pub(super) fn spawn_collector(
        self,
        mut runtime_rx: mpsc::Receiver<AgentEvent>,
        run_events: Option<RunAgentEventReceiver>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut sanitizer = self.stream_sanitizer();
            if let Some(run_events) = run_events {
                let RunAgentEventReceiver {
                    events: mut event_rx,
                    barriers: mut barrier_rx,
                } = run_events;
                let mut barrier_open = true;
                loop {
                    tokio::select! {
                        event = runtime_rx.recv() => {
                            match event {
                                Some(event) => {
                                    if is_terminal_runtime_event(&event) {
                                        self.drain_agent_events(&mut event_rx, &mut sanitizer).await;
                                    }
                                    self.observe_stream_event(&mut sanitizer, event).await;
                                }
                                None => {
                                    self.drain_agent_events(&mut event_rx, &mut sanitizer).await;
                                    break;
                                }
                            }
                        }
                        event = event_rx.recv() => {
                            match event {
                                Ok(event) if should_bridge_agent_event(&event) => {
                                    self.observe_stream_event(&mut sanitizer, event).await;
                                }
                                Ok(_) => {}
                                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                    tracing::warn!(skipped, "run event bridge lagged while collecting run events");
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    while let Some(event) = runtime_rx.recv().await {
                                        self.observe_stream_event(&mut sanitizer, event).await;
                                    }
                                    break;
                                }
                            }
                        }
                        barrier = barrier_rx.recv(), if barrier_open => {
                            match barrier {
                                Some(ack) => {
                                    self.drain_agent_events(&mut event_rx, &mut sanitizer).await;
                                    let _ = ack.send(());
                                }
                                None => barrier_open = false,
                            }
                        }
                    }
                }
            } else {
                while let Some(event) = runtime_rx.recv().await {
                    self.observe_stream_event(&mut sanitizer, event).await;
                }
            }
            self.finish_stream(&mut sanitizer).await;
        })
    }

    pub(super) fn spawn_forwarder(
        self,
        mut runtime_rx: mpsc::Receiver<AgentEvent>,
        tx: mpsc::Sender<AgentEvent>,
        run_events: Option<RunAgentEventReceiver>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut sanitizer = self.stream_sanitizer();
            let mut forward_open = true;
            if let Some(run_events) = run_events {
                let RunAgentEventReceiver {
                    events: mut event_rx,
                    barriers: mut barrier_rx,
                } = run_events;
                let mut barrier_open = true;
                loop {
                    tokio::select! {
                        event = runtime_rx.recv() => {
                            match event {
                                Some(event) => {
                                    if is_terminal_runtime_event(&event)
                                        && !self.drain_agent_events_forwarded(
                                            &mut event_rx,
                                            &tx,
                                            &mut sanitizer,
                                        ).await
                                    {
                                        forward_open = false;
                                        break;
                                    }
                                    if !self.observe_stream_event_and_forward(
                                        &mut sanitizer,
                                        event,
                                        &tx,
                                    ).await {
                                        forward_open = false;
                                        break;
                                    }
                                }
                                None => {
                                    forward_open = self
                                        .drain_agent_events_forwarded(
                                            &mut event_rx,
                                            &tx,
                                            &mut sanitizer,
                                        )
                                        .await;
                                    break;
                                }
                            }
                        }
                        event = event_rx.recv() => {
                            match event {
                                Ok(event) if should_bridge_agent_event(&event) => {
                                    if !self.observe_stream_event_and_forward(
                                        &mut sanitizer,
                                        event,
                                        &tx,
                                    ).await {
                                        forward_open = false;
                                        break;
                                    }
                                }
                                Ok(_) => {}
                                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                    tracing::warn!(skipped, "run event bridge lagged while streaming run events");
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    while let Some(event) = runtime_rx.recv().await {
                                        if !self.observe_stream_event_and_forward(
                                            &mut sanitizer,
                                            event,
                                            &tx,
                                        ).await {
                                            forward_open = false;
                                            break;
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        barrier = barrier_rx.recv(), if barrier_open => {
                            match barrier {
                                Some(ack) => {
                                    let drained = self
                                        .drain_agent_events_forwarded(
                                            &mut event_rx,
                                            &tx,
                                            &mut sanitizer,
                                        )
                                        .await;
                                    let _ = ack.send(());
                                    if !drained {
                                        forward_open = false;
                                        break;
                                    }
                                }
                                None => barrier_open = false,
                            }
                        }
                    }
                }
            } else {
                while let Some(event) = runtime_rx.recv().await {
                    if !self
                        .observe_stream_event_and_forward(&mut sanitizer, event, &tx)
                        .await
                    {
                        forward_open = false;
                        break;
                    }
                }
            }
            if forward_open {
                self.finish_stream_forwarded(&mut sanitizer, &tx).await;
            }
        })
    }

    #[cfg(test)]
    pub(super) async fn observe(&self, event: &AgentEvent) {
        let event = self.sanitize(event);
        self.observe_sanitized(&event).await;
    }

    #[cfg(test)]
    fn sanitize(&self, event: &AgentEvent) -> AgentEvent {
        self.security_provider
            .as_deref()
            .map(|provider| crate::security::sanitize_agent_event(provider, event))
            .unwrap_or_else(|| event.clone())
    }

    fn stream_sanitizer(&self) -> crate::security::AgentEventStreamSanitizer {
        crate::security::AgentEventStreamSanitizer::new(self.security_provider.clone())
    }

    async fn observe_stream_event(
        &self,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
        event: AgentEvent,
    ) {
        for event in sanitizer.push(event) {
            self.observe_sanitized(&event).await;
        }
    }

    async fn finish_stream(&self, sanitizer: &mut crate::security::AgentEventStreamSanitizer) {
        for event in sanitizer.finish() {
            self.observe_sanitized(&event).await;
        }
    }

    async fn observe_sanitized(&self, event: &AgentEvent) {
        let _ = self
            .run_store
            .record_event(&self.run_id, event.clone())
            .await;
        if let Some(executor) = &self.hook_executor {
            executor
                .record_agent_event(event, &self.run_id, &self.session_id)
                .await;
        }
        self.subagent_tasks.record_event(event).await;
        self.apply(event).await;
    }

    async fn observe_sanitized_and_forward(
        &self,
        event: AgentEvent,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> bool {
        self.observe_sanitized(&event).await;
        if tx.send(event).await.is_ok() {
            true
        } else {
            // Receiver dropped or buffer full; preserve the existing stream contract
            // by stopping instead of silently dropping later terminal events.
            tracing::warn!("stream forwarder: receiver dropped, stopping event forward");
            false
        }
    }

    async fn observe_stream_event_and_forward(
        &self,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
        event: AgentEvent,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> bool {
        for event in sanitizer.push(event) {
            if !self.observe_sanitized_and_forward(event, tx).await {
                return false;
            }
        }
        true
    }

    async fn finish_stream_forwarded(
        &self,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
        tx: &mpsc::Sender<AgentEvent>,
    ) {
        for event in sanitizer.finish() {
            if !self.observe_sanitized_and_forward(event, tx).await {
                break;
            }
        }
    }

    async fn drain_agent_events(
        &self,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
    ) {
        loop {
            match event_rx.try_recv() {
                Ok(event) if should_bridge_agent_event(&event) => {
                    self.observe_stream_event(sanitizer, event).await
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "run event bridge lagged while draining run events");
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
    }

    async fn drain_agent_events_forwarded(
        &self,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        tx: &mpsc::Sender<AgentEvent>,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
    ) -> bool {
        loop {
            match event_rx.try_recv() {
                Ok(event) if should_bridge_agent_event(&event) => {
                    if !self
                        .observe_stream_event_and_forward(sanitizer, event, tx)
                        .await
                    {
                        return false;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        skipped,
                        "run event bridge lagged while draining streamed run events"
                    );
                }
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        true
    }

    async fn apply(&self, event: &AgentEvent) {
        match event {
            AgentEvent::End { usage, .. } => {
                crate::error::write_or_recover(&self.persistence_state).record_usage(usage);
            }
            AgentEvent::TaskUpdated { tasks, .. } => {
                crate::error::write_or_recover(&self.persistence_state)
                    .replace_tasks(tasks.clone());
            }
            AgentEvent::ToolExecutionStart { id, name, .. } => {
                self.active_tools.write().await.insert(
                    id.clone(),
                    ActiveToolState {
                        tool_name: name.clone(),
                        started_at_ms: session_clock::now_ms(),
                    },
                );
            }
            AgentEvent::ToolEnd { id, .. }
            | AgentEvent::PermissionDenied { tool_id: id, .. }
            | AgentEvent::ConfirmationRequired { tool_id: id, .. }
            | AgentEvent::ConfirmationReceived { tool_id: id, .. }
            | AgentEvent::ConfirmationTimeout { tool_id: id, .. } => {
                self.active_tools.write().await.remove(id);
            }
            _ => {}
        }
    }
}

fn should_bridge_agent_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::SubagentStart { .. }
            | AgentEvent::SubagentProgress { .. }
            | AgentEvent::SubagentEnd { .. }
    )
}

fn is_terminal_runtime_event(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::End { .. } | AgentEvent::Error { .. })
}

#[derive(Clone)]
pub(super) struct RunCleanupState {
    run_id: String,
    active_tools: ActiveToolMap,
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
}

impl RunCleanupState {
    pub(super) fn from_session(session: &AgentSession, run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            active_tools: Arc::clone(&session.active_tools),
            current_run_id: Arc::clone(&session.current_run_id),
            cancel_token: Arc::clone(&session.cancel_token),
        }
    }

    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(super) async fn set_cancel_token(&self, token: tokio_util::sync::CancellationToken) {
        *self.cancel_token.lock().await = Some(token);
    }

    /// Share the per-run cancel-token slot. Used by stream worker state to
    /// observe cancellation when classifying a failed run.
    pub(super) fn cancel_token_slot(
        &self,
    ) -> Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>> {
        Arc::clone(&self.cancel_token)
    }

    pub(super) async fn clear_cancel_token(&self) {
        *self.cancel_token.lock().await = None;
    }

    /// Returns `true` when the per-run cancellation token (or any parent it
    /// was derived from, such as the session-level token) has been fired.
    /// Used by lifecycle `complete()` to classify a failed run as `Cancelled`
    /// vs `Failed` when an `Err` comes back from the agent loop.
    pub(super) async fn was_cancelled(&self) -> bool {
        self.cancel_token
            .lock()
            .await
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false)
    }

    pub(super) async fn finish(&self) {
        self.active_tools.write().await.clear();
        let mut current = self.current_run_id.lock().await;
        if current.as_deref() == Some(self.run_id.as_str()) {
            *current = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct RecordingRuntimeEvents {
        events: std::sync::Mutex<Vec<AgentEvent>>,
    }

    #[async_trait::async_trait]
    impl crate::hooks::HookExecutor for RecordingRuntimeEvents {
        async fn fire(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
            crate::hooks::HookResult::Continue(None)
        }

        async fn record_agent_event(&self, event: &AgentEvent, _run_id: &str, _session_id: &str) {
            self.events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event.clone());
        }
    }

    fn active_tools() -> ActiveToolMap {
        Arc::new(tokio::sync::RwLock::new(HashMap::new()))
    }

    fn persistence_state(
    ) -> Arc<std::sync::RwLock<super::super::session_persistence::SessionPersistenceState>> {
        Arc::new(std::sync::RwLock::new(
            super::super::session_persistence::SessionPersistenceState::default(),
        ))
    }

    #[tokio::test]
    async fn tool_events_update_active_tool_state() {
        let run_store = Arc::new(crate::run::InMemoryRunStore::new());
        let run = run_store.create_run("session-1", "prompt").await;
        let active_tools = active_tools();
        let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&run_store),
            run_id: run.id.clone(),
            session_id: "session-1".to_string(),
            hook_executor: None,
            security_provider: None,
            persistence_state: persistence_state(),
            active_tools: Arc::clone(&active_tools),
            subagent_tasks: Arc::new(
                crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
            ),
        });

        sink.observe(&AgentEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
        })
        .await;
        assert!(
            active_tools.read().await.is_empty(),
            "model-side tool preparation must not be reported as running"
        );

        sink.observe(&AgentEvent::ToolExecutionStart {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            args: serde_json::json!({ "command": "true" }),
        })
        .await;
        assert_eq!(active_tools.read().await.len(), 1);
        assert_eq!(
            active_tools
                .read()
                .await
                .get("tool-1")
                .map(|tool| tool.tool_name.as_str()),
            Some("bash")
        );

        sink.observe(&AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            args: Some(serde_json::json!({ "command": "true" })),
            output: "ok".to_string(),
            exit_code: 0,
            metadata: None,
            error_kind: None,
        })
        .await;
        assert!(active_tools.read().await.is_empty());
    }

    #[tokio::test]
    async fn observe_records_events_on_run_store() {
        let run_store = Arc::new(crate::run::InMemoryRunStore::new());
        let run = run_store.create_run("session-1", "prompt").await;
        let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&run_store),
            run_id: run.id.clone(),
            session_id: "session-1".to_string(),
            hook_executor: None,
            security_provider: None,
            persistence_state: persistence_state(),
            active_tools: active_tools(),
            subagent_tasks: Arc::new(
                crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
            ),
        });

        sink.observe(&AgentEvent::TextDelta {
            text: "hello".to_string(),
        })
        .await;

        let events = run_store.events(&run.id).await;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, AgentEvent::TextDelta { .. }));
        assert_eq!(run_store.snapshot(&run.id).await.unwrap().event_count, 1);
    }

    #[tokio::test]
    async fn run_owned_agent_events_do_not_cross_run_boundaries() {
        let run_store = Arc::new(crate::run::InMemoryRunStore::new());
        let run_a = run_store.create_run("session-1", "run a").await;
        let run_b = run_store.create_run("session-1", "run b").await;
        let tracker = Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new());

        let sink_a = RuntimeEventSink::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&run_store),
            run_id: run_a.id.clone(),
            session_id: "session-1".to_string(),
            hook_executor: None,
            security_provider: None,
            persistence_state: persistence_state(),
            active_tools: active_tools(),
            subagent_tasks: Arc::clone(&tracker),
        });
        let sink_b = RuntimeEventSink::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&run_store),
            run_id: run_b.id.clone(),
            session_id: "session-1".to_string(),
            hook_executor: None,
            security_provider: None,
            persistence_state: persistence_state(),
            active_tools: active_tools(),
            subagent_tasks: tracker,
        });

        let (runtime_tx_a, runtime_rx_a) = mpsc::channel(4);
        let (runtime_tx_b, runtime_rx_b) = mpsc::channel(4);
        let (agent_tx_a, barrier_a, agent_rx_a) = run_agent_event_channel(8);
        let (agent_tx_b, barrier_b, agent_rx_b) = run_agent_event_channel(8);
        let collector_a = sink_a.spawn_collector(runtime_rx_a, Some(agent_rx_a));
        let collector_b = sink_b.spawn_collector(runtime_rx_b, Some(agent_rx_b));

        // Run B is already active when a background child owned by Run A
        // finishes. The per-run sender must route the late event only to A.
        runtime_tx_b
            .send(AgentEvent::TextDelta {
                text: "run b active".to_string(),
            })
            .await
            .unwrap();
        agent_tx_a
            .send(AgentEvent::SubagentEnd {
                task_id: "late-task-a".to_string(),
                session_id: "task-run-late-task-a".to_string(),
                agent: "explore".to_string(),
                output: "late result".to_string(),
                success: true,
                finished_ms: 1,
            })
            .unwrap();
        barrier_a.flush().await;

        agent_tx_b
            .send(AgentEvent::SubagentStart {
                task_id: "task-b".to_string(),
                session_id: "task-run-task-b".to_string(),
                parent_session_id: "session-1".to_string(),
                agent: "explore".to_string(),
                description: "owned by b".to_string(),
                started_ms: 2,
            })
            .unwrap();
        barrier_b.flush().await;

        drop(runtime_tx_a);
        drop(runtime_tx_b);
        collector_a.await.unwrap();
        collector_b.await.unwrap();

        let events_a = run_store.events(&run_a.id).await;
        let events_b = run_store.events(&run_b.id).await;
        assert!(events_a.iter().any(|record| matches!(
            &record.event,
            AgentEvent::SubagentEnd { task_id, .. } if task_id == "late-task-a"
        )));
        assert!(!events_b.iter().any(|record| matches!(
            &record.event,
            AgentEvent::SubagentEnd { task_id, .. } if task_id == "late-task-a"
        )));
        assert!(events_b.iter().any(|record| matches!(
            &record.event,
            AgentEvent::SubagentStart { task_id, .. } if task_id == "task-b"
        )));
    }

    #[tokio::test]
    async fn forwarder_exposes_and_persists_only_sanitized_events() {
        let run_store = Arc::new(crate::run::InMemoryRunStore::new());
        let run = run_store.create_run("session-1", "prompt").await;
        let provider: Arc<dyn crate::security::SecurityProvider> =
            Arc::new(crate::security::DefaultSecurityProvider::new());
        let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&run_store),
            run_id: run.id.clone(),
            session_id: "session-1".to_string(),
            hook_executor: None,
            security_provider: Some(provider),
            persistence_state: persistence_state(),
            active_tools: active_tools(),
            subagent_tasks: Arc::new(
                crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
            ),
        });
        let (runtime_tx, runtime_rx) = mpsc::channel(4);
        let (stream_tx, mut stream_rx) = mpsc::channel(4);
        let forwarder = sink.spawn_forwarder(runtime_rx, stream_tx, None);

        runtime_tx
            .send(AgentEvent::ToolEnd {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                args: Some(serde_json::json!({"command": "echo user@example.com"})),
                output: "user@example.com".to_string(),
                exit_code: 0,
                metadata: None,
                error_kind: None,
            })
            .await
            .unwrap();
        drop(runtime_tx);
        forwarder.await.unwrap();

        let streamed = stream_rx.recv().await.unwrap();
        let persisted = run_store.events(&run.id).await;
        assert_eq!(persisted.len(), 1);
        for event in [&streamed, &persisted[0].event] {
            let json = serde_json::to_string(event).unwrap();
            assert!(
                !json.contains("user@example.com"),
                "unsanitized event: {json}"
            );
            assert!(json.contains("REDACTED:EMAIL"));
            assert!(json.contains("tool-1"));
        }
    }

    #[tokio::test]
    async fn split_stream_secret_is_sanitized_before_stream_store_and_hooks() {
        let run_store = Arc::new(crate::run::InMemoryRunStore::new());
        let run = run_store.create_run("session-1", "prompt").await;
        let provider: Arc<dyn crate::security::SecurityProvider> =
            Arc::new(crate::security::DefaultSecurityProvider::new());
        let hook = Arc::new(RecordingRuntimeEvents::default());
        let hook_executor: Arc<dyn crate::hooks::HookExecutor> = hook.clone();
        let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
            run_store: Arc::clone(&run_store),
            run_id: run.id.clone(),
            session_id: "session-1".to_string(),
            hook_executor: Some(hook_executor),
            security_provider: Some(provider),
            persistence_state: persistence_state(),
            active_tools: active_tools(),
            subagent_tasks: Arc::new(
                crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
            ),
        });
        let (runtime_tx, runtime_rx) = mpsc::channel(32);
        let (stream_tx, mut stream_rx) = mpsc::channel(32);
        let forwarder = sink.spawn_forwarder(runtime_rx, stream_tx, None);

        for event in [
            AgentEvent::TextDelta {
                text: "text@".to_string(),
            },
            AgentEvent::TextDelta {
                text: "example.com".to_string(),
            },
            AgentEvent::ReasoningDelta {
                text: "reasoning@".to_string(),
            },
            AgentEvent::ReasoningDelta {
                text: "example.com".to_string(),
            },
            AgentEvent::ToolInputDelta {
                id: Some("tool-1".to_string()),
                delta: "input@".to_string(),
            },
            AgentEvent::ToolInputDelta {
                id: Some("tool-1".to_string()),
                delta: "example.com".to_string(),
            },
            AgentEvent::ToolExecutionStart {
                id: "tool-1".to_string(),
                name: "test".to_string(),
                args: serde_json::json!({}),
            },
            AgentEvent::ToolOutputDelta {
                id: "tool-1".to_string(),
                name: "test".to_string(),
                delta: "output@".to_string(),
            },
            AgentEvent::ToolOutputDelta {
                id: "tool-1".to_string(),
                name: "test".to_string(),
                delta: "example.com".to_string(),
            },
            AgentEvent::ToolEnd {
                id: "tool-1".to_string(),
                name: "test".to_string(),
                args: Some(serde_json::json!({})),
                output: "done".to_string(),
                exit_code: 0,
                metadata: None,
                error_kind: None,
            },
            AgentEvent::End {
                text: "done".to_string(),
                usage: crate::llm::TokenUsage::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                meta: None,
            },
        ] {
            runtime_tx.send(event).await.unwrap();
        }
        drop(runtime_tx);
        forwarder.await.unwrap();

        let mut streamed = Vec::new();
        while let Some(event) = stream_rx.recv().await {
            streamed.push(event);
        }
        let persisted = run_store
            .events(&run.id)
            .await
            .into_iter()
            .map(|record| record.event)
            .collect::<Vec<_>>();
        let hooked = hook
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        for events in [&streamed, &persisted, &hooked] {
            let serialized = serde_json::to_string(events).unwrap();
            for secret in [
                "text@example.com",
                "reasoning@example.com",
                "input@example.com",
                "output@example.com",
            ] {
                assert!(!serialized.contains(secret), "unsanitized secret: {secret}");
            }
            assert_eq!(serialized.matches("REDACTED:EMAIL").count(), 4);
            assert!(matches!(events.last(), Some(AgentEvent::End { .. })));
        }
        assert_eq!(
            serde_json::to_value(&streamed).unwrap(),
            serde_json::to_value(&persisted).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&streamed).unwrap(),
            serde_json::to_value(&hooked).unwrap()
        );
    }
}
