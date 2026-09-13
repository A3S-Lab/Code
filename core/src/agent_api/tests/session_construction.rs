use super::*;

#[tokio::test]
async fn test_local_session_installs_native_sandbox_by_default() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(SessionOptions::new().with_memory(Arc::new(a3s_memory::InMemoryStore::new()))),
        )
        .await
        .unwrap();

    assert!(session.tool_context.sandbox.is_some());

    #[cfg(not(windows))]
    let command = "printf core-default-sandbox";
    #[cfg(windows)]
    let command = "[Console]::Out.Write('core-default-sandbox')";
    let output = session.bash(command).await.unwrap();
    assert_eq!(output, "core-default-sandbox");
}

#[tokio::test]
async fn test_session_uses_workspace_backend_for_direct_tools() {
    let fs = Arc::new(TestWorkspaceFs::default());
    fs.insert("app.txt", "hello from backend\n");
    let fs_backend: Arc<dyn crate::workspace::WorkspaceFileSystem> = fs.clone();
    let runner = Arc::new(TestWorkspaceRunner::default());
    let runner_backend: Arc<dyn crate::workspace::WorkspaceCommandRunner> = runner.clone();
    let services = crate::workspace::WorkspaceServices::builder(
        crate::workspace::WorkspaceRef::new("session-workspace", "session://workspace"),
        fs_backend,
    )
    .command_runner(runner_backend)
    .build();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            "/server/session-construction-placeholder",
            Some(
                SessionOptions::new()
                    .with_workspace_backend(services)
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new())),
            ),
        )
        .await
        .unwrap();

    assert!(session.tool_context.sandbox.is_none());

    let tool_names = session.tool_names();
    assert!(tool_names.contains(&"read".to_string()));
    assert!(tool_names.contains(&"write".to_string()));
    assert!(tool_names.contains(&"ls".to_string()));
    assert!(tool_names.contains(&"bash".to_string()));
    assert!(!tool_names.contains(&"search".to_string()));
    assert!(!tool_names.contains(&"git".to_string()));

    let read = session.read_file("app.txt").await.unwrap();
    assert!(read.contains("hello from backend"));

    fs.insert("long.txt", "one\ntwo\nthree\nfour\n");
    let window = session
        .read_file_with_options(
            "long.txt",
            crate::ReadFileOptions {
                offset: Some(1),
                limit: Some(2),
            },
        )
        .await
        .unwrap();
    assert!(window.contains("two"));
    assert!(window.contains("three"));
    assert!(!window.contains("one"));
    assert!(!window.contains("four"));

    let write = session
        .write_file("created.txt", "one\ntwo\n")
        .await
        .unwrap();
    assert_eq!(write.exit_code, 0, "{}", write.output);
    assert_eq!(fs.read_raw("created.txt").as_deref(), Some("one\ntwo\n"));

    let listing = session.ls(None).await.unwrap();
    assert_eq!(listing.exit_code, 0, "{}", listing.output);
    assert!(listing.output.contains("created.txt"));

    let edit = session
        .edit_file("created.txt", "one", "uno", false)
        .await
        .unwrap();
    assert_eq!(edit.exit_code, 0, "{}", edit.output);
    assert_eq!(fs.read_raw("created.txt").as_deref(), Some("uno\ntwo\n"));

    let patch = session
        .patch_file("created.txt", "@@ -1,2 +1,2 @@\n uno\n-two\n+dos")
        .await
        .unwrap();
    assert_eq!(patch.exit_code, 0, "{}", patch.output);
    assert_eq!(fs.read_raw("created.txt").as_deref(), Some("uno\ndos\n"));

    let bash = session.bash("pwd").await.unwrap();
    assert_eq!(bash, "session runner: pwd\n");
    crate::external_observation::release_session(session.id());
}

#[tokio::test]
async fn test_session_routes_agents_md_through_context_provider() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("AGENTS.md"),
        "Always run focused tests before reporting completion.",
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), None)
        .await
        .unwrap();

    let agents_provider = session
        .config
        .context_providers
        .iter()
        .find(|provider| provider.name() == "agents_md")
        .expect("AGENTS.md provider should be registered");
    assert!(!session
        .config
        .prompt_slots
        .extra
        .as_deref()
        .unwrap_or_default()
        .contains("Instructions (personal + project AGENTS.md chain)"));

    let result = agents_provider
        .query(&crate::context::ContextQuery::new("complete the task"))
        .await
        .unwrap();

    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, "agents_md");
    assert!(result.items[0].is_required());
    assert!(result.items[0]
        .content
        .contains("Always run focused tests before reporting completion."));
    assert_eq!(result.items[0].relevance, 1.0);
}

