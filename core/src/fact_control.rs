//! Fact-log control for a coding session.
//!
//! The next transition comes from [`a3s_effect::resume_coding`]. This module
//! does not keep a message loop, a confirmation timer, or a question wait
//! table. Steer is another `user.message` fact.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a3s_effect::{
    answer_fact, confirm_fact, ingest_coding_with_cancel, message_fact, resume_coding_with_cancel,
    ActorError, CodingPhase, CodingServices, CodingView, Compactor, Completion, CompletionRequest,
    Exit, FileLog, HarnessConfig, HarnessGraph, LogStore, ModelDecision, NewFact, ToolCall,
    ToolRunner, ToolSpec,
};
use anyhow::Result;

use crate::ask_user::{self, AskUserError};
use crate::llm::{Attachment, LlmClient, LlmResponse, Message, ToolDefinition};
use crate::permissions::{PermissionChecker, PermissionDecision, PermissionPolicy};
use crate::queue::SessionLane;
use crate::tools::ToolExecutor;

const THREAD: &str = "thread-1";

mod checkpoints;
mod compaction;
mod executor_tools;
mod fact_reads;
mod goal_evidence;
mod history_seed;
mod model_call;
mod model_wait;
mod receipts;
mod recovery_unproved;
mod tool_batch;
mod tool_results;
mod turn_facts;

#[cfg(test)]
use compaction::compact_handoff;
use compaction::{bounded_summary, SessionCompact, UsageStamp};
#[cfg(test)]
use executor_tools::{batch_can_run_in_parallel, is_parallel_safe_write};
use executor_tools::{is_parallel_safe_read, CheckpointState, ExecutorTools, NoopCompact};
pub(crate) use fact_reads::{
    append_compaction_done, compaction_transcript, compaction_transcript_from_facts,
    retained_transcript, tool_result_weights, ToolResultWeight,
};
use fact_reads::{exit_to_error, message_from_fact};
pub use fact_reads::{phase_name, project_needs_confirmation, read_workspace_facts};
pub(crate) use goal_evidence::{
    assess_goal_step, commit_goal_step, evidence_for_goal_step, settle_goal_slice,
};
pub(crate) use history_seed::seed_history;
use history_seed::{
    attachments_from_user_payload, emit_tool_request_bound, folded_message_with_images,
    images_for_user_line, recorded_call_from, recorded_tool_calls, recorded_tool_messages,
    repeat_note_prefix, repeated_result_note, scheduler_tool_record_indexes, user_attachment_plan,
    user_message_fact, user_message_with_images, visible_tool_output, workspace_boundary_error,
    RecordedCall,
};
pub(crate) use model_call::model_error;
#[cfg(test)]
use model_call::project_model_frame;
use model_call::{interrupted_response, model_context, model_response, record_run_event};
use model_wait::{
    await_model_task_with_idle, execution_time_left, execution_timeout, llm_idle_timeout_error,
    model_idle_sleep, AbortOnDrop,
};
pub use tool_results::admit_tool_call;
pub(crate) use tool_results::exposed_tool_specs;
#[cfg(test)]
use tool_results::read_only_evidence_exhausted;
use tool_results::{
    confirmation_parked, decision_from_response, decode_fact_tool_result, definitions_for,
    execution_permission, fact_tool_runner_value, latest_user_text, message_from_folded_tool,
    read_only_evidence_exhausted_with_calls, skill_denial, split_folded_message,
    verified_turn_text, FACT_TOOL_RESULT_KIND,
};
#[cfg(test)]
use turn_facts::append_steer_input;
pub use turn_facts::TURN_COMPLETE_FACT_KIND;
use turn_facts::{
    append_context_sections_fact, apply_run_controls, replay_unsettled_mutations_from_facts,
    surface_fact_log,
};
pub(crate) use turn_facts::{
    append_turn_complete_fact, has_unsettled_effects, replay_unsettled_mutations,
    restore_checkpoint_mutation_cache, settle_terminal_run_controls, unsettled_mutation_paths,
    CheckpointMutationCacheCoverage,
};

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
    /// The Run's shared fact-log writer. Constructed here as a default and
    /// replaced by the session constructors with the instance the run's
    /// tool contexts already hold, so actor appends and Run-owned background
    /// readers serialize through one instance mutex (a second `FileLog`
    /// handle over the same directory races duplicate seqs).
    log: Arc<FileLog>,
    thread: String,
    actor: a3s_effect::Actor<CodingServices, CodingView>,
    services: Arc<CodingServices>,
    limit: u32,
    /// Tool names exposed when this run was admitted. A later posture does not rewrite them.
    admitted_tools: Vec<String>,
    /// Copied from [`crate::meta_harness::KernelPolicy::admit`]. Hosts cannot clear it.
    completion_gate: bool,
    /// Permission policy frozen at admission, including posture allow rules.
    admitted_permission: PermissionPolicy,
    /// Digest admitted for this run. Legacy opens leave this empty.
    standing_digest: Option<String>,
    current_goal: Option<crate::goal_control::DurableGoalDefinitionV1>,
    /// Host cancel for this run. Interactive sessions share [`SessionSurface::cancel`].
    cancel: tokio_util::sync::CancellationToken,
}

/// Optional host mounts for Meta Harness admission.
#[derive(Default)]
pub struct FactRunHostMounts<'a> {
    pub compose: Option<&'a crate::meta_harness::HarnessComposeOptions>,
    pub registry: Option<&'a dyn crate::meta_harness::HostHarnessRegistry>,
    pub assembler: Option<&'a dyn crate::meta_harness::HostHarnessAssembler>,
    pub compactor: Option<Arc<dyn Compactor>>,
}

struct CappedCompletion {
    inner: Arc<dyn Completion>,
    run_id: String,
}

