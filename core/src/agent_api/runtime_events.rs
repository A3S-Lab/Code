//! Runtime event tracking for agent runs.
//!
//! This module owns the contract from `AgentEvent` to run records, hook
//! forwarding, and active-tool state. Run orchestration can start workers without
//! knowing which events mutate tracking state.

use super::{runtime_checkpoints::RuntimeCheckpointReceiver, session_clock, AgentSession};
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
        mut checkpoints: Option<RuntimeCheckpointReceiver>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut sanitizer = self.stream_sanitizer();
            if let Some(run_events) = run_events {
                let RunAgentEventReceiver {
                    events: mut event_rx,
                    barriers: mut barrier_rx,
                } = run_events;
                let mut barrier_open = true;
                let mut checkpoint_open = checkpoints.is_some();
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
                        boundary = receive_checkpoint(&mut checkpoints), if checkpoint_open => {
                            match boundary {
                                Some(boundary) => {
                                    self.drain_agent_events(&mut event_rx, &mut sanitizer).await;
                                    self.drain_runtime_events(&mut runtime_rx, &mut sanitizer).await;
                                    if let Some(checkpoints) = &checkpoints {
                                        checkpoints.commit(boundary).await;
                                    }
                                }
                                None => checkpoint_open = false,
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
        mut checkpoints: Option<RuntimeCheckpointReceiver>,
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
                let mut checkpoint_open = checkpoints.is_some();
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
                        boundary = receive_checkpoint(&mut checkpoints), if checkpoint_open => {
                            match boundary {
                                Some(boundary) => {
                                    let drained = self
                                        .drain_agent_events_forwarded(
                                            &mut event_rx,
                                            &tx,
                                            &mut sanitizer,
                                        )
                                        .await
                                        && self
                                            .drain_runtime_events_forwarded(
                                                &mut runtime_rx,
                                                &tx,
                                                &mut sanitizer,
                                            )
                                            .await;
                                    if !drained {
                                        forward_open = false;
                                        break;
                                    }
                                    if let Some(checkpoints) = &checkpoints {
                                        checkpoints.commit(boundary).await;
                                    }
                                }
                                None => checkpoint_open = false,
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

    async fn drain_runtime_events(
        &self,
        runtime_rx: &mut mpsc::Receiver<AgentEvent>,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
    ) {
        while let Ok(event) = runtime_rx.try_recv() {
            self.observe_stream_event(sanitizer, event).await;
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

    async fn drain_runtime_events_forwarded(
        &self,
        runtime_rx: &mut mpsc::Receiver<AgentEvent>,
        tx: &mpsc::Sender<AgentEvent>,
        sanitizer: &mut crate::security::AgentEventStreamSanitizer,
    ) -> bool {
        while let Ok(event) = runtime_rx.try_recv() {
            if !self
                .observe_stream_event_and_forward(sanitizer, event, tx)
                .await
            {
                return false;
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
            // A delegated child that inherits the parent confirmation provider
            // waits for the parent UI to answer these events. Filtering them
            // here leaves the child blocked even though its confirmation is
            // registered on the shared provider.
            | AgentEvent::ConfirmationRequired { .. }
            | AgentEvent::ConfirmationReceived { .. }
            | AgentEvent::ConfirmationTimeout { .. }
    )
}

fn is_terminal_runtime_event(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::End { .. } | AgentEvent::Error { .. })
}

async fn receive_checkpoint(
    checkpoints: &mut Option<RuntimeCheckpointReceiver>,
) -> Option<super::runtime_checkpoints::RuntimeCheckpointBoundary> {
    match checkpoints {
        Some(checkpoints) => checkpoints.recv().await,
        None => None,
    }
}

#[derive(Clone)]
pub(super) struct RunCleanupState {
    run_id: String,
    active_tools: ActiveToolMap,
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    active_run_control: Arc<tokio::sync::Mutex<Option<Arc<crate::run_control::RunControlInbox>>>>,
    host_env: Arc<crate::host_env::HostEnv>,
}

impl RunCleanupState {
    pub(super) fn from_session(session: &AgentSession, run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            active_tools: Arc::clone(&session.active_tools),
            current_run_id: Arc::clone(&session.current_run_id),
            cancel_token: Arc::clone(&session.cancel_token),
            active_run_control: Arc::clone(&session.active_run_control),
            host_env: Arc::clone(&session.config.host_env),
        }
    }

    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(super) async fn set_cancel_token(&self, token: tokio_util::sync::CancellationToken) {
        *self.cancel_token.lock().await = Some(token);
    }

    pub(super) async fn clear_cancel_token(&self) {
        *self.cancel_token.lock().await = None;
    }

    pub(super) async fn finish(&self) {
        self.active_tools.write().await.clear();
        let mut current = self.current_run_id.lock().await;
        if current.as_deref() == Some(self.run_id.as_str()) {
            *current = None;
            drop(current);
            let control = self.active_run_control.lock().await.take();
            if let Some(control) = control {
                control.close(self.host_env.now_ms()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
