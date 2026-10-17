#![cfg(test)]

use super::{A3sCodeAgent, SessionSetup};
use a3s_code_core::{sandbox::SandboxNetworkMode, CodeConfig};

#[test]
fn typed_setup_preserves_host_safety_and_context_without_activation() {
    let root = tempfile::tempdir().expect("workspace");
    let skills = root.path().join("configured-skills");
    std::fs::create_dir(&skills).expect("skill directory");
    let store = root.path().join("must-not-create-store");
    let config = CodeConfig {
        sandbox_network: SandboxNetworkMode::Deny,
        sandbox_network_allow: vec!["https://host.example.test".into()],
        skill_dirs: vec!["configured-skills".into()],
        ..Default::default()
    };
    let setup = SessionSetup {
        workspace: root.path().to_owned(),
        model_id: "fixture/exact-model-id",
        effort: None,
        host: crate::host_context::StudioHost {
            guidelines: "Preserve these host instructions".into(),
            output_language: Some("zh-CN".into()),
            ..Default::default()
        },
        components: &[],
        mcp_servers: Vec::new(),
    };
    let options = A3sCodeAgent::host_session_options("exact-session-id", &setup, &config, &store)
        .expect("host options");
    assert_eq!(options.model.as_deref(), Some("fixture/exact-model-id"));
    assert_eq!(options.session_id.as_deref(), Some("exact-session-id"));
    assert_eq!(options.require_assistant_final_response, Some(true));
    assert!(options.code_intelligence);
    assert_eq!(options.sandbox_network, config.sandbox_network);
    assert_eq!(options.sandbox_network_allow, config.sandbox_network_allow);
    assert!(options.permission_checker.is_some());
    let confirmation = options
        .confirmation_policy
        .expect("interactive confirmation");
    assert!(confirmation.enabled);
    assert!(matches!(
        confirmation.timeout_action,
        a3s_code_core::hitl::TimeoutAction::Reject
    ));
    assert!(confirmation.yolo_lanes.is_empty());
    assert!(options.skill_dirs.contains(&skills));
    assert!(options
        .watch_skill_dirs
        .contains(&root.path().join(".a3s/skills")));
    assert!(!store.exists());
    let slots = options.prompt_slots.expect("prompt slots");
    assert_eq!(slots.output_language.as_deref(), Some("zh-CN"));
    let extra = slots.extra.expect("host context");
    assert!(extra.contains("Preserve these host instructions"));
    assert!(extra.contains("bm25"));
    assert!(extra.contains("code_symbols"));
    assert!(extra.contains("语义检索关闭"));
    assert!(options.hook_executor.is_some());
}

#[test]
fn host_session_status_names_live_search_and_closed_semantic() {
    let root = tempfile::tempdir().expect("workspace");
    let store = root.path().join("store");
    let setup = SessionSetup {
        workspace: root.path().to_owned(),
        model_id: "fixture/model",
        effort: None,
        host: Default::default(),
        components: &[],
        mcp_servers: Vec::new(),
    };
    let trust = root.path().join("hooks-trust.json");
    let options = A3sCodeAgent::core_session_options("host-session", setup.model_id, setup.effort);
    let mut slots = crate::host_context::merge_studio_host(
        options.prompt_slots.clone().unwrap_or_default(),
        &setup.host,
    );
    let options = A3sCodeAgent::host_session_options_with_hook_roots(
        options,
        &mut slots,
        &setup,
        &CodeConfig::default(),
        &store,
        None,
        Some(&trust),
        None,
    )
    .expect("host options");
    assert!(options.hook_executor.is_some());
    let extra = options
        .prompt_slots
        .expect("prompt slots")
        .extra
        .expect("capability status");
    assert!(extra.contains("bm25"));
    assert!(extra.contains("code_symbols"));
    assert!(extra.contains("语义检索关闭"));
    assert!(extra.contains("memory: 耐久集合"));
    assert!(extra.contains("hooks: 0"));
    assert!(extra.contains("mcp: 0"));
    assert!(extra.contains("pending hooks: 0"));
    assert!(!extra.contains("semantic, hybrid"));
}

