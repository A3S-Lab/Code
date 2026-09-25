//! Fact-log control for a coding session.
//!
//! The next transition comes from [`a3s_effect::resume_coding`]. This module
//! does not keep a message loop, a confirmation timer, or a question wait
//! table. Steer is another `user.message` fact.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a3s_effect::{
    answer_fact, confirm_fact, ingest_coding, message_fact, resume_coding, ActorError, CodingPhase,
    CodingServices, CodingView, Compactor, Completion, CompletionRequest, Exit, FileLog,
    HarnessConfig, HarnessGraph, LogStore, ModelDecision, NewFact, ToolCall, ToolRunner, ToolSpec,
};
use anyhow::Result;

use crate::ask_user::{self, AskUserError};
use crate::llm::{LlmClient, LlmResponse, Message, ToolDefinition};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::queue::SessionLane;
use crate::tools::ToolExecutor;

const THREAD: &str = "thread-1";

/// Thread id for one session. Valid ids are kept so two sessions in one
/// workspace fold two logs.
pub fn thread_for_session(session_id: &str) -> String {
    let valid = !session_id.is_empty()
        && session_id.len() <= 128
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'));
    if valid {
        return session_id.to_string();
    }
    let hex: String = session_id.bytes().fold(String::new(), |mut hex, byte| {
        hex.push_str(&format!("{byte:02x}"));
        hex
    });
    let mut thread = format!("s-{hex}");
    thread.truncate(128);
    thread
}

/// Whether a tool call parks for confirmation.
///
/// `PermissionPolicy` is the only Allow/Deny/Ask decision. A YOLO lane is
/// recorded as Allow before this is consulted, so it does not flip Ask.
pub fn confirmation_required(policy: &PermissionPolicy, tool_name: &str) -> bool {
    policy.check(tool_name, &serde_json::json!({})) == PermissionDecision::Ask
}

pub struct FactRun {
    dir: PathBuf,
    thread: String,
    actor: a3s_effect::Actor<CodingServices, CodingView>,
    services: Arc<CodingServices>,
    limit: u32,
}

struct CappedCompletion {
    inner: Arc<dyn Completion>,
    run_id: String,
}

impl Completion for CappedCompletion {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        let inner = Arc::clone(&self.inner);
        let run_id = self.run_id.clone();
        Box::pin(async move {
            let decision = inner.complete(request).await?;
            if let ModelDecision::Question { .. } = &decision {
                if let Err(error) = ask_user::begin(&run_id, "q", "cap-check", &[], false) {
                    if matches!(error, AskUserError::CapExceeded) {
                        return Err(ActorError::Defect("ask_user question cap exceeded".into()));
                    }
                }
            }
            Ok(decision)
        })
    }
}

/// Pinned coding runtime used while the fact log chooses transitions.
///
/// Hooks, context, and delegation planning come from the admitted run. They
/// do not keep a message loop.
#[derive(Clone)]
pub(crate) struct SessionSurface {
    pub(crate) agent: crate::agent::AgentLoop,
    pub(crate) session_id: String,
    pub(crate) checkpoint: Option<CheckpointEmit>,
    pub(crate) events: Option<tokio::sync::mpsc::Sender<crate::agent::AgentEvent>>,
    pub(crate) cancel: tokio_util::sync::CancellationToken,
    pub(crate) transcript: Arc<Mutex<Vec<Message>>>,
    pub(crate) usage: Arc<Mutex<crate::llm::TokenUsage>>,
    pub(crate) confirmation: Option<Arc<dyn crate::hitl::ConfirmationProvider>>,
    pub(crate) run_store: Option<Arc<crate::run::InMemoryRunStore>>,
    pub(crate) run_id: Option<String>,
    pub(crate) ledger: Arc<Mutex<crate::harness_loop::MutationLedger>>,
    pub(crate) reports: Arc<Mutex<Vec<crate::verification::VerificationReport>>>,
    pub(crate) run_control: Option<Arc<crate::run_control::RunControlInbox>>,
    pub(crate) harness: Option<crate::meta_harness::HarnessComposeOptions>,
}

/// Exports one portable checkpoint after a tool result lands on the log.
#[derive(Clone)]
pub(crate) struct CheckpointEmit {
    pub(crate) sink: Arc<dyn crate::loop_checkpoint::LoopCheckpointSink>,
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) capability_binding: Option<crate::capability::RunCapabilityBindingV1>,
}

pub struct LiveCompletion {
    client: Arc<dyn LlmClient>,
    calls: Arc<AtomicUsize>,
    policy: PermissionPolicy,
    catalog: Vec<ToolDefinition>,
    surface: Option<SessionSurface>,
    context_ready: Arc<AtomicBool>,
    cached_system: Arc<Mutex<Option<String>>>,
    cached_prompt: Arc<Mutex<Option<String>>>,
}

impl LiveCompletion {
    pub fn new(
        client: Arc<dyn LlmClient>,
        policy: PermissionPolicy,
        catalog: Vec<ToolDefinition>,
    ) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                client,
                calls: Arc::clone(&calls),
                policy,
                catalog,
                surface: None,
                context_ready: Arc::new(AtomicBool::new(false)),
                cached_system: Arc::new(Mutex::new(None)),
                cached_prompt: Arc::new(Mutex::new(None)),
            },
            calls,
        )
    }

    pub(crate) fn with_surface(mut self, surface: SessionSurface) -> Self {
        self.surface = Some(surface);
        self
    }
}

fn interrupted_response() -> crate::llm::LlmResponse {
    crate::llm::LlmResponse {
        message: Message::assistant("(Response interrupted by the user.)"),
        usage: crate::llm::TokenUsage::default(),
        stop_reason: Some("cancelled".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

async fn complete_detached(
    client: Arc<dyn LlmClient>,
    messages: &[Message],
    system: Option<&str>,
    tools: &[ToolDefinition],
) -> Result<crate::llm::LlmResponse, ActorError> {
    let cancel = tokio_util::sync::CancellationToken::new();
    match client
        .complete_streaming(messages, system, tools, cancel)
        .await
    {
        Ok(mut events) => {
            let mut done = None;
            while let Some(event) = events.recv().await {
                if let crate::llm::StreamEvent::Done(response) = event {
                    done = Some(response);
                }
            }
            if let Some(response) = done {
                return Ok(response);
            }
            client
                .complete(messages, system, tools)
                .await
                .map_err(model_error)
        }
        Err(_) => client
            .complete(messages, system, tools)
            .await
            .map_err(model_error),
    }
}

fn model_error(error: anyhow::Error) -> ActorError {
    if let Some(message) = crate::llm::non_retryable_llm_error_message(&error) {
        return ActorError::Defect(message.to_string());
    }
    ActorError::Handler {
        key: "infer".into(),
        message: error.to_string(),
    }
}

async fn model_response(
    client: Arc<dyn LlmClient>,
    messages: Vec<Message>,
    system: Option<String>,
    tools: Vec<ToolDefinition>,
    surface: Option<SessionSurface>,
) -> Result<crate::llm::LlmResponse, ActorError> {
    let Some(surface) = surface else {
        return complete_detached(client, &messages, system.as_deref(), &tools).await;
    };
    surface
        .agent
        .fact_budget_gate(&surface.session_id, &surface.cancel)
        .await
        .map_err(|error| ActorError::Defect(error.to_string()))?;
    if surface.cancel.is_cancelled() {
        return Ok(interrupted_response());
    }
    // The provider read runs on its own task. A stalled socket poll must not
    // pin the deadline to that worker: aborting the task is what lets the
    // fold retry the attempt. Host cancellation aborts that task immediately
    // and is not a retryable provider failure.
    let timeout = surface.agent.llm_api_timeout();
    let cancel = surface.cancel.clone();
    let attempt = surface.cancel.child_token();
    let attempt_for_task = attempt.clone();
    let mut handle = tokio::spawn(async move {
        read_model_response(
            client.as_ref(),
            &messages,
            system.as_deref(),
            &tools,
            &surface,
            &attempt_for_task,
        )
        .await
    });
    let Some(timeout) = timeout else {
        return await_model_task(handle, &cancel).await;
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            handle.abort();
            let _ = handle.await;
            Ok(interrupted_response())
        }
        joined = tokio::time::timeout(timeout, &mut handle) => match joined {
            Ok(joined) => flatten_model_task(joined),
            Err(_) => {
                handle.abort();
                attempt.cancel();
                Err(ActorError::Handler {
                    key: "infer".into(),
                    message: format!("LLM call timed out after {} ms", timeout.as_millis()),
                })
            }
        },
    }
}

async fn await_model_task(
    mut handle: tokio::task::JoinHandle<Result<crate::llm::LlmResponse, ActorError>>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::llm::LlmResponse, ActorError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            handle.abort();
            let _ = handle.await;
            Ok(interrupted_response())
        }
        joined = &mut handle => flatten_model_task(joined),
    }
}

fn flatten_model_task(
    joined: Result<Result<crate::llm::LlmResponse, ActorError>, tokio::task::JoinError>,
) -> Result<crate::llm::LlmResponse, ActorError> {
    match joined {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Err(ActorError::Handler {
            key: "infer".into(),
            message: "LLM call timed out".into(),
        }),
        Err(error) => Err(ActorError::Defect(format!("model task failed: {error}"))),
    }
}

