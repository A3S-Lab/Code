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
fn remembered_http_402_outranks_stream_closed_class() {
    let mut progress = RunProgress::new();
    progress.stream_closed_without_terminal_event = true;
    progress
        .remember_error("deepseek API returned HTTP 402: Insufficient Balance (request_id: test)");
    let error = anyhow::anyhow!("A3S Code stream ended without a terminal event");
    let (outcome, reason) = classify_failure(&progress, &error);
    assert!(matches!(outcome, ExecutionResultOutcomeV1::Failed));
    assert!(matches!(reason, TerminalReason::ProviderRejected));
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
