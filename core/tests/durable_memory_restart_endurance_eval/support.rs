use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::retention::SessionRetentionLimits;
use a3s_code_core::store::FileSessionStore;
use a3s_code_core::{DurableMemorySession, PlanningMode, SessionOptions};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Barrier};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
pub(crate) struct Fixture {
    pub(crate) schema_version: u32,
    pub(crate) binding_schema_version: u32,
    pub(crate) context_identity_profile: String,
    pub(crate) epochs: usize,
    pub(crate) turns_per_agent_per_epoch: usize,
    pub(crate) max_runs_retained: usize,
    pub(crate) query: String,
    pub(crate) active_node: VersionedNode,
    pub(crate) candidate_node: Node,
    pub(crate) foreign_node: Node,
    pub(crate) agents: Vec<AgentFixture>,
    pub(crate) thresholds: Thresholds,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VersionedNode {
    pub(crate) id: String,
    pub(crate) revisions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Node {
    pub(crate) id: String,
    pub(crate) content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentFixture {
    pub(crate) id: String,
    pub(crate) session_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Thresholds {
    pub(crate) expected_model_calls: usize,
    pub(crate) expected_admissions: u64,
    pub(crate) expected_candidate_admissions: u64,
    pub(crate) maximum_forbidden_context_hits: usize,
    pub(crate) expected_repository_opens: usize,
    pub(crate) expected_session_resumes: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub(crate) current_visible: bool,
    pub(crate) forbidden_visible: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report<'a> {
    pub(crate) schema_version: u32,
    pub(crate) binding_schema_version: u32,
    pub(crate) context_identity_profile: &'a str,
    pub(crate) epochs: usize,
    pub(crate) independent_agents_per_epoch: usize,
    pub(crate) model_calls: usize,
    pub(crate) admissions: u64,
    pub(crate) candidate_admissions: u64,
    pub(crate) forbidden_context_hits: usize,
    pub(crate) repository_opens: usize,
    pub(crate) session_resumes: usize,
    pub(crate) reused_retained_run_ids: bool,
    pub(crate) final_node_revision: u64,
}

#[derive(Clone)]
pub(crate) struct EpochClient {
    agent_id: Arc<str>,
    epoch: usize,
    query: Arc<str>,
    current_content: Arc<str>,
    forbidden_contents: Arc<[String]>,
    barrier: Arc<Barrier>,
    barrier_calls: usize,
    calls: Arc<AtomicUsize>,
    observations: Arc<Mutex<Vec<Observation>>>,
}

impl EpochClient {
    pub(crate) fn new(
        agent_id: &str,
        epoch: usize,
        fixture: &Fixture,
        current_content: &str,
        barrier: Arc<Barrier>,
        observations: Arc<Mutex<Vec<Observation>>>,
    ) -> Self {
        let mut forbidden_contents = fixture.active_node.revisions.clone();
        forbidden_contents.retain(|content| content != current_content);
        forbidden_contents.push(fixture.candidate_node.content.clone());
        forbidden_contents.push(fixture.foreign_node.content.clone());
        Self {
            agent_id: Arc::from(agent_id),
            epoch,
            query: Arc::from(fixture.query.as_str()),
            current_content: Arc::from(current_content),
            forbidden_contents: Arc::from(forbidden_contents),
            barrier,
            barrier_calls: fixture.turns_per_agent_per_epoch,
            calls: Arc::new(AtomicUsize::new(0)),
            observations,
        }
    }

    async fn respond(
        &self,
        messages: &[Message],
        system: Option<&str>,
    ) -> anyhow::Result<LlmResponse> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        if call < self.barrier_calls {
            self.barrier.wait().await;
        }
        let query = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(Message::text)
            .ok_or_else(|| anyhow::anyhow!("restart endurance evaluation has no user query"))?;
        anyhow::ensure!(query == self.query.as_ref(), "unexpected evaluation query");
        let system = system.unwrap_or_default();
        let current_visible = system.contains(self.current_content.as_ref());
        let forbidden_visible = self
            .forbidden_contents
            .iter()
            .any(|content| system.contains(content));
        self.observations.lock().unwrap().push(Observation {
            current_visible,
            forbidden_visible,
        });
        Ok(response(if current_visible && !forbidden_visible {
            format!("PASS:{}:epoch{}", self.agent_id, self.epoch)
        } else {
            format!("MISS:{}:epoch{}", self.agent_id, self.epoch)
        }))
    }
}

#[async_trait]
impl LlmClient for EpochClient {
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

pub(crate) fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, 22, 0, 0)
        .single()
        .expect("valid restart endurance fixture time")
}

pub(crate) fn offline_config() -> CodeConfig {
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

pub(crate) fn session_options(
    agent: &AgentFixture,
    fixture: &Fixture,
    store: Arc<FileSessionStore>,
    durable: DurableMemorySession,
    client: Arc<EpochClient>,
) -> SessionOptions {
    let timestamp = u64::try_from(fixture_time().timestamp_millis()).unwrap();
    SessionOptions::new()
        .with_planning_mode(PlanningMode::Disabled)
        .with_memory(Arc::new(InMemoryStore::new()))
        .with_session_store(store)
        .with_auto_save(false)
        .with_retention_limits(
            SessionRetentionLimits::new().with_max_runs(fixture.max_runs_retained),
        )
        .with_durable_memory(durable)
        .with_host_env(Arc::new(HostEnv::new(
            Arc::new(SequentialIdGenerator::new(format!("restart-{}", agent.id))),
            Arc::new(FixedClock::new(timestamp)),
        )))
        .with_llm_client(client)
}