async fn read_model_response(
    client: &dyn LlmClient,
    messages: &[Message],
    system: Option<&str>,
    tools: &[ToolDefinition],
    surface: &SessionSurface,
    attempt: &tokio_util::sync::CancellationToken,
) -> Result<crate::llm::LlmResponse, ActorError> {
    let (evidence_events, usage_binding) = surface
        .agent
        .fact_model_evidence(messages, system, tools)
        .await;
    for event in evidence_events {
        record_run_event(surface, event).await;
    }
    match client
        .complete_streaming(messages, system, tools, attempt.clone())
        .await
    {
        Ok(mut events) => {
            let mut done = None;
            loop {
                let event = tokio::select! {
                    biased;
                    // Host cancellation is the parent of the attempt token, so
                    // it must win over the deadline branch.
                    _ = surface.cancel.cancelled() => return Ok(interrupted_response()),
                    _ = attempt.cancelled() => {
                        return Err(ActorError::Handler {
                            key: "infer".into(),
                            message: "LLM call timed out".into(),
                        });
                    }
                    event = events.recv() => event,
                };
                let Some(event) = event else {
                    break;
                };
                match event {
                    crate::llm::StreamEvent::TextDelta(text) => {
                        let event = crate::agent::AgentEvent::TextDelta { text };
                        if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
                            store.record_event(run_id, event.clone()).await;
                        }
                        if let Some(sender) = &surface.events {
                            let _ = sender.send(event).await;
                        }
                    }
                    crate::llm::StreamEvent::Done(response) => done = Some(response),
                    _ => {}
                }
            }
            if let Some(response) = done {
                record_usage(surface, &response);
                record_model_usage_event(surface, usage_binding.as_ref(), &response).await;
                return Ok(response);
            }
            if surface.cancel.is_cancelled() || attempt.is_cancelled() {
                return Ok(interrupted_response());
            }
            Err(ActorError::Handler {
                key: "infer".into(),
                message: "stream ended before a response".into(),
            })
        }
        Err(error) => {
            if surface.cancel.is_cancelled() {
                return Ok(interrupted_response());
            }
            if crate::llm::non_retryable_llm_error_message(&error).is_some() {
                return Err(model_error(error));
            }
            let response = client
                .complete(messages, system, tools)
                .await
                .map_err(model_error)?;
            record_usage(surface, &response);
            record_model_usage_event(surface, usage_binding.as_ref(), &response).await;
            Ok(response)
        }
    }
}

async fn record_run_event(surface: &SessionSurface, event: crate::agent::AgentEvent) {
    if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
        store.record_event(run_id, event.clone()).await;
    }
    if let Some(sender) = &surface.events {
        let _ = sender.send(event).await;
    }
}

async fn record_model_usage_event(
    surface: &SessionSurface,
    binding: Option<&crate::harness_evidence::ModelUsageBinding>,
    response: &crate::llm::LlmResponse,
) {
    let Some(binding) = binding else {
        return;
    };
    let Some(event) = crate::agent::AgentLoop::fact_model_usage_event(binding, &response.usage)
    else {
        return;
    };
    record_run_event(surface, event).await;
}

fn record_usage(surface: &SessionSurface, response: &crate::llm::LlmResponse) {
    let mut usage = response.usage.clone();
    // Some providers return the assistant text and omit the usage object.
    // A real turn still has to move the carried-forward totals.
    if usage.total_tokens == 0 {
        let reply = response.text();
        if !reply.is_empty() {
            let estimated = reply.len().div_ceil(4).max(1);
            usage.completion_tokens = usage.completion_tokens.max(estimated);
            usage.total_tokens = estimated;
        }
    }
    surface
        .usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .accumulate(&usage);
}

/// Bound context setup so a stalled provider cannot consume the whole turn
/// before the model is asked for a tool call. A timeout keeps the original
/// prompt and continues. A real context error still fails the attempt.
async fn model_context(
    surface: &SessionSurface,
    prompt: &str,
    message_count: usize,
) -> Result<(String, Option<String>), ActorError> {
    let work = surface
        .agent
        .fact_model_context(prompt, &surface.session_id, message_count);
    let Some(limit) = surface.agent.llm_api_timeout() else {
        return work
            .await
            .map_err(|error| ActorError::Defect(error.to_string()));
    };
    let limit = limit.min(std::time::Duration::from_secs(20));
    match tokio::time::timeout(limit, work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(ActorError::Defect(error.to_string())),
        Err(_) => Ok((prompt.to_string(), None)),
    }
}

async fn apply_run_controls(surface: &SessionSurface, messages: &mut Vec<Message>) {
    let Some(control) = &surface.run_control else {
        return;
    };
    let snapshot = control.snapshot().await;
    let pending = control.drain().await;
    if pending.is_empty() {
        return;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    for item in pending {
        let (input, reason) = match &item.request.command {
            crate::run_control::RunControlCommand::Steer { input } => (Some(input.clone()), None),
            crate::run_control::RunControlCommand::Interrupt { reason, .. } => {
                (None, reason.clone())
            }
        };
        if let Some(text) = &input {
            append_steer_fact(surface, &item.receipt.request_id, text);
            messages.push(Message::user(text));
        }
        let receipt = control
            .mark_applied(
                &item,
                snapshot.turn_id.clone(),
                snapshot.turn_revision,
                now_ms,
            )
            .await;
        if receipt.state != crate::run_control::RunControlReceiptState::Applied {
            continue;
        }
        record_run_event(
            surface,
            crate::agent::AgentEvent::RunControlApplied {
                request_id: receipt.request_id,
                operation: receipt.operation,
                turn_id: receipt.turn_id,
                turn_revision: receipt.turn_revision,
                input,
                reason,
            },
        )
        .await;
    }
}

fn append_steer_fact(surface: &SessionSurface, request_id: &str, text: &str) {
    let workspace = &surface.agent.tool_context_handle().workspace;
    let Ok(log) = FileLog::open(log_dir(workspace)) else {
        return;
    };
    let _ = log.append(
        &thread_for_session(&surface.session_id),
        &[message_fact(format!("steer:{request_id}"), text)],
        None,
    );
}

fn definitions_for(catalog: &[ToolDefinition], specs: &[ToolSpec]) -> Vec<ToolDefinition> {
    specs
        .iter()
        .map(|spec| {
            catalog
                .iter()
                .find(|tool| tool.name == spec.name)
                .cloned()
                .unwrap_or(ToolDefinition {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "additionalProperties": true
                    }),
                })
        })
        .collect()
}

/// Once the latest folded message is a verified tool result, that completion
/// does not need another provider call. A later `user.message` still does.
/// A narrative allow still calls the model so the answer can use the tool output.
fn verified_turn_text(surface: &SessionSurface, messages: &[String]) -> Option<String> {
    let last_is_tool = messages
        .last()
        .is_some_and(|line| split_folded_message(line).0 == "tool");
    if !last_is_tool {
        return None;
    }
    let ledger = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let reports = surface
        .reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match surface.agent.fact_completion_gate(&ledger, &reports) {
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Verified { .. }
            | crate::harness_loop::CompletionTerminal::Waived { .. },
        ) => Some("completed".into()),
        _ => None,
    }
}

fn split_folded_message(text: &str) -> (&str, String) {
    if let Some(body) = text.strip_prefix("user\n") {
        ("user", body.to_string())
    } else if let Some(body) = text.strip_prefix("tool\n") {
        ("tool", body.to_string())
    } else {
        ("user", text.to_string())
    }
}

fn question_from_call(call: &crate::llm::ToolCall) -> ModelDecision {
    let question = call
        .args
        .get("question")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let options = call
        .args
        .get("options")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let allow_free_text = call
        .args
        .get("allow_free_text")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    ModelDecision::Question {
        question_id: if call.id.is_empty() {
            "ask".into()
        } else {
            call.id.clone()
        },
        question,
        allow_free_text,
        options,
    }
}

/// Allow, deny, or ask for one tool call.
///
/// A policy deny is final. A policy allow, including a YOLO lane, runs the
/// tool. When the policy asks, the session permission checker decides, so a
/// plan-mode guardrail denies a write the substituted default policy would ask.
fn execution_permission(
    policy: &PermissionPolicy,
    checker: Option<&dyn crate::permissions::PermissionChecker>,
    name: &str,
    args: &serde_json::Value,
) -> PermissionDecision {
    let policy_decision = policy.check(name, args);
    if policy_decision == PermissionDecision::Deny {
        return PermissionDecision::Deny;
    }
    let Some(checker) = checker else {
        return policy_decision;
    };
    if policy_decision == PermissionDecision::Allow {
        return PermissionDecision::Allow;
    }
    checker.check(name, args)
}

fn skill_denial(agent: &crate::agent::AgentLoop, name: &str) -> Option<(String, String)> {
    agent.skill_restriction_denial(name)
}

async fn confirmation_parked(
    policy: &PermissionPolicy,
    surface: Option<&SessionSurface>,
    name: &str,
    args: &serde_json::Value,
) -> bool {
    if let Some(surface) = surface {
        if skill_denial(&surface.agent, name).is_some() {
            return false;
        }
    }
    let checker = surface.and_then(|surface| surface.agent.permission_checker());
    let checker = checker.as_deref();
    if execution_permission(policy, checker, name, args) != PermissionDecision::Ask {
        return false;
    }
    match surface.and_then(|surface| surface.confirmation.as_ref()) {
        Some(manager) => manager.requires_confirmation_for(name, args).await,
        // A bare fact run still parks on Ask. A session with no confirmation
        // manager has HITL disabled and runs the tool.
        None => surface.is_none(),
    }
}

/// Structured calls win. A text-only turn can still be a tool call when the
/// provider wrote the invocation as DSML or another leaked protocol, including
/// a reasoning channel with no normal content.
fn leaked_model_calls(response: &LlmResponse) -> Vec<(String, String, serde_json::Value)> {
    let mut calls = crate::llm::recover_leaked_tool_calls(&response.text());
    if calls.is_empty() {
        if let Some(reasoning) = response
            .message
            .reasoning_content
            .as_deref()
            .filter(|text| !text.is_empty())
        {
            calls = crate::llm::recover_leaked_tool_calls(reasoning);
        }
    }
    calls
}

