use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::store::{FileSessionStore, SessionStore};
use a3s_code_core::{
    Agent, CodeError, DurableMemoryActivation, DurableMemoryRecallPolicy, DurableMemorySession,
    DurableMemoryUse, PlanningMode, SessionOptions,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, FileMemoryRepository, InMemoryRepository,
    MemoryChangeSet, MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository,
    MemoryStatus,
};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SESSION_ID: &str = "durable-memory-file-restart-v1";
const NODE_ID: &str = "restart-procedure";
const MEMORY_CONTENT: &str =
    "After restarting the SDK, reopen the workspace durable memory repository.";
const QUERY: &str = "How should I reopen durable memory after restarting the SDK?";

fn time(hour: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, hour, 0, 0)
        .single()
        .unwrap()
}

fn evidence(uri: &str, kind: EvidenceKind, hour: u32) -> EvidenceRef {
    EvidenceRef::try_new(uri, format!("sha256:{:0>64}", uri), kind, time(hour)).unwrap()
}

fn host_env(prefix: &str, hour: u32) -> Arc<HostEnv> {
    Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new(prefix)),
        Arc::new(FixedClock::new(
            u64::try_from(time(hour).timestamp_millis()).unwrap(),
        )),
    ))
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        memory: Some(MemoryConfig {
            llm_extraction: false,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[derive(Clone)]
struct InspectingClient {
    observations: Arc<Mutex<Vec<bool>>>,
}

impl InspectingClient {
    fn new() -> Self {
        Self {
            observations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn observations(&self) -> Vec<bool> {
        self.observations.lock().unwrap().clone()
    }

    fn respond(&self, system: Option<&str>) -> LlmResponse {
        let visible = system.is_some_and(|value| value.contains(MEMORY_CONTENT));
        self.observations.lock().unwrap().push(visible);
        let text = if visible {
            "MEMORY_VISIBLE"
        } else {
            "MEMORY_ABSENT"
        };
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("end_turn".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }
}

#[async_trait]
impl LlmClient for InspectingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(self.respond(system))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.respond(system);
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::TextDelta(response.text())).await;
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

fn recall_policy() -> DurableMemoryRecallPolicy {
    DurableMemoryRecallPolicy::try_new(3, 0.20)
        .unwrap()
        .try_with_related_lookups(2)
        .unwrap()
}

fn session_options(
    session_store: Arc<FileSessionStore>,
    client: Arc<InspectingClient>,
    binding: Option<DurableMemorySession>,
    env: Arc<HostEnv>,
) -> SessionOptions {
    let mut options = SessionOptions::new()
        .with_memory(Arc::new(InMemoryStore::new()))
        .with_session_store(session_store)
        .with_llm_client(client)
        .with_host_env(env)
        .with_planning_mode(PlanningMode::Disabled);
    if let Some(binding) = binding {
        options = options.with_durable_memory(binding);
    }
    options
}

#[tokio::test]
async fn file_repository_restart_requires_exact_binding_and_preserves_access_history() {
    let repository_root = tempfile::tempdir().unwrap();
    let session_root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace-a").unwrap();

    let repository = Arc::new(
        FileMemoryRepository::open(repository_root.path())
            .await
            .unwrap(),
    );
    let repository_lifetime = Arc::downgrade(&repository);
    repository
        .apply(MemoryChangeSet::new(
            "create-restart-candidate",
            namespace.clone(),
            time(18),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    NODE_ID,
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Candidate,
                    MEMORY_CONTENT,
                    vec![evidence(
                        "a3s://session/restart/source",
                        EvidenceKind::SessionTurn,
                        18,
                    )],
                    time(18),
                ),
            }],
        ))
        .await
        .unwrap();
    let binding =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), recall_policy());
    let expected_binding = binding.binding();
    let session_store = Arc::new(FileSessionStore::new(session_root.path()).await.unwrap());
    let first_client = Arc::new(InspectingClient::new());
    let first_agent = Agent::from_config(offline_config()).await.unwrap();
    let first_session = first_agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                session_options(
                    session_store.clone(),
                    first_client.clone(),
                    Some(binding.clone()),
                    host_env("before-restart", 19),
                )
                .with_session_id(SESSION_ID),
            ),
        )
        .await
        .unwrap();
    first_session.send(QUERY, None).await.unwrap();
    assert_eq!(first_client.observations(), vec![false]);
    first_session.save().await.unwrap();
    let saved = session_store
        .load_snapshot(SESSION_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.session.durable_memory_binding.as_ref(),
        Some(&expected_binding)
    );
    first_session.close().await;
    drop(first_session);
    first_agent.close().await;
    drop(first_agent);
    drop(binding);
    drop(repository);
    drop(session_store);
    assert_eq!(repository_lifetime.strong_count(), 0);

    let repository = Arc::new(
        FileMemoryRepository::open(repository_root.path())
            .await
            .unwrap(),
    );
    let binding =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), recall_policy());
    let activated = binding
        .activate_candidate(
            DurableMemoryActivation::try_new(
                "activate-after-restart",
                NODE_ID,
                1,
                evidence(
                    "a3s://verification/restart-procedure",
                    EvidenceKind::Verification,
                    20,
                ),
                time(20),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(activated.status, MemoryStatus::Active);
    assert_eq!(activated.revision, 2);

    let session_store = Arc::new(FileSessionStore::new(session_root.path()).await.unwrap());
    let second_client = Arc::new(InspectingClient::new());
    let second_agent = Agent::from_config(offline_config()).await.unwrap();
    let missing = second_agent
        .resume_session_async(
            SESSION_ID,
            session_options(
                session_store.clone(),
                second_client.clone(),
                None,
                host_env("missing-binding", 21),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        missing,
        CodeError::SessionConfiguration {
            field: "durable_memory",
            ..
        }
    ));

    let drifted_namespace = MemoryNamespace::try_new("tenant", "principal", "workspace-b").unwrap();
    let drifted = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        drifted_namespace,
        recall_policy(),
    );
    let drift = second_agent
        .resume_session_async(
            SESSION_ID,
            session_options(
                session_store.clone(),
                second_client.clone(),
                Some(drifted),
                host_env("drifted-binding", 21),
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        drift,
        CodeError::SessionConfiguration {
            field: "durable_memory",
            ..
        }
    ));

    let resumed = second_agent
        .resume_session_async(
            SESSION_ID,
            session_options(
                session_store.clone(),
                second_client.clone(),
                Some(binding.clone()),
                host_env("exact-binding", 21),
            ),
        )
        .await
        .unwrap();
    resumed.send(QUERY, None).await.unwrap();
    assert_eq!(second_client.observations(), vec![true]);
    binding
        .record_use(
            DurableMemoryUse::try_new("use-after-restart", NODE_ID, 2, time(22))
                .unwrap()
                .with_context_id("restart-turn-2"),
        )
        .await
        .unwrap();
    resumed.save().await.unwrap();
    resumed.close().await;
    drop(resumed);
    second_agent.close().await;
    drop(second_agent);
    drop(binding);
    drop(repository);
    drop(session_store);

    let reopened = FileMemoryRepository::open(repository_root.path())
        .await
        .unwrap();
    let node = reopened.get(&namespace, NODE_ID).await.unwrap().unwrap();
    assert_eq!(node.status, MemoryStatus::Active);
    assert_eq!(node.revision, 2);
    assert_eq!(node.evidence.len(), 2);
    let usage = reopened.usage_summary(&namespace, NODE_ID).await.unwrap();
    assert_eq!(usage.admissions, 1);
    assert_eq!(usage.uses, 1);

    let reopened_store = FileSessionStore::new(session_root.path()).await.unwrap();
    let saved = reopened_store
        .load_snapshot(SESSION_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.session.durable_memory_binding.as_ref(),
        Some(&expected_binding)
    );
}

#[path = "durable_memory_restart/run_identity.rs"]
mod run_identity;
