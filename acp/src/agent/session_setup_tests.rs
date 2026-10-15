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
    assert!(slots
        .extra
        .expect("host context")
        .contains("Preserve these host instructions"));
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
