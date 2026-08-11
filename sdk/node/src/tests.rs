use super::*;

#[test]
fn agent_run_spawn_object_preserves_snapshot_and_replay_state() {
    let spawn = RustAgentRunSpawn::Replayed {
        snapshot: a3s_code_core::run::RunSnapshot {
            id: "host-run-1".to_string(),
            session_id: "session-1".to_string(),
            status: a3s_code_core::run::RunStatus::Created,
            prompt: "inspect the workspace".to_string(),
            created_at_ms: 10,
            updated_at_ms: 20,
            result_text: None,
            error: None,
            event_count: 0,
        },
    };

    let object = agent_run_spawn_to_object(&spawn).unwrap();
    assert!(object.replayed);
    assert_eq!(object.snapshot["id"], "host-run-1");
    assert_eq!(object.snapshot["status"], "created");
}

#[test]
fn inline_skill_conversion_is_typed_and_rejects_invalid_input() {
    let skill = inline_skill_to_rust(InlineSkill {
        name: "  live-review  ".to_string(),
        kind: "tool".to_string(),
        content: "Review the current change.".to_string(),
    })
    .unwrap();

    assert_eq!(skill.name, "live-review");
    assert_eq!(skill.kind, RustSkillKind::Tool);
    assert_eq!(skill.content, "Review the current change.");
    assert!(inline_skill_to_rust(InlineSkill {
        name: "live-review".to_string(),
        kind: "unknown".to_string(),
        content: String::new(),
    })
    .is_err());
}

#[test]
fn orchestration_object_conversions_round_trip_fields() {
    let schema = serde_json::json!({ "type": "object" });
    let spec = AgentStepSpecObject {
        task_id: "t1".into(),
        agent: "explore".into(),
        description: "d".into(),
        prompt: "p".into(),
        max_steps: Some(5),
        parent_session_id: Some("parent".into()),
        output_schema: Some(schema.clone()),
    };
    let rust: RustAgentStepSpec = spec.into();
    assert_eq!(rust.task_id, "t1");
    assert_eq!(rust.agent, "explore");
    assert_eq!(rust.max_steps, Some(5));
    assert_eq!(rust.parent_session_id.as_deref(), Some("parent"));
    assert_eq!(rust.output_schema, Some(schema));

    let outcome = RustStepOutcome {
        task_id: "t1".into(),
        session_id: "task-run-t1".into(),
        agent: "explore".into(),
        output: "out".into(),
        success: true,
        structured: Some(serde_json::json!({ "k": 1 })),
        source_anchors: vec![RustToolSourceAnchor {
            tool: "read".into(),
            url_or_path: "docs/source.md".into(),
        }],
    };
    let obj = StepOutcomeObject::from(outcome);
    assert_eq!(obj.task_id, "t1");
    assert!(obj.success);
    assert_eq!(obj.structured, Some(serde_json::json!({ "k": 1 })));
    assert_eq!(obj.source_anchors[0].url_or_path, "docs/source.md");
}

