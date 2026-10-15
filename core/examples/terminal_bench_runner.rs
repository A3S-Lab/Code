//! Minimal headless A3S Code runner used by the Harbor Terminal-Bench adapter.
//!
//! The benchmark harness owns the container and verifier. This example keeps the
//! agent boundary in `a3s-code-core`: it loads the caller's ACL, binds one
//! session to the task workspace, and exposes the native Code tools through the
//! normal session loop. No MCP transport or benchmark-specific tool executor is
//! involved.
//!
//! Usage:
//! `terminal_bench_runner --config /run/a3s/config.acl --workspace /root --prompt-file /run/a3s/instruction.md --result-file /logs/agent/a3s-code.result.json`

#[path = "terminal_bench_runner/result.rs"]
mod result;
#[path = "terminal_bench_runner/sandbox.rs"]
mod sandbox;

use a3s_code_core::execution_identity::ExecutionResultOutcomeV1;
use a3s_code_core::hitl::AutoApproveConfirmation;
use a3s_code_core::llm::CodexLoginClient;
use a3s_code_core::skills::SkillRegistry;
use a3s_code_core::verification::{
    VerificationCheck, VerificationReport, VerificationStatus, VERIFICATION_REPORT_SCHEMA,
};
use a3s_code_core::{
    Agent, AgentEvent, AgentStyle, PlanningMode, SessionOptions, SystemPromptSlots,
};
use a3s_memory::InMemoryStore;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Retries when a stream makes no new workspace mutation: the model ends
/// before any admission gate, ends idle after a bind, or the stream dies
/// without a terminal event. A distinct mutation digest is progress and does
/// not consume this budget. The same digest twice still fails closed. The
/// execution deadline bounds a run that keeps producing new digests.
const HARBOR_HOST_IDLE_ATTEMPTS: usize = 16;
/// A tool call after a host bind is not evidence the task grader would pass.
/// Ask the model to prove the original requirements again, this many times.
/// A new mutation digest does not consume the cap. The same digest twice
/// still fails closed.
const HARBOR_HOST_UNPROVEN_STOPS: usize = 3;
/// One streamed model call. The resilience default of 120s cancels a
/// deep-effort turn that is still emitting tokens. Ten minutes stays inside
/// the benchmark's outer execution deadline and is not a per-task budget.
const HARBOR_HOST_LLM_TIMEOUT_MS: u64 = 600_000;
/// DeepSeek's coding comparisons score the `max` menu entry above `high`.
/// Every benchmark task uses that menu entry.
const HARBOR_HOST_REASONING_EFFORT: &str = "max";
const MUTATION_DIGEST_IN_GATE: &str = "workspace mutation ";

/// Shown on every benchmark turn. It restates the runner's own rule: the
/// host bind is not the grader. It names no task and no hidden check.
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

fn bind_continuation(digest: &str) -> String {
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

fn fresh_review_prompt(task_prompt: &str) -> String {
    format!("{task_prompt}\n\n{TASK_PROOF_NOTE}")
}

/// Summary focus for the one fresh reading after a capped stop. The reading
/// exists because that stop still carries a transcript that claimed the
/// sample passed. Keep the requirements and the current program, and drop
/// the claim that a check already proved them.
fn fresh_review_focus() -> &'static str {
    "Keep the task requirements, the current program, and every requirement a check showed is still open. Drop claims that a check already proved the task."
}

fn unproven_stop_continuation() -> String {
    "You used tools and then stopped. The task has not been graded. The host \
     admission bind is not the task grader. Re-read the original requirements \
     and prove each one again with a tool. A warning or forbidden output means \
     that requirement is still open. Prove it on an input the task says must \
     work and that is not the unchanged sample. Do not stop with a status \
     summary."
        .to_string()
}

use result::{
    classify_failure, count_artifact_evidence, persist_result, ExecutionPhase, RunProgress,
    TerminalReason, DEFAULT_EXECUTION_BUDGET_MS,
};

use sandbox::ProcessHostBashSandbox;

