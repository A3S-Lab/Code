#![cfg(test)]

use super::*;
use crate::agent::{AgentConfig, AgentLoop};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::memory::{AgentMemory, MemoryConfig, MemoryObservation, MemoryObserver};
use a3s_memory::{FileMemoryStore, InMemoryStore, MemoryItem, MemoryStore};
use async_trait::async_trait;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tracing_subscriber::layer::SubscriberExt;

const PROMPT: &str = "Remember the durable verification workflow for retention-boundary changes.";
const CONTENT: &str = "Run focused retention-boundary checks before publishing workspace changes.";

#[derive(Clone, Copy, Debug)]
enum StoreKind {
    Memory,
    File,
}
const STORES: [StoreKind; 2] = [StoreKind::Memory, StoreKind::File];

#[derive(Clone, Copy, Debug)]
enum WriteCase {
    Success,
    Reject,
    CommitError,
    HoldBefore,
    HoldAfter,
}

struct BoundaryStore {
    inner: Arc<dyn MemoryStore>,
    primary: Mutex<VecDeque<WriteCase>>,
    archive: Mutex<VecDeque<WriteCase>>,
    primary_entries: AtomicUsize,
    archive_entries: AtomicUsize,
    blocked: Semaphore,
    release: Semaphore,
    read_barrier: Mutex<Option<(String, bool)>>,
    read_entered: Semaphore,
    read_release: Semaphore,
}

impl BoundaryStore {
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.blocked.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    async fn hold(&self) {
        self.blocked.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
    fn entries(&self) -> usize {
        self.primary_entries.load(Ordering::SeqCst) + self.archive_entries.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl MemoryStore for BoundaryStore {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        self.archive_entries.fetch_add(1, Ordering::SeqCst);
        let case = self
            .archive
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(WriteCase::Success);
        if matches!(case, WriteCase::Reject) {
            anyhow::bail!("controlled store refusal");
        }
        if matches!(case, WriteCase::HoldBefore) {
            self.hold().await;
        }
        self.inner.store(item).await?;
        if matches!(case, WriteCase::CommitError) {
            anyhow::bail!("controlled response lost after physical commit");
        }
        if matches!(case, WriteCase::HoldAfter) {
            self.hold().await;
        }
        Ok(())
    }
    async fn store_and_return(&self, item: MemoryItem) -> anyhow::Result<MemoryItem> {
        self.primary_entries.fetch_add(1, Ordering::SeqCst);
        let case = self
            .primary
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(WriteCase::Success);
        if matches!(case, WriteCase::Reject) {
            anyhow::bail!("controlled store refusal");
        }
        if matches!(case, WriteCase::HoldBefore) {
            self.hold().await;
        }
        let canonical = self.inner.store_and_return(item).await?;
        if matches!(case, WriteCase::CommitError) {
            anyhow::bail!("controlled response lost after physical commit");
        }
        if matches!(case, WriteCase::HoldAfter) {
            self.hold().await;
        }
        Ok(canonical)
    }
    async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
        let item = self.inner.retrieve(id).await?;
        let barrier = self
            .read_barrier
            .lock()
            .unwrap()
            .clone()
            .filter(|(target, _)| target == id);
        if let Some((_, missing)) = barrier {
            self.read_entered.add_permits(1);
            self.read_release.acquire().await.unwrap().forget();
            if missing {
                return Ok(None);
            }
        }
        Ok(item)
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
}

struct Fixture {
    store: Arc<BoundaryStore>,
    directory: tempfile::TempDir,
    kind: StoreKind,
}

impl Fixture {
    async fn new(kind: StoreKind, primary: Vec<WriteCase>, archive: Vec<WriteCase>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let inner: Arc<dyn MemoryStore> = match kind {
            StoreKind::Memory => Arc::new(InMemoryStore::new()),
            StoreKind::File => Arc::new(FileMemoryStore::new(directory.path()).await.unwrap()),
        };
        Self {
            directory,
            kind,
            store: Arc::new(BoundaryStore {
                inner,
                primary: Mutex::new(primary.into()),
                archive: Mutex::new(archive.into()),
                primary_entries: AtomicUsize::new(0),
                archive_entries: AtomicUsize::new(0),
                blocked: Semaphore::new(0),
                release: Semaphore::new(0),
                read_barrier: Mutex::new(None),
                read_entered: Semaphore::new(0),
                read_release: Semaphore::new(0),
            }),
        }
    }
    async fn read_back(&self) -> Vec<MemoryItem> {
        let reader: Arc<dyn MemoryStore> = match self.kind {
            StoreKind::Memory => self.store.inner.clone(),
            StoreKind::File => Arc::new(FileMemoryStore::new(self.directory.path()).await.unwrap()),
        };
        let items = reader.get_recent(100).await.unwrap();
        assert_eq!(reader.count().await.unwrap(), items.len());
        for item in &items {
            let actual = reader.retrieve(&item.id).await.unwrap().unwrap();
            assert_eq!(actual.id, item.id);
            assert_eq!(actual.content, item.content);
            assert_eq!(actual.metadata, item.metadata);
        }
        items
    }
}

#[derive(Clone, Default)]
struct TraceCapture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

struct Visitor(BTreeMap<String, String>);
impl tracing::field::Visit for Visitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().into(), value.to_string());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.name().into(), value.to_string());
    }
}
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TraceCapture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() == "memory_retention" {
            let mut visitor = Visitor(BTreeMap::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().push(visitor.0);
        }
    }
}
impl TraceCapture {
    fn install(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }
    fn last(&self) -> BTreeMap<String, String> {
        self.0.lock().unwrap().last().unwrap().clone()
    }
}

