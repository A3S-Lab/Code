use super::{
    agent_loop_runtime::build_agent_loop,
    run_lifecycle::{
        BlockingRunLifecycle, RunControlState, StreamRunLifecycle, StreamRunWorkerState,
    },
    runtime_events::{run_agent_event_channel, RuntimeEventSink},
    session_persistence::SessionPersistenceContext,
    AgentSession,
};
use crate::agent::{AgentEvent, AgentLoop, AgentResult, InvocationContext};
use crate::error::{read_or_recover, Result};
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
    invocation: InvocationContext,
    runtime_collector: JoinHandle<()>,
    lifecycle: BlockingRunLifecycle,
}

impl BlockingRunContext {
    pub(super) async fn start(
        session: &AgentSession,
        prompt: &str,
        persistence: Option<SessionPersistenceContext>,
    ) -> Self {
        let run = RunControlState::from_session(session)
            .start_run(prompt)
            .await;
        let run_id = run.id().to_string();
        let mut agent_loop = build_agent_loop(session);
        agent_loop.set_checkpoint_run(&run_id);
        let (runtime_tx, runtime_rx) = mpsc::channel(2048);
        let lifecycle = BlockingRunLifecycle::from_session(session, &run_id, persistence);
        let cancel_token = session.session_cancel.child_token();
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
        let runtime_collector = RuntimeEventSink::from_session(session, &run_id)
            .spawn_collector(runtime_rx, Some(agent_events));

        Self {
            agent_loop,
            invocation,
            runtime_collector,
            lifecycle,
        }
    }

    pub(super) async fn execute_with_prompt(
        self,
        messages: &[Message],
        prompt: &str,
        _session_id: &str,
    ) -> Result<AgentResult> {
        let Self {
            agent_loop,
            invocation,
            runtime_collector,
            lifecycle,
        } = self;
        let result = agent_loop
            .execute_with_invocation(messages, prompt, &invocation)
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
            invocation,
            runtime_collector,
            lifecycle,
        } = self;
        let result = agent_loop
            .execute_from_messages_with_invocation_seeded(messages, &invocation, seed)
            .await;
        drop(invocation);
        lifecycle.complete(runtime_collector, result).await
    }
}

pub(super) struct StreamRunContext {
    agent_loop: AgentLoop,
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
    ) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let (runtime_tx, runtime_rx) = mpsc::channel(256);
        let mut agent_loop = build_agent_loop(session);
        let run = RunControlState::from_session(session)
            .start_run(prompt)
            .await;
        let run_id = run.id().to_string();
        agent_loop.set_checkpoint_run(&run_id);
        let lifecycle = StreamRunLifecycle::from_session(session, &run_id, persistence);
        let cancel_token = session.session_cancel.child_token();
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
        );

        Self {
            agent_loop,
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
    ) -> (mpsc::Receiver<AgentEvent>, JoinHandle<()>) {
        let Self {
            agent_loop,
            invocation,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        } = self;
        let handle = tokio::spawn(async move {
            let result = agent_loop
                .execute_with_invocation(&messages, &prompt, &invocation)
                .await;
            worker_state.complete(result).await;
        });
        (rx, lifecycle.wrap(handle, forwarder))
    }

    pub(super) fn spawn_from_messages(
        self,
        messages: Vec<Message>,
    ) -> (mpsc::Receiver<AgentEvent>, JoinHandle<()>) {
        let Self {
            agent_loop,
            invocation,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        } = self;
        let handle = tokio::spawn(async move {
            let result = agent_loop
                .execute_from_messages_with_invocation_seeded(messages, &invocation, None)
                .await;
            worker_state.complete(result).await;
        });
        (rx, lifecycle.wrap(handle, forwarder))
    }
}
