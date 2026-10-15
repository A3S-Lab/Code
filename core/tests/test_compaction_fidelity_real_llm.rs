#![cfg(test)]

//! COMPACT-FIDEL1: actual model inputs cross repeated fact-log compactions.
//! No memory extraction or commands can mask transcript fidelity failures.
//! Run with Rust1.97.1, the configured real provider, and --ignored.

mod support;

use a3s_code_core::budget::{BudgetDecision, BudgetGuard};
use a3s_code_core::fact_control::{read_workspace_facts, thread_for_session};
use a3s_code_core::harness_loop::CompletionTerminal;
use a3s_code_core::llm::{
    LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, PlanningMode, RunStatus, SessionOptions};
use a3s_memory::MemoryStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_PROVIDER_ENTRIES: usize = 20;
const MAX_GOVERNED_ENTRIES: usize = 20;
const TIMEOUT: Duration = Duration::from_secs(240);
const CODENAME: &str = "MAPLEFJORD-8812";
const CORRECTED: &str = "CEDARORBIT-4627";
const FILLER_A: &str = "The release train left the yard at dawn carrying fourteen crates of glass insulators, a spare dynamo, two accountants, and one very tired stationmaster. Summarize the shipment in one short sentence. Do not use any tools.";
const FILLER_B: &str = "A lighthouse keeper counted forty-one ships, miscounted twice, argued with the log book, and finally settled on thirty-nine. The log book won. Summarize the disagreement in one short sentence. Do not use any tools.";

#[derive(Default)]
struct RefuseCommands(AtomicUsize);
#[async_trait::async_trait]
impl a3s_code_core::sandbox::BashSandbox for RefuseCommands {
    async fn exec_command(
        &self,
        _: &str,
        _: &str,
    ) -> anyhow::Result<a3s_code_core::sandbox::SandboxOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("compaction fidelity fixture refuses commands")
    }
    async fn shutdown(&self) {}
}

#[derive(Default)]
struct Accounting {
    entries: AtomicUsize,
    usage: Mutex<Vec<TokenUsage>>,
}
#[async_trait::async_trait]
impl BudgetGuard for Accounting {
    async fn check_before_llm(&self, _: &str, estimated: usize) -> BudgetDecision {
        if self.entries.fetch_add(1, Ordering::SeqCst) >= MAX_GOVERNED_ENTRIES || estimated > 24_000
        {
            return BudgetDecision::Deny {
                resource: "compaction_fidelity_governed".into(),
                reason: "declared call/input bound".into(),
            };
        }
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.usage.lock().unwrap().push(usage.clone());
    }
    async fn check_before_tool(&self, _: &str, _: &str) -> BudgetDecision {
        BudgetDecision::Deny {
            resource: "compaction_fidelity_tools".into(),
            reason: "no tool effects admitted".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Transport {
    Complete,
    Streaming,
}
#[derive(Clone, Serialize)]
struct InputObservation {
    transport: Transport,
    messages_digest: String,
    message_payload_bytes: usize,
    system_digest: Option<String>,
    // Only role and text are exported. Private model reasoning stays omitted.
    messages: Vec<(String, String)>,
    complete_usage: Option<TokenUsage>,
}
#[derive(Clone)]
struct ObservedProvider {
    inner: Arc<dyn LlmClient>,
    inputs: Arc<Mutex<Vec<InputObservation>>>,
}

// Independently frame the published input-digest contract. Observed inputs
// are hashed before the provider adapter, without modifying any message.
fn evidence_digest(domain: &str, value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).unwrap();
    let mut h = Sha256::new();
    h.update(b"a3s-code-harness-evidence\0");
    h.update((domain.len() as u64).to_be_bytes());
    h.update(domain.as_bytes());
    h.update(&bytes);
    h.update((bytes.len() as u64).to_be_bytes());
    format!("sha256:{:x}", h.finalize())
}
impl ObservedProvider {
    fn observe(
        &self,
        transport: Transport,
        messages: &[Message],
        system: Option<&str>,
    ) -> anyhow::Result<usize> {
        let bytes = serde_json::to_vec(messages)?;
        anyhow::ensure!(bytes.len() <= 1024 * 1024, "bounded fixture input");
        let mut inputs = self.inputs.lock().unwrap();
        anyhow::ensure!(
            inputs.len() < MAX_PROVIDER_ENTRIES,
            "declared total provider entry ceiling"
        );
        let index = inputs.len();
        inputs.push(InputObservation {
            transport,
            messages_digest: evidence_digest("a3s.code.model-input-messages.v1", &messages),
            message_payload_bytes: bytes.len(),
            system_digest: system.map(|s| evidence_digest("a3s.code.model-input-system.v1", &s)),
            messages: messages
                .iter()
                .map(|m| (m.role.clone(), m.text()))
                .collect(),
            complete_usage: None,
        });
        Ok(index)
    }
    fn wrap(&self, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(Self {
            inner,
            inputs: self.inputs.clone(),
        })
    }
}
#[async_trait::async_trait]
impl LlmClient for ObservedProvider {
    fn model_generation_concurrency(&self) -> a3s_code_core::llm::ModelGenerationConcurrency {
        self.inner.model_generation_concurrency()
    }
    fn model_generation_pool(&self) -> Option<a3s_code_core::llm::ModelGenerationPool> {
        self.inner.model_generation_pool()
    }
    fn fork_for_session(&self, id: &str) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .fork_for_session(id)
            .map(|inner| self.wrap(inner))
    }
    fn with_active_generation_timeout(&self, timeout: Duration) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .with_active_generation_timeout(timeout)
            .map(|inner| self.wrap(inner))
    }
    fn with_output_cap(&self, cap: u32) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .with_output_cap(cap)
            .map(|inner| self.wrap(inner))
    }
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let index = self.observe(Transport::Complete, messages, system)?;
        let response = self.inner.complete(messages, system, tools).await?;
        self.inputs.lock().unwrap()[index].complete_usage = Some(response.usage.clone());
        Ok(response)
    }
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        self.observe(Transport::Streaming, messages, system)?;
        // Return the original provider receiver: no forwarding task or replay.
        self.inner
            .complete_streaming(messages, system, tools, cancel)
            .await
    }
}