async fn decision_from_response(
    response: &LlmResponse,
    policy: &PermissionPolicy,
    surface: Option<&SessionSurface>,
) -> ModelDecision {
    let structured: Vec<(String, String, serde_json::Value)> = response
        .tool_calls()
        .into_iter()
        .map(|call| (call.id, call.name, call.args))
        .collect();
    let calls = if structured.is_empty() {
        leaked_model_calls(response)
    } else {
        structured
    };
    if let Some((id, name, args)) = calls.iter().find(|call| call.1 == "ask_user") {
        return question_from_call(&crate::llm::ToolCall {
            id: id.clone(),
            name: name.clone(),
            args: args.clone(),
        });
    }
    if let Some((id, name, args)) = calls.into_iter().next() {
        let needs_confirmation = confirmation_parked(policy, surface, &name, &args).await;
        let text = crate::llm::strip_leaked_tool_protocol(&response.text());
        let text = if text.trim().is_empty() {
            None
        } else {
            Some(text)
        };
        let reasoning = response
            .message
            .reasoning_content
            .as_ref()
            .map(|text| crate::llm::strip_leaked_tool_protocol(text))
            .filter(|text| !text.trim().is_empty());
        return ModelDecision::Tool {
            call: ToolCall {
                id: if id.is_empty() { "tool".into() } else { id },
                needs_confirmation,
                name,
                args,
                text,
                reasoning,
            },
        };
    }
    ModelDecision::Text {
        text: response.text(),
    }
}

impl Completion for LiveCompletion {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let client = Arc::clone(&self.client);
        let policy = self.policy.clone();
        let catalog = self.catalog.clone();
        let surface = self.surface.clone();
        let ready_flag = Arc::clone(&self.context_ready);
        let system_slot = Arc::clone(&self.cached_system);
        let prompt_slot = Arc::clone(&self.cached_prompt);
        Box::pin(async move {
            if request.messages.len() == 1 {
                if let Some(surface) = &surface {
                    if let Some(args) = surface
                        .agent
                        .fact_auto_delegation_args(&split_folded_message(&request.messages[0]).1)
                    {
                        let needs_confirmation =
                            confirmation_parked(&policy, Some(surface), "task", &args).await;
                        return Ok(ModelDecision::Tool {
                            call: ToolCall {
                                id: "auto-task".into(),
                                name: "task".into(),
                                args,
                                needs_confirmation,
                                text: None,
                                reasoning: None,
                            },
                        });
                    }
                }
            }
            if let Some(surface) = &surface {
                if let Some(text) = verified_turn_text(surface, &request.messages) {
                    return Ok(ModelDecision::Text { text });
                }
            }
            let mut system = if request.system.is_empty() {
                None
            } else {
                Some(request.system.join("\n"))
            };
            let mut prompt_override = None;
            if let Some(surface) = &surface {
                if ready_flag
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let prompt = request
                        .messages
                        .first()
                        .map(|text| split_folded_message(text).1)
                        .unwrap_or_default();
                    let (effective, augmented) =
                        model_context(surface, &prompt, request.messages.len()).await?;
                    prompt_override = Some(effective);
                    if let Some(augmented) = augmented.filter(|text| !text.is_empty()) {
                        system = Some(match system {
                            Some(base) => format!("{base}\n{augmented}"),
                            None => augmented,
                        });
                    }
                    *prompt_slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        prompt_override.clone();
                    *system_slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = system.clone();
                } else {
                    prompt_override = prompt_slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(augmented) = system_slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                    {
                        system = Some(augmented);
                    }
                }
            }
            if !request.summary.is_empty() {
                if let Some(surface) = &surface {
                    if let Some(sender) = &surface.events {
                        let _ = sender
                            .send(crate::agent::AgentEvent::ContextCompacted {
                                session_id: surface.session_id.clone(),
                                before_messages: request.messages.len().saturating_add(1),
                                after_messages: 1,
                                percent_before: 1.0,
                                summary: Some(request.summary.clone()),
                            })
                            .await;
                    }
                }
            }
            let mut folded = Vec::new();
            if !request.summary.is_empty() {
                folded.push(format!("user\n{}", request.summary));
            }
            folded.extend(request.messages.iter().cloned());
            let prompt_index = usize::from(!request.summary.is_empty());
            let mut recorded_calls = surface
                .as_ref()
                .map(|surface| {
                    recorded_tool_calls(
                        &surface.agent.tool_context_handle().workspace,
                        &thread_for_session(&surface.session_id),
                    )
                })
                .unwrap_or_default()
                .into_iter();
            let mut messages: Vec<Message> = folded
                .iter()
                .enumerate()
                .flat_map(|(index, text)| {
                    let (role, body) = split_folded_message(text);
                    let body = if index == prompt_index {
                        prompt_override.clone().unwrap_or(body)
                    } else {
                        body
                    };
                    if role == "tool" {
                        let call = recorded_calls.next().unwrap_or_else(|| RecordedCall {
                            id: format!("fact-tool-{index}"),
                            name: "tool".into(),
                            args: serde_json::json!({}),
                            text: String::new(),
                            reasoning: None,
                        });
                        let id = call.id.clone();
                        vec![
                            assistant_tool_call(&call),
                            Message::tool_result(&id, &body, false),
                        ]
                    } else {
                        vec![Message::user(&body)]
                    }
                })
                .collect();
            if let Some(surface) = &surface {
                apply_run_controls(surface, &mut messages).await;
                if surface.cancel.is_cancelled() {
                    return Ok(ModelDecision::Text {
                        text: "(Response interrupted by the user.)".into(),
                    });
                }
            }
            let tools = definitions_for(&catalog, &request.tools);
            let response = model_response(
                Arc::clone(&client),
                messages.clone(),
                system.clone(),
                tools,
                surface.clone(),
            )
            .await?;
            if let Some(surface) = &surface {
                if surface.cancel.is_cancelled() {
                    apply_run_controls(surface, &mut messages).await;
                    let text = "(Response interrupted by the user.)";
                    messages.push(Message::assistant(text));
                    *surface
                        .transcript
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = messages;
                    return Ok(ModelDecision::Text {
                        text: text.to_string(),
                    });
                }
                let prompt = request
                    .messages
                    .first()
                    .map(|text| split_folded_message(text).1)
                    .unwrap_or_default();
                let prompt = prompt.as_str();
                surface
                    .agent
                    .fact_observe_model(&surface.session_id, prompt, &response)
                    .await;
                let mut transcript = messages.clone();
                if response.tool_calls().is_empty() {
                    transcript.push(Message::assistant(&response.text()));
                }
                *surface
                    .transcript
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = transcript;
                if response.tool_calls().is_empty() {
                    surface
                        .agent
                        .fact_post_response(
                            &surface.session_id,
                            &response.text(),
                            0,
                            &response.usage,
                        )
                        .await;
                }
            }
            Ok(decision_from_response(&response, &policy, surface.as_ref()).await)
        })
    }
}

struct ExecutorTools {
    executor: Arc<ToolExecutor>,
    calls: Arc<AtomicUsize>,
    checkpoint: Option<CheckpointState>,
    events: Option<tokio::sync::mpsc::Sender<crate::agent::AgentEvent>>,
    context: crate::tools::ToolContext,
    permission: PermissionPolicy,
    checker: Option<Arc<dyn crate::permissions::PermissionChecker>>,
    agent: Option<crate::agent::AgentLoop>,
    run_store: Option<Arc<crate::run::InMemoryRunStore>>,
    run_id: Option<String>,
    ledger: Option<Arc<Mutex<crate::harness_loop::MutationLedger>>>,
    reports: Option<Arc<Mutex<Vec<crate::verification::VerificationReport>>>>,
}

#[derive(Clone)]
struct CheckpointState {
    sink: Arc<dyn crate::loop_checkpoint::LoopCheckpointSink>,
    run_id: String,
    session_id: String,
    capability_binding: Option<crate::capability::RunCapabilityBindingV1>,
    turns: Arc<AtomicUsize>,
}

impl ExecutorTools {
    async fn emit(&self, event: crate::agent::AgentEvent) {
        if let (Some(store), Some(run_id)) = (&self.run_store, &self.run_id) {
            store.record_event(run_id, event.clone()).await;
        }
        if let Some(events) = &self.events {
            let _ = events.send(event).await;
        }
    }

    async fn forward_host_events(
        &self,
        bridged: &mut Option<tokio::sync::broadcast::Receiver<crate::agent::AgentEvent>>,
    ) {
        let Some(receiver) = bridged.as_mut() else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok(event) if host_bridge_event(&event) => self.emit(event).await,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }

    async fn save_checkpoint(&self, call: &ToolCall, output: &str, is_error: bool) {
        let Some(checkpoint) = &self.checkpoint else {
            return;
        };
        let turn = checkpoint.turns.fetch_add(1, Ordering::SeqCst) + 1;
        let checkpoint_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        checkpoint
            .sink
            .save_checkpoint(&crate::loop_checkpoint::LoopCheckpoint {
                schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
                run_id: checkpoint.run_id.clone(),
                session_id: checkpoint.session_id.clone(),
                capability_binding: checkpoint.capability_binding.clone(),
                turn,
                messages: vec![
                    Message {
                        role: "assistant".into(),
                        content: vec![crate::llm::ContentBlock::ToolUse {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            input: call.args.clone(),
                        }],
                        reasoning_content: None,
                        transcript_text: None,
                        transcript_visibility: Default::default(),
                    },
                    Message::tool_result(&call.id, output, is_error),
                ],
                total_usage: crate::llm::TokenUsage::default(),
                tool_calls_count: turn,
                verification_reports: Vec::new(),
                convergence: crate::loop_checkpoint::LoopConvergenceState::default(),
                checkpoint_ms,
            })
            .await;
    }
}

fn host_bridge_event(event: &crate::agent::AgentEvent) -> bool {
    matches!(
        event,
        crate::agent::AgentEvent::SubagentStart { .. }
            | crate::agent::AgentEvent::SubagentProgress { .. }
            | crate::agent::AgentEvent::SubagentEnd { .. }
            | crate::agent::AgentEvent::TaskUpdated { .. }
            | crate::agent::AgentEvent::ConfirmationRequired { .. }
            | crate::agent::AgentEvent::ConfirmationReceived { .. }
            | crate::agent::AgentEvent::ConfirmationTimeout { .. }
            | crate::agent::AgentEvent::UserQuestion { .. }
    )
}