fn field(event: &BTreeMap<String, String>, name: &str, value: &str) {
    assert_eq!(
        event.get(name).map(String::as_str),
        Some(value),
        "{event:?}"
    );
}
fn ids(event: &BTreeMap<String, String>) -> Vec<String> {
    serde_json::from_str(event.get("canonical_ids").unwrap()).unwrap()
}

enum Reply {
    Text(String),
    Error,
    Pending,
}
struct Client {
    reply: Reply,
    calls: AtomicUsize,
    entered: Semaphore,
}
#[async_trait]
impl LlmClient for Client {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        let text = match &self.reply {
            Reply::Text(text) => text,
            // Looks like a budget refusal; it is deliberately NOT typed.
            Reply::Error => anyhow::bail!("BudgetExhausted: cancelled; invalid item"),
            Reply::Pending => return std::future::pending().await,
        };
        Ok(LlmResponse {
            message: Message::assistant(text),
            usage: Default::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }
    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("unused")
    }
}

fn proposal(content: &str, supersedes: &[String]) -> serde_json::Value {
    json!({"memory_type":"procedural","content":content,"importance":0.9,"confidence":0.96,"source":"workflow","scope":"workspace","reason":"This repeatable workflow applies to future verification changes.","supersedes":supersedes})
}
fn output(items: Vec<serde_json::Value>) -> String {
    json!({"items":items}).to_string()
}
fn client(reply: Reply) -> Arc<Client> {
    Arc::new(Client {
        reply,
        calls: AtomicUsize::new(0),
        entered: Semaphore::new(0),
    })
}

fn agent(
    memory: Arc<AgentMemory>,
    provider: Arc<Client>,
    workspace: &std::path::Path,
    budget: Option<Arc<dyn crate::budget::BudgetGuard>>,
) -> AgentLoop {
    AgentLoop::new(
        provider,
        Arc::new(crate::tools::ToolExecutor::new(
            workspace.to_string_lossy().into_owned(),
        )),
        crate::tools::ToolContext::new(workspace.to_owned()),
        AgentConfig {
            memory: Some(memory),
            budget_guard: budget,
            ..Default::default()
        },
    )
}

async fn extract(agent: &AgentLoop, cancel: &CancellationToken) {
    agent
        .fact_extract_turn_memory(
            PROMPT,
            "The verification workflow is complete.",
            "progress-session",
            cancel,
        )
        .await;
}