#[tokio::test]
async fn test_session_initializes_without_legacy_agentic_tools() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let _session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_session_with_model_override() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_model("openai/gpt-4o");
    let session = agent.session_async("/tmp/test-workspace", Some(opts)).await;
    assert!(session.is_ok());
}

#[tokio::test]
async fn test_session_with_invalid_model_format() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_model("gpt-4o");
    let error = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration { field: "model", .. }
    ));
}

#[tokio::test]
async fn test_session_with_model_not_found() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_model("openai/nonexistent");
    let error = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration { field: "model", .. }
    ));
}

#[tokio::test]
async fn test_session_skill_dirs_preserve_agent_registry_validator() {
    use crate::skills::validator::DefaultSkillValidator;
    use crate::skills::SkillRegistry;

    let registry = Arc::new(SkillRegistry::new());
    registry.set_validator(Arc::new(DefaultSkillValidator::default()));

    let temp_dir = tempfile::tempdir().unwrap();
    let invalid_skill = temp_dir.path().join("invalid.md");
    std::fs::write(
        &invalid_skill,
        r#"---
name: BadName
description: "invalid skill name"
kind: instruction
---
# Invalid Skill
"#,
    )
    .unwrap();

    let opts = SessionOptions::new().with_skill_dirs([temp_dir.path()]);
    let effective_registry = build_effective_registry_for_test(Some(registry), &opts);
    assert!(effective_registry.get("BadName").is_none());
}

#[tokio::test]
async fn test_session_skill_registry_overrides_agent_registry_without_polluting_parent() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "shared-skill".to_string(),
        description: "agent level".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "agent content".to_string(),
        tags: vec![],
        version: None,
    }));

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "shared-skill".to_string(),
        description: "session level".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "session content".to_string(),
        tags: vec![],
        version: None,
    }));

    let opts = SessionOptions::new().with_skill_registry(session_registry);
    let effective_registry = build_effective_registry_for_test(Some(registry.clone()), &opts);

    assert_eq!(
        effective_registry.get("shared-skill").unwrap().content,
        "session content"
    );
    assert_eq!(
        registry.get("shared-skill").unwrap().content,
        "agent content"
    );
}

#[tokio::test]
async fn test_session_skill_dirs_override_session_registry_and_skip_invalid_entries() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "shared-skill".to_string(),
        description: "session registry".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "registry content".to_string(),
        tags: vec![],
        version: None,
    }));

    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("shared.md"),
        r#"---
name: shared-skill
description: "skill dir override"
kind: instruction
---
# Shared Skill
dir content
"#,
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("README.md"), "# not a skill").unwrap();

    let opts = SessionOptions::new()
        .with_skill_registry(session_registry)
        .with_skill_dirs([temp_dir.path()]);
    let effective_registry = build_effective_registry_for_test(None, &opts);

    assert_eq!(
        effective_registry.get("shared-skill").unwrap().description,
        "skill dir override"
    );
    assert!(effective_registry.get("README").is_none());
}

#[tokio::test]
async fn test_session_specific_skills_do_not_leak_across_sessions() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    let agent_registry = Arc::new(SkillRegistry::with_builtins());
    agent.config.skill_registry = Some(agent_registry);

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "session-only".to_string(),
        description: "only for first session".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "session one".to_string(),
        tags: vec![],
        version: None,
    }));

    let session_one = agent
        .session_async(
            "/tmp/test-workspace",
            Some(SessionOptions::new().with_skill_registry(session_registry)),
        )
        .await
        .unwrap();
    let session_two = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();

    assert!(session_one
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("session-only")
        .is_some());
    assert!(session_two
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("session-only")
        .is_none());
}

