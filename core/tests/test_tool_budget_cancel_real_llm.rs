#![cfg(test)]

//! CANCEL-TOOL-BUDGET1: original guard cancellation and same-session recovery.
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

const MAX_PROVIDER_ENTRIES: usize = 3;
const MAX_GOVERNED_ENTRIES: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(240);
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
    tool_checks: AtomicUsize,
    active: AtomicUsize,
    dropped: AtomicUsize,
    completed: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
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
    async fn check_before_tool(&self, _: &str, tool: &str) -> BudgetDecision {
        if tool != "read" || self.tool_checks.fetch_add(1, Ordering::SeqCst) > 0 {
            return BudgetDecision::Deny {
                resource: "tool_cancel_fixture".into(),
                reason: "only the declared original read is admitted".into(),
            };
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        let _original = OriginalBudgetFuture(self);
        self.entered.notify_one();
        self.release.notified().await;
        self.completed.fetch_add(1, Ordering::SeqCst);
        BudgetDecision::Allow
    }
}

struct OriginalBudgetFuture<'a>(&'a Accounting);
impl Drop for OriginalBudgetFuture<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
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
    // Export roles, text and public tool blocks. Omit private model reasoning.
    messages: Vec<(String, String)>,
    complete_usage: Option<TokenUsage>,
    tool_calls: Vec<serde_json::Value>,
    tool_results: Vec<serde_json::Value>,
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
            tool_calls: messages
                .iter()
                .flat_map(|m| m.tool_calls())
                .map(|c| serde_json::json!({"id":c.id,"name":c.name,"args":c.args}))
                .collect(),
            tool_results: messages
                .iter()
                .flat_map(|m| &m.content)
                .filter_map(|block| match block {
                    a3s_code_core::llm::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } => Some(
                        serde_json::json!({"id":tool_use_id,"content":content,"is_error":is_error}),
                    ),
                    _ => None,
                })
                .collect(),
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

