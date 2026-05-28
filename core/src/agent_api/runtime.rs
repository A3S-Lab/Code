use super::{
    agent_loop_runtime::build_agent_loop,
    run_lifecycle::{
        BlockingRunLifecycle, RunControlState, StreamRunLifecycle, StreamRunWorkerState,
    },
    runtime_events::RuntimeEventSink,
    session_persistence::SessionPersistenceContext,
    AgentSession,
};
use crate::agent::{AgentEvent, AgentLoop, AgentResult};
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
    runtime_tx: mpsc::Sender<AgentEvent>,
    cancel_token: tokio_util::sync::CancellationToken,
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
        let agent_loop = build_agent_loop(session);
        let (runtime_tx, runtime_rx) = mpsc::channel(2048);
        let runtime_collector =
            RuntimeEventSink::from_session(session, &run_id).spawn_collector(runtime_rx);
        let lifecycle = BlockingRunLifecycle::from_session(session, &run_id, persistence);
        let cancel_token = session.session_cancel.child_token();
        lifecycle.set_cancel_token(cancel_token.clone()).await;

        Self {
            agent_loop,
            runtime_tx,
            cancel_token,
            runtime_collector,
            lifecycle,
        }
    }

    pub(super) async fn execute_with_prompt(
        self,
        messages: &[Message],
        prompt: &str,
        session_id: &str,
    ) -> Result<AgentResult> {
        let Self {
            agent_loop,
            runtime_tx,
            cancel_token,
            runtime_collector,
            lifecycle,
        } = self;
        let result = agent_loop
            .execute_with_session(
                messages,
                prompt,
                Some(session_id),
                Some(runtime_tx),
                Some(&cancel_token),
            )
            .await;
        lifecycle.complete(runtime_collector, result).await
    }

    pub(super) async fn execute_from_messages(
        self,
        messages: Vec<Message>,
        session_id: &str,
    ) -> Result<AgentResult> {
        let Self {
            agent_loop,
            runtime_tx,
            cancel_token,
            runtime_collector,
            lifecycle,
        } = self;
        let result = agent_loop
            .execute_from_messages(
                messages,
                Some(session_id),
                Some(runtime_tx),
                Some(&cancel_token),
            )
            .await;
        lifecycle.complete(runtime_collector, result).await
    }
}

pub(super) struct StreamRunContext {
    agent_loop: AgentLoop,
    runtime_tx: mpsc::Sender<AgentEvent>,
    session_id: String,
    cancel_token: tokio_util::sync::CancellationToken,
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
        let agent_loop = build_agent_loop(session);
        let run = RunControlState::from_session(session)
            .start_run(prompt)
            .await;
        let run_id = run.id().to_string();
        let lifecycle = StreamRunLifecycle::from_session(session, &run_id, persistence);
        let cancel_token = session.session_cancel.child_token();
        lifecycle.set_cancel_token(cancel_token.clone()).await;
        let worker_state = lifecycle.worker_state();
        let forwarder =
            RuntimeEventSink::from_session(session, &run_id).spawn_forwarder(runtime_rx, tx);

        Self {
            agent_loop,
            runtime_tx,
            session_id: session.session_id.clone(),
            cancel_token,
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
            runtime_tx,
            session_id,
            cancel_token,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        } = self;
        let handle = tokio::spawn(async move {
            let result = agent_loop
                .execute_with_session(
                    &messages,
                    &prompt,
                    Some(&session_id),
                    Some(runtime_tx),
                    Some(&cancel_token),
                )
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
            runtime_tx,
            session_id,
            cancel_token,
            worker_state,
            forwarder,
            lifecycle,
            rx,
        } = self;
        let handle = tokio::spawn(async move {
            let result = agent_loop
                .execute_from_messages(
                    messages,
                    Some(&session_id),
                    Some(runtime_tx),
                    Some(&cancel_token),
                )
                .await;
            worker_state.complete(result).await;
        });
        (rx, lifecycle.wrap(handle, forwarder))
    }
}
