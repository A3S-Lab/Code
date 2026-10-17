#![cfg(test)]

use super::*;
use std::collections::HashMap;

#[test]
fn cc_switch_picker_label_includes_other_profiles() {
    assert_eq!(
        model_picker_label("cc-switch", "glm-5.3"),
        "cc-switch · glm-5.3"
    );
    assert_eq!(
        model_picker_label("cc-switch-zhipu-glm-01", "Zhipu GLM 01 · glm-5.2"),
        "cc-switch · Zhipu GLM 01 · glm-5.2"
    );
    assert_eq!(
        model_picker_description("cc-switch-zhipu-glm-01"),
        "Imported from local CC Switch"
    );
    assert_eq!(
        model_picker_label("cursor", "Composer 2.5"),
        "cursor · Composer 2.5"
    );
    assert_eq!(
        model_picker_description("cursor"),
        "Logged-in Cursor account"
    );
}

#[test]
fn cursor_catalog_uses_the_logged_in_account_label() {
    let mut launch = test_launch(None);
    launch.config.providers.push(a3s_code_core::ProviderConfig {
        name: "cursor".to_string(),
        api_key: Some("not-logged".to_string()),
        base_url: Some("https://api2.cursor.sh".to_string()),
        headers: std::collections::HashMap::new(),
        session_id_header: None,
        models: vec![test_model("composer-2.5", "Composer 2.5", "cursor")],
    });
    let agent = A3sCodeAgent::new(launch);
    let info = agent
        .available_model_infos()
        .into_iter()
        .find(|info| info.model_id.0.as_ref() == "cursor/composer-2.5")
        .expect("cursor model");
    assert_eq!(info.name, "cursor · Composer 2.5");
    assert_eq!(
        info.description.as_deref(),
        Some("Logged-in Cursor account")
    );
}

#[tokio::test]
async fn unknown_model_id_is_rejected_before_session_lookup() {
    let agent = A3sCodeAgent::new(test_launch(None));
    let error = acp::Agent::set_session_model(
        &agent,
        acp::SetSessionModelRequest::new(
            acp::SessionId::new("missing-session"),
            acp::ModelId::new("cursor/not-a-listed-model"),
        ),
    )
    .await
    .expect_err("unknown model");
    let rendered = format!("{error} {error:?}");
    assert!(
        rendered.contains("unknown model id"),
        "model check must fail first: {rendered}"
    );
    assert!(
        !rendered.contains("unknown session"),
        "session lookup must not run first: {rendered}"
    );
}

#[test]
fn effort_tokens_keep_one_tool_ceiling() {
    let meta = |effort: &str| {
        let mut m = acp::Meta::new();
        m.insert(
            "reasoningEffort".into(),
            serde_json::Value::String(effort.into()),
        );
        m
    };
    assert_eq!(reasoning_effort_token(Some(&meta("none"))), Some("none"));
    assert_eq!(
        reasoning_effort_token(Some(&meta("medium"))),
        Some("medium")
    );
    let low = effort_limits("low").expect("low");
    let high = effort_limits("high").expect("high");
    let max = effort_limits("max").expect("max");
    assert!(low.max_tool_rounds < high.max_tool_rounds);
    assert!(high.max_tool_rounds < max.max_tool_rounds);
    assert!(low.thinking_budget < high.thinking_budget);
    assert!(high.thinking_budget < max.thinking_budget);
    assert_eq!(
        max.max_tool_rounds,
        a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
    );

    let claude = A3sCodeAgent::core_session_options("sid", "anthropic/claude-opus-4-6", Some(high));
    assert_eq!(claude.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(claude.thinking_budget, Some(high.thinking_budget));
    assert_eq!(claude.max_tool_rounds, Some(high.max_tool_rounds));
    let claude_budget = claude
        .prompt_slots
        .as_ref()
        .and_then(|slots| slots.guidelines.as_deref())
        .expect("native models still receive the budget line");
    assert!(claude_budget.contains(&format!("thinking_budget={}", high.thinking_budget)));
    assert!(claude_budget.contains(&format!("tool_rounds={}", high.max_tool_rounds)));
    assert!(claude.auto_compact);
    assert_eq!(
        claude.auto_compact_threshold,
        Some(a3s_code_core::store::DEFAULT_AUTO_COMPACT_THRESHOLD)
    );
    assert!(claude.max_context_tokens.is_none());

    let local = A3sCodeAgent::core_session_options("sid", "ollama/llama3", Some(high));
    let local_budget = local
        .prompt_slots
        .as_ref()
        .and_then(|slots| slots.guidelines.as_deref())
        .expect("host budget line");
    assert!(local_budget.contains(&format!("thinking_budget={}", high.thinking_budget)));
    assert!(local_budget.contains(effort_guideline("high").expect("high guideline")));
}

#[test]
fn budget_line_states_a_maximum() {
    let high = effort_limits("high").expect("high");
    assert_eq!(high.thinking_budget, 16_384);
    assert_eq!(high.max_tool_rounds, 128);
    let options = A3sCodeAgent::core_session_options("sid", "boyue/deepseek-v4-flash", Some(high));
    let text = options
        .prompt_slots
        .and_then(|slots| slots.guidelines)
        .expect("budget line");
    assert!(text.contains("These limits are a maximum, not a quota."));
    assert!(text.contains("thinking_budget=16384"));
    assert!(text.contains("tool_rounds=128"));
    assert!(text.contains("Stop when the user's outcome is done."));
    assert_eq!(
        effort_limits("max").expect("max").max_tool_rounds,
        a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
    );
}

#[test]
fn high_depth_is_about_the_user_outcome() {
    let high = effort_guideline("high").expect("high guideline");
    assert!(high.contains("Depth applies to the user's outcome"));
    assert!(high.contains("not to rediscovering the running product"));
    assert!(effort_guideline("medium").is_none());
    let low = effort_guideline("low").expect("low guideline");
    assert!(low.starts_with("[effort: low]"));
    assert!(!low.contains("rediscovering"));
}

#[test]
fn empty_registry_still_states_the_product_boundary() {
    let registry = a3s_code_core::skills::SkillRegistry::new();
    assert!(registry.to_system_prompt().is_empty());
    registry.register_unchecked(std::sync::Arc::new(a3s_code_core::skills::Skill {
        name: "live-report".to_string(),
        description: "Build a report".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: a3s_code_core::skills::SkillKind::Instruction,
        content: "Build a report".to_string(),
        tags: Vec::new(),
        version: None,
    }));
    let catalog = registry.to_system_prompt();
    assert!(catalog.contains("search_skills"));
    assert!(!catalog.contains("skills add"));
    let high = effort_limits("high").expect("high");
    let options = A3sCodeAgent::core_session_options("sid", "boyue/deepseek-v4-flash", Some(high));
    let text = options
        .prompt_slots
        .and_then(|slots| slots.guidelines)
        .expect("session boundary");
    assert!(text.contains("Do not read or explain the product implementation"));
    assert!(text.contains("a3s code skills add <git-url>"));
    assert!(text.contains("Finish the user's outcome in this turn."));
}

#[test]
fn studio_host_keeps_the_budget_line_in_the_session_options() {
    let high = effort_limits("high").expect("high");
    let host = crate::host_context::StudioHost {
        work_policy: None,
        role: String::new(),
        guidelines: "每次都带上".to_string(),
        review_subagent: true,
        output_language: Some("zh-CN".to_string()),
        assets: None,
    };
    let options = A3sCodeAgent::host_session_options(
        "sid",
        &SessionSetup {
            workspace: PathBuf::from("/tmp"),
            model_id: "anthropic/claude-opus-4-6",
            effort: Some(high),
            host,
            components: &[],
            mcp_servers: Vec::new(),
        },
        &a3s_code_core::CodeConfig {
            sandbox_network: a3s_code_core::sandbox::SandboxNetworkMode::Deny,
            ..Default::default()
        },
        std::path::Path::new("/tmp/a3s-session-store"),
    )
    .expect("host options");
    let slots = options.prompt_slots.expect("slots");
    let guidelines = slots.guidelines.expect("budget line");
    assert!(guidelines.contains(&format!("thinking_budget={}", high.thinking_budget)));
    assert!(!guidelines.contains("每次都带上"));
    let extra = slots.extra.expect("host extra");
    assert!(extra.contains("Host instructions:\n每次都带上"));
    assert!(extra.contains("代码审查：在独立子会话中审查本次改动"));
    assert_eq!(slots.output_language.as_deref(), Some("zh-CN"));
}

/// Studio's session path: the standing prompt names installed assets and
/// omits their bodies. Opening one item returns that item. A deny still
/// skips the read. The user sentence is not rewritten.
#[tokio::test]
async fn studio_local_assets_session_opens_only_the_selected_item() {
    const CONNECTOR_BODY: &str = "BODY-connector-9f3a-unique";
    const SKILL_BODY: &str = "BODY-skill-9f3a-unique";
    const WIKI_BODY: &str = "BODY-wiki-9f3a-unique";
    const SECRET: &str = "do-not-leak-connector-cmd-9f3a";
    let root = std::env::temp_dir().join(format!("a3s-acp-local-assets-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::remove_dir_all(&root);
    let capabilities = root.join("capabilities");
    let skills = capabilities.join("skills").join("brief-writer");
    let wiki = root.join("wiki");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::create_dir_all(&wiki).unwrap();
    // mcp.json is the server spec reconcile_at writes. The usage note comes
    // from sync_connector_notes, the same writer reconcile_at calls.
    std::fs::write(
        capabilities.join("mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "session-notes": {
                    "type": "stdio",
                    "command": SECRET,
                    "args": ["--secret", "do-not-leak-env-9f3a"],
                    "env": { "TOKEN": "do-not-leak-token-9f3a" }
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let mut usage_prefix = String::from("Installed session notes");
    while usage_prefix.chars().count() < 160 {
        usage_prefix.push_str(" notes");
    }
    let usage_prefix: String = usage_prefix.chars().take(160).collect();
    let usage = format!("{usage_prefix}\n{CONNECTOR_BODY}");
    crate::local_assets::sync_connector_notes(&capabilities, &[("session-notes", usage.as_str())])
        .unwrap();
    let note = std::fs::read_to_string(capabilities.join("session-notes.md")).unwrap();
    assert!(note.contains(CONNECTOR_BODY));
    assert!(!note.contains(SECRET));
    assert!(!note.contains("do-not-leak-token-9f3a"));
    let mcp = std::fs::read_to_string(capabilities.join("mcp.json")).unwrap();
    assert!(!mcp.contains("description"));
    assert!(mcp.contains(SECRET));
    std::fs::write(
        skills.join("SKILL.md"),
        format!(
            "---\nname: brief-writer\ndescription: \"Write a short brief\"\n---\n{SKILL_BODY}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        wiki.join("handbook.md"),
        format!(
            "---\ntitle: \"产品手册甲\"\ndescription: \"Saved handbook page\"\n---\n{WIKI_BODY}\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(wiki.join("local")).unwrap();
    std::fs::write(
        wiki.join("local").join("食谱.md"),
        "红烧排骨只写在个人笔记9f3a。\n",
    )
    .unwrap();

    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let payload = serde_json::json!({
        "cwd": root,
        "mcpServers": [],
        "_meta": {
            "a3sHost": {
                "assets": {
                    "connectors": capabilities,
                    "skills": capabilities.join("skills"),
                    "wiki": wiki
                }
            }
        }
    });
    let request: acp::NewSessionRequest =
        serde_json::from_value(payload).expect("session/new with asset roots decodes");
    let created = acp::Agent::new_session(&agent, request)
        .await
        .expect("new session admits the local asset catalog");
    let session_id = created.session_id.0.as_ref().to_string();
    let (session, host) = {
        let sessions = agent.sessions.lock().await;
        let live = sessions.get(&session_id).expect("live session");
        (Arc::clone(&live.session), live.host.clone())
    };
    let options = A3sCodeAgent::host_session_options(
        &session_id,
        &SessionSetup {
            workspace: root.clone(),
            model_id: "anthropic/claude-sonnet-4-20250514",
            effort: None,
            host: host.clone(),
            components: &[],
            mcp_servers: Vec::new(),
        },
        &a3s_code_core::CodeConfig {
            sandbox_network: a3s_code_core::sandbox::SandboxNetworkMode::Deny,
            ..Default::default()
        },
        &root.join("sessions"),
    )
    .expect("host options");
    let standing = options.prompt_slots.expect("slots").build();
    assert!(standing.contains("session-notes"));
    assert!(standing.contains("Installed session notes"));
    assert!(standing.contains("brief-writer"));
    assert!(standing.contains("Write a short brief"));
    assert!(standing.contains("search_wiki"));
    assert!(standing.contains("scope=agent"));
    assert!(!standing.contains("产品手册甲"));
    assert!(!standing.contains("Saved handbook page"));
    assert!(!standing.contains("红烧排骨只写在个人笔记9f3a"));
    assert!(standing.contains("open_local_asset"));
    assert!(!standing.contains(CONNECTOR_BODY));
    assert!(!standing.contains(SKILL_BODY));
    assert!(!standing.contains(WIKI_BODY));
    assert!(!standing.contains(SECRET));
    assert!(!standing.contains("do-not-leak-token-9f3a"));
    let user = "今天只问天气";
    assert_eq!(
        crate::host_context::apply_turn_directives(
            user,
            &crate::host_context::TurnContext::default()
        ),
        user
    );
    assert!(!user.contains(SKILL_BODY));
    assert!(!user.contains(WIKI_BODY));

    let names: Vec<String> = session
        .tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert!(names.iter().any(|name| name == "open_local_asset"));
    let connector = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "connector", "name": "session-notes" }),
        )
        .await
        .expect("connector open");
    let skill = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "skill", "name": "brief-writer" }),
        )
        .await
        .expect("skill open");
    let page = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "wiki", "name": "产品手册甲" }),
        )
        .await
        .expect("wiki open");
    assert!(connector.output.contains(CONNECTOR_BODY));
    assert!(!connector.output.contains(SKILL_BODY));
    assert!(!connector.output.contains(WIKI_BODY));
    assert!(!connector.output.contains(SECRET));
    assert!(skill.output.contains(SKILL_BODY));
    assert!(!skill.output.contains(CONNECTOR_BODY));
    assert!(!skill.output.contains(WIKI_BODY));
    assert!(page.output.contains(WIKI_BODY));
    assert!(!page.output.contains(SKILL_BODY));
    assert!(!page.output.contains(CONNECTOR_BODY));
    let mut nested = None;
    for _ in 0..80 {
        let hit = session
            .tool("search_wiki", serde_json::json!({ "query": "红烧排骨" }))
            .await
            .expect("wiki search");
        if hit.output.contains("红烧排骨只写在个人笔记9f3a") {
            nested = Some(hit);
            break;
        }
        assert!(
            hit.output == "not-ready" || hit.output == "no-match",
            "unpublished wiki text leaked: {}",
            hit.output
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let nested = nested.expect("nested personal note was not searchable");
    assert!(nested.output.contains("local/食谱.md"));
    assert!(!nested.output.contains(WIKI_BODY));
    assert!(!nested.output.contains(SECRET));

    {
        let mut sessions = agent.sessions.lock().await;
        let live = sessions.get_mut(&session_id).expect("live session");
        live.denies.push("Skill".to_string());
        live.denies.push("mcp__session-notes".to_string());
        remember_denies(live);
    }
    let denied_skill = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "skill", "name": "brief-writer" }),
        )
        .await
        .expect("denied skill still returns a tool result");
    let denied_connector = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "connector", "name": "session-notes" }),
        )
        .await
        .expect("denied connector still returns a tool result");
    assert_eq!(denied_skill.output, "denied");
    assert_eq!(denied_connector.output, "denied");
    assert!(!denied_skill.output.contains(SKILL_BODY));
    assert!(!denied_connector.output.contains(CONNECTOR_BODY));
    let wiki_still = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "wiki", "name": "产品手册甲" }),
        )
        .await
        .expect("wiki stays readable");
    assert!(wiki_still.output.contains(WIKI_BODY));
    {
        let mut sessions = agent.sessions.lock().await;
        let live = sessions.get_mut(&session_id).expect("live session");
        live.denies.push("open_local_asset".to_string());
        remember_denies(live);
    }
    let denied_wiki = session
        .tool(
            "open_local_asset",
            serde_json::json!({ "kind": "wiki", "name": "产品手册甲" }),
        )
        .await
        .expect("tool deny still returns a result");
    assert_eq!(denied_wiki.output, "denied");
    assert!(!denied_wiki.output.contains(WIKI_BODY));
    eprintln!(
        "local-asset deny skill={} connector={} wiki={}",
        denied_skill.output, denied_connector.output, denied_wiki.output
    );

    drop(created);
    drop(session);
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
}