#[tokio::test]
async fn trusted_session_hook_runs_and_a_pending_hook_does_not() {
    let root = tempfile::tempdir().expect("workspace");
    let hooks_dir = root.path().join(".a3s");
    std::fs::create_dir_all(&hooks_dir).expect("hook directory");
    std::fs::write(
        hooks_dir.join("hooks.json"),
        r#"{
            "hooks": {
                "SessionStart": [{
                    "hooks": [
                        {"type": "command", "command": "printf x > trusted-ran", "timeout": 15},
                        {"type": "command", "command": "printf x > pending-ran", "timeout": 15}
                    ]
                }]
            }
        }"#,
    )
    .expect("hooks.json");
    let trust = root.path().join("hooks-trust.json");
    let preview =
        a3s_code_core::hooks::CommandHookExecutor::discover(root.path(), None, trust.clone())
            .expect("discover");
    let listed = preview.status_value();
    let short_id = listed["hooks"]
        .as_array()
        .expect("hooks")
        .iter()
        .find(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| command.contains("trusted-ran"))
        })
        .expect("trusted hook")["shortId"]
        .as_str()
        .expect("short id");
    preview.manage(&format!("trust {short_id}")).expect("trust");

    let store = root.path().join("store");
    let setup = SessionSetup {
        workspace: root.path().to_owned(),
        model_id: "fixture/model",
        effort: None,
        host: Default::default(),
        components: &[],
        mcp_servers: Vec::new(),
    };
    let options = A3sCodeAgent::core_session_options("hook-session", setup.model_id, setup.effort);
    let mut slots = crate::host_context::merge_studio_host(
        options.prompt_slots.clone().unwrap_or_default(),
        &setup.host,
    );
    let options = A3sCodeAgent::host_session_options_with_hook_roots(
        options,
        &mut slots,
        &setup,
        &CodeConfig::default(),
        &store,
        None,
        Some(&trust),
        None,
    )
    .expect("host options");
    let extra = options
        .prompt_slots
        .as_ref()
        .and_then(|slots| slots.extra.as_deref())
        .expect("capability status");
    assert!(extra.contains("hooks: 2"));
    assert!(extra.contains("pending hooks: 1"));
    assert!(extra.contains("语义检索关闭"));

    let executor = options.hook_executor.expect("hook executor");
    executor
        .fire(&a3s_code_core::hooks::HookEvent::SessionStart(
            a3s_code_core::hooks::SessionStartEvent {
                session_id: "hook-session".to_string(),
                system_prompt: None,
                model_provider: "fixture".to_string(),
                model_name: "model".to_string(),
            },
        ))
        .await;
    assert!(root.path().join("trusted-ran").is_file());
    assert!(!root.path().join("pending-ran").exists());
}

#[test]
fn host_contract_setup_is_scoped_and_refuses_invalid_roots_before_activation() {
    use a3s_code_core::config::WorkspaceVerificationContract;
    use a3s_code_core::verification::VerificationContractV1;

    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let store = root.path().join("not-created");
    let mut config = CodeConfig {
        verification_contracts: vec![WorkspaceVerificationContract {
            workspace: root.path().to_owned(),
            contract: VerificationContractV1::new("host", "python3 gate.py", ["src/**"]).unwrap(),
        }],
        ..Default::default()
    };
    let mut setup = SessionSetup {
        workspace: root.path().to_owned(),
        model_id: "fixture/model",
        effort: None,
        host: Default::default(),
        components: &[],
        mcp_servers: Vec::new(),
    };
    let options = A3sCodeAgent::host_session_options("session", &setup, &config, &store).unwrap();
    assert_eq!(options.verification_contracts.len(), 1);
    assert_eq!(options.verification_contracts[0].command, "python3 gate.py");
    assert!(options.permission_checker.is_some());
    setup.workspace = other.path().to_owned();
    assert!(
        A3sCodeAgent::host_session_options("session", &setup, &config, &store)
            .unwrap()
            .verification_contracts
            .is_empty()
    );
    config.verification_contracts[0].workspace = root.path().join("missing");
    assert!(A3sCodeAgent::host_session_options("session", &setup, &config, &store).is_err());
    assert!(!store.exists());
}

