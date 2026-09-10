use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_session_options_builders() {
    let opts = SessionOptions::new()
        .with_session_id("test-id")
        .with_auto_save(true)
        .with_tool_timeout(5_000)
        .with_llm_api_timeout(30_000)
        .with_max_parallel_tasks(3)
        .with_auto_delegation_enabled(true)
        .with_manual_delegation_enabled(false)
        .with_auto_parallel_delegation(false)
        .with_active_skill_tool_restrictions(true);
    assert_eq!(opts.session_id, Some("test-id".to_string()));
    assert!(opts.auto_save);
    assert_eq!(opts.tool_timeout_ms, Some(5_000));
    assert_eq!(opts.llm_api_timeout_ms, Some(30_000));
    assert_eq!(opts.max_parallel_tasks, Some(3));
    assert_eq!(opts.manual_delegation_enabled, Some(false));
    assert_eq!(opts.auto_parallel_delegation, Some(false));
    assert_eq!(opts.enforce_active_skill_tool_restrictions, Some(true));
    let auto = opts.auto_delegation.expect("auto delegation config");
    assert!(auto.enabled);
    assert!(!auto.allow_manual_delegation);
    assert!(!auto.auto_parallel);

    let resilient = SessionOptions::new().with_resilience_defaults();
    assert_eq!(resilient.max_parse_retries, Some(2));
    assert_eq!(resilient.tool_timeout_ms, Some(120_000));
    assert_eq!(resilient.llm_api_timeout_ms, Some(120_000));
    assert_eq!(resilient.circuit_breaker_threshold, Some(3));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_active_skill_tool_restriction_option_defaults_and_overrides() {
    let agent = Agent::from_config(test_config()).await.unwrap();

    let default_session = agent
        .session_async("/tmp/test-ws-skill-restriction-default", None)
        .await
        .unwrap();
    assert!(
        !default_session
            .config
            .enforce_active_skill_tool_restrictions
    );

    let legacy_session = agent
        .session_async(
            "/tmp/test-ws-skill-restriction-legacy",
            Some(SessionOptions::new().with_active_skill_tool_restrictions(true)),
        )
        .await
        .unwrap();
    assert!(legacy_session.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_options_with_rl_trajectory_records_jsonl() {
    let dir = tempfile::TempDir::new().unwrap();
    let trajectory_path = dir.path().join("trajectory.jsonl");
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new().with_rl_trajectory(
        crate::rl_trajectory::RlTrajectoryConfig::new(&trajectory_path).with_max_text_bytes(4),
    );
    let session = agent
        .session_async("/tmp/test-ws-rl-trajectory", Some(opts))
        .await
        .unwrap();

    assert!(session.config.rl_trajectory_recorder.is_enabled());
    session
        .config
        .rl_trajectory_recorder
        .record_execution_start(crate::rl_trajectory::ExecutionStartRecord {
            session_id: "sess-rl",
            workspace: std::path::Path::new("/tmp/test-ws-rl-trajectory"),
            prompt: "abcdef",
            history: &[],
            system_prompt: None,
            max_tool_rounds: 16,
            planning_mode: "disabled",
        });

    let content = std::fs::read_to_string(&trajectory_path).unwrap();
    let record: serde_json::Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(record["schema"], crate::rl_trajectory::RL_TRAJECTORY_SCHEMA);
    assert_eq!(record["event_type"], "execution_start");
    assert_eq!(record["session_id"], "sess-rl");
    assert_eq!(record["payload"]["prompt"]["text"], "abcd");
    assert_eq!(record["payload"]["prompt"]["truncated"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_max_parallel_tasks_config_and_override() {
    let mut config = test_config();
    config.max_parallel_tasks = Some(6);
    config.auto_delegation.enabled = true;
    config.auto_delegation.auto_parallel = false;
    let agent = Agent::from_config(config).await.unwrap();

    let default_session = agent
        .session_async("/tmp/test-ws-parallel-default", None)
        .await
        .unwrap();
    assert_eq!(default_session.config.max_parallel_tasks, 6);
    assert!(default_session.config.auto_delegation.enabled);
    assert!(!default_session.config.auto_delegation.auto_parallel);

    let override_session = agent
        .session_async(
            "/tmp/test-ws-parallel-override",
            Some(
                SessionOptions::new()
                    .with_max_parallel_tasks(2)
                    .with_auto_parallel_delegation(true),
            ),
        )
        .await
        .unwrap();
    assert_eq!(override_session.config.max_parallel_tasks, 2);
    assert!(override_session.config.auto_delegation.auto_parallel);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_auto_parallel_override_preserves_base_auto_delegation() {
    let mut config = test_config();
    config.auto_delegation.enabled = true;
    config.auto_delegation.auto_parallel = true;
    let agent = Agent::from_config(config).await.unwrap();

    let session = agent
        .session_async(
            "/tmp/test-ws-auto-parallel-only",
            Some(SessionOptions::new().with_auto_parallel_delegation(false)),
        )
        .await
        .unwrap();

    assert!(session.config.auto_delegation.enabled);
    assert!(!session.config.auto_delegation.auto_parallel);
}

// ========================================================================
// Memory Integration Tests
// ========================================================================