/// A handwritten note under `wiki/local` is what Studio admits. `search_wiki`
/// returns it only after the index publishes, and a markdown file outside
/// `wiki/` stays out of the hits.
#[tokio::test]
async fn local_note_is_searchable() {
    const SENTENCE: &str = "桂花糖藕只写在本地笔记7c2e";
    const OUTSIDE: &str = "工作区根上的句子不要被检索到9d1f";
    let root = std::env::temp_dir().join(format!("a3s-acp-local-note-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::remove_dir_all(&root);
    let wiki = root.join("wiki");
    let local = wiki.join("local");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("家常.md"), format!("# 家常\n\n{SENTENCE}\n")).unwrap();
    std::fs::write(root.join("readme.md"), format!("{OUTSIDE}\n")).unwrap();

    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let payload = serde_json::json!({
        "cwd": root,
        "mcpServers": [],
        "_meta": {
            "a3sHost": {
                "assets": { "wiki": wiki }
            }
        }
    });
    let request: acp::NewSessionRequest =
        serde_json::from_value(payload).expect("session/new with the workspace wiki decodes");
    let created = acp::Agent::new_session(&agent, request)
        .await
        .expect("new session admits the workspace wiki");
    let session_id = created.session_id.0.as_ref().to_string();
    let session = {
        let sessions = agent.sessions.lock().await;
        let live = sessions.get(&session_id).expect("live session");
        Arc::clone(&live.session)
    };

    let mut found = None;
    for _ in 0..80 {
        let hit = session
            .tool("search_wiki", serde_json::json!({ "query": "桂花糖藕" }))
            .await
            .expect("wiki search");
        if hit.output.contains(SENTENCE) {
            found = Some(hit);
            break;
        }
        assert!(
            hit.output == "not-ready" || hit.output == "no-match",
            "unpublished wiki text leaked: {}",
            hit.output
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let found = found.expect("local note was not searchable");
    assert!(found.output.contains("local/家常.md"));
    assert!(!found.output.contains(OUTSIDE));

    drop(created);
    drop(session);
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reasoning_meta_offers_parseable_budget_levels() {
    let meta = reasoning_effort_catalog_meta("anthropic/claude-opus-4-6");
    assert_eq!(
        meta.get("supportsReasoningEffort"),
        Some(&serde_json::Value::Bool(true))
    );
    let efforts = meta
        .get("reasoningEfforts")
        .and_then(|v| v.as_array())
        .expect("reasoningEfforts");
    let values: Vec<&str> = efforts
        .iter()
        .filter_map(|entry| entry.get("value").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(values, ["low", "medium", "high", "xhigh", "max"]);
    let high = efforts
        .iter()
        .find(|entry| entry.get("value").and_then(|v| v.as_str()) == Some("high"))
        .expect("high");
    assert_eq!(high.get("default"), Some(&serde_json::Value::Bool(true)));
    assert!(effort_guideline("high").is_some());
    assert!(effort_guideline("medium").is_none());
    assert_eq!(effort_limits("high").expect("high").token, "high");
}

#[test]
fn effort_limits_share_the_tool_ceiling() {
    let medium = effort_limits("medium").expect("medium");
    assert!(medium.max_tool_rounds < a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING);
    assert_eq!(medium.thinking_budget, 8192);
    assert_eq!(medium.max_tool_rounds, 48);
    assert_eq!(medium.max_parallel_tasks, HOST_PARALLEL_TASKS);
    assert_eq!(medium.max_continuation_turns, HOST_CONTINUATION_TURNS);

    let low = effort_limits("low").expect("low");
    let max = effort_limits("max").expect("max");
    let ultra = effort_limits("ultracode").expect("ultracode");
    assert_eq!(low.max_parallel_tasks, max.max_parallel_tasks);
    assert_eq!(low.max_continuation_turns, max.max_continuation_turns);
    assert_eq!(ultra.max_parallel_tasks, max.max_parallel_tasks);
    assert_eq!(ultra.max_continuation_turns, max.max_continuation_turns);
    assert_eq!(
        max.max_tool_rounds,
        a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
    );
    assert_eq!(ultra.max_tool_rounds, max.max_tool_rounds);
    assert_eq!(ultra.thinking_budget, max.thinking_budget);
    assert!(medium.max_tool_rounds < max.max_tool_rounds);
    let ultra_options =
        A3sCodeAgent::core_session_options("sid", "anthropic/claude-opus-4-6", Some(ultra));
    assert_eq!(
        ultra_options.planning_mode,
        a3s_code_core::PlanningMode::Auto
    );
    assert!(effort_limits("not-a-level").is_none());
}

fn effort_values(meta: &serde_json::Map<String, serde_json::Value>) -> Vec<&str> {
    meta.get("reasoningEfforts")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("value").and_then(|value| value.as_str()))
        .collect()
}

#[test]
fn reasoning_menu_follows_the_model() {
    let claude = reasoning_effort_catalog_meta("anthropic/claude-opus-4-6");
    assert_eq!(
        claude.get("supportsReasoningEffort"),
        Some(&serde_json::Value::Bool(true))
    );
    assert!(effort_values(&claude).contains(&"xhigh"));

    let glm = reasoning_effort_catalog_meta("zhipu/glm-5");
    let glm_values = effort_values(&glm);
    assert!(!glm_values.contains(&"medium"));
    assert!(!glm_values.contains(&"xhigh"));
    assert_eq!(glm_values, vec!["low", "high", "max"]);

    let local = reasoning_effort_catalog_meta("ollama/llama3");
    assert_eq!(
        local.get("supportsReasoningEffort"),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        effort_values(&local),
        vec!["low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn subagent_progress_reports_the_resolved_window() {
    let (window, early) = context_usage(128_000, 64_000);
    let (_, later) = context_usage(128_000, 96_000);
    assert_eq!(window, 128_000);
    assert_eq!(early, 50);
    assert!(later > early);
    let idle = serde_json::json!({});
    assert_eq!(progress_tokens_used(&idle), 0);
    let growing = serde_json::json!({"prompt_tokens": 96_000});
    let (reported, pct) = context_usage(128_000, progress_tokens_used(&growing));
    assert_eq!(reported, 128_000);
    assert_eq!(pct, later);

    let mut progress = ChildProgress::default();
    note_child_progress(
        &mut progress,
        "tool_completed",
        &serde_json::json!({"tool": "read", "exit_code": 0}),
    );
    note_child_progress(
        &mut progress,
        "tool_completed",
        &serde_json::json!({"tool": "bash", "exit_code": 1}),
    );
    note_child_progress(
        &mut progress,
        "turn_completed",
        &serde_json::json!({"turn": 2}),
    );
    assert_eq!(progress.tool_call_count, 2);
    assert_eq!(
        progress.tools_used,
        vec!["read".to_string(), "bash".to_string()]
    );
    assert_eq!(progress.error_count, 1);
    assert_eq!(progress.turn_count, 2);
    note_child_progress(
        &mut progress,
        "tool_completed",
        &serde_json::json!({"tool": "read", "exit_code": 0, "prompt_tokens": 4_000}),
    );
    assert_eq!(progress.tokens_used, 4_000);
    let finished = subagent_finished_update("task-1", "child-1", true, "ok", 500, &progress);
    assert_eq!(finished["tool_calls"], 3);
    assert_eq!(finished["turns"], 2);
    assert_eq!(finished["tokens_used"], 4_000);
    assert_eq!(finished["status"], "completed");
    let parsed =
        parse_interject_params(r#"{"sessionId":"s1","text":" look here ","interjectionId":"i1"}"#)
            .expect("interject");
    assert_eq!(parsed.session_id, "s1");
    assert_eq!(parsed.text, "look here");
    assert!(parse_interject_params(r#"{"sessionId":"s1"}"#).is_err());
    assert_eq!(progress_duration_ms(1_000, 1_500), 500);
    assert_eq!(progress_duration_ms(0, 1_500), 0);
}

#[test]
fn turn_outcome_payload_names_the_wire_shape() {
    let summary = a3s_code_core::verification::VerificationSummary::from_reports(&[]);
    let narrative = turn_outcome_payload(
        &a3s_code_core::harness_loop::CompletionTerminal::Narrative,
        &summary,
        None,
    );
    assert_eq!(narrative["sessionUpdate"], "turn_outcome");
    assert_eq!(narrative["terminal"], "narrative");
    assert_eq!(narrative["status"], "skipped");
    let verified = turn_outcome_payload(
        &a3s_code_core::harness_loop::CompletionTerminal::Verified {
            effect_digest: "d".into(),
        },
        &summary,
        None,
    );
    assert_eq!(verified["terminal"], "verified");
}

/// A raw client that reads the ask off the wire and then never answers —
/// the parked-permission hang shape. Both deadline tests ride the same
/// duplex; the second ask stays unanswered because the client stops
/// reading after the first line.
async fn never_answering_client(mut client_end: tokio::io::DuplexStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt as _};
    let mut buf = [0_u8; 1024];
    // Read enough to consume the ask; never write a reply.
    let _ = client_end.read(&mut buf).await;
    std::future::pending::<()>().await;
    let _ = client_end.shutdown().await;
}

/// A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS is process-global; serialise.
/// The env must hold for the WHOLE await: `ask_deadline()` reads it when
/// the ask future is polled, so a lazy future awaited after a restore
/// would silently see the default again (this test hung 30 minutes on
/// exactly that before the await moved inside).
#[allow(clippy::await_holding_lock)]
async fn ask_env_locked<F: std::future::Future>(test: impl FnOnce() -> F) -> F::Output {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let prior = std::env::var_os("A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS");
    std::env::set_var("A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS", "1");
    let outcome = test().await;
    match prior {
        Some(value) => std::env::set_var("A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS", value),
        None => std::env::remove_var("A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS"),
    }
    outcome
}

/// One agent whose client connection is answered by nobody: both ask kinds
/// must fail closed at the (env-shortened) deadline instead of parking.
#[tokio::test]
async fn unanswered_asks_fail_closed_at_the_deadline() {
    ask_env_locked(|| async {
        let root = tempfile::tempdir().unwrap();
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let handler_agent = A3sCodeAgent::new(tests::test_launch(None));
                let (client_end, agent_end) = tokio::io::duplex(64 * 1024);
                use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
                let (agent_read, agent_write) = tokio::io::split(agent_end);
                let (conn, io) = acp::AgentSideConnection::new(
                    handler_agent,
                    agent_write.compat_write(),
                    agent_read.compat(),
                    |fut| {
                        tokio::task::spawn_local(fut);
                    },
                );
                tokio::task::spawn_local(io);
                tokio::task::spawn_local(never_answering_client(client_end));
                let agent = A3sCodeAgent::new(tests::test_launch(None));
                agent.set_client(Rc::new(conn));

                let session_id = acp::SessionId::new("s1");
                let approved = agent
                    .request_tool_permission(
                        &session_id,
                        "tool-1",
                        "bash",
                        &serde_json::json!({"command": "ls"}),
                        root.path(),
                    )
                    .await;
                assert!(
                    !approved,
                    "an unanswered permission ask must deny, not park the turn forever"
                );

                let answer = agent
                    .request_question_answer(
                        &session_id,
                        "q-timeout",
                        "Pick one",
                        &["left".to_string(), "right".to_string()],
                        false,
                    )
                    .await;
                assert!(answer.is_none(), "an unanswered question yields no answer");
            })
            .await;
    })
    .await;
}

#[test]
fn question_options_are_not_permission_grants() {
    let options = question_permission_options(&[" left ".into(), "  ".into(), "right".into()]);
    assert_eq!(options.len(), 2);
    assert!(options
        .iter()
        .all(|option| option.kind == acp::PermissionOptionKind::RejectOnce));
    assert!(options
        .iter()
        .all(|option| option.kind != acp::PermissionOptionKind::AllowOnce));
    assert_eq!(options[0].name, "left");
    assert_eq!(options[1].name, "right");
}

/// A dead turn worker must surface as a FAILED turn, not a clean one: the
/// join failure names the panic payload (or the abort) so the user sees why.
#[tokio::test]
async fn a_dead_turn_worker_names_its_cause() {
    let panicking = tokio::spawn(async {
        panic!("spine exploded");
    });
    let join_error = panicking.await.expect_err("the panicking task must fail");
    let message = worker_join_failure(join_error);
    assert!(
        message.contains("turn worker died") && message.contains("spine exploded"),
        "panic payload must be named: {message}"
    );

    let aborted = tokio::spawn(async {
        std::future::pending::<()>().await;
    });
    aborted.abort();
    let join_error = aborted.await.expect_err("the aborted task must fail");
    let message = worker_join_failure(join_error);
    assert!(
        message.contains("turn worker died") && message.contains("worker aborted"),
        "abort must be named: {message}"
    );
}

#[test]
fn choice_questions_use_the_dialog_not_a_transcript_line() {
    assert!(question_has_choices(&["keep".into(), "  ".into()]));
    assert!(!question_has_choices(&["  ".into(), String::new()]));
}

#[test]
fn a_core_error_is_not_a_completed_turn() {
    let err = agent_failure_error("completion gate: workspace mutation");
    let detail = err
        .data
        .as_ref()
        .and_then(|value| value.as_str())
        .unwrap_or("");
    assert!(
        detail.contains("completion gate:"),
        "pager TurnFailed reads error data, got {detail:?}"
    );
}

#[test]
fn end_text_fills_a_turn_that_had_no_deltas() {
    assert!(should_emit_end_text(false, "done"));
    assert!(!should_emit_end_text(true, "done"));
    assert!(!should_emit_end_text(false, ""));
}

#[test]
fn selected_question_option_is_the_answer_text() {
    let options = vec!["left".to_string(), "right".to_string()];
    let selected = acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
        acp::PermissionOptionId::new("question-1"),
    ));
    assert_eq!(
        answer_from_permission_outcome(&selected, &options).as_deref(),
        Some("right")
    );
    assert_eq!(
        answer_from_permission_outcome(&acp::RequestPermissionOutcome::Cancelled, &options),
        None
    );
    let grant = acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
        acp::PermissionOptionId::new("allow-once"),
    ));
    assert_eq!(answer_from_permission_outcome(&grant, &options), None);
}

#[test]
fn a_question_id_is_prompted_once() {
    let mut seen = std::collections::HashSet::new();
    assert!(claim_question_prompt(&mut seen, "q1"));
    assert!(!claim_question_prompt(&mut seen, "q1"));
    assert!(claim_question_prompt(&mut seen, "q2"));
}

#[test]
fn question_permission_requests_carry_the_question_meta() {
    let session_id = acp::SessionId::new("s");
    for allow_free_text in [true, false] {
        let request = question_permission_request(
            &session_id,
            "q",
            "which?",
            &["left".into(), "right".into()],
            allow_free_text,
        )
        .expect("choices present");
        let marker = request
            .meta
            .as_ref()
            .and_then(|meta| meta.get(QUESTION_META_KEY))
            .expect("question marker");
        assert_eq!(
            marker
                .get("allowFreeText")
                .and_then(|value| value.as_bool()),
            Some(allow_free_text)
        );
        assert!(
            request
                .options
                .iter()
                .all(|option| option.kind == acp::PermissionOptionKind::RejectOnce),
            "choices must stay RejectOnce so a YOLO AllowOnce drain cannot answer them"
        );
    }
}

#[test]
fn a_typed_followup_message_is_trimmed_and_blank_counts_as_absent() {
    let selected = acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
        acp::PermissionOptionId::new("question-0"),
    ));
    let plain = acp::RequestPermissionResponse::new(selected.clone());
    assert_eq!(followup_message(&plain), None);

    let typed = acp::RequestPermissionResponse::new(selected).meta(
        serde_json::json!({"followup_message": "  the typed one  "})
            .as_object()
            .cloned(),
    );
    assert_eq!(followup_message(&typed), Some("the typed one"));

    let blank = acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled).meta(
        serde_json::json!({"followup_message": "   "})
            .as_object()
            .cloned(),
    );
    assert_eq!(followup_message(&blank), None);
}

