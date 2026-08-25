use super::{
    agent_loop_runtime::build_pinned_agent_loop,
    run_lifecycle::{
        BlockingRunLifecycle, RunControlState, StreamRunLifecycle, StreamRunWorkerState,
    },
    runtime_checkpoints::runtime_checkpoint_channel,
    runtime_events::{run_agent_event_channel, RuntimeEventSink},
    session_persistence::SessionPersistenceContext,
    AgentSession,
};
use crate::agent::{AgentEvent, AgentLoop, AgentResult, InvocationContext};
use crate::error::{read_or_recover, CodeError, Result};
use crate::llm::{Attachment, Message};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub(super) struct ConversationInput {
    pub(super) messages: Vec<Message>,
    pub(super) persistence: Option<SessionPersistenceContext>,
}

impl ConversationInput {
    pub(super) fn from_history(session: &AgentSession, history: Option<&[Message]>) -> Self {
        let use_internal = history.is_none();
        let messages = match history {
            Some(history) => history.to_vec(),
            None => read_or_recover(&session.history).clone(),
        };
        Self {
            messages,
            persistence: use_internal.then(|| SessionPersistenceContext::from_session(session)),
        }
    }

    pub(super) fn with_attachments(
        session: &AgentSession,
        history: Option<&[Message]>,
        prompt: &str,
        attachments: &[Attachment],
    ) -> Self {
        let mut input = Self::from_history(session, history);
        input
            .messages
            .push(Message::user_with_attachments(prompt, attachments));
        input
    }
}

pub(super) struct BlockingRunContext {
    agent_loop: AgentLoop,
    capability_run: crate::capability::SessionCapabilityRun,
    invocation: InvocationContext,
    runtime_collector: JoinHandle<()>,
    lifecycle: BlockingRunLifecycle,
}

impl BlockingRunContext {
    pub(super) async fn start(
        session: &AgentSession,
        prompt: &str,
        persistence: Option<SessionPersistenceContext>,
    ) -> Result<Self> {
        let cancel_token = session.session_cancel.child_token();
        let (agent_loop, capability_run) =
            build_pinned_agent_loop(session, cancel_token.clone()).await?;
        Self::from_pinned_run(
            session,
            prompt,
            persistence,
            agent_loop,
            capability_run,
            cancel_token,
        )
        .await
    }

    pub(super) async fn from_pinned_run(
        session: &AgentSession,
        prompt: &str,
        persistence: Option<SessionPersistenceContext>,
        mut agent_loop: AgentLoop,
        capability_run: crate::capability::SessionCapabilityRun,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        let cognitive_binding = super::agent_loop_runtime::cognitive_binding_for_projection(
            session,
            capability_run.projection(),
        );
        let Some(capability_binding) = agent_loop.checkpoint_capability_binding().cloned() else {
            close_rejected_capability_run(&capability_run).await;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        };
        let run = match RunControlState::from_session(session)
            .start_run_with_bindings(prompt, cognitive_binding, capability_binding)
            .await
        {
            Ok(run) => run,
            Err(error) => {
                close_rejected_capability_run(&capability_run).await;
                return Err(error);
            }
        };
        let run_id = run.id().to_string();
        agent_loop.set_checkpoint_run(&run_id);
        let (checkpoint_sink, checkpoints) = runtime_checkpoint_channel(session);
        if let Some(checkpoint_sink) = checkpoint_sink {
            agent_loop = agent_loop.with_checkpoint_sink(checkpoint_sink);
        }
        let (runtime_tx, runtime_rx) = mpsc::channel(2048);
        let lifecycle = BlockingRunLifecycle::from_session(session, &run_id, persistence);
        lifecycle.set_cancel_token(cancel_token.clone()).await;
        let (agent_event_tx, agent_event_barrier, agent_events) = run_agent_event_channel(2048);
        let invocation = agent_loop
            .invocation_context(
                run_id.clone(),
                Some(&session.session_id),
                Some(runtime_tx),
                cancel_token,
            )
            .with_agent_events(agent_event_tx, agent_event_barrier);
        let runtime_collector = RuntimeEventSink::from_session(session, &run_id).spawn_collector(
            runtime_rx,
            Some(agent_events),
            Some(checkpoints),
        );

        Ok(Self {
            agent_loop,
            capability_run,
            invocation,
            runtime_collector,
            lifecycle,
        })
    }

