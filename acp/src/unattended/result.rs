//! Terminal-Bench result and evidence state for an unattended ACP run.
//!
//! The record matches the native `terminal_bench_runner` schema field for
//! field, so Harbor reads either lane the same way. It stays adapter-local:
//! the generic Code run store remains provider-neutral.

use a3s_code_core::execution_identity::ExecutionResultOutcomeV1;
use anyhow::{Context, Result};
use serde::Serialize;
use std::time::Instant;

const TERMINAL_BENCH_RESULT_SCHEMA_V1: &str = "a3s.code.terminal-bench-result.v1";
const MAX_REPORTED_ERROR_BYTES: usize = 4 * 1024;

/// Stable host-facing reason for why a Terminal-Bench run stopped.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalReason {
    AgentEnd,
    StreamClosedWithoutTerminalEvent,
    /// The worker joined successfully and produced no error and no end event.
    /// This is not sixteen silent retries of the generic stream-closed text.
    WorkerExitedWithoutTerminalEvent,
    WorkerJoinFailed,
    StartupFailed,
    ExecutionFailed,
    EvidenceMissing,
    ProviderRejected,
    ProviderExhausted,
    DeadlineExceeded,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalBenchRunResultV1 {
    schema: &'static str,
    outcome: ExecutionResultOutcomeV1,
    reason: TerminalReason,
    terminal_event: bool,
    exit_code: i32,
    duration_ms: u64,
    turns: usize,
    tool_calls: usize,
    successful_tool_calls: usize,
    artifact_evidence_count: usize,
    error_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionPhase {
    Startup,
    Streaming,
    Joining,
}

#[derive(Debug)]
pub(crate) struct RunProgress {
    pub(crate) started_at: Instant,
    pub(crate) phase: ExecutionPhase,
    pub(crate) turns: usize,
    pub(crate) tool_calls: usize,
    pub(crate) successful_tool_calls: usize,
    pub(crate) artifact_evidence_count: usize,
    pub(crate) error_count: usize,
    pub(crate) provider: Option<String>,
    pub(crate) provider_status: Option<u16>,
    pub(crate) terminal_event: bool,
    pub(crate) stream_closed_without_terminal_event: bool,
    pub(crate) worker_exited_without_terminal_event: bool,
    pub(crate) evidence_missing: bool,
    last_error: Option<String>,
}

impl RunProgress {
    pub(crate) fn new() -> Self {
        Self {
            started_at: Instant::now(),
            phase: ExecutionPhase::Startup,
            turns: 0,
            tool_calls: 0,
            successful_tool_calls: 0,
            artifact_evidence_count: 0,
            error_count: 0,
            provider: None,
            provider_status: None,
            terminal_event: false,
            stream_closed_without_terminal_event: false,
            worker_exited_without_terminal_event: false,
            evidence_missing: false,
            last_error: None,
        }
    }

    pub(crate) fn duration_ms(&self) -> u64 {
        self.started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    pub(crate) fn remember_error(&mut self, error: impl std::fmt::Display) {
        let next = bound_error(error.to_string());
        match self.last_error.as_deref() {
            None => self.last_error = Some(next),
            Some(current)
                if is_generic_stream_close(current) && !is_generic_stream_close(&next) =>
            {
                self.last_error = Some(next);
            }
            Some(_) => {}
        }
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(crate) fn mark_worker_exited_without_terminal_event(&mut self) {
        self.worker_exited_without_terminal_event = true;
    }

    pub(crate) fn remember_failure(&mut self, error: &anyhow::Error) {
        self.remember_error(error);
        if let Some(provider_error) =
            error.downcast_ref::<a3s_code_core::llm::NonRetryableLlmError>()
        {
            self.provider = provider_error.provider().map(str::to_owned);
            self.provider_status = provider_error.status();
        } else if let Some(retry_error) =
            error.downcast_ref::<a3s_code_core::llm::RetryExhaustedError>()
        {
            self.provider_status = Some(retry_error.status_code());
        }
    }

    pub(crate) fn has_action_evidence(&self) -> bool {
        self.successful_tool_calls > 0
    }

    pub(crate) fn mark_evidence_missing(&mut self) {
        self.evidence_missing = true;
        self.remember_error("agent ended without a successful tool execution");
    }

    pub(crate) fn report(
        &self,
        outcome: ExecutionResultOutcomeV1,
        reason: TerminalReason,
    ) -> TerminalBenchRunResultV1 {
        TerminalBenchRunResultV1 {
            schema: TERMINAL_BENCH_RESULT_SCHEMA_V1,
            outcome,
            reason,
            terminal_event: self.terminal_event,
            exit_code: if matches!(outcome, ExecutionResultOutcomeV1::Succeeded) {
                0
            } else {
                1
            },
            duration_ms: self.duration_ms(),
            turns: self.turns,
            tool_calls: self.tool_calls,
            successful_tool_calls: self.successful_tool_calls,
            artifact_evidence_count: self.artifact_evidence_count,
            error_count: self.error_count,
            provider: self.provider.clone(),
            provider_status: self.provider_status,
            last_error: self.last_error.clone(),
        }
    }
}

pub(crate) fn is_provider_rejection(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("http 401")
        || text.contains("http 402")
        || text.contains("http 403")
        || text.contains("insufficient balance")
        || text.contains("no balance")
}

pub(crate) fn is_generic_stream_close(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("stream ended without a terminal event")
        || text.contains("stream closed without terminal event")
}

fn provider_status_code(text: &str) -> Option<u16> {
    let lower = text.to_ascii_lowercase();
    for status in [401, 402, 403] {
        if lower.contains(&format!("http {status}")) {
            return Some(status);
        }
    }
    None
}

pub(crate) fn note_provider_rejection(progress: &mut RunProgress, text: &str) {
    if !is_provider_rejection(text) {
        return;
    }
    progress.remember_error(text);
    if progress.provider_status.is_none() {
        progress.provider_status = provider_status_code(text);
    }
}

fn bound_error(error: String) -> String {
    if error.len() <= MAX_REPORTED_ERROR_BYTES {
        return error;
    }
    let mut bounded = String::with_capacity(MAX_REPORTED_ERROR_BYTES);
    for character in error.chars() {
        if bounded.len() + character.len_utf8() + 3 > MAX_REPORTED_ERROR_BYTES {
            break;
        }
        bounded.push(character);
    }
    bounded.push('…');
    bounded
}

pub(crate) fn count_artifact_evidence(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(object) => {
            let own = usize::from(
                object
                    .get("artifact_uri")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
            );
            own + object.values().map(count_artifact_evidence).sum::<usize>()
        }
        serde_json::Value::Array(values) => values.iter().map(count_artifact_evidence).sum(),
        _ => 0,
    }
}

pub(crate) async fn persist_result(
    path: &std::path::Path,
    result: &TerminalBenchRunResultV1,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(result).context("serialize terminal run result")?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create result directory {}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "a3s-code.result.json".to_string());
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    tokio::fs::write(&temporary, bytes)
        .await
        .with_context(|| format!("write temporary result {}", temporary.display()))?;
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).with_context(|| format!("publish result {}", path.display()));
    }
    Ok(())
}

