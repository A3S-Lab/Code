#![cfg(test)]

use super::*;
use crate::agent_api::{Agent, AgentRunStream, AgentSession, SessionOptions};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) struct HeldProvider {
    entered: tokio::sync::Semaphore,
    streams: Mutex<Vec<(tokio::sync::mpsc::Sender<StreamEvent>, CancellationToken)>>,
}

impl Default for HeldProvider {
    fn default() -> Self {
        Self {
            entered: tokio::sync::Semaphore::new(0),
            streams: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for HeldProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("public cancellation fixture must use its actual streaming call")
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        actual: CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        self.streams.lock().await.push((sender, actual));
        self.entered.add_permits(1);
        Ok(receiver)
    }
}

pub(super) struct PublicFixture {
    _workspace: tempfile::TempDir,
    _agent: Agent,
    pub(super) session: AgentSession,
    pub(super) provider: Arc<HeldProvider>,
}

pub(super) async fn public_fixture() -> PublicFixture {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = crate::agent_api::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let provider = Arc::new(HeldProvider::default());
    let options = SessionOptions::new()
        .with_session_id("public-original-cancel")
        .with_retention_limits(crate::retention::SessionRetentionLimits {
            max_runs_retained: Some(1),
            ..Default::default()
        })
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()));
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options.with_llm_client(provider.clone())),
        )
        .await
        .unwrap();
    PublicFixture {
        _workspace: workspace,
        _agent: agent,
        session,
        provider,
    }
}

pub(super) async fn start(fixture: &PublicFixture) -> AgentRunStream {
    let stream = fixture
        .session
        .stream_with_run("Return the fixture answer", None)
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.provider.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    assert_eq!(
        fixture.session.current_run().await.unwrap().id(),
        stream.run.as_ref().unwrap().id()
    );
    stream
}

pub(super) async fn finish(fixture: &PublicFixture, mut stream: AgentRunStream) {
    let sender = fixture
        .provider
        .streams
        .lock()
        .await
        .last()
        .unwrap()
        .0
        .clone();
    sender
        .send(StreamEvent::Done(LlmResponse {
            message: Message::assistant("actual provider result"),
            usage: Default::default(),
            stop_reason: Some("end_turn".into()),
            token_logprobs: Vec::new(),
            meta: None,
        }))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while stream.events.recv().await.is_some() {}
        stream.worker.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn retained_public_handle_requests_only_its_original_and_cannot_cancel_next_run() {
    let fixture = public_fixture().await;
    let first = start(&fixture).await;
    let retained = first.run.as_ref().unwrap().clone();
    assert!(retained.cancel().await);
    assert!(fixture.provider.streams.lock().await[0].1.is_cancelled());
    finish(&fixture, first).await;
    assert_eq!(retained.status().await, Some(RunStatus::Cancelled));
    let second = start(&fixture).await;
    let next = second.run.as_ref().unwrap().clone();
    assert_ne!(retained.id(), next.id());
    assert!(!retained.cancel().await);
    assert!(!fixture.provider.streams.lock().await[1].1.is_cancelled());
    assert_eq!(fixture.session.current_run().await.unwrap().id(), next.id());
    finish(&fixture, second).await;
    assert_eq!(next.status().await, Some(RunStatus::Completed));
}

async fn exact_start(
    fixture: &PublicFixture,
    id: &str,
) -> (RunHandle, tokio::task::JoinHandle<()>) {
    let started = fixture
        .session
        .spawn_run_with_id(id, "Return the exact fixture answer")
        .await
        .unwrap();
    let worker = match started {
        crate::agent_api::AgentRunSpawn::Started { snapshot, worker } => {
            assert_eq!(snapshot.id, id);
            worker
        }
        crate::agent_api::AgentRunSpawn::Replayed { .. } => {
            panic!("actual evicted ID must be freshly Created")
        }
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.provider.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let actual = fixture.session.current_run().await.unwrap();
    assert_eq!(actual.id(), id);
    (actual, worker)
}

async fn finish_exact(fixture: &PublicFixture, worker: tokio::task::JoinHandle<()>) {
    let sender = fixture
        .provider
        .streams
        .lock()
        .await
        .last()
        .unwrap()
        .0
        .clone();
    sender
        .send(StreamEvent::Done(LlmResponse {
            message: Message::assistant("actual exact result"),
            usage: Default::default(),
            stop_reason: Some("end_turn".into()),
            token_logprobs: Vec::new(),
            meta: None,
        }))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn actual_fifo_eviction_and_same_id_recreation_cannot_retarget_retained_cancel_pin() {
    let fixture = public_fixture().await; // actual Session RunStore max_runs_retained=1
    let (retained, first) = exact_start(&fixture, "reused-original-id").await;
    finish_exact(&fixture, first).await;
    assert_eq!(retained.status().await, Some(RunStatus::Completed));
    let (_, evictor) = exact_start(&fixture, "actual-fifo-evictor").await;
    assert!(fixture
        .session
        .run_snapshot("reused-original-id")
        .await
        .is_none());
    finish_exact(&fixture, evictor).await;
    let (replacement, worker) = exact_start(&fixture, "reused-original-id").await;
    assert_eq!(replacement.id(), retained.id()); // exact same label, distinct actual owner
    assert!(fixture
        .session
        .run_snapshot("actual-fifo-evictor")
        .await
        .is_none());
    let snapshot = replacement.snapshot().await.unwrap();
    let controls = fixture.session.run_control_snapshot().await.unwrap();
    let token = fixture.provider.streams.lock().await[2].1.clone();
    assert!(!token.is_cancelled());
    assert!(!retained.cancel().await);
    assert!(!token.is_cancelled());
    let after = replacement.snapshot().await.unwrap();
    assert_eq!(
        (
            after.status,
            after.event_count,
            after.updated_at_ms,
            after.result_text,
            after.error
        ),
        (
            snapshot.status,
            snapshot.event_count,
            snapshot.updated_at_ms,
            snapshot.result_text,
            snapshot.error
        )
    );
    assert_eq!(
        fixture.session.run_control_snapshot().await.unwrap(),
        controls
    );
    assert_eq!(
        fixture.session.current_run().await.unwrap().id(),
        replacement.id()
    );
    assert!(replacement
        .events()
        .await
        .iter()
        .all(|event| !matches!(event.event, AgentEvent::Error { .. })));
    finish_exact(&fixture, worker).await;
    assert_eq!(replacement.status().await, Some(RunStatus::Completed));
}
