use super::*;

#[test]
fn agent_run_spawn_dict_preserves_snapshot_and_replay_state() {
    pyo3::prepare_freethreaded_python();
    let spawn = RustAgentRunSpawn::Replayed {
        snapshot: a3s_code_core::run::RunSnapshot {
            id: "host-run-1".to_string(),
            session_id: "session-1".to_string(),
            status: a3s_code_core::run::RunStatus::Created,
            prompt: "inspect the workspace".to_string(),
            cognitive_package_binding: None,
            created_at_ms: 10,
            updated_at_ms: 20,
            result_text: None,
            error: None,
            event_count: 0,
            workspace_change_set: None,
        },
    };

    Python::with_gil(|py| {
        let object = agent_run_spawn_to_py(py, &spawn).unwrap();
        let dict = object.bind(py).downcast::<PyDict>().unwrap();
        assert!(dict
            .get_item("replayed")
            .unwrap()
            .unwrap()
            .extract::<bool>()
            .unwrap());
        let snapshot_object = dict.get_item("snapshot").unwrap().unwrap();
        let snapshot = snapshot_object.downcast::<PyDict>().unwrap();
        assert_eq!(
            snapshot
                .get_item("id")
                .unwrap()
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "host-run-1"
        );
        assert_eq!(
            snapshot
                .get_item("status")
                .unwrap()
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "created"
        );
    });
}

