//! Minimal headless A3S Code runner used by the Harbor Terminal-Bench adapter.
//!
//! The benchmark harness owns the container and verifier. This example keeps the
//! agent boundary in `a3s-code-core`: it loads the caller's ACL, binds one
//! session to the task workspace, and exposes the native Code tools through the
//! normal session loop. No MCP transport or benchmark-specific tool executor is
//! involved.
//!
//! Usage:
//! `terminal_bench_runner --config /run/a3s/config.acl --workspace /root --prompt-file /run/a3s/instruction.md`

use a3s_code_core::hitl::AutoApproveConfirmation;
use a3s_code_core::llm::CodexLoginClient;
use a3s_code_core::sandbox::{
    BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput,
};
use a3s_code_core::{
    Agent, AgentEvent, AgentStyle, PlanningMode, SessionOptions, SystemPromptSlots,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

/// Harbor already supplies the outer container boundary. A nested bubblewrap
/// sandbox is not available in many benchmark images, so this explicit host
/// adapter executes through that container boundary instead of asking the
/// model to retry a permanently unavailable default sandbox.
struct ContainerBashSandbox {
    workspace: PathBuf,
}

impl ContainerBashSandbox {
    fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl BashSandbox for ContainerBashSandbox {
    async fn exec_command(&self, command: &str, guest_workspace: &str) -> Result<SandboxOutput> {
        let output = self
            .exec(SandboxCommandRequest {
                command: command.to_string(),
                guest_workspace: guest_workspace.to_string(),
                timeout_ms: 120_000,
                output_observer: None,
                env: None,
            })
            .await?;
        Ok(SandboxOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        })
    }

    async fn exec(&self, request: SandboxCommandRequest) -> Result<SandboxExecutionOutput> {
        let mut shell = Command::new("bash");
        shell
            .arg("-lc")
            .arg(&request.command)
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(env) = request.env.as_deref() {
            shell.envs(env);
        }
        let child = shell.spawn().context("spawn container bash")?;
        let result = tokio::time::timeout(
            tokio::time::Duration::from_millis(request.timeout_ms.max(1)),
            child.wait_with_output(),
        )
        .await;
        let (stdout, stderr, exit_code, timed_out) = match result {
            Ok(output) => {
                let output = output.context("wait for container bash")?;
                (
                    String::from_utf8_lossy(&output.stdout).into_owned(),
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                    output.status.code().unwrap_or(1),
                    false,
                )
            }
            Err(_) => (
                String::new(),
                "command timed out in the Harbor container\n".to_string(),
                124,
                true,
            ),
        };
        if let Some(observer) = request.output_observer {
            if !stdout.is_empty() {
                observer.on_output_delta(&stdout).await;
            }
            if !stderr.is_empty() {
                observer.on_output_delta(&stderr).await;
            }
            observer
                .on_output_complete(&a3s_code_core::workspace::CommandOutputSummary {
                    total_bytes: stdout.len() + stderr.len(),
                    captured_bytes: stdout.len() + stderr.len(),
                    truncated: false,
                    timed_out,
                })
                .await;
        }
        Ok(SandboxExecutionOutput {
            stdout,
            stderr,
            exit_code,
            timed_out,
        })
    }

    async fn shutdown(&self) {}
}

#[derive(Debug)]
struct Args {
    config: PathBuf,
    workspace: PathBuf,
    prompt_file: PathBuf,
    codex_auth: Option<PathBuf>,
    codex_model: Option<String>,
    codex_reasoning_effort: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut args = std::env::args_os().skip(1);
    let mut config = None;
    let mut workspace = None;
    let mut prompt_file = None;
    let mut codex_auth = None;
    let mut codex_model = None;
    let mut codex_reasoning_effort = None;
    while let Some(flag) = args.next() {
        let flag = flag.to_string_lossy();
        let value = args
            .next()
            .with_context(|| format!("missing value for {flag}"))?;
        match flag.as_ref() {
            "--config" => config = Some(PathBuf::from(value)),
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--prompt-file" => prompt_file = Some(PathBuf::from(value)),
            "--codex-auth" => codex_auth = Some(PathBuf::from(value)),
            "--codex-model" => codex_model = Some(value.to_string_lossy().into_owned()),
            "--codex-reasoning-effort" => {
                codex_reasoning_effort = Some(value.to_string_lossy().into_owned())
            }
            "--help" | "-h" => {
                println!(
                    "usage: terminal_bench_runner --config PATH --workspace PATH --prompt-file PATH [--codex-auth PATH --codex-model MODEL --codex-reasoning-effort EFFORT]"
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    Ok(Args {
        config: config.context("--config is required")?,
        workspace: workspace.context("--workspace is required")?,
        prompt_file: prompt_file.context("--prompt-file is required")?,
        codex_auth,
        codex_model,
        codex_reasoning_effort,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
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
        .with_max_tool_rounds(64)
        .with_max_continuation_turns(2)
        .with_sandbox_handle(Arc::new(ContainerBashSandbox::new(args.workspace.clone())));
    if let Some(auth_path) = args.codex_auth {
        let model = args
            .codex_model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .context("--codex-model is required with --codex-auth")?;
        let client = CodexLoginClient::from_auth_file(
            auth_path,
            model,
            "terminal-bench-a3s-code",
            args.codex_reasoning_effort,
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
    let mut completed = false;
    while let Some(event) = events.recv().await {
        match event {
            AgentEvent::TextDelta { text } => print!("{text}"),
            AgentEvent::ToolStart { name, .. } => {
                eprintln!("[a3s-code tool={name}]");
            }
            AgentEvent::Error { message } => {
                eprintln!("[a3s-code error] {message}");
            }
            AgentEvent::End { .. } => completed = true,
            _ => {}
        }
    }
    worker.await.context("join A3S Code stream")?;
    session.close().await;
    if !completed {
        anyhow::bail!("A3S Code stream ended without a terminal event")
    }
    Ok(())
}