fn sdk_test_config() -> a3s_code_core::CodeConfig {
    a3s_code_core::CodeConfig {
        default_model: Some("openai/gpt-4o".to_string()),
        providers: vec![a3s_code_core::ProviderConfig {
            name: "openai".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![a3s_code_core::ModelConfig {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                family: "gpt-4".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: a3s_code_core::ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

#[test]
fn memory_session_store_identity_survives_save_and_resume() {
    let sdk_store = MemorySessionStore::new();
    let session_id = format!(
        "node-memory-store-{}",
        a3s_code_core::host_env::HostEnv::system().next_id()
    );
    let options = |with_session_id: bool| SessionOptions {
        session_store: Some(JsSessionStore {
            backend: sdk_store.backend.clone(),
            dir: None,
            instance_id: Some(sdk_store.instance_id.clone()),
        }),
        session_id: with_session_id.then(|| session_id.clone()),
        ..Default::default()
    };
    let create_options = js_session_options_to_rust(Some(options(true))).unwrap();
    let resume_options = js_session_options_to_rust(Some(options(false))).unwrap();

    fallback_runtime().block_on(async {
        let agent = RustAgent::from_config(sdk_test_config()).await.unwrap();
        let session = agent
            .session_async(
                "/tmp/a3s-code-node-memory-session-store",
                Some(create_options),
            )
            .await
            .unwrap();
        session.save().await.unwrap();
        drop(session);

        let resumed = agent
            .resume_session_async(&session_id, resume_options)
            .await
            .unwrap();
        assert_eq!(resumed.session_id(), session_id);
    });
}

#[test]
fn memory_session_store_handles_fail_closed_and_do_not_leak() {
    let sdk_store = MemorySessionStore::new();
    let instance_id = sdk_store.instance_id.clone();
    let store = resolve_node_memory_session_store(Some(&instance_id)).unwrap();
    let weak = Arc::downgrade(&store);
    drop(store);
    drop(sdk_store);

    assert!(weak.upgrade().is_none());
    assert!(resolve_node_memory_session_store(Some(&instance_id)).is_err());
    assert!(resolve_node_memory_session_store(Some("forged-memory-store-handle")).is_err());
    assert!(resolve_node_memory_session_store(None).is_err());
    let registry = node_memory_session_store_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    assert!(!registry.contains_key(&instance_id));
}

fn build_test_session() -> Session {
    let agent = fallback_runtime()
        .block_on(RustAgent::from_config(sdk_test_config()))
        .unwrap();
    let session = fallback_runtime()
        .block_on(agent.session_async("/tmp/a3s-code-node-sdk-api", None))
        .unwrap();
    Session {
        inner: Arc::new(session),
    }
}

fn verification_report_json() -> serde_json::Value {
    serde_json::json!({
        "schema": "a3s.verification_report.v1",
        "subject": "sdk:test",
        "status": "passed",
        "checks": [{
            "id": "check:sdk",
            "kind": "test",
            "description": "Run SDK tests",
            "status": "passed",
            "required": true
        }]
    })
}

#[test]
fn artifact_store_limits_maps_to_rust_session_options() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        artifact_store_limits: Some(ArtifactStoreLimits {
            max_artifacts: Some(3.0),
            max_bytes: Some(4096.0),
        }),
        ..Default::default()
    }))
    .unwrap();

    let limits = opts.artifact_store_limits.expect("limits");
    assert_eq!(limits.max_artifacts, 3);
    assert_eq!(limits.max_bytes, 4096);
}

#[test]
fn tool_result_transform_policy_maps_to_rust_session_options() {
    let policy = a3s_code_core::tools::ToolResultTransformPolicyV1::context_efficient();
    let opts = js_session_options_to_rust(Some(SessionOptions {
        tool_result_transform_policy: Some(ToolResultTransformPolicy {
            schema: policy.schema.clone(),
            max_output_bytes: policy.max_output_bytes as f64,
            head_bytes: policy.head_bytes as f64,
            tail_bytes: policy.tail_bytes as f64,
            fold_repeated_lines: policy.fold_repeated_lines,
            repeated_line_threshold: policy.repeated_line_threshold as f64,
            structured_sample_items: policy.structured_sample_items as f64,
        }),
        ..Default::default()
    }))
    .unwrap();

    assert_eq!(opts.tool_result_transform_policy, Some(policy));
}

#[test]
fn session_options_maps_model_context_window() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        auto_compact: Some(true),
        auto_compact_threshold: Some(0.75),
        max_context_tokens: Some(128_000.0),
        ..Default::default()
    }))
    .unwrap();

    assert!(opts.auto_compact);
    assert_eq!(opts.auto_compact_threshold, Some(0.75));
    assert_eq!(opts.max_context_tokens, Some(128_000));
}