async fn scenario(corrected: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    for directory in [".git", "personal"] {
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    let configured = support::layer_c_model::load_pinned_layer_c_config();
    let route = configured.default_model.clone().unwrap();
    let config = a3s_code_core::config::CodeConfig {
        providers: configured.providers,
        default_model: Some(route.clone()),
        user_instructions_dir: Some(root.join("personal")),
        memory: Some(a3s_code_core::memory::MemoryConfig {
            llm_extraction: false,
            prune_policy: None,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(
        config.mcp_servers.is_empty()
            && config.workspace_retrieval.is_none()
            && config.notifications.is_none()
    );
    let mut llm = config.default_llm_config().unwrap();
    llm.max_tokens = Some(4096);
    llm.api_timeout_ms = Some(120_000);
    llm.temperature = Some(0.0);
    let raw_provider = a3s_code_core::llm::create_client_with_config(llm);
    assert!(
        !raw_provider.model_generation_is_managed(),
        "observation wraps only the raw configured provider"
    );
    let observer = Arc::new(ObservedProvider {
        inner: raw_provider,
        inputs: Arc::new(Mutex::new(vec![])),
    });
    let accounting = Arc::new(Accounting::default());
    let commands = Arc::new(RefuseCommands::default());
    let memory = Arc::new(a3s_memory::InMemoryStore::new());
    let agent = Agent::from_config(config).await.unwrap();
    let session_id = format!("compact-fidelity-{corrected}");
    let mut policy = PermissionPolicy::new();
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            root.display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id(&session_id)
                    .with_llm_client(observer.clone())
                    .with_sandbox_handle(commands.clone())
                    .with_budget_guard(accounting.clone())
                    .with_permission_policy(policy)
                    .with_memory(memory.clone())
                    .with_read_only_session(true)
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_parse_retries(0)
                    .with_max_tool_rounds(2)
                    .with_llm_api_timeout(120_000)
                    .with_auto_compact(true)
                    .with_auto_compact_threshold(0.05)
                    .with_max_context_tokens(1024)
                    .with_retention_limits(
                        a3s_code_core::retention::SessionRetentionLimits::default()
                            .with_max_runs(16)
                            .with_max_events_per_run(65_536)
                            .with_max_event_bytes_per_run(16 * 1024 * 1024),
                    ),
            ),
        )
        .await
        .unwrap();
    let teach = format!("The current workspace release codename is {CODENAME}. Keep this fact for later in this conversation. Reply with exactly OK. Do not use tools.");
    let correction = format!("更正：当前工作区发布代号改为 {CORRECTED}。原代号 {CODENAME} 已撤销，之后问当前代号时只能使用新值。回复 OK，不要调用工具。");
    let mut prompts = vec![teach.clone()];
    if corrected {
        prompts.push(correction.clone());
    }
    prompts.extend([FILLER_A, FILLER_B, FILLER_A, FILLER_B].map(str::to_string));
    prompts.push("What is the current workspace release codename? Reply with exactly the codename and nothing else. Do not use any tools.".into());
    let expected = if corrected { CORRECTED } else { CODENAME };
    let mut turns = vec![];
    let mut all_events = vec![];
    for (index, prompt) in prompts.iter().enumerate() {
        let mut stream = session.stream_with_run(prompt, None).await.unwrap();
        let run = stream.run.take().expect("original Run");
        let mut end = None;
        tokio::time::timeout(TIMEOUT, async {
            while let Some(event) = stream.events.recv().await {
                if let AgentEvent::End {
                    text,
                    usage,
                    completion_terminal,
                    forced_stop,
                    ..
                } = event
                {
                    assert!(end
                        .replace((text, usage, completion_terminal, forced_stop))
                        .is_none());
                }
            }
            stream.worker.await.unwrap();
        })
        .await
        .expect("original stream timeout");
        let (text, usage, completion, forced_stop) = end.expect("one actual End");
        let run = run.snapshot().await.unwrap();
        let events = session.run_events(&run.id).await;
        let public_usage = serde_json::to_value(&usage).unwrap();
        let retained: Vec<_> = events
            .iter()
            .filter_map(|r| match &r.event {
                AgentEvent::End { usage, .. } => Some(usage),
                _ => None,
            })
            .collect();
        assert_eq!(retained.len(), 1);
        assert_eq!(serde_json::to_value(retained[0]).unwrap(), public_usage);
        turns.push(serde_json::json!({"index":index,"run_id":run.id,"status":run.status,"text":text,"usage":usage,"completion":completion,"forced_stop":forced_stop}));
        all_events.extend(
            events
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r.event,
                        AgentEvent::ReasoningDelta { .. } | AgentEvent::TextDelta { .. }
                    )
                })
                .map(|r| (run.id.clone(), r)),
        );
        let facts = read_workspace_facts(&root, &thread_for_session(&session_id)).unwrap();
        let evidence_facts: Vec<_> = facts.iter().filter(|f| f.kind != "model.turn").collect();
        let observed = serde_json::json!({"schema":"a3s.code.compaction-fidelity.evidence.v2","route":route,"corrected":corrected,
            "session_id":session_id,"last_turn":index,"final_snapshot":index+1==prompts.len(),"turns":turns,
            "provider_inputs":observer.inputs.lock().unwrap().clone(),"successful_governed_callbacks":accounting.usage.lock().unwrap().clone(),
            "governed_entries":accounting.entries.load(Ordering::SeqCst),"memory_item_count":memory.count().await.unwrap(),
            "command_invocations":commands.0.load(Ordering::SeqCst),"facts":evidence_facts,"run_events":all_events,
            "billing":"unknown","private_reasoning_omitted":true});
        let raw = serde_json::to_string(&observed).unwrap();
        assert!(raw.len() <= 2 * 1024 * 1024);
        eprintln!("compaction-fidelity evidence={raw}");
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(completion, CompletionTerminal::Narrative);
        assert!(forced_stop.is_none() && !text.trim().is_empty());
    }
    let facts = read_workspace_facts(&root, &thread_for_session(&session_id)).unwrap();
    let revision_seq = facts
        .iter()
        .find(|f| {
            f.kind == "user.message"
                && f.payload["text"].as_str()
                    == Some(if corrected {
                        correction.as_str()
                    } else {
                        teach.as_str()
                    })
        })
        .expect("original fact")
        .seq;
    let cuts: Vec<_> = facts
        .iter()
        .filter(|f| f.kind == "compaction.done" && f.seq > revision_seq)
        .collect();
    assert!(
        cuts.len() >= 2,
        "actual repeated post-teach/correction compactions"
    );
    let last = cuts.last().unwrap();
    let summary = last.payload["summary"].as_str().unwrap();
    assert!(
        summary.contains(expected),
        "current fact must reach the actual last summary"
    );
    let kept = last.payload["kept"].as_array().unwrap();
    for line in kept {
        assert_ne!(line.as_str().unwrap(), format!("user\n{teach}"));
        if corrected {
            assert_ne!(line.as_str().unwrap(), format!("user\n{correction}"));
        }
    }
    let inputs = observer.inputs.lock().unwrap().clone();
    let streaming: Vec<_> = inputs
        .iter()
        .filter(|x| matches!(x.transport, Transport::Streaming))
        .collect();
    let blocking: Vec<_> = inputs
        .iter()
        .filter(|x| matches!(x.transport, Transport::Complete))
        .collect();
    assert_eq!(streaming.len(), prompts.len());
    assert_eq!(blocking.len(), cuts.len());
    assert!(blocking.iter().all(|x| x
        .complete_usage
        .as_ref()
        .is_some_and(|u| u.prompt_tokens > 0 && u.completion_tokens > 0)));
    let final_input = streaming.last().unwrap();
    assert!(
        final_input
            .messages
            .iter()
            .any(|(_, text)| text.contains(summary)),
        "original final provider input must contain the actual summary"
    );
    assert!(
        final_input
            .messages
            .iter()
            .all(|(_, text)| text != &teach && (!corrected || text != &correction)),
        "raw teach/correction is not replayed as a separate message"
    );
    assert_eq!(
        turns.last().unwrap()["text"].as_str().unwrap().trim(),
        expected
    );
    assert_eq!(memory.count().await.unwrap(), 0);
    assert_eq!(commands.0.load(Ordering::SeqCst), 0);
    assert!(!facts
        .iter()
        .any(|f| f.kind.starts_with("tool.") || f.kind.starts_with("memory.")));
    let callbacks = accounting.usage.lock().unwrap().clone();
    assert_eq!(callbacks.len(), inputs.len());
    let mut streaming_callbacks = vec![];
    for (input, callback) in inputs.iter().zip(&callbacks) {
        if matches!(input.transport, Transport::Complete) {
            assert_eq!(
                serde_json::to_value(input.complete_usage.as_ref().unwrap()).unwrap(),
                serde_json::to_value(callback).unwrap()
            );
        } else {
            streaming_callbacks.push(callback);
        }
    }
    assert_eq!(streaming_callbacks.len(), streaming.len());
    assert_eq!(
        facts.iter().filter(|f| f.kind == "model.turn").count(),
        streaming_callbacks.len()
    );
    let mut bound = vec![];
    let mut usage_bound = vec![];
    for (run_id, record) in &all_events {
        match &record.event {
            AgentEvent::ModelInputBound { snapshot } => {
                snapshot.validate().unwrap();
                bound.push((run_id, snapshot));
            }
            AgentEvent::ModelUsageBound { snapshot } => {
                snapshot.validate().unwrap();
                usage_bound.push((run_id, snapshot));
            }
            _ => {}
        }
    }
    assert_eq!(bound.len(), streaming.len());
    assert_eq!(usage_bound.len(), streaming.len());
    for ((input, (run_id, snapshot)), (usage_run, reported)) in
        streaming.iter().zip(&bound).zip(&usage_bound)
    {
        assert_eq!(run_id, usage_run);
        assert_eq!(snapshot.messages_digest, input.messages_digest);
        assert_eq!(
            snapshot.message_payload_bytes as usize,
            input.message_payload_bytes
        );
        assert_eq!(snapshot.system_digest, input.system_digest);
        assert_eq!(reported.input_snapshot_digest, snapshot.snapshot_digest);
        assert_eq!(reported.call_sequence, snapshot.call_sequence);
        assert_eq!(
            reported.estimated_prompt_tokens,
            snapshot.estimated_prompt_tokens
        );
    }
    for (turn, callback) in turns.iter().zip(streaming_callbacks) {
        assert!(callback.prompt_tokens > 0 && callback.completion_tokens > 0);
        assert_eq!(turn["usage"]["prompt_tokens"], callback.prompt_tokens);
        assert_eq!(
            turn["usage"]["completion_tokens"],
            callback.completion_tokens
        );
        assert_eq!(turn["usage"]["total_tokens"], callback.total_tokens);
    }
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires configured real provider; repeated compaction and original inputs"]
async fn a_fact_taught_before_compaction_survives_it() {
    scenario(false).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires configured real provider; correction survives repeated compaction"]
async fn a_corrected_fact_survives_repeated_compaction_without_memory_recall() {
    scenario(true).await;
}