impl ToolRunner for ExecutorTools {
    fn run(
        &self,
        call: ToolCall,
    ) -> a3s_effect::coding::BoxFuture<Result<serde_json::Value, ActorError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let executor = Arc::clone(&self.executor);
        let context = self.context.clone();
        let events = self.events.clone();
        let permission = self.permission.clone();
        let checker = self.checker.clone();
        let agent = self.agent.clone();
        let checkpoint = self.checkpoint.clone();
        let run_store = self.run_store.clone();
        let run_id = self.run_id.clone();
        let ledger = self.ledger.clone();
        let reports = self.reports.clone();
        Box::pin(async move {
            let tools = ExecutorTools {
                executor,
                calls: Arc::new(AtomicUsize::new(0)),
                checkpoint,
                events,
                context,
                permission,
                checker,
                agent,
                run_store,
                run_id,
                ledger,
                reports,
            };
            emit_tool_request_bound(&tools, &call).await;
            if let Some(agent) = &tools.agent {
                if let Some((output, reason)) = skill_denial(agent, &call.name) {
                    tools
                        .emit(crate::agent::AgentEvent::PermissionDenied {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            args: call.args.clone(),
                            reason,
                        })
                        .await;
                    tools.save_checkpoint(&call, &output, true).await;
                    return Ok(serde_json::Value::String(output));
                }
            }
            if execution_permission(
                &tools.permission,
                tools.checker.as_deref(),
                &call.name,
                &call.args,
            ) == PermissionDecision::Deny
            {
                let output = format!(
                    "Permission denied: Tool '{}' is blocked by permission policy.",
                    call.name
                );
                tools
                    .emit(crate::agent::AgentEvent::PermissionDenied {
                        tool_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        args: call.args.clone(),
                        reason: "Blocked by deny rule in permission policy".into(),
                    })
                    .await;
                tools.save_checkpoint(&call, &output, true).await;
                return Ok(serde_json::Value::String(output));
            }
            tools
                .emit(crate::agent::AgentEvent::ToolStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                })
                .await;
            let mut args = call.args.clone();
            if let Some(agent) = &tools.agent {
                let session_id = tools.context.session_id.clone().unwrap_or_default();
                let decision = agent
                    .fire_pre_tool_use(&session_id, &call.name, &args, Vec::new())
                    .await;
                if let Some(denial) = decision.denial {
                    let output = format!("Hook denied {}: {}", call.name, denial.reason);
                    tools
                        .emit(crate::agent::AgentEvent::PermissionDenied {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            args: args.clone(),
                            reason: denial.reason,
                        })
                        .await;
                    return Ok(serde_json::Value::String(output));
                }
                if let Some(updated) = decision.updated_args {
                    args = updated;
                }
            }
            tools
                .emit(crate::agent::AgentEvent::ToolExecutionStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: args.clone(),
                })
                .await;
            let mut bridged = tools
                .context
                .agent_event_tx
                .as_ref()
                .map(|sender| sender.subscribe());
            let cancel = tools.context.cancellation_token();
            let started = std::time::Instant::now();
            let executed = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let message = "cancelled".to_string();
                    tools
                        .emit(crate::agent::AgentEvent::ToolEnd {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            args: Some(args.clone()),
                            output: message.clone(),
                            exit_code: 1,
                            metadata: None,
                            error_kind: None,
                        })
                        .await;
                    return Ok(serde_json::Value::String(message));
                }
                result = tools
                    .executor
                    .execute_with_context(&call.name, &args, &tools.context) => result,
            };
            tools.forward_host_events(&mut bridged).await;
            let result = match executed {
                Ok(result) => result,
                Err(error) => {
                    let message = visible_tool_output(tools.agent.as_ref(), error.to_string());
                    tools
                        .emit(crate::agent::AgentEvent::ToolEnd {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            args: Some(call.args.clone()),
                            output: message.clone(),
                            exit_code: 1,
                            metadata: None,
                            error_kind: None,
                        })
                        .await;
                    if workspace_boundary_error(&message) {
                        return Ok(serde_json::Value::String(message));
                    }
                    return Err(ActorError::Handler {
                        key: call.name.clone(),
                        message,
                    });
                }
            };
            let mut output = visible_tool_output(tools.agent.as_ref(), result.output);
            if let Some(agent) = &tools.agent {
                let session_id = tools.context.session_id.clone().unwrap_or_default();
                agent
                    .fire_post_tool_use(
                        &session_id,
                        &call.name,
                        &args,
                        &output,
                        result.exit_code == 0,
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    )
                    .await;
            }
            let mut metadata = result.metadata;
            if let Some(agent) = &tools.agent {
                let mut bound = tools
                    .reports
                    .as_ref()
                    .map(|reports| {
                        reports
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                    })
                    .unwrap_or_default();
                if let Some(ledger) = &tools.ledger {
                    let mut ledger_value = ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    agent
                        .record_fact_tool_effect(
                            &call.name,
                            &call.args,
                            result.exit_code,
                            &mut output,
                            &mut metadata,
                            &mut ledger_value,
                            &mut bound,
                        )
                        .await;
                    *ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = ledger_value;
                }
                if let Some(reports) = &tools.reports {
                    *reports
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = bound;
                }
            } else if let Some(ledger) = &tools.ledger {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observe_tool(&call.name, result.exit_code, metadata.as_ref());
            }
            tools
                .emit(crate::agent::AgentEvent::ToolEnd {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: Some(call.args.clone()),
                    output: output.clone(),
                    exit_code: result.exit_code,
                    metadata: metadata.clone(),
                    error_kind: result.error_kind.clone(),
                })
                .await;
            tools
                .save_checkpoint(&call, &output, result.exit_code != 0)
                .await;
            Ok(serde_json::Value::String(output))
        })
    }
}

struct NoopCompact;

impl Compactor for NoopCompact {
    fn compact(
        &self,
        messages: &[String],
    ) -> a3s_effect::coding::BoxFuture<Result<String, ActorError>> {
        let summary = messages.join(" ");
        Box::pin(async move { Ok(summary) })
    }
}

struct RecordedCall {
    id: String,
    name: String,
    args: serde_json::Value,
    text: String,
    reasoning: Option<String>,
}