#[test]
fn session_options_rejects_invalid_model_context_window() {
    for value in [0.0, -1.0, 128_000.5, f64::NAN] {
        let result = js_session_options_to_rust(Some(SessionOptions {
            max_context_tokens: Some(value),
            ..Default::default()
        }));
        assert!(result.is_err(), "maxContextTokens={value:?} must fail");
    }
}

#[test]
fn artifact_store_limits_rejects_fractional_values() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        artifact_store_limits: Some(ArtifactStoreLimits {
            max_artifacts: Some(1.5),
            max_bytes: Some(4096.0),
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn verification_reports_from_value_accepts_array_and_single_report() {
    let single = verification_reports_from_value(verification_report_json()).unwrap();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].subject, "sdk:test");

    let array =
        verification_reports_from_value(serde_json::json!([verification_report_json()])).unwrap();
    assert_eq!(array.len(), 1);
    assert_eq!(array[0].checks[0].id, "check:sdk");
}

#[test]
fn session_records_verification_reports() {
    let session = build_test_session();
    session
        .record_verification_reports(serde_json::json!([verification_report_json()]))
        .unwrap();

    let reports = session.verification_reports().unwrap();
    assert_eq!(reports.as_array().unwrap().len(), 1);
    assert_eq!(reports[0]["subject"], "sdk:test");

    let summary = session.verification_summary().unwrap();
    assert_eq!(summary["status"], "passed");
}

#[test]
fn session_get_artifact_returns_null_for_missing_uri() {
    let session = build_test_session();
    let artifact = session
        .get_artifact("a3s://tool-output/missing".to_string())
        .unwrap();
    assert!(artifact.is_null());
}

/// Phase 8 alignment: when the Rust core surfaces a typed
/// `ToolErrorKind`, `tool_result_from_core` must round-trip it into
/// `error_kind_json` on the SDK shape. Tests both the JSON envelope
/// and the discriminator (`type`) field.
#[test]
fn tool_result_from_core_threads_error_kind_json() {
    let core_result = a3s_code_core::ToolCallResult {
        name: "edit".to_string(),
        output: "Concurrent modification detected".to_string(),
        exit_code: 1,
        metadata: None,
        error_kind: Some(a3s_code_core::ToolErrorKind::VersionConflict {
            path: "doc.md".to_string(),
            expected: "etag-1".to_string(),
            actual: Some("etag-2".to_string()),
        }),
    };
    let sdk_result = tool_result_from_core(core_result);
    let json_str = sdk_result
        .error_kind_json
        .expect("typed error_kind must round-trip into error_kind_json");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["type"], "version_conflict");
    assert_eq!(parsed["path"], "doc.md");
    assert_eq!(parsed["expected"], "etag-1");
    assert_eq!(parsed["actual"], "etag-2");
}