#[tokio::test]
async fn parser_rejections_and_original_typed_cap_are_classified_in_both_stores() {
    for kind in STORES {
        let mut ten_invalid = vec![json!({"content":42})];
        ten_invalid.extend((0..10).map(|_| {
            let mut value = proposal(CONTENT, &[]);
            value["confidence"] = json!(0.1);
            value
        }));
        ten_invalid.push(proposal(
            "The eleventh typed proposal must never become an extra write.",
            &[],
        ));
        for (text, accepted, rejected, disposition, entered) in [
            (output(Vec::new()), "0", "0", "no_items", "0"),
            ("not JSON".into(), "0", "0", "refused", "0"),
            (
                output(vec![json!({"content":42})]),
                "0",
                "1",
                "refused",
                "0",
            ),
            (
                output(vec![proposal(CONTENT, &[]), json!({"content":42})]),
                "1",
                "1",
                "partial",
                "1",
            ),
            (output(ten_invalid), "0", "11", "refused", "0"),
        ] {
            let fixture = Fixture::new(kind, vec![], vec![]).await;
            let memory = Arc::new(AgentMemory::with_config(
                fixture.store.clone(),
                MemoryConfig {
                    llm_extraction_max_items: 10,
                    ..Default::default()
                },
            ));
            let workspace = tempfile::tempdir().unwrap();
            let provider = client(Reply::Text(text));
            let agent = agent(memory, provider, workspace.path(), None);
            let traces = TraceCapture::default();
            let _guard = traces.install();
            extract(&agent, &CancellationToken::new()).await;
            let event = traces.last();
            field(&event, "accepted_items", accepted);
            field(&event, "rejected_items", rejected);
            field(&event, "disposition", disposition);
            field(&event, "entered_store_calls", entered);
            assert_eq!(fixture.store.entries().to_string(), entered);
            assert_eq!(fixture.read_back().await.len().to_string(), entered);
        }
    }
}

struct DenyBudget {
    checks: AtomicUsize,
}
#[async_trait]
impl crate::budget::BudgetGuard for DenyBudget {
    async fn check_before_llm(
        &self,
        _session: &str,
        _estimated: usize,
    ) -> crate::budget::BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        crate::budget::BudgetDecision::Deny {
            resource: "test-tokens".into(),
            reason: "controlled typed denial".into(),
        }
    }
}

#[tokio::test]
async fn typed_budget_provider_error_and_effective_bound_cancel_stay_distinct_in_both_stores() {
    for kind in STORES {
        for mode in ["budget", "provider", "bound_cancel"] {
            let fixture = Fixture::new(kind, vec![], vec![]).await;
            let memory = Arc::new(AgentMemory::new(fixture.store.clone()));
            let workspace = tempfile::tempdir().unwrap();
            let provider = client(Reply::Error);
            let budget = Arc::new(DenyBudget {
                checks: AtomicUsize::new(0),
            });
            let mut agent = agent(
                memory.clone(),
                provider.clone(),
                workspace.path(),
                if mode == "budget" {
                    Some(budget.clone())
                } else {
                    None
                },
            );
            let supplied = CancellationToken::new();
            let bound = CancellationToken::new();
            if mode == "bound_cancel" {
                let invocation = agent.invocation_context(
                    "bound-run",
                    Some("progress-session"),
                    None,
                    bound.clone(),
                );
                agent = invocation.bind_agent_loop(&agent);
                bound.cancel();
                assert!(!supplied.is_cancelled());
            }
            let traces = TraceCapture::default();
            let _guard = traces.install();
            extract(&agent, &supplied).await;
            let event = traces.last();
            field(&event, "disposition", "refused");
            field(
                &event,
                "refusal",
                match mode {
                    "budget" => "budget_denied",
                    "provider" => "provider_failed",
                    _ => "cancelled",
                },
            );
            assert_eq!(
                provider.calls.load(Ordering::SeqCst),
                usize::from(mode == "provider")
            );
            assert_eq!(
                budget.checks.load(Ordering::SeqCst),
                usize::from(mode == "budget")
            );
            assert_eq!(fixture.store.entries(), 0);
            assert!(fixture.read_back().await.is_empty());
            assert!(memory.drain_llm_extractions(Duration::from_secs(1)).await);
        }
    }
}

