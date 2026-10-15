#![cfg(test)]

use super::super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use std::collections::BTreeMap;
use tokio::task::JoinHandle;
use tracing_subscriber::prelude::*;

pub(super) const MAIN_TEXT: &str = "The workspace answer is complete.";
pub(super) const SESSION_ID: &str = "retention-lifecycle";

type UsageRecord = (usize, usize, usize, Option<usize>, Option<usize>);
type GuardRecord = (String, UsageRecord);

pub(super) fn main_usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 17,
        completion_tokens: 13,
        total_tokens: 30,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}
pub(super) fn extraction_usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 11,
        completion_tokens: 7,
        total_tokens: 18,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}
pub(super) fn usage_tuple(usage: &TokenUsage) -> (usize, usize, usize) {
    (
        usage.prompt_tokens,
        usage.completion_tokens,
        usage.total_tokens,
    )
}

pub(super) fn record_tuple(usage: &TokenUsage) -> UsageRecord {
    (
        usage.prompt_tokens,
        usage.completion_tokens,
        usage.total_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
    )
}

// These are observations of the supplied original host callback, never a
// charge performed by the fixture or an alternative billing authority.
#[derive(Default)]
pub(super) struct Guard {
    pub checks: Mutex<Vec<(String, bool, usize)>>,
    pub records: Mutex<Vec<(String, TokenUsage)>>,
    pub main_returned: AtomicBool,
    pub deny_extractor: AtomicBool,
}
#[async_trait]
impl BudgetGuard for Guard {
    async fn check_before_llm(&self, session: &str, estimated: usize) -> BudgetDecision {
        let after_main = self.main_returned.load(Ordering::SeqCst);
        self.checks
            .lock()
            .unwrap()
            .push((session.to_owned(), after_main, estimated));
        if after_main && self.deny_extractor.load(Ordering::SeqCst) {
            BudgetDecision::Deny {
                resource: "fixture_tokens".into(),
                reason: "typed fixture denial".into(),
            }
        } else {
            BudgetDecision::Allow
        }
    }
    async fn record_after_llm(&self, session: &str, usage: &TokenUsage) {
        self.records
            .lock()
            .unwrap()
            .push((session.to_owned(), usage.clone()));
    }
}
impl Guard {
    pub fn records(&self) -> Vec<GuardRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|(session, usage)| (session.clone(), record_tuple(usage)))
            .collect()
    }
    pub fn assert_checks(&self, main: usize, extraction: usize) {
        let checks = self.checks.lock().unwrap();
        assert!(
            checks.iter().all(|(id, _, _)| id == SESSION_ID),
            "{checks:?}"
        );
        assert_eq!(
            checks.iter().filter(|(_, after, _)| !after).count(),
            main,
            "{checks:?}"
        );
        assert_eq!(
            checks.iter().filter(|(_, after, _)| *after).count(),
            extraction,
            "{checks:?}"
        );
    }
}