#[tokio::test]
async fn test_session_for_agent_applies_definition_and_keeps_skill_overrides_isolated() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};
    use crate::subagent::AgentDefinition;

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.config.skill_registry = Some(Arc::new(SkillRegistry::with_builtins()));

    let definition = AgentDefinition::new("reviewer", "Review code")
        .with_prompt("Agent definition prompt")
        .with_max_steps(7);

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "agent-session-skill".to_string(),
        description: "agent session only".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "agent session content".to_string(),
        tags: vec![],
        version: None,
    }));

    let session_one = agent
        .session_for_agent_async(
            "/tmp/test-workspace",
            &definition,
            Some(SessionOptions::new().with_skill_registry(session_registry)),
        )
        .await
        .unwrap();
    let session_two = agent
        .session_for_agent_async("/tmp/test-workspace", &definition, None)
        .await
        .unwrap();

    assert_eq!(session_one.config.max_tool_rounds, 7);
    let extra = session_one.config.prompt_slots.extra.as_deref().unwrap();
    assert!(extra.contains("Agent definition prompt"));
    assert!(!extra.contains("agent-session-skill"));
    assert!(session_one
        .config
        .context_providers
        .iter()
        .any(|provider| provider.name() == "skills_catalog"));
    assert!(session_one
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("agent-session-skill")
        .is_some());
    assert!(session_two
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("agent-session-skill")
        .is_none());
}

#[tokio::test]
async fn test_session_for_agent_preserves_existing_prompt_slots_when_injecting_definition_prompt() {
    use crate::prompts::SystemPromptSlots;
    use crate::subagent::AgentDefinition;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let definition = AgentDefinition::new("planner", "Plan work")
        .with_prompt("Definition extra prompt")
        .with_max_steps(3);

    let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
        style: None,
        role: Some("Custom role".to_string()),
        guidelines: None,
        response_style: None,
        output_language: None,
        extra: None,
    });

    let session = agent
        .session_for_agent_async("/tmp/test-workspace", &definition, Some(opts))
        .await
        .unwrap();

    assert_eq!(
        session.config.prompt_slots.role.as_deref(),
        Some("Custom role")
    );
    assert!(session
        .config
        .prompt_slots
        .extra
        .as_deref()
        .unwrap()
        .contains("Definition extra prompt"));
    assert_eq!(session.config.max_tool_rounds, 3);
}

#[tokio::test]
async fn test_new_with_acl_string() {
    let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
    let agent = Agent::new(acl).await;
    assert!(agent.is_ok());
}

#[tokio::test]
async fn test_create_alias_acl() {
    let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
    let agent = Agent::create(acl).await;
    assert!(agent.is_ok());
}

#[tokio::test]
async fn test_create_and_new_produce_same_result() {
    let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
    let agent_new = Agent::new(acl).await;
    let agent_create = Agent::create(acl).await;
    assert!(agent_new.is_ok());
    assert!(agent_create.is_ok());

    // Both should produce working sessions
    let session_new = agent_new
        .unwrap()
        .session_async("/tmp/test-ws-new", None)
        .await;
    let session_create = agent_create
        .unwrap()
        .session_async("/tmp/test-ws-create", None)
        .await;
    assert!(session_new.is_ok());
    assert!(session_create.is_ok());
}

#[tokio::test]
async fn test_new_with_existing_acl_file_uses_file_loading() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("agent.acl");
    std::fs::write(&config_path, "providers {").unwrap();

    let err = Agent::new(config_path.display().to_string())
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("Failed to load config"));
    assert!(msg.contains("agent.acl"));
    assert!(!msg.contains("Failed to parse config as ACL string"));
}

#[tokio::test]
async fn test_new_with_missing_acl_file_reports_not_found() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_path = temp_dir.path().join("agent.acl");

    let err = Agent::new(missing_path.display().to_string())
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("Config file not found"));
    assert!(msg.contains("agent.acl"));
    assert!(!msg.contains("Failed to parse config as ACL string"));
}

#[tokio::test]
async fn test_new_rejects_hcl_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("agent.hcl");
    std::fs::write(&config_path, "default_model = \"openai/test\"").unwrap();

    let err = Agent::new(config_path.display().to_string())
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("HCL config files are not supported in 2.0"));
    assert!(msg.contains(".acl"));
}

#[test]
fn test_from_config_defers_default_model_validation_to_session() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = CodeConfig {
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![],
        }],
        ..Default::default()
    };
    let agent = rt
        .block_on(Agent::from_config(config))
        .expect("agent bootstrap must allow a host-supplied session client");
    let workspace = tempfile::tempdir().unwrap();
    let error = rt
        .block_on(agent.session_async(workspace.path().display().to_string(), None))
        .unwrap_err();

    assert!(error.to_string().contains("default_model"), "{error:#}");
}