    pub(super) async fn execute_with_prompt(
        self,
        messages: &[Message],
        prompt: &str,
        _session_id: &str,
    ) -> Result<AgentResult> {
        let Self {
            agent_loop,
            capability_run,
            invocation,
            runtime_collector,
            lifecycle,
        } = self;
        let result = settle_capability_run(
            agent_loop
                .execute_with_invocation(messages, prompt, &invocation)
                .await,
            capability_run,
        )
        .await;
        // Drop the run-owned event sender before waiting for the collector;
        // otherwise the receiver can never observe channel closure.
        drop(invocation);
        lifecycle.complete(runtime_collector, result).await
    }

    pub(super) async fn execute_from_messages(
        self,
        messages: Vec<Message>,
        _session_id: &str,
    ) -> Result<AgentResult> {
        self.execute_from_messages_seeded(messages, _session_id, None)
            .await
    }

    /// Execute from a prebuilt message list, seeding the loop's cumulative
    /// metrics from a checkpoint. Used by `resume_run` so resumed runs
    /// continue token/tool-call accounting from the checkpoint instead of
    /// re-starting at zero.
    pub(super) async fn execute_from_messages_seeded(
        self,
        messages: Vec<Message>,
        _session_id: &str,
        seed: Option<crate::agent::ExecutionSeed>,
    ) -> Result<AgentResult> {
        let Self {
            agent_loop,
            capability_run,
            invocation,
            runtime_collector,
            lifecycle,
        } = self;
        let result = settle_capability_run(
            agent_loop
                .execute_from_messages_with_invocation_seeded(messages, &invocation, seed)
                .await,
            capability_run,
        )
        .await;
        drop(invocation);
        lifecycle.complete(runtime_collector, result).await
    }
}

pub(super) struct StreamRunContext {
    agent_loop: AgentLoop,
    capability_run: crate::capability::SessionCapabilityRun,
    invocation: InvocationContext,
    worker_state: StreamRunWorkerState,
    forwarder: JoinHandle<()>,
    lifecycle: StreamRunLifecycle,
    rx: mpsc::Receiver<AgentEvent>,
}

impl StreamRunContext {
    pub(super) async fn start(
        session: &AgentSession,
        prompt: &str,
        persistence: Option<SessionPersistenceContext>,
    ) -> Result<Self> {
        let cancel_token = session.session_cancel.child_token();
        let (agent_loop, capability_run) =
            build_pinned_agent_loop(session, cancel_token.clone()).await?;
        let cognitive_binding = super::agent_loop_runtime::cognitive_binding_for_projection(
            session,
            capability_run.projection(),
        );
        let Some(capability_binding) = agent_loop.checkpoint_capability_binding().cloned() else {
            close_rejected_capability_run(&capability_run).await;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        };
        let run = match RunControlState::from_session(session)
            .start_run_with_bindings(prompt, cognitive_binding, capability_binding)
            .await
        {
            Ok(run) => run,
            Err(error) => {
                close_rejected_capability_run(&capability_run).await;
                return Err(error);
            }
        };
        Ok(Self::for_prepared_run(
            session,
            run.id().to_string(),
            persistence,
            agent_loop,
            capability_run,
            cancel_token,
        )
        .await)
    }

    pub(super) async fn for_run(
        session: &AgentSession,
        run_id: String,
        persistence: Option<SessionPersistenceContext>,
    ) -> Result<Self> {
        let cancel_token = session.session_cancel.child_token();
        let (agent_loop, capability_run) =
            build_pinned_agent_loop(session, cancel_token.clone()).await?;
        Self::from_pinned_run(
            session,
            run_id,
            persistence,
            agent_loop,
            capability_run,
            cancel_token,
        )
        .await
    }

    pub(super) async fn from_pinned_run(
        session: &AgentSession,
        run_id: String,
        persistence: Option<SessionPersistenceContext>,
        agent_loop: AgentLoop,
        capability_run: crate::capability::SessionCapabilityRun,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        let Some(capability_binding) = agent_loop.checkpoint_capability_binding().cloned() else {
            close_rejected_capability_run(&capability_run).await;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        };
        let run_control = RunControlState::from_session(session);
        if let Err(error) = run_control
            .bind_capability_generation(&run_id, capability_binding)
            .await
        {
            close_rejected_capability_run(&capability_run).await;
            return Err(error);
        }
        if let Some(binding) = super::agent_loop_runtime::cognitive_binding_for_projection(
            session,
            capability_run.projection(),
        ) {
            if let Err(error) = run_control.bind_cognitive_package(&run_id, binding).await {
                close_rejected_capability_run(&capability_run).await;
                return Err(error);
            }
        }
        Ok(Self::for_prepared_run(
            session,
            run_id,
            persistence,
            agent_loop,
            capability_run,
            cancel_token,
        )
        .await)
    }