pub(super) struct Barrier {
    pub entered: Semaphore,
    pub release: Semaphore,
}
impl Barrier {
    pub fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
    pub async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    async fn hold(&self) {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum MainReply {
    Success,
    Empty,
    Error,
}

pub(super) struct Client {
    pub guard: Arc<Guard>,
    pub main: MainReply,
    pub main_calls: AtomicUsize,
    pub main_tokens: Mutex<Vec<tokio_util::sync::CancellationToken>>,
    pub extraction_calls: AtomicUsize,
    pub extraction: Barrier,
    pub block_extraction: bool,
    pub extraction_dropped_pending: AtomicBool,
    pub extraction_returned: AtomicBool,
    pub arm_pre_store: Arc<AtomicBool>,
}
struct PendingExtraction<'a> {
    client: &'a Client,
    completed: bool,
}
impl Drop for PendingExtraction<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.client
                .extraction_dropped_pending
                .store(true, Ordering::SeqCst);
        }
    }
}
impl Client {
    pub fn new(
        guard: Arc<Guard>,
        main: MainReply,
        block_extraction: bool,
        arm: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            guard,
            main,
            main_calls: AtomicUsize::new(0),
            main_tokens: Mutex::new(Vec::new()),
            extraction_calls: AtomicUsize::new(0),
            extraction: Barrier::new(),
            block_extraction,
            extraction_dropped_pending: AtomicBool::new(false),
            extraction_returned: AtomicBool::new(false),
            arm_pre_store: arm,
        })
    }
    fn main_response(&self) -> anyhow::Result<LlmResponse> {
        self.main_calls.fetch_add(1, Ordering::SeqCst);
        let text = match self.main {
            MainReply::Success => MAIN_TEXT,
            MainReply::Empty => "",
            MainReply::Error => {
                return Err(
                    crate::llm::NonRetryableLlmError::new("typed main fixture failure").into(),
                )
            }
        };
        self.guard.main_returned.store(true, Ordering::SeqCst);
        let mut result = response(text);
        result.usage = main_usage();
        Ok(result)
    }
}
#[async_trait]
impl LlmClient for Client {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        // Fixture stage is identified by actual production tool availability,
        // never by prompt meaning or an injected host charge.
        if !tools.is_empty() {
            return self.main_response();
        }
        self.extraction_calls.fetch_add(1, Ordering::SeqCst);
        self.extraction.entered.add_permits(1);
        let mut pending = PendingExtraction {
            client: self,
            completed: false,
        };
        if self.block_extraction {
            self.extraction.release.acquire().await.unwrap().forget();
        }
        self.arm_pre_store.store(true, Ordering::SeqCst);
        self.extraction_returned.store(true, Ordering::SeqCst);
        pending.completed = true;
        let mut result = response(&valid_model_output());
        result.usage = extraction_usage();
        Ok(result)
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.main_tokens.lock().unwrap().push(cancel);
        let result = self.main_response()?;
        let (tx, rx) = mpsc::channel(2);
        if !result.message.text().is_empty() {
            tx.try_send(StreamEvent::TextDelta(result.message.text()))
                .unwrap();
        }
        tx.try_send(StreamEvent::Done(result)).unwrap();
        Ok(rx)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum StoreOutcome {
    Ack,
    ErrBefore,
    CommitThenErr,
}

// Only this wrapper injects a bounded fault/barrier. Every successful write
// and every read still goes through the selected original V1 adapter.
pub(super) struct StoreBoundary {
    pub inner: Arc<AuditedStore>,
    pub pre_store: Barrier,
    pub entered_store: Barrier,
    pub arm: Arc<AtomicBool>,
    pub block_pre_store: bool,
    pub block_entered_store: bool,
    pub outcome: StoreOutcome,
    pub entered: AtomicUsize,
    pub committed: Mutex<Vec<MemoryItem>>,
}
#[async_trait]
impl MemoryStore for StoreBoundary {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        self.store_and_return(item).await.map(|_| ())
    }
    async fn store_and_return(&self, item: MemoryItem) -> anyhow::Result<MemoryItem> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        if self.block_entered_store {
            self.entered_store.hold().await;
        }
        if matches!(self.outcome, StoreOutcome::ErrBefore) {
            anyhow::bail!("entered fixture before inner store");
        }
        let canonical = self.inner.store_and_return(item).await?;
        self.committed.lock().unwrap().push(canonical.clone());
        if matches!(self.outcome, StoreOutcome::CommitThenErr) {
            anyhow::bail!("original adapter committed; fixture response lost");
        }
        Ok(canonical)
    }
    async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
        self.inner.retrieve(id).await
    }
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        // Exact controlled distilled output selects the duplicate-lookup seam
        // after validation. It is not a semantic grant/classifier.
        if self.block_pre_store && query == DISTILLED && self.arm.swap(false, Ordering::SeqCst) {
            self.pre_store.hold().await;
        }
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

pub(super) struct FixturePolicy {
    pub disabled: bool,
    pub deny: bool,
}