fn public_event(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::ReasoningDelta { .. } | AgentEvent::TextDelta { .. }
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the configured real LLM provider"]
async fn original_guard_cancel_stops_tool_effects_and_allows_a_new_run() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    for directory in [".git", "personal"] {
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    let original_file = "This file must not be read by the cancelled operation.\n";
    std::fs::write(root.join("note.txt"), original_file).unwrap();
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
    let raw = a3s_code_core::llm::create_client_with_config(llm);
    assert!(!raw.model_generation_is_managed());
    let observer = Arc::new(ObservedProvider {
        inner: raw,
        inputs: Arc::new(Mutex::new(Vec::new())),
    });
    let accounting = Arc::new(Accounting::default());
    let commands = Arc::new(RefuseCommands::default());
    let memory = Arc::new(a3s_memory::InMemoryStore::new());
    let agent = Agent::from_config(config).await.unwrap();
    let session_id = "tool-budget-cancel-live";
    let mut policy = PermissionPolicy::new().allow("read(note.txt)");
    policy.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            root.display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id(session_id)
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
                    .with_auto_compact(false)
                    .with_max_context_tokens(4096),
            ),
        )
        .await
        .unwrap();
    let mut first=session.stream_with_run("Read note.txt exactly once using read with {\"file_path\":\"note.txt\"}, then reply OK. Do not use any other tools.",None).await.unwrap();
    let original = first.run.take().unwrap();
    let original_id = original.snapshot().await.unwrap().id;
    let mut public = Vec::new();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            tokio::select! {
                biased;
                _=accounting.entered.notified()=>break,
                event=first.events.recv()=> {
                    let event=event.expect("original stream ended before the tool budget was entered");
                    if public_event(&event) { public.push(event); }
                }
            }
        }
    }).await.expect("original budget entry timeout");
    assert_eq!(accounting.active.load(Ordering::SeqCst), 1);
    let accepted = session.cancel().await;
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = first.events.recv().await {
            if public_event(&event) {
                public.push(event);
            }
        }
        (&mut first.worker).await.unwrap();
    })
    .await;
    let timely = observed.is_ok();
    if !timely {
        // Diagnose and drain this exact worker; never restart it or call an
        // outer abort a successful cooperative cancellation.
        accounting.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = first.events.recv().await {
                if public_event(&event) {
                    public.push(event);
                }
            }
            (&mut first.worker).await.unwrap();
        })
        .await
        .expect("diagnostic original-worker drain");
    }
    let first_snapshot = original.snapshot().await.unwrap();
    let original_public_event_count = public.len();
    let mut run_ids = vec![original_id.clone()];
    let mut follow_up = None;
    if timely {
        assert!(
            session
                .cancel_and_settle(Duration::from_millis(100), Duration::ZERO)
                .await
        );
        let mut next=session.stream_with_run("The previous read was cancelled. Do not retry it or use any tools. Reply with exactly OK.",None).await.unwrap();
        let next_run = next.run.take().unwrap();
        let mut next_end = None;
        tokio::time::timeout(TIMEOUT, async {
            while let Some(event) = next.events.recv().await {
                if let AgentEvent::End {
                    text,
                    usage,
                    completion_terminal,
                    forced_stop,
                    ..
                } = &event
                {
                    assert!(next_end
                        .replace((
                            text.clone(),
                            usage.clone(),
                            completion_terminal.clone(),
                            *forced_stop
                        ))
                        .is_none());
                }
                if public_event(&event) {
                    public.push(event);
                }
            }
            next.worker.await.unwrap();
        })
        .await
        .unwrap();
        let snapshot = next_run.snapshot().await.unwrap();
        run_ids.push(snapshot.id.clone());
        follow_up = Some((snapshot, next_end));
    }
    let facts = read_workspace_facts(&root, &thread_for_session(session_id)).unwrap();
    let evidence_facts: Vec<_> = facts.iter().filter(|f| f.kind != "model.turn").collect();
    let original_calls:Vec<_>=facts.iter().filter(|f|f.kind=="model.turn" && f.payload["kind"]=="tool")
        .flat_map(|f|f.payload.get("call").into_iter().chain(f.payload.get("also").and_then(|v|v.as_array()).into_iter().flatten())
            .map(|c|serde_json::json!({"seq":f.seq,"id":c["id"],"name":c["name"],"args":c["args"]}))).collect();
    let mut events = Vec::new();
    for id in &run_ids {
        events.extend(
            session
                .run_events(id)
                .await
                .into_iter()
                .filter(|r| public_event(&r.event))
                .map(|r| (id.clone(), r)),
        );
    }
    let inputs = observer.inputs.lock().unwrap().clone();
    let callbacks = accounting.usage.lock().unwrap().clone();
    let witness = serde_json::json!({"schema":"a3s.code.tool-budget-cancel.evidence.v1","route":route,"session_id":session_id,
        "final_snapshot":true,"original_run":first_snapshot,"follow_up":follow_up,"cancel_accepted":accepted,"cooperative_worker_drained":timely,
        "guard_entries":accounting.tool_checks.load(Ordering::SeqCst),"guard_active":accounting.active.load(Ordering::SeqCst),
        "guard_dropped":accounting.dropped.load(Ordering::SeqCst),"guard_completed":accounting.completed.load(Ordering::SeqCst),
        "provider_inputs":inputs,"successful_governed_callbacks":callbacks,"governed_entries":accounting.entries.load(Ordering::SeqCst),
        "command_invocations":commands.0.load(Ordering::SeqCst),"memory_item_count":memory.count().await.unwrap(),
        "original_calls":original_calls,"facts":evidence_facts,"run_events":events,"public_events":public,
        "original_public_event_count":original_public_event_count,
        "billing":"unknown","private_reasoning_omitted":true});
    let raw = serde_json::to_string(&witness).unwrap();
    assert!(raw.len() <= 2 * 1024 * 1024);
    eprintln!("tool-budget-cancel evidence={raw}");
    assert!(
        accepted && timely,
        "original cancellation must cooperatively drain without releasing the host guard"
    );
    assert_eq!(first_snapshot.status, RunStatus::Cancelled);
    let original_public = &public[..original_public_event_count];
    assert!(!original_public
        .iter()
        .any(|event| matches!(event, AgentEvent::End { .. })));
    let original_errors: Vec<_> = original_public
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Error { message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        original_errors,
        [&format!("Run '{original_id}' was cancelled")]
    );
    assert_eq!(
        original.snapshot().await.unwrap().status,
        RunStatus::Cancelled
    );
    assert_eq!(accounting.tool_checks.load(Ordering::SeqCst), 1);
    assert_eq!(accounting.active.load(Ordering::SeqCst), 0);
    assert_eq!(accounting.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(accounting.completed.load(Ordering::SeqCst), 0);
    assert_eq!(commands.0.load(Ordering::SeqCst), 0);
    assert_eq!(memory.count().await.unwrap(), 0);
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        original_file
    );
    assert!(events
        .iter()
        .all(|(_, r)| !matches!(r.event, AgentEvent::ToolExecutionStart { .. })));
    assert!(!events
        .iter()
        .any(|(id, r)| id == &original_id && matches!(r.event, AgentEvent::End { .. })));
    assert_eq!(original_calls.len(), 1);
    let prepared: Vec<_> = facts.iter().filter(|f| f.kind == "tool.prepared").collect();
    let results: Vec<_> = facts.iter().filter(|f| f.kind == "tool.result").collect();
    assert_eq!(prepared.len(), 1);
    assert_eq!(results.len(), 1);
    let p: a3s_effect::ToolPreparedV1 =
        serde_json::from_value(prepared[0].payload["prepared"].clone()).unwrap();
    let result: a3s_effect::ToolExecutionResultV1 =
        serde_json::from_value(results[0].payload["receipt"].clone()).unwrap();
    p.validate().unwrap();
    result.ensure_prepared(&p).unwrap();
    assert_eq!(result.outcome, a3s_effect::ToolExecutionOutcome::Cancelled);
    assert_eq!(
        result.effects.completeness,
        a3s_effect::ToolObservationCompleteness::Complete
    );
    assert!(result.effects.changes.is_empty());
    assert_eq!(
        p.identity.tool_call_id,
        original_calls[0]["id"].as_str().unwrap()
    );
    assert_eq!(
        p.original_args_digest,
        a3s_effect::tool_args_digest(&original_calls[0]["args"]).unwrap()
    );
    assert_eq!(p.identity.admission.as_ref().unwrap().run_id, original_id);
    assert_eq!(
        p.identity.admission.as_ref().unwrap().thread_id,
        thread_for_session(session_id)
    );
    let (next, next_end) = follow_up.unwrap();
    let (text, usage, completion, forced) = next_end.unwrap();
    assert_ne!(next.id, original_id);
    assert_eq!(next.status, RunStatus::Completed);
    assert_eq!(text.trim(), "OK");
    assert_eq!(completion, CompletionTerminal::Narrative);
    assert!(forced.is_none());
    let retained_end: Vec<_> = events
        .iter()
        .filter_map(|(id, r)| match &r.event {
            AgentEvent::End { usage, .. } if id == &next.id => Some(usage),
            _ => None,
        })
        .collect();
    assert_eq!(retained_end.len(), 1);
    assert_eq!(
        serde_json::to_value(retained_end[0]).unwrap(),
        serde_json::to_value(&usage).unwrap()
    );
    assert_eq!(inputs.len(), 2);
    assert_eq!(callbacks.len(), 2);
    let bound: Vec<_> = events
        .iter()
        .filter_map(|(id, r)| match &r.event {
            AgentEvent::ModelInputBound { snapshot } => Some((id, snapshot)),
            _ => None,
        })
        .collect();
    let usage_bound: Vec<_> = events
        .iter()
        .filter_map(|(id, r)| match &r.event {
            AgentEvent::ModelUsageBound { snapshot } => Some((id, snapshot)),
            _ => None,
        })
        .collect();
    assert_eq!(bound.len(), 2);
    assert_eq!(usage_bound.len(), 2);
    for ((input, (run, snapshot)), (usage_run, reported)) in
        inputs.iter().zip(&bound).zip(&usage_bound)
    {
        snapshot.validate().unwrap();
        reported.validate().unwrap();
        assert_eq!(run, usage_run);
        assert_eq!(input.messages_digest, snapshot.messages_digest);
        assert_eq!(input.system_digest, snapshot.system_digest);
        assert_eq!(
            input.message_payload_bytes,
            snapshot.message_payload_bytes as usize
        );
        assert_eq!(reported.input_snapshot_digest, snapshot.snapshot_digest);
        assert_eq!(reported.call_sequence, snapshot.call_sequence);
    }
    assert!(callbacks
        .iter()
        .all(|u| u.prompt_tokens > 0 && u.completion_tokens > 0));
    assert_eq!(
        serde_json::to_value(&usage).unwrap(),
        serde_json::to_value(&callbacks[1]).unwrap()
    );
    assert!(inputs[1]
        .tool_calls
        .iter()
        .any(|c| c["id"] == original_calls[0]["id"]
            && c["name"] == original_calls[0]["name"]
            && c["args"] == original_calls[0]["args"]));
    assert!(inputs[1]
        .tool_results
        .iter()
        .any(|r| r["id"] == original_calls[0]["id"]));
    session.close().await;
    agent.close().await;
}
