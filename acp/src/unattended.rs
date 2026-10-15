//! `a3s-code-acp --unattended`: one task run with nobody on the other end of
//! the wire, such as a Terminal-Bench trial driven by Harbor's ACP client.
//!
//! Interactive launches never build an [`Unattended`]. With it, the agent
//! answers its own confirmations and questions, keeps harness state outside
//! the graded workspace, and wraps each prompt in the host admission loop the
//! native `terminal_bench_runner` uses.

#[path = "unattended/result.rs"]
pub(crate) mod result;
#[cfg(test)]
#[path = "unattended/tests.rs"]
mod tests;

use a3s_code_core::goal_control::GoalContinuationPolicyV1;
use a3s_code_core::verification::{VerificationCheck, VerificationReport, VerificationStatus};
use a3s_code_core::{AgentEvent, AgentStyle, PlanningMode, SessionOptions, SystemPromptSlots};
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use result::{count_artifact_evidence, RunProgress};

/// Same default as the native runner: inside Harbor's outer agent timeout.
pub const DEFAULT_EXECUTION_BUDGET_MS: u64 = 840_000;

/// Retries when a stream makes no new workspace mutation: the model ends
/// before any admission gate, ends idle after a bind, or the stream dies
/// without a terminal event. A distinct mutation digest is progress and does
/// not consume this budget. The same digest twice still fails closed. The
/// execution deadline bounds a run that keeps producing new digests.
const HOST_IDLE_ATTEMPTS: usize = 16;
/// A tool call after a host bind is not evidence the task grader would pass.
/// Ask the model to prove the original requirements again, this many times.
const HOST_UNPROVEN_STOPS: usize = 3;
/// One streamed model call. The resilience default of 120s cancels a
/// deep-effort turn that is still emitting tokens.
const LLM_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_REASONING_EFFORT: &str = "max";
const MAX_TOOL_ROUNDS: usize = 256;
const MAX_CONTINUATION_TURNS: u32 = 32;
/// Sibling child agents one delegation may run at once.
const MAX_PARALLEL_TASKS: usize = 4;

/// The reply to every `ask_user` question. Nobody is there to answer.
pub(crate) const QUESTION_ANSWER: &str = "Nobody is available to answer. \
Continue with your best judgment from the task instructions.";

/// Shown on every turn. It restates the host's own rule: the host bind is not
/// the grader. It names no task and no hidden check.
const TASK_PROOF_NOTE: &str = "\
Host note: A host admission bind only reopens the completion gate. It is not \
the task grader. Before you stop, re-read every requirement in the task and \
check it with a tool. A check that reports forbidden output, a warning, or a \
failed criterion means that requirement is still open. Change the program \
until every requirement holds at the same time, including on an input the \
task says must work and that is not the unchanged sample. Do not keep \
forbidden output because a sample path appears to need it. Do not stop with \
a status summary. Delete temporary files you created for a check. The tree \
you leave is what gets graded.";

/// The `--unattended` launch: execution budget and optional result record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unattended {
    pub(crate) max_execution_ms: u64,
    pub(crate) result_file: Option<PathBuf>,
    pub(crate) reasoning_effort: &'static str,
    /// A validated engineering policy name; every session carries its policy.
    pub(crate) engineering_policy: Option<String>,
    /// The first attempt runs as one Core goal slice bound to the task.
    pub(crate) goal_continuation: bool,
}

impl Unattended {
    pub fn new(max_execution_ms: u64, result_file: Option<PathBuf>) -> Result<Self> {
        if max_execution_ms == 0 {
            bail!("--max-execution-ms must be greater than zero");
        }
        Ok(Self {
            max_execution_ms,
            result_file,
            reasoning_effort: DEFAULT_REASONING_EFFORT,
            engineering_policy: None,
            goal_continuation: false,
        })
    }

    /// Bind each task as a durable goal and run its first attempt as a slice.
    pub fn with_goal_continuation(mut self) -> Self {
        self.goal_continuation = true;
        self
    }

    /// The `goal_control` policy a goal launch needs when the ACL declares
    /// none: one automatic slice, ending at the run deadline.
    pub fn goal_control(&self, now_unix_ms: u64) -> Result<Option<GoalContinuationPolicyV1>> {
        if !self.goal_continuation {
            return Ok(None);
        }
        let one = NonZeroU64::MIN;
        let deadline = NonZeroU64::new(now_unix_ms.saturating_add(self.max_execution_ms))
            .context("goal deadline needs a positive host clock")?;
        Ok(Some(GoalContinuationPolicyV1 {
            automatic_enabled: true,
            max_slices: one,
            max_no_progress_slices: one,
            deadline_unix_ms: deadline,
        }))
    }

    /// Run every session under a named engineering policy (`engineering-v1`).
    pub fn with_engineering_policy(mut self, name: &str) -> Result<Self> {
        crate::engineering::named_policy(name).map_err(anyhow::Error::msg)?;
        self.engineering_policy = Some(name.to_string());
        Ok(self)
    }