pub(super) struct Fixture {
    pub store: StoreFixture,
    pub boundary: Arc<StoreBoundary>,
    pub client: Arc<Client>,
    pub guard: Arc<Guard>,
    pub session: Arc<AgentSession>,
    pub workspace: tempfile::TempDir,
}
impl Fixture {
    pub async fn new(
        kind: StoreKind,
        main: MainReply,
        block_extraction: bool,
        block_pre_store: bool,
        block_entered_store: bool,
        outcome: StoreOutcome,
        policy: FixturePolicy,
    ) -> Self {
        let FixturePolicy { disabled, deny } = policy;
        let store = StoreFixture::new(kind).await;
        let arm = Arc::new(AtomicBool::new(false));
        let boundary = Arc::new(StoreBoundary {
            inner: store.store.clone(),
            pre_store: Barrier::new(),
            entered_store: Barrier::new(),
            arm: arm.clone(),
            block_pre_store,
            block_entered_store,
            outcome,
            entered: AtomicUsize::new(0),
            committed: Mutex::new(Vec::new()),
        });
        let guard = Arc::new(Guard::default());
        guard.deny_extractor.store(deny, Ordering::SeqCst);
        let client = Client::new(guard.clone(), main, block_extraction, arm);
        let base = std::env::temp_dir().canonicalize().unwrap();
        let workspace = tempfile::tempdir_in(base).unwrap();
        let mut config = crate::agent_api::tests::test_config();
        config.memory = Some(crate::memory::MemoryConfig {
            llm_extraction: !disabled,
            prune_policy: None,
            ..crate::memory::MemoryConfig::default()
        });
        let agent = crate::Agent::from_config(config).await.unwrap();
        let session_store = Arc::new(crate::store::MemorySessionStore::new());
        let options = crate::SessionOptions::new()
            .with_session_id(SESSION_ID)
            .with_planning_mode(crate::prompts::PlanningMode::Disabled)
            .with_memory(boundary.clone())
            .with_budget_guard(guard.clone())
            .with_session_store(session_store)
            .with_auto_save(true);
        let session = Arc::new(
            agent
                .build_session(
                    workspace.path().to_string_lossy().into_owned(),
                    client.clone(),
                    &options,
                )
                .unwrap(),
        );
        let original_guard: Arc<dyn BudgetGuard> = guard.clone();
        assert!(Arc::ptr_eq(
            session.config.budget_guard.as_ref().unwrap(),
            &original_guard
        ));
        assert_eq!(session.workspace, workspace.path().canonicalize().unwrap());
        assert_eq!(
            session.memory().unwrap().llm_extraction_enabled(),
            !disabled
        );
        Self {
            store,
            boundary,
            client,
            guard,
            session,
            workspace,
        }
    }
    pub async fn ordinary(kind: StoreKind) -> Self {
        Self::new(
            kind,
            MainReply::Success,
            false,
            false,
            false,
            StoreOutcome::Ack,
            FixturePolicy {
                disabled: false,
                deny: false,
            },
        )
        .await
    }
    pub fn original_cancel(&self) -> tokio_util::sync::CancellationToken {
        self.session.session_cancel_token()
    }
    pub fn assert_original_cancelled(&self) {
        assert!(self.session.session_cancel_token().is_cancelled());
        let tokens = self.client.main_tokens.lock().unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            tokens[0].is_cancelled(),
            "actual main provider token descends from the original Session"
        );
    }
    pub fn assert_calls(&self, main: usize, extraction: usize, entered: usize) {
        assert_eq!(self.client.main_calls.load(Ordering::SeqCst), main);
        assert_eq!(
            self.client.extraction_calls.load(Ordering::SeqCst),
            extraction
        );
        assert_eq!(self.boundary.entered.load(Ordering::SeqCst), entered);
    }
    pub async fn drain(&self) {
        assert!(
            self.session
                .memory()
                .unwrap()
                .drain_llm_extractions(Duration::from_secs(5))
                .await
        );
        assert_no_ticket(&self.session).await;
    }
    pub async fn canonical(&self) -> Vec<MemoryItem> {
        let items = self.store.read_back().await;
        for item in &items {
            assert_eq!(item.content, DISTILLED);
            assert_eq!(
                item.metadata.get("schema").map(String::as_str),
                Some("a3s.memory.durable.v1")
            );
            assert_eq!(
                item.metadata.get("session_id").map(String::as_str),
                Some(SESSION_ID)
            );
            assert_eq!(
                item.metadata.get("workspace").map(String::as_str),
                Some(self.workspace.path().to_str().unwrap())
            );
        }
        items
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Route {
    Blocking,
    Stream,
    Detached,
    Exact,
}
pub(super) const ROUTES: [Route; 4] = [
    Route::Blocking,
    Route::Stream,
    Route::Detached,
    Route::Exact,
];
#[derive(Debug)]
pub(super) struct CallOutcome {
    pub result: Option<std::result::Result<crate::agent::AgentResult, crate::error::CodeError>>,
    pub events: Vec<AgentEvent>,
    pub run_id: Option<String>,
    route: Route,
    persisted: Option<crate::store::SessionSnapshotV1>,
    workspace: String,
    original_cancelled: bool,
}
impl CallOutcome {
    pub fn assert_success(&self) {
        self.assert_successful_publication(MAIN_TEXT);
    }

    pub fn assert_successful_publication(&self, expected_text: &str) {
        if let Some(result) = &self.result {
            match result {
                Ok(result) => {
                    assert_eq!(result.text, expected_text);
                    assert_eq!(record_tuple(&result.usage), record_tuple(&main_usage()));
                }
                Err(error) if error.cancelled_run_id().is_some() => {
                    assert!(matches!(self.route, Route::Blocking));
                    assert!(self.original_cancelled);
                    assert_eq!(error.cancelled_run_id(), self.run_id.as_deref());
                }
                Err(error) => panic!("unexpected main-turn failure: {error}"),
            }
        } else {
            assert!(self.run_id.as_ref().is_some_and(|id| !id.is_empty()));
            assert!(
                !self
                    .events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Error { .. })),
                "{self:?}"
            );
            let ends = self
                .events
                .iter()
                .filter_map(|event| match event {
                    AgentEvent::End { text, usage, .. } => Some((text, usage)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(ends.len(), 1, "{self:?}");
            assert_eq!(ends[0].0, expected_text);
            // Stream's host End is a completion marker with default usage.
            // Detached/exact events are actual retained Run-store events.
            if !matches!(self.route, Route::Stream) {
                assert_eq!(record_tuple(ends[0].1), record_tuple(&main_usage()));
            }
        }

        // Read by the caller after the original returned worker joined. Only
        // TurnFinish/checkpoint auto-save writes this original SessionStore;
        // the fixture never constructs or saves an expected snapshot.
        let persisted = self.persisted.as_ref().unwrap();
        assert_eq!(persisted.session.id, SESSION_ID);
        assert_eq!(persisted.session.config.workspace, self.workspace);
        assert_eq!(
            record_tuple(&persisted.session.total_usage),
            record_tuple(&main_usage())
        );
        assert_eq!(persisted.run_records.len(), 1, "{self:?}");
        let record = &persisted.run_records[0];
        assert_eq!(record.snapshot.session_id, SESSION_ID);
        assert!(!record.snapshot.id.is_empty());
        assert_eq!(&record.snapshot.id, self.run_id.as_ref().unwrap());
        assert_eq!(
            record.snapshot.prompt,
            if matches!(self.route, Route::Exact) {
                "<resume run=source-run>"
            } else {
                REMEMBER_PROMPT
            }
        );
        assert!(!record
            .events
            .iter()
            .any(|event| matches!(&event.event, AgentEvent::Error { .. })));
        let ends = record
            .events
            .iter()
            .filter_map(|record| match &record.event {
                AgentEvent::End { text, usage, .. } => Some((text, usage)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if ends.is_empty() {
            // Blocking extracts before TurnFinish. Original cancellation in
            // that interval returns typed cancellation and omits End, while
            // preserving the actual successful main-model usage below.
            assert!(matches!(self.route, Route::Blocking));
            assert!(matches!(self.result.as_ref(),
                Some(Err(error))
                if error.cancelled_run_id().is_some()
                    && error.cancelled_run_id() == self.run_id.as_deref()));
            assert!(self.original_cancelled);
            assert_eq!(record.snapshot.status, crate::run::RunStatus::Cancelled);
        } else {
            assert_eq!(ends.len(), 1, "{self:?}");
            assert_eq!(ends[0].0, expected_text);
            assert_eq!(record_tuple(ends[0].1), record_tuple(&main_usage()));
        }
    }
}

// This is the test caller, not another production executor. It retains the
// actual Session and awaits the returned actual worker. It proves no caller-
// Drop/last-Session ownership or public RunHandle cancellation guarantee.
pub(super) fn start(session: &Arc<AgentSession>, route: Route) -> JoinHandle<CallOutcome> {
    let session = session.clone();
    tokio::spawn(async move {
        let (result, events, run_id) = match route {
            Route::Blocking => (
                Some(session.send(REMEMBER_PROMPT, None).await),
                Vec::new(),
                None,
            ),
            Route::Stream => {
                let stream = session
                    .stream_with_run(REMEMBER_PROMPT, None)
                    .await
                    .unwrap();
                let id = stream.run.as_ref().unwrap().id().to_owned();
                let mut events = stream.events;
                let mut observed = Vec::new();
                while let Some(event) = events.recv().await {
                    observed.push(event);
                }
                stream.worker.await.unwrap();
                (None, observed, Some(id))
            }
            Route::Detached | Route::Exact => {
                let run = match route {
                    Route::Exact => {
                        session
                            .session_store
                            .as_ref()
                            .unwrap()
                            .save_loop_checkpoint("source-run", &checkpoint(&session))
                            .await
                            .unwrap();
                        session
                            .spawn_recovery_with_run_id("source-run", "lifecycle-recovered")
                            .await
                            .unwrap()
                    }
                    _ => session
                        .spawn_run_with_id("lifecycle-detached", REMEMBER_PROMPT)
                        .await
                        .unwrap(),
                };
                let AgentRunSpawn::Started { snapshot, worker } = run else {
                    panic!("fresh identity replayed");
                };
                worker.await.unwrap();
                let events = session
                    .run_events(&snapshot.id)
                    .await
                    .into_iter()
                    .map(|record| record.event)
                    .collect();
                (None, events, Some(snapshot.id))
            }
        };
        // Blocking has no returned RunHandle. Its fresh fixture still has
        // exactly one actual original Run; bind the saved ID to that read.
        let run_id = match run_id {
            Some(id) => Some(id),
            None => {
                assert!(matches!(route, Route::Blocking));
                let records = session.run_store.records().await;
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].snapshot.session_id, SESSION_ID);
                assert_eq!(records[0].snapshot.prompt, REMEMBER_PROMPT);
                Some(records[0].snapshot.id.clone())
            }
        };
        let persisted = session
            .session_store
            .as_ref()
            .unwrap()
            .load_snapshot(SESSION_ID)
            .await
            .unwrap();
        CallOutcome {
            result,
            events,
            run_id,
            route,
            persisted,
            workspace: session.workspace.to_str().unwrap().to_owned(),
            original_cancelled: session.session_cancel_token().is_cancelled(),
        }
    })
}
pub(super) async fn join(call: JoinHandle<CallOutcome>) -> CallOutcome {
    tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .unwrap()
        .unwrap()
}

// This records the actual pinned close future's wake, without driving it,
// cancelling anything, or spawning another owner/task.
#[derive(Default)]
pub(super) struct CloseWake {
    count: AtomicUsize,
    at: Mutex<Option<tokio::time::Instant>>,
    notified: tokio::sync::Notify,
}
impl std::task::Wake for CloseWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.at
            .lock()
            .unwrap()
            .get_or_insert_with(tokio::time::Instant::now);
        self.count.fetch_add(1, Ordering::SeqCst);
        self.notified.notify_one();
    }
}
impl CloseWake {
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) -> tokio::time::Instant {
        loop {
            let notified = self.notified.notified();
            if self.count() != 0 {
                return self.at.lock().unwrap().unwrap();
            }
            // notify_one retains a permit if the actual wake happens after
            // the count check but before this waiter is first polled.
            notified.await;
        }
    }
}
pub(super) fn poll_close<F: std::future::Future<Output = ()>>(
    close: std::pin::Pin<&mut F>,
    wake: &Arc<CloseWake>,
) -> Poll<()> {
    let waker = std::task::Waker::from(Arc::clone(wake));
    let mut context = std::task::Context::from_waker(&waker);
    let future = tokio::task::unconstrained(close);
    tokio::pin!(future);
    std::future::Future::poll(future.as_mut(), &mut context)
}

