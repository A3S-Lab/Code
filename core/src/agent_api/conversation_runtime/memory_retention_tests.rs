#![cfg(test)]

use super::*;
use a3s_memory::{FileMemoryStore, InMemoryStore, MemoryItem, MemoryStore};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::Semaphore;

const REMEMBER_PROMPT: &str = "Please remember this durable workspace preference for future sessions: always use the verification codename REMEMBER-TOKEN-4411.";
const DISTILLED: &str = "Use verification codename REMEMBER-TOKEN-4411 for this workspace.";

fn valid_model_output() -> String {
    serde_json::json!({"items":[{
        "memory_type":"semantic", "content":DISTILLED,
        "importance":0.95, "confidence":0.98, "tags":["verification"],
        "source":"preference", "scope":"workspace",
        "reason":"The user supplied a durable workspace verification preference."
    }]})
    .to_string()
}

#[derive(Clone, Copy, Debug)]
enum StoreKind {
    Memory,
    File,
}

const STORES: [StoreKind; 2] = [StoreKind::Memory, StoreKind::File];

// Count actual writer entries while delegating canonical IDs/deduplication and
// every read to the original adapter. No test-side write or success emulation.
struct AuditedStore {
    inner: Arc<dyn MemoryStore>,
    entered: AtomicUsize,
}

#[async_trait]
impl MemoryStore for AuditedStore {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.inner.store(item).await
    }
    async fn store_and_return(&self, item: MemoryItem) -> anyhow::Result<MemoryItem> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.inner.store_and_return(item).await
    }
    async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
        self.inner.retrieve(id).await
    }
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        self.inner.search(query, limit).await
    }
    async fn search_by_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>> {
        self.inner.search_by_tags(tags, limit).await
    }
    async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        self.inner.get_recent(limit).await
    }
    async fn get_important(&self, threshold: f32, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        self.inner.get_important(threshold, limit).await
    }
    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.inner.delete(id).await
    }
    async fn clear(&self) -> anyhow::Result<()> {
        self.inner.clear().await
    }
    async fn count(&self) -> anyhow::Result<usize> {
        self.inner.count().await
    }
    async fn prune(&self, policy: &a3s_memory::PrunePolicy) -> anyhow::Result<usize> {
        self.inner.prune(policy).await
    }
}

struct StoreFixture {
    store: Arc<AuditedStore>,
    directory: tempfile::TempDir,
    kind: StoreKind,
}

impl StoreFixture {
    async fn new(kind: StoreKind) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let inner: Arc<dyn MemoryStore> = match kind {
            StoreKind::Memory => Arc::new(InMemoryStore::new()),
            StoreKind::File => Arc::new(FileMemoryStore::new(directory.path()).await.unwrap()),
        };
        Self {
            store: Arc::new(AuditedStore {
                inner,
                entered: AtomicUsize::new(0),
            }),
            directory,
            kind,
        }
    }

    async fn read_back(&self) -> Vec<MemoryItem> {
        let reader: Arc<dyn MemoryStore> = match self.kind {
            StoreKind::Memory => self.store.inner.clone(),
            // Reopen the actual file adapter, including its persisted index.
            StoreKind::File => Arc::new(FileMemoryStore::new(self.directory.path()).await.unwrap()),
        };
        let items = reader.get_recent(20).await.unwrap();
        assert_eq!(reader.count().await.unwrap(), items.len());
        for item in &items {
            let canonical = reader.retrieve(&item.id).await.unwrap().unwrap();
            assert_eq!(canonical.content, item.content);
            assert_eq!(canonical.metadata, item.metadata);
        }
        items
    }
}

