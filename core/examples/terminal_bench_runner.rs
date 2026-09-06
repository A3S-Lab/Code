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

mod terminal_bench_result;
mod terminal_bench_sandbox;

use a3s_code_core::execution_identity::ExecutionResultOutcomeV1;
use a3s_code_core::hitl::AutoApproveConfirmation;
use a3s_code_core::llm::CodexLoginClient;
use a3s_code_core::{
    Agent, AgentEvent, AgentStyle, PlanningMode, SessionOptions, SystemPromptSlots,
};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use terminal_bench_result::{
    classify_failure, count_artifact_evidence, persist_result, ExecutionPhase, RunProgress,
    TerminalReason, DEFAULT_EXECUTION_BUDGET_MS,
};

use terminal_bench_sandbox::ContainerBashSandbox;

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
            ..SystemPromptSlots::default()
        })
        // Harbor supplies the complete task instruction. Disable A3S planning
        // pre-analysis so the model receives that instruction verbatim as the
        // user message instead of a planner-rewritten wrapper.
        .with_planning_mode(PlanningMode::Disabled)
        .with_resilience_defaults()
        .with_auto_compact(true)
        // Terminal-Bench includes long-horizon tasks whose official agent
        // timeout is measured in hours. Keep the framework's internal
        // progress budget below that outer deadline without truncating a
        // valid solution after a short fixed number of tool turns.
        .with_max_tool_rounds(256)
        .with_max_continuation_turns(8)
        .with_sandbox_handle(Arc::new(ContainerBashSandbox::new(
            args.workspace.clone(),
            progress
                .started_at
                .checked_add(Duration::from_millis(args.max_execution_time_ms))
                .unwrap_or_else(Instant::now),
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
    let (mut events, worker) = session
        .stream(&task_prompt, None)
        .await
        .context("start A3S Code stream")?;
    eprintln!("a3s-code: stream started");
    progress.phase = ExecutionPhase::Streaming;
    while let Some(event) = events.recv().await {
        match event {
            AgentEvent::TextDelta { text } => print!("{text}"),
            AgentEvent::TurnStart { turn } => {
                progress.turns = progress.turns.max(turn);
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
            }
            AgentEvent::End { .. } => progress.terminal_event = true,
            _ => {}
        }
    }
    progress.phase = ExecutionPhase::Joining;
    let worker_error = worker.await.err();
    // Session close is unconditional: even a failed worker join must release
    // the Run-owned sandbox, capability, and event resources before execute
    // returns to the process boundary.
    session.close().await;
    if let Some(error) = worker_error {
        progress.remember_error(&error);
        // An End event is the first terminal observation. Preserve it when a
        // late worker join failure occurs during cleanup; this mirrors the
        // monotonic RunStore terminal transition instead of turning a valid
        // result into a runner-only failure.
        if !progress.terminal_event {
            return Err(error).context("join A3S Code stream");
        }
        eprintln!("[a3s-code warning] worker ended after terminal event: {error}");
    }
    if !progress.terminal_event {
        progress.stream_closed_without_terminal_event = true;
        anyhow::bail!("A3S Code stream ended without a terminal event")
    }
    Ok(())
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

    #[cfg(unix)]
    #[tokio::test]
    async fn sandbox_timeout_terminates_the_command_process_group() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let sandbox = ContainerBashSandbox::new(
            directory.path().to_path_buf(),
            Instant::now() + Duration::from_secs(5),
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