#[tokio::test]
async fn primary_entry_ack_error_and_lost_response_have_actual_readback_in_both_stores() {
    for kind in STORES {
        for case in [
            WriteCase::Success,
            WriteCase::Reject,
            WriteCase::CommitError,
            WriteCase::HoldBefore,
            WriteCase::HoldAfter,
        ] {
            let fixture = Fixture::new(kind, vec![case], vec![]).await;
            let memory = Arc::new(AgentMemory::new(fixture.store.clone()));
            let workspace = tempfile::tempdir().unwrap();
            let provider = client(Reply::Text(output(vec![proposal(CONTENT, &[])])));
            let agent = agent(memory.clone(), provider, workspace.path(), None);
            let cancel = CancellationToken::new();
            let traces = TraceCapture::default();
            let _guard = traces.install();
            let mut future = Box::pin(extract(&agent, &cancel));
            if matches!(case, WriteCase::HoldBefore | WriteCase::HoldAfter) {
                tokio::select! { _ = fixture.store.wait() => {}, _ = &mut future => panic!("store barrier was bypassed") }
                assert_eq!(fixture.store.entries(), 1);
                assert_eq!(
                    fixture.read_back().await.len(),
                    usize::from(matches!(case, WriteCase::HoldAfter))
                );
                drop(future);
            } else {
                future.await;
            }
            let event = traces.last();
            field(&event, "entered_store_calls", "1");
            if matches!(case, WriteCase::Success) {
                field(&event, "disposition", "stored");
                field(&event, "unproved_write", "false");
                let items = fixture.read_back().await;
                assert_eq!(items.len(), 1);
                assert_eq!(ids(&event), vec![items[0].id.clone()]);
            } else {
                field(&event, "disposition", "unproved");
                field(&event, "unproved_write", "true");
                assert!(ids(&event).is_empty());
                assert_eq!(
                    fixture.read_back().await.len(),
                    usize::from(matches!(
                        case,
                        WriteCase::CommitError | WriteCase::HoldAfter
                    ))
                );
            }
            assert!(memory.drain_llm_extractions(Duration::from_secs(1)).await);
        }
    }
}

struct PendingObserver {
    entered: Semaphore,
}
#[async_trait]
impl MemoryObserver for PendingObserver {
    async fn on_memory_stored(&self, _observation: MemoryObservation) -> anyhow::Result<()> {
        self.entered.add_permits(1);
        std::future::pending().await
    }
}

#[tokio::test]
async fn primary_ack_precedes_observer_await_and_drop_keeps_later_obligations_in_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind, vec![], vec![]).await;
        let observer = Arc::new(PendingObserver {
            entered: Semaphore::new(0),
        });
        let memory = Arc::new(AgentMemory::with_config_and_observers(
            fixture.store.clone(),
            MemoryConfig::default(),
            vec![observer.clone()],
        ));
        let workspace = tempfile::tempdir().unwrap();
        let provider = client(Reply::Text(output(vec![
            proposal(CONTENT, &[]),
            proposal(
                "Always preserve independent archive acknowledgements in verification reports.",
                &[],
            ),
        ])));
        let agent = agent(memory.clone(), provider, workspace.path(), None);
        let cancel = CancellationToken::new();
        let traces = TraceCapture::default();
        let _guard = traces.install();
        let mut future = Box::pin(extract(&agent, &cancel));
        tokio::select! { _ = observer.entered.acquire() => {}, _ = &mut future => panic!("observer must remain pending") }
        assert_eq!(fixture.store.entries(), 1);
        let stored = fixture.read_back().await;
        assert_eq!(stored.len(), 1);
        drop(future);
        let event = traces.last();
        field(&event, "disposition", "partial");
        field(&event, "accepted_items", "2");
        field(&event, "entered_store_calls", "1");
        field(&event, "unproved_write", "false");
        field(&event, "abandoned", "true");
        assert_eq!(ids(&event), vec![stored[0].id.clone()]);
        assert!(memory.drain_llm_extractions(Duration::from_secs(1)).await);
    }
}

#[tokio::test]
async fn later_primary_error_preserves_earlier_canonical_ack_in_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(
            kind,
            vec![WriteCase::Success, WriteCase::CommitError],
            vec![],
        )
        .await;
        let memory = Arc::new(AgentMemory::new(fixture.store.clone()));
        let workspace = tempfile::tempdir().unwrap();
        let provider = client(Reply::Text(output(vec![
            proposal(CONTENT, &[]),
            proposal(
                "Record the exact storage boundary before assessing archive relationships.",
                &[],
            ),
        ])));
        let agent = agent(memory, provider, workspace.path(), None);
        let traces = TraceCapture::default();
        let _guard = traces.install();
        extract(&agent, &CancellationToken::new()).await;
        let event = traces.last();
        field(&event, "disposition", "partial");
        field(&event, "entered_store_calls", "2");
        field(&event, "unproved_write", "true");
        field(&event, "refusal", "store_failed");
        let known = ids(&event);
        assert_eq!(known.len(), 1);
        let stored = fixture.read_back().await;
        assert_eq!(stored.len(), 2);
        assert!(stored
            .iter()
            .any(|item| item.id == known[0] && item.content == CONTENT));
    }
}