#[test]
fn free_text_only_questions_get_one_carrying_row_when_allowed() {
    let session_id = acp::SessionId::new("s");
    assert!(
        question_permission_request(&session_id, "q", "which?", &["  ".into()], false).is_none(),
        "no choices and no free text leaves the ask unprompted"
    );
    let request =
        question_permission_request(&session_id, "q", "which?", &[" ".into()], true).expect("row");
    assert_eq!(request.options.len(), 1);
    assert_eq!(
        request.options[0].kind,
        acp::PermissionOptionKind::RejectOnce
    );
    assert_eq!(request.options[0].option_id.0.as_ref(), "question-0");
    // A bare selection maps past the empty options list: no fabricated answer.
    let selected = acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
        acp::PermissionOptionId::new("question-0"),
    ));
    assert_eq!(answer_from_permission_outcome(&selected, &[]), None);
}

#[test]
fn tool_cards_use_pager_kind_and_raw_input() {
    assert_eq!(tool_kind_for("bash"), acp::ToolKind::Execute);
    assert_eq!(tool_kind_for("read"), acp::ToolKind::Read);
    assert_eq!(tool_kind_for("edit"), acp::ToolKind::Edit);
    assert_eq!(tool_kind_for("grep"), acp::ToolKind::Search);
    assert_eq!(tool_kind_for("web_fetch"), acp::ToolKind::Fetch);
    assert_eq!(tool_kind_for("web_search"), acp::ToolKind::Search);
    assert_eq!(tool_title("update_plan"), "Updating plan");

    let ls = prepared_raw_input("ls", &serde_json::json!({"path": "src"})).unwrap();
    assert_eq!(ls["target_directory"], "src");

    let search =
        prepared_raw_input("web_search", &serde_json::json!({"query": "a3s code"})).unwrap();
    assert_eq!(search["variant"], "WebSearch");

    let bash = tool_raw_output(
        "bash",
        Some(&serde_json::json!({"command": "echo hi"})),
        "hi\n",
        0,
    )
    .unwrap();
    assert_eq!(bash["type"], "Bash");
    assert_eq!(bash["exit_code"], 0);
    assert_eq!(bash["command"], "echo hi");
    assert_eq!(bash["output"], "hi\n");
    assert_eq!(bash["total_bytes"], 3);
    let delta = streaming_bash_delta("chunk");
    assert_eq!(
        (&delta["output"], &delta["output_delta"]),
        (&"".into(), &"chunk".into())
    );
}