#[derive(Debug)]
struct Args {
    config: PathBuf,
    workspace: PathBuf,
    prompt_file: PathBuf,
    result_file: PathBuf,
    max_execution_time_ms: u64,
    codex_auth: Option<PathBuf>,
    codex_model: Option<String>,
    codex_reasoning_effort: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut args = std::env::args_os().skip(1);
    let mut config = None;
    let mut workspace = None;
    let mut prompt_file = None;
    let mut result_file = None;
    let mut max_execution_time_ms = DEFAULT_EXECUTION_BUDGET_MS;
    let mut codex_auth = None;
    let mut codex_model = None;
    let mut codex_reasoning_effort = None;
    while let Some(flag) = args.next() {
        let flag = flag.to_string_lossy();
        if matches!(flag.as_ref(), "--help" | "-h") {
            println!(
                "usage: terminal_bench_runner --config PATH --workspace PATH --prompt-file PATH [--result-file PATH --max-execution-time-ms MS --codex-auth PATH --codex-model MODEL --codex-reasoning-effort EFFORT]"
            );
            std::process::exit(0);
        }
        let value = args
            .next()
            .with_context(|| format!("missing value for {flag}"))?;
        match flag.as_ref() {
            "--config" => config = Some(PathBuf::from(value)),
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--prompt-file" => prompt_file = Some(PathBuf::from(value)),
            "--result-file" => result_file = Some(PathBuf::from(value)),
            "--max-execution-time-ms" => {
                max_execution_time_ms = value
                    .to_string_lossy()
                    .parse::<u64>()
                    .context("--max-execution-time-ms must be an integer")?;
                if max_execution_time_ms == 0 {
                    anyhow::bail!("--max-execution-time-ms must be greater than zero");
                }
            }
            "--codex-auth" => codex_auth = Some(PathBuf::from(value)),
            "--codex-model" => codex_model = Some(value.to_string_lossy().into_owned()),
            "--codex-reasoning-effort" => {
                codex_reasoning_effort = Some(value.to_string_lossy().into_owned())
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let workspace = workspace.context("--workspace is required")?;
    Ok(Args {
        config: config.context("--config is required")?,
        result_file: result_file.unwrap_or_else(|| workspace.join(".a3s-code.result.json")),
        workspace,
        prompt_file: prompt_file.context("--prompt-file is required")?,
        max_execution_time_ms,
        codex_auth,
        codex_model,
        codex_reasoning_effort,
    })
}

fn parse_completion_gate_mutation_digest(message: &str) -> Option<String> {
    if !message.contains("completion gate:") {
        return None;
    }
    let start = message.find(MUTATION_DIGEST_IN_GATE)? + MUTATION_DIGEST_IN_GATE.len();
    let rest = message.get(start..)?;
    let digest: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .collect();
    if digest.len() == 64 {
        Some(digest)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostAdmissionStep {
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
struct HostAdmissionBudget {
    idle_used: usize,
    non_terminal_used: usize,
    unproven_stops: usize,
    fresh_review_used: bool,
}

fn idle_retry_available(used: usize) -> bool {
    used.saturating_add(1) < HARBOR_HOST_IDLE_ATTEMPTS
}

fn unproven_stop_available(used: usize) -> bool {
    used < HARBOR_HOST_UNPROVEN_STOPS
}

fn execution_deadline_reached(elapsed_ms: u64, budget_ms: u64) -> bool {
    elapsed_ms >= budget_ms
}

/// Decide the next host-admission cycle from the stream that just ended.
///
/// Inserts a digest only when it is new. Callers record the Harbor host
/// report for `BindNewDigest` and fail closed for `SameDigestStillOpen`.
fn host_admission_step(
    bound_digests: &mut HashSet<String>,
    budget: &mut HostAdmissionBudget,
    gate_digest: Option<&str>,
    tools_this_attempt: usize,
    terminal_event: bool,
    worker_failed: bool,
) -> HostAdmissionStep {
    if let Some(digest) = gate_digest {
        if !bound_digests.insert(digest.to_string()) {
            return HostAdmissionStep::SameDigestStillOpen;
        }
        return HostAdmissionStep::BindNewDigest;
    }
    if tools_this_attempt == 0 && !bound_digests.is_empty() {
        if idle_retry_available(budget.idle_used) {
            budget.idle_used = budget.idle_used.saturating_add(1);
            return HostAdmissionStep::NudgeIdleAfterBind;
        }
    } else if bound_digests.is_empty() && !worker_failed && terminal_event {
        if idle_retry_available(budget.idle_used) {
            budget.idle_used = budget.idle_used.saturating_add(1);
            return HostAdmissionStep::NudgeBeforeAnyGate;
        }
    } else if !bound_digests.is_empty()
        && tools_this_attempt > 0
        && terminal_event
        && !worker_failed
        && unproven_stop_available(budget.unproven_stops)
    {
        budget.unproven_stops = budget.unproven_stops.saturating_add(1);
        return HostAdmissionStep::NudgeUnprovenStop;
    } else if !bound_digests.is_empty()
        && tools_this_attempt > 0
        && terminal_event
        && !worker_failed
        && !budget.fresh_review_used
    {
        // A capped stop still carries the transcript that claimed the sample
        // passed. One fresh reading gets its own proof budget. A second capped
        // stop finishes. The same digest stays fail-closed.
        budget.fresh_review_used = true;
        budget.unproven_stops = 0;
        return HostAdmissionStep::FreshContextReview;
    }
    if !terminal_event {
        if idle_retry_available(budget.non_terminal_used) {
            budget.non_terminal_used = budget.non_terminal_used.saturating_add(1);
            return HostAdmissionStep::RetryNonTerminal;
        }
        return HostAdmissionStep::NonTerminalExhausted;
    }
    HostAdmissionStep::Finish
}

fn harbor_host_verification_report(digest: &str) -> VerificationReport {
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

async fn execute(args: &Args, progress: &mut RunProgress) -> Result<()> {
    let task_prompt = tokio::fs::read_to_string(&args.prompt_file)
        .await
        .with_context(|| format!("read prompt {}", args.prompt_file.display()))?;
    eprintln!("a3s-code: loading config");
    let agent = Agent::new(args.config.to_string_lossy().to_string())
        .await
        .context("load A3S Code ACL")?;
    let mut options = SessionOptions::new()
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        // Benchmark instructions are arbitrary user tasks. Pin the execution
        // role to the writable general agent so words such as "findall" in a
        // task do not accidentally select the read-only Explore style.
        .with_prompt_slots(SystemPromptSlots {
            style: Some(AgentStyle::GeneralPurpose),
            extra: Some(TASK_PROOF_NOTE.to_string()),
            ..SystemPromptSlots::default()
        })
        // Harbor supplies the complete task instruction. Disable A3S planning
        // pre-analysis so the model receives that instruction verbatim as the
        // user message instead of a planner-rewritten wrapper.
        .with_planning_mode(PlanningMode::Disabled)
        // The benchmark model is the ACL default. Request its `max` thinking
        // menu entry for every task.
        .with_reasoning_effort(HARBOR_HOST_REASONING_EFFORT)
        .with_resilience_defaults()
        .with_llm_api_timeout(HARBOR_HOST_LLM_TIMEOUT_MS)
        .with_auto_compact(true)
        // Terminal-Bench includes long-horizon tasks whose official agent
        // timeout is measured in hours. Keep the framework's internal
        // progress budget below that outer deadline without truncating a
        // valid solution after a short fixed number of tool turns.
        .with_max_tool_rounds(256)
        .with_max_continuation_turns(32)
        // Harbor tasks are ephemeral; avoid creating `.a3s/memory` under a
        // non-writable workspace root (seen as startup_failed Permission denied).
        .with_memory(Arc::new(InMemoryStore::new()))
        // Empty skill registry: models sometimes call Skill("view-image") which
        // is not shipped in the TB image; a missing skill must not abort the run.
        .with_skill_registry(Arc::new(SkillRegistry::new()))
        .with_allow_process_host_sandbox(true)
        .with_sandbox_handle(Arc::new(ProcessHostBashSandbox::new(
            args.workspace.clone(),
            Some(
                progress
                    .started_at
                    .checked_add(Duration::from_millis(args.max_execution_time_ms))
                    .unwrap_or_else(Instant::now),
            ),
        )));
    // `max_execution_time_ms` is an existing public SessionOptions field. Set
    // it after the builder chain so this benchmark-only deadline change does
    // not collide with hosts that provide their own option extension methods.
    options.max_execution_time_ms = Some(args.max_execution_time_ms);
    if let Some(auth_path) = &args.codex_auth {
        let model = args
            .codex_model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .context("--codex-model is required with --codex-auth")?;
        let client = CodexLoginClient::from_auth_file(
            auth_path.clone(),
            model,
            "terminal-bench-a3s-code",
            args.codex_reasoning_effort.clone(),
        )
        .context("load Codex login client")?;
        options = options
            .with_model(format!("codex/{model}"))
            .with_llm_client(Arc::new(client));
        eprintln!("a3s-code: using Codex login model {model}");
    }
    let session = agent
        .session_async(args.workspace.to_string_lossy().to_string(), Some(options))
        .await
        .context("build workspace-bound A3S Code session")?;
    eprintln!("a3s-code: session ready");

    let mut prompt = format!("{task_prompt}\n\n{TASK_PROOF_NOTE}");
    let mut bound_digests = HashSet::new();
    let mut admission_budget = HostAdmissionBudget::default();
    let mut stream_index = 0usize;
    loop {
        if execution_deadline_reached(progress.duration_ms(), args.max_execution_time_ms) {
            session.close().await;
            anyhow::bail!("A3S Code execution deadline exceeded");
        }
        stream_index = stream_index.saturating_add(1);
        eprintln!(
            "a3s-code: stream started attempt={} bound={}",
            stream_index,
            bound_digests.len()
        );
        // Stream attempts are the reliable turn proxy while session.stream may
        // omit TurnStart on this headless path (observed turns==0 with tools>0).
        progress.turns = progress.turns.saturating_add(1);
        progress.phase = ExecutionPhase::Streaming;
        progress.terminal_event = false;
        let tools_before = progress.tool_calls;
        let (mut events, worker) = session
            .stream(&prompt, None)
            .await
            .context("start A3S Code stream")?;
        let mut gate_digest: Option<String> = None;
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::TextDelta { text } => {
                    print!("{text}");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                AgentEvent::TurnStart { turn } => {
                    // Count observed turn starts. Core may emit turn==0 for the
                    // first LLM round; max(turn) alone under-reports as 0.
                    progress.turns = progress.turns.saturating_add(1).max(turn);
                }
                AgentEvent::ToolStart { name, .. } => {
                    progress.tool_calls = progress.tool_calls.saturating_add(1);
                    eprintln!("[a3s-code tool={name}]");
                }
                AgentEvent::ToolEnd {
                    exit_code,
                    metadata,
                    ..
                } => {
                    if exit_code == 0 {
                        progress.successful_tool_calls =
                            progress.successful_tool_calls.saturating_add(1);
                        if let Some(metadata) = metadata.as_ref() {
                            progress.artifact_evidence_count = progress
                                .artifact_evidence_count
                                .saturating_add(count_artifact_evidence(metadata));
                        }
                    }
                }
                AgentEvent::Error { message } => {
                    progress.error_count = progress.error_count.saturating_add(1);
                    progress.remember_error(&message);
                    eprintln!("[a3s-code error] {message}");
                    if let Some(digest) = parse_completion_gate_mutation_digest(&message) {
                        gate_digest = Some(digest);
                    }
                }
                AgentEvent::End { .. } => progress.terminal_event = true,
                _ => {}
            }
        }
        progress.phase = ExecutionPhase::Joining;
        let tools_this_attempt = progress.tool_calls.saturating_sub(tools_before);
        let worker_error = worker.await.err();
        if let Some(error) = &worker_error {
            progress.remember_error(error);
            if gate_digest.is_none() {
                if let Some(digest) = parse_completion_gate_mutation_digest(&error.to_string()) {
                    gate_digest = Some(digest);
                }
            }
        }

        let step = host_admission_step(
            &mut bound_digests,
            &mut admission_budget,
            gate_digest.as_deref(),
            tools_this_attempt,
            progress.terminal_event,
            worker_error.is_some(),
        );
        match step {
            HostAdmissionStep::BindNewDigest => {
                let Some(digest) = gate_digest else {
                    session.close().await;
                    anyhow::bail!("host admission bind missing mutation digest");
                };
                debug_assert_eq!(
                    harbor_host_verification_report(&digest).schema,
                    VERIFICATION_REPORT_SCHEMA
                );
                session.record_verification_reports([harbor_host_verification_report(&digest)]);
                eprintln!("a3s-code: bound Harbor host verification for mutation {digest}");
                prompt = bind_continuation(&digest);
            }
            HostAdmissionStep::SameDigestStillOpen => {
                session.close().await;
                let digest = gate_digest.as_deref().unwrap_or("");
                anyhow::bail!("completion gate still open after host verification for {digest}");
            }
            HostAdmissionStep::NudgeIdleAfterBind => {
                eprintln!(
                    "a3s-code: idle end after host admission bind; continuing (attempt {})",
                    stream_index.saturating_add(1)
                );
                prompt = "You stopped without taking further tool actions after the host \
                    admission bind. Harbor's task grader has not passed yet. Continue \
                    investigating and fixing until the task requirements are met; do not \
                    stop with a status summary."
                    .to_string();
            }
            HostAdmissionStep::NudgeBeforeAnyGate => {
                eprintln!(
                    "a3s-code: ended before any host admission gate; continuing (attempt {})",
                    stream_index.saturating_add(1)
                );
                prompt = "You stopped before producing a graded workspace solution. \
                    Harbor's verifier still has nothing to accept. Continue implementing \
                    the task with concrete file edits and local checks; do not stop with \
                    a short status word or plan-only summary."
                    .to_string();
            }
            HostAdmissionStep::NudgeUnprovenStop => {
                eprintln!(
                    "a3s-code: tool use after a bind is not a grade; continuing (attempt {})",
                    stream_index.saturating_add(1)
                );
                prompt = unproven_stop_continuation();
            }
            HostAdmissionStep::FreshContextReview => {
                eprintln!(
                    "a3s-code: capped stop kept an anchored transcript; compacting before another reading (attempt {})",
                    stream_index.saturating_add(1)
                );
                if let Err(error) = session
                    .compact_conversation(Some(fresh_review_focus()))
                    .await
                {
                    eprintln!("a3s-code: compact skipped: {error}");
                }
                prompt = fresh_review_prompt(&task_prompt);
            }
            HostAdmissionStep::RetryNonTerminal => {
                let detail = worker_error
                    .as_ref()
                    .map(|error| format!("{error:#}"))
                    .unwrap_or_else(|| "stream closed without terminal event".to_string());
                eprintln!(
                    "a3s-code: non-terminal stream end ({detail}); continuing (attempt {})",
                    stream_index.saturating_add(1)
                );
                prompt = format!(
                    "The previous attempt ended abruptly ({detail}). Continue solving the \
                     Terminal-Bench task with the available tools (bash/read/write/edit). \
                     Do not call Skill tools that are not installed."
                );
            }
            HostAdmissionStep::NonTerminalExhausted => {
                session.close().await;
                if let Some(error) = worker_error {
                    return Err(error).context("join A3S Code stream");
                }
                progress.stream_closed_without_terminal_event = true;
                anyhow::bail!("A3S Code stream ended without a terminal event");
            }
            HostAdmissionStep::Finish => {
                // Session close is unconditional once the gate is not requesting
                // another host bind. A late worker join failure after End must
                // not discard the terminal observation.
                session.close().await;
                if let Some(error) = worker_error {
                    if !progress.terminal_event {
                        return Err(error).context("join A3S Code stream");
                    }
                    eprintln!("[a3s-code warning] worker ended after terminal event: {error}");
                }
                if !progress.terminal_event {
                    progress.stream_closed_without_terminal_event = true;
                    anyhow::bail!("A3S Code stream ended without a terminal event");
                }
                return Ok(());
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let result_path = args.result_file.clone();
    let mut progress = RunProgress::new();
    let execution = execute(&args, &mut progress).await;
    let mut execution_error = execution.err();
    if let Some(error) = &execution_error {
        progress.remember_failure(error);
    }
    if execution_error.is_none() && !progress.has_action_evidence() {
        progress.mark_evidence_missing();
        execution_error = Some(anyhow::anyhow!(
            "A3S Code agent ended without a successful tool execution"
        ));
    }

    let (outcome, reason) = match &execution_error {
        None => (
            ExecutionResultOutcomeV1::Succeeded,
            TerminalReason::AgentEnd,
        ),
        Some(error) => classify_failure(&progress, error),
    };
    let report = progress.report(outcome, reason);
    if let Err(write_error) = persist_result(&result_path, &report).await {
        if execution_error.is_none() {
            return Err(write_error);
        }
        eprintln!("[a3s-code warning] could not persist terminal result: {write_error}");
    }

    if let Some(error) = execution_error {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_core::sandbox::{BashSandbox, SandboxCommandRequest};
    use std::time::Instant;

    #[test]
    fn parse_completion_gate_mutation_digest_extracts_sha256_hex() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let message = format!(
            "completion gate: workspace mutation {digest} has no bound Passed verification and no host waiver."
        );
        assert_eq!(
            parse_completion_gate_mutation_digest(&message).as_deref(),
            Some(digest)
        );
        assert!(parse_completion_gate_mutation_digest("unrelated error").is_none());
    }

    #[test]
    fn distinct_mutation_digests_do_not_consume_the_idle_retry_budget() {
        let mut bound = HashSet::new();
        let mut budget = HostAdmissionBudget::default();
        for index in 0..32u32 {
            let digest = format!("{index:064x}");
            assert_eq!(
                host_admission_step(&mut bound, &mut budget, Some(&digest), 3, true, true,),
                HostAdmissionStep::BindNewDigest,
            );
        }
        assert_eq!(bound.len(), 32);
        assert_eq!(budget.idle_used, 0);
        assert_eq!(budget.non_terminal_used, 0);
        assert_eq!(budget.unproven_stops, 0);
        let repeated = format!("{:064x}", 0u32);
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, Some(&repeated), 3, true, true),
            HostAdmissionStep::SameDigestStillOpen,
        );
    }

    #[test]
    fn a_tool_using_stop_after_a_bind_is_not_a_grade_until_the_cap() {
        let mut bound = HashSet::new();
        bound.insert(format!("{:064x}", 7u32));
        let mut budget = HostAdmissionBudget::default();
        for expected in 1..=HARBOR_HOST_UNPROVEN_STOPS {
            assert_eq!(
                host_admission_step(&mut bound, &mut budget, None, 2, true, false),
                HostAdmissionStep::NudgeUnprovenStop,
            );
            assert_eq!(budget.unproven_stops, expected);
        }
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, None, 2, true, false),
            HostAdmissionStep::FreshContextReview,
        );
        assert!(budget.fresh_review_used);
        assert_eq!(budget.unproven_stops, 0);
        for expected in 1..=HARBOR_HOST_UNPROVEN_STOPS {
            assert_eq!(
                host_admission_step(&mut bound, &mut budget, None, 2, true, false),
                HostAdmissionStep::NudgeUnprovenStop,
            );
            assert_eq!(budget.unproven_stops, expected);
        }
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, None, 2, true, false),
            HostAdmissionStep::Finish,
        );
        assert_eq!(budget.idle_used, 0);
        let repeated = format!("{:064x}", 7u32);
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, Some(&repeated), 2, true, true),
            HostAdmissionStep::SameDigestStillOpen,
        );
    }

    #[test]
    fn admission_continuations_keep_the_bind_distinct_from_the_grader() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let bound = bind_continuation(digest);
        assert!(bound.contains(digest));
        assert!(bound.contains("not the task grader"));
        assert!(unproven_stop_continuation().contains("not the task grader"));
        assert!(TASK_PROOF_NOTE.contains("not the task grader"));
        assert!(TASK_PROOF_NOTE.contains("re-read every requirement"));
        assert!(TASK_PROOF_NOTE.contains("temporary"));
        assert!(TASK_PROOF_NOTE.contains("requirement is still open"));
        assert!(TASK_PROOF_NOTE.contains("not the unchanged sample"));
        assert!(unproven_stop_continuation().contains("prove each one again"));
        assert!(unproven_stop_continuation().contains("requirement is still open"));
        assert!(unproven_stop_continuation().contains("not the unchanged sample"));
        assert!(!unproven_stop_continuation().contains("copied from an input"));
        assert!(fresh_review_prompt("task").contains("task"));
        assert!(fresh_review_prompt("task").contains("not the task grader"));
        assert!(fresh_review_prompt("task").contains("requirement is still open"));
        assert!(!fresh_review_prompt("task").contains("copied from an input"));
        assert!(fresh_review_focus().contains("task requirements"));
        assert!(fresh_review_focus().contains("current program"));
        assert!(fresh_review_focus().contains("still open"));
        assert!(fresh_review_focus().contains("Drop claims"));
        let handoff = a3s_code_core::compaction_system_prompt();
        assert!(handoff.contains("already finished"));
        assert!(handoff.contains("requirements"));
        assert!(handoff.contains("still open"));
        assert!(handoff.contains("unchanged sample"));
        assert!(handoff.contains("edits that are still in the tree"));
        let host_notes = [
            TASK_PROOF_NOTE,
            bound.as_str(),
            &unproven_stop_continuation(),
            fresh_review_focus(),
            handoff,
        ];
        for note in host_notes {
            for aimed in [
                "bun-sourcemap-leak",
                "private-trace-probe",
                "sourcesContent",
                "(value) => value",
            ] {
                assert!(!note.contains(aimed), "{aimed} must stay out of host notes");
            }
        }
        let resumed = SessionOptions::new()
            .with_resilience_defaults()
            .with_llm_api_timeout(HARBOR_HOST_LLM_TIMEOUT_MS);
        assert_eq!(resumed.llm_api_timeout_ms, Some(HARBOR_HOST_LLM_TIMEOUT_MS));
        assert!(HARBOR_HOST_LLM_TIMEOUT_MS > 120_000);
        let effort = SessionOptions::new().with_reasoning_effort(HARBOR_HOST_REASONING_EFFORT);
        assert_eq!(effort.reasoning_effort.as_deref(), Some("max"));
        assert_eq!(
            a3s_code_core::llm::deepseek_thinking_top_p("max"),
            Some(0.95)
        );
        assert_eq!(a3s_code_core::llm::deepseek_thinking_top_p("none"), None);
        assert_eq!(
            a3s_code_core::context_window_for_model(
                None,
                None,
                "boyue/deepseek-v4-flash",
                true,
                200_000,
            ),
            1_000_000
        );
        assert_eq!(
            a3s_code_core::context_window_for_model(
                None,
                Some(32_000),
                "boyue/deepseek-v4-flash",
                true,
                200_000,
            ),
            32_000
        );
        assert_eq!(
            a3s_code_core::context_window_for_model(None, None, "openai/gpt-test", true, 200_000),
            128_000
        );
        assert_eq!(
            a3s_code_core::documented_context_tokens("boyue/bailian/deepseek-v4.1-flash"),
            Some(1_000_000)
        );
    }