#[test]
fn inline_skill_conversion_is_typed_and_rejects_invalid_input() {
    pyo3::prepare_freethreaded_python();
    let skill = inline_skill_to_rust(
        "  live-review  ".to_string(),
        "Review the current change.".to_string(),
        "tool",
    )
    .unwrap();

    assert_eq!(skill.name, "live-review");
    assert_eq!(skill.kind, RustSkillKind::Tool);
    assert_eq!(skill.content, "Review the current change.");
    assert!(inline_skill_to_rust("live-review".to_string(), String::new(), "unknown",).is_err());
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
    pyo3::prepare_freethreaded_python();
    let session_id = "python-memory-store-roundtrip".to_string();
    let (create_options, resume_options) = Python::with_gil(|py| {
        let store = Py::new(py, PyMemorySessionStore::new()).unwrap();
        let mut create = PySessionOptions::new();
        create.session_id = Some(session_id.clone());
        create.session_store = Some(store.clone_ref(py).into_any());
        let mut resume = PySessionOptions::new();
        resume.session_store = Some(store.into_any());
        (
            build_rust_session_options(create).unwrap(),
            build_rust_session_options(resume).unwrap(),
        )
    });

    get_runtime().block_on(async {
        let agent = RustAgent::from_config(sdk_test_config()).await.unwrap();
        let session = agent
            .session_async(
                "/tmp/a3s-code-python-memory-session-store",
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

fn build_test_session() -> PySession {
    let agent = get_runtime()
        .block_on(RustAgent::from_config(sdk_test_config()))
        .unwrap();
    let session = get_runtime()
        .block_on(agent.session_async("/tmp/a3s-code-python-sdk-api", None))
        .unwrap();
    PySession {
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
fn python_hook_response_parser_rejects_ambiguous_actions() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let missing = PyDict::new(py);
        assert!(parse_py_hook_response(py, missing.as_any()).is_err());

        let unknown = PyDict::new(py);
        unknown.set_item("action", "permit").unwrap();
        assert!(parse_py_hook_response(py, unknown.as_any()).is_err());

        let explicit = PyDict::new(py);
        explicit.set_item("action", "continue").unwrap();
        let response = parse_py_hook_response(py, explicit.as_any()).unwrap();
        assert!(matches!(
            response.action,
            a3s_code_core::hooks::HookAction::Continue
        ));
        assert!(response.modified.is_none());
    });
}

#[test]
fn python_hook_response_parser_preserves_retry_reason() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let retry = PyDict::new(py);
        retry.set_item("action", "retry").unwrap();
        retry
            .set_item("reason", "policy backend is warming up")
            .unwrap();
        retry.set_item("delay_ms", 400_u64).unwrap();

        let response = parse_py_hook_response(py, retry.as_any()).unwrap();
        assert!(matches!(
            response.action,
            a3s_code_core::hooks::HookAction::Retry
        ));
        assert_eq!(
            response.reason.as_deref(),
            Some("policy backend is warming up")
        );
        assert_eq!(response.retry_delay_ms, Some(400));
    });
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
fn session_options_map_parallel_delegation_controls() {
    let mut session_options = PySessionOptions::new();
    session_options.max_parallel_tasks = Some(3);
    session_options.auto_delegation = Some(PyAutoDelegationConfig::new(true, true, 0.8, 2));
    session_options.auto_parallel = Some(false);
    session_options.manual_delegation_enabled = Some(false);

    let opts = build_rust_session_options(session_options).unwrap();
    assert_eq!(opts.max_parallel_tasks, Some(3));
    assert_eq!(opts.auto_parallel_delegation, Some(false));
    assert_eq!(opts.manual_delegation_enabled, Some(false));
    let auto = opts.auto_delegation.expect("auto delegation config");
    assert!(auto.enabled);
    assert!(!auto.auto_parallel);
    assert!((auto.min_confidence - 0.8).abs() < f32::EPSILON);
    assert_eq!(auto.max_tasks, 2);
}

#[test]
fn session_options_map_task_priority() {
    let mut session_options = PySessionOptions::new();
    session_options.task_priority = Some("background".to_string());
    let opts = build_rust_session_options(session_options).unwrap();
    assert_eq!(opts.task_priority, a3s_code_core::TaskPriority::Background);
}

#[test]
fn session_options_map_resilience_controls() {
    let mut session_options = PySessionOptions::new();
    session_options.tool_timeout_ms = Some(1_000);
    session_options.llm_api_timeout_ms = Some(2_000);
    session_options.circuit_breaker_threshold = Some(4);
    session_options.duplicate_tool_call_threshold = Some(5);

    let opts = build_rust_session_options(session_options).unwrap();
    assert_eq!(opts.tool_timeout_ms, Some(1_000));
    assert_eq!(opts.llm_api_timeout_ms, Some(2_000));
    assert_eq!(opts.circuit_breaker_threshold, Some(4));
    assert_eq!(opts.duplicate_tool_call_threshold, Some(5));
}

#[test]
fn session_options_preserve_serializable_permission_policy() {
    let mut session_options = PySessionOptions::new();
    session_options.permission_policy = Some(PyPermissionPolicy::new(
        Some(vec!["read".to_string()]),
        Some(vec!["bash".to_string()]),
        None,
        Some("ask".to_string()),
        true,
    ));

    let opts = build_rust_session_options(session_options).unwrap();
    let policy = opts
        .permission_policy
        .as_ref()
        .expect("Python policy must remain serializable for snapshots and child runs");
    assert_eq!(policy.allow.len(), 1);
    assert_eq!(policy.deny.len(), 1);
    assert!(opts.permission_checker.is_some());
}

#[test]
fn session_options_security_provider_is_typed_and_fail_closed() {
    pyo3::prepare_freethreaded_python();

    let valid = Python::with_gil(|py| {
        let provider = Py::new(py, PyDefaultSecurityProvider::new()).unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.security_provider = Some(provider.into_any());
        build_rust_session_options(session_options)
    })
    .unwrap();
    assert!(valid.security_provider.is_some());

    let invalid = Python::with_gil(|py| {
        let provider = PyDict::new(py);
        provider.set_item("kind", "unknown").unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.security_provider = Some(provider.into_any().unbind());
        build_rust_session_options(session_options)
    });
    let error = invalid.expect_err("unknown security providers must not be ignored");
    assert!(error.to_string().contains("DefaultSecurityProvider"));
}

#[test]
fn session_options_store_providers_are_typed_and_fail_closed() {
    pyo3::prepare_freethreaded_python();

    let valid = Python::with_gil(|py| {
        let memory = Py::new(py, PyFileMemoryStore::new("./memory".to_string())).unwrap();
        let sessions = Py::new(py, PyFileSessionStore::new("./sessions".to_string())).unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.memory_store = Some(memory.into_any());
        session_options.session_store = Some(sessions.into_any());
        build_rust_session_options(session_options)
    });
    assert!(valid.is_ok());

    let invalid_memory = Python::with_gil(|py| {
        let mut session_options = PySessionOptions::new();
        session_options.memory_store = Some(PyDict::new(py).into_any().unbind());
        build_rust_session_options(session_options)
    });
    let error = invalid_memory.expect_err("unknown memory stores must not be ignored");
    assert!(error.to_string().contains("FileMemoryStore"));

    let invalid_session = Python::with_gil(|py| {
        let mut session_options = PySessionOptions::new();
        session_options.session_store = Some(PyDict::new(py).into_any().unbind());
        build_rust_session_options(session_options)
    });
    let error = invalid_session.expect_err("unknown session stores must not be ignored");
    assert!(error.to_string().contains("SessionStore"));
}

#[test]
fn session_options_map_active_skill_tool_restriction_control() {
    let default_opts = build_rust_session_options(PySessionOptions::new()).unwrap();
    assert_eq!(default_opts.enforce_active_skill_tool_restrictions, None);

    let mut session_options = PySessionOptions::new();
    session_options.enforce_active_skill_tool_restrictions = Some(true);

    let opts = build_rust_session_options(session_options).unwrap();
    assert_eq!(opts.enforce_active_skill_tool_restrictions, Some(true));
}

#[test]
fn session_options_map_model_context_window() {
    let mut session_options = PySessionOptions::new();
    session_options.auto_compact = true;
    session_options.auto_compact_threshold = Some(0.75);
    session_options.max_context_tokens = Some(128_000);

    let opts = build_rust_session_options(session_options).unwrap();
    assert!(opts.auto_compact);
    assert_eq!(opts.auto_compact_threshold, Some(0.75));
    assert_eq!(opts.max_context_tokens, Some(128_000));
}

#[test]
fn session_options_reject_zero_model_context_window() {
    pyo3::prepare_freethreaded_python();
    let mut session_options = PySessionOptions::new();
    let error = session_options
        .set_max_context_tokens(Some(0))
        .expect_err("zero context window must fail");
    assert!(error.to_string().contains("positive integer"));
}

#[test]
fn session_options_map_rl_trajectory_controls() {
    let mut session_options = PySessionOptions::new();
    session_options.trajectory_path = Some("/tmp/a3s-trajectory.jsonl".to_string());
    session_options.trajectory_mode = Some("on".to_string());
    session_options.trajectory_max_text_bytes = Some(1234);
    session_options.trajectory_include_messages = Some(false);

    let opts = build_rust_session_options(session_options).unwrap();
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
fn session_options_map_llm_logprob_controls() {
    let mut session_options = PySessionOptions::new();
    session_options.llm_logprobs = Some(true);
    session_options.llm_top_logprobs = Some(1);

    let opts = build_rust_session_options(session_options).unwrap();
    assert_eq!(opts.llm_logprobs, Some(true));
    assert_eq!(opts.llm_top_logprobs, Some(1));
}

#[test]
fn session_options_map_deterministic_host_env() {
    let mut session_options = PySessionOptions::new();
    session_options.host_env = Some(PyHostEnvConfig {
        sequential_id_prefix: Some("replay".to_string()),
        fixed_time_ms: Some(1_700_000_000_000),
    });

    let opts = build_rust_session_options(session_options).unwrap();
    let host_env = opts.host_env.expect("host env");
    assert_eq!(host_env.next_id(), "replay-0");
    assert_eq!(host_env.next_id(), "replay-1");
    assert_eq!(host_env.now_ms(), 1_700_000_000_000);
}

#[test]
fn session_options_map_every_retention_limit() {
    pyo3::prepare_freethreaded_python();
    let retention = Python::with_gil(|py| {
        let dict = PyDict::new(py);
        dict.set_item("unbounded", false).unwrap();
        dict.set_item("max_runs_retained", 10).unwrap();
        dict.set_item("max_events_per_run", 20).unwrap();
        dict.set_item("max_event_bytes_per_run", 30).unwrap();
        dict.set_item("max_trace_events", 40).unwrap();
        dict.set_item("max_terminal_subagent_tasks", 50).unwrap();
        dict.into_any().unbind()
    });
    let mut session_options = PySessionOptions::new();
    session_options.retention_limits = Some(retention);

    let opts = build_rust_session_options(session_options).unwrap();
    let limits = opts.retention_limits.expect("retention limits");
    assert_eq!(limits.max_runs_retained, Some(10));
    assert_eq!(limits.max_events_per_run, Some(20));
    assert_eq!(limits.max_event_bytes_per_run, Some(30));
    assert_eq!(limits.max_trace_events, Some(40));
    assert_eq!(limits.max_terminal_subagent_tasks, Some(50));
}

#[test]
fn artifact_store_limits_map_to_rust_session_options() {
    let mut session_options = PySessionOptions::new();
    session_options.artifact_store_limits = Some(PyArtifactStoreLimits {
        max_artifacts: 3,
        max_bytes: 4096,
    });

    let opts = build_rust_session_options(session_options).unwrap();
    let limits = opts.artifact_store_limits.expect("limits");
    assert_eq!(limits.max_artifacts, 3);
    assert_eq!(limits.max_bytes, 4096);
}

#[test]
fn tool_result_transform_policy_maps_to_rust_session_options() {
    let policy = a3s_code_core::tools::ToolResultTransformPolicyV1::context_efficient();
    let mut session_options = PySessionOptions::new();
    session_options.tool_result_transform_policy = Some(policy.clone().into());

    let opts = build_rust_session_options(session_options).unwrap();
    assert_eq!(opts.tool_result_transform_policy, Some(policy));
}

#[test]
fn tool_presentation_profile_maps_to_rust_session_options() {
    for profile in [
        a3s_code_core::tools::ToolPresentationProfileV1::adaptive(),
        a3s_code_core::tools::ToolPresentationProfileV1::direct(),
        a3s_code_core::tools::ToolPresentationProfileV1::code(),
        a3s_code_core::tools::ToolPresentationProfileV1::disabled(),
    ] {
        let mut session_options = PySessionOptions::new();
        session_options.tool_presentation_profile = Some(profile.clone().into());

        let opts = build_rust_session_options(session_options).unwrap();
        assert_eq!(opts.tool_presentation_profile, Some(profile));
    }
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
fn py_session_records_verification_reports() {
    pyo3::prepare_freethreaded_python();
    let session = build_test_session();

    Python::with_gil(|py| {
        let json_mod = py.import("json").unwrap();
        let reports = json_mod
            .call_method1(
                "loads",
                (serde_json::json!([verification_report_json()]).to_string(),),
            )
            .unwrap();
        session.record_verification_reports(py, &reports).unwrap();
    });

    let reports = session.inner.verification_reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].subject, "sdk:test");
    assert!(matches!(
        session.inner.verification_summary().status,
        RustVerificationStatus::Passed
    ));
}

#[test]
fn py_session_get_artifact_returns_none_for_missing_uri() {
    pyo3::prepare_freethreaded_python();
    let session = build_test_session();

    Python::with_gil(|py| {
        let artifact = session
            .get_artifact(py, "a3s://tool-output/missing")
            .unwrap();
        assert!(artifact.bind(py).is_none());
    });
}

#[test]
fn local_workspace_backend_maps_to_rust_session_options() {
    pyo3::prepare_freethreaded_python();
    let opts = Python::with_gil(|py| {
        let backend = Py::new(
            py,
            PyLocalWorkspaceBackend {
                root: ".".to_string(),
            },
        )
        .unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.workspace_backend = Some(backend.into_any());
        build_rust_session_options(session_options)
    })
    .unwrap();

    assert!(opts.workspace_services.is_some());
}

#[test]
fn s3_workspace_backend_maps_to_rust_session_options() {
    pyo3::prepare_freethreaded_python();
    let opts = Python::with_gil(|py| {
        let backend = Py::new(
            py,
            PyS3WorkspaceBackend {
                bucket: "workspace".to_string(),
                prefix: "users/u1/sessions/s1".to_string(),
                access_key_id: "AKIA".to_string(),
                secret_access_key: "secret".to_string(),
                endpoint: Some("https://minio.local:9000".to_string()),
                region: Some("us-east-1".to_string()),
                session_token: None,
                force_path_style: true,
                max_read_bytes: None,
                search_enabled: false,
                max_objects_scanned: None,
                max_grep_bytes_per_object: None,
                search_concurrency: None,
            },
        )
        .unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.workspace_backend = Some(backend.into_any());
        build_rust_session_options(session_options)
    })
    .unwrap();

    let services = opts.workspace_services.expect("s3 backend builds services");
    let caps = services.capabilities();
    assert!(caps.read);
    assert!(caps.write);
    assert!(!caps.exec);
    assert!(!caps.git);
    assert!(!caps.search);
}

#[test]
fn s3_phase1_3_options_thread_through_to_core() {
    pyo3::prepare_freethreaded_python();
    let opts = Python::with_gil(|py| {
        let backend = Py::new(
            py,
            PyS3WorkspaceBackend {
                bucket: "workspace".to_string(),
                prefix: "u1/s1".to_string(),
                access_key_id: "AKIA".to_string(),
                secret_access_key: "secret".to_string(),
                endpoint: None,
                region: None,
                session_token: None,
                force_path_style: false,
                max_read_bytes: Some(4 * 1024 * 1024),
                search_enabled: true,
                max_objects_scanned: Some(250),
                max_grep_bytes_per_object: Some(512 * 1024),
                search_concurrency: None,
            },
        )
        .unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.workspace_backend = Some(backend.into_any());
        build_rust_session_options(session_options)
    })
    .unwrap();

    let services = opts.workspace_services.expect("services built");
    assert!(
        services.capabilities().search,
        "search_enabled=true must enable the search capability"
    );
    assert!(services.search().is_some());
}

#[test]
fn remote_git_attaches_on_top_of_s3_backend() {
    pyo3::prepare_freethreaded_python();
    let opts = Python::with_gil(|py| {
        let backend = Py::new(
            py,
            PyS3WorkspaceBackend {
                bucket: "workspace".to_string(),
                prefix: "u1/s1".to_string(),
                access_key_id: "AKIA".to_string(),
                secret_access_key: "secret".to_string(),
                endpoint: None,
                region: None,
                session_token: None,
                force_path_style: false,
                max_read_bytes: None,
                search_enabled: false,
                max_objects_scanned: None,
                max_grep_bytes_per_object: None,
                search_concurrency: None,
            },
        )
        .unwrap();
        let mut session_options = PySessionOptions::new();
        session_options.workspace_backend = Some(backend.into_any());
        session_options.remote_git = Some(PyRemoteGitBackendConfig {
            base_url: "https://gitserver.internal".to_string(),
            repo_id: "u1/s1".to_string(),
            bearer_token: Some("tok".to_string()),
            client_cert_pem: None,
            client_key_pem: None,
            request_timeout_ms: Some(10_000),
            max_diff_bytes: None,
            max_log_entries: None,
        });
        build_rust_session_options(session_options)
    })
    .unwrap();

    let services = opts.workspace_services.expect("services built");
    assert!(services.git().is_some());
    assert!(services.git_stash().is_some());
    // Worktree intentionally unavailable on remote-git workspaces (RFC §8).
    assert!(services.git_worktree().is_none());
    assert!(services.capabilities().git);
}

/// Phase 8 alignment: a typed `ToolErrorKind` from the Rust core
/// must arrive at the Python SDK as a JSON envelope on
/// `error_kind_json`, with the discriminator on `type`. We assert
/// both the raw string shape and the parsed serde_json round-trip
/// (Python's `error_kind` getter calls `json_string_to_py` on the
/// same string, so this test fully covers the contract without
/// needing a Python interpreter to run JSON.parse).
#[test]
fn py_tool_result_threads_error_kind_json() {
    let kind = a3s_code_core::ToolErrorKind::VersionConflict {
        path: "doc.md".to_string(),
        expected: "etag-1".to_string(),
        actual: Some("etag-2".to_string()),
    };
    let result = PyToolResult::from(a3s_code_core::ToolCallResult {
        name: "edit".to_string(),
        output: "conflict".to_string(),
        exit_code: 1,
        metadata: Some(serde_json::json!({ "attempt": 2 })),
        error_kind: Some(kind),
    });
    assert_eq!(result.metadata_json.as_deref(), Some("{\"attempt\":2}"));
    let json = result.error_kind_json.expect("typed failure is projected");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "version_conflict");
    assert_eq!(parsed["path"], "doc.md");
    assert_eq!(parsed["expected"], "etag-1");
    assert_eq!(parsed["actual"], "etag-2");
}

