//! Terminal-Bench result and evidence state for the headless runner.
//!
//! This module is intentionally adapter-local. The generic Code run store and
//! evaluation substrate remain provider-neutral; Harbor needs this smaller
//! process-boundary record to classify runner failures without scraping logs.

use a3s_code_core::execution_identity::ExecutionResultOutcomeV1;
use anyhow::{Context, Result};
use serde::Serialize;
use std::time::Instant;

pub(super) const DEFAULT_EXECUTION_BUDGET_MS: u64 = 840_000;
const TERMINAL_BENCH_RESULT_SCHEMA_V1: &str = "a3s.code.terminal-bench-result.v1";
const MAX_REPORTED_ERROR_BYTES: usize = 4 * 1024;

/// Stable host-facing reason for why a Terminal-Bench run stopped.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminalReason {
    AgentEnd,
    StreamClosedWithoutTerminalEvent,
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
pub(super) struct TerminalBenchRunResultV1 {
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
pub(super) enum ExecutionPhase {
    Startup,
    Streaming,
    Joining,
}

#[derive(Debug)]
pub(super) struct RunProgress {
    pub(super) started_at: Instant,
    pub(super) phase: ExecutionPhase,
    pub(super) turns: usize,
    pub(super) tool_calls: usize,
    pub(super) successful_tool_calls: usize,
    pub(super) artifact_evidence_count: usize,
    pub(super) error_count: usize,
    pub(super) provider: Option<String>,
    pub(super) provider_status: Option<u16>,
    pub(super) terminal_event: bool,
    pub(super) stream_closed_without_terminal_event: bool,
    pub(super) evidence_missing: bool,
    last_error: Option<String>,
}

impl RunProgress {
    pub(super) fn new() -> Self {
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
            evidence_missing: false,
            last_error: None,
        }
    }