impl Completion for CappedCompletion {
    fn complete(
        &self,
        request: CompletionRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        let inner = Arc::clone(&self.inner);
        let run_id = self.run_id.clone();
        Box::pin(async move {
            let decision = inner.complete(request, cancel).await?;
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

    fn reported_prompt_tokens(&self) -> Option<u64> {
        self.inner.reported_prompt_tokens()
    }
}

/// One turn's recall telemetry: the recall query and the recalled items.
pub(crate) struct MemoryTelemetryBatch {
    pub(crate) query: String,
    pub(crate) items: Vec<(String, String, f32)>,
}

/// Pinned coding runtime used while the fact log chooses transitions.
///
/// Hooks, context, and delegation planning come from the admitted run. They
/// do not keep a message loop.
#[derive(Clone)]
pub(crate) struct SessionSurface {
    pub(crate) agent: crate::agent::AgentLoop,
    pub(crate) session_id: String,
    pub(crate) fact_log: Arc<Mutex<Option<Arc<FileLog>>>>,
    pub(crate) checkpoint: Option<CheckpointEmit>,
    pub(crate) events: Option<tokio::sync::mpsc::Sender<crate::agent::AgentEvent>>,
    /// Recall telemetry (memory id, content, relevance) collected by the
    /// actor's model-context assembly; the stream worker drains and emits it
    /// before `End`. The actor never sends into `events` directly.
    pub(crate) pending_memory_telemetry:
        std::sync::Arc<std::sync::Mutex<Vec<MemoryTelemetryBatch>>>,
    /// Set when this surface already streamed text deltas to the host, so the
    /// settled replay must not re-send the full assistant text.
    pub(crate) streamed_text: Arc<std::sync::atomic::AtomicBool>,
    /// Set when the harness forced this run to stop short (read-only
    /// evidence ceiling; the budget twin is read off the fact log at settle)
    /// so the settled End event can say why the turn ended.
    pub(crate) forced_stop: Arc<Mutex<Option<crate::harness_loop::ForcedStop>>>,
    pub(crate) cancel: tokio_util::sync::CancellationToken,
    pub(crate) transcript: Arc<Mutex<Vec<Message>>>,
    pub(crate) usage: Arc<Mutex<crate::llm::TokenUsage>>,
    pub(crate) confirmation: Option<Arc<dyn crate::hitl::ConfirmationProvider>>,
    pub(crate) run_store: Option<Arc<crate::run::InMemoryRunStore>>,
    pub(crate) run_id: Option<String>,
    /// Automatic tick origin for this exact Run; absent for ordinary Runs.
    pub(crate) goal_tick: Option<Box<crate::agent_api::GoalTickOrigin>>,
    pub(crate) ledger: Arc<Mutex<crate::harness_loop::MutationLedger>>,
    pub(crate) reports: Arc<Mutex<Vec<crate::verification::VerificationReport>>>,
    pub(crate) run_control: Option<Arc<crate::run_control::RunControlInbox>>,
    /// Paths targeted by this run's tool calls (bounded, newest last).
    /// Path-scoped instruction fragments inject into the next model input
    /// from these — the fact-path twin of the legacy loop's
    /// `inject_turn_scoped_evidence`.
    pub(crate) targeted_paths: Arc<Mutex<Vec<String>>>,
    pub(crate) harness: Option<crate::meta_harness::HarnessComposeOptions>,
    pub(crate) host_harness_registry:
        Option<std::sync::Arc<dyn crate::meta_harness::HostHarnessRegistry>>,
    pub(crate) host_harness_assembler:
        Option<std::sync::Arc<dyn crate::meta_harness::HostHarnessAssembler>>,
}

/// Exports one portable checkpoint after a tool result lands on the log.
#[derive(Clone)]
pub(crate) struct CheckpointEmit {
    pub(crate) sink: Arc<dyn crate::loop_checkpoint::LoopCheckpointSink>,
    pub(crate) run_id: String,
    pub(crate) session_id: String,
    pub(crate) capability_binding: Option<crate::capability::RunCapabilityBindingV1>,
}

#[derive(Clone)]
struct PreparedPrompt {
    prefix: Vec<String>,
    summary: String,
    text: String,
}

impl PreparedPrompt {
    fn capture(request: &CompletionRequest, text: String) -> Option<Self> {
        let source = request
            .messages
            .iter()
            .rposition(|line| line.starts_with("user\n"))?;
        Some(Self {
            prefix: request.messages[..=source].to_vec(),
            summary: request.summary.clone(),
            text,
        })
    }

    fn source_index(&self, request: &CompletionRequest) -> Option<usize> {
        (request.summary == self.summary && request.messages.starts_with(&self.prefix))
            .then_some(self.prefix.len().saturating_sub(1))
    }
}

pub struct LiveCompletion {
    client: Arc<dyn LlmClient>,
    calls: Arc<AtomicUsize>,
    policy: PermissionPolicy,
    /// Model-visible tool filter. A session checker wins; otherwise the policy.
    visibility: Arc<dyn crate::permissions::PermissionChecker>,
    /// Model attempts on this completion. Early returns do not increment it.
    model_turns: Arc<AtomicUsize>,
    catalog: Vec<ToolDefinition>,
    surface: Option<SessionSurface>,
    context_ready: Arc<AtomicBool>,
    cached_system: Arc<Mutex<Option<String>>>,
    cached_prompt: Arc<Mutex<Option<Arc<PreparedPrompt>>>>,
    prompt_tokens: Arc<AtomicU64>,
    standing_system: Option<String>,
    goal_context: Option<String>,
    checkpoint: Option<CheckpointState>,
    checkpoint_attempted_seq: Arc<AtomicU64>,
    original_model: Option<crate::agent_api::OriginalModelCallPort>,
    /// One completion serves one run, so every continuation segment of the
    /// run spends the session's `max_execution_time_ms` from this instant.
    started_at: std::time::Instant,
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
                visibility: Arc::new(policy.clone()),
                model_turns: Arc::new(AtomicUsize::new(0)),
                policy,
                catalog,
                surface: None,
                context_ready: Arc::new(AtomicBool::new(false)),
                cached_system: Arc::new(Mutex::new(None)),
                cached_prompt: Arc::new(Mutex::new(None)),
                prompt_tokens: Arc::new(AtomicU64::new(0)),
                standing_system: None,
                goal_context: None,
                checkpoint: None,
                checkpoint_attempted_seq: Arc::new(AtomicU64::new(0)),
                original_model: None,
                started_at: std::time::Instant::now(),
            },
            calls,
        )
    }

