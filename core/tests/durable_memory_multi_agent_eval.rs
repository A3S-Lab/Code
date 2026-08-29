use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::{
    Agent, DurableMemoryRecallPolicy, DurableMemorySession, PlanningMode, SessionOptions,
    DURABLE_MEMORY_BINDING_SCHEMA_VERSION, DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, FileMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus,
};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Barrier};
use tokio_util::sync::CancellationToken;

const FIXTURE: &str = include_str!("fixtures/durable-memory-multi-agent-v1/evaluation.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    schema_version: u32,
    binding_schema_version: u32,
    context_identity_profile: String,
    query: String,
    shared_node: Node,
    candidate_node: Node,
    foreign_node: Node,
    agents: Vec<AgentFixture>,
    thresholds: Thresholds,
}

#[derive(Debug, Deserialize)]
struct Node {
    id: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AgentFixture {
    id: String,
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct Thresholds {
    expected_model_calls: usize,
    expected_shared_admissions: u64,
    expected_candidate_admissions: u64,
    maximum_forbidden_context_hits: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    schema_version: u32,
    binding_schema_version: u32,
    context_identity_profile: &'a str,
    independent_agents: usize,
    model_calls: usize,
    colliding_local_run_ids: bool,
    shared_admissions: u64,
    candidate_admissions: u64,
    forbidden_context_hits: usize,
    replayed_admissions: u64,
}

#[derive(Debug, Clone)]
struct Observation {
    shared_visible: bool,
    forbidden_visible: bool,
}

#[derive(Clone)]
struct CoordinatedClient {
    agent_id: Arc<str>,
    query: Arc<str>,
    shared_content: Arc<str>,
    forbidden_contents: Arc<[String]>,
    first_call_barrier: Arc<Barrier>,
    first_call: Arc<AtomicBool>,
    observations: Arc<Mutex<Vec<Observation>>>,
}

impl CoordinatedClient {
    fn new(
        agent_id: &str,
        fixture: &Fixture,
        first_call_barrier: Arc<Barrier>,
        observations: Arc<Mutex<Vec<Observation>>>,
    ) -> Self {
        Self {
            agent_id: Arc::from(agent_id),
            query: Arc::from(fixture.query.as_str()),
            shared_content: Arc::from(fixture.shared_node.content.as_str()),
            forbidden_contents: Arc::from([
                fixture.candidate_node.content.clone(),
                fixture.foreign_node.content.clone(),
            ]),
            first_call_barrier,
            first_call: Arc::new(AtomicBool::new(true)),
            observations,
        }
    }

    async fn respond(
        &self,
        messages: &[Message],
        system: Option<&str>,
    ) -> anyhow::Result<LlmResponse> {
        if self.first_call.swap(false, Ordering::AcqRel) {
            self.first_call_barrier.wait().await;
        }
        let query = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(Message::text)
            .ok_or_else(|| anyhow::anyhow!("multi-agent evaluation has no user query"))?;
        anyhow::ensure!(query == self.query.as_ref(), "unexpected evaluation query");
        let system = system.unwrap_or_default();
        let shared_visible = system.contains(self.shared_content.as_ref());
        let forbidden_visible = self
            .forbidden_contents
            .iter()
            .any(|content| system.contains(content));
        self.observations.lock().unwrap().push(Observation {
            shared_visible,
            forbidden_visible,
        });
        Ok(response(if shared_visible && !forbidden_visible {
            format!("PASS:{}", self.agent_id)
        } else {
            format!("MISS:{}", self.agent_id)
        }))
    }
}

#[async_trait]
impl LlmClient for CoordinatedClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.respond(messages, system).await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.respond(messages, system).await?;
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::TextDelta(response.text())).await;
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

fn response(text: String) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text { text }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, 21, 0, 0)
        .single()
        .expect("valid multi-agent fixture time")
}

fn evidence(node: &Node, kind: EvidenceKind) -> EvidenceRef {
    EvidenceRef::try_new(
        format!("a3s://fixture/durable-memory-multi-agent-v1/{}", node.id),
        format!("sha256:{:x}", Sha256::digest(node.content.as_bytes())),
        kind,
        fixture_time(),
    )
    .expect("valid multi-agent fixture evidence")
}

fn draft(node: &Node, namespace: &MemoryNamespace, status: MemoryStatus) -> MemoryNodeDraft {
    let evidence_kind = if status == MemoryStatus::Active {
        EvidenceKind::Verification
    } else {
        EvidenceKind::SessionTurn
    };
    MemoryNodeDraft::new(
        &node.id,
        namespace.clone(),
        DurableMemoryKind::Procedural,
        status,
        &node.content,
        vec![evidence(node, evidence_kind)],
        fixture_time(),
    )
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

fn colliding_host_env() -> Arc<HostEnv> {
    Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new("shared-local-id")),
        Arc::new(FixedClock::new(
            u64::try_from(fixture_time().timestamp_millis()).unwrap(),
        )),
    ))
}

fn options(
    agent: &AgentFixture,
    durable: DurableMemorySession,
    client: Arc<CoordinatedClient>,
) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(&agent.session_id)
        .with_planning_mode(PlanningMode::Disabled)
        .with_memory(Arc::new(InMemoryStore::new()))
        .with_durable_memory(durable)
        .with_host_env(colliding_host_env())
        .with_llm_client(client)
}