#[test]
fn tool_result_from_core_leaves_error_kind_json_none_on_success() {
    let core_result = a3s_code_core::ToolCallResult {
        name: "read".to_string(),
        output: "hello".to_string(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    };
    let sdk_result = tool_result_from_core(core_result);
    assert!(sdk_result.error_kind_json.is_none());
}

#[test]
fn planning_mode_parser_accepts_explicit_tristate() {
    assert!(matches!(
        parse_planning_mode("auto").unwrap(),
        RustPlanningMode::Auto
    ));
    assert!(matches!(
        parse_planning_mode("enabled").unwrap(),
        RustPlanningMode::Enabled
    ));
    assert!(matches!(
        parse_planning_mode("disabled").unwrap(),
        RustPlanningMode::Disabled
    ));
    assert!(parse_planning_mode("sometimes").is_err());
}

#[test]
fn planning_mode_takes_precedence_over_legacy_bool() {
    let opts =
        apply_planning_mode(RustSessionOptions::new(), Some("disabled"), Some(true)).unwrap();
    assert!(matches!(opts.planning_mode, RustPlanningMode::Disabled));

    let opts = apply_planning_mode(RustSessionOptions::new(), None, Some(true)).unwrap();
    assert!(matches!(opts.planning_mode, RustPlanningMode::Enabled));
}

#[test]
fn hook_event_type_parser_accepts_harness_control_points() {
    assert!(matches!(
        parse_hook_event_type("pre_planning").unwrap(),
        RustHookEventType::PrePlanning
    ));
    assert!(matches!(
        parse_hook_event_type("post_planning").unwrap(),
        RustHookEventType::PostPlanning
    ));
    assert!(matches!(
        parse_hook_event_type("pre_memory_recall").unwrap(),
        RustHookEventType::PreMemoryRecall
    ));
    assert!(matches!(
        parse_hook_event_type("intent_detection").unwrap(),
        RustHookEventType::IntentDetection
    ));

    let error = parse_hook_event_type("planning").unwrap_err().to_string();
    assert!(error.contains("pre_planning"));
    assert!(error.contains("post_planning"));
}

#[test]
fn session_options_maps_parallel_delegation_controls() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        max_parallel_tasks: Some(3),
        auto_delegation: Some(AutoDelegationOptions {
            enabled: Some(true),
            auto_parallel: Some(true),
            min_confidence: Some(0.8),
            max_tasks: Some(2),
        }),
        auto_parallel: Some(false),
        manual_delegation_enabled: Some(false),
        ..Default::default()
    }))
    .unwrap();

    assert_eq!(opts.max_parallel_tasks, Some(3));
    assert_eq!(opts.auto_parallel_delegation, Some(false));
    assert_eq!(opts.manual_delegation_enabled, Some(false));
    let auto = opts.auto_delegation.expect("auto delegation options");
    assert!(auto.enabled);
    assert!(!auto.auto_parallel);
    assert!((auto.min_confidence - 0.8).abs() < f32::EPSILON);
    assert_eq!(auto.max_tasks, 2);
}

#[test]
fn session_options_maps_resilience_controls() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        tool_timeout_ms: Some(1_000.0),
        llm_api_timeout_ms: Some(2_000.0),
        circuit_breaker_threshold: Some(4),
        duplicate_tool_call_threshold: Some(5),
        ..Default::default()
    }))
    .unwrap();

    assert_eq!(opts.tool_timeout_ms, Some(1_000));
    assert_eq!(opts.llm_api_timeout_ms, Some(2_000));
    assert_eq!(opts.circuit_breaker_threshold, Some(4));
    assert_eq!(opts.duplicate_tool_call_threshold, Some(5));
}

#[test]
fn session_options_maps_active_skill_tool_restriction_control() {
    let default_opts = js_session_options_to_rust(Some(SessionOptions {
        ..Default::default()
    }))
    .unwrap();
    assert_eq!(default_opts.enforce_active_skill_tool_restrictions, None);

    let legacy_opts = js_session_options_to_rust(Some(SessionOptions {
        enforce_active_skill_tool_restrictions: Some(true),
        ..Default::default()
    }))
    .unwrap();
    assert_eq!(
        legacy_opts.enforce_active_skill_tool_restrictions,
        Some(true)
    );
}

#[test]
fn session_options_maps_rl_trajectory_controls() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        trajectory_path: Some("/tmp/a3s-trajectory.jsonl".to_string()),
        trajectory_mode: Some("on".to_string()),
        trajectory_max_text_bytes: Some(1234),
        trajectory_include_messages: Some(false),
        ..Default::default()
    }))
    .unwrap();

    let config = opts.rl_trajectory.expect("trajectory config");
    assert_eq!(
        config.path,
        std::path::PathBuf::from("/tmp/a3s-trajectory.jsonl")
    );
    assert_eq!(config.mode, a3s_code_core::RlTrajectoryMode::On);
    assert_eq!(config.max_text_bytes, 1234);
    assert!(!config.include_messages);
}

#[test]
fn session_options_maps_llm_logprob_controls() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        llm_logprobs: Some(true),
        llm_top_logprobs: Some(1),
        ..Default::default()
    }))
    .unwrap();

    assert_eq!(opts.llm_logprobs, Some(true));
    assert_eq!(opts.llm_top_logprobs, Some(1));
}