    pub(crate) fn with_standing(mut self, projection: String) -> Self {
        self.standing_system = Some(projection);
        self
    }

    fn with_original_model(
        mut self,
        original: Option<crate::agent_api::OriginalModelCallPort>,
    ) -> Self {
        self.original_model = original;
        self
    }

    fn with_checkpoint(mut self, checkpoint: Option<CheckpointState>) -> Self {
        self.checkpoint = checkpoint;
        self
    }

    pub(crate) fn with_surface(mut self, surface: SessionSurface) -> Self {
        self.surface = Some(surface);
        self
    }

    pub(crate) fn with_visibility(
        mut self,
        checker: Arc<dyn crate::permissions::PermissionChecker>,
    ) -> Self {
        self.visibility = checker;
        self
    }
}

impl Completion for LiveCompletion {
    fn complete(
        &self,
        request: CompletionRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let client = Arc::clone(&self.client);
        let policy = self.policy.clone();
        let visibility = Arc::clone(&self.visibility);
        let model_turns = Arc::clone(&self.model_turns);
        let catalog = self.catalog.clone();
        let surface = self.surface.clone();
        let ready_flag = Arc::clone(&self.context_ready);
        let system_slot = Arc::clone(&self.cached_system);
        let prompt_slot = Arc::clone(&self.cached_prompt);
        let prompt_tokens = Arc::clone(&self.prompt_tokens);
        let standing_system = self.standing_system.clone();
        let goal_context = self.goal_context.clone();
        let checkpoint = self.checkpoint.clone();
        let checkpoint_attempted_seq = Arc::clone(&self.checkpoint_attempted_seq);
        let original_model = self.original_model.clone();
        let started_at = self.started_at;
        Box::pin(async move {
            prompt_tokens.store(0, Ordering::SeqCst);
            if let Some(surface) = &surface {
                if let Some(checkpoint) = &checkpoint {
                    checkpoints::save_committed(surface, checkpoint, &checkpoint_attempted_seq)
                        .await?;
                } else {
                    replay_unsettled_mutations(surface);
                }
            }
            if request.messages.len() == 1 && request.messages[0].starts_with("user\n") {
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
                            also: Vec::new(),
                        });
                    }
                }
            }
            if let Some(surface) = &surface {
                if let Some(text) = verified_turn_text(surface, &request.messages) {
                    return Ok(ModelDecision::Text { text });
                }
            }
            let owns_context = standing_system.is_some() || surface.is_some();
            let mut system = if owns_context || request.system.is_empty() {
                None
            } else {
                Some(request.system.join("\n"))
            };
            let mut prompt_override = None;
            if let Some(standing) = standing_system.clone() {
                system = (!standing.is_empty()).then_some(standing);
                if let Some(surface) = &surface {
                    if ready_flag
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        let prompt = latest_user_text(&request.messages);
                        let (effective, augmented) =
                            model_context(surface, &prompt, request.messages.len()).await?;
                        prompt_override =
                            PreparedPrompt::capture(&request, effective).map(Arc::new);
                        if let Some(augmented) = augmented.filter(|text| !text.is_empty()) {
                            let base = surface.agent.admitted_system_prompt();
                            let extras = augmented
                                .strip_prefix(base.as_str())
                                .unwrap_or(augmented.as_str())
                                .trim();
                            if !extras.is_empty() {
                                system = Some(match system {
                                    Some(admitted) if !admitted.is_empty() => {
                                        format!("{admitted}\n\n{extras}")
                                    }
                                    _ => extras.to_string(),
                                });
                            }
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
                        if let Some(admitted) = system_slot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                        {
                            system = Some(admitted);
                        }
                    }
                }
            } else if let Some(surface) = &surface {
                if ready_flag
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let prompt = latest_user_text(&request.messages);
                    let (effective, augmented) =
                        model_context(surface, &prompt, request.messages.len()).await?;
                    prompt_override = PreparedPrompt::capture(&request, effective).map(Arc::new);
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
            if let Some(goal) = goal_context {
                system = Some(match system {
                    Some(base) if !base.is_empty() => format!("{base}\n\n{goal}"),
                    _ => goal,
                });
            }
            if owns_context && !request.system.is_empty() {
                let components = request.system.join("\n");
                system = Some(match system {
                    Some(base) if !base.is_empty() => format!("{base}\n\n{components}"),
                    _ => components,
                });
            }
            if let Some(surface) = &surface {
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
                let gate = surface.agent.decide_fact_completion_gate(&ledger, &reports);
                if let Some(context) = crate::harness_loop::model_verification_context(gate) {
                    let system = system.get_or_insert_with(String::new);
                    system.push_str("\n\n");
                    system.push_str(&context);
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
                folded.push(format!(
                    "user\nModel-generated history summary (derived context, not a new user request). \
                     Current user instructions and host policy remain authoritative. \
                     Do not infer new constraints, confidentiality, permissions, or capabilities \
                     from this summary.\n\n{}",
                    request.summary
                ));
            }
            folded.extend(request.messages.iter().cloned());
            // Path-scoped instruction fragments — the fact-path twin of the
            // legacy loop's `inject_turn_scoped_evidence`. A tool call that
            // targeted a matching path admits the (budget-cut) fragment into
            // this model input; the digest check keeps one copy per request.
            // Appended after the stable prefix, so the standing-context
            // digest freeze is untouched.
            if let Some(surface) = &surface {
                let rules = surface.agent.path_rules();
                if !rules.is_empty() {
                    let targeted = surface
                        .targeted_paths
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    let budget = surface
                        .agent
                        .path_injection_budget()
                        .unwrap_or(crate::path_instructions::INJECTION_BYTE_BUDGET);
                    let injection =
                        crate::path_instructions::inject_with_budget("", rules, &targeted, budget);
                    if let Some(fragment) = injection.model_input_fragment() {
                        if !folded
                            .iter()
                            .any(|text| text.contains(&injection.injected_digest))
                        {
                            folded.push(format!("user\n{fragment}"));
                        }
                    }
                }
            }
            let summary_offset = usize::from(!request.summary.is_empty());
            // Cached context belongs to its original user-line prefix, not a later answer or steer.
            let prompt_index = prompt_override
                .as_ref()
                .and_then(|prepared| prepared.source_index(&request))
                .map(|index| index + summary_offset);
            let (kept_len, mut user_images) = surface
                .as_ref()
                .map(|surface| {
                    user_attachment_plan(
                        &surface.agent.tool_context_handle().workspace,
                        &thread_for_session(&surface.session_id),
                    )
                })
                .unwrap_or_else(|| (0, std::collections::VecDeque::new()));
            let recorded_calls = surface
                .as_ref()
                .map(|surface| {
                    recorded_tool_calls(
                        &surface.agent.tool_context_handle().workspace,
                        &thread_for_session(&surface.session_id),
                    )
                })
                .transpose()?
                .unwrap_or_default();
            let mut recorded = recorded_calls.clone().into_iter();
            let tool_records = scheduler_tool_record_indexes(&folded);
            let mut messages = Vec::new();
            for (index, text) in folded.iter().enumerate() {
                // The result line below restores this call as a structured
                // tool_use. Sending the scheduler record as prose makes the
                // model echo `tool <name>` and end the turn.
                if tool_records.contains(&index) {
                    continue;
                }
                let (role, body) = split_folded_message(text);
                let body = if Some(index) == prompt_index {
                    prompt_override
                        .as_ref()
                        .map(|prepared| prepared.text.clone())
                        .unwrap_or(body)
                } else {
                    body
                };
                if role == "tool" || role == "tool-error" {
                    messages.extend(recorded_tool_messages(
                        &mut recorded,
                        &body,
                        role == "tool-error",
                    )?);
                } else {
                    let images = if text.starts_with("user\n") {
                        images_for_user_line(index, summary_offset, kept_len, &mut user_images)
                    } else {
                        Vec::new()
                    };
                    messages.push(folded_message_with_images(role, &body, images));
                }
            }
            if recorded.next().is_some() {
                return Err(ActorError::Schema(
                    "tool history has calls absent from the folded transcript".into(),
                ));
            }
            if let Some(surface) = &surface {
                apply_run_controls(surface, &mut messages).await?;
                if surface.cancel.is_cancelled() {
                    return Ok(ModelDecision::Text {
                        text: "(Response interrupted by the user.)".into(),
                    });
                }
            }
            let read_only_final = surface.as_ref().is_some_and(|surface| {
                let ledger = surface
                    .ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // A second iterator pairs tool lines with recorded call
                // names in the same wire order the conversion above uses.
                let mut recorded = recorded_calls.into_iter();
                read_only_evidence_exhausted_with_calls(&ledger, &request.messages, &mut recorded)
            });
            if read_only_final {
                if let Some(surface) = surface.as_ref() {
                    *surface
                        .forced_stop
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(crate::harness_loop::ForcedStop::ReadOnlyEvidence);
                }
                let line = crate::agent::READ_ONLY_EVIDENCE_FINALIZATION;
                system = Some(match system.take() {
                    Some(existing) if !existing.is_empty() => format!("{existing}\n\n{line}"),
                    _ => line.to_string(),
                });
            }
            // The standing system prompt is cached on the first call. Restate
            // the latest user script on every call, and do not write it back
            // into the cache.
            let user_text = latest_user_text(&request.messages);
            let published = surface
                .as_ref()
                .and_then(|surface| surface.agent.tool_context_handle().reply_language());
            let (next_system, turn_language) = crate::prompts::apply_turn_output_language(
                system.take(),
                &user_text,
                published.as_deref(),
                false,
            );
            system = next_system;
            if let (Some(language), Some(surface)) = (&turn_language, surface.as_ref()) {
                surface
                    .agent
                    .tool_context_handle()
                    .set_reply_language(Some(language));
            }
            let tools = if read_only_final {
                Vec::new()
            } else {
                definitions_for(&catalog, &request.tools)
                    .into_iter()
                    .filter(|tool| visibility.expose_to_model(&tool.name))
                    .collect()
            };
            let execution_limit = surface
                .as_ref()
                .and_then(|surface| surface.agent.max_execution_time());
            let time_left = execution_time_left(started_at, execution_limit)?;
            let turn = model_turns.fetch_add(1, Ordering::SeqCst).saturating_add(1);
            if let Some(surface) = &surface {
                record_run_event(surface, crate::agent::AgentEvent::TurnStart { turn }).await;
            }
            let chat = crate::telemetry::gen_ai::chat_span();
            let call = model_response(
                Arc::clone(&client),
                messages.clone(),
                system.clone(),
                tools,
                surface.clone(),
                cancel.clone(),
                original_model.clone(),
            );
            let call = tracing::Instrument::instrument(call, chat.clone());
            let response = match time_left {
                Some(left) => tokio::time::timeout(left, call)
                    .await
                    .unwrap_or_else(|_| Err(execution_timeout(started_at, execution_limit))),
                None => call.await,
            };
            crate::telemetry::gen_ai::record_chat_outcome(&chat, &response);
            let response = match response {
                Ok(response) => {
                    if let Some(surface) = &surface {
                        record_run_event(
                            surface,
                            crate::agent::AgentEvent::TurnEnd {
                                turn,
                                usage: response.usage.clone(),
                            },
                        )
                        .await;
                    }
                    response
                }
                Err(error) => {
                    if let Some(surface) = &surface {
                        record_run_event(
                            surface,
                            crate::agent::AgentEvent::TurnEnd {
                                turn,
                                usage: crate::llm::TokenUsage::default(),
                            },
                        )
                        .await;
                    }
                    return Err(error);
                }
            };
            if let Some(surface) = &surface {
                if surface.cancel.is_cancelled() {
                    apply_run_controls(surface, &mut messages).await?;
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
            prompt_tokens.store(response.usage.prompt_tokens as u64, Ordering::SeqCst);
            let decision = decision_from_response(&response, &policy, surface.as_ref()).await;
            // An empty tool list is advisory. A provider that still returns a
            // call must not open another evidence round.
            if read_only_final {
                if let Some(surface) = surface.as_ref() {
                    *surface
                        .forced_stop
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(crate::harness_loop::ForcedStop::ReadOnlyEvidence);
                }
                if let ModelDecision::Tool { call, .. } = decision {
                    let text = call
                        .text
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| {
                            crate::agent::READ_ONLY_EVIDENCE_FINALIZATION.to_string()
                        });
                    return Ok(ModelDecision::Text { text });
                }
            }
            Ok(decision)
        })
    }

    fn reported_prompt_tokens(&self) -> Option<u64> {
        let tokens = self.prompt_tokens.load(Ordering::SeqCst);
        (tokens > 0).then_some(tokens)
    }
}