#[tokio::test]
async fn source_review_has_no_tools_even_after_a_yolo_mode_change() {
    use agent_client_protocol::{self as acp, Agent as _};
    use serde_json::json;

    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("private.txt"),
        "not part of the supplied snapshot",
    )
    .unwrap();
    let mut launch = super::test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().into();
    let host = A3sCodeAgent::new(launch);
    let meta = json!({"a3sHost":{"sourceReview":true}})
        .as_object()
        .unwrap()
        .clone();
    let id = host
        .new_session(acp::NewSessionRequest::new(root.path()).meta(meta))
        .await
        .unwrap()
        .session_id;
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert!(!session.tool_definitions().is_empty());
    assert!(session
        .presented_tool_definitions("Review the supplied text.")
        .unwrap()
        .is_empty());
    host.set_session_mode(acp::SetSessionModeRequest::new(id.clone(), "yolo"))
        .await
        .unwrap();
    assert!(session
        .presented_tool_definitions("Read private.txt using bash or read.")
        .unwrap()
        .is_empty());
    let result = session
        .governed_tool("read", json!({"file_path":"private.txt"}))
        .await
        .unwrap();
    assert_ne!(result.exit_code, 0);
    assert!(result.output.contains("Permission denied"));
    assert!(!result.output.contains("not part of the supplied snapshot"));
    session.close().await;

    let ordinary = host
        .new_session(acp::NewSessionRequest::new(root.path()))
        .await
        .unwrap();
    let ordinary = host
        .sessions
        .lock()
        .await
        .get(ordinary.session_id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert!(!ordinary
        .presented_tool_definitions("Read private.txt.")
        .unwrap()
        .is_empty());
    ordinary.close().await;
}

#[tokio::test]
async fn durable_collection_stays_closed_until_the_first_recall() {
    let root = tempfile::tempdir().expect("workspace");
    let memory_dir = root.path().join(".a3s/memory");
    std::fs::create_dir_all(memory_dir.join("items")).expect("legacy items");
    std::fs::write(
        memory_dir.join("items/legacy-1.json"),
        r#"{
            "id": "legacy-1",
            "content": "legacy memory about e2e fixtures",
            "timestamp": "2026-01-01T00:00:00Z",
            "importance": 0.7,
            "tags": ["legacy"],
            "memory_type": "semantic",
            "metadata": {},
            "access_count": 0,
            "last_accessed": null
        }"#,
    )
    .expect("write legacy item");
    let store = root.path().join("session-store");
    let setup = SessionSetup {
        workspace: root.path().to_owned(),
        model_id: "fixture/model",
        effort: None,
        host: Default::default(),
        components: &[],
        mcp_servers: Vec::new(),
    };
    let config = CodeConfig {
        default_model: Some("fixture/model".to_string()),
        providers: vec![a3s_code_core::ProviderConfig {
            name: "fixture".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
            headers: Default::default(),
            session_id_header: None,
            models: vec![a3s_code_core::ModelConfig {
                id: "model".to_string(),
                name: "model".to_string(),
                family: "fixture".to_string(),
                api_key: None,
                base_url: None,
                headers: Default::default(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: Default::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    };
    let options = A3sCodeAgent::host_session_options("memory-session", &setup, &config, &store)
        .expect("host options");
    assert!(options.memory_store.is_some());
    assert!(!memory_dir.join("durable_memory").exists());
    let extra = options
        .prompt_slots
        .as_ref()
        .and_then(|slots| slots.extra.as_deref())
        .expect("capability status");
    assert!(extra.contains("memory: 耐久集合"));

    let agent = a3s_code_core::Agent::from_config(config)
        .await
        .expect("agent");
    let session = agent
        .session_async(root.path().to_string_lossy().to_string(), Some(options))
        .await
        .expect("session");
    assert!(!memory_dir.join("durable_memory").exists());
    assert!(memory_dir.join("items").is_dir());

    let found = session
        .memory()
        .expect("memory")
        .recall_similar("legacy e2e fixtures", 5)
        .await
        .expect("recall");
    assert_eq!(found.len(), 1);
    assert!(found[0].content.contains("legacy memory"));
    assert!(memory_dir.join("durable_memory").exists());
    assert!(memory_dir.join("items.imported").is_dir());
    assert!(!memory_dir.join("items").exists());

    let again = session
        .memory()
        .expect("memory")
        .recall_similar("legacy e2e fixtures", 5)
        .await
        .expect("second recall");
    assert_eq!(again.len(), 1);
    assert!(!memory_dir.join("items").exists());
    session.close().await;
}