#[test]
fn permission_response_maps_allow_and_reject() {
    let allow = acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Selected(
        acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new("allow-once")),
    ));
    assert!(permission_response_approved(&allow));

    let reject = acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Selected(
        acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new("reject-once")),
    ));
    assert!(!permission_response_approved(&reject));

    let cancelled = acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled);
    assert!(!permission_response_approved(&cancelled));
}

fn test_model(id: &str, name: &str, family: &str) -> a3s_code_core::ModelConfig {
    a3s_code_core::ModelConfig {
        id: id.to_string(),
        name: name.to_string(),
        family: family.to_string(),
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
        cost: a3s_code_core::ModelCost::default(),
        limit: a3s_code_core::ModelLimit::default(),
    }
}

pub(crate) fn test_launch(sessions_dir: Option<std::path::PathBuf>) -> crate::config::LaunchConfig {
    let config = a3s_code_core::CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![
            a3s_code_core::ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                models: vec![test_model(
                    "claude-sonnet-4-20250514",
                    "Claude Sonnet 4",
                    "claude-sonnet",
                )],
            },
            a3s_code_core::ProviderConfig {
                name: "openai".to_string(),
                api_key: Some("test-openai-key".to_string()),
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                models: vec![test_model("gpt-4o", "GPT-4o", "gpt-4")],
            },
        ],
        sessions_dir,
        ..a3s_code_core::CodeConfig::default()
    };
    crate::config::LaunchConfig {
        workspace: std::env::temp_dir(),
        model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
        config,
    }
}

/// Studio's `session/new` payload is decoded and applied by the real ACP
/// agent. GitHub and Linear land on the core manager; Slack was not installed.
#[tokio::test]
async fn session_new_mcp_servers_land_on_the_core_manager() {
    let root = std::env::temp_dir().join(format!("a3s-acp-session-mcp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let payload = serde_json::json!({
        "cwd": root,
        "mcpServers": [
            {
                "name": "github",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": [{ "name": "GITHUB_PERSONAL_ACCESS_TOKEN", "value": "ghp_x" }]
            },
            {
                "type": "http",
                "name": "linear",
                "url": "https://mcp.linear.app/mcp",
                "headers": [{ "name": "LINEAR_API_KEY", "value": "lin_x" }]
            }
        ]
    });
    let request: acp::NewSessionRequest = serde_json::from_value(payload)
        .expect("studio session/new payload decodes into NewSessionRequest");
    let created = acp::Agent::new_session(&agent, request).await;
    let configs = {
        let guard = agent.core_agent.lock().await;
        match guard.as_ref() {
            Some(core) => core.global_mcp_configs().await,
            None => Vec::new(),
        }
    };
    eprintln!(
        "core manager mcp={}",
        serde_json::to_string(&configs).unwrap_or_else(|_| "[]".to_string())
    );
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
    let created = created.expect("new session applies mcp servers before it opens");
    assert!(!created.session_id.0.as_ref().is_empty());

    let github = configs
        .iter()
        .find(|server| server.name == "github")
        .expect("github is on the core manager");
    match &github.transport {
        a3s_code_core::mcp::McpTransportConfig::Stdio { command, args } => {
            assert_eq!(command, "npx");
            assert_eq!(
                args,
                &vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string()
                ]
            );
        }
        other => panic!("github transport is stdio, got {other:?}"),
    }
    assert_eq!(
        github
            .env
            .get("GITHUB_PERSONAL_ACCESS_TOKEN")
            .map(String::as_str),
        Some("ghp_x")
    );

    let linear = configs
        .iter()
        .find(|server| server.name == "linear")
        .expect("linear is on the core manager");
    match &linear.transport {
        a3s_code_core::mcp::McpTransportConfig::StreamableHttp { url, headers } => {
            assert_eq!(url, "https://mcp.linear.app/mcp");
            assert_eq!(
                headers.get("LINEAR_API_KEY").map(String::as_str),
                Some("lin_x")
            );
        }
        other => panic!("linear transport is streamable http, got {other:?}"),
    }
    assert!(
        configs.iter().all(|server| server.name != "slack"),
        "slack was not installed"
    );
    assert_eq!(configs.len(), 2);
}