/// Successful tool calls and tool calls that fail without a typed
/// reason must leave `error_kind_json` as `None` so SDK callers can
/// rely on its presence as the sole "is this a typed failure?"
/// signal.
#[test]
fn py_tool_result_error_kind_json_is_none_when_no_kind() {
    let result = PyToolResult::from(a3s_code_core::ToolCallResult {
        name: "read".to_string(),
        output: "hello".to_string(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    });
    assert!(result.metadata_json.is_none());
    assert!(result.error_kind_json.is_none());
}

#[test]
fn remote_git_without_workspace_backend_errors_clearly() {
    pyo3::prepare_freethreaded_python();
    let result = Python::with_gil(|_py| {
        let mut session_options = PySessionOptions::new();
        session_options.remote_git = Some(PyRemoteGitBackendConfig {
            base_url: "https://gitserver".to_string(),
            repo_id: "r".to_string(),
            bearer_token: None,
            client_cert_pem: None,
            client_key_pem: None,
            request_timeout_ms: None,
            max_diff_bytes: None,
            max_log_entries: None,
        });
        build_rust_session_options(session_options)
    });

    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("workspace_backend"),
        "error must mention missing field, got: {}",
        msg
    );
}

#[test]
fn delegate_task_args_use_core_task_schema() {
    let item = delegate_task_args(
        "explore".to_string(),
        "Find auth files".to_string(),
        "Inspect auth files".to_string(),
        true,
        Some(3),
    );
    let args = delegated_tasks_args(serde_json::json!([item])).unwrap();

    assert_eq!(args["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(args["tasks"][0]["agent"], "explore");
    assert_eq!(args["tasks"][0]["description"], "Find auth files");
    assert_eq!(args["tasks"][0]["prompt"], "Inspect auth files");
    assert_eq!(args["tasks"][0]["background"], true);
    assert_eq!(args["tasks"][0]["max_steps"], 3);
    assert!(args["tasks"][0].get("role").is_none());
}

#[test]
fn delegated_tasks_args_use_unified_task_schema() {
    let args = delegated_tasks_args(serde_json::json!([
        { "agent": "explore", "description": "Find tests", "prompt": "Locate tests" },
        { "agent": "verification", "description": "Check risks", "prompt": "Review risks" }
    ]))
    .unwrap();

    assert_eq!(args["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(args["tasks"][0]["agent"], "explore");
    assert_eq!(args["tasks"][1]["agent"], "verification");
    assert!(delegated_tasks_args(serde_json::json!({ "agent": "explore" })).is_err());
}

#[test]
fn program_options_normalize_to_script_tool_contract() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let dict = PyDict::new(py);
        dict.set_item(
            "source",
            "async function run(ctx, inputs) { return inputs; }",
        )
        .unwrap();
        dict.set_item(
            "inputs",
            serde_json::json!({ "needle": "auth" }).to_string(),
        )
        .unwrap();
        dict.set_item("allowedTools", vec!["grep", "read"]).unwrap();

        let args = normalize_program_script_options(&dict).unwrap();
        assert_eq!(args["type"], "script");
        assert_eq!(args["language"], "javascript");
        assert_eq!(args["allowed_tools"], serde_json::json!(["grep", "read"]));
    });
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
        "timeout_ms": 1500
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

// ---- orchestration conversion + pipeline-stage bridge (#43) ----

#[test]
fn py_to_step_spec_parses_full_dict() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let dict = PyDict::new(py);
        dict.set_item("task_id", "t1").unwrap();
        dict.set_item("agent", "explore").unwrap();
        dict.set_item("description", "d").unwrap();
        dict.set_item("prompt", "p").unwrap();
        dict.set_item("max_steps", 5u32).unwrap();
        dict.set_item("parent_session_id", "parent").unwrap();
        let schema = PyDict::new(py);
        schema.set_item("type", "object").unwrap();
        dict.set_item("output_schema", &schema).unwrap();

        let spec = py_to_step_spec(py, dict.as_any()).unwrap();
        assert_eq!(spec.task_id, "t1");
        assert_eq!(spec.agent, "explore");
        assert_eq!(spec.prompt, "p");
        assert_eq!(spec.max_steps, Some(5));
        assert_eq!(spec.parent_session_id.as_deref(), Some("parent"));
        assert!(spec.output_schema.is_some());
    });
}

