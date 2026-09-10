use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_session_tool_timeout_configured() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_tool_timeout(5000)
        .with_llm_api_timeout(30_000);
    let session = agent
        .session_async("/tmp/test-ws-timeout", Some(opts))
        .await
        .unwrap();
    assert!(!session.id().is_empty());
    assert_eq!(session.config.tool_timeout_ms, Some(5_000));
    assert_eq!(session.config.llm_api_timeout_ms, Some(30_000));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_llm_api_timeout_does_not_configure_tool_timeout() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_llm_api_timeout(30_000);
    let session = agent
        .session_async("/tmp/test-ws-api-timeout-only", Some(opts))
        .await
        .unwrap();

    assert_eq!(session.config.llm_api_timeout_ms, Some(30_000));
    assert_eq!(
        session.config.tool_timeout_ms, None,
        "model API timeout must not also constrain tool execution"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_duplicate_tool_call_threshold_configured() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_duplicate_tool_call_threshold(12);
    let session = agent
        .session_async("/tmp/test-ws-duplicate-threshold", Some(opts))
        .await
        .unwrap();

    assert_eq!(session.config.duplicate_tool_call_threshold, 12);
    assert_eq!(
        session.parent_run_context().duplicate_tool_call_threshold,
        Some(12),
        "delegated child runs must inherit the same repeated-tool guard"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_timeout_does_not_configure_tool_timeout() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let confirmation = crate::hitl::ConfirmationPolicy::enabled()
        .with_timeout(5_000, crate::hitl::TimeoutAction::Reject);
    let opts = SessionOptions::new().with_confirmation_policy(confirmation);
    let session = agent
        .session_async("/tmp/test-ws-confirmation-timeout-only", Some(opts))
        .await
        .unwrap();

    assert!(session.config.confirmation_manager.is_some());
    assert_eq!(
        session
            .config
            .confirmation_policy
            .as_ref()
            .map(|policy| policy.default_timeout_ms),
        Some(5_000)
    );
    assert_eq!(
        session.config.tool_timeout_ms, None,
        "HITL confirmation waiting must not consume or configure the tool execution timeout budget"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_and_tool_timeouts_remain_independent() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let confirmation = crate::hitl::ConfirmationPolicy::enabled()
        .with_timeout(5_000, crate::hitl::TimeoutAction::Reject);
    let opts = SessionOptions::new()
        .with_confirmation_policy(confirmation)
        .with_tool_timeout(300);
    let session = agent
        .session_async(
            "/tmp/test-ws-independent-confirmation-tool-timeouts",
            Some(opts),
        )
        .await
        .unwrap();

    assert_eq!(
        session
            .config
            .confirmation_policy
            .as_ref()
            .map(|policy| policy.default_timeout_ms),
        Some(5_000)
    );
    assert_eq!(
        session.config.tool_timeout_ms,
        Some(300),
        "tool timeout must stay as the explicit execution budget, not the HITL wait budget"
    );
}

// ========================================================================
// Queue fallback tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_without_queue_builds_ok() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-no-queue", None)
        .await
        .unwrap();
    assert!(!session.id().is_empty());
}

// ========================================================================
// Concurrent history access tests
// ========================================================================
