use super::*;

fn new_summary() -> Summary {
    Summary::new(
        &Info {
            id: acp::SessionId::new("test"),
            cwd: "/tmp".into(),
        },
        default_model_id(),
    )
    .unwrap()
}

fn inline_definition(name: &str) -> a3s_code_agent::AgentDefinition {
    a3s_code_agent::AgentDefinition::from_json(&serde_json::json!({
        "name": name,
        "description": "A custom profile",
    }))
    .unwrap()
}

#[test]
fn set_agent_replaces_selection() {
    let mut summary = new_summary();

    summary.set_agent(PersistedAgent::Inline(Box::new(inline_definition(
        "custom-profile",
    ))));
    assert!(summary.agent_profile().is_some());

    summary.set_agent(PersistedAgent::Named("grok-build-plan".to_string()));
    assert_eq!(Some("grok-build-plan"), summary.agent_name());
    assert!(summary.agent_profile().is_none());
}

#[test]
fn reasserting_current_agent_name_preserves_inline_profile() {
    let mut summary = new_summary();
    summary.set_agent(PersistedAgent::Inline(Box::new(inline_definition(
        "custom",
    ))));

    summary.set_agent(PersistedAgent::Named("custom".to_string()));

    assert!(summary.agent_profile().is_some());
}

#[test]
fn persisted_agent_from_definition_classifies_inline_vs_named() {
    let inline = PersistedAgent::from(&inline_definition("custom-profile"));
    assert!(matches!(&inline, PersistedAgent::Inline(def) if def.name == "custom-profile"));

    let named = PersistedAgent::from(&a3s_code_agent::AgentDefinition::grok_build_plan());
    assert!(matches!(named, PersistedAgent::Named(_)));
}

#[test]
fn inline_profile_survives_json_round_trip() {
    let def = a3s_code_agent::AgentDefinition::from_json(&serde_json::json!({
        "name": "custom-profile",
        "description": "A custom profile",
        "promptBody": "Custom system instructions.",
    }))
    .unwrap();
    let mut summary = new_summary();
    summary.set_agent(PersistedAgent::Inline(Box::new(def)));

    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("agent_name"));
    assert!(json.contains("agent_profile"));

    let restored: Summary = serde_json::from_str(&json).unwrap();
    let profile = restored.agent_profile().expect("inline profile restored");
    assert_eq!("custom-profile", profile.name);
    assert_eq!(
        Some("Custom system instructions."),
        profile.prompt_body.as_deref()
    );
}

#[test]
fn mismatched_or_bad_agent_profile_falls_back_to_named() {
    for profile in [
        serde_json::json!({ "unexpected": "shape" }),
        serde_json::json!({ "name": "B", "description": "different name" }),
    ] {
        let mut value = serde_json::to_value(new_summary()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("agent_name".to_string(), serde_json::json!("A"));
        object.insert("agent_profile".to_string(), profile);

        let restored: Summary = serde_json::from_value(value).expect("summary still loads");
        assert_eq!(Some("A"), restored.agent_name());
        assert!(restored.agent_profile().is_none());
    }
}

#[test]
fn orphan_agent_profile_without_name_loads_as_none() {
    let mut value = serde_json::to_value(new_summary()).unwrap();
    value.as_object_mut().unwrap().insert(
        "agent_profile".to_string(),
        serde_json::json!({ "name": "orphan", "description": "no matching name key" }),
    );

    let restored: Summary = serde_json::from_value(value).expect("summary still loads");
    assert_eq!(None, restored.agent_name());
    assert!(restored.agent_profile().is_none());
}

#[test]
fn summary_deserializes_without_agent_name_backward_compat() {
    let json = r#"{
            "info": { "id": "old-session", "cwd": "/tmp" },
            "session_summary": "",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "num_messages": 0,
            "num_chat_messages": 0,
            "current_model_id": "test-model"
        }"#;
    let summary: Summary = serde_json::from_str(json).unwrap();
    assert_eq!(None, summary.agent_name());
}

#[test]
fn serialize_omits_agent_name_when_unset_and_includes_it_when_set() {
    let json = serde_json::to_string(&new_summary()).unwrap();
    assert!(!json.contains("agent_name"));

    let mut summary = new_summary();
    summary.set_agent(PersistedAgent::Named("cursor".to_string()));
    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("agent_name"));
    assert!(json.contains("cursor"));
}

#[test]
fn named_agent_round_trips() {
    let mut summary = new_summary();
    summary.set_agent(PersistedAgent::Named("grok-build".to_string()));

    let json = serde_json::to_string(&summary).unwrap();
    let deserialized: Summary = serde_json::from_str(&json).unwrap();
    assert_eq!(Some("grok-build"), deserialized.agent_name());
}

#[test]
fn summary_with_agent_name_in_full_json() {
    let json = r#"{
            "info": { "id": "full-session", "cwd": "/tmp" },
            "session_summary": "test session",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "num_messages": 10,
            "num_chat_messages": 5,
            "current_model_id": "cursor-model",
            "agent_name": "cursor",
            "generated_title": "Fix editor mode",
            "head_branch": "main"
        }"#;
    let summary: Summary = serde_json::from_str(json).unwrap();
    assert_eq!(Some("cursor"), summary.agent_name());
}
