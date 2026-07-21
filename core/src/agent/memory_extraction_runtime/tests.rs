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
                {"memory_type":"semantic","content":"A3S asks the LLM to judge every completed non-empty turn for durable memory.","importance":0.8,"tags":["memory"],"source":"project_fact"},
                {"memory_type":"semantic","content":42,"importance":"high"},
                {"memory_type":"procedural","content":"Run memory extraction tests after changing extraction parsing behavior.","importance":0.7,"tags":["tests"],"source":"workflow"}
            ]}"#,
        )
        .unwrap();

    assert_eq!(items.len(), 2);
    assert!(items[0].content.contains("completed non-empty turn"));
    assert!(items[1].content.contains("extraction parsing"));
}

#[test]
fn missing_extracted_content_is_skipped_during_item_conversion() {
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: String::new(),
        importance: Some(0.8),
        confidence: Some(0.9),
        tags: vec!["memory".to_string()],
        source: Some("project_fact".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This project fact affects future memory behavior.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
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
        confidence: Some(0.95),
        tags: vec!["Memory!".to_string(), "Tests".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This repeatable verification prevents storage regressions.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
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
    assert_eq!(item.metadata.get("confidence").unwrap(), "0.95");
    assert_eq!(item.metadata.get("scope").unwrap(), "workspace");
    assert_eq!(
        item.metadata.get("workspace").map(String::as_str),
        Some("optimize memory")
    );
    assert!(item.metadata.get("reason").unwrap().contains("regressions"));
    assert!(!item.metadata.contains_key("prompt"));
}

#[test]
fn extracted_memory_skips_sensitive_content() {
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "The production API key is sk-1234567890abcdef1234567890abcdef.".to_string(),
        importance: Some(0.9),
        confidence: Some(0.95),
        tags: vec!["secret".to_string()],
        source: Some("project_fact".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("Future provider setup would otherwise use this value.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };

    assert!(extracted
        .into_memory_item("remember the key", "sess-1", &HashSet::new())
        .is_none());
}

#[test]
fn extracted_memory_does_not_persist_the_turn_prompt() {
    let extracted = ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Use environment variables when configuring provider credentials.".to_string(),
        importance: Some(0.8),
        confidence: Some(0.9),
        tags: vec!["config".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some(
            "This reusable rule prevents credentials from entering configuration.".to_string(),
        ),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };

    let (item, _, _) = extracted
        .into_memory_item("/workspace", "sess-1", &HashSet::new())
        .unwrap();

    assert!(!item.metadata.contains_key("prompt"));
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
fn extraction_rejects_missing_or_unknown_source() {
    let missing = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "A3S memory uses LLM value judgment after completed turns.".to_string(),
        importance: Some(0.8),
        confidence: Some(0.9),
        tags: vec![],
        source: None,
        scope: Some("workspace".to_string()),
        reason: Some("This behavior controls future memory persistence.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };
    let extracted = ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "A3S memory lets the LLM judge value after completed turns.".to_string(),
        importance: Some(0.8),
        confidence: Some(0.9),
        tags: vec![],
        source: Some("api_key = sk-1234567890abcdef1234567890abcdef".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This behavior controls future memory persistence.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };

    assert!(missing
        .into_memory_item("memory design", "sess-1", &HashSet::new())
        .is_none());
    assert!(extracted
        .into_memory_item("memory design", "sess-1", &HashSet::new())
        .is_none());
}

#[test]
fn extraction_rejects_episodic_turn_history() {
    let extracted = ExtractedMemory {
        memory_type: "episodic".to_string(),
        content: "In this turn, the user asked the assistant to run the memory tests.".to_string(),
        importance: Some(0.9),
        confidence: Some(0.95),
        tags: vec!["history".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This only describes what happened in the current turn.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };

    assert!(extracted
        .into_memory_item("run memory tests", "sess-1", &HashSet::new())
        .is_none());
}

#[test]
fn extraction_rejects_low_importance_items() {
    let extracted = ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Run focused memory tests after changing memory persistence behavior.".to_string(),
        importance: Some(0.2),
        confidence: Some(0.95),
        tags: vec!["memory".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This repeatable check can prevent persistence regressions.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };

    assert!(extracted
        .into_memory_item("memory design", "sess-1", &HashSet::new())
        .is_none());
}

#[test]
fn extraction_requires_confident_scoped_and_justified_llm_judgement() {
    let candidate = || ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Run focused memory tests after changing memory persistence behavior.".to_string(),
        importance: Some(0.85),
        confidence: Some(0.95),
        tags: vec!["memory".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This repeatable check prevents future persistence regressions.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    };

    let mut low_confidence = candidate();
    low_confidence.confidence = Some(0.4);
    assert!(low_confidence
        .into_memory_item("/workspace", "sess-1", &HashSet::new())
        .is_none());

    let mut missing_scope = candidate();
    missing_scope.scope = None;
    assert!(missing_scope
        .into_memory_item("/workspace", "sess-1", &HashSet::new())
        .is_none());

    let mut missing_reason = candidate();
    missing_reason.reason = None;
    assert!(missing_reason
        .into_memory_item("/workspace", "sess-1", &HashSet::new())
        .is_none());
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
        confidence: Some(0.95),
        tags: vec!["memory".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This verification workflow prevents persistence regressions.".to_string()),
        supersedes: vec![allowed_id.clone(), ignored_id],
        conflicts_with: vec![],
        evolution: None,
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
        confidence: Some(0.9),
        tags: vec!["memory".to_string()],
        source: Some("decision".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This decision determines where future sessions persist memory.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![conflict_id.clone(), ignored_id],
        evolution: None,
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
fn extracted_evolution_signal_populates_validated_metadata() {
    let extracted = reusable_skill_memory(Some(ExtractedEvolution {
        kind: "skill".to_string(),
        pattern_key: " Skill / Focused Verification ".to_string(),
        title: "Focused verification".to_string(),
        summary: "Run the smallest relevant checks before broad validation.".to_string(),
        instructions: vec![
            "Identify the smallest relevant test target.".to_string(),
            "Run focused checks before the full workspace suite.".to_string(),
        ],
    }));

    let (item, _, _) = extracted
        .into_memory_item("/workspace", "session-one", &HashSet::new())
        .unwrap();

    assert!(item.tags.contains(&"evolution".to_string()));
    assert!(item.tags.contains(&"evolution-skill".to_string()));
    assert_eq!(
        item.metadata.get("evolution_kind").map(String::as_str),
        Some("skill")
    );
    assert_eq!(
        item.metadata.get("evolution_pattern").map(String::as_str),
        Some("skill.focused.verification")
    );
    let instructions: Vec<String> =
        serde_json::from_str(item.metadata.get("evolution_instructions").unwrap()).unwrap();
    assert_eq!(instructions.len(), 2);
    assert!(instructions[0].contains("smallest relevant test target"));
}

#[test]
fn invalid_or_sensitive_evolution_description_is_not_persisted() {
    let cases = [
        ExtractedEvolution {
            kind: "preference".to_string(),
            pattern_key: "preference.output.concise".to_string(),
            title: "Concise output".to_string(),
            summary: "Keep future responses compact and evidence-backed.".to_string(),
            instructions: vec!["Lead with the result before details.".to_string()],
        },
        ExtractedEvolution {
            kind: "skill".to_string(),
            pattern_key: "single".to_string(),
            title: "Invalid pattern".to_string(),
            summary: "This pattern lacks the required semantic segments.".to_string(),
            instructions: vec!["Run the relevant validation target.".to_string()],
        },
        ExtractedEvolution {
            kind: "skill".to_string(),
            pattern_key: "skill.provider.setup".to_string(),
            title: "Provider setup".to_string(),
            summary: "Configure the provider using the reusable local workflow.".to_string(),
            instructions: vec!["Set api_key=supersecret123 before running the command.".to_string()],
        },
    ];

    for signal in cases {
        let extracted = reusable_skill_memory(Some(signal));
        let (item, _, _) = extracted
            .into_memory_item("/workspace", "session-one", &HashSet::new())
            .unwrap();
        assert!(!item.tags.contains(&"evolution".to_string()));
        assert!(!item.metadata.contains_key("evolution_kind"));
        assert!(!item.metadata.contains_key("evolution_instructions"));
    }
}

fn reusable_skill_memory(evolution: Option<ExtractedEvolution>) -> ExtractedMemory {
    ExtractedMemory {
        memory_type: "procedural".to_string(),
        content: "Run focused checks after changing memory persistence behavior.".to_string(),
        importance: Some(0.9),
        confidence: Some(0.95),
        tags: vec!["memory".to_string(), "tests".to_string()],
        source: Some("workflow".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some(
            "This repeatable workflow prevents future persistence regressions.".to_string(),
        ),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution,
    }
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
fn duplicate_memory_detection_only_handles_exact_normalized_content() {
    assert!(memory_contents_are_duplicates(
        "Run focused memory store tests after changing FileMemoryStore behavior.",
        "  run focused memory store tests after changing FileMemoryStore behavior.  "
    ));
    assert!(!memory_contents_are_duplicates(
        "Run focused memory store regression tests after changing FileMemoryStore behavior.",
        "Run focused memory store tests after changing FileMemoryStore behavior."
    ));
    assert!(!memory_contents_are_duplicates(
        "Run focused memory store tests after changing FileMemoryStore behavior.",
        "Prefer HCL configuration files for repository-level product settings."
    ));
}

#[test]
fn extraction_evaluation_requires_a_completed_turn() {
    let state = ExecutionLoopState::new(&[]);
    assert!(!should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "",
        "hello"
    ));
    assert!(!should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "hello",
        ""
    ));
}

#[test]
fn every_completed_turn_is_sent_to_the_llm_value_judge() {
    let state = ExecutionLoopState::new(&[]);
    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "hi",
        "hello"
    ));
    assert!(should_attempt_llm_memory_extraction(
        &snapshot(&state),
        "请继续",
        "好的"
    ));
}