#[cfg(test)]
fn log_dir(workspace: &Path) -> PathBuf {
    crate::session_folder::legacy_effect_log_dir(workspace)
}

/// Tool budget for one fact-log turn.
///
/// `/effort` sets `max_tool_rounds`. That number is both the deny budget and
/// the point where the next completion is asked to finish without tools. A
/// fixed budget of 8 ended the turn while tools were still offered. The step
/// limit covers an infer and a tool result for every round, plus the prompt,
/// compaction, and a closing answer.
pub(crate) fn lease_generation_specs(
    mut specs: Vec<ToolSpec>,
    executor: &ToolExecutor,
) -> Vec<ToolSpec> {
    let Some(generation) = executor.use_generation() else {
        return specs;
    };
    specs.retain(|spec| generation.allows_catalog_tool(&spec.name));
    for name in generation.skills().iter().chain(generation.runtime_tasks()) {
        if specs.iter().any(|spec| spec.name == *name) {
            continue;
        }
        specs.push(ToolSpec {
            name: name.clone(),
            description: "Projected from the leased Use generation.".into(),
        });
    }
    specs
}

fn session_harness_config(
    max_tool_rounds: usize,
    compact_after_chars: usize,
    specs: Vec<ToolSpec>,
) -> Result<HarnessConfig> {
    session_harness_config_with(max_tool_rounds, compact_after_chars, specs, None, None)
}

