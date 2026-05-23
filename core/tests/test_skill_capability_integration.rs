use a3s_code_core::{Agent, CodeConfig, SessionOptions};
use serde_json::json;

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

#[tokio::test]
async fn session_registers_skill_tools_and_searches_session_skill_dirs() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let skill_dir = tempfile::tempdir().expect("skill tempdir");
    std::fs::write(
        skill_dir.path().join("db-review.md"),
        r#"---
name: db-review
description: Review database schema changes
allowed-tools:
  - Read
  - Grep
  - Bash(cargo test -p a3s-code-core:*)
kind: instruction
tags:
  - database
  - review
---
# DB Review

Check migrations, schema drift, and tests.
"#,
    )
    .expect("write test skill");

    let agent = Agent::from_config(test_config()).await.expect("agent");
    let session = agent
        .session(
            workspace.path().display().to_string(),
            Some(SessionOptions::new().with_skill_dirs([skill_dir.path()])),
        )
        .expect("session");

    let tool_names = session.tool_names();
    assert!(tool_names.contains(&"search_skills".to_string()));
    assert!(tool_names.contains(&"Skill".to_string()));

    let result = session
        .tool(
            "search_skills",
            json!({"query": "database schema", "limit": 5}),
        )
        .await
        .expect("search_skills result");

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("db-review"));
    let metadata = result.metadata.expect("search metadata");
    assert_eq!(metadata["skills"][0]["name"], "db-review");
    assert_eq!(
        metadata["skills"][0]["allowed_tools"],
        "Read, Grep, Bash(cargo test -p a3s-code-core:*)"
    );
}