#[tokio::test]
async fn archives_recheck_after_read_and_keep_exact_unit_ack_or_unknown_in_both_stores() {
    for kind in STORES {
        for mode in [
            "cancel_read",
            "missing",
            "ack",
            "error_after_commit",
            "drop_after_commit",
        ] {
            let case = match mode {
                "error_after_commit" => WriteCase::CommitError,
                "drop_after_commit" => WriteCase::HoldAfter,
                _ => WriteCase::Success,
            };
            let fixture = Fixture::new(kind, vec![], vec![case]).await;
            let old = MemoryItem::new(format!(
                "{PROMPT} Existing workflow before the retention-boundary revision."
            ));
            let old_id = old.id.clone();
            fixture.store.inner.store(old).await.unwrap();
            let memory = Arc::new(AgentMemory::new(fixture.store.clone()));
            let workspace = tempfile::tempdir().unwrap();
            let provider = client(Reply::Text(output(vec![proposal(
                CONTENT,
                std::slice::from_ref(&old_id),
            )])));
            let agent = agent(memory.clone(), provider, workspace.path(), None);
            let cancel = CancellationToken::new();
            let traces = TraceCapture::default();
            let _guard = traces.install();
            if matches!(mode, "cancel_read" | "missing") {
                *fixture.store.read_barrier.lock().unwrap() =
                    Some((old_id.clone(), mode == "missing"));
            }
            let mut future = Box::pin(extract(&agent, &cancel));
            if matches!(mode, "cancel_read" | "missing") {
                tokio::select! { _ = fixture.store.read_entered.acquire() => {}, _ = &mut future => panic!("archive read barrier was bypassed") }
                assert_eq!(fixture.store.entries(), 1);
                if mode == "cancel_read" {
                    cancel.cancel();
                } else {
                    fixture.store.inner.delete(&old_id).await.unwrap();
                }
                fixture.store.read_release.add_permits(1);
                future.await;
            } else if mode == "drop_after_commit" {
                tokio::select! { _ = fixture.store.wait() => {}, _ = &mut future => panic!("archive store barrier was bypassed") }
                drop(future);
            } else {
                future.await;
            }
            let event = traces.last();
            let items = fixture.read_back().await;
            let canonical_ids = ids(&event);
            assert_eq!(canonical_ids.len(), 1);
            assert!(items.iter().any(|item| item.id == canonical_ids[0]));
            assert!(event["archives"].contains(&old_id));
            let archive_entered = !matches!(mode, "cancel_read" | "missing");
            field(
                &event,
                "archive_entered",
                if archive_entered { "1" } else { "0" },
            );
            field(
                &event,
                "archive_acknowledged",
                if mode == "ack" { "1" } else { "0" },
            );
            field(
                &event,
                "archive_applied",
                if mode == "ack" { "1" } else { "0" },
            );
            field(
                &event,
                "disposition",
                if mode == "ack" { "stored" } else { "partial" },
            );
            field(
                &event,
                "unproved_write",
                if matches!(mode, "error_after_commit" | "drop_after_commit") {
                    "true"
                } else {
                    "false"
                },
            );
            assert_eq!(
                fixture.store.archive_entries.load(Ordering::SeqCst),
                usize::from(archive_entered)
            );
            if archive_entered {
                let old = items.iter().find(|item| item.id == old_id).unwrap();
                assert_eq!(
                    old.metadata.get("a3s.memory.status").map(String::as_str),
                    Some("superseded")
                );
                assert_eq!(old.metadata.get("superseded_by"), canonical_ids.first());
            }
            assert!(memory.drain_llm_extractions(Duration::from_secs(1)).await);
        }
    }
}

#[tokio::test]
async fn fifo_and_provider_future_drop_release_original_ticket_without_fabricating_items() {
    for kind in STORES {
        for queued in [false, true] {
            let fixture = Fixture::new(kind, vec![], vec![]).await;
            let memory = Arc::new(AgentMemory::new(fixture.store.clone()));
            let workspace = tempfile::tempdir().unwrap();
            let predecessor = queued.then(|| memory.enqueue_llm_extraction());
            let provider = client(Reply::Pending);
            let agent = agent(memory.clone(), provider.clone(), workspace.path(), None);
            let cancel = CancellationToken::new();
            let traces = TraceCapture::default();
            let _guard = traces.install();
            let mut future = Box::pin(extract(&agent, &cancel));
            if queued {
                assert!(matches!(futures::poll!(&mut future), Poll::Pending));
                assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
            } else {
                tokio::select! { _ = provider.entered.acquire() => {}, _ = &mut future => panic!("provider must remain pending") }
            }
            drop(future);
            drop(predecessor);
            let event = traces.last();
            field(&event, "disposition", "refused");
            field(&event, "refusal", "admission_unproved");
            field(&event, "entered_store_calls", "0");
            field(&event, "abandoned", "true");
            assert!(ids(&event).is_empty());
            assert_eq!(fixture.store.entries(), 0);
            assert!(fixture.read_back().await.is_empty());
            assert!(memory.drain_llm_extractions(Duration::from_secs(1)).await);
        }
    }
}