#[derive(Clone, Default)]
pub(super) struct TraceCapture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
struct Visitor(BTreeMap<String, String>);
impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.0.insert(f.name().into(), format!("{v:?}"));
    }
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        self.0.insert(f.name().into(), v.into());
    }
    fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
        self.0.insert(f.name().into(), v.to_string());
    }
    fn record_bool(&mut self, f: &tracing::field::Field, v: bool) {
        self.0.insert(f.name().into(), v.to_string());
    }
}
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TraceCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "memory_retention"
            || event
                .metadata()
                .target()
                .ends_with("agent_api::session_close")
        {
            let mut v = Visitor(BTreeMap::new());
            event.record(&mut v);
            v.0.insert("__target".into(), event.metadata().target().into());
            self.0.lock().unwrap().push(v.0);
        }
    }
}
impl TraceCapture {
    pub fn install(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }
    pub fn events(&self) -> Vec<BTreeMap<String, String>> {
        self.0.lock().unwrap().clone()
    }
    pub fn last(&self) -> BTreeMap<String, String> {
        self.events()
            .into_iter()
            .rfind(|event| event.get("__target").map(String::as_str) == Some("memory_retention"))
            .unwrap()
    }
}
pub(super) fn field(event: &BTreeMap<String, String>, name: &str, value: &str) {
    assert_eq!(
        event.get(name).map(String::as_str),
        Some(value),
        "{event:?}"
    );
}
pub(super) fn canonical_ids(event: &BTreeMap<String, String>) -> Vec<String> {
    serde_json::from_str(event.get("canonical_ids").unwrap()).unwrap()
}