    pub(super) fn duration_ms(&self) -> u64 {
        self.started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    pub(super) fn remember_error(&mut self, error: impl std::fmt::Display) {
        self.last_error = Some(bound_error(error.to_string()));
    }

    pub(super) fn remember_failure(&mut self, error: &anyhow::Error) {
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

    pub(super) fn has_action_evidence(&self) -> bool {
        self.successful_tool_calls > 0
    }

    pub(super) fn mark_evidence_missing(&mut self) {
        self.evidence_missing = true;
        self.remember_error("agent ended without a successful tool execution");
    }

    pub(super) fn report(
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

pub(super) fn count_artifact_evidence(value: &serde_json::Value) -> usize {
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

pub(super) async fn persist_result(
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

pub(super) fn classify_failure(
    progress: &RunProgress,
    error: &anyhow::Error,
) -> (ExecutionResultOutcomeV1, TerminalReason) {
    if progress.evidence_missing {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::EvidenceMissing,
        );
    }
    if progress.stream_closed_without_terminal_event {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::StreamClosedWithoutTerminalEvent,
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
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("deadline") || message.contains("execution timeout") {
        return (
            ExecutionResultOutcomeV1::TimedOut,
            TerminalReason::DeadlineExceeded,
        );
    }
    // Older Agent paths may have rendered the typed exhaustion into a plain
    // anyhow message. Keep the narrow prefix fallback for those records while
    // preferring the public typed projection above whenever it survives.
    if message.contains("llm api request failed after") {
        return (
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderExhausted,
        );
    }
    if message.contains("circuit breaker") || message.contains("provider") {
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
mod tests {
    use super::*;

    #[test]
    fn report_serializes_terminal_reason_and_counters() {
        let mut progress = RunProgress::new();
        progress.turns = 3;
        progress.tool_calls = 5;
        progress.successful_tool_calls = 4;
        progress.artifact_evidence_count = 2;
        progress.error_count = 1;
        progress.provider = Some("openai".to_string());
        progress.provider_status = Some(402);
        progress.terminal_event = true;
        let report = progress.report(
            ExecutionResultOutcomeV1::Succeeded,
            TerminalReason::AgentEnd,
        );
        let json = serde_json::to_value(report).expect("report JSON");
        assert_eq!(json["schema"], TERMINAL_BENCH_RESULT_SCHEMA_V1);
        assert_eq!(json["outcome"], "succeeded");
        assert_eq!(json["reason"], "agent_end");
        assert_eq!(json["terminalEvent"], true);
        assert_eq!(json["turns"], 3);
        assert_eq!(json["toolCalls"], 5);
        assert_eq!(json["successfulToolCalls"], 4);
        assert_eq!(json["artifactEvidenceCount"], 2);
        assert_eq!(json["errorCount"], 1);
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["providerStatus"], 402);
    }

    #[test]
    fn reported_errors_are_bounded_without_breaking_utf8() {
        let error = bound_error("错误".repeat(MAX_REPORTED_ERROR_BYTES));
        assert!(error.is_char_boundary(error.len()));
        assert!(error.len() <= MAX_REPORTED_ERROR_BYTES);
        assert!(error.ends_with('…'));
    }

    #[test]
    fn stream_eof_is_reported_as_a_failed_terminal_reason() {
        let mut progress = RunProgress::new();
        progress.stream_closed_without_terminal_event = true;
        let error = anyhow::anyhow!("stream ended");
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
        assert!(matches!(
            reason,
            TerminalReason::StreamClosedWithoutTerminalEvent
        ));
    }

    #[test]
    fn execution_deadline_is_reported_as_timed_out() {
        let progress = RunProgress::new();
        let error = anyhow::anyhow!("Execution timeout exceeded");
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::TimedOut));
        assert!(matches!(reason, TerminalReason::DeadlineExceeded));
    }

    #[test]
    fn missing_action_evidence_is_a_terminal_failure() {
        let mut progress = RunProgress::new();
        progress.terminal_event = true;
        progress.mark_evidence_missing();
        let error = anyhow::anyhow!("agent ended without evidence");
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
        assert!(matches!(reason, TerminalReason::EvidenceMissing));
    }

    #[test]
    fn typed_provider_response_is_classified_without_text_matching() {
        let progress = RunProgress::new();
        let error = anyhow::Error::new(a3s_code_core::llm::NonRetryableLlmError::from_status(
            "openai", 402, "quota",
        ));
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
        assert!(matches!(reason, TerminalReason::ProviderRejected));
    }

    #[test]
    fn typed_provider_response_wins_over_deadline_words_in_body() {
        let progress = RunProgress::new();
        let error = anyhow::Error::new(a3s_code_core::llm::NonRetryableLlmError::from_status(
            "openai",
            400,
            "request deadline rejected by provider",
        ));
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
        assert!(matches!(reason, TerminalReason::ProviderRejected));
    }

    #[test]
    fn retry_exhaustion_is_classified_as_provider_exhausted() {
        let progress = RunProgress::new();
        let error = anyhow::anyhow!(
            "LLM API request failed after 11 attempts. Last status: 429 Body: rate limited"
        );
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
        assert!(matches!(reason, TerminalReason::ProviderExhausted));
    }

    #[test]
    fn typed_retry_exhaustion_is_classified_without_rendered_text() {
        let progress = RunProgress::new();
        let error = anyhow::Error::new(a3s_code_core::llm::RetryExhaustedError::from_status(
            2,
            503,
            "service unavailable",
        ));
        let (outcome, reason) = classify_failure(&progress, &error);
        assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
        assert!(matches!(reason, TerminalReason::ProviderExhausted));
    }

    #[test]
    fn typed_provider_failure_metadata_is_retained_in_report() {
        let mut progress = RunProgress::new();
        let error = anyhow::Error::new(a3s_code_core::llm::NonRetryableLlmError::from_status(
            "anthropic",
            401,
            "denied",
        ));
        progress.remember_failure(&error);
        let report = progress.report(
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderRejected,
        );
        let json = serde_json::to_value(report).expect("report JSON");
        assert_eq!(json["provider"], "anthropic");
        assert_eq!(json["providerStatus"], 401);
    }

    #[test]
    fn typed_retry_failure_status_is_retained_in_report() {
        let mut progress = RunProgress::new();
        let error = anyhow::Error::new(a3s_code_core::llm::RetryExhaustedError::from_status(
            3,
            429,
            "rate limited",
        ));
        progress.remember_failure(&error);
        let report = progress.report(
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ProviderExhausted,
        );
        let json = serde_json::to_value(report).expect("report JSON");
        assert_eq!(json["providerStatus"], 429);
    }

    #[test]
    fn artifact_evidence_counter_walks_nested_tool_metadata() {
        let metadata = serde_json::json!({
            "artifact_uri": "a3s://one",
            "nested": [{"artifact_uri": "a3s://two"}, {"other": true}]
        });
        assert_eq!(count_artifact_evidence(&metadata), 2);
    }

    #[tokio::test]
    async fn result_persistence_is_atomic_and_replaces_previous_report() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("a3s-code.result.json");
        let mut progress = RunProgress::new();
        progress.terminal_event = true;
        let first = progress.report(
            ExecutionResultOutcomeV1::Succeeded,
            TerminalReason::AgentEnd,
        );
        persist_result(&path, &first)
            .await
            .expect("write first report");

        let second = progress.report(
            ExecutionResultOutcomeV1::Failed,
            TerminalReason::ExecutionFailed,
        );
        persist_result(&path, &second)
            .await
            .expect("replace report");
        let bytes = tokio::fs::read(&path).await.expect("read report");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid report JSON");
        assert_eq!(value["outcome"], "failed");
        assert_eq!(value["reason"], "execution_failed");
        let entries = std::fs::read_dir(directory.path())
            .expect("list result directory")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("read result directory entries");
        assert_eq!(entries.len(), 1, "temporary file must not remain");
    }
}
