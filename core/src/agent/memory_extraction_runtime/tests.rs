use super::*;

fn snapshot(state: &ExecutionLoopState) -> MemoryExtractionSnapshot {
    MemoryExtractionSnapshot::from_state(state)
}

#[test]
fn parses_fenced_extraction_json() {
    let items = parse_extracted_memories(
            r#"```json
{"items":[{"memory_type":"semantic","content":"A3S memory should store durable project facts.","importance":0.8,"tags":["A3S","Memory"],"source":"project_fact"}]}
```"#,
        )
        .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].memory_type, "semantic");
}

#[test]
fn parser_keeps_valid_items_when_sibling_is_malformed() {
    let items = parse_extracted_memories(
            r#"{"items":[
                {"memory_type":"semantic","content":"A3S memory extraction runs after significant completed turns.","importance":0.8,"tags":["memory"],"source":"project_fact"},
                {"memory_type":"semantic","content":42,"importance":"high"},
                {"memory_type":"procedural","content":"Run memory extraction tests after changing extraction parsing behavior.","importance":0.7,"tags":["tests"],"source":"workflow"}
            ]}"#,
        )
        .unwrap();

    assert_eq!(items.len(), 2);
    assert!(items[0].content.contains("completed turns"));
    assert!(items[1].content.contains("extraction parsing"));
}

#[test]
fn missing_extracted_content_is_skipped_during_item_conversion() {
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: String::new(),
        importance: Some(0.8),
        tags: vec!["memory".to_string()],
        source: Some("project_fact".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
    };

    assert!(extracted
        .into_memory_item("remember memory behavior", "sess-1", &HashSet::new())
        .is_none());
}

#[test]
fn extracted_memory_becomes_tagged_item() {
    let extracted = ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Run focused memory tests after changing FileMemoryStore.".to_string(),
        importance: Some(0.9),
        tags: vec!["Memory!".to_string(), "Tests".to_string()],
        source: Some("workflow".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
    };

    let (item, supersedes, conflicts_with) = extracted
        .into_memory_item("optimize memory", "sess-1", &HashSet::new())
        .unwrap();
    assert!(supersedes.is_empty());
    assert!(conflicts_with.is_empty());
    assert_eq!(item.memory_type, MemoryType::Procedural);
    assert!(item.tags.contains(&"llm".to_string()));
    assert!(item.tags.contains(&"memory".to_string()));
    assert_eq!(item.metadata.get("source").unwrap(), "workflow");
}

#[test]
fn extracted_memory_skips_sensitive_content() {
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "The production API key is sk-1234567890abcdef1234567890abcdef.".to_string(),
        importance: Some(0.9),
        tags: vec!["secret".to_string()],
        source: Some("project_fact".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
    };

    assert!(extracted
        .into_memory_item("remember the key", "sess-1", &HashSet::new())
        .is_none());
}

#[test]
fn extracted_memory_redacts_sensitive_prompt_metadata() {
    let extracted = ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Use environment variables when configuring provider credentials.".to_string(),
        importance: Some(0.8),
        tags: vec!["config".to_string()],
        source: Some("workflow".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
    };

    let (item, _, _) = extracted
        .into_memory_item(
            "provider api_key = sk-1234567890abcdef1234567890abcdef",
            "sess-1",
            &HashSet::new(),
        )
        .unwrap();

    let prompt = item.metadata.get("prompt").unwrap();
    assert!(prompt.contains(SENSITIVE_REDACTION));
    assert!(!prompt.contains("sk-1234567890abcdef"));
}

#[test]
fn extraction_prompt_redacts_sensitive_turn_fields() {
    let prompt = build_extraction_prompt(
        "provider api_key = sk-1234567890abcdef1234567890abcdef",
        "Use token: ghp_1234567890abcdef1234567890abcdef",
        "assistant: password = supersecret123",
        "None",
        3,
    );

    assert!(prompt.contains(SENSITIVE_REDACTION));
    assert!(!prompt.contains("sk-1234567890abcdef"));
    assert!(!prompt.contains("ghp_1234567890abcdef"));
    assert!(!prompt.contains("supersecret123"));
}

#[test]
fn extraction_source_is_whitelisted() {
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "A3S memory uses gated LLM extraction after completed turns.".to_string(),
        importance: Some(0.8),
        tags: vec![],
        source: Some("api_key = sk-1234567890abcdef1234567890abcdef".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
    };

    let (item, _, _) = extracted
        .into_memory_item("memory design", "sess-1", &HashSet::new())
        .unwrap();
    assert_eq!(item.metadata.get("source").unwrap(), "llm_extractor");
}

#[test]
fn extracted_memory_records_allowed_supersedes() {
    let allowed_id = uuid::Uuid::new_v4().to_string();
    let ignored_id = uuid::Uuid::new_v4().to_string();
    let allowed = HashSet::from([allowed_id.clone()]);
    let extracted = ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Run focused memory and file-store tests after changing memory persistence."
            .to_string(),
        importance: Some(0.9),
        tags: vec!["memory".to_string()],
        source: Some("workflow".to_string()),
        supersedes: vec![allowed_id.clone(), ignored_id],
        conflicts_with: vec![],
    };

    let (item, supersedes, conflicts_with) = extracted
        .into_memory_item("memory design", "sess-1", &allowed)
        .unwrap();

    assert_eq!(supersedes, vec![allowed_id.clone()]);
    assert!(conflicts_with.is_empty());
    assert!(item.tags.contains(&"consolidated".to_string()));
    assert_eq!(item.metadata.get("supersedes").unwrap(), &allowed_id);
}

#[test]
fn extracted_memory_records_allowed_conflicts() {
    let conflict_id = uuid::Uuid::new_v4().to_string();
    let ignored_id = uuid::Uuid::new_v4().to_string();
    let allowed = HashSet::from([conflict_id.clone()]);
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "This project currently prefers workspace-local memory stores.".to_string(),
        importance: Some(0.75),
        tags: vec!["memory".to_string()],
        source: Some("decision".to_string()),
        supersedes: vec![],
        conflicts_with: vec![conflict_id.clone(), ignored_id],
    };

    let (item, supersedes, conflicts_with) = extracted
        .into_memory_item("memory design", "sess-1", &allowed)
        .unwrap();

    assert!(supersedes.is_empty());
    assert_eq!(conflicts_with, vec![conflict_id.clone()]);
    assert!(item.tags.contains(&"conflict".to_string()));
    assert_eq!(item.metadata.get("conflicts_with").unwrap(), &conflict_id);
}

#[test]
fn related_memories_are_formatted_as_json_lines() {
    let item = MemoryItem::new("Run focused memory store tests after FileMemoryStore changes.")
        .with_type(MemoryType::Procedural)
        .with_importance(0.84)
        .with_tag("Memory!")
        .with_metadata("source", "workflow");

    let formatted = format_related_memories_for_extraction(vec![item.clone()]);

    assert!(formatted.prompt.contains(&format!(r#""id":"{}""#, item.id)));
    assert!(formatted.prompt.contains(r#""type":"procedural""#));
    assert!(formatted.prompt.contains(r#""source":"workflow""#));
    assert!(formatted.prompt.contains(r#""tags":["memory"]"#));
    assert!(formatted.prompt.contains("FileMemoryStore changes"));
    assert!(formatted.allowed_supersedes.contains(&item.id));
}

#[test]
fn related_memories_include_existing_relation_metadata() {
    let item =
        MemoryItem::new("Use the consolidated memory workflow for project-specific preferences.")
            .with_type(MemoryType::Semantic)
            .with_metadata("supersedes", "old-preference, bad id with spaces")
            .with_metadata("conflicts_with", "legacy-default,<script>");

    let formatted = format_related_memories_for_extraction(vec![item]);

    assert!(formatted
        .prompt
        .contains(r#""supersedes":["old-preference"]"#));
    assert!(formatted
        .prompt
        .contains(r#""conflicts_with":["legacy-default"]"#));
    assert!(!formatted.prompt.contains("bad id with spaces"));
    assert!(!formatted.prompt.contains("<script>"));
}

#[test]
fn related_memories_skip_sensitive_items() {
    let secret = MemoryItem::new("The provider token is sk-1234567890abcdef1234567890abcdef.")
        .with_type(MemoryType::Semantic);
    let safe = MemoryItem::new("Prefer environment variables for provider credentials.")
        .with_type(MemoryType::Procedural);

    let formatted = format_related_memories_for_extraction(vec![secret, safe]);

    assert!(!formatted.prompt.contains("sk-1234567890abcdef"));
    assert!(formatted.prompt.contains("environment variables"));
    assert_eq!(formatted.allowed_supersedes.len(), 1);
}

#[tokio::test]
async fn related_memories_are_loaded_for_extraction_prompt() {
    let memory = Arc::new(AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new())));
    memory
        .remember(
            MemoryItem::new(
                "Run focused memory store tests after changing FileMemoryStore behavior.",
            )
            .with_type(MemoryType::Procedural)
            .with_tag("memory"),
        )
        .await
        .unwrap();

    let related = related_memories_for_extraction(
        &memory,
        "remember FileMemoryStore testing workflow",
        "Use focused memory tests.",
    )
    .await;
    let prompt = build_extraction_prompt("p", "r", "t", &related.prompt, 2);

    assert!(prompt.contains("Related existing memories"));
    assert!(prompt.contains("FileMemoryStore behavior"));
    assert!(prompt.contains("avoid duplicates"));
    assert_eq!(related.allowed_supersedes.len(), 1);
}

#[test]
fn duplicate_memory_detection_is_conservative() {
    assert!(memory_contents_are_duplicates(
        "Run focused memory store tests after changing FileMemoryStore behavior.",
        "  run focused memory store tests after changing FileMemoryStore behavior.  "
    ));
    assert!(memory_contents_are_duplicates(
        "Run focused memory store regression tests after changing FileMemoryStore behavior.",
        "Run focused memory store tests after changing FileMemoryStore behavior."
    ));
    assert!(!memory_contents_are_duplicates(
        "Run focused memory store tests after changing FileMemoryStore behavior.",
        "Prefer HCL configuration files for repository-level product settings."
    ));
}

#[test]
fn extraction_gate_skips_trivial_turns() {
    let state = ExecutionLoopState::new(&[]);
    assert!(!should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "hi",
        "hello"
    ));
}

#[test]
fn extraction_gate_accepts_tool_turns() {
    let mut state = ExecutionLoopState::new(&[]);
    state.tool_calls_count = 1;
    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "run tests",
        "tests passed"
    ));
}

#[test]
fn extraction_gate_skips_short_successful_read_only_tool_turns() {
    let mut state = ExecutionLoopState::new(&[]);
    state.tool_calls_count = 1;
    state.remember_tool_signature("read", &serde_json::json!({"path":"README.md"}), false);

    assert!(!should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "read README",
        "Here is the README."
    ));
}

#[test]
fn extraction_gate_skips_short_successful_code_intelligence_turns() {
    for tool in ["code_symbols", "code_navigation", "code_diagnostics"] {
        let mut state = ExecutionLoopState::new(&[]);
        state.tool_calls_count = 1;
        state.remember_tool_signature(tool, &serde_json::json!({}), false);

        assert!(!should_attempt_llm_memory_extraction(
            &snapshot(&state),
            "inspect code",
            "Structured result"
        ));
    }
}

#[test]
fn extraction_gate_accepts_read_only_tool_turns_with_memory_language() {
    let mut state = ExecutionLoopState::new(&[]);
    state.tool_calls_count = 1;
    state.remember_tool_signature("grep", &serde_json::json!({"pattern":"memory"}), false);

    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "remember that this repo keeps memory config in HCL",
        "Got it."
    ));
}

#[test]
fn extraction_gate_accepts_tool_failures() {
    let mut state = ExecutionLoopState::new(&[]);
    state.tool_calls_count = 1;
    state.remember_tool_signature("read", &serde_json::json!({"path":"missing"}), true);

    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "read missing file",
        "The read failed."
    ));
}

#[test]
fn extraction_gate_accepts_write_capable_tool_turns() {
    let mut state = ExecutionLoopState::new(&[]);
    state.tool_calls_count = 1;
    state.remember_tool_signature("bash", &serde_json::json!({"cmd":"cargo test"}), false);

    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "run tests",
        "tests passed"
    ));
}

#[test]
fn extraction_gate_accepts_explicit_memory_language() {
    let state = ExecutionLoopState::new(&[]);
    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "remember that this repo prefers HCL config",
        "Got it."
    ));
}

#[test]
fn extraction_gate_accepts_chinese_memory_language() {
    let state = ExecutionLoopState::new(&[]);
    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "请记住：这个项目的记忆系统默认必须启用 LLM 抽取",
        "已记录这个设计约定。"
    ));
}