#[tokio::test]
async fn independent_agents_share_exact_memory_without_admission_collisions() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid multi-agent fixture");
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(
        fixture.binding_schema_version,
        DURABLE_MEMORY_BINDING_SCHEMA_VERSION
    );
    assert_eq!(fixture.agents.len(), 2);
    assert_eq!(
        fixture.context_identity_profile,
        DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1
    );

    let repository_root = tempfile::tempdir().expect("create memory repository root");
    let workspace = tempfile::tempdir().expect("create shared workspace");
    let repository = Arc::new(
        FileMemoryRepository::open(repository_root.path())
            .await
            .expect("open memory repository"),
    );
    let namespace =
        MemoryNamespace::try_new("atlas-tenant", "atlas-team", "shared-workspace").unwrap();
    let foreign_namespace =
        MemoryNamespace::try_new("atlas-tenant", "foreign-team", "shared-workspace").unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-multi-agent-v1",
            namespace.clone(),
            fixture_time(),
            vec![
                MemoryOperation::Create {
                    node: draft(&fixture.shared_node, &namespace, MemoryStatus::Active),
                },
                MemoryOperation::Create {
                    node: draft(&fixture.candidate_node, &namespace, MemoryStatus::Candidate),
                },
            ],
        ))
        .await
        .expect("seed shared namespace");
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-multi-agent-foreign-v1",
            foreign_namespace.clone(),
            fixture_time(),
            vec![MemoryOperation::Create {
                node: draft(
                    &fixture.foreign_node,
                    &foreign_namespace,
                    MemoryStatus::Active,
                ),
            }],
        ))
        .await
        .expect("seed foreign namespace");

    let policy = DurableMemoryRecallPolicy::try_new(3, 0.20).unwrap();
    let durable =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), policy);
    let binding = durable.binding();
    assert_eq!(binding.schema_version(), fixture.binding_schema_version);
    assert_eq!(
        binding.context_id_profile(),
        fixture.context_identity_profile
    );
    let barrier = Arc::new(Barrier::new(2));
    let observations = Arc::new(Mutex::new(Vec::new()));
    let alpha_client = Arc::new(CoordinatedClient::new(
        &fixture.agents[0].id,
        &fixture,
        barrier.clone(),
        observations.clone(),
    ));
    let beta_client = Arc::new(CoordinatedClient::new(
        &fixture.agents[1].id,
        &fixture,
        barrier,
        observations.clone(),
    ));
    let alpha_agent = Agent::from_config(offline_config()).await.unwrap();
    let beta_agent = Agent::from_config(offline_config()).await.unwrap();
    let alpha = alpha_agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options(&fixture.agents[0], durable.clone(), alpha_client)),
        )
        .await
        .unwrap();
    let beta = beta_agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options(&fixture.agents[1], durable.clone(), beta_client)),
        )
        .await
        .unwrap();

    let concurrent = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            alpha.send(&fixture.query, None),
            beta.send(&fixture.query, None)
        )
    })
    .await
    .expect("independent agents reached the model concurrently");
    assert_eq!(concurrent.0.unwrap().text, "PASS:alpha");
    assert_eq!(concurrent.1.unwrap().text, "PASS:beta");
    let alpha_first_runs = alpha.runs().await;
    let beta_first_runs = beta.runs().await;
    assert_eq!(alpha_first_runs.len(), 1);
    assert_eq!(beta_first_runs.len(), 1);
    let colliding_local_run_ids = alpha_first_runs[0].id == beta_first_runs[0].id;
    assert!(colliding_local_run_ids);

    beta.close().await;
    drop(beta);
    beta_agent.close().await;
    drop(beta_agent);
    assert_eq!(
        alpha.send(&fixture.query, None).await.unwrap().text,
        "PASS:alpha"
    );

    let observations = observations.lock().unwrap().clone();
    assert_eq!(observations.len(), fixture.thresholds.expected_model_calls);
    assert!(observations.iter().all(|item| item.shared_visible));
    assert!(
        observations
            .iter()
            .filter(|item| item.forbidden_visible)
            .count()
            <= fixture.thresholds.maximum_forbidden_context_hits
    );
    let shared_admissions = repository
        .usage_summary(&namespace, &fixture.shared_node.id)
        .await
        .unwrap()
        .admissions;
    assert_eq!(
        shared_admissions, fixture.thresholds.expected_shared_admissions,
        "each session/run context must have one independent admission"
    );
    let candidate_admissions = repository
        .usage_summary(&namespace, &fixture.candidate_node.id)
        .await
        .unwrap()
        .admissions;
    assert_eq!(
        candidate_admissions,
        fixture.thresholds.expected_candidate_admissions
    );
    let forbidden_context_hits = observations
        .iter()
        .filter(|item| item.forbidden_visible)
        .count();

    alpha.close().await;
    drop(alpha);
    alpha_agent.close().await;
    drop(alpha_agent);
    drop(durable);
    drop(repository);

    let reopened = FileMemoryRepository::open(repository_root.path())
        .await
        .expect("reopen shared memory repository");
    let replayed_admissions = reopened
        .usage_summary(&namespace, &fixture.shared_node.id)
        .await
        .unwrap()
        .admissions;
    assert_eq!(
        replayed_admissions, fixture.thresholds.expected_shared_admissions,
        "multi-agent admissions must survive journal replay"
    );
    assert_eq!(
        reopened
            .usage_summary(&foreign_namespace, &fixture.foreign_node.id)
            .await
            .unwrap()
            .admissions,
        0,
        "an exact shared binding must not admit foreign-principal memory"
    );
    println!(
        "A3S_DURABLE_MEMORY_MULTI_AGENT_EVAL={}",
        serde_json::to_string(&Report {
            schema_version: fixture.schema_version,
            binding_schema_version: DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
            context_identity_profile: DURABLE_MEMORY_CONTEXT_ID_PROFILE_V1,
            independent_agents: fixture.agents.len(),
            model_calls: observations.len(),
            colliding_local_run_ids,
            shared_admissions,
            candidate_admissions,
            forbidden_context_hits,
            replayed_admissions,
        })
        .expect("serialize multi-agent report")
    );
}