/// Studio's `session/new` payload keeps plugin tools on `_meta.a3sTools`.
/// A top-level `tools` array does not survive `NewSessionRequest`. The
/// agent registers the entry, and `AgentSession::tool` is what starts it.
#[tokio::test]
async fn session_new_plugin_tool_runs_the_unpacked_executable() {
    let root = std::env::temp_dir().join(format!("a3s-acp-plugin-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("tools")).expect("temp workspace");
    let entry = root.join("tools").join("echo");
    std::fs::write(
        &entry,
        "#!/bin/sh\nprintf '%s\\n' '{\"message\":\"cognitive package ready\"}'\n",
    )
    .expect("echo script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755))
            .expect("echo is executable");
    }
    let entry = std::fs::canonicalize(&entry).expect("canonical echo");
    let dropped: acp::NewSessionRequest = serde_json::from_value(serde_json::json!({
        "cwd": root,
        "mcpServers": [],
        "tools": [{ "id": "echo", "entry": entry }]
    }))
    .expect("a top-level tools array still decodes");
    assert!(
        dropped
            .meta
            .as_ref()
            .and_then(|meta| meta.get("a3sTools"))
            .is_none(),
        "NewSessionRequest drops a top-level tools array"
    );

    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let payload = serde_json::json!({
        "cwd": root,
        "mcpServers": [],
        "_meta": {
            "a3sHost": { "role": "You are ToBot" },
            "a3sTools": [{ "id": "echo", "entry": entry }]
        }
    });
    let request: acp::NewSessionRequest = serde_json::from_value(payload)
        .expect("studio session/new payload decodes into NewSessionRequest");
    let kept = request
        .meta
        .as_ref()
        .and_then(|meta| meta.get("a3sTools"))
        .and_then(|tools| tools.as_array())
        .expect("a3sTools survives NewSessionRequest");
    let entry_text = entry.to_string_lossy().into_owned();
    assert_eq!(kept[0]["id"], "echo");
    assert_eq!(kept[0]["entry"].as_str(), Some(entry_text.as_str()));
    let created = acp::Agent::new_session(&agent, request)
        .await
        .expect("new session registers the plugin tool");
    let session_id = created.session_id.0.as_ref().to_string();
    let (session, host) = {
        let sessions = agent.sessions.lock().await;
        let live = sessions.get(&session_id).expect("live session");
        (Arc::clone(&live.session), live.host.clone())
    };
    assert_eq!(host.role, "You are ToBot");
    let ran = session
        .tool("echo", serde_json::json!({}))
        .await
        .expect("the session tool runner starts tools/echo");
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(ran.exit_code, 0);
    assert!(
        ran.output.contains("cognitive package ready"),
        "echo output: {}",
        ran.output
    );
}

#[tokio::test]
async fn seam_admission_acp_permission_and_goal() {
    let root = std::env::temp_dir().join(format!("a3s-acp-seam-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let created = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone()))
        .await
        .expect("new session");
    let session_id = created.session_id.0.as_ref();
    assert_ne!(session_id, "tui-session");
    assert!(!session_id.is_empty());

    let applied = crate::goal::apply_goal(
        None,
        crate::goal::parse_goal_command("/goal ship the seam").expect("goal command"),
    );
    assert_eq!(
        applied.plan.mode(),
        Some(a3s_code_core::PlanningMode::Enabled)
    );
    assert_eq!(
        applied.next.as_ref().map(|goal| goal.objective.as_str()),
        Some("ship the seam")
    );
    let registered = agent.sessions.lock().await.contains_key(session_id);
    let home = a3s_code_core::session_folder::find_session_dir(&root, session_id)
        .expect("session folder lookup")
        .expect("session folder");
    assert!(home.starts_with(root.join(".a3s").join("sessions")));
    assert_eq!(
        home.file_name().and_then(|name| name.to_str()),
        Some(session_id)
    );
    let day = home
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .expect("date group");
    assert_eq!(day.len(), 10);
    let marker = std::fs::read_to_string(home.join("session.json")).expect("session.json");
    assert!(marker.contains(session_id));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::symlink_metadata(&home)
            .expect("session dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
    drop(created);
    let _ = std::fs::remove_dir_all(&root);
    assert!(registered);

    let mut executed = false;
    let decision = crate::check_admitted_tool(
        "yolo",
        "bash(*)",
        "bash",
        &serde_json::json!({"command": "echo hi"}),
        || {
            executed = true;
        },
    );
    assert_eq!(decision, Err("deny"));
    assert!(!executed);
}

#[test]
fn interactive_session_store_dir_from_configured_sessions_dir_wins() {
    let dir = interactive_session_store_dir_from(
        Some(std::path::Path::new("/tmp/acl-sessions")),
        Some(std::ffi::OsStr::new("/tmp/product")),
        Some(std::ffi::OsStr::new("/tmp/os")),
    );
    assert_eq!(dir, PathBuf::from("/tmp/acl-sessions"));
}

#[test]
fn interactive_session_store_dir_from_product_env_joins_session_store() {
    let dir = interactive_session_store_dir_from(
        None,
        Some(std::ffi::OsStr::new("/tmp/product")),
        Some(std::ffi::OsStr::new("/tmp/os")),
    );
    assert_eq!(dir, PathBuf::from("/tmp/product/session-store"));
}

#[test]
fn interactive_session_store_dir_from_os_home_joins_dot_a3s_session_store() {
    let dir = interactive_session_store_dir_from(
        None,
        Some(std::ffi::OsStr::new("")),
        Some(std::ffi::OsStr::new("/tmp/os")),
    );
    assert_eq!(dir, PathBuf::from("/tmp/os/.a3s/session-store"));
}

#[test]
fn interactive_session_store_dir_from_relative_fallback_without_homes() {
    let dir = interactive_session_store_dir_from(None, None, Some(std::ffi::OsStr::new("")));
    assert_eq!(dir, PathBuf::from(".a3s/session-store"));
}

#[test]
fn four_homes_when_inputs_differ() {
    let homes = recorded_session_homes(
        Some(std::ffi::OsStr::new("/tmp/a")),
        Some(std::ffi::OsStr::new("/tmp/b")),
        Some(std::ffi::OsStr::new("/tmp/c")),
    );
    assert_eq!(
        homes,
        vec![
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/c/.a3s"),
            PathBuf::from("/tmp/c/.grok"),
        ]
    );
}

#[test]
fn duplicate_product_home_collapses() {
    let homes = recorded_session_homes(
        Some(std::ffi::OsStr::new("/tmp/c/.a3s")),
        Some(std::ffi::OsStr::new("/tmp/b")),
        Some(std::ffi::OsStr::new("/tmp/c")),
    );
    assert_eq!(
        homes,
        vec![
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/c/.a3s"),
            PathBuf::from("/tmp/c/.grok"),
        ]
    );
}

#[test]
fn delete_removes_only_the_named_id() {
    let root =
        std::env::temp_dir().join(format!("a3s-acp-session-delete-{}", uuid::Uuid::new_v4()));
    let home_a = root.join("home-a");
    let home_b = root.join("home-b");
    let outsider = root.join("outsider");
    for (home, bucket) in [(&home_a, "cwd-1"), (&home_b, "cwd-2"), (&outsider, "cwd-3")] {
        for id in ["target-id", "other-id"] {
            let dir = home.join("sessions").join(bucket).join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("summary.json"), b"{}").unwrap();
        }
    }
    remove_recorded_session_under("target-id", &[home_a.clone(), home_b.clone()])
        .expect("delete the named session");
    assert!(!home_a.join("sessions/cwd-1/target-id").exists());
    assert!(!home_b.join("sessions/cwd-2/target-id").exists());
    assert!(home_a.join("sessions/cwd-1/other-id").is_dir());
    assert!(home_b.join("sessions/cwd-2/other-id").is_dir());
    assert!(outsider.join("sessions/cwd-3/target-id").is_dir());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn new_session_attaches_file_session_store() {
    let root = std::env::temp_dir().join(format!("a3s-acp-session-store-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let sessions_dir = root.join("sessions");
    let mut launch = test_launch(Some(sessions_dir.clone()));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let attached = async {
        let created =
            acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone())).await?;
        let live = agent.sessions.lock().await;
        let session = live
            .get(created.session_id.0.as_ref())
            .expect("live session");
        Ok::<_, acp::Error>(session.session.session_store().is_some())
    }
    .await;
    let store_exists = sessions_dir.is_dir();
    let _ = std::fs::remove_dir_all(&root);
    assert!(attached.expect("new session"));
    assert!(store_exists);
}

#[tokio::test]
async fn set_session_model_replaces_a_live_session() {
    let root = std::env::temp_dir().join(format!("a3s-acp-model-switch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let sessions_dir = root.join("sessions");
    let mut launch = test_launch(Some(sessions_dir));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let switched = async {
        let created =
            acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone())).await?;
        acp::Agent::set_session_model(
            &agent,
            acp::SetSessionModelRequest::new(
                created.session_id.clone(),
                acp::ModelId::new("openai/gpt-4o"),
            ),
        )
        .await?;
        let live = agent
            .sessions
            .lock()
            .await
            .get(created.session_id.0.as_ref())
            .map(|session| (session.model_id.clone(), session.session.is_closed()));
        Ok::<_, acp::Error>(live)
    }
    .await;
    let _ = std::fs::remove_dir_all(&root);
    let live = switched.expect("model switch must replace the live session");
    assert_eq!(live, Some(("openai/gpt-4o".to_string(), false)));
}

#[tokio::test]
async fn the_model_config_option_selects_the_session_model() {
    let root = std::env::temp_dir().join(format!("a3s-acp-model-option-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let outcome = async {
        let created =
            acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone())).await?;
        let advertised = created.config_options.clone().unwrap_or_default();
        let unknown = acp::Agent::set_session_config_option(
            &agent,
            acp::SetSessionConfigOptionRequest::new(
                created.session_id.clone(),
                "mode",
                acp::SessionConfigOptionValue::value_id("plan"),
            ),
        )
        .await;
        let response = acp::Agent::set_session_config_option(
            &agent,
            acp::SetSessionConfigOptionRequest::new(
                created.session_id.clone(),
                "model",
                acp::SessionConfigOptionValue::value_id("openai/gpt-4o"),
            ),
        )
        .await?;
        let live = agent
            .sessions
            .lock()
            .await
            .get(created.session_id.0.as_ref())
            .map(|session| session.model_id.clone());
        Ok::<_, acp::Error>((advertised, unknown.is_err(), response, live))
    }
    .await;
    let _ = std::fs::remove_dir_all(&root);
    let (advertised, unknown_rejected, response, live) = outcome.expect("model option");
    let current = |options: &[acp::SessionConfigOption]| match options {
        [option] => {
            assert_eq!(option.id.0.as_ref(), "model");
            assert_eq!(
                option.category,
                Some(acp::SessionConfigOptionCategory::Model)
            );
            match &option.kind {
                acp::SessionConfigKind::Select(select) => select.current_value.0.to_string(),
                other => panic!("model option must be a select: {other:?}"),
            }
        }
        other => panic!("exactly one model option expected: {other:?}"),
    };
    assert_ne!(current(&advertised), "openai/gpt-4o");
    assert!(unknown_rejected, "only the model option is settable");
    assert_eq!(current(&response.config_options), "openai/gpt-4o");
    assert_eq!(live.as_deref(), Some("openai/gpt-4o"));
}

#[test]
fn task_maps_to_acp_plan_entry_status_and_priority() {
    use a3s_code_core::planning::{Task, TaskPriority, TaskStatus};

    let pending = Task::new("t1", "Investigate").with_priority(TaskPriority::High);
    let entry = task_to_plan_entry(&pending);
    assert_eq!(entry.content, "Investigate");
    assert_eq!(entry.priority, acp::PlanEntryPriority::High);
    assert_eq!(entry.status, acp::PlanEntryStatus::Pending);
    assert_eq!(
        entry.meta.as_ref().and_then(|m| m.get("id")),
        Some(&serde_json::Value::String("t1".into()))
    );

    let failed = Task::new("t2", "Broken")
        .with_status(TaskStatus::Failed)
        .with_priority(TaskPriority::Low);
    let entry = task_to_plan_entry(&failed);
    assert_eq!(entry.status, acp::PlanEntryStatus::Completed);
    assert_eq!(entry.priority, acp::PlanEntryPriority::Low);
    assert_eq!(
        entry.meta.as_ref().and_then(|m| m.get("a3sStatus")),
        Some(&serde_json::Value::String("failed".into()))
    );
}

#[test]
fn tui_mappings_cover_cards_catalog_and_questions() {
    for name in [
        "shell",
        "execute",
        "git",
        "write",
        "patch",
        "glob",
        "search",
        "semantic",
        "hybrid",
        "bm25",
        "code_symbols",
        "download",
        "other",
    ] {
        let _ = tool_kind_for(name);
    }
    assert!(prepared_raw_input("read", &serde_json::Value::Null).is_none());
    assert!(prepared_raw_input("read", &serde_json::json!([1])).is_some());
    assert_eq!(
        prepared_raw_input("write", &serde_json::json!({"path": "a"})).unwrap()["variant"],
        "Write"
    );
    assert_eq!(
        prepared_raw_input("update_plan", &serde_json::json!({})).unwrap()["variant"],
        "TodoWrite"
    );
    assert_eq!(
        prepared_raw_input("task", &serde_json::json!({})).unwrap()["variant"],
        "Task"
    );
    let search = prepared_raw_input("semantic", &serde_json::json!({"query": "q"})).unwrap();
    assert_eq!(search["pattern"], "q");

    let edit = serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"});
    assert_eq!(tool_call_content("edit", Some(&edit), "").len(), 1);
    assert!(tool_call_content("read", None, "").is_empty());
    let long = "x".repeat(4_001);
    assert_eq!(tool_call_content("read", None, &long).len(), 1);
    assert!(edit_diff("read", Some(&edit)).is_none());
    assert!(edit_diff(
        "write",
        Some(&serde_json::json!({"path": "a", "content": "z"}))
    )
    .is_some());
    assert!(tool_raw_output("read", None, "out", 0).is_none());
    assert!(tool_raw_output("shell", None, "out", 1).is_some());

    assert!(effort_guideline("low").is_some());
    assert!(effort_guideline("high").is_some());
    assert!(effort_guideline("xhigh").is_some());
    assert!(effort_guideline("max").is_some());
    assert!(effort_limits("ultracode").is_some());
    assert!(effort_guideline("medium").is_none());
    let registered = a3s_code_core::tools::ToolExecutor::new(".".into())
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    let ultra_text = effort_guideline("ultracode").expect("ultracode guideline");
    for named in ultra_text.split('`').skip(1).step_by(2) {
        assert!(
            registered.iter().any(|name| name == named),
            "ultracode names unregistered tool `{named}`"
        );
    }
    for token in [
        "none",
        "minimal",
        "low",
        "medium",
        "high",
        "xhigh",
        "max",
        "ultracode",
    ] {
        let _ = effort_limits(token);
    }
    let ultra = effort_limits("ultracode").expect("ultracode");
    let _ = A3sCodeAgent::core_session_options("sid", "model", Some(ultra));
    let medium = effort_limits("medium").expect("medium");
    let _ = A3sCodeAgent::core_session_options("sid", "model", Some(medium));
    let _ = A3sCodeAgent::core_session_options("sid", "model", None);
    assert!(effort_limits_from_meta(None).is_none());

    let text =
        A3sCodeAgent::prompt_text(&[acp::ContentBlock::Text(acp::TextContent::new(" hello "))]);
    assert_eq!(text, " hello ");

    let session_id = acp::SessionId::new("s");
    let request = tool_permission_request(
        &session_id,
        "tool-1",
        "bash",
        &serde_json::json!({"command": "true"}),
    );
    assert_eq!(request.options.len(), 2);
    assert!(
        question_permission_request(&session_id, "q", "which", &["  ".into()], false).is_none()
    );
    assert!(
        question_permission_request(&session_id, "q", "which", &["  ".into()], true)
            .is_some_and(|request| request.options.len() == 1),
        "blank choices with free text allowed still get one carrying row"
    );
    let question = question_permission_request(&session_id, "q", "which", &["keep".into()], true)
        .expect("choice");
    assert_eq!(question.options[0].name, "keep");

    let options = vec![" ".to_string(), "keep".to_string()];
    assert!(
        answer_from_permission_outcome(&acp::RequestPermissionOutcome::Cancelled, &options)
            .is_none()
    );
    assert!(answer_from_permission_outcome(
        &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
            acp::PermissionOptionId::new("allow-once")
        )),
        &options
    )
    .is_none());
    assert!(answer_from_permission_outcome(
        &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
            acp::PermissionOptionId::new("question-0")
        )),
        &options
    )
    .is_none());
    assert_eq!(
        answer_from_permission_outcome(
            &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("question-1")
            )),
            &options
        )
        .as_deref(),
        Some("keep")
    );

    let mut launch = test_launch(None);
    launch.config.providers[0].name = "cc-switch".into();
    launch.config.providers[0].models[0].name = "  ".into();
    launch.config.providers[1].name = "grok".into();
    let catalog = A3sCodeAgent::new(launch);
    let names: Vec<_> = catalog
        .available_model_infos()
        .into_iter()
        .map(|info| info.name)
        .collect();
    assert!(names.iter().any(|name| name.starts_with("cc-switch")));
    assert!(names.iter().any(|name| name.starts_with("grok")));
    let fallback = catalog.model_state_with("missing/id", Some("low"));
    assert_ne!(fallback.current_model_id.0.as_ref(), "missing/id");

    let empty = crate::config::LaunchConfig {
        workspace: std::env::temp_dir(),
        model_id: "fallback/model".into(),
        config: a3s_code_core::CodeConfig {
            default_model: Some("fallback/model".into()),
            ..a3s_code_core::CodeConfig::default()
        },
    };
    let agent = A3sCodeAgent::new(empty);
    assert_eq!(agent.available_model_infos().len(), 1);
    assert!(!agent.known_model("missing/model"));
    assert!(agent.known_model("fallback/model"));
}