#[test]
fn py_to_step_spec_minimal_defaults_optionals() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let dict = PyDict::new(py);
        dict.set_item("task_id", "t1").unwrap();
        dict.set_item("agent", "explore").unwrap();
        dict.set_item("description", "d").unwrap();
        dict.set_item("prompt", "p").unwrap();
        let spec = py_to_step_spec(py, dict.as_any()).unwrap();
        assert_eq!(spec.max_steps, None);
        assert_eq!(spec.parent_session_id, None);
        assert_eq!(spec.output_schema, None);
    });
}

#[test]
fn py_to_step_spec_missing_required_field_errors() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let dict = PyDict::new(py);
        dict.set_item("task_id", "t1").unwrap();
        dict.set_item("agent", "explore").unwrap();
        dict.set_item("description", "d").unwrap();
        // No "prompt" — a required field with no serde default.
        let err = py_to_step_spec(py, dict.as_any()).unwrap_err();
        assert!(
            err.to_string().contains("AgentStepSpec") || err.to_string().contains("prompt"),
            "got: {err}"
        );
    });
}

#[test]
fn step_outcome_to_py_uses_snake_case_keys() {
    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let outcome = RustStepOutcome {
            task_id: "t1".into(),
            session_id: "task-run-t1".into(),
            agent: "explore".into(),
            output: "o".into(),
            success: true,
            structured: Some(serde_json::json!({ "k": 1 })),
            source_anchors: vec![a3s_code_core::orchestration::ToolSourceAnchor {
                tool: "read".into(),
                url_or_path: "docs/source.md".into(),
            }],
        };
        let obj = step_outcome_to_py(py, &outcome).unwrap();
        let bound = obj.bind(py);
        let dict = bound.downcast::<PyDict>().unwrap();
        // snake_case keys — the casing the pipeline `ctx['previous']` relies on.
        assert_eq!(
            dict.get_item("task_id")
                .unwrap()
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "t1"
        );
        assert_eq!(
            dict.get_item("session_id")
                .unwrap()
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "task-run-t1"
        );
        assert!(dict
            .get_item("success")
            .unwrap()
            .unwrap()
            .extract::<bool>()
            .unwrap());
        assert!(dict.get_item("structured").unwrap().is_some());
        let source_anchors_value = dict.get_item("source_anchors").unwrap().unwrap();
        let source_anchors = source_anchors_value
            .downcast::<pyo3::types::PyList>()
            .unwrap();
        assert_eq!(source_anchors.len(), 1);
    });
}