    /// One of the host effort levels; the provider maps it to its own scale.
    pub fn with_reasoning_effort(mut self, effort: &str) -> Result<Self> {
        let levels = crate::agent::HOST_EFFORT_LEVELS;
        let Some(level) = levels.iter().find(|level| **level == effort) else {
            bail!(
                "--reasoning-effort must be one of {}: {effort}",
                levels.join(", ")
            );
        };
        self.reasoning_effort = level;
        Ok(self)
    }

    /// Core options for every unattended session. Every capability the
    /// interactive host offers stays on — `a3s-vec` FTS search, Code
    /// Intelligence, file memory, skills, web tools, delegation — while the
    /// interactive guardrail and skill watching are absent: one asks a person,
    /// the other waits for installs nobody makes. File memory and indexes land
    /// under the private state root, not the graded workspace.
    pub(crate) fn session_options(
        &self,
        session_id: &str,
        model_id: &str,
        skill_dirs: Vec<PathBuf>,
    ) -> Result<SessionOptions, String> {
        let mut options = SessionOptions::new()
            .with_assistant_final_response(true)
            .with_session_id(session_id.to_string())
            .with_model(model_id.to_string())
            .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
            // Task text is arbitrary. Pin the writable general style so words
            // in a task cannot select a read-only style.
            .with_prompt_slots(SystemPromptSlots {
                style: Some(AgentStyle::GeneralPurpose),
                extra: Some(TASK_PROOF_NOTE.to_string()),
                ..SystemPromptSlots::default()
            })
            // The model receives the task verbatim, not a planner rewrite.
            .with_planning_mode(PlanningMode::Disabled)
            .with_reasoning_effort(self.reasoning_effort)
            .with_resilience_defaults()
            .with_llm_api_timeout(LLM_TIMEOUT_MS)
            .with_auto_compact(true)
            .with_max_tool_rounds(MAX_TOOL_ROUNDS)
            .with_max_continuation_turns(MAX_CONTINUATION_TURNS)
            .with_code_intelligence()
            .with_manual_delegation_enabled(true)
            .with_auto_delegation_enabled(true)
            .with_auto_parallel_delegation(true)
            .with_max_parallel_tasks(MAX_PARALLEL_TASKS);
        if !skill_dirs.is_empty() {
            options = options.with_skill_dirs(skill_dirs);
        }
        if let Some(name) = &self.engineering_policy {
            options = options.with_work_policy(crate::engineering::named_policy(name)?);
        }
        if self.goal_continuation {
            // A goal slice executes its accepted plan step.
            options = options
                .with_planning_mode(PlanningMode::Enabled)
                .with_goal_tracking(true);
        }
        options.max_execution_time_ms = Some(self.max_execution_ms);
        Ok(options)
    }
}

/// Create a private state root outside the workspace and make it the
/// process-wide harness state directory. Call once, before any session opens.
pub fn install_state_root() -> Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("a3s-code-acp-{}", uuid::Uuid::new_v4()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&root)
        .with_context(|| format!("create unattended state root {}", root.display()))?;
    a3s_code_core::state_root::set_state_root_override(&root)?;
    Ok(root)
}

pub(crate) fn first_prompt(task: &str) -> String {
    format!("{task}\n\n{TASK_PROOF_NOTE}")
}

pub(crate) fn bind_continuation(digest: &str) -> String {
    format!(
        "Continue the task. The host bound admission digest {digest} only so \
         the completion gate can reopen. That bind is not the task grader. \
         Paths named by the gate are files the edit already touched, not a \
         list of missing deliverables. Re-read the original requirements and \
         prove each one with a tool. A warning or forbidden output means that \
         requirement is still open. Prove it on an input the task says must \
         work and that is not the unchanged sample. Do not stop with a status \
         summary."
    )
}

/// Summary focus for the one fresh reading after a capped stop: keep the
/// requirements and the current program, drop claims that a check passed.
pub(crate) const FRESH_REVIEW_FOCUS: &str = "Keep the task requirements, the current program, and every requirement a check showed is still open. Drop claims that a check already proved the task.";

pub(crate) const UNPROVEN_STOP_CONTINUATION: &str = "You used tools and then \
stopped. The task has not been graded. The host admission bind is not the task \
grader. Re-read the original requirements and prove each one again with a \
tool. A warning or forbidden output means that requirement is still open. \
Prove it on an input the task says must work and that is not the unchanged \
sample. Do not stop with a status summary.";

pub(crate) const IDLE_AFTER_BIND_CONTINUATION: &str = "You stopped without \
taking further tool actions after the host admission bind. The task grader \
has not passed yet. Continue investigating and fixing until the task \
requirements are met; do not stop with a status summary.";

pub(crate) const BEFORE_ANY_GATE_CONTINUATION: &str = "You stopped before \
producing a graded workspace solution. The verifier still has nothing to \
accept. Continue implementing the task with concrete file edits and local \
checks; do not stop with a short status word or plan-only summary.";