fn seeded_user_text(message: &Message) -> String {
    let tool_text = message
        .content
        .iter()
        .filter_map(|block| {
            if let crate::llm::ContentBlock::ToolResult { content, .. } = block {
                Some(content.as_text())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    if !tool_text.is_empty() {
        return tool_text;
    }
    message.text()
}

fn assistant_tool_call(call: &RecordedCall) -> Message {
    let mut content = Vec::new();
    if !call.text.is_empty() {
        content.push(crate::llm::ContentBlock::Text {
            text: call.text.clone(),
        });
    }
    content.push(crate::llm::ContentBlock::ToolUse {
        id: call.id.clone(),
        name: call.name.clone(),
        input: call.args.clone(),
    });
    Message {
        role: "assistant".into(),
        content,
        reasoning_content: call.reasoning.clone(),
        transcript_text: None,
        transcript_visibility: crate::llm::TranscriptVisibility::Wire,
    }
}

/// Tool calls that already have a result, in log order.
///
/// The folded transcript only keeps `tool\n{output}`. The provider still needs
/// the real name, id, and arguments before that result. Compaction drops those
/// lines from the view, so calls before the latest `compaction.done` are not
/// paired with the messages that remain.
fn recorded_tool_calls(workspace: &Path, thread: &str) -> Vec<RecordedCall> {
    let Ok(facts) = read_workspace_facts(workspace, thread) else {
        return Vec::new();
    };
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == "compaction.done")
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut pending = None;
    let mut calls = Vec::new();
    for fact in facts.into_iter().skip(start) {
        if fact.kind == "model.turn"
            && fact.payload.get("kind").and_then(|kind| kind.as_str()) == Some("tool")
        {
            let call = fact.payload.get("call");
            let id = call
                .and_then(|value| value.get("id"))
                .and_then(|value| value.as_str())
                .unwrap_or("tool");
            let name = call
                .and_then(|value| value.get("name"))
                .and_then(|value| value.as_str())
                .unwrap_or("tool");
            let args = call
                .and_then(|value| value.get("args"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let text = call
                .and_then(|value| value.get("text"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let reasoning = call
                .and_then(|value| value.get("reasoning"))
                .and_then(|value| value.as_str())
                .filter(|text| !text.is_empty())
                .map(str::to_string);
            pending = Some(RecordedCall {
                id: id.to_string(),
                name: name.to_string(),
                args,
                text,
                reasoning,
            });
        } else if fact.kind == "tool.result" {
            if let Some(call) = pending.take() {
                calls.push(call);
            }
        }
    }
    calls
}

fn workspace_boundary_error(message: &str) -> bool {
    message.contains("escapes workspace") || message.contains("Workspace boundary")
}

fn visible_tool_output(agent: Option<&crate::agent::AgentLoop>, output: String) -> String {
    match agent {
        Some(agent) => agent.sanitize_tool_output(&output),
        None => output,
    }
}

async fn emit_tool_request_bound(tools: &ExecutorTools, call: &a3s_effect::ToolCall) {
    let Ok(snapshot) = crate::harness_evidence::ToolRequestSnapshotV1::capture(
        &call.id,
        &call.name,
        &call.args,
        crate::harness_evidence::ToolRequestOriginV1::Agent,
    ) else {
        return;
    };
    tools
        .emit(crate::agent::AgentEvent::ToolRequestBound {
            tool_id: call.id.clone(),
            tool_name: call.name.clone(),
            snapshot,
        })
        .await;
}

fn log_dir(workspace: &Path) -> PathBuf {
    workspace.join(".a3s").join("effect-log")
}

/// Drop a previous process's fact log so a fixed test workspace starts idle.
#[cfg(test)]
pub(crate) fn reset_session_fact_log(workspace: impl AsRef<Path>) {
    let _ = std::fs::remove_dir_all(log_dir(workspace.as_ref()));
}

impl FactRun {
    pub fn open(
        dir: impl Into<PathBuf>,
        completion: Arc<dyn Completion>,
        tools: Arc<dyn ToolRunner>,
        config: HarnessConfig,
        run_id: impl Into<String>,
    ) -> Result<Self> {
        Self::open_composed(dir, completion, tools, config, run_id, None)
    }

    /// Admit an optional Meta Harness compose recipe (default = stock actor).
    pub fn open_composed(
        dir: impl Into<PathBuf>,
        completion: Arc<dyn Completion>,
        tools: Arc<dyn ToolRunner>,
        config: HarnessConfig,
        run_id: impl Into<String>,
        compose: Option<&crate::meta_harness::HarnessComposeOptions>,
    ) -> Result<Self> {
        let step_limit = config.step_limit();
        let (graph, _kernel) = crate::meta_harness::admit_from_compose(compose, config)?;
        Self::open_with_graph(dir, completion, tools, step_limit, run_id, graph)
    }

    /// Admit an explicit Meta Harness graph. Kernel policy (permissions +
    /// completion gate) remains Core-owned and cannot be cleared by the graph.
    pub fn open_with_graph(
        dir: impl Into<PathBuf>,
        completion: Arc<dyn Completion>,
        tools: Arc<dyn ToolRunner>,
        step_limit: u32,
        run_id: impl Into<String>,
        graph: HarnessGraph,
    ) -> Result<Self> {
        let _kernel = crate::meta_harness::KernelPolicy::default().admit();
        let dir = dir.into();
        let capped = Arc::new(CappedCompletion {
            inner: completion,
            run_id: run_id.into(),
        });
        let services = Arc::new(CodingServices {
            completion: capped,
            tools,
            compactor: Arc::new(NoopCompact),
        });
        Ok(Self {
            dir,
            thread: THREAD.to_string(),
            actor: graph.into_actor(),
            services,
            limit: step_limit,
        })
    }

    pub fn open_session(
        workspace: &Path,
        session_id: &str,
        client: Arc<dyn LlmClient>,
        executor: Arc<ToolExecutor>,
        permission: PermissionPolicy,
        yolo_lanes: &[SessionLane],
        max_tool_rounds: usize,
    ) -> Result<Self> {
        let catalog = executor.definitions();
        let specs = catalog
            .iter()
            .map(|tool| ToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
            })
            .collect();
        let permission = permission.allow_yolo_lanes(yolo_lanes.iter().copied());
        let (completion, _) = LiveCompletion::new(client, permission.clone(), catalog);
        let cap = u32::try_from(max_tool_rounds).unwrap_or(u32::MAX);
        let config = HarnessConfig::new(8, 1_000_000, 32, 2, Vec::new(), specs)
            .map_err(|error| anyhow::anyhow!(error))?
            .with_tool_round_cap(cap);
        let context = executor.registry().context();
        let mut run = Self::open(
            log_dir(workspace),
            Arc::new(completion),
            Arc::new(ExecutorTools {
                executor,
                calls: Arc::new(AtomicUsize::new(0)),
                checkpoint: None,
                events: None,
                context,
                permission: permission.clone(),
                checker: None,
                agent: None,
                run_store: None,
                run_id: None,
                ledger: None,
                reports: None,
            }),
            config,
            session_id,
        )?;
        run.thread = thread_for_session(session_id);
        Ok(run)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_projected(
        workspace: &Path,
        session_id: &str,
        client: Arc<dyn LlmClient>,
        executor: Arc<ToolExecutor>,
        permission: PermissionPolicy,
        yolo_lanes: &[SessionLane],
        max_tool_rounds: usize,
        surface: SessionSurface,
    ) -> Result<Self> {
        let catalog = executor.definitions();
        let specs = catalog
            .iter()
            .map(|tool| ToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone(),
            })
            .collect();
        let permission = permission.allow_yolo_lanes(yolo_lanes.iter().copied());
        let (completion, _) = LiveCompletion::new(client, permission.clone(), catalog);
        let events = surface.events.clone();
        let run_store = surface.run_store.clone();
        let run_id = surface.run_id.clone();
        let ledger = Some(Arc::clone(&surface.ledger));
        let reports = Some(Arc::clone(&surface.reports));
        let checker = surface.agent.permission_checker();
        let agent = surface.agent.clone();
        let context = surface
            .agent
            .tool_context_handle()
            .with_cancellation(surface.cancel.clone());
        let completion = completion.with_surface(surface.clone());
        let cap = u32::try_from(max_tool_rounds).unwrap_or(u32::MAX);
        let config = HarnessConfig::new(
            8,
            agent.fact_compact_after_chars(),
            32,
            2,
            Vec::new(),
            specs,
        )
        .map_err(|error| anyhow::anyhow!(error))?
        .with_tool_round_cap(cap);
        let harness = surface.harness.clone();
        let checkpoint = surface.checkpoint.map(|checkpoint| CheckpointState {
            sink: checkpoint.sink,
            run_id: checkpoint.run_id,
            session_id: checkpoint.session_id,
            capability_binding: checkpoint.capability_binding,
            turns: Arc::new(AtomicUsize::new(0)),
        });
        let mut run = Self::open_composed(
            log_dir(workspace),
            Arc::new(completion),
            Arc::new(ExecutorTools {
                executor,
                calls: Arc::new(AtomicUsize::new(0)),
                checkpoint,
                events,
                context,
                permission,
                checker,
                agent: Some(agent),
                run_store,
                run_id,
                ledger,
                reports,
            }),
            config,
            session_id,
            harness.as_ref(),
        )?;
        run.thread = thread_for_session(session_id);
        Ok(run)
    }

    fn open_log(&self) -> Result<FileLog> {
        FileLog::open(&self.dir).map_err(|error| anyhow::anyhow!(error))
    }

    pub fn read_facts(&self) -> Result<Vec<a3s_effect::Fact>> {
        let log = self.open_log()?;
        log.read(&self.thread)
            .map_err(|error| anyhow::anyhow!(error))
    }

    /// Drop this thread's log so a checkpoint resume is not a leftover session.
    pub fn reset_thread_log(&self) -> Result<()> {
        let path = self.dir.join(format!("{}.jsonl", self.thread));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| anyhow::anyhow!(error))?;
        }
        Ok(())
    }

    /// Record prior turns so the new prompt is the only infer that runs.
    ///
    /// Each assistant text is a `model.turn` whose cause is `infer:{turn}:{cycle}`.
    /// An existing log is already the control source and is left unchanged.
    pub fn seed_history(&self, history: &[Message]) -> Result<()> {
        let log = self.open_log()?;
        if !log
            .read(&self.thread)
            .map_err(|error| anyhow::anyhow!(error))?
            .is_empty()
        {
            return Ok(());
        }
        let mut turn = 0u64;
        let mut cycle = 0u64;
        for message in history {
            match message.role.as_str() {
                "user" => {
                    turn += 1;
                    cycle = 0;
                    log.append(
                        &self.thread,
                        &[message_fact(
                            format!("m-hist-{turn}"),
                            seeded_user_text(message),
                        )],
                        None,
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                }
                "assistant" => {
                    let payload = serde_json::to_value(ModelDecision::Text {
                        text: message.text(),
                    })
                    .map_err(|error| anyhow::anyhow!(error))?;
                    log.append(
                        &self.thread,
                        &[NewFact {
                            kind: "model.turn".into(),
                            key: format!("model:{turn}:{cycle}"),
                            payload,
                        }],
                        Some(&format!("infer:{turn}:{cycle}")),
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                    cycle += 1;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub async fn user_text(&self, text: &str) -> Result<a3s_effect::Settlement<CodingView>> {
        let log = self.open_log()?;
        let key = format!("m-{}", log.read(&self.thread).unwrap_or_default().len());
        ingest_coding(
            &self.actor,
            &log,
            Arc::clone(&self.services),
            &self.thread,
            message_fact(key, text),
            self.limit,
        )
        .await
        .map_err(exit_to_error)
    }

    pub async fn steer(&self, text: &str) -> Result<a3s_effect::Settlement<CodingView>> {
        self.user_text(text).await
    }

    pub async fn resume_limit(&self, limit: u32) -> Result<a3s_effect::Settlement<CodingView>> {
        let log = self.open_log()?;
        resume_coding(
            &self.actor,
            &log,
            Arc::clone(&self.services),
            &self.thread,
            limit,
        )
        .await
        .map_err(exit_to_error)
    }

    pub async fn confirm(
        &self,
        tool_call_id: &str,
        approved: bool,
    ) -> Result<a3s_effect::Settlement<CodingView>> {
        let log = self.open_log()?;
        let key = format!("c-{}", log.read(&self.thread).unwrap_or_default().len());
        ingest_coding(
            &self.actor,
            &log,
            Arc::clone(&self.services),
            &self.thread,
            confirm_fact(key, tool_call_id, approved),
            self.limit,
        )
        .await
        .map_err(exit_to_error)
    }

    /// Append `confirmation.answered` only when this tool id is the parked one.
    pub async fn confirm_if_pending(&self, tool_call_id: &str, approved: bool) -> Result<bool> {
        let pending = match self.resume_limit(0).await {
            Ok(settled) => {
                settled.view.phase == CodingPhase::Confirm
                    && settled
                        .view
                        .pending_confirmation
                        .as_ref()
                        .is_some_and(|pending| pending.tool_call_id == tool_call_id)
            }
            Err(error) => {
                let rendered = format!("{error:#}");
                if rendered.contains("StepLimit") {
                    false
                } else {
                    return Err(error);
                }
            }
        };
        if !pending {
            return Ok(false);
        }
        let before = self.read_facts()?.len();
        let settled = self.confirm(tool_call_id, approved).await?;
        Ok(settled.log.len() > before)
    }

    pub async fn answer(&self, text: &str) -> Result<a3s_effect::Settlement<CodingView>> {
        let log = self.open_log()?;
        let key = format!("a-{}", log.read(&self.thread).unwrap_or_default().len());
        ingest_coding(
            &self.actor,
            &log,
            Arc::clone(&self.services),
            &self.thread,
            answer_fact(key, text),
            self.limit,
        )
        .await
        .map_err(exit_to_error)
    }

    pub async fn append_model_turn(&self, payload: serde_json::Value) -> Result<()> {
        let log = self.open_log()?;
        log.append(
            &self.thread,
            &[NewFact {
                kind: "model.turn".into(),
                key: "model:bad".into(),
                payload,
            }],
            None,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
        Ok(())
    }
}

pub fn read_workspace_facts(workspace: &Path, thread: &str) -> Result<Vec<a3s_effect::Fact>> {
    let log = FileLog::open(log_dir(workspace)).map_err(|error| anyhow::anyhow!(error))?;
    log.read(thread).map_err(|error| anyhow::anyhow!(error))
}

fn exit_to_error(error: Exit<ActorError>) -> anyhow::Error {
    let message = match error {
        Exit::Die(message) => message,
        Exit::Interrupt => "cancelled".to_string(),
        Exit::Fail(ActorError::Handler { message, .. }) => message,
        Exit::Fail(ActorError::Defect(message)) => message,
        Exit::Fail(error @ ActorError::StepLimit { .. }) => format!("StepLimit: {error}"),
        Exit::Fail(other) => other.to_string(),
    };
    anyhow::anyhow!(message)
}

pub fn phase_name(phase: CodingPhase) -> &'static str {
    match phase {
        CodingPhase::Idle => "idle",
        CodingPhase::Infer => "infer",
        CodingPhase::Compact => "compact",
        CodingPhase::Confirm => "confirm",
        CodingPhase::Question => "question",
        CodingPhase::Tool => "tool",
        CodingPhase::Deny => "deny",
        CodingPhase::Done => "done",
    }
}

/// Map a permission decision onto the tool call the fold will store.
pub fn project_needs_confirmation(policy: &PermissionPolicy, tool_name: &str) -> bool {
    confirmation_required(policy, tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct ScriptModel {
        decisions: Mutex<Vec<Result<ModelDecision, ActorError>>>,
        calls: Arc<AtomicUsize>,
        tool_counts: Mutex<Vec<usize>>,
        messages: Mutex<Vec<Vec<String>>>,
    }

    impl Completion for ScriptModel {
        fn complete(
            &self,
            request: CompletionRequest,
        ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.tool_counts.lock().unwrap().push(request.tools.len());
            self.messages.lock().unwrap().push(request.messages.clone());
            let decision =
                self.decisions
                    .lock()
                    .unwrap()
                    .pop()
                    .unwrap_or(Ok(ModelDecision::Text {
                        text: "empty".into(),
                    }));
            Box::pin(async move { decision })
        }
    }

    struct ScriptTools {
        calls: Arc<AtomicUsize>,
        fail_first: AtomicUsize,
    }

    impl ToolRunner for ScriptTools {
        fn run(
            &self,
            call: ToolCall,
        ) -> a3s_effect::coding::BoxFuture<Result<serde_json::Value, ActorError>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail_first.load(Ordering::SeqCst) > 0 && n == 0;
            let name = call.name;
            Box::pin(async move {
                if fail {
                    Err(ActorError::Handler {
                        key: name,
                        message: "down".into(),
                    })
                } else {
                    Ok(serde_json::json!(format!("ran {name}")))
                }
            })
        }
    }

    fn config(cap: Option<u32>, budget: u32) -> HarnessConfig {
        let config = HarnessConfig::new(
            budget,
            1_000_000,
            16,
            2,
            vec!["system".into()],
            vec![ToolSpec {
                name: "read".into(),
                description: "Read".into(),
            }],
        )
        .unwrap();
        match cap {
            Some(cap) => config.with_tool_round_cap(cap),
            None => config,
        }
    }

    fn run_with(
        decisions: Vec<Result<ModelDecision, ActorError>>,
        cap: Option<u32>,
        budget: u32,
        fail_first: bool,
    ) -> (
        tempfile::TempDir,
        FactRun,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<ScriptModel>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(ScriptModel {
            decisions: Mutex::new(decisions.into_iter().rev().collect()),
            calls: Arc::clone(&calls),
            tool_counts: Mutex::new(Vec::new()),
            messages: Mutex::new(Vec::new()),
        });
        let fact = FactRun::open(
            dir.path(),
            model.clone(),
            Arc::new(ScriptTools {
                calls: Arc::clone(&tool_calls),
                fail_first: AtomicUsize::new(usize::from(fail_first)),
            }),
            config(cap, budget),
            {
                static FACT_RUNS: AtomicUsize = AtomicUsize::new(0);
                format!("run-{}", FACT_RUNS.fetch_add(1, Ordering::Relaxed))
            },
        )
        .unwrap();
        (dir, fact, calls, tool_calls, model)
    }

    #[tokio::test]
    async fn fact_log_second_resume_does_not_call_the_model() {
        let (_dir, fact, calls, tools, _) = run_with(
            vec![Ok(ModelDecision::Text { text: "hi".into() })],
            None,
            4,
            false,
        );
        let settled = fact.user_text("hello").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Done);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let again = fact.resume_limit(8).await.unwrap();
        assert_eq!(again.steps, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(tools.load(Ordering::SeqCst), 0);
        let facts = fact.read_facts().unwrap();
        assert!(facts.iter().any(|fact| fact.kind == "model.turn"));
    }

    #[tokio::test]
    async fn fact_log_non_decision_sets_schema_error_without_a_tool_or_parse_retry() {
        let (_dir, fact, calls, tools, _) = run_with(vec![], None, 4, false);
        fact.append_model_turn(serde_json::json!({"kind": "nope"}))
            .await
            .unwrap();
        let settled = fact.resume_limit(8).await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Done);
        assert!(settled.view.schema_error.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(tools.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fact_log_denial_does_not_run_the_tool() {
        let (_dir, fact, _, tools, _) = run_with(
            vec![Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: true,
                    text: None,
                    reasoning: None,
                },
            })],
            None,
            4,
            false,
        );
        let parked = fact.user_text("read it").await.unwrap();
        assert_eq!(parked.view.phase, CodingPhase::Confirm);
        assert_eq!(tools.load(Ordering::SeqCst), 0);
        let denied = fact.confirm("t1", false).await.unwrap();
        assert_eq!(denied.view.assistant.as_deref(), Some("denied"));
        assert_eq!(tools.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fact_log_question_options_and_allow_free_text_survive_reopen() {
        let (dir, fact, calls, tools, _) = run_with(
            vec![Ok(ModelDecision::Question {
                question_id: "q1".into(),
                question: "Which?".into(),
                allow_free_text: true,
                options: vec!["left".into(), "right".into()],
            })],
            None,
            4,
            false,
        );
        let parked = fact.user_text("ask").await.unwrap();
        assert_eq!(parked.view.phase, CodingPhase::Question);
        drop(parked);
        let reopened = FactRun::open(
            dir.path(),
            Arc::new(ScriptModel {
                decisions: Mutex::new(vec![]),
                calls: Arc::clone(&calls),
                tool_counts: Mutex::new(Vec::new()),
                messages: Mutex::new(Vec::new()),
            }),
            Arc::new(ScriptTools {
                calls: Arc::new(AtomicUsize::new(0)),
                fail_first: AtomicUsize::new(0),
            }),
            config(None, 4),
            "run-fact-2",
        )
        .unwrap();
        let still = reopened.resume_limit(8).await.unwrap();
        assert_eq!(still.steps, 0);
        let question = still.view.pending_question.expect("question");
        assert!(question.allow_free_text);
        assert_eq!(
            question.options,
            vec!["left".to_string(), "right".to_string()]
        );
        assert_eq!(tools.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fact_log_question_stays_parked_until_the_answer_fact() {
        let (_dir, fact, calls, _, _) = run_with(
            vec![
                Ok(ModelDecision::Question {
                    question_id: "q1".into(),
                    question: "Which?".into(),
                    allow_free_text: true,
                    options: vec!["left".into(), "right".into()],
                }),
                Ok(ModelDecision::Text {
                    text: "left".into(),
                }),
            ],
            None,
            4,
            false,
        );
        let parked = fact.user_text("ask").await.unwrap();
        assert_eq!(parked.view.phase, CodingPhase::Question);
        let calls_while_parked = calls.load(Ordering::SeqCst);
        let resumed = fact.resume_limit(4).await.unwrap();
        assert_eq!(resumed.steps, 0);
        assert_eq!(resumed.view.phase, CodingPhase::Question);
        assert_eq!(calls.load(Ordering::SeqCst), calls_while_parked);
        assert!(resumed
            .log
            .iter()
            .all(|fact| fact.kind != "question.answered"));
        let answered = fact.answer("left").await.unwrap();
        assert!(answered
            .log
            .iter()
            .any(|fact| fact.kind == "question.answered"));
        assert_eq!(answered.view.phase, CodingPhase::Done);
        assert!(calls.load(Ordering::SeqCst) > calls_while_parked);
    }

    #[tokio::test]
    async fn fact_log_missing_tool_result_runs_once_and_does_not_ask_the_model_again() {
        let (_dir, fact, calls, tools, _) = run_with(
            vec![Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: false,
                    text: None,
                    reasoning: None,
                },
            })],
            None,
            4,
            true,
        );
        let failed = fact.user_text("go").await;
        assert!(failed.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(tools.load(Ordering::SeqCst), 1);
        let again = fact.resume_limit(1).await;
        assert!(again.is_err(), "the step limit stops the following infer");
        assert_eq!(tools.load(Ordering::SeqCst), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fact_log_step_limit_does_not_start_a_pending_transition() {
        let (_dir, fact, calls, _, _) = run_with(vec![], None, 4, false);
        let log = fact.open_log().unwrap();
        log.append(THREAD, &[message_fact("m1", "hello")], None)
            .unwrap();
        drop(log);
        let error = match fact.resume_limit(0).await {
            Ok(_) => panic!("a pending transition at limit 0 must not run"),
            Err(error) => error,
        };
        let rendered = format!("{error:#}");
        assert!(rendered.contains("StepLimit"), "{rendered}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fact_log_budget_does_not_call_the_tool_body() {
        let (_dir, fact, _, tools, _) = run_with(
            vec![
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t1".into(),
                        name: "read".into(),
                        args: serde_json::json!({}),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },
                }),
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t2".into(),
                        name: "read".into(),
                        args: serde_json::json!({}),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },
                }),
            ],
            None,
            1,
            false,
        );
        let settled = fact.user_text("two tools").await.unwrap();
        assert_eq!(tools.load(Ordering::SeqCst), 1);
        assert!(settled.log.iter().any(|fact| fact.kind == "budget.denied"));
    }

    #[tokio::test]
    async fn fact_log_tool_round_cap_uses_an_empty_tool_list() {
        let (_dir, fact, calls, _, model) = run_with(
            vec![Ok(ModelDecision::Text { text: "ok".into() })],
            Some(0),
            4,
            false,
        );
        fact.user_text("hello").await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(model.tool_counts.lock().unwrap().as_slice(), &[0]);
        assert!(model
            .messages
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .all(|line| !line.contains("TOOL_BUDGET_FINALIZATION")));
    }

    #[tokio::test]
    async fn fact_log_steer_is_a_user_message_fact() {
        let (_dir, fact, calls, _, _) = run_with(
            vec![
                Ok(ModelDecision::Text {
                    text: "first".into(),
                }),
                Ok(ModelDecision::Text {
                    text: "steered".into(),
                }),
            ],
            None,
            4,
            false,
        );
        fact.user_text("hello").await.unwrap();
        let steered = fact.steer("turn left").await.unwrap();
        assert_eq!(steered.view.assistant.as_deref(), Some("steered"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let facts = fact.read_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .filter(|fact| fact.kind == "user.message")
                .count(),
            2
        );
    }

    struct HangClient;

    #[async_trait::async_trait]
    impl LlmClient for HangClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<crate::llm::LlmResponse> {
            std::future::pending().await
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            tokio::spawn(async move {
                std::future::pending::<()>().await;
                drop(sender);
            });
            Ok(receiver)
        }
    }

    #[tokio::test]
    async fn fact_log_model_stream_timeout_stops_at_the_provider_deadline() {
        let workspace = tempfile::tempdir().expect("workspace");
        let executor = Arc::new(crate::tools::ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let mut config = crate::agent::AgentConfig::default();
        config.llm_api_timeout_ms = Some(80);
        config.planning_mode = crate::prompts::PlanningMode::Disabled;
        let agent = crate::agent::AgentLoop::new(
            Arc::new(HangClient),
            Arc::clone(&executor),
            crate::tools::ToolContext::new(workspace.path().to_path_buf()),
            config,
        );
        let surface = SessionSurface {
            agent,
            session_id: "timeout-session".into(),
            checkpoint: None,
            events: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            transcript: Arc::new(Mutex::new(Vec::new())),
            usage: Arc::new(Mutex::new(crate::llm::TokenUsage::default())),
            confirmation: None,
            run_store: None,
            run_id: None,
            ledger: Arc::new(Mutex::new(crate::harness_loop::MutationLedger::default())),
            reports: Arc::new(Mutex::new(Vec::new())),
            run_control: None,
            harness: None,
        };
        let run = FactRun::open_projected(
            workspace.path(),
            "timeout-session",
            Arc::new(HangClient),
            executor,
            PermissionPolicy::new().allow("*"),
            &[],
            4,
            surface,
        )
        .expect("fact run");
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(3), run.user_text("hello"))
                .await
                .expect("provider deadline did not stop the model call");
        let Err(error) = result else {
            panic!("a hanging model must not settle");
        };
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("timed out"),
            "expected the provider deadline, got {rendered}"
        );
    }

    #[test]
    fn fact_log_plan_guardrail_denies_a_write_the_default_policy_would_ask() {
        let policy = PermissionPolicy::new();
        let checker = crate::permissions::InteractiveToolGuardrail::for_mode("plan");
        let write_args = serde_json::json!({
            "file_path": "hello.txt",
            "content": "hello"
        });
        assert_eq!(
            execution_permission(&policy, Some(&checker), "write", &write_args),
            PermissionDecision::Deny
        );
        assert_eq!(
            execution_permission(
                &policy,
                Some(&checker),
                "read",
                &serde_json::json!({ "file_path": "hello.txt" })
            ),
            PermissionDecision::Allow
        );
        let yolo = PermissionPolicy::new().allow_yolo_lanes([SessionLane::Execute]);
        let asking = PermissionPolicy::new();
        assert_eq!(
            execution_permission(&yolo, Some(&asking), "bash", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        let denied = PermissionPolicy::new().deny("write(*)");
        let allowing = PermissionPolicy::new().allow("write(*)");
        assert_eq!(
            execution_permission(&denied, Some(&allowing), "write", &write_args),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn fact_log_yolo_lane_is_allow_not_a_second_policy() {
        let policy = PermissionPolicy::new().allow_yolo_lanes([SessionLane::Execute]);
        assert!(!project_needs_confirmation(&policy, "bash"));
        assert!(!project_needs_confirmation(&policy, "write"));
        assert!(!project_needs_confirmation(&policy, "unknown_tool"));
        assert!(project_needs_confirmation(&policy, "read"));
        assert!(project_needs_confirmation(&policy, "search"));
        let only_read = PermissionPolicy::new().allow("read(*)");
        assert!(project_needs_confirmation(&only_read, "bash"));
        let denied = PermissionPolicy::new()
            .deny("bash(*)")
            .allow_yolo_lanes([SessionLane::Execute]);
        assert_eq!(
            denied.check("bash", &serde_json::json!({})),
            PermissionDecision::Deny
        );
        assert_eq!(
            denied.check("write", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        assert!(!crate::hitl::ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Execute])
            .is_yolo("bash"));
    }

    #[tokio::test]
    async fn fact_log_question_cap_rejects_before_a_fourth_fact() {
        let questions = (1..=4)
            .map(|index| {
                Ok(ModelDecision::Question {
                    question_id: format!("q{index}"),
                    question: format!("q{index}?"),
                    allow_free_text: false,
                    options: vec!["a".into()],
                })
            })
            .collect();
        let (_dir, fact, _, _, _) = run_with(questions, None, 4, false);
        fact.user_text("one").await.unwrap();
        fact.answer("a").await.unwrap();
        fact.answer("b").await.unwrap();
        let rejected = match fact.answer("c").await {
            Ok(_) => panic!("fourth question must fail before a model turn"),
            Err(error) => error,
        };
        let rendered = format!("{rejected:#}");
        assert!(
            rendered.contains("ask_user question cap exceeded"),
            "{rendered}"
        );
        let facts = fact.read_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .filter(|fact| fact.kind == "model.turn")
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn fact_log_confirm_is_true_only_for_the_pending_tool() {
        let (_dir, fact, _, tools, _) = run_with(
            vec![Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: true,
                    text: None,
                    reasoning: None,
                },
            })],
            None,
            4,
            false,
        );
        fact.user_text("read it").await.unwrap();
        assert!(!fact.confirm_if_pending("other", true).await.unwrap());
        assert!(fact
            .read_facts()
            .unwrap()
            .iter()
            .all(|fact| fact.kind != "confirmation.answered"));
        assert!(fact.confirm_if_pending("t1", false).await.unwrap());
        assert_eq!(tools.load(Ordering::SeqCst), 0);
        assert_eq!(
            fact.read_facts()
                .unwrap()
                .iter()
                .filter(|fact| fact.kind == "confirmation.answered")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn fact_log_history_seeds_a_caused_model_turn_and_one_new_call() {
        let (_dir, fact, calls, _, _) = run_with(
            vec![Ok(ModelDecision::Text {
                text: "next".into(),
            })],
            None,
            4,
            false,
        );
        fact.seed_history(&[Message::user("old"), Message::assistant("stored")])
            .unwrap();
        let settled = fact.user_text("new").await.unwrap();
        assert_eq!(settled.view.assistant.as_deref(), Some("next"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let facts = fact.read_facts().unwrap();
        assert_eq!(
            facts
                .iter()
                .filter(|fact| fact.kind == "user.message")
                .count(),
            2
        );
        let stored = facts
            .iter()
            .find(|fact| fact.cause.as_deref() == Some("infer:1:0"))
            .expect("caused history turn");
        assert_eq!(stored.payload["text"], "stored");
        assert_eq!(
            facts
                .iter()
                .filter(|fact| fact.kind == "model.turn")
                .count(),
            2
        );
    }

    fn tool_use(name: &str, id: &str, args: serde_json::Value) -> Message {
        Message {
            role: "assistant".into(),
            content: vec![crate::llm::ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: args,
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Product,
        }
    }

    struct SeqClient {
        calls: Arc<AtomicUsize>,
        tools: Mutex<Vec<Vec<String>>>,
        seen: Mutex<Vec<Vec<Message>>>,
        responses: Mutex<Vec<Message>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for SeqClient {
        async fn complete(
            &self,
            messages: &[Message],
            _system: Option<&str>,
            tools: &[crate::llm::ToolDefinition],
        ) -> Result<crate::llm::LlmResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(messages.to_vec());
            self.tools
                .lock()
                .unwrap()
                .push(tools.iter().map(|tool| tool.name.clone()).collect());
            let message = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Message::assistant("done"));
            Ok(crate::llm::LlmResponse {
                message,
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
            _tools: &[crate::llm::ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            anyhow::bail!("unused")
        }
    }

    fn open_live(
        responses: Vec<Message>,
        policy: PermissionPolicy,
        tool_name: &str,
    ) -> (tempfile::TempDir, FactRun, Arc<AtomicUsize>, Arc<SeqClient>) {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let client = Arc::new(SeqClient {
            calls: Arc::clone(&calls),
            tools: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into_iter().rev().collect()),
        });
        let (completion, _) = LiveCompletion::new(client.clone(), policy, Vec::new());
        let config = HarnessConfig::new(
            4,
            1_000_000,
            16,
            1,
            Vec::new(),
            vec![ToolSpec {
                name: tool_name.into(),
                description: "tool".into(),
            }],
        )
        .unwrap();
        let fact = FactRun::open(
            dir.path(),
            Arc::new(completion),
            Arc::new(ScriptTools {
                calls: Arc::new(AtomicUsize::new(0)),
                fail_first: AtomicUsize::new(0),
            }),
            config,
            format!("live-{}", calls.as_ptr() as usize),
        )
        .unwrap();
        (dir, fact, calls, client)
    }

    #[tokio::test]
    async fn fact_log_live_completion_stores_a_tool_call_and_passes_tools() {
        let (_dir, fact, calls, client) = open_live(
            vec![tool_use(
                "bash",
                "call-1",
                serde_json::json!({"command": "ls"}),
            )],
            PermissionPolicy::new(),
            "bash",
        );
        let settled = fact.user_text("list").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Confirm);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.tools.lock().unwrap().as_slice(), &[vec!["bash"]]);
        let turn = fact
            .read_facts()
            .unwrap()
            .into_iter()
            .find(|fact| fact.kind == "model.turn")
            .expect("model turn");
        assert_eq!(turn.payload["kind"], "tool");
        assert_eq!(turn.payload["call"]["needs_confirmation"], true);
    }

    #[tokio::test]
    async fn fact_log_live_completion_yolo_lane_runs_the_tool() {
        let (_dir, fact, _, client) = open_live(
            vec![
                tool_use("bash", "call-1", serde_json::json!({})),
                Message::assistant("done"),
            ],
            PermissionPolicy::new().allow_yolo_lanes([SessionLane::Execute]),
            "bash",
        );
        let settled = fact.user_text("list").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Done);
        assert!(settled.log.iter().any(|fact| fact.kind == "tool.result"));
        assert!(!settled
            .log
            .iter()
            .any(|fact| fact.kind == "confirmation.answered"));
        assert!(client
            .tools
            .lock()
            .unwrap()
            .iter()
            .any(|tools| !tools.is_empty()));
    }

    #[tokio::test]
    async fn fact_log_live_completion_ask_user_parks_with_options() {
        let (_dir, fact, _, _) = open_live(
            vec![tool_use(
                "ask_user",
                "q-live",
                serde_json::json!({
                    "question": "Which?",
                    "options": ["left", "right"],
                    "allow_free_text": true
                }),
            )],
            PermissionPolicy::new(),
            "ask_user",
        );
        let settled = fact.user_text("ask").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Question);
        let question = settled.view.pending_question.expect("question");
        assert!(question.allow_free_text);
        assert_eq!(
            question.options,
            vec!["left".to_string(), "right".to_string()]
        );
    }

    #[tokio::test]
    async fn fact_log_sessions_do_not_share_a_thread() {
        let workspace = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let client = Arc::new(SeqClient {
            calls: Arc::clone(&calls),
            tools: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(vec![Message::assistant("bee"), Message::assistant("aye")]),
        });
        let executor = Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().to_string(),
        ));
        let first = FactRun::open_session(
            workspace.path(),
            "session-a",
            client.clone(),
            Arc::clone(&executor),
            PermissionPolicy::new(),
            &[],
            4,
        )
        .unwrap();
        let second = FactRun::open_session(
            workspace.path(),
            "session-b",
            client,
            executor,
            PermissionPolicy::new(),
            &[],
            4,
        )
        .unwrap();
        first.user_text("from-a").await.unwrap();
        second.user_text("from-b").await.unwrap();
        let facts_a = read_workspace_facts(workspace.path(), "session-a").unwrap();
        let facts_b = read_workspace_facts(workspace.path(), "session-b").unwrap();
        assert!(facts_a.iter().any(|fact| fact.payload["text"] == "from-a"));
        assert!(facts_b.iter().any(|fact| fact.payload["text"] == "from-b"));
        assert!(facts_a.iter().all(|fact| fact.payload["text"] != "from-b"));
        assert!(facts_b.iter().all(|fact| fact.payload["text"] != "from-a"));
    }

    fn dsml_bash(command: &str) -> String {
        let ns = "\u{FF5C}DSML\u{FF5C}";
        format!(
            "<{ns}invoke name=\"bash\"><{ns}parameter name=\"command\" string=\"true\">{command}</{ns}parameter></{ns}invoke>"
        )
    }

    #[tokio::test]
    async fn fact_log_leaked_dsml_text_is_a_tool_decision() {
        let (_dir, fact, _, _) = open_live(
            vec![Message::assistant(&dsml_bash("pwd"))],
            PermissionPolicy::new(),
            "bash",
        );
        let settled = fact.user_text("list").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Confirm);
        let turn = settled
            .log
            .iter()
            .find(|fact| fact.kind == "model.turn")
            .expect("model turn");
        assert_eq!(turn.payload["kind"], "tool");
        assert_eq!(turn.payload["call"]["name"], "bash");
        assert_eq!(turn.payload["call"]["args"]["command"], "pwd");
    }

    #[tokio::test]
    async fn fact_log_leaked_dsml_reasoning_is_a_tool_decision() {
        let markup = dsml_bash("pwd");
        let (_dir, fact, _, _) = open_live(
            vec![Message {
                role: "assistant".into(),
                content: Vec::new(),
                reasoning_content: Some(markup),
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            }],
            PermissionPolicy::new(),
            "bash",
        );
        let settled = fact.user_text("list").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Confirm);
        let turn = settled
            .log
            .iter()
            .find(|fact| fact.kind == "model.turn")
            .expect("model turn");
        assert_eq!(turn.payload["call"]["name"], "bash");
        assert_eq!(turn.payload["call"]["args"]["command"], "pwd");
    }

    #[tokio::test]
    async fn fact_log_follow_up_sends_the_recorded_tool_call() {
        let workspace = tempfile::tempdir().unwrap();
        let log = log_dir(workspace.path());
        std::fs::create_dir_all(&log).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let client = Arc::new(SeqClient {
            calls: Arc::clone(&calls),
            tools: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(vec![Message::assistant("done"), {
                let mut planned = tool_use(
                    "read",
                    "call-read",
                    serde_json::json!({"file_path": "note.txt"}),
                );
                planned.content.insert(
                    0,
                    crate::llm::ContentBlock::Text {
                        text: "Next, rerun the test.".into(),
                    },
                );
                planned.reasoning_content = Some("the stats file is still wrong".into());
                planned
            }]),
        });
        let executor = Arc::new(crate::tools::ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let mut config = crate::agent::AgentConfig::default();
        config.planning_mode = crate::prompts::PlanningMode::Disabled;
        let agent = crate::agent::AgentLoop::new(
            Arc::new(SeqClient {
                calls: Arc::new(AtomicUsize::new(0)),
                tools: Mutex::new(Vec::new()),
                seen: Mutex::new(Vec::new()),
                responses: Mutex::new(Vec::new()),
            }),
            executor,
            crate::tools::ToolContext::new(workspace.path().to_path_buf()),
            config,
        );
        let surface = SessionSurface {
            agent,
            session_id: THREAD.into(),
            checkpoint: None,
            events: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            transcript: Arc::new(Mutex::new(Vec::new())),
            usage: Arc::new(Mutex::new(crate::llm::TokenUsage::default())),
            confirmation: None,
            run_store: None,
            run_id: None,
            ledger: Arc::new(Mutex::new(crate::harness_loop::MutationLedger::default())),
            reports: Arc::new(Mutex::new(Vec::new())),
            run_control: None,
            harness: None,
        };
        let (completion, _) = LiveCompletion::new(
            client.clone(),
            PermissionPolicy::new().allow("*"),
            Vec::new(),
        );
        let completion = completion.with_surface(surface);
        let fact = FactRun::open(
            log,
            Arc::new(completion),
            Arc::new(ScriptTools {
                calls: Arc::new(AtomicUsize::new(0)),
                fail_first: AtomicUsize::new(0),
            }),
            HarnessConfig::new(
                4,
                1_000_000,
                16,
                1,
                Vec::new(),
                vec![ToolSpec {
                    name: "read".into(),
                    description: "Read".into(),
                }],
            )
            .unwrap(),
            "recorded-tool",
        )
        .unwrap();
        let settled = fact.user_text("read the note").await.unwrap();
        assert_eq!(settled.view.phase, CodingPhase::Done);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let seen = client.seen.lock().unwrap();
        let follow = seen.last().expect("follow-up completion");
        let recorded = follow
            .iter()
            .find_map(|message| {
                let calls = message.tool_calls();
                if calls.is_empty() {
                    None
                } else {
                    Some(calls)
                }
            })
            .expect("recorded tool call");
        assert_eq!(recorded[0].id, "call-read");
        assert_eq!(recorded[0].name, "read");
        assert_eq!(recorded[0].args["file_path"], "note.txt");
        let replay = follow
            .iter()
            .find(|message| !message.tool_calls().is_empty())
            .expect("assistant tool message");
        assert!(replay.text().contains("Next, rerun the test."));
        assert_eq!(
            replay.reasoning_content.as_deref(),
            Some("the stats file is still wrong")
        );
        assert!(follow.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    crate::llm::ContentBlock::ToolResult { tool_use_id, .. }
                        if tool_use_id == "call-read"
                )
            })
        }));
    }

    #[test]
    fn fact_log_recorded_tool_calls_start_after_the_latest_compaction() {
        use a3s_effect::LogStore;
        let workspace = tempfile::tempdir().unwrap();
        let dir = log_dir(workspace.path());
        std::fs::create_dir_all(&dir).unwrap();
        let log = a3s_effect::FileLog::open(&dir).unwrap();
        let tool = |id: &str, name: &str| {
            serde_json::json!({
                "kind": "tool",
                "call": {
                    "id": id,
                    "name": name,
                    "args": {"file_path": name},
                    "needs_confirmation": false
                }
            })
        };
        let append = |key: &str, kind: &str, payload: serde_json::Value| {
            log.append(
                THREAD,
                &[a3s_effect::NewFact {
                    kind: kind.into(),
                    key: key.into(),
                    payload,
                }],
                None,
            )
            .unwrap();
        };
        append("m1", "model.turn", tool("old", "edit"));
        append(
            "t1",
            "tool.result",
            serde_json::json!({"toolCallId": "old", "ok": true, "output": "old"}),
        );
        append(
            "c1",
            "compaction.done",
            serde_json::json!({"summary": "earlier edit"}),
        );
        append("m2", "model.turn", tool("new", "write"));
        append(
            "t2",
            "tool.result",
            serde_json::json!({"toolCallId": "new", "ok": true, "output": "new"}),
        );
        let calls = recorded_tool_calls(workspace.path(), THREAD);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "new");
        assert_eq!(calls[0].name, "write");
    }
}