#[test]
fn session_options_map_deterministic_host_env() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        host_env: Some(HostEnvOptions {
            sequential_id_prefix: Some("replay".to_string()),
            fixed_time_ms: Some(1_700_000_000_000.0),
        }),
        ..Default::default()
    }))
    .unwrap();

    let host_env = opts.host_env.expect("host env");
    assert_eq!(host_env.next_id(), "replay-0");
    assert_eq!(host_env.next_id(), "replay-1");
    assert_eq!(host_env.now_ms(), 1_700_000_000_000);
}

#[test]
fn session_options_reject_invalid_fixed_time() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        host_env: Some(HostEnvOptions {
            sequential_id_prefix: None,
            fixed_time_ms: Some(-1.0),
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn session_options_map_every_retention_limit() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        retention_limits: Some(RetentionLimitsObject {
            unbounded: Some(false),
            max_runs_retained: Some(10),
            max_events_per_run: Some(20),
            max_event_bytes_per_run: Some(30),
            max_trace_events: Some(40),
            max_terminal_subagent_tasks: Some(50),
        }),
        ..Default::default()
    }))
    .unwrap();

    let limits = opts.retention_limits.expect("retention limits");
    assert_eq!(limits.max_runs_retained, Some(10));
    assert_eq!(limits.max_events_per_run, Some(20));
    assert_eq!(limits.max_event_bytes_per_run, Some(30));
    assert_eq!(limits.max_trace_events, Some(40));
    assert_eq!(limits.max_terminal_subagent_tasks, Some(50));
}

#[test]
fn confirmation_policy_maps_yolo_lanes_to_rust_options() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        confirmation_policy: Some(ConfirmationPolicy {
            enabled: Some(true),
            default_timeout_ms: Some(5_000),
            timeout_action: Some("auto_approve".to_string()),
            yolo_lanes: Some(vec!["query".to_string(), "execute".to_string()]),
        }),
        ..Default::default()
    }))
    .unwrap();

    let policy = opts.confirmation_policy.unwrap();
    assert!(policy.enabled);
    assert_eq!(policy.default_timeout_ms, 5_000);
    assert!(matches!(
        policy.timeout_action,
        RustTimeoutAction::AutoApprove
    ));
    assert!(policy.yolo_lanes.contains(&RustSessionLane::Query));
    assert!(policy.yolo_lanes.contains(&RustSessionLane::Execute));
}

#[test]
fn worker_agent_spec_maps_to_rust_session_options() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        worker_agents: Some(vec![WorkerAgentSpec {
            name: "frontend-cow".to_string(),
            description: "Fix frontend bugs".to_string(),
            kind: Some("implementer".to_string()),
            model: Some("openai/gpt-4o".to_string()),
            max_steps: Some(8),
            ..Default::default()
        }]),
        ..Default::default()
    }))
    .unwrap();

    assert_eq!(opts.worker_agents.len(), 1);
    assert_eq!(opts.worker_agents[0].name, "frontend-cow");
    assert_eq!(opts.worker_agents[0].kind.as_str(), "implementer");
    assert_eq!(
        opts.worker_agents[0]
            .model
            .as_ref()
            .map(|model| model.model_ref()),
        Some("openai/gpt-4o".to_string())
    );
}

#[test]
fn local_workspace_backend_maps_to_rust_session_options() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: Some(JsWorkspaceBackend {
            kind: "local".to_string(),
            root: Some(".".to_string()),
            s3: None,
        }),
        ..Default::default()
    }))
    .unwrap();

    assert!(opts.workspace_services.is_some());
}