    async fn for_prepared_run(
        session: &AgentSession,
        run_id: String,
        persistence: Option<SessionPersistenceContext>,
        mut agent_loop: AgentLoop,
        capability_run: crate::capability::SessionCapabilityRun,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let (runtime_tx, runtime_rx) = mpsc::channel(256);
        agent_loop.set_checkpoint_run(&run_id);
        let (checkpoint_sink, checkpoints) = runtime_checkpoint_channel(session);
        if let Some(checkpoint_sink) = checkpoint_sink {
            agent_loop = agent_loop.with_checkpoint_sink(checkpoint_sink);
        }
        let lifecycle = StreamRunLifecycle::from_session(session, &run_id, persistence);
        lifecycle.set_cancel_token(cancel_token.clone()).await;
        let (agent_event_tx, agent_event_barrier, agent_events) = run_agent_event_channel(2048);
        let invocation = agent_loop
            .invocation_context(
                run_id.clone(),
                Some(&session.session_id),
                Some(runtime_tx),
                cancel_token,
            )
            .with_agent_events(agent_event_tx, agent_event_barrier);
        let worker_state = lifecycle.worker_state();
        let forwarder = RuntimeEventSink::from_session(session, &run_id).spawn_forwarder(
            runtime_rx,
            tx,
            Some(agent_events),
            Some(checkpoints),
        );

        Self {
            agent_loop,
            capability_run,
            invocation,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        }
    }

    pub(super) fn spawn_with_prompt(
        self,
        messages: Vec<Message>,
        prompt: String,
    ) -> (
        mpsc::Receiver<AgentEvent>,
        JoinHandle<()>,
        Vec<tokio::task::AbortHandle>,
    ) {
        let Self {
            agent_loop,
            capability_run,
            invocation,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        } = self;
        let handle = tokio::spawn(async move {
            let result = settle_capability_run(
                agent_loop
                    .execute_with_invocation(&messages, &prompt, &invocation)
                    .await,
                capability_run,
            )
            .await;
            worker_state.complete(result).await;
        });
        let (lifecycle, worker_aborts) = lifecycle.wrap(handle, forwarder);
        (rx, lifecycle, worker_aborts)
    }

    pub(super) fn spawn_from_messages(
        self,
        messages: Vec<Message>,
    ) -> (
        mpsc::Receiver<AgentEvent>,
        JoinHandle<()>,
        Vec<tokio::task::AbortHandle>,
    ) {
        self.spawn_from_messages_seeded(messages, None)
    }

    pub(super) fn spawn_from_messages_seeded(
        self,
        messages: Vec<Message>,
        seed: Option<crate::agent::ExecutionSeed>,
    ) -> (
        mpsc::Receiver<AgentEvent>,
        JoinHandle<()>,
        Vec<tokio::task::AbortHandle>,
    ) {
        let Self {
            agent_loop,
            capability_run,
            invocation,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        } = self;
        let handle = tokio::spawn(async move {
            let result = settle_capability_run(
                agent_loop
                    .execute_from_messages_with_invocation_seeded(messages, &invocation, seed)
                    .await,
                capability_run,
            )
            .await;
            worker_state.complete(result).await;
        });
        let (lifecycle, worker_aborts) = lifecycle.wrap(handle, forwarder);
        (rx, lifecycle, worker_aborts)
    }
}

async fn close_rejected_capability_run(run: &crate::capability::SessionCapabilityRun) {
    if let Err(error) = run.close().await {
        tracing::warn!(error = %error, "Capability Run close failed after Run binding rejection");
    }
}

async fn settle_capability_run(
    execution: anyhow::Result<AgentResult>,
    capability_run: crate::capability::SessionCapabilityRun,
) -> Result<AgentResult> {
    let close = capability_run.close().await;
    match (execution, close) {
        (Ok(result), Ok(_)) => Ok(result),
        (Ok(_), Err(error)) => Err(CodeError::Capability(error)),
        (Err(error), Ok(_)) => Err(CodeError::Internal(error)),
        (Err(error), Err(close_error)) => {
            tracing::warn!(error = %close_error, "Capability Run close also failed after execution failure");
            Err(CodeError::Internal(error))
        }
    }
}
