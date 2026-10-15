#![cfg(test)]

use crate::agent::{AgentConfig, AgentLoop};
use crate::durable_memory::{DurableMemoryRecallPolicy, DurableMemorySession};
use crate::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::memory::{AgentMemory, MemoryConfig};
use a3s_memory::repository::*;
use a3s_memory::{FileMemoryStore, InMemoryStore, MemoryItem, MemoryStore, MemoryType};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;

pub(super) const RAW: &str =
    "Remember the durable workflow for FileMemoryStore changes before the next release.";
pub(super) const CONTENT: &str =
    "Run focused memory store tests after changing FileMemoryStore behavior.";
pub(super) const RESPONSE: &str = "The FileMemoryStore verification workflow is complete.";
pub(super) const SESSION: &str = "T006-Session";
pub(super) const REASON: &str =
    "This exact workflow is reusable after future FileMemoryStore changes.";
pub(super) const HOST_MILLIS: u64 = 1_760_000_000_000;

#[derive(Clone, Copy, Debug)]
pub(super) enum StoreKind {
    Memory,
    File,
}
pub(super) const STORES: [StoreKind; 2] = [StoreKind::Memory, StoreKind::File];

pub(super) struct ObservedStore {
    pub(super) inner: Arc<dyn MemoryStore>,
    pub(super) reject_primary: AtomicBool,
    pub(super) reject_archive: AtomicBool,
    pub(super) primary_inputs: Mutex<Vec<MemoryItem>>,
    pub(super) primary_results: Mutex<Vec<MemoryItem>>,
    pub(super) archive_inputs: Mutex<Vec<MemoryItem>>,
}

#[async_trait]
impl MemoryStore for ObservedStore {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        self.archive_inputs.lock().unwrap().push(item.clone());
        if self.reject_archive.load(Ordering::SeqCst) {
            anyhow::bail!("controlled archive failure");
        }
        self.inner.store(item).await
    }
    async fn store_and_return(&self, item: MemoryItem) -> anyhow::Result<MemoryItem> {
        self.primary_inputs.lock().unwrap().push(item.clone());
        if self.reject_primary.load(Ordering::SeqCst) {
            anyhow::bail!("controlled primary failure");
        }
        let canonical = self.inner.store_and_return(item).await?;
        self.primary_results.lock().unwrap().push(canonical.clone());
        Ok(canonical)
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
}

pub(super) struct Fixture {
    kind: StoreKind,
    directory: tempfile::TempDir,
    pub(super) store: Arc<ObservedStore>,
}
impl Fixture {
    pub(super) async fn new(kind: StoreKind) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let inner: Arc<dyn MemoryStore> = match kind {
            StoreKind::Memory => Arc::new(InMemoryStore::new()),
            StoreKind::File => Arc::new(FileMemoryStore::new(directory.path()).await.unwrap()),
        };
        Self {
            kind,
            directory,
            store: Arc::new(ObservedStore {
                inner,
                reject_primary: AtomicBool::new(false),
                reject_archive: AtomicBool::new(false),
                primary_inputs: Mutex::new(Vec::new()),
                primary_results: Mutex::new(Vec::new()),
                archive_inputs: Mutex::new(Vec::new()),
            }),
        }
    }
    // Explicit prerequisite rows bypass observation counters; they never stand
    // in for the host/model operation under test.
    pub(super) async fn seed(&self, item: MemoryItem) -> MemoryItem {
        self.store.inner.store_and_return(item).await.unwrap()
    }
    pub(super) fn memory(&self, durable: Option<DurableMemorySession>) -> Arc<AgentMemory> {
        Arc::new(AgentMemory::with_config_observers_and_durable(
            self.store.clone(),
            MemoryConfig::default(),
            Vec::new(),
            durable,
        ))
    }
    pub(super) async fn read_back(&self) -> Vec<MemoryItem> {
        let reader: Arc<dyn MemoryStore> = match self.kind {
            StoreKind::Memory => self.store.inner.clone(),
            // A genuinely new File adapter reloads the persisted index/items.
            StoreKind::File => Arc::new(FileMemoryStore::new(self.directory.path()).await.unwrap()),
        };
        let rows = reader.get_recent(100).await.unwrap();
        assert_eq!(reader.count().await.unwrap(), rows.len());
        for row in &rows {
            assert_item(&reader.retrieve(&row.id).await.unwrap().unwrap(), row);
        }
        rows
    }
    pub(super) fn inputs(&self) -> Vec<MemoryItem> {
        self.store.primary_inputs.lock().unwrap().clone()
    }
    pub(super) fn results(&self) -> Vec<MemoryItem> {
        self.store.primary_results.lock().unwrap().clone()
    }
    pub(super) fn archives(&self) -> Vec<MemoryItem> {
        self.store.archive_inputs.lock().unwrap().clone()
    }
}