pub(crate) fn classify_failure(
    progress: &RunProgress,
    error: &anyhow::Error,
) -> (ExecutionResultOutcomeV1, TerminalReason) {
    if progress.evidence_missing {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::EvidenceMissing,
        );
    }
    // Provider clients preserve terminal HTTP responses as a typed error. Do
    // not infer this class from rendered text: bodies are bounded and may be
    // localized or omit the word "provider" entirely.
    if error
        .downcast_ref::<a3s_code_core::llm::NonRetryableLlmError>()
        .is_some()
    {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderRejected,
        );
    }
    if error
        .downcast_ref::<a3s_code_core::llm::RetryExhaustedError>()
        .is_some()
    {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderExhausted,
        );
    }
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    let remembered = progress.last_error.as_deref().unwrap_or("");
    // Stream may close without End after a rendered provider rejection (for
    // example DeepSeek HTTP 402, or Boyue reporting an empty account as
    // HTTP 401). Prefer
    // provider_rejected over the generic stream-closed class so TB
    // diagnostics do not hide quota/auth failures.
    if is_provider_rejection(remembered) || is_provider_rejection(&message) {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderRejected,
        );
    }
    if progress.worker_exited_without_terminal_event {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::WorkerExitedWithoutTerminalEvent,
        );
    }
    if progress.stream_closed_without_terminal_event {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::StreamClosedWithoutTerminalEvent,
        );
    }
    if lower.contains("deadline") || lower.contains("execution timeout") {
        return (
            ExecutionResultOutcomeV1::TimedOut,
            TerminalReason::DeadlineExceeded,
        );
    }
    // Older Agent paths may have rendered the typed exhaustion into a plain
    // anyhow message. Keep the narrow prefix fallback for those records while
    // preferring the public typed projection above whenever it survives.
    if lower.contains("llm api request failed after") {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderExhausted,
        );
    }
    if lower.contains("circuit breaker") || lower.contains("provider") {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderExhausted,
        );
    }
    let reason = match progress.phase {
        ExecutionPhase::Startup => TerminalReason::StartupFailed,
        ExecutionPhase::Joining => TerminalReason::WorkerJoinFailed,
        ExecutionPhase::Streaming => TerminalReason::ExecutionFailed,
    };
    (ExecutionResultOutcomeV1::Failed, reason)
}

#[cfg(test)]
#[path = "result_tests.rs"]
mod tests;