#[test]
fn python_pipeline_stage_none_raise_and_spec() {
    pyo3::prepare_freethreaded_python();
    let (none_cb, raise_cb, spec_cb) = Python::with_gil(|py| {
        let none_cb = py.eval(c"lambda ctx: None", None, None).unwrap().unbind();
        // A raising stage must fail closed (caught → None), not abort.
        let raise_cb = py.eval(c"lambda ctx: 1 / 0", None, None).unwrap().unbind();
        // Reads ctx['previous']['task_id'] (snake_case) and returns a spec.
        let spec_cb = py
            .eval(
                c"lambda ctx: {'task_id': 'ps', 'agent': 'review', 'description': 'd', 'prompt': 'prev=' + str(ctx['previous']['task_id'])}",
                None,
                None,
            )
            .unwrap()
            .unbind();
        (none_cb, raise_cb, spec_cb)
    });

    assert!(PythonPipelineStage { callback: none_cb }
        .invoke(None, &serde_json::json!({ "x": 1 }))
        .is_none());
    assert!(
        PythonPipelineStage { callback: raise_cb }
            .invoke(None, &serde_json::json!({ "x": 1 }))
            .is_none(),
        "a raising stage fails closed to None"
    );

    let prev = RustStepOutcome {
        task_id: "prior".into(),
        session_id: "s".into(),
        agent: "a".into(),
        output: "o".into(),
        success: true,
        structured: None,
        source_anchors: Vec::new(),
    };
    let spec = PythonPipelineStage { callback: spec_cb }
        .invoke(Some(&prev), &serde_json::json!({ "x": 1 }))
        .expect("spec returned");
    assert_eq!(spec.task_id, "ps");
    assert!(
        spec.prompt.contains("prior"),
        "ctx['previous']['task_id'] (snake_case) was readable: {}",
        spec.prompt
    );
}