pub(super) fn assert_item(actual: &MemoryItem, expected: &MemoryItem) {
    assert_eq!(actual.id, expected.id);
    assert_eq!(actual.content, expected.content);
    assert_eq!(actual.timestamp, expected.timestamp);
    assert_eq!(actual.importance, expected.importance);
    assert_eq!(actual.memory_type, expected.memory_type);
    assert_eq!(actual.tags, expected.tags);
    assert_eq!(actual.metadata, expected.metadata);
    assert_eq!(actual.content_lower, expected.content_lower);
    // Backend retrieval records access independently; it is not a new write
    // acknowledgement and is deliberately outside these persistence fields.
}
pub(super) fn workspace(prefix: &str) -> tempfile::TempDir {
    // Supply the real host path, rather than macOS's /var symlink spelling.
    // ToolContext's existing canonicalization policy remains unchanged.
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
        .unwrap()
}
pub(super) fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 8, 29, 16, 0, 1)
        .single()
        .unwrap()
}
pub(super) fn seeded(id: &str, content: &str) -> MemoryItem {
    let mut item = MemoryItem::new(content)
        .with_type(MemoryType::Procedural)
        .with_importance(0.4)
        .with_tag("seed")
        .with_metadata("host_note", "retained");
    item.id = id.into();
    item.timestamp = fixed_time();
    item
}
pub(super) fn proposal(content: &str) -> Value {
    json!({"memory_type":"procedural", "content":content, "importance":0.85, "confidence":0.94,
        "tags":["memory","tests"], "source":"workflow", "scope":"workspace", "reason":REASON})
}
pub(super) fn output(items: Vec<Value>) -> String {
    json!({"items":items}).to_string()
}
pub(super) fn expected_metadata(workspace: &Path, session: &str) -> HashMap<String, String> {
    [
        ("source", "workflow".into()),
        ("confidence", "0.94".into()),
        ("scope", "workspace".into()),
        ("reason", REASON.into()),
        ("workspace", workspace.display().to_string()),
        ("session_id", session.into()),
        ("schema", "a3s.memory.durable.v1".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
pub(super) fn expected_tags() -> Vec<String> {
    ["llm", "extracted", "workflow", "memory", "tests"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}
pub(super) fn assert_distilled(item: &MemoryItem, content: &str, workspace: &Path, session: &str) {
    assert_eq!(item.content, content);
    assert_eq!(item.importance, 0.85);
    assert_eq!(item.memory_type, MemoryType::Procedural);
    assert_eq!(item.tags, expected_tags());
    assert_eq!(item.metadata, expected_metadata(workspace, session));
    assert!(uuid::Uuid::parse_str(&item.id).is_ok());
}

pub(super) struct Client {
    text: String,
    pub(super) calls: AtomicUsize,
    pub(super) messages: Mutex<Vec<Vec<Message>>>,
}
#[async_trait]
impl LlmClient for Client {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.messages.lock().unwrap().push(messages.to_vec());
        Ok(LlmResponse {
            message: Message::assistant(&self.text),
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
        anyhow::bail!("unused in T006 extractor fixture")
    }
}
pub(super) fn client(text: String) -> Arc<Client> {
    Arc::new(Client {
        text,
        calls: AtomicUsize::new(0),
        messages: Mutex::new(Vec::new()),
    })
}
pub(super) fn agent(memory: Arc<AgentMemory>, client: Arc<Client>, workspace: &Path) -> AgentLoop {
    AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new(
            workspace.display().to_string(),
        )),
        crate::tools::ToolContext::new(workspace.to_owned()),
        AgentConfig {
            memory: Some(memory),
            host_env: Arc::new(HostEnv::new(
                Arc::new(SequentialIdGenerator::new("T006-turn")),
                Arc::new(FixedClock::new(HOST_MILLIS)),
            )),
            ..Default::default()
        },
    )
}
pub(super) async fn extract(agent: &AgentLoop, prompt: &str) {
    agent
        .fact_extract_turn_memory(prompt, RESPONSE, SESSION, &CancellationToken::new())
        .await;
}

#[derive(Clone, Default)]
pub(super) struct TraceCapture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
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
    pub(super) fn install(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }
    pub(super) fn last(&self) -> BTreeMap<String, String> {
        self.0.lock().unwrap().last().unwrap().clone()
    }
}
pub(super) fn field(event: &BTreeMap<String, String>, name: &str, value: &str) {
    assert_eq!(
        event.get(name).map(String::as_str),
        Some(value),
        "{event:?}"
    );
}
pub(super) fn ids(event: &BTreeMap<String, String>) -> Vec<String> {
    serde_json::from_str(event.get("canonical_ids").unwrap()).unwrap()
}

pub(super) struct ShadowRepository {
    pub(super) inner: InMemoryRepository,
    pub(super) reject: bool,
    pub(super) attempts: Mutex<Vec<MemoryChangeSet>>,
}
impl ShadowRepository {
    pub(super) fn new(reject: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryRepository::new(),
            reject,
            attempts: Mutex::new(Vec::new()),
        })
    }
    pub(super) fn binding(self: &Arc<Self>, namespace: MemoryNamespace) -> DurableMemorySession {
        DurableMemorySession::active_recall(
            self.clone(),
            namespace,
            DurableMemoryRecallPolicy::try_new(5, 0.0).unwrap(),
        )
    }
}
#[async_trait]
impl MemoryRepository for ShadowRepository {
    async fn apply(
        &self,
        change_set: MemoryChangeSet,
    ) -> Result<MemoryChangeResult, MemoryRepositoryError> {
        self.attempts.lock().unwrap().push(change_set.clone());
        if self.reject {
            return Err(MemoryRepositoryError::Persistence {
                operation: "controlled shadow apply".into(),
                message: "controlled shadow failure".into(),
            });
        }
        self.inner.apply(change_set).await
    }
    async fn get(
        &self,
        namespace: &MemoryNamespace,
        id: &str,
    ) -> Result<Option<MemoryNode>, MemoryRepositoryError> {
        self.inner.get(namespace, id).await
    }
    async fn query(&self, query: MemoryQuery) -> Result<MemoryQueryResult, MemoryRepositoryError> {
        self.inner.query(query).await
    }
    async fn record_admission(
        &self,
        event: MemoryAccessEvent,
    ) -> Result<(), MemoryRepositoryError> {
        self.inner.record_admission(event).await
    }
    async fn record_use(&self, event: MemoryAccessEvent) -> Result<(), MemoryRepositoryError> {
        self.inner.record_use(event).await
    }
    async fn usage_summary(
        &self,
        namespace: &MemoryNamespace,
        id: &str,
    ) -> Result<MemoryUsageSummary, MemoryRepositoryError> {
        self.inner.usage_summary(namespace, id).await
    }
}