struct RefuseProgress;
impl MemoryWriteProgress for RefuseProgress {
    fn entered(&mut self) -> bool {
        false
    }
    fn acknowledged(&mut self, _canonical_id: Option<&str>) {
        panic!("refused entry cannot receive an Ack");
    }
}

#[tokio::test]
async fn progress_refusal_cannot_enter_either_original_store_call_in_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind, vec![], vec![]).await;
        let memory = AgentMemory::new(fixture.store.clone());
        let cancel = CancellationToken::new();
        assert!(memory
            .remember_item_with_progress(
                MemoryItem::new(CONTENT),
                Some(&cancel),
                Some(&mut RefuseProgress)
            )
            .await
            .is_err());
        let old =
            MemoryItem::new("Original workflow still requires actual archive entry bookkeeping.");
        let old_id = old.id.clone();
        fixture.store.inner.store(old).await.unwrap();
        assert!(memory
            .mark_superseded_with_progress(
                &old_id,
                "replacement",
                Some(&cancel),
                Some(&mut RefuseProgress)
            )
            .await
            .is_err());
        assert_eq!(fixture.store.entries(), 0);
        let items = fixture.read_back().await;
        assert_eq!(items.len(), 1);
        assert!(!items[0].metadata.contains_key("a3s.memory.status"));
        let mut progress =
            MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, 1, 0).unwrap();
        assert!(PrimaryProgress::new(&mut progress, 1).is_none());
        assert!(ArchiveProgress::new(&mut progress, 0).is_none());
    }
}

#[tokio::test]
async fn ten_primary_fifty_archive_calls_keep_sixty_entries_and_exact_ids_in_both_stores() {
    for kind in STORES {
        let fixture = Fixture::new(kind, vec![], vec![]).await;
        let mut old_ids = Vec::new();
        for index in 0..5 {
            let old = MemoryItem::new(format!(
                "{PROMPT} Existing retention-boundary workflow variant {index}."
            ));
            old_ids.push(old.id.clone());
            fixture.store.inner.store(old).await.unwrap();
        }
        let memory = Arc::new(AgentMemory::with_config(
            fixture.store.clone(),
            MemoryConfig {
                llm_extraction_max_items: 10,
                ..Default::default()
            },
        ));
        let items = (0..11).map(|index| proposal(&format!("Run retention-boundary workflow revision number {index} before publishing changes."), &old_ids)).collect();
        let provider = client(Reply::Text(output(items)));
        let workspace = tempfile::tempdir().unwrap();
        let agent = agent(memory, provider, workspace.path(), None);
        let traces = TraceCapture::default();
        let _guard = traces.install();
        extract(&agent, &CancellationToken::new()).await;
        let event = traces.last();
        field(&event, "disposition", "stored");
        field(&event, "accepted_items", "10");
        field(&event, "entered_store_calls", "60");
        field(&event, "archive_entered", "50");
        field(&event, "archive_acknowledged", "50");
        field(&event, "archive_applied", "50");
        field(&event, "unproved_write", "false");
        assert_eq!(fixture.store.primary_entries.load(Ordering::SeqCst), 10);
        assert_eq!(fixture.store.archive_entries.load(Ordering::SeqCst), 50);
        let canonical = ids(&event);
        assert_eq!(canonical.len(), 10);
        let actual = fixture.read_back().await;
        assert_eq!(actual.len(), 15);
        for id in &canonical {
            assert!(actual.iter().any(|item| &item.id == id));
        }
        for id in &old_ids {
            assert!(event["archives"].contains(id));
            let archived = actual.iter().find(|item| &item.id == id).unwrap();
            assert_eq!(
                archived
                    .metadata
                    .get("a3s.memory.status")
                    .map(String::as_str),
                Some("superseded")
            );
            assert!(canonical.contains(archived.metadata.get("superseded_by").unwrap()));
        }
        assert!(!actual
            .iter()
            .any(|item| item.content.contains("number 10 before")));
    }
}