#[tokio::test]
async fn tui_turn_renders_every_pager_event() {
    use a3s_code_core::planning::{Complexity, ExecutionPlan, Task, TaskStatus};
    use a3s_code_core::verification::VerificationSummary;
    use a3s_code_core::TokenUsage;

    let root = std::env::temp_dir().join(format!("a3s-acp-events-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("workspace");
    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let created = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone()))
        .await
        .expect("session");
    let init = acp::Agent::initialize(
        &agent,
        acp::InitializeRequest::new(acp::ProtocolVersion::V1),
    )
    .await
    .expect("initialize");
    let meta = init.meta.expect("meta");
    assert_eq!(meta.get("a3sCode"), Some(&serde_json::Value::Bool(true)));
    acp::Agent::authenticate(&agent, acp::AuthenticateRequest::new("none"))
        .await
        .expect("authenticate");
    let _ = acp::Agent::new_session(
        &agent,
        acp::NewSessionRequest::new(std::path::PathBuf::new()),
    )
    .await;

    let empty = acp::Agent::prompt(
        &agent,
        acp::PromptRequest::new(
            created.session_id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new("   "))],
        ),
    )
    .await
    .expect("blank prompt");
    assert_eq!(empty.stop_reason, acp::StopReason::EndTurn);
    let unknown = acp::Agent::prompt(
        &agent,
        acp::PromptRequest::new(
            acp::SessionId::new("missing"),
            vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))],
        ),
    )
    .await;
    assert!(unknown.is_err());
    acp::Agent::cancel(
        &agent,
        acp::CancelNotification::new(created.session_id.clone()),
    )
    .await
    .expect("cancel");
    acp::Agent::cancel(
        &agent,
        acp::CancelNotification::new(acp::SessionId::new("missing")),
    )
    .await
    .expect("cancel missing");

    let session = agent
        .sessions
        .lock()
        .await
        .get(created.session_id.0.as_ref())
        .expect("live")
        .session
        .clone();
    let mut state = TurnState {
        observer: None,
        turn_failure: None,
        saw_text: false,
        delta_text: String::new(),
        end_text: None,
        end_outcome: None,
        achieved_goal: None,
        subagent_started_ms: HashMap::new(),
        child_progress: HashMap::new(),
        prompted_questions: std::collections::HashSet::new(),
        painted_tools: std::collections::HashSet::new(),
    };
    let sid = created.session_id.clone();
    let events = vec![
        AgentEvent::End {
            text: "only end".into(),
            usage: TokenUsage::default(),
            verification_summary: Box::new(VerificationSummary::from_reports(&[])),
            completion_terminal: Default::default(),
            forced_stop: None,
            meta: None,
        },
        AgentEvent::TextDelta {
            text: String::new(),
        },
        AgentEvent::TextDelta { text: "hi".into() },
        AgentEvent::End {
            text: "restated".into(),
            usage: TokenUsage::default(),
            verification_summary: Box::new(VerificationSummary::from_reports(&[])),
            completion_terminal: Default::default(),
            forced_stop: None,
            meta: None,
        },
        AgentEvent::ReasoningDelta {
            text: String::new(),
        },
        AgentEvent::ReasoningDelta {
            text: "think".into(),
        },
        AgentEvent::ToolStart {
            id: "t1".into(),
            name: "bash".into(),
        },
        AgentEvent::ToolExecutionStart {
            id: "t1".into(),
            name: "write".into(),
            args: serde_json::json!({"path": "a.txt", "content": "z"}),
        },
        AgentEvent::ToolOutputDelta {
            id: "t1".into(),
            name: "bash".into(),
            delta: String::new(),
        },
        AgentEvent::ToolOutputDelta {
            id: "t1".into(),
            name: "bash".into(),
            delta: "out".into(),
        },
        AgentEvent::ToolEnd {
            id: "t1".into(),
            name: "bash".into(),
            args: Some(serde_json::json!({"command": "echo"})),
            output: "ok".into(),
            exit_code: 0,
            metadata: None,
            error_kind: None,
        },
        AgentEvent::ToolEnd {
            id: "t2".into(),
            name: "edit".into(),
            args: Some(serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"})),
            output: String::new(),
            exit_code: 0,
            metadata: None,
            error_kind: None,
        },
        AgentEvent::PermissionDenied {
            tool_id: "t3".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"file_path": "secret"}),
            reason: "denied".into(),
        },
        AgentEvent::ConfirmationRequired {
            tool_id: "t4".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({"command": "true"}),
            timeout_ms: 1,
        },
        AgentEvent::TaskUpdated {
            session_id: "s".into(),
            tasks: vec![],
        },
        AgentEvent::TaskUpdated {
            session_id: "s".into(),
            tasks: vec![Task::new("p1", "Plan").with_status(TaskStatus::InProgress)],
        },
        AgentEvent::PlanningEnd {
            plan: {
                let mut plan = ExecutionPlan::new("goal", Complexity::Simple);
                plan.steps.push(Task::new("p2", "Step"));
                plan
            },
            estimated_steps: 1,
        },
        AgentEvent::SubagentStart {
            task_id: "child".into(),
            session_id: "child-session".into(),
            parent_session_id: "parent".into(),
            agent: "review".into(),
            description: "check".into(),
            started_ms: 10,
        },
        AgentEvent::SubagentProgress {
            task_id: "child".into(),
            session_id: "child-session".into(),
            status: "running".into(),
            metadata: serde_json::json!({}),
        },
        AgentEvent::SubagentProgress {
            task_id: "child".into(),
            session_id: "child-session".into(),
            status: String::new(),
            metadata: serde_json::json!({}),
        },
        AgentEvent::SubagentEnd {
            task_id: "child".into(),
            session_id: "child-session".into(),
            agent: "review".into(),
            output: "notes".into(),
            success: false,
            finished_ms: 40,
        },
        AgentEvent::SubagentEnd {
            task_id: "missing".into(),
            session_id: "child-session".into(),
            agent: "review".into(),
            output: String::new(),
            success: true,
            finished_ms: 5,
        },
        AgentEvent::SubagentEnd {
            task_id: "empty-fail".into(),
            session_id: "child-session".into(),
            agent: "review".into(),
            output: "   ".into(),
            success: false,
            finished_ms: 8,
        },
        AgentEvent::UserQuestion {
            question_id: "q1".into(),
            question: "free?".into(),
            options: vec![],
            allow_free_text: true,
            form: None,
        },
        AgentEvent::UserQuestion {
            question_id: "q2".into(),
            question: "which?".into(),
            options: vec!["keep".into()],
            allow_free_text: false,
            form: None,
        },
        AgentEvent::UserQuestion {
            question_id: "q2".into(),
            question: "which?".into(),
            options: vec!["keep".into()],
            allow_free_text: false,
            form: None,
        },
        AgentEvent::Error {
            message: "completion gate: blocked".into(),
        },
        AgentEvent::Start { prompt: "x".into() },
    ];
    for event in events {
        agent.handle_event(&sid, &session, &mut state, event).await;
    }
    assert_eq!(
        state.turn_failure.as_deref(),
        Some("completion gate: blocked")
    );
    assert!(state.saw_text);
    assert!(
        state.end_outcome.is_some(),
        "the End event must stage the turn outcome for the client notification"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Session functions the TUI renders. Each entry is an observed effect, not a
/// line of `agent.rs` that executed.
#[test]
fn tui_session_function_coverage_is_at_least_95_percent() {
    let mut covered = Vec::new();
    let mut missing = Vec::new();
    let mut record = |name: &str, ok: bool| {
        if ok {
            covered.push(name.to_string());
        } else {
            missing.push(name.to_string());
        }
    };

    record(
        "tool-card:bash",
        tool_kind_for("bash") == acp::ToolKind::Execute,
    );
    record(
        "tool-card:read",
        tool_kind_for("read") == acp::ToolKind::Read,
    );
    record(
        "tool-card:edit",
        tool_kind_for("edit") == acp::ToolKind::Edit,
    );
    record(
        "tool-card:grep",
        tool_kind_for("grep") == acp::ToolKind::Search,
    );
    record(
        "tool-card:web-fetch",
        tool_kind_for("web_fetch") == acp::ToolKind::Fetch,
    );
    record("plan-title", tool_title("update_plan") == "Updating plan");
    let edit = serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"});
    record("edit-diff", edit_diff("edit", Some(&edit)).is_some());
    record(
        "bash-output",
        tool_raw_output("bash", None, "ok", 0).is_some(),
    );
    let session_id = acp::SessionId::new("s");
    let permission = tool_permission_request(
        &session_id,
        "tool-1",
        "bash",
        &serde_json::json!({"command": "true"}),
    );
    record(
        "permission-dialog",
        permission.options.len() == 2
            && permission.options[0].kind == acp::PermissionOptionKind::AllowOnce
            && permission.options[1].kind == acp::PermissionOptionKind::RejectOnce,
    );
    record(
        "question-free-text",
        question_permission_request(&session_id, "q", "which", &["  ".into()], true).is_some_and(
            |request| {
                request.options.len() == 1
                    && request.options[0].option_id.0.as_ref() == "question-0"
            },
        ) && question_permission_request(&session_id, "q", "which", &["  ".into()], false)
            .is_none(),
    );
    let question = question_permission_request(&session_id, "q", "which", &["keep".into()], true);
    record(
        "question-choices",
        question.as_ref().is_some_and(|request| {
            request.options.len() == 1
                && request.options[0].kind == acp::PermissionOptionKind::RejectOnce
                && request.options[0].name == "keep"
        }),
    );
    let choices = vec!["keep".to_string()];
    record(
        "question-cancel",
        answer_from_permission_outcome(&acp::RequestPermissionOutcome::Cancelled, &choices)
            .is_none(),
    );
    record(
        "question-answer",
        answer_from_permission_outcome(
            &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("question-0"),
            )),
            &choices,
        )
        .as_deref()
            == Some("keep"),
    );
    record(
        "end-without-deltas",
        should_emit_end_text(false, "reply") && !should_emit_end_text(false, ""),
    );
    record("end-after-deltas", !should_emit_end_text(true, "reply"));
    let failure = agent_failure_error("completion gate: blocked");
    let detail = failure
        .data
        .as_ref()
        .and_then(|value| value.as_str())
        .unwrap_or("");
    record("turn-failed", detail.contains("completion gate:"));
    let menu = reasoning_effort_catalog_meta("anthropic/claude-opus-4-6");
    let values = menu
        .get("reasoningEfforts")
        .and_then(|value| value.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("value").and_then(|value| value.as_str()))
                .collect::<Vec<_>>()
        });
    record(
        "effort-menu",
        values.as_deref() == Some(&["low", "medium", "high", "xhigh", "max"][..]),
    );
    record(
        "effort-high-budget",
        effort_limits("high").is_some_and(|limits| {
            limits.token == "high"
                && limits.thinking_budget == 16_384
                && limits.max_tool_rounds == 128
                && limits.max_tool_rounds < a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
        }),
    );
    record(
        "effort-medium-no-guideline",
        effort_guideline("medium").is_none(),
    );

    let total = covered.len() + missing.len();
    let percent = covered.len() * 100 / total.max(1);
    assert!(
        percent >= 95,
        "TUI session function coverage is {percent}% ({}/{}). Missing: {}",
        covered.len(),
        total,
        missing.join(", ")
    );
}