struct ControlledClient {
    output: String,
    extraction_prompts: Mutex<Vec<String>>,
    main_calls: AtomicUsize,
    entered: Semaphore,
    release: Semaphore,
    blocked: bool,
    concurrency: crate::llm::ModelGenerationConcurrency,
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl ControlledClient {
    fn new(output: String, blocked: bool) -> Arc<Self> {
        Self::with_concurrency(
            output,
            blocked,
            crate::llm::ModelGenerationConcurrency::single_flight(),
        )
    }

    fn with_concurrency(
        output: String,
        blocked: bool,
        concurrency: crate::llm::ModelGenerationConcurrency,
    ) -> Arc<Self> {
        Arc::new(Self {
            output,
            extraction_prompts: Mutex::new(Vec::new()),
            main_calls: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            blocked,
            concurrency,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }

    fn enter_provider(&self) -> ProviderCall<'_> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        assert!(active <= self.concurrency.max_concurrency().get());
        ProviderCall(self)
    }

    fn extraction_calls(&self) -> usize {
        self.extraction_prompts.lock().unwrap().len()
    }

    async fn wait_for_extraction(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

struct ProviderCall<'a>(&'a ControlledClient);

impl Drop for ProviderCall<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn response(text: &str) -> crate::llm::LlmResponse {
    crate::llm::LlmResponse {
        message: Message::assistant(text),
        usage: Default::default(),
        stop_reason: None,
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl crate::llm::LlmClient for ControlledClient {
    fn model_generation_concurrency(&self) -> crate::llm::ModelGenerationConcurrency {
        self.concurrency
    }

    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        let _actual_call = self.enter_provider();
        // Planning is disabled. Tool availability identifies the existing main
        // vs extractor call in this controlled fixture, never user meaning.
        if tools.is_empty() {
            self.extraction_prompts
                .lock()
                .unwrap()
                .push(messages[0].text());
            self.entered.add_permits(1);
            if self.blocked {
                self.release.acquire().await.unwrap().forget();
            }
            Ok(response(&self.output))
        } else {
            self.main_calls.fetch_add(1, Ordering::SeqCst);
            Ok(response("The workspace answer is complete."))
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        let _actual_call = self.enter_provider();
        self.main_calls.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel(2);
        sender
            .try_send(crate::llm::StreamEvent::TextDelta(
                "The workspace answer is complete.".into(),
            ))
            .unwrap();
        sender
            .try_send(crate::llm::StreamEvent::Done(response(
                "The workspace answer is complete.",
            )))
            .unwrap();
        Ok(receiver)
    }
}

#[derive(Debug)]
struct EndBarrier {
    armed: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}

impl EndBarrier {
    fn new(armed: bool) -> Arc<Self> {
        Arc::new(Self {
            armed: AtomicBool::new(armed),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}

#[async_trait]
impl crate::hooks::HookExecutor for EndBarrier {
    async fn fire(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::continue_()
    }
    async fn record_agent_event(&self, event: &AgentEvent, _run_id: &str, _session_id: &str) {
        if self.armed.load(Ordering::SeqCst) && matches!(event, AgentEvent::End { .. }) {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
    }
}

async fn session(
    workspace: &std::path::Path,
    store: &StoreFixture,
    client: Arc<ControlledClient>,
    hook: Option<Arc<EndBarrier>>,
    session_store: Option<Arc<crate::store::MemorySessionStore>>,
) -> AgentSession {
    let agent = crate::Agent::from_config(crate::agent_api::tests::test_config())
        .await
        .unwrap();
    let mut options = crate::SessionOptions::new()
        .with_session_id("retention-authority")
        .with_planning_mode(crate::prompts::PlanningMode::Disabled)
        .with_memory(store.store.clone());
    if let Some(hook) = hook {
        options = options.with_hook_executor(hook);
    }
    if let Some(session_store) = session_store {
        options = options.with_session_store(session_store);
    }
    agent
        .build_session(workspace.to_string_lossy().into_owned(), client, &options)
        .unwrap()
}

async fn assert_pending_ticket(session: &AgentSession) {
    let drain = session
        .memory()
        .unwrap()
        .drain_llm_extractions(Duration::from_secs(5));
    tokio::pin!(drain);
    assert!(matches!(futures::poll!(&mut drain), Poll::Pending));
}

async fn assert_no_ticket(session: &AgentSession) {
    let drain = session
        .memory()
        .unwrap()
        .drain_llm_extractions(Duration::from_secs(5));
    tokio::pin!(drain);
    assert!(matches!(futures::poll!(&mut drain), Poll::Ready(true)));
}

async fn receive_end(events: &mut mpsc::Receiver<AgentEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await.unwrap() {
                AgentEvent::End { .. } => break,
                AgentEvent::Error { message } => panic!("unexpected run failure: {message}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
}

fn checkpoint(session: &AgentSession) -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: "source-run".into(),
        session_id: session.session_id.clone(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        messages: vec![Message::user(REMEMBER_PROMPT)],
        total_usage: Default::default(),
        tool_calls_count: 0,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    }
}

#[tokio::test]
async fn same_remember_bytes_require_valid_model_authority_in_both_stores() {
    for kind in STORES {
        for (output, expected) in [
            (valid_model_output(), 1), (r#"{"items":[]}"#.into(), 0),
            ("I will keep that in mind.".into(), 0), ("{broken".into(), 0),
            (r#"{"items":[{"memory_type":"semantic","content":"Use verification codename REMEMBER-TOKEN-4411 for this workspace.","importance":0.95,"confidence":0.98,"source":"unknown","scope":"workspace","reason":"The user supplied a durable workspace verification preference."}]}"#.into(), 0),
        ] {
            let store = StoreFixture::new(kind).await;
            let workspace = tempfile::tempdir().unwrap();
            let client = ControlledClient::new(output, false);
            let session = session(workspace.path(), &store, client.clone(), None, None).await;
            session.send(REMEMBER_PROMPT, None).await.unwrap();
            assert_eq!(client.extraction_calls(), 1, "{kind:?}");
            assert!(client.extraction_prompts.lock().unwrap()[0].contains(REMEMBER_PROMPT));
            assert_eq!(store.store.entered.load(Ordering::SeqCst), expected, "{kind:?}");
            let items = store.read_back().await;
            assert_eq!(items.len(), expected, "{kind:?}");
            if let Some(item) = items.first() {
                assert_eq!(item.content, DISTILLED);
                assert_eq!(item.metadata.get("schema").map(String::as_str), Some("a3s.memory.durable.v1"));
                assert_eq!(item.metadata.get("source").map(String::as_str), Some("preference"));
            }
            assert_no_ticket(&session).await;
            session.close().await;
        }
    }
}

#[tokio::test]
async fn former_matcher_inputs_cannot_grant_writes_in_both_stores() {
    // Exact inputs migrated from the removed meaning-matcher unit test.
    let prompts = [
        "remember this preference: my codename is Q4-Falcon.",
        "My codename is Q4-Falcon. remember this preference.",
        "please remember to close the door tonight",
        "请记住:我的代号是 Y9-Kestrel。这是长期个人偏好,以后回答时引用。",
        "記住這個偏好",
        "覚えておいて:私の好きなエディタは vim です",
        "What is my codename?",
        "The warehouse has seven vats and a cracked hourglass.",
    ];
    for kind in STORES {
        for prompt in prompts {
            let store = StoreFixture::new(kind).await;
            let workspace = tempfile::tempdir().unwrap();
            let client = ControlledClient::new("I will keep that in mind.".into(), false);
            let session = session(workspace.path(), &store, client.clone(), None, None).await;
            session.send(prompt, None).await.unwrap();
            assert_eq!(client.extraction_calls(), 1);
            assert!(client.extraction_prompts.lock().unwrap()[0].contains(prompt));
            assert_eq!(
                store.store.entered.load(Ordering::SeqCst),
                0,
                "{kind:?}: {prompt}"
            );
            assert!(store.read_back().await.is_empty());
            session.close().await;
        }
    }
}

#[tokio::test]
async fn explicit_host_typed_writer_remains_a_separate_positive_in_both_stores() {
    for kind in STORES {
        let store = StoreFixture::new(kind).await;
        let workspace = tempfile::tempdir().unwrap();
        let client = ControlledClient::new(r#"{"items":[]}"#.into(), false);
        let session = session(workspace.path(), &store, client, None, None).await;
        session.send(REMEMBER_PROMPT, None).await.unwrap();
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 0);
        let canonical = session
            .memory()
            .unwrap()
            .remember_item(
                MemoryItem::new("Host supplied durable workspace preference.")
                    .with_metadata("host_fixture", "typed-write"),
            )
            .await
            .unwrap();
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 1);
        let items = store.read_back().await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, canonical.id);
        assert_eq!(items[0].content, canonical.content);
        session.close().await;
    }
}

#[tokio::test]
async fn stream_registers_before_end_preserves_fifo_and_immediate_close_in_both_stores() {
    for kind in STORES {
        let store = StoreFixture::new(kind).await;
        let workspace = tempfile::tempdir().unwrap();
        // The blocked first extraction overlaps the second main provider
        // future. Advertise this fixture's real capacity; the product's
        // conservative single-flight default remains unchanged.
        let client = ControlledClient::with_concurrency(
            valid_model_output(),
            true,
            crate::llm::ModelGenerationConcurrency::bounded(
                std::num::NonZeroUsize::new(2).unwrap(),
            ),
        );
        let hook = EndBarrier::new(true);
        let session_store = Arc::new(crate::store::MemorySessionStore::new());
        let session = session(
            workspace.path(),
            &store,
            client.clone(),
            Some(hook.clone()),
            Some(session_store),
        )
        .await;
        let first_prompt = format!("{REMEMBER_PROMPT} First completed turn.");
        let mut first = session.stream_with_run(&first_prompt, None).await.unwrap();
        hook.wait().await;
        assert_pending_ticket(&session).await;
        assert_eq!(client.extraction_calls(), 0);
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 0);
        assert!(session
            .run_events(first.run.as_ref().unwrap().id())
            .await
            .iter()
            .any(|record| matches!(record.event, AgentEvent::End { .. })));
        hook.release.add_permits(1);
        receive_end(&mut first.events).await;
        first.worker.await.unwrap();
        client.wait_for_extraction().await;

        let second_prompt = format!("{REMEMBER_PROMPT} Second completed turn.");
        let mut second = session.stream_with_run(&second_prompt, None).await.unwrap();
        hook.wait().await;
        assert_eq!(client.peak.load(Ordering::SeqCst), 2);
        assert_pending_ticket(&session).await;
        assert_eq!(
            client.extraction_calls(),
            1,
            "FIFO successor cannot enter provider yet"
        );
        hook.release.add_permits(1);
        receive_end(&mut second.events).await;
        second.worker.await.unwrap();

        let closing = session.close();
        tokio::pin!(closing);
        assert!(matches!(futures::poll!(&mut closing), Poll::Pending));
        assert!(session.close_handle.is_closed());
        assert!(!session.session_cancel.is_cancelled());
        client.release.add_permits(1);
        client.wait_for_extraction().await;
        let prompts = client.extraction_prompts.lock().unwrap().clone();
        assert!(prompts[0].contains(&first_prompt));
        assert!(prompts[1].contains(&second_prompt));
        assert!(matches!(futures::poll!(&mut closing), Poll::Pending));
        client.release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), &mut closing)
            .await
            .unwrap();
        assert_eq!(
            store.store.entered.load(Ordering::SeqCst),
            2,
            "both valid turns enter the original canonical writer"
        );
        let canonical = store.read_back().await;
        assert_eq!(
            canonical.len(),
            1,
            "original duplicate merge retains one item"
        );
        assert_eq!(canonical[0].content, DISTILLED);
        assert_no_ticket(&session).await;
        // These counters observe actual provider futures, not a claim about
        // the duration of the original generation permit after returning rx.
        assert_eq!(client.active.load(Ordering::SeqCst), 0);
        assert_eq!(client.peak.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn detached_and_exact_recovery_register_before_publication_in_both_stores() {
    use crate::store::SessionStore;
    for kind in STORES {
        for recovery in [false, true] {
            let store = StoreFixture::new(kind).await;
            let workspace = tempfile::tempdir().unwrap();
            let client = ControlledClient::new(valid_model_output(), true);
            let hook = EndBarrier::new(true);
            let session_store = Arc::new(crate::store::MemorySessionStore::new());
            let session = session(
                workspace.path(),
                &store,
                client.clone(),
                Some(hook.clone()),
                Some(session_store.clone()),
            )
            .await;
            let spawned = if recovery {
                session_store
                    .save_loop_checkpoint("source-run", &checkpoint(&session))
                    .await
                    .unwrap();
                session
                    .spawn_recovery_with_run_id("source-run", "recovered-run")
                    .await
                    .unwrap()
            } else {
                session
                    .spawn_run_with_id("detached-run", REMEMBER_PROMPT)
                    .await
                    .unwrap()
            };
            let AgentRunSpawn::Started {
                snapshot,
                mut worker,
            } = spawned
            else {
                panic!("fresh run must start")
            };
            hook.wait().await;
            assert_pending_ticket(&session).await;
            assert_eq!(client.extraction_calls(), 0);
            assert_eq!(store.store.entered.load(Ordering::SeqCst), 0);
            assert!(session
                .run_events(&snapshot.id)
                .await
                .iter()
                .any(|record| matches!(record.event, AgentEvent::End { .. })));
            hook.release.add_permits(1);
            client.wait_for_extraction().await;
            assert!(
                matches!(futures::poll!(&mut worker), Poll::Pending),
                "same worker awaits extraction"
            );
            let closing = session.close();
            tokio::pin!(closing);
            assert!(matches!(futures::poll!(&mut closing), Poll::Pending));
            assert!(!session.session_cancel.is_cancelled());
            client.release.add_permits(1);
            tokio::time::timeout(Duration::from_secs(5), &mut closing)
                .await
                .unwrap();
            worker.await.unwrap();
            assert_eq!(store.store.entered.load(Ordering::SeqCst), 1);
            assert_eq!(store.read_back().await[0].content, DISTILLED);
            assert_no_ticket(&session).await;
        }
    }
}

#[tokio::test]
async fn same_worker_joins_original_committer_before_end_and_extraction_in_both_stores() {
    for kind in STORES {
        let store = StoreFixture::new(kind).await;
        let workspace = tempfile::tempdir().unwrap();
        let client = ControlledClient::new(valid_model_output(), true);
        let session_store = Arc::new(crate::store::MemorySessionStore::new());
        let session = session(
            workspace.path(),
            &store,
            client.clone(),
            None,
            Some(session_store),
        )
        .await;
        let lease = ExecutionCoordinator::admit(&session, "same original committer memory fixture")
            .await
            .unwrap();
        let mut captured = OwnedConversationCapture::capture(&session, false).unwrap();
        let cancellation = captured.preparation.cancellation.clone();
        let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
            &session,
            lease,
            run_admission::ExecutionMode::Detached,
            cancellation,
        )
        .unwrap();
        captured.controls = captured.controls.bind_original(original.clone()).unwrap();
        let (sender_ready, sender_rx) = tokio::sync::oneshot::channel();
        let (tx, mut events) = mpsc::channel(8);
        let worker = tokio::spawn(async move {
            assert!(start.wait().await.unwrap());
            original
                .record_scope_not_attempted(run_admission::OriginalScopeKind::Command)
                .unwrap();
            let pinned = prepare_owned_fact(
                captured,
                original.clone(),
                REMEMBER_PROMPT,
                None,
                Some(tx.clone()),
            )
            .await
            .unwrap();
            // Hold the actual checkpoint sender. The original cell owns the
            // unique real committer; no proxy or replacement writer is used.
            let sender = pinned
                .parts
                .surface
                .as_ref()
                .unwrap()
                .checkpoint
                .as_ref()
                .unwrap()
                .sink
                .clone();
            sender_ready.send(sender).ok().unwrap();
            let exact = Some(pinned.run.id().to_owned());
            settle_owned_fact(
                pinned,
                OwnedFactExecution {
                    prompt: REMEMBER_PROMPT.to_owned(),
                    attachments: Vec::new(),
                    history: None,
                    checkpoint: None,
                    ordinary: false,
                    exact,
                    mode: run_admission::ExecutionMode::Detached,
                },
                tx,
            )
            .await
            .unwrap();
        });
        let (disposition, _observed) = tokio::sync::oneshot::channel();
        let mut worker = handoff.supervise(worker, Vec::new(), disposition);
        let sender = sender_rx.await.unwrap();
        // The final checkpoint writer must finish before terminal publication
        // and extraction admission. The old fixture waited for End while
        // retaining this exact sender, creating an impossible circular wait.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await.unwrap() {
                    AgentEvent::TurnEnd { .. } => break,
                    AgentEvent::End { .. } => panic!("terminal before original committer EOF"),
                    AgentEvent::Error { message } => panic!("unexpected run failure: {message}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        let snapshot = session.runs().await.pop().unwrap();
        assert!(!snapshot.status.is_terminal());
        assert_no_ticket(&session).await;
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 0);
        assert!(matches!(futures::poll!(&mut worker), Poll::Pending));
        drop(sender);
        receive_end(&mut events).await;
        assert!(
            events.recv().await.is_none(),
            "host channel closes before extraction completes"
        );
        client.wait_for_extraction().await;
        assert_pending_ticket(&session).await;
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 0);
        assert!(matches!(futures::poll!(&mut worker), Poll::Pending));
        client.release.add_permits(1);
        assert!(
            session
                .memory()
                .unwrap()
                .drain_llm_extractions(Duration::from_secs(5))
                .await
        );
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 1);
        assert_eq!(store.read_back().await[0].content, DISTILLED);
        tokio::time::timeout(Duration::from_secs(5), &mut worker)
            .await
            .unwrap()
            .unwrap();
        session.close().await;
    }
}

#[tokio::test]
async fn ordinary_log_and_checkpoint_resumes_admit_no_extraction_in_both_stores() {
    use crate::store::SessionStore;
    for kind in STORES {
        let store = StoreFixture::new(kind).await;
        let workspace = tempfile::tempdir().unwrap();
        let client = ControlledClient::new(r#"{"items":[]}"#.into(), false);
        let hook = EndBarrier::new(false);
        let session_store = Arc::new(crate::store::MemorySessionStore::new());
        let session = session(
            workspace.path(),
            &store,
            client.clone(),
            Some(hook.clone()),
            Some(session_store.clone()),
        )
        .await;
        session.send(REMEMBER_PROMPT, None).await.unwrap();
        assert_eq!(client.extraction_calls(), 1);
        let main_before = client.main_calls.load(Ordering::SeqCst);
        session.resume_run("absent-checkpoint").await.unwrap();
        assert_eq!(client.main_calls.load(Ordering::SeqCst), main_before);
        assert_eq!(client.extraction_calls(), 1);
        assert_no_ticket(&session).await;

        session_store
            .save_loop_checkpoint("source-run", &checkpoint(&session))
            .await
            .unwrap();
        hook.armed.store(true, Ordering::SeqCst);
        let resumed = session.resume_run("source-run");
        tokio::pin!(resumed);
        tokio::select! {
            _ = hook.wait() => {}
            result = &mut resumed => panic!("resume returned before publication barrier: {result:?}"),
        }
        assert_eq!(client.extraction_calls(), 1);
        assert_no_ticket(&session).await;
        assert_eq!(store.store.entered.load(Ordering::SeqCst), 0);
        hook.release.add_permits(1);
        resumed.await.unwrap();
        assert_eq!(client.extraction_calls(), 1);
        assert_no_ticket(&session).await;
        assert!(store.read_back().await.is_empty());
        session.close().await;
    }
}

mod lifecycle;