#[test]
fn workspace_backend_rejects_missing_local_root() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: Some(JsWorkspaceBackend {
            kind: "local".to_string(),
            root: None,
            s3: None,
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn s3_workspace_backend_maps_to_rust_session_options() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: Some(JsWorkspaceBackend {
            kind: "s3".to_string(),
            root: None,
            s3: Some(JsS3BackendConfig {
                endpoint: Some("https://minio.local:9000".to_string()),
                region: Some("us-east-1".to_string()),
                access_key_id: "AKIA".to_string(),
                secret_access_key: "secret".to_string(),
                session_token: None,
                bucket: "workspace".to_string(),
                prefix: "users/u1/sessions/s1".to_string(),
                force_path_style: Some(true),
                ..Default::default()
            }),
        }),
        ..Default::default()
    }))
    .unwrap();

    let services = opts.workspace_services.expect("s3 backend builds services");
    let caps = services.capabilities();
    assert!(caps.read);
    assert!(caps.write);
    assert!(!caps.exec, "S3 must not expose bash");
    assert!(!caps.git, "S3 must not expose git");
    assert!(!caps.search, "S3 must not expose grep/glob");
}

#[test]
fn workspace_backend_rejects_missing_s3_config() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: Some(JsWorkspaceBackend {
            kind: "s3".to_string(),
            root: None,
            s3: None,
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn s3_phase1_3_options_thread_through_to_core() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: Some(JsWorkspaceBackend {
            kind: "s3".to_string(),
            root: None,
            s3: Some(JsS3BackendConfig {
                access_key_id: "AKIA".to_string(),
                secret_access_key: "secret".to_string(),
                bucket: "workspace".to_string(),
                prefix: "u1/s1".to_string(),
                max_read_bytes: Some(4 * 1024 * 1024),
                search_enabled: Some(true),
                max_objects_scanned: Some(250),
                max_grep_bytes_per_object: Some(512 * 1024),
                ..Default::default()
            }),
        }),
        ..Default::default()
    }))
    .unwrap();

    let services = opts.workspace_services.expect("s3 backend builds services");
    assert!(
        services.capabilities().search,
        "searchEnabled=true must enable the search capability"
    );
    assert!(services.search().is_some());
}

#[test]
fn remote_git_attaches_on_top_of_s3_backend() {
    let opts = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: Some(JsWorkspaceBackend {
            kind: "s3".to_string(),
            root: None,
            s3: Some(JsS3BackendConfig {
                access_key_id: "AKIA".to_string(),
                secret_access_key: "secret".to_string(),
                bucket: "workspace".to_string(),
                prefix: "u1/s1".to_string(),
                ..Default::default()
            }),
        }),
        remote_git: Some(JsRemoteGitBackendConfig {
            base_url: "https://gitserver.internal".to_string(),
            repo_id: "u1/s1".to_string(),
            bearer_token: Some("tok".to_string()),
            request_timeout_ms: Some(10_000),
            ..Default::default()
        }),
        ..Default::default()
    }))
    .unwrap();

    let services = opts.workspace_services.expect("services built");
    assert!(
        services.git().is_some(),
        "remoteGit must register a git provider"
    );
    assert!(services.git_stash().is_some());
    // Worktree is intentionally not available — see RFC §8.
    assert!(services.git_worktree().is_none());
    assert!(services.capabilities().git);
}

#[test]
fn remote_git_without_workspace_backend_errors_clearly() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        workspace_backend: None,
        remote_git: Some(JsRemoteGitBackendConfig {
            base_url: "https://gitserver".to_string(),
            repo_id: "r".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    }));

    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("workspaceBackend"),
        "error message must mention the missing field, got: {}",
        err
    );
}

