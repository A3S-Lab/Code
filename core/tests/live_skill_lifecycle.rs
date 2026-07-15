use a3s_code_core::skills::{Skill, SkillKind, SkillRegistry};
use a3s_code_core::{Agent, CodeConfig, CodeError, SessionOptions};
use serde_json::json;
use std::sync::Arc;

fn test_config() -> CodeConfig {
    CodeConfig::from_acl(
        r#"
            default_model = "openai/gpt-4o"

            providers "openai" {
              api_key = "sk-test"

              models "gpt-4o" {
                name = "GPT-4o"
              }
            }
        "#,
    )
    .expect("valid test config")
}

fn skill(name: &str, description: &str) -> Arc<Skill> {
    Arc::new(Skill {
        name: name.to_string(),
        description: description.to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: description.to_string(),
        tags: vec!["live".to_string()],
        version: None,
    })
}

#[tokio::test]
async fn live_skill_add_remove_is_visible_and_restores_the_session_shadow() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let base_registry = Arc::new(SkillRegistry::new());
    base_registry.register_unchecked(skill("report", "Base report guidance"));

    let agent = Agent::from_config(test_config()).await.expect("agent");
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(SessionOptions::new().with_skill_registry(base_registry)),
        )
        .await
        .expect("session");

    let base = session
        .tool("search_skills", json!({"query": "report"}))
        .await
        .expect("base search");
    assert!(base.output.contains("Base report guidance"));

    session
        .add_skill(skill("report", "Live report guidance"))
        .expect("add live skill");
    assert_eq!(session.skill_names(), vec!["report".to_string()]);
    let live = session
        .tool("search_skills", json!({"query": "report"}))
        .await
        .expect("live search");
    assert!(live.output.contains("Live report guidance"));
    assert!(!live.output.contains("Base report guidance"));

    session
        .add_skill(skill("report", "Upgraded live report guidance"))
        .expect("upgrade live skill");
    let upgraded = session
        .tool("search_skills", json!({"query": "report"}))
        .await
        .expect("upgraded search");
    assert!(upgraded.output.contains("Upgraded live report guidance"));
    assert!(!upgraded.output.contains("Base report guidance"));

    session.remove_skill("report").expect("remove live skill");
    let restored = session
        .tool("search_skills", json!({"query": "report"}))
        .await
        .expect("restored search");
    assert!(restored.output.contains("Base report guidance"));
    assert!(!restored.output.contains("Live report guidance"));
    assert!(!restored.output.contains("Upgraded live report guidance"));
}

#[tokio::test]
async fn closed_session_rejects_live_skill_mutation() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let agent = Agent::from_config(test_config()).await.expect("agent");
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .expect("session");
    session.close().await;

    let add_error = session
        .add_skill(skill("late", "Late skill"))
        .expect_err("closed session must reject add");
    assert!(matches!(add_error, CodeError::SessionClosed { .. }));

    let remove_error = session
        .remove_skill("late")
        .expect_err("closed session must reject remove");
    assert!(matches!(remove_error, CodeError::SessionClosed { .. }));
}