#[test]
fn confirmation_then_execution_opens_one_tool_row() {
    let mut painted = std::collections::HashSet::new();
    assert!(matches!(
        note_tool_row(&mut painted, "call-bash"),
        ToolRowPaint::Open
    ));
    assert!(matches!(
        note_tool_row(&mut painted, "call-bash"),
        ToolRowPaint::Update
    ));
    assert!(matches!(
        note_tool_row(&mut painted, "call-search"),
        ToolRowPaint::Open
    ));
}

#[test]
fn configured_skill_directory_that_is_missing_is_named() {
    let root = std::env::temp_dir().join(format!("a3s-acp-skill-dirs-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".a3s/skills")).unwrap();
    let missing = root.join("missing-skills");
    let (dirs, absent) = interactive_skill_dirs_from(&root, std::slice::from_ref(&missing));
    assert!(dirs.iter().any(|dir| dir.ends_with(".a3s/skills")));
    assert_eq!(absent, vec![missing]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn watched_skill_dirs_include_home_agent_roots_before_they_exist() {
    let workspace = std::path::Path::new("/tmp/a3s-watch-skills-workspace");
    let dirs = watched_skill_dirs(workspace);
    assert!(dirs.iter().any(|dir| dir == &workspace.join("skills")));
    assert!(dirs
        .iter()
        .any(|dir| dir == &workspace.join(".a3s").join("skills")));
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(home) = home.filter(|value| !value.is_empty()) else {
        return;
    };
    let home = std::path::PathBuf::from(home);
    for root in [".a3s", ".agents", ".claude", ".codex"] {
        let personal = home.join(root).join("skills");
        assert!(
            dirs.iter().any(|dir| dir == &personal),
            "watch list missing {}",
            personal.display()
        );
    }
    assert!(dirs
        .iter()
        .all(|dir| !dir.components().any(|part| part.as_os_str() == ".grok")));
}

#[test]
fn delete_names_a_sessions_path_that_is_not_a_directory() {
    let root = std::env::temp_dir().join(format!(
        "a3s-acp-session-delete-file-{}",
        uuid::Uuid::new_v4()
    ));
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("sessions"), b"not-a-directory").unwrap();
    let error = remove_recorded_session_under("target-id", &[home]).unwrap_err();
    assert!(error.contains("sessions"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn turn_queue_rejects_beyond_the_bound_and_frees_slots_on_drop() {
    let queue = Arc::new(TurnQueue::new());
    // One turn is running; it is not a waiter.
    let running = queue.acquire_owned().await.expect("first acquire");

    let mut slots = Vec::new();
    for _ in 0..MAX_QUEUED_PROMPTS {
        slots.push(queue.reserve().expect("within the bound"));
    }
    // The newest prompt is rejected once the bound is reached…
    assert!(
        queue.reserve().is_none(),
        "one past the bound must be rejected"
    );
    // …a dropped waiter (cancelled future) frees its slot…
    slots.pop();
    let recycled = queue.reserve().expect("freed slot is reusable");
    // …and the next one is over the bound again.
    assert!(queue.reserve().is_none());
    drop(recycled);
    drop(slots);
    drop(running);
    // With the running turn done and no waiters, acquisition proceeds.
    queue
        .acquire_owned()
        .await
        .expect("queue drains after the turn");
}

#[tokio::test]
async fn cancelled_acquisition_returns_its_waiting_slot() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    let queue = Arc::new(TurnQueue::new());
    let running = queue.acquire_owned().await.expect("running turn");

    // A queued acquisition parks while the turn runs. Poll it once with a
    // noop waker (deterministic — no spawn/timing race): it must register
    // as a waiter and stay pending…
    let mut queued = Box::pin(queue.acquire_owned());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(
        matches!(queued.as_mut().poll(&mut cx), Poll::Pending),
        "queued acquisition parks behind the running turn"
    );

    // …then the client drops the request future. Dropping the pinned
    // future runs the slot guard's Drop — the slot must come back…
    drop(queued);
    let mut slots = Vec::new();
    for _ in 0..MAX_QUEUED_PROMPTS {
        slots.push(queue.reserve().expect("slot was returned"));
    }
    assert!(
        queue.reserve().is_none(),
        "no capacity leaked by the cancelled future"
    );
    drop(slots);
    drop(running);
}

/// A `UserQuestion` carrying a form spec must reach the client as a
/// `_a3s/form/request` ext round trip, and the submitted values object
/// must land in `ask_user` so the parked run resumes with it.
#[tokio::test]
async fn user_question_with_form_round_trips_the_submitted_values() {
    let root = std::env::temp_dir().join(format!("a3s-acp-form-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    struct Workspace(std::path::PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _workspace = Workspace(root.clone());
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut launch = tests::test_launch(Some(root.join("sessions")));
            launch.workspace = root.clone();
            let handler_agent = A3sCodeAgent::new(tests::test_launch(Some(root.join("sessions"))));
            let (client_end, agent_end) = tokio::io::duplex(64 * 1024);
            use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
            let (agent_read, agent_write) = tokio::io::split(agent_end);
            let (conn, io) = acp::AgentSideConnection::new(
                handler_agent,
                agent_write.compat_write(),
                agent_read.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(io);
            let agent = A3sCodeAgent::new(launch);
            agent.set_client(Rc::new(conn));

            let request: acp::NewSessionRequest = serde_json::from_value(serde_json::json!({
                "cwd": root,
                "mcpServers": []
            }))
            .unwrap();
            let created = acp::Agent::new_session(&agent, request).await.unwrap();
            let session = {
                let sessions = agent.sessions.lock().await;
                Arc::clone(&sessions.get(created.session_id.0.as_ref()).unwrap().session)
            };
            let mut state = TurnState {
                observer: None,
                turn_failure: None,
                saw_text: false,
                delta_text: String::new(),
                end_text: None,
                end_outcome: None,
        achieved_goal: None,
                subagent_started_ms: HashMap::new(),
                child_progress: HashMap::new(),
                prompted_questions: std::collections::HashSet::new(),
                painted_tools: std::collections::HashSet::new(),
            };
            let spec = r#"<Card title="Leave request"><Input name="reason" label="Reason" /><Button action="submit" label="Submit" /></Card>"#;
            let event = AgentEvent::UserQuestion {
                question_id: "ask-form-run-1-1".into(),
                question: "Please fill in the leave request".into(),
                options: vec![],
                allow_free_text: true,
                form: Some(spec.into()),
            };
            // handle_event takes &self through a fresh handle; the event arm
            // only reads shared state, so a second agent from the same launch
            // would diverge — instead move the original agent into the task.
            tokio::task::spawn_local(async move {
                agent.handle_event(&created.session_id, &session, &mut state, event).await;
            });

            // Raw client: read the ext request line, answer with a values object.
            let (mut client_read, mut client_write) = tokio::io::split(client_end);
            let mut line = String::new();
            let mut reader = tokio::io::BufReader::new(&mut client_read);
            use tokio::io::AsyncBufReadExt;
            loop {
                line.clear();
                reader.read_line(&mut line).await.expect("read ext request");
                if line.contains("_a3s/form/request") {
                    break;
                }
            }
            let request_frame: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(request_frame["method"], "_a3s/form/request");
            assert_eq!(
                request_frame["params"]["formId"], "ask-form-run-1-1",
                "{request_frame}"
            );
            assert_eq!(request_frame["params"]["spec"], spec);
            assert_eq!(request_frame["params"]["format"], "openui");
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request_frame["id"],
                "result": { "values": { "reason": "Feeling unwell" } }
            });
            use tokio::io::AsyncWriteExt;
            client_write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();

            let cancel = tokio_util::sync::CancellationToken::new();
            let answered = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                a3s_code_core::ask_user::wait_for_answer("ask-form-run-1-1", &cancel),
            )
            .await
            .expect("answer arrives")
            .expect("stored answer");
            assert_eq!(answered, r#"{"reason":"Feeling unwell"}"#);
        })
        .await;
}

/// A choices question rides `session/request_permission` marked as a question
/// (`_meta.question`), keeps every choice on the `RejectOnce` kind, and the
/// typed followup answer beats the selected choice row; without typed text the
/// selected choice's own text is the answer.
#[tokio::test]
async fn user_question_choices_round_trip_typed_and_selected_answers() {
    let root = std::env::temp_dir().join(format!("a3s-acp-question-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    struct Workspace(std::path::PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _workspace = Workspace(root.clone());
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut launch = tests::test_launch(Some(root.join("sessions")));
            launch.workspace = root.clone();
            let handler_agent = A3sCodeAgent::new(tests::test_launch(Some(root.join("sessions"))));
            let (client_end, agent_end) = tokio::io::duplex(64 * 1024);
            use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
            let (agent_read, agent_write) = tokio::io::split(agent_end);
            let (conn, io) = acp::AgentSideConnection::new(
                handler_agent,
                agent_write.compat_write(),
                agent_read.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(io);
            let agent = A3sCodeAgent::new(launch);
            agent.set_client(Rc::new(conn));

            let request: acp::NewSessionRequest = serde_json::from_value(serde_json::json!({
                "cwd": root,
                "mcpServers": []
            }))
            .unwrap();
            let created = acp::Agent::new_session(&agent, request).await.unwrap();
            let session = {
                let sessions = agent.sessions.lock().await;
                Arc::clone(&sessions.get(created.session_id.0.as_ref()).unwrap().session)
            };
            let mut state = TurnState {
                observer: None,
                turn_failure: None,
                saw_text: false,
                delta_text: String::new(),
                end_text: None,
                end_outcome: None,
                achieved_goal: None,
                subagent_started_ms: HashMap::new(),
                child_progress: HashMap::new(),
                prompted_questions: std::collections::HashSet::new(),
                painted_tools: std::collections::HashSet::new(),
            };
            let session_id = created.session_id.clone();
            tokio::task::spawn_local(async move {
                for (question_id, question, options, allow_free_text) in [
                    ("ask-e2e-typed-1", "Pick one", vec!["left", "right"], true),
                    (
                        "ask-e2e-choice-1",
                        "Pick again",
                        vec!["alpha", "beta"],
                        false,
                    ),
                    ("ask-e2e-free-1", "Name the target", vec![], true),
                ] {
                    agent
                        .handle_event(
                            &session_id,
                            &session,
                            &mut state,
                            AgentEvent::UserQuestion {
                                question_id: question_id.into(),
                                question: question.into(),
                                options: options.into_iter().map(str::to_string).collect(),
                                allow_free_text,
                                form: None,
                            },
                        )
                        .await;
                }
            });

            let (mut client_read, mut client_write) = tokio::io::split(client_end);
            let mut reader = tokio::io::BufReader::new(&mut client_read);
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            for (expect_free, reply_option, typed, expected_answer, question_id) in [
                (
                    true,
                    "question-1",
                    Some("the typed one"),
                    "the typed one",
                    "ask-e2e-typed-1",
                ),
                (false, "question-0", None, "alpha", "ask-e2e-choice-1"),
                (
                    true,
                    "question-0",
                    Some("my own words"),
                    "my own words",
                    "ask-e2e-free-1",
                ),
            ] {
                let mut line = String::new();
                loop {
                    line.clear();
                    reader
                        .read_line(&mut line)
                        .await
                        .expect("read permission request");
                    if line.contains("session/request_permission") {
                        break;
                    }
                }
                let frame: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let params = &frame["params"];
                assert_eq!(
                    params["_meta"]["question"]["allowFreeText"].as_bool(),
                    Some(expect_free),
                    "question marker missing or wrong: {frame}"
                );
                assert!(
                    params["toolCall"]["title"]
                        .as_str()
                        .is_some_and(|title| !title.is_empty()),
                    "question text must ride the tool-call title: {frame}"
                );
                for option in params["options"].as_array().expect("choice options") {
                    assert_eq!(
                        option["kind"], "reject_once",
                        "choices ride RejectOnce so YOLO cannot answer: {frame}"
                    );
                }
                let mut result = serde_json::json!({
                    "outcome": { "outcome": "selected", "optionId": reply_option }
                });
                if let Some(text) = typed {
                    result["_meta"] = serde_json::json!({ "followup_message": text });
                }
                let reply =
                    serde_json::json!({ "jsonrpc": "2.0", "id": frame["id"], "result": result });
                client_write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();

                let cancel = tokio_util::sync::CancellationToken::new();
                let answered = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    a3s_code_core::ask_user::wait_for_answer(question_id, &cancel),
                )
                .await
                .expect("answer arrives")
                .expect("stored answer");
                assert_eq!(answered, expected_answer);
            }
        })
        .await;
}

#[test]
fn completion_bell_respects_the_configured_floor_and_signal() {
    use a3s_code_core::config::{NotificationsConfig, RunCompletionSignal};
    let bell = NotificationsConfig {
        run_completion: RunCompletionSignal::Bell,
        min_seconds: 2,
    };
    let off = NotificationsConfig {
        run_completion: RunCompletionSignal::Off,
        min_seconds: 2,
    };
    let at = |secs: u64| std::time::Duration::from_secs(secs);
    assert!(super::completion_bell_due(&bell, at(2)));
    assert!(super::completion_bell_due(&bell, at(30)));
    assert!(!super::completion_bell_due(&bell, at(1)), "below the floor");
    assert!(
        !super::completion_bell_due(&off, at(3600)),
        "off never signals regardless of duration"
    );
}

#[tokio::test]
async fn sampling_consent_resolves_the_owning_session() {
    let root = std::env::temp_dir().join(format!("a3s-acp-consent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let opened = async {
        let one = serde_json::json!({
            "cwd": root,
            "mcpServers": [
                { "name": "alpha", "command": "printf", "args": ["a"],
                  "env": [{ "name": "E", "value": "1" }] }
            ]
        });
        let request: acp::NewSessionRequest = serde_json::from_value(one).expect("decodes");
        let first = acp::Agent::new_session(&agent, request).await?;
        let second =
            acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone())).await?;
        Ok::<_, acp::Error>((first, second))
    }
    .await;
    let _ = std::fs::remove_dir_all(&root);
    let (first, second) = opened.expect("sessions open");
    let sessions = agent.sessions.lock().await;
    // The declaring session owns its server's consent questions.
    assert_eq!(
        super::resolve_consent_session(&sessions, "alpha").map(|id| id.0.to_string()),
        Some(first.session_id.0.to_string())
    );
    // With no sessions at all there is nobody to ask; the caller
    // refuses (fail closed).
    assert!(super::resolve_consent_session(&HashMap::new(), "config-server").is_none());
    drop(sessions);
    drop(agent);
    let _ = second;
}

#[test]
fn union_mcp_desired_covers_remodel_and_close_shapes() {
    use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
    fn server(name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            transport: McpTransportConfig::Stdio {
                command: command.into(),
                args: Vec::new(),
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }
    fn acp_server(name: &str, command: &str) -> acp::McpServer {
        acp::McpServer::Stdio(acp::McpServerStdio::new(name, command))
    }
    fn names(set: &[McpServerConfig]) -> Vec<&str> {
        set.iter().map(|s| s.name.as_str()).collect()
    }

    let baseline = vec![server("config", "cfg")];
    let live = vec![
        ("s1".to_string(), vec![server("a", "a-cmd")]),
        ("s2".to_string(), vec![server("b", "b-cmd")]),
    ];

    // Plain union across sessions.
    let union = super::union_mcp_desired(baseline.clone(), &live, None, &[])
        .expect("distinct names never conflict");
    assert_eq!(names(&union), vec!["config", "a", "b"]);

    // Remodel: s2's stale set is skipped, its new set arrives via `add`.
    let remodeled = super::union_mcp_desired(
        baseline.clone(),
        &live,
        Some("s2"),
        &[acp_server("b2", "new")],
    )
    .expect("a remodeled session's new set merges in");
    assert_eq!(names(&remodeled), vec!["config", "a", "b2"]);

    // Close: with s1 gone from the live map, no skip, no add — exactly the
    // remaining union.
    let remaining = vec![("s2".to_string(), vec![server("b", "b-cmd")])];
    let closed = super::union_mcp_desired(baseline, &remaining, None, &[])
        .expect("the close resync never conflicts");
    assert_eq!(names(&closed), vec!["config", "b"]);
}

#[test]
fn union_mcp_desired_refuses_a_cross_session_name_takeover() {
    use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
    fn server(name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            transport: McpTransportConfig::Stdio {
                command: command.into(),
                args: Vec::new(),
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }
    fn acp_server(name: &str, command: &str) -> acp::McpServer {
        acp::McpServer::Stdio(acp::McpServerStdio::new(name, command))
    }

    // s1 live-holds "probe" under one command; s2's new set tries the same
    // name under a different definition — a takeover of the mcp__probe__
    // prefix must be refused, naming both origins.
    let live = vec![(
        "s1".to_string(),
        vec![server("probe", "session-one-command")],
    )];
    let error = super::union_mcp_desired(
        Vec::new(),
        &live,
        Some("s2"),
        &[acp_server("probe", "attacker-command")],
    )
    .expect_err("a different definition under a live session's name must refuse");
    assert!(error.contains("probe"), "names the server: {error}");
    assert!(error.contains("s1"), "names the holder: {error}");
    assert!(error.contains("s2"), "names the refused origin: {error}");
}

#[test]
fn union_mcp_desired_dedupes_a_same_definition_shared_name() {
    use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
    fn server(name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            transport: McpTransportConfig::Stdio {
                command: command.into(),
                args: Vec::new(),
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }
    // Two live sessions sharing one identical server definition is the
    // legitimate sharing case: one entry, no refusal.
    let live = vec![
        ("s1".to_string(), vec![server("probe", "shared-command")]),
        ("s2".to_string(), vec![server("probe", "shared-command")]),
    ];
    let union = super::union_mcp_desired(Vec::new(), &live, None, &[])
        .expect("identical definitions dedupe");
    assert_eq!(union.len(), 1);
    assert_eq!(union[0].name, "probe");
}

#[test]
fn union_mcp_desired_keeps_same_session_updates_later_wins() {
    use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
    fn server(name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            transport: McpTransportConfig::Stdio {
                command: command.into(),
                args: Vec::new(),
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }
    // A session re-declaring its OWN server updates it — the within-origin
    // later-wins rule the old primitive pinned.
    let live = vec![(
        "s1".to_string(),
        vec![server("probe", "old"), server("probe", "new")],
    )];
    let union = super::union_mcp_desired(Vec::new(), &live, None, &[])
        .expect("same-origin updates never conflict");
    assert_eq!(union.len(), 1);
    let command = match &union[0].transport {
        McpTransportConfig::Stdio { command, .. } => command.clone(),
        _ => String::new(),
    };
    assert_eq!(command, "new");
}

#[tokio::test]
async fn set_session_model_keeps_the_sessions_mcp_contribution_alive() {
    let root = std::env::temp_dir().join(format!("a3s-acp-model-mcp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");
    let mut launch = test_launch(Some(root.join("sessions")));
    launch.workspace = root.clone();
    let agent = A3sCodeAgent::new(launch);
    let outcome = async {
        let payload = serde_json::json!({
            "cwd": root,
            "mcpServers": [
                {
                    "name": "alpha",
                    "command": "printf",
                    "args": ["alpha"],
                    "env": [{ "name": "ALPHA_PROBE", "value": "1" }]
                }
            ]
        });
        let request: acp::NewSessionRequest =
            serde_json::from_value(payload).expect("payload decodes");
        let created = acp::Agent::new_session(&agent, request).await?;

        acp::Agent::set_session_model(
            &agent,
            acp::SetSessionModelRequest::new(
                created.session_id.clone(),
                acp::ModelId::new("openai/gpt-4o"),
            ),
        )
        .await?;

        // A second session opening forces a union resync: the remodeled
        // session's recorded set must still contribute "alpha" — before
        // the fix, the rebuild dropped it and this sync evicted the
        // servers of a still-live session.
        acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone())).await?;
        let configs = {
            let guard = agent.core_agent.lock().await;
            match guard.as_ref() {
                Some(core) => core.global_mcp_configs().await,
                None => Vec::new(),
            }
        };
        Ok::<_, acp::Error>(configs)
    }
    .await;
    drop(agent);
    let _ = std::fs::remove_dir_all(&root);
    let configs = outcome.expect("model switch then resync");
    assert!(
        configs.iter().any(|server| server.name == "alpha"),
        "the remodeled session's servers survive the next union sync: {configs:?}"
    );
}

#[test]
fn only_a_learnable_bash_command_offers_allow_always_and_only_it_learns() {
    let session_id = acp::SessionId::new("s");
    let learnable = tool_permission_request(
        &session_id,
        "tool-1",
        "bash",
        &serde_json::json!({"command": "git status --short"}),
    );
    let kinds = learnable
        .options
        .iter()
        .map(|option| option.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            acp::PermissionOptionKind::AllowOnce,
            acp::PermissionOptionKind::AllowAlways,
            acp::PermissionOptionKind::RejectOnce,
        ]
    );
    assert!(learnable.options[1].name.contains("`git status`"));
    for (tool, command) in [("bash", "git status; rm -rf /"), ("write", "ls")] {
        let request = tool_permission_request(
            &session_id,
            "tool-2",
            tool,
            &serde_json::json!({"command": command}),
        );
        assert!(
            request
                .options
                .iter()
                .all(|option| option.kind != acp::PermissionOptionKind::AllowAlways),
            "{tool} {command}"
        );
    }
    let answer = |id: &str| {
        acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Selected(
            acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new(id.to_string())),
        ))
    };
    assert!(permission_response_approved(&answer("allow-once")));
    assert!(!permission_response_always(&answer("allow-once")));
    assert!(permission_response_approved(&answer("allow-always")));
    assert!(permission_response_always(&answer("allow-always")));
    assert!(!permission_response_approved(&answer("reject-once")));
    assert!(!permission_response_always(&answer("reject-once")));
}