    #[test]
    fn summary_transcript_omits_chain_of_thought() {
        let mut message = a3s_code_core::llm::Message::assistant("edited build.ts");
        message.reasoning_content = Some("the sample check already proved the task".to_string());
        message
            .content
            .push(a3s_code_core::llm::ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "echo hi"}),
            });
        let rendered = a3s_code_core::render_message_for_summary(&message);
        assert!(rendered.contains("edited build.ts"));
        assert!(rendered.contains("Tool call bash"));
        assert!(rendered.contains("echo hi"));
        assert!(!rendered.contains("already proved"));
        assert!(!rendered.to_ascii_lowercase().contains("reasoning"));
    }

    #[tokio::test]
    async fn a_read_past_the_line_limit_marks_the_omitted_bytes() {
        let dir = std::env::temp_dir().join(format!("a3s-read-line-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("long.txt"), "a".repeat(2500)).unwrap();
        std::fs::write(dir.join("short.txt"), "hello\n").unwrap();
        let executor = a3s_code_core::tools::ToolExecutor::new(dir.to_string_lossy().to_string());
        let long_result = executor
            .execute("read", &serde_json::json!({"file_path": "long.txt"}))
            .await
            .unwrap();
        assert_eq!(long_result.exit_code, 0, "{}", long_result.output);
        assert!(long_result
            .output
            .contains("[line truncated, 500 bytes omitted]"));
        assert!(long_result
            .output
            .contains("continue with byte_offset=2000"));
        assert!(!long_result.output.contains(&"a".repeat(2500)));
        let short_result = executor
            .execute("read", &serde_json::json!({"file_path": "short.txt"}))
            .await
            .unwrap();
        assert_eq!(short_result.exit_code, 0, "{}", short_result.output);
        assert!(short_result.output.contains("hello"));
        assert!(!short_result.output.contains("line truncated"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn idle_and_non_terminal_retries_still_stop() {
        let mut bound = HashSet::new();
        let mut budget = HostAdmissionBudget::default();
        for _ in 0..(HARBOR_HOST_IDLE_ATTEMPTS - 1) {
            assert_eq!(
                host_admission_step(&mut bound, &mut budget, None, 2, true, false),
                HostAdmissionStep::NudgeBeforeAnyGate,
            );
        }
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, None, 2, true, false),
            HostAdmissionStep::Finish,
        );

        let mut bound = HashSet::new();
        bound.insert(format!("{:064x}", 4u32));
        let mut budget = HostAdmissionBudget::default();
        for _ in 0..(HARBOR_HOST_IDLE_ATTEMPTS - 1) {
            assert_eq!(
                host_admission_step(&mut bound, &mut budget, None, 0, false, true),
                HostAdmissionStep::NudgeIdleAfterBind,
            );
        }
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, None, 0, true, false),
            HostAdmissionStep::Finish,
        );

        let mut bound = HashSet::new();
        let mut budget = HostAdmissionBudget::default();
        for _ in 0..(HARBOR_HOST_IDLE_ATTEMPTS - 1) {
            assert_eq!(
                host_admission_step(&mut bound, &mut budget, None, 4, false, true),
                HostAdmissionStep::RetryNonTerminal,
            );
        }
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, None, 4, false, true),
            HostAdmissionStep::NonTerminalExhausted,
        );
    }

    #[test]
    fn execution_deadline_is_reached_when_elapsed_meets_the_budget() {
        assert!(!execution_deadline_reached(0, 1));
        assert!(execution_deadline_reached(10, 10));
        assert!(execution_deadline_reached(11, 10));
    }

    #[test]
    fn harbor_host_report_binds_passed_required_check() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let report = harbor_host_verification_report(digest);
        assert_eq!(report.schema, VERIFICATION_REPORT_SCHEMA);
        assert_eq!(report.effect_digest.as_deref(), Some(digest));
        assert_eq!(report.status, VerificationStatus::Passed);
        assert!(report
            .checks
            .iter()
            .all(|check| { check.required && check.status == VerificationStatus::Passed }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandbox_timeout_terminates_the_command_process_group() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let sandbox = ProcessHostBashSandbox::new(
            directory.path().to_path_buf(),
            Some(Instant::now() + Duration::from_secs(5)),
        );
        let output = sandbox
            .exec(SandboxCommandRequest {
                command: "sleep 5".to_string(),
                guest_workspace: directory.path().to_string_lossy().into_owned(),
                timeout_ms: 20,
                output_observer: None,
                env: None,
            })
            .await
            .expect("sandbox execution");
        assert!(output.timed_out);
        assert_eq!(output.exit_code, 124);
    }
}