#[test]
fn confirmation_policy_rejects_invalid_yolo_lane() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        confirmation_policy: Some(ConfirmationPolicy {
            enabled: Some(true),
            yolo_lanes: Some(vec!["unknown".to_string()]),
            ..Default::default()
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn session_options_reject_invalid_permission_decision() {
    let result = js_session_options_to_rust(Some(SessionOptions {
        permission_policy: Some(PermissionPolicy {
            default_decision: Some("maybe".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn queue_config_rejects_invalid_lane_handler() {
    let mut lane_handlers = std::collections::HashMap::new();
    lane_handlers.insert(
        "unknown".to_string(),
        LaneHandlerConfig {
            mode: "external".to_string(),
            timeout_ms: None,
        },
    );

    let result = js_session_options_to_rust(Some(SessionOptions {
        queue_config: Some(SessionQueueConfig {
            lane_handlers: Some(lane_handlers),
            ..Default::default()
        }),
        ..Default::default()
    }));

    assert!(result.is_err());
}

#[test]
fn program_options_normalize_to_script_tool_contract() {
    let args = normalize_program_script_options(serde_json::json!({
        "source": "async function run(ctx, inputs) { return inputs; }",
        "inputs": { "needle": "auth" },
        "allowedTools": ["grep", "read"],
        "limits": { "maxToolCalls": 4 }
    }))
    .unwrap();

    assert_eq!(args["type"], "script");
    assert_eq!(args["language"], "javascript");
    assert_eq!(args["allowed_tools"], serde_json::json!(["grep", "read"]));
    assert_eq!(args["inputs"]["needle"], "auth");
}

#[test]
fn delegate_task_options_use_core_task_schema() {
    let args = delegated_tasks_options_to_args(vec![DelegateTaskOptions {
        agent: "explore".to_string(),
        description: "Find auth files".to_string(),
        prompt: "Inspect auth files".to_string(),
        background: Some(false),
        max_steps: Some(3),
    }]);

    assert_eq!(args["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(args["tasks"][0]["agent"], "explore");
    assert_eq!(args["tasks"][0]["description"], "Find auth files");
    assert_eq!(args["tasks"][0]["prompt"], "Inspect auth files");
    assert_eq!(args["tasks"][0]["background"], false);
    assert_eq!(args["tasks"][0]["max_steps"], 3);
    assert!(args["tasks"][0].get("role").is_none());
}

#[test]
fn delegated_tasks_options_use_unified_task_schema() {
    let args = delegated_tasks_options_to_args(vec![
        DelegateTaskOptions {
            agent: "explore".to_string(),
            description: "Find tests".to_string(),
            prompt: "Locate tests".to_string(),
            background: None,
            max_steps: None,
        },
        DelegateTaskOptions {
            agent: "verification".to_string(),
            description: "Check risks".to_string(),
            prompt: "Review risks".to_string(),
            background: None,
            max_steps: Some(2),
        },
    ]);

    assert_eq!(args["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(args["tasks"][0]["agent"], "explore");
    assert_eq!(args["tasks"][1]["agent"], "verification");
    assert_eq!(args["tasks"][1]["max_steps"], 2);
}

#[test]
fn mcp_config_object_accepts_nested_transport_and_timeout_ms() {
    let config = normalize_mcp_server_config(serde_json::json!({
        "name": "github",
        "transport": {
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"]
        },
        "env": { "GITHUB_TOKEN": "test" },
        "timeoutMs": 1500
    }))
    .unwrap();

    assert_eq!(config.name, "github");
    assert_eq!(config.tool_timeout_secs, 2);
    match config.transport {
        a3s_code_core::mcp::protocol::McpTransportConfig::Stdio { command, args } => {
            assert_eq!(command, "npx");
            assert_eq!(args, vec!["-y", "@modelcontextprotocol/server-github"]);
        }
        _ => panic!("expected stdio transport"),
    }
}

#[test]
fn mcp_config_object_accepts_streamable_http_alias() {
    let config = normalize_mcp_server_config(serde_json::json!({
        "name": "remote",
        "transport": {
            "type": "streamable_http",
            "url": "https://example.com/mcp",
            "headers": { "Authorization": "Bearer token" }
        }
    }))
    .unwrap();

    match config.transport {
        a3s_code_core::mcp::protocol::McpTransportConfig::StreamableHttp { url, headers } => {
            assert_eq!(url, "https://example.com/mcp");
            assert_eq!(
                headers.get("Authorization").map(String::as_str),
                Some("Bearer token")
            );
        }
        _ => panic!("expected streamable-http transport"),
    }
}