pub(crate) fn non_terminal_continuation(detail: &str) -> String {
    format!(
        "The previous attempt ended abruptly ({detail}). Continue solving the \
         task with the available tools (bash/read/write/edit). Do not call \
         Skill tools that are not installed."
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostAdmissionStep {
    BindNewDigest,
    SameDigestStillOpen,
    NudgeIdleAfterBind,
    NudgeBeforeAnyGate,
    /// Tools ran after a bind, then the model stopped. That is not a grade.
    NudgeUnprovenStop,
    /// The proof cap was used. Drop the anchored transcript once and read the
    /// task again. This is not a grade and does not forgive an open digest.
    FreshContextReview,
    RetryNonTerminal,
    Finish,
    NonTerminalExhausted,
}

#[derive(Debug, Default)]
pub(crate) struct HostAdmissionBudget {
    idle_used: usize,
    non_terminal_used: usize,
    unproven_stops: usize,
    fresh_review_used: bool,
}

fn idle_retry_available(used: usize) -> bool {
    used.saturating_add(1) < HOST_IDLE_ATTEMPTS
}

/// What one stream attempt showed the host.
#[derive(Debug, Default)]
pub(crate) struct AttemptFacts {
    pub(crate) gate_digest: Option<String>,
    pub(crate) tools: usize,
    pub(crate) failed: bool,
    pub(crate) last_error: Option<String>,
}

/// Decide the next host-admission cycle from the stream that just ended.
///
/// Inserts a digest only when it is new. Callers record the host report for
/// `BindNewDigest` and fail closed for `SameDigestStillOpen`.
pub(crate) fn host_admission_step(
    bound_digests: &mut HashSet<String>,
    budget: &mut HostAdmissionBudget,
    facts: &AttemptFacts,
    terminal_event: bool,
) -> HostAdmissionStep {
    if let Some(digest) = &facts.gate_digest {
        if !bound_digests.insert(digest.clone()) {
            return HostAdmissionStep::SameDigestStillOpen;
        }
        return HostAdmissionStep::BindNewDigest;
    }
    if facts.tools == 0 && !bound_digests.is_empty() {
        if idle_retry_available(budget.idle_used) {
            budget.idle_used += 1;
            return HostAdmissionStep::NudgeIdleAfterBind;
        }
    } else if bound_digests.is_empty() && !facts.failed && terminal_event {
        if idle_retry_available(budget.idle_used) {
            budget.idle_used += 1;
            return HostAdmissionStep::NudgeBeforeAnyGate;
        }
    } else if !bound_digests.is_empty() && facts.tools > 0 && terminal_event && !facts.failed {
        if budget.unproven_stops < HOST_UNPROVEN_STOPS {
            budget.unproven_stops += 1;
            return HostAdmissionStep::NudgeUnprovenStop;
        }
        if !budget.fresh_review_used {
            // One fresh reading gets its own proof budget. A second capped
            // stop finishes. The same digest stays fail-closed.
            budget.fresh_review_used = true;
            budget.unproven_stops = 0;
            return HostAdmissionStep::FreshContextReview;
        }
    }
    if !terminal_event {
        if idle_retry_available(budget.non_terminal_used) {
            budget.non_terminal_used += 1;
            return HostAdmissionStep::RetryNonTerminal;
        }
        return HostAdmissionStep::NonTerminalExhausted;
    }
    HostAdmissionStep::Finish
}

/// The host's own check for a digest it chose to bind. The task verifier,
/// not this report, decides acceptance.
pub(crate) fn host_verification_report(digest: &str) -> VerificationReport {
    VerificationReport::new(
        "harbor:terminal-bench",
        vec![VerificationCheck::required(
            "check:harbor-host",
            "host",
            "Harbor TB host accepts this workspace mutation digest; Harbor's native verifier owns task acceptance.",
        )
        .with_status(VerificationStatus::Passed)],
    )
    .with_effect_digest(digest.to_string())
}

/// Count one core event into the run record and this attempt's facts.
pub(crate) fn note_event(progress: &mut RunProgress, facts: &mut AttemptFacts, event: &AgentEvent) {
    match event {
        // Core may emit turn 0 for the first round; count starts as well.
        AgentEvent::TurnStart { turn } => {
            progress.turns = progress.turns.saturating_add(1).max(*turn);
        }
        // The ACP stream does not announce every call with ToolStart. Every
        // attempted call, failed or not, ends with ToolEnd.
        AgentEvent::ToolEnd {
            exit_code,
            metadata,
            ..
        } => {
            progress.tool_calls += 1;
            facts.tools += 1;
            if *exit_code == 0 {
                progress.successful_tool_calls += 1;
                if let Some(metadata) = metadata {
                    progress.artifact_evidence_count += count_artifact_evidence(metadata);
                }
            }
        }
        AgentEvent::Error { message } => {
            progress.error_count += 1;
            progress.remember_error(message);
            facts.failed = true;
            facts.last_error = Some(message.clone());
            if let Some(digest) = a3s_code_core::harness_loop::unbound_mutation_digest(message) {
                facts.gate_digest = Some(digest.to_string());
            }
        }
        AgentEvent::End { .. } => progress.terminal_event = true,
        _ => {}
    }
}