pub(crate) fn session_harness_config_with(
    max_tool_rounds: usize,
    compact_after_chars: usize,
    specs: Vec<ToolSpec>,
    step_limit: Option<u32>,
    model_attempts: Option<u32>,
) -> Result<HarnessConfig> {
    let rounds = u32::try_from(max_tool_rounds).unwrap_or(u32::MAX).max(1);
    let derived_steps = rounds.saturating_mul(4).max(32);
    let step_limit = step_limit.unwrap_or(derived_steps);
    let model_attempts = model_attempts.unwrap_or(2);
    Ok(HarnessConfig::new(
        rounds,
        compact_after_chars,
        step_limit,
        model_attempts,
        Vec::new(),
        specs,
    )
    .map_err(|error| anyhow::anyhow!(error))?
    .with_tool_round_cap(rounds)
    .with_model_retry_delay(crate::retry::RetryConfig::default().delay_for_attempt(0)))
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
        Self::open_composed_with_hosts(
            dir,
            completion,
            tools,
            config,
            run_id,
            FactRunHostMounts {
                compose,
                registry: None,
                assembler: None,
                compactor: None,
            },
        )
    }

    /// Admit compose with optional host registry / full assembler.
    pub fn open_composed_with_hosts(
        dir: impl Into<PathBuf>,
        completion: Arc<dyn Completion>,
        tools: Arc<dyn ToolRunner>,
        config: HarnessConfig,
        run_id: impl Into<String>,
        hosts: FactRunHostMounts<'_>,
    ) -> Result<Self> {
        let step_limit = config.step_limit();
        let graph = Self::admit_graph(config, &hosts)?;
        let compactor = hosts.compactor.clone();
        Self::open_with_graph(dir, completion, tools, step_limit, run_id, graph, compactor)
    }

    fn admit_graph(config: HarnessConfig, hosts: &FactRunHostMounts<'_>) -> Result<HarnessGraph> {
        if let Some(assembler) = hosts.assembler {
            let graph = assembler.assemble(config)?;
            let _kernel = crate::meta_harness::KernelPolicy::default().admit();
            Ok(graph)
        } else {
            let (graph, _kernel) = crate::meta_harness::admit_from_compose_with_registry(
                hosts.compose,
                hosts.registry,
                config,
            )?;
            Ok(graph)
        }
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
        compactor: Option<Arc<dyn Compactor>>,
    ) -> Result<Self> {
        let kernel = crate::meta_harness::KernelPolicy::default().admit();
        let dir = dir.into();
        let log = Arc::new(FileLog::open(&dir).map_err(|error| anyhow::anyhow!(error))?);
        let capped = Arc::new(CappedCompletion {
            inner: completion,
            run_id: run_id.into(),
        });
        let compactor = compactor.unwrap_or_else(|| Arc::new(NoopCompact) as Arc<dyn Compactor>);
        let services = Arc::new(CodingServices {
            completion: capped,
            tools,
            compactor,
        });
        Ok(Self {
            dir,
            log,
            thread: THREAD.to_string(),
            actor: graph.into_actor(),
            services,
            limit: step_limit,
            admitted_tools: Vec::new(),
            completion_gate: kernel.completion_gate,
            admitted_permission: PermissionPolicy::default(),
            standing_digest: None,
            current_goal: None,
            cancel: tokio_util::sync::CancellationToken::new(),
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
        let permission = permission.allow_yolo_lanes(yolo_lanes.iter().copied());
        let specs = lease_generation_specs(
            exposed_tool_specs(&catalog, |name| permission.expose_to_model(name)),
            executor.as_ref(),
        );
        let admitted_tools: Vec<String> = specs.iter().map(|spec| spec.name.clone()).collect();
        let (completion, _) = LiveCompletion::new(client, permission.clone(), catalog);
        let config = session_harness_config(max_tool_rounds, 1_000_000, specs)?;
        let effect_dir = crate::session_folder::effect_log_dir(workspace, session_id);
        let thread = thread_for_session(session_id);
        // Standing admission, actor and background readers share the Run's handle.
        let shared_log =
            Arc::new(FileLog::open(&effect_dir).map_err(|error| anyhow::anyhow!(error))?);
        let facts = shared_log
            .read(&thread)
            .map_err(|error| anyhow::anyhow!(error))?;
        crate::goal_control::refuse_unbound_goal_execution(&facts)?;
        if crate::work_policy::prepare_policy(&facts, &thread, None)?
            .policy
            .is_some()
        {
            anyhow::bail!("work_policy.invalid: bound policy requires the original session owner");
        }
        let admission = crate::standing_context::admit(
            shared_log.as_ref(),
            &facts,
            workspace,
            &thread,
            None,
            &[],
            &admitted_tools,
            config.budget(),
            config.step_limit(),
            config.model_attempts(),
        )?;
        let standing_digest = admission.as_ref().map(|item| item.digest.clone());
        let completion = match admission {
            Some(admitted) => completion.with_standing(admitted.projection),
            None => completion,
        };
        let context = executor.registry().context();
        let context = context.with_fact_log(Arc::clone(&shared_log));
        let mut run = Self::open(
            effect_dir,
            Arc::new(completion),
            Arc::new(ExecutorTools {
                capability_operation: None,
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
                targeted_paths: None,
                pre_tool_hook_resolved: false,
                repeats: Arc::default(),
            }),
            config,
            session_id,
        )?;
        run.log = shared_log;
        run.thread = thread;
        run.admitted_tools = admitted_tools;
        run.admitted_permission = permission;
        run.standing_digest = standing_digest;
        Ok(run)
    }

    /// Tool catalog frozen at admission.
    pub fn admitted_tool_catalog(&self) -> &[String] {
        &self.admitted_tools
    }

    /// Step limit frozen at admission.
    pub fn admitted_step_limit(&self) -> u32 {
        self.limit
    }

    /// Completion gate required for this run. Admission cannot turn it off.
    pub fn completion_gate_required(&self) -> bool {
        self.completion_gate
    }

    /// Permission policy frozen at admission.
    pub fn admitted_permission(&self) -> &PermissionPolicy {
        &self.admitted_permission
    }

    /// Digest admitted for this run. Legacy logs leave this empty.
    pub fn standing_context_digest(&self) -> Option<&str> {
        self.standing_digest.as_deref()
    }

    /// Bind the host token tests cancel. [`Self::open_projected`] copies the session token.
    #[cfg(test)]
    pub(crate) fn with_host_cancel(mut self, cancel: tokio_util::sync::CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    #[cfg(test)]
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
        Self::open_projected_with_original_model(
            workspace,
            session_id,
            client,
            executor,
            (permission, yolo_lanes, max_tool_rounds),
            surface,
            None,
        )
    }

    pub(crate) fn open_projected_with_original_model(
        workspace: &Path,
        session_id: &str,
        client: Arc<dyn LlmClient>,
        executor: Arc<ToolExecutor>,
        execution_policy: (PermissionPolicy, &[SessionLane], usize),
        surface: SessionSurface,
        original_model: Option<crate::agent_api::OriginalModelCallPort>,
    ) -> Result<Self> {
        let (permission, yolo_lanes, max_tool_rounds) = execution_policy;
        let mut surface = surface;
        let catalog = executor.definitions();
        let permission = permission.allow_yolo_lanes(yolo_lanes.iter().copied());
        let checker = surface.agent.permission_checker();
        let specs = lease_generation_specs(
            exposed_tool_specs(&catalog, |name| match &checker {
                Some(checker) => checker.expose_to_model(name),
                None => permission.expose_to_model(name),
            }),
            executor.as_ref(),
        );
        let admitted_tools: Vec<String> = specs.iter().map(|spec| spec.name.clone()).collect();
        let admitted_permission = permission.clone();
        let shared_log = surface_fact_log(&surface).map_err(|error| anyhow::anyhow!(error))?;
        let writer = shared_log
            .try_writer_fence()
            .map_err(|error| anyhow::anyhow!(error))?;
        let thread = thread_for_session(session_id);
        let facts = shared_log
            .read(&thread)
            .map_err(|error| anyhow::anyhow!(error))?;
        surface.agent.validate_durable_source(session_id, &facts)?;
        let policy =
            crate::work_policy::prepare_policy(&facts, &thread, surface.agent.work_policy())?;
        let policy_digest = policy
            .policy
            .as_ref()
            .map(crate::WorkPolicyV1::digest)
            .transpose()?;
        let tool_context = executor.registry().context();
        let workspace_ref = tool_context.workspace_services.workspace_ref();
        let continuation = match surface.goal_tick.as_deref() {
            Some((command, source)) => {
                // Re-validate the tick under this writer; a preview is never authority.
                let decision = crate::goal_control::preview_tick(
                    &facts,
                    session_id,
                    workspace_ref,
                    policy_digest.as_deref(),
                    command,
                    false,
                    Some(source),
                )?;
                if decision != crate::goal_control::GoalTickDecisionV1::Advance
                    || surface.run_id.as_deref() != Some(command.operation_id.as_str())
                {
                    anyhow::bail!(
                        "goal.conflict: automatic tick no longer advances ({})",
                        decision.as_str()
                    );
                }
                Some(source)
            }
            None => None,
        };
        let mut goal = crate::goal_control::prepare_run(
            &facts,
            session_id,
            workspace_ref,
            policy_digest.as_deref(),
            surface.run_id.as_deref(),
            continuation,
        )?;
        if goal.is_some()
            && !surface.run_id.as_deref().is_some_and(|run_id| {
                original_model.as_ref().is_some_and(|original| {
                    original.admits_original_run(&shared_log, session_id, run_id)
                })
            })
        {
            anyhow::bail!("goal.host_driver_conflict: goal execution requires the actual original Run and log");
        }
        if policy.policy.is_some()
            && !original_model
                .as_ref()
                .is_some_and(|original| original.admits_original_log(&shared_log, session_id))
        {
            anyhow::bail!(
                "work_policy.invalid: bound policy requires the original admitted worker and log"
            );
        }
        surface.agent = surface.agent.with_work_policy(policy.policy.clone());
        let (completion, _) = LiveCompletion::new(Arc::clone(&client), permission.clone(), catalog);
        let completion = match &checker {
            Some(checker) => completion.with_visibility(Arc::clone(checker)),
            None => completion,
        };
        let events = surface.events.clone();
        let run_store = surface.run_store.clone();
        let run_id = surface.run_id.clone();
        let ledger = Some(Arc::clone(&surface.ledger));
        let reports = Some(Arc::clone(&surface.reports));
        let agent = surface.agent.clone();
        let context = surface
            .agent
            .tool_context_handle()
            .with_cancellation(surface.cancel.clone());
        let completion = completion
            .with_surface(surface.clone())
            .with_original_model(original_model);
        let limits = surface.harness.as_ref();
        let mut config = session_harness_config_with(
            max_tool_rounds,
            agent.fact_compact_after_chars(),
            specs,
            limits.and_then(|options| options.step_limit),
            limits.and_then(|options| options.model_attempts),
        )?;
        if let Some(after_tokens) = agent.fact_compact_after_tokens() {
            config = config.with_token_compact(after_tokens, agent.fact_compact_keep_tokens());
        }
        config = config.with_duplicate_threshold(agent.duplicate_tool_threshold());
        let budget = config.budget();
        let step_limit = config.step_limit();
        let model_attempts = config.model_attempts();
        let graph = Self::admit_graph(
            config,
            &FactRunHostMounts {
                compose: surface.harness.as_ref(),
                registry: surface.host_harness_registry.as_deref(),
                assembler: surface.host_harness_assembler.as_deref(),
                compactor: None,
            },
        )?;
        let agent_system = agent.admitted_system_prompt();
        let effect_dir = crate::session_folder::effect_log_dir(workspace, session_id);
        // One shared writer for the actor and any Run-owned background
        // readers this run admits (WC-BG1); see `FactRun::log`.
        let context = context.with_fact_log(Arc::clone(&shared_log));
        let admission = crate::standing_context::admit(
            shared_log.as_ref(),
            &facts,
            workspace,
            &thread,
            Some(agent_system.as_str()),
            &[],
            &admitted_tools,
            budget,
            step_limit,
            model_attempts,
        )?;
        crate::work_policy::commit_policy(&shared_log, &writer, &thread, &policy)?;
        if let Some(prepared) = &mut goal {
            if let Some(pending) = prepared.pending.take() {
                let written = shared_log.append(&thread, &[pending], None)?;
                prepared.definition.goal_sequence = written
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("original Goal Run binding was not retained"))?
                    .seq;
            }
        }
        let current_goal = goal.map(|prepared| prepared.definition);
        let standing_digest = admission.as_ref().map(|item| item.digest.clone());
        let mut completion = match admission {
            Some(admitted) => completion.with_standing(admitted.projection),
            None => completion,
        };
        completion.goal_context = current_goal
            .as_ref()
            .map(|goal| {
                let run_id = run_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("original Goal Run identity is absent"))?;
                crate::goal_control::render_run_context(goal, run_id).map_err(anyhow::Error::from)
            })
            .transpose()?;
        let durable_source =
            crate::store::SessionFactSourceV1::from_facts(session_id, &shared_log.read(&thread)?)?;
        let checkpoint = surface.checkpoint.map(|checkpoint| CheckpointState {
            sink: checkpoint.sink,
            run_id: checkpoint.run_id,
            session_id: checkpoint.session_id,
            capability_binding: checkpoint.capability_binding,
            standing_context_digest: standing_digest.clone(),
            durable_source: durable_source.clone(),
            turns: Arc::new(AtomicUsize::new(0)),
        });
        let completion = completion.with_checkpoint(checkpoint.clone());
        let prompt_tokens = Arc::new(AtomicU64::new(0));
        let completion = Arc::new(UsageStamp {
            inner: Arc::new(completion),
            prompt_tokens: Arc::clone(&prompt_tokens),
        });
        let compactor = Arc::new(SessionCompact {
            client: Arc::clone(&client),
            source: durable_source,
            events: surface.events.clone(),
            window_tokens: agent.context_window_tokens() as u64,
            prompt_tokens,
        });
        let mut run = Self::open_with_graph(
            effect_dir,
            completion,
            Arc::new(ExecutorTools {
                capability_operation: None,
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
                targeted_paths: Some(Arc::clone(&surface.targeted_paths)),
                pre_tool_hook_resolved: false,
                repeats: Arc::default(),
            }),
            step_limit,
            session_id,
            graph,
            Some(compactor),
        )?;
        run.thread = thread;
        run.admitted_tools = admitted_tools;
        run.admitted_permission = admitted_permission;
        run.standing_digest = standing_digest;
        run.current_goal = current_goal;
        run.cancel = surface.cancel.clone();
        run.log = shared_log;
        Ok(run)
    }

    pub(crate) fn current_goal(&self) -> Option<&crate::goal_control::DurableGoalDefinitionV1> {
        self.current_goal.as_ref()
    }

    fn open_log(&self) -> Result<Arc<FileLog>> {
        Ok(Arc::clone(&self.log))
    }

    /// Commit a host recovery assertion through the original actor input
    /// writer. This does not resume the actor or acquire a tool effect.
    pub(crate) fn append_recovery_unproved(
        &self,
        checkpoint: &crate::loop_checkpoint::LoopCheckpoint,
        session_id: &str,
    ) -> Result<()> {
        let input = recovery_unproved::RecoveryUnprovedV1::from_checkpoint(
            checkpoint,
            session_id,
            &self.thread,
        )?;
        let fact = input.into_fact()?;
        a3s_effect::append_input(self.log.as_ref(), &self.thread, fact).map_err(|error| {
            anyhow::anyhow!("tool_effect_recovery_unproved_append_failed: {error}")
        })
    }

    pub fn read_facts(&self) -> Result<Vec<a3s_effect::Fact>> {
        let log = self.open_log()?;
        log.read(&self.thread)
            .map_err(|error| anyhow::anyhow!(error))
    }

    /// An authoritative thread is immutable. Legacy callers may check an
    /// empty thread, but cannot erase preparation or settlement evidence.
    pub fn reset_thread_log(&self) -> Result<()> {
        if !self.read_facts()?.is_empty() {
            anyhow::bail!(
                "tool_effect_log_immutable: cannot reset thread '{}' in {}",
                self.thread,
                self.dir.display()
            );
        }
        Ok(())
    }

    /// Import conversation only when no authoritative conversation exists.
    /// An unverified terminal narrative may request verification again. A
    /// prepared invocation without its terminal is resumed at its original
    /// position so the actor records Unknown instead of losing that debt.
    pub(crate) fn seed_recovery_history(&self, history: &[Message], prompt: &str) -> Result<()> {
        self.seed_history(history)?;
        let facts = self.read_facts()?;
        let debt = receipts::fold_debt(&facts);
        if !debt.unresolved.is_empty() || debt.paths.is_empty() {
            return Ok(());
        }
        let terminal_narrative = facts
            .iter()
            .rev()
            .find(|fact| fact.kind == "model.turn")
            .is_some_and(|fact| fact.payload["kind"] == "text");
        if terminal_narrative {
            let text = format!("{prompt}\n\n[recovery] Verify the already recorded workspace changes: {}. Do not repeat completed mutations.", debt.paths.join(", "));
            self.log
                .append(
                    &self.thread,
                    &[message_fact(
                        format!("recovery-input:{}", facts.len()),
                        text,
                    )],
                    None,
                )
                .map_err(|error| anyhow::anyhow!(error))?;
        }
        Ok(())
    }

    /// Record prior turns so the new prompt is the only infer that runs.
    ///
    /// Each assistant text is a `model.turn` whose cause is `infer:{turn}:{cycle}`.
    /// An existing conversation log is already the control source and is left
    /// unchanged. Standing-context and policy metadata are not that conversation.
    pub fn seed_history(&self, history: &[Message]) -> Result<()> {
        seed_history(self.open_log()?.as_ref(), &self.thread, history)
    }

    pub async fn user_text(&self, text: &str) -> Result<a3s_effect::Settlement<CodingView>> {
        self.user_text_with_attachments(text, &[]).await
    }

    /// Same turn as [`Self::user_text`], with image bytes stored beside `text`.
    ///
    /// The effect fold only copies `text` into the scheduler transcript. The
    /// model call reads `images` back from this fact so the pixels are not
    /// reduced to a byte count.
    pub async fn user_text_with_attachments(
        &self,
        text: &str,
        attachments: &[Attachment],
    ) -> Result<a3s_effect::Settlement<CodingView>> {
        let log = self.open_log()?;
        let key = format!("m-{}", log.read(&self.thread).unwrap_or_default().len());
        ingest_coding_with_cancel(
            &self.actor,
            log.as_ref(),
            Arc::clone(&self.services),
            &self.thread,
            user_message_fact(key, text, attachments),
            self.limit,
            &self.cancel,
        )
        .await
        .map_err(exit_to_error)
    }

    pub async fn steer(&self, text: &str) -> Result<a3s_effect::Settlement<CodingView>> {
        self.user_text(text).await
    }

    pub async fn resume_limit(&self, limit: u32) -> Result<a3s_effect::Settlement<CodingView>> {
        let log = self.open_log()?;
        resume_coding_with_cancel(
            &self.actor,
            log.as_ref(),
            Arc::clone(&self.services),
            &self.thread,
            limit,
            &self.cancel,
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
        ingest_coding_with_cancel(
            &self.actor,
            log.as_ref(),
            Arc::clone(&self.services),
            &self.thread,
            confirm_fact(key, tool_call_id, approved),
            self.limit,
            &self.cancel,
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
        ingest_coding_with_cancel(
            &self.actor,
            log.as_ref(),
            Arc::clone(&self.services),
            &self.thread,
            answer_fact(key, text),
            self.limit,
            &self.cancel,
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

#[cfg(test)]
#[path = "fact_control_tests.rs"]
mod tests;

#[cfg(test)]
mod unsettled_debt_tests {
    use super::unsettled_mutation_paths;
    use a3s_effect::Fact;

    fn fact(kind: &str, payload: serde_json::Value) -> Fact {
        Fact {
            seq: 0,
            kind: kind.to_string(),
            key: String::new(),
            cause: None,
            payload,
        }
    }

    #[test]
    fn unsettled_mutation_paths_reads_only_past_the_last_settlement() {
        let settled = fact("turn.complete", serde_json::json!({"terminal": "verified"}));
        let mut mutation_turn = fact(
            "model.turn",
            serde_json::json!({"kind": "tool", "call": {"id":"edit-one", "name": "edit", "args": {"file_path": "src/lib.rs"}}}),
        );
        mutation_turn.key = "model:1:0".into();
        let mut mutation_result = fact(
            "tool.result",
            serde_json::json!({"ok": true,"toolCallId":"edit-one"}),
        );
        mutation_result.key = "tool-result:1:0:edit-one".into();
        let read_turn = fact(
            "model.turn",
            serde_json::json!({"kind": "tool", "call": {"name": "read", "args": {"file_path": "x.rs"}}}),
        );
        // Debt before a settlement is owed; after the last settlement it
        // is not (the marker alone bounds the replay).
        let owed = unsettled_mutation_paths(&[
            mutation_turn.clone(),
            mutation_result.clone(),
            settled.clone(),
            read_turn,
        ]);
        assert!(owed.is_empty(), "settled mutations are not owed: {owed:?}");
        let owed = unsettled_mutation_paths(&[settled, mutation_turn, mutation_result]);
        assert_eq!(owed, vec!["src/lib.rs".to_string()]);
    }
}

#[cfg(test)]
mod model_governance_tests;

#[cfg(test)]
mod model_error_tests;

#[cfg(test)]
mod steer_terminal_tests;

#[cfg(test)]
mod conversation_history_tests;

#[cfg(test)]
mod tool_history_tests;
