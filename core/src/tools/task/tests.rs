use super::*;

#[test]
fn test_task_params_deserialize() {
    let json = r#"{
            "agent": "explore",
            "description": "Find auth code",
            "prompt": "Search for authentication files"
        }"#;

    let params: TaskParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.agent, "explore");
    assert_eq!(params.description, "Find auth code");
    assert!(!params.background);
}

#[test]
fn test_task_params_with_background() {
    let json = r#"{
            "agent": "general",
            "description": "Long task",
            "prompt": "Do something complex",
            "background": true
        }"#;

    let params: TaskParams = serde_json::from_str(json).unwrap();
    assert!(params.background);
}

#[test]
fn test_task_params_with_max_steps() {
    let json = r#"{
            "agent": "plan",
            "description": "Planning task",
            "prompt": "Create a plan",
            "max_steps": 10
        }"#;

    let params: TaskParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.agent, "plan");
    assert_eq!(params.max_steps, Some(10));
    assert!(!params.background);
}

#[test]
fn test_task_params_all_fields() {
    let json = r#"{
            "agent": "general",
            "description": "Complex task",
            "prompt": "Do everything",
            "background": true,
            "max_steps": 20
        }"#;

    let params: TaskParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.agent, "general");
    assert_eq!(params.description, "Complex task");
    assert_eq!(params.prompt, "Do everything");
    assert!(params.background);
    assert_eq!(params.max_steps, Some(20));
}

#[test]
fn test_task_params_missing_required_field() {
    let json = r#"{
            "agent": "explore",
            "description": "Missing prompt"
        }"#;

    let result: Result<TaskParams, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_task_params_serialize() {
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Test task".to_string(),
        prompt: "Test prompt".to_string(),
        background: false,
        max_steps: Some(5),
        output_schema: None,
    };

    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("explore"));
    assert!(json.contains("Test task"));
    assert!(json.contains("Test prompt"));
}

#[test]
fn test_task_params_clone() {
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Test".to_string(),
        prompt: "Prompt".to_string(),
        background: true,
        max_steps: None,
        output_schema: None,
    };

    let cloned = params.clone();
    assert_eq!(params.agent, cloned.agent);
    assert_eq!(params.description, cloned.description);
    assert_eq!(params.background, cloned.background);
}

#[test]
fn test_task_result_serialize() {
    let result = TaskResult {
        output: "Found 5 files".to_string(),
        session_id: "session-123".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-456".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("Found 5 files"));
    assert!(json.contains("explore"));
}

#[test]
fn test_task_result_deserialize() {
    let json = r#"{
            "output": "Task completed",
            "session_id": "sess-789",
            "agent": "general",
            "success": false,
            "task_id": "task-123"
        }"#;

    let result: TaskResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.output, "Task completed");
    assert_eq!(result.session_id, "sess-789");
    assert_eq!(result.agent, "general");
    assert!(!result.success);
    assert_eq!(result.task_id, "task-123");
    assert!(result.source_anchors.is_empty());
}

#[test]
fn test_task_result_clone() {
    let result = TaskResult {
        output: "Output".to_string(),
        session_id: "session-1".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };

    let cloned = result.clone();
    assert_eq!(result.output, cloned.output);
    assert_eq!(result.success, cloned.success);
}

#[test]
fn test_compact_task_output_preserves_small_output() {
    let (output, truncated) = compact_task_output("short result");
    assert_eq!(output, "short result");
    assert!(!truncated);
}

#[test]
fn test_format_task_result_for_context_truncates_large_output() {
    let result = TaskResult {
        output: format!("{}TAIL", "x".repeat(TASK_OUTPUT_CONTEXT_LIMIT + 500)),
        session_id: "session-1".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };

    let (formatted, truncated) = format_task_result_for_context(&result);
    assert!(truncated);
    assert!(formatted.contains("Output excerpt"));
    assert!(formatted.contains("bytes omitted"));
    assert!(formatted.contains("Artifact ID: task-output:task-1"));
    assert!(formatted.contains("Artifact URI: a3s://tasks/session-1/runs/task-1/output"));
    assert!(formatted.contains("TAIL"));
    assert!(formatted.len() < result.output.len());
}

#[test]
fn test_task_artifact_reference_is_stable() {
    let result = TaskResult {
        output: "done".to_string(),
        session_id: "session-1".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };

    assert_eq!(task_artifact_id(&result), "task-output:task-1");
    assert_eq!(
        task_artifact_uri(&result),
        "a3s://tasks/session-1/runs/task-1/output"
    );

    let (formatted, truncated) = format_task_result_for_context(&result);
    assert!(!truncated);
    assert!(formatted.contains("Artifact URI: a3s://tasks/session-1/runs/task-1/output"));
}

#[test]
fn successful_source_tool_metadata_becomes_normalized_anchors() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;
    let read = AgentEvent::ToolEnd {
        id: "read-1".to_string(),
        name: "read".to_string(),
        args: Some(serde_json::json!({"file_path": "./docs/source.md"})),
        output: "contents".to_string(),
        exit_code: 0,
        metadata: Some(serde_json::json!({
            "source_anchors": ["./docs/source.md"]
        })),
        error_kind: None,
    };
    collect_tool_source_anchors(&read, &ctx, &mut anchors, &mut seen, &mut scanned);

    let web = AgentEvent::ToolEnd {
        id: "search-1".to_string(),
        name: "web_search".to_string(),
        args: Some(serde_json::json!({"query": "source"})),
        output: "results".to_string(),
        exit_code: 0,
        metadata: Some(serde_json::json!({
            "source_anchors": ["HTTPS://user:password@Example.COM/report?token=secret#section"]
        })),
        error_kind: None,
    };
    collect_tool_source_anchors(&web, &ctx, &mut anchors, &mut seen, &mut scanned);

    let failed = AgentEvent::ToolEnd {
        id: "fetch-1".to_string(),
        name: "web_fetch".to_string(),
        args: Some(serde_json::json!({"url": "https://example.com/failed"})),
        output: "failed".to_string(),
        exit_code: 1,
        metadata: Some(serde_json::json!({
            "source_anchors": ["https://example.com/failed"]
        })),
        error_kind: None,
    };
    collect_tool_source_anchors(&failed, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert_eq!(
        anchors,
        vec![
            ToolSourceAnchor {
                tool: "read".to_string(),
                url_or_path: "docs/source.md".to_string(),
            },
            ToolSourceAnchor {
                tool: "web_search".to_string(),
                url_or_path: "https://example.com/report".to_string(),
            },
        ]
    );
}

fn source_tool_end(name: &str, exit_code: i32, source_anchors: Vec<String>) -> AgentEvent {
    AgentEvent::ToolEnd {
        id: format!("{name}-source"),
        name: name.to_string(),
        args: None,
        output: String::new(),
        exit_code,
        metadata: Some(serde_json::json!({
            "source_anchors": source_anchors,
        })),
        error_kind: None,
    }
}

#[test]
fn oversized_successful_source_url_is_not_collected() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let oversized_url = format!(
        "https://example.com/{}",
        "a".repeat(MAX_TASK_SOURCE_VALUE_BYTES)
    );
    let event = source_tool_end("web_fetch", 0, vec![oversized_url]);
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;

    collect_tool_source_anchors(&event, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert!(anchors.is_empty());
    assert!(seen.is_empty());
}

#[test]
fn task_json_sanitizer_redacts_nested_string_values() {
    let sanitized = sanitize_task_json(
        &RedactingSourceSecurityProvider,
        &serde_json::json!({
            "verdict": "private",
            "nested": ["safe", {"detail": "private detail"}],
        }),
    );

    assert_eq!(
        sanitized,
        serde_json::json!({
            "verdict": "[redacted]",
            "nested": ["safe", {"detail": "[redacted] detail"}],
        })
    );
}

#[test]
fn source_anchor_count_cap_is_shared_across_tool_events() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let first_urls = (0..MAX_TASK_SOURCE_ANCHORS - 1)
        .map(|index| format!("https://example.com/source-{index}"))
        .collect();
    let first_event = source_tool_end("web_search", 0, first_urls);
    let last_url = "https://example.com/last".to_string();
    let overflow_url = "https://example.com/overflow".to_string();
    let second_event =
        source_tool_end("web_fetch", 0, vec![last_url.clone(), overflow_url.clone()]);
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;

    collect_tool_source_anchors(&first_event, &ctx, &mut anchors, &mut seen, &mut scanned);
    collect_tool_source_anchors(&second_event, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert_eq!(anchors.len(), MAX_TASK_SOURCE_ANCHORS);
    assert_eq!(anchors[0].url_or_path, "https://example.com/source-0");
    assert_eq!(anchors[MAX_TASK_SOURCE_ANCHORS - 1].url_or_path, last_url);
    assert!(!seen.contains(&ToolSourceAnchor {
        tool: "web_fetch".to_string(),
        url_or_path: overflow_url,
    }));
}

#[test]
fn duplicate_source_anchors_do_not_hide_a_later_valid_anchor() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let duplicate = "https://example.com/duplicate".to_string();
    let later = "https://example.com/later".to_string();
    let mut urls = vec![duplicate.clone(); MAX_TASK_SOURCE_ANCHORS + 1];
    urls.push(later.clone());
    let event = source_tool_end("web_search", 0, urls);
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;

    collect_tool_source_anchors(&event, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert_eq!(
        anchors,
        vec![
            ToolSourceAnchor {
                tool: "web_search".to_string(),
                url_or_path: duplicate,
            },
            ToolSourceAnchor {
                tool: "web_search".to_string(),
                url_or_path: later,
            },
        ]
    );
}

#[test]
fn source_anchor_candidate_scan_stops_an_invalid_value_flood() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let mut values = vec!["not-a-url".to_string(); MAX_TASK_SOURCE_CANDIDATES];
    values.push("https://example.com/after-limit".to_string());
    let event = source_tool_end("web_fetch", 0, values);
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;

    collect_tool_source_anchors(&event, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert_eq!(scanned, MAX_TASK_SOURCE_CANDIDATES);
    assert!(anchors.is_empty());
    assert!(seen.is_empty());
}

#[test]
fn source_anchor_candidate_scan_stops_a_duplicate_flood() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let duplicate = "https://example.com/duplicate".to_string();
    let mut values = vec![duplicate.clone(); MAX_TASK_SOURCE_CANDIDATES];
    values.push("https://example.com/after-limit".to_string());
    let event = source_tool_end("web_search", 0, values);
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;

    collect_tool_source_anchors(&event, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert_eq!(scanned, MAX_TASK_SOURCE_CANDIDATES);
    assert_eq!(
        anchors,
        vec![ToolSourceAnchor {
            tool: "web_search".to_string(),
            url_or_path: duplicate,
        }]
    );
}

#[test]
fn parallel_result_source_anchor_budget_prioritizes_successful_results() {
    let result = |task_id: &str, success: bool, count: usize| TaskResult {
        output: String::new(),
        session_id: format!("session-{task_id}"),
        agent: "worker".to_string(),
        success,
        task_id: task_id.to_string(),
        structured: None,
        source_anchors: (0..count)
            .map(|index| ToolSourceAnchor {
                tool: "read".to_string(),
                url_or_path: format!("{task_id}-source-{index}.md"),
            })
            .collect(),
    };
    let successful_count = 8;
    let results = vec![
        result("failed-first", false, MAX_TASK_SOURCE_ANCHORS),
        result("successful-later", true, successful_count),
    ];

    let counts = parallel_source_anchor_counts(&results);

    assert_eq!(counts[1], successful_count);
    assert_eq!(
        counts[0],
        MAX_PARALLEL_TASK_SOURCE_ANCHORS - successful_count
    );
    assert_eq!(
        counts.iter().sum::<usize>(),
        MAX_PARALLEL_TASK_SOURCE_ANCHORS
    );
}

#[test]
fn failed_source_tool_exit_remains_excluded() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let event = source_tool_end(
        "web_fetch",
        1,
        vec!["https://example.com/failed".to_string()],
    );
    let mut anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;

    collect_tool_source_anchors(&event, &ctx, &mut anchors, &mut seen, &mut scanned);

    assert!(anchors.is_empty());
    assert!(seen.is_empty());
}

#[test]
fn test_task_params_schema() {
    let schema = task_params_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["agent"].is_object());
    assert!(schema["properties"]["prompt"].is_object());
}

#[test]
fn test_task_params_schema_required_fields() {
    let schema = task_params_schema();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("agent")));
    assert!(required.contains(&serde_json::json!("description")));
    assert!(required.contains(&serde_json::json!("prompt")));
}

#[test]
fn test_task_params_schema_properties() {
    let schema = task_params_schema();
    let props = &schema["properties"];

    assert_eq!(props["agent"]["type"], "string");
    assert_eq!(props["description"]["type"], "string");
    assert_eq!(props["prompt"]["type"], "string");
    assert_eq!(props["background"]["type"], "boolean");
    assert_eq!(props["background"]["default"], false);
    assert_eq!(props["max_steps"]["type"], "integer");
    assert_eq!(props["output_schema"]["type"], "object");
}

#[test]
fn test_task_params_schema_descriptions() {
    let schema = task_params_schema();
    let props = &schema["properties"];

    assert!(props["agent"]["description"].is_string());
    assert!(props["description"]["description"].is_string());
    assert!(props["prompt"]["description"].is_string());
    assert!(props["background"]["description"].is_string());
    assert!(props["max_steps"]["description"].is_string());
    assert!(props["output_schema"]["description"].is_string());
}

#[test]
fn test_task_params_default_background() {
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Test".to_string(),
        prompt: "Test prompt".to_string(),
        background: false,
        max_steps: None,
        output_schema: None,
    };
    assert!(!params.background);
}

#[test]
fn test_task_params_serialize_skip_none() {
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Test".to_string(),
        prompt: "Test prompt".to_string(),
        background: false,
        max_steps: None,
        output_schema: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    // max_steps should not appear when None
    assert!(!json.contains("max_steps"));
}

#[test]
fn test_task_params_serialize_with_max_steps() {
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Test".to_string(),
        prompt: "Test prompt".to_string(),
        background: false,
        max_steps: Some(15),
        output_schema: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("max_steps"));
    assert!(json.contains("15"));
}

#[test]
fn test_task_result_success_true() {
    let result = TaskResult {
        output: "Success".to_string(),
        session_id: "sess-1".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };
    assert!(result.success);
}

#[test]
fn test_task_result_success_false() {
    let result = TaskResult {
        output: "Failed".to_string(),
        session_id: "sess-1".to_string(),
        agent: "explore".to_string(),
        success: false,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };
    assert!(!result.success);
}

#[test]
fn test_task_params_empty_strings() {
    let params = TaskParams {
        agent: "".to_string(),
        description: "".to_string(),
        prompt: "".to_string(),
        background: false,
        max_steps: None,
        output_schema: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    let deserialized: TaskParams = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.agent, "");
    assert_eq!(deserialized.description, "");
    assert_eq!(deserialized.prompt, "");
}

#[test]
fn test_task_result_empty_output() {
    let result = TaskResult {
        output: "".to_string(),
        session_id: "sess-1".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };
    assert_eq!(result.output, "");
}

#[test]
fn test_task_params_debug_format() {
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Test".to_string(),
        prompt: "Test prompt".to_string(),
        background: false,
        max_steps: None,
        output_schema: None,
    };
    let debug_str = format!("{:?}", params);
    assert!(debug_str.contains("explore"));
    assert!(debug_str.contains("Test"));
}

#[test]
fn test_task_result_debug_format() {
    let result = TaskResult {
        output: "Output".to_string(),
        session_id: "sess-1".to_string(),
        agent: "explore".to_string(),
        success: true,
        task_id: "task-1".to_string(),
        structured: None,
        source_anchors: Vec::new(),
    };
    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("Output"));
    assert!(debug_str.contains("explore"));
}

#[test]
fn test_task_params_roundtrip() {
    let original = TaskParams {
        agent: "general".to_string(),
        description: "Roundtrip test".to_string(),
        prompt: "Test roundtrip serialization".to_string(),
        background: true,
        max_steps: Some(42),
        output_schema: None,
    };
    let json = serde_json::to_string(&original).unwrap();
    let deserialized: TaskParams = serde_json::from_str(&json).unwrap();
    assert_eq!(original.agent, deserialized.agent);
    assert_eq!(original.description, deserialized.description);
    assert_eq!(original.prompt, deserialized.prompt);
    assert_eq!(original.background, deserialized.background);
    assert_eq!(original.max_steps, deserialized.max_steps);
}

#[test]
fn test_task_result_roundtrip() {
    let original = TaskResult {
        output: "Roundtrip output".to_string(),
        session_id: "sess-roundtrip".to_string(),
        agent: "plan".to_string(),
        success: false,
        task_id: "task-roundtrip".to_string(),
        structured: None,
        source_anchors: vec![ToolSourceAnchor {
            tool: "read".to_string(),
            url_or_path: "docs/source.md".to_string(),
        }],
    };
    let json = serde_json::to_string(&original).unwrap();
    let deserialized: TaskResult = serde_json::from_str(&json).unwrap();
    assert_eq!(original.output, deserialized.output);
    assert_eq!(original.session_id, deserialized.session_id);
    assert_eq!(original.agent, deserialized.agent);
    assert_eq!(original.success, deserialized.success);
    assert_eq!(original.task_id, deserialized.task_id);
    assert_eq!(original.source_anchors, deserialized.source_anchors);
}

#[test]
fn test_parallel_task_params_deserialize() {
    let json = r#"{
            "tasks": [
                { "agent": "explore", "description": "Find auth", "prompt": "Search auth files" },
                { "agent": "general", "description": "Fix bug", "prompt": "Fix the login bug" }
            ]
        }"#;

    let params: ParallelTaskParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.tasks.len(), 2);
    assert_eq!(params.tasks[0].agent, "explore");
    assert_eq!(params.tasks[1].agent, "general");
}

#[test]
fn test_parallel_task_params_single_task() {
    let json = r#"{
            "tasks": [
                { "agent": "plan", "description": "Plan work", "prompt": "Create a plan" }
            ]
        }"#;

    let params: ParallelTaskParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.tasks.len(), 1);
}

#[test]
fn test_parallel_task_params_empty_tasks() {
    let json = r#"{ "tasks": [] }"#;
    let params: ParallelTaskParams = serde_json::from_str(json).unwrap();
    assert!(params.tasks.is_empty());
    assert!(!params.allow_partial_failure);
}

#[test]
fn test_parallel_task_params_missing_tasks() {
    let json = r#"{}"#;
    let result: Result<ParallelTaskParams, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_parallel_task_params_serialize() {
    let params = ParallelTaskParams {
        tasks: vec![
            TaskParams {
                agent: "explore".to_string(),
                description: "Task 1".to_string(),
                prompt: "Prompt 1".to_string(),
                background: false,
                max_steps: None,
                output_schema: None,
            },
            TaskParams {
                agent: "general".to_string(),
                description: "Task 2".to_string(),
                prompt: "Prompt 2".to_string(),
                background: false,
                max_steps: Some(10),
                output_schema: None,
            },
        ],
        allow_partial_failure: false,
        timeout_ms: None,
        min_success_count: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("explore"));
    assert!(json.contains("general"));
    assert!(json.contains("Prompt 1"));
    assert!(json.contains("Prompt 2"));
}

#[test]
fn test_parallel_task_params_roundtrip() {
    let original = ParallelTaskParams {
        tasks: vec![
            TaskParams {
                agent: "explore".to_string(),
                description: "Explore".to_string(),
                prompt: "Find files".to_string(),
                background: false,
                max_steps: None,
                output_schema: None,
            },
            TaskParams {
                agent: "plan".to_string(),
                description: "Plan".to_string(),
                prompt: "Make plan".to_string(),
                background: false,
                max_steps: Some(5),
                output_schema: None,
            },
        ],
        allow_partial_failure: true,
        timeout_ms: None,
        min_success_count: None,
    };
    let json = serde_json::to_string(&original).unwrap();
    let deserialized: ParallelTaskParams = serde_json::from_str(&json).unwrap();
    assert_eq!(original.tasks.len(), deserialized.tasks.len());
    assert_eq!(original.tasks[0].agent, deserialized.tasks[0].agent);
    assert_eq!(original.tasks[1].agent, deserialized.tasks[1].agent);
    assert_eq!(original.tasks[1].max_steps, deserialized.tasks[1].max_steps);
    assert_eq!(
        original.allow_partial_failure,
        deserialized.allow_partial_failure
    );
}

#[test]
fn test_parallel_task_params_clone() {
    let params = ParallelTaskParams {
        tasks: vec![TaskParams {
            agent: "explore".to_string(),
            description: "Test".to_string(),
            prompt: "Prompt".to_string(),
            background: false,
            max_steps: None,
            output_schema: None,
        }],
        allow_partial_failure: false,
        timeout_ms: None,
        min_success_count: None,
    };
    let cloned = params.clone();
    assert_eq!(params.tasks.len(), cloned.tasks.len());
    assert_eq!(params.tasks[0].agent, cloned.tasks[0].agent);
}

#[test]
fn test_parallel_task_params_schema() {
    let schema = parallel_task_params_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"]["tasks"].is_object());
    assert_eq!(schema["properties"]["tasks"]["type"], "array");
    assert_eq!(
        schema["properties"]["tasks"]["maxItems"],
        MAX_PARALLEL_TASKS_PER_CALL
    );
    assert_eq!(schema["properties"]["tasks"]["minItems"], 2);
    assert_eq!(
        schema["properties"]["allow_partial_failure"]["type"],
        "boolean"
    );
    assert_eq!(
        schema["properties"]["allow_partial_failure"]["default"],
        false
    );
    assert_eq!(schema["properties"]["timeout_ms"]["type"], "integer");
    assert_eq!(schema["properties"]["min_success_count"]["type"], "integer");
}

#[test]
fn test_parallel_task_params_schema_required() {
    let schema = parallel_task_params_schema();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("tasks")));
}

#[test]
fn test_parallel_task_params_schema_items() {
    let schema = parallel_task_params_schema();
    let items = &schema["properties"]["tasks"]["items"];
    assert_eq!(items["type"], "object");
    assert_eq!(items["additionalProperties"], false);
    let item_required = items["required"].as_array().unwrap();
    assert!(item_required.contains(&serde_json::json!("agent")));
    assert!(item_required.contains(&serde_json::json!("description")));
    assert!(item_required.contains(&serde_json::json!("prompt")));
    assert!(items["properties"].get("background").is_none());
    assert_eq!(items["properties"]["max_steps"]["type"], "integer");
    assert_eq!(items["properties"]["output_schema"]["type"], "object");
}

#[test]
fn test_task_schema_examples_use_delegation_core() {
    let task = task_params_schema();
    let task_examples = task["examples"].as_array().unwrap();
    assert_eq!(task_examples[0]["agent"], "explore");
    assert!(task_examples[0].get("task").is_none());

    let parallel = parallel_task_params_schema();
    let parallel_examples = parallel["examples"].as_array().unwrap();
    assert!(!parallel_examples[0]["tasks"].as_array().unwrap().is_empty());
}

#[test]
fn test_parallel_task_params_debug() {
    let params = ParallelTaskParams {
        tasks: vec![TaskParams {
            agent: "explore".to_string(),
            description: "Debug test".to_string(),
            prompt: "Test".to_string(),
            background: false,
            max_steps: None,
            output_schema: None,
        }],
        allow_partial_failure: false,
        timeout_ms: None,
        min_success_count: None,
    };
    let debug_str = format!("{:?}", params);
    assert!(debug_str.contains("explore"));
    assert!(debug_str.contains("Debug test"));
}

#[test]
fn test_parallel_task_params_large_count() {
    let tasks: Vec<TaskParams> = (0..MAX_PARALLEL_TASKS_PER_CALL)
        .map(|i| TaskParams {
            agent: "explore".to_string(),
            description: format!("Task {}", i),
            prompt: format!("Prompt for task {}", i),
            background: false,
            max_steps: Some(10),
            output_schema: None,
        })
        .collect();

    let params = ParallelTaskParams {
        tasks,
        allow_partial_failure: false,
        timeout_ms: None,
        min_success_count: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    let deserialized: ParallelTaskParams = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.tasks.len(), MAX_PARALLEL_TASKS_PER_CALL);
    assert_eq!(deserialized.tasks[0].description, "Task 0");
    assert_eq!(
        deserialized.tasks[MAX_PARALLEL_TASKS_PER_CALL - 1].description,
        "Task 31"
    );
}

#[test]
fn test_task_params_max_steps_zero() {
    // max_steps = 0 is a valid edge case (callers decide enforcement)
    let params = TaskParams {
        agent: "explore".to_string(),
        description: "Edge case".to_string(),
        prompt: "Zero steps".to_string(),
        background: false,
        max_steps: Some(0),
        output_schema: None,
    };
    let json = serde_json::to_string(&params).unwrap();
    let deserialized: TaskParams = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.max_steps, Some(0));
}

#[test]
fn test_parallel_task_params_all_background() {
    let tasks: Vec<TaskParams> = (0..5)
        .map(|i| TaskParams {
            agent: "general".to_string(),
            description: format!("BG task {}", i),
            prompt: "Run in background".to_string(),
            background: true,
            max_steps: None,
            output_schema: None,
        })
        .collect();
    let params = ParallelTaskParams {
        tasks,
        allow_partial_failure: false,
        timeout_ms: None,
        min_success_count: None,
    };
    for task in &params.tasks {
        assert!(task.background);
    }
}

#[test]
fn test_task_params_rejects_permissive_field() {
    let json = r#"{
            "agent": "general",
            "description": "Legacy field rejection",
            "prompt": "Verify legacy fields are rejected",
            "permissive": true
        }"#;

    let result: Result<TaskParams, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_task_params_schema_hides_permissive_field() {
    let schema = task_params_schema();
    let props = &schema["properties"];

    assert!(props.get("permissive").is_none());
}

// ========================================================================
// Contract tests — verify task delegation with MockLlmClient (no network)
// ========================================================================

use crate::agent::tests::MockLlmClient;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{ContentBlock, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use crate::permissions::{PermissionChecker, PermissionDecision, PermissionPolicy};
use crate::subagent::AgentRegistry;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, Barrier, Notify};

fn text_response(text: impl Into<String>) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text { text: text.into() }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn pre_analysis_response(messages: &[Message]) -> LlmResponse {
    let prompt = last_text(messages);
    let response = serde_json::json!({
        "intent": "GeneralPurpose",
        "requires_planning": false,
        "goal": {
            "description": prompt,
            "success_criteria": []
        },
        "execution_plan": {
            "complexity": "Simple",
            "steps": [{
                "id": "step-1",
                "description": prompt,
                "dependencies": [],
                "success_criteria": "Complete the request"
            }],
            "required_tools": []
        },
        "optimized_input": prompt
    });
    text_response(response.to_string())
}

fn is_pre_analysis_system(system: Option<&str>) -> bool {
    system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM))
}

fn last_text(messages: &[Message]) -> String {
    messages
        .last()
        .and_then(|message| {
            message.content.iter().find_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.clone())
                } else {
                    None
                }
            })
        })
        .unwrap_or_default()
}

/// Client for the schema-coercion tests. The agent's own turn returns
/// plain text (which ends the loop); the structured-output coercion call
/// — recognizable by the injected `step_output` tool — returns a tool call
/// carrying the object.
struct SchemaCoercionClient;

#[async_trait::async_trait]
impl LlmClient for SchemaCoercionClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }
        // The structured-output coercion injects a synthetic tool named
        // `emit_<schema_name>` (here `emit_step_output`).
        if tools.iter().any(|t| t.name == "emit_step_output") {
            return Ok(MockLlmClient::tool_call_response(
                "coerce-1",
                "emit_step_output",
                serde_json::json!({ "verdict": "ok" }),
            ));
        }
        Ok(text_response("The verdict is ok."))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by schema coercion tests")
    }

    fn native_structured_support(&self) -> crate::llm::structured::NativeStructuredSupport {
        crate::llm::structured::NativeStructuredSupport::ForcedTool
    }
}

struct SchemaFastPathClient {
    output: &'static str,
    coercion_calls: Arc<AtomicUsize>,
}

struct DelegatedNoPreAnalysisClient {
    pre_analysis_calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl LlmClient for DelegatedNoPreAnalysisClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            self.pre_analysis_calls.fetch_add(1, Ordering::SeqCst);
            return Ok(pre_analysis_response(messages));
        }
        Ok(text_response(r#"{"verdict":"planned-by-parent"}"#))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming unused")
    }
}

#[async_trait::async_trait]
impl LlmClient for SchemaFastPathClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }
        if tools.iter().any(|tool| tool.name == "emit_step_output") {
            self.coercion_calls.fetch_add(1, Ordering::SeqCst);
            return Ok(MockLlmClient::tool_call_response(
                "coerce-fast-path-fallback",
                "emit_step_output",
                serde_json::json!({ "verdict": "repaired" }),
            ));
        }
        Ok(text_response(self.output))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming unused")
    }

    fn native_structured_support(&self) -> crate::llm::structured::NativeStructuredSupport {
        crate::llm::structured::NativeStructuredSupport::ForcedTool
    }
}

struct CountingSchemaCoercionClient {
    attempts: Arc<AtomicUsize>,
    successful_calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl LlmClient for CountingSchemaCoercionClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        self.successful_calls.fetch_add(1, Ordering::SeqCst);
        SchemaCoercionClient.complete(messages, system, tools).await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("streaming is not used by schema coercion tests")
    }

    fn native_structured_support(&self) -> crate::llm::structured::NativeStructuredSupport {
        crate::llm::structured::NativeStructuredSupport::ForcedTool
    }
}

#[derive(Default)]
struct CountingAllowBudgetGuard {
    checks: AtomicUsize,
    records: AtomicUsize,
    sessions: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl BudgetGuard for CountingAllowBudgetGuard {
    async fn check_before_llm(
        &self,
        session_id: &str,
        _estimated_prompt_tokens: usize,
    ) -> BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.sessions.lock().unwrap().push(session_id.to_string());
        BudgetDecision::Allow
    }

    async fn record_after_llm(&self, _session_id: &str, _usage: &TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
    }
}

fn child_context_with_budget(
    budget_guard: Arc<dyn BudgetGuard>,
) -> crate::child_run::ChildRunContext {
    crate::child_run::ChildRunContext {
        security_provider: None,
        hook_engine: None,
        skill_registry: None,
        permission_checker: None,
        permission_policy: None,
        tool_timeout_ms: None,
        llm_api_timeout_ms: None,
        max_parallel_tasks: None,
        max_execution_time_ms: None,
        circuit_breaker_threshold: None,
        duplicate_tool_call_threshold: None,
        confirmation_manager: None,
        enforce_active_skill_tool_restrictions: None,
        workspace_services: None,
        sandbox_handle: None,
        budget_guard: Some(budget_guard),
    }
}

fn verdict_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "verdict": { "type": "string" } },
        "required": ["verdict"]
    })
}

#[tokio::test]
async fn execute_step_with_schema_coerces_structured_output() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaCoercionClient),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("step-1", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "step should succeed: {}", outcome.output);
    assert_eq!(
        outcome.structured,
        Some(serde_json::json!({ "verdict": "ok" })),
        "a schema'd step returns the validated object in `structured`"
    );
}

#[tokio::test]
async fn schema_valid_child_json_skips_llm_coercion() {
    let workspace = tempfile::tempdir().unwrap();
    let coercion_calls = Arc::new(AtomicUsize::new(0));
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaFastPathClient {
            output: r#"{"verdict":"local"}"#,
            coercion_calls: Arc::clone(&coercion_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("local-json", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "step should succeed: {}", outcome.output);
    assert_eq!(
        outcome.structured,
        Some(serde_json::json!({ "verdict": "local" }))
    );
    assert_eq!(coercion_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn schema_valid_json_embedded_in_dirty_output_skips_llm_coercion() {
    let workspace = tempfile::tempdir().unwrap();
    let coercion_calls = Arc::new(AtomicUsize::new(0));
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaFastPathClient {
            output: "Completed.\n```json\n{\"verdict\":\"local-fenced\"}\n```\n",
            coercion_calls: Arc::clone(&coercion_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("dirty-json", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "step should succeed: {}", outcome.output);
    assert_eq!(
        outcome.structured,
        Some(serde_json::json!({ "verdict": "local-fenced" }))
    );
    assert_eq!(coercion_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn schema_invalid_child_output_still_uses_llm_coercion_fallback() {
    let workspace = tempfile::tempdir().unwrap();
    let coercion_calls = Arc::new(AtomicUsize::new(0));
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaFastPathClient {
            output: "The result needs repair.",
            coercion_calls: Arc::clone(&coercion_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("repair-json", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(
        outcome.success,
        "fallback should succeed: {}",
        outcome.output
    );
    assert_eq!(
        outcome.structured,
        Some(serde_json::json!({ "verdict": "repaired" }))
    );
    assert_eq!(coercion_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn empty_child_output_is_a_failure_and_never_triggers_schema_coercion() {
    let workspace = tempfile::tempdir().unwrap();
    let coercion_calls = Arc::new(AtomicUsize::new(0));
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaFastPathClient {
            output: "",
            coercion_calls: Arc::clone(&coercion_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("empty-json", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(!outcome.success, "unexpected outcome: {outcome:#?}");
    assert_eq!(outcome.structured, None);
    assert!(outcome.output.contains("no progress was being made"));
    assert_eq!(coercion_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn delegated_child_skips_redundant_pre_analysis_model_call() {
    let workspace = tempfile::tempdir().unwrap();
    let pre_analysis_calls = Arc::new(AtomicUsize::new(0));
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(DelegatedNoPreAnalysisClient {
            pre_analysis_calls: Arc::clone(&pre_analysis_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("preplanned", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "unexpected outcome: {outcome:#?}");
    assert_eq!(
        outcome.structured,
        Some(serde_json::json!({ "verdict": "planned-by-parent" }))
    );
    assert_eq!(pre_analysis_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tool_free_schema_step_uses_one_structured_model_call() {
    let workspace = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let successful_calls = Arc::new(AtomicUsize::new(0));
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(CountingSchemaCoercionClient {
            attempts: Arc::clone(&attempts),
            successful_calls: Arc::clone(&successful_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new(
        "loop-plan",
        "loop-planner",
        "plan",
        "Plan the bounded loop.",
    )
    .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "step should succeed: {}", outcome.output);
    assert_eq!(
        outcome.structured,
        Some(serde_json::json!({ "verdict": "ok" }))
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(successful_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn task_child_and_schema_coercion_share_the_scoped_budget_boundary() {
    let workspace = tempfile::tempdir().unwrap();
    let provider_attempts = Arc::new(AtomicUsize::new(0));
    let successful_provider_calls = Arc::new(AtomicUsize::new(0));
    let guard = Arc::new(CountingAllowBudgetGuard::default());
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(CountingSchemaCoercionClient {
            attempts: Arc::clone(&provider_attempts),
            successful_calls: Arc::clone(&successful_provider_calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    )
    .with_parent_context(child_context_with_budget(
        Arc::clone(&guard) as Arc<dyn BudgetGuard>
    ));
    let spec = AgentStepSpec::new("budgeted-schema", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "step should succeed: {}", outcome.output);
    let attempts = provider_attempts.load(Ordering::SeqCst);
    let successful_calls = successful_provider_calls.load(Ordering::SeqCst);
    assert!(
        successful_calls >= 2,
        "child turn and coercion must both call the provider"
    );
    assert_eq!(
            guard.checks.load(Ordering::SeqCst),
            attempts,
            "every provider attempt, including a failed streaming attempt before fallback, must be gated"
        );
    assert_eq!(
        guard.records.load(Ordering::SeqCst),
        successful_calls,
        "only successful provider calls report usage"
    );
    assert!(guard
        .sessions
        .lock()
        .unwrap()
        .iter()
        .all(|session| session == "task-run-budgeted-schema"));
}

#[tokio::test]
async fn execute_step_without_schema_has_no_structured_output() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaCoercionClient),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("step-2", "general", "assess", "Assess the thing.");

    let outcome = executor.execute_step(spec, None).await;

    assert!(outcome.success, "step should succeed: {}", outcome.output);
    assert_eq!(
        outcome.structured, None,
        "no schema requested → no structured output, no coercion call"
    );
}

/// The agent's turn returns text; the coercion call (`emit_step_output`)
/// always returns an object that VIOLATES the schema, so `generate_blocking`
/// exhausts its repairs and bails.
struct SchemaFailClient;

#[async_trait::async_trait]
impl LlmClient for SchemaFailClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }
        if tools.iter().any(|t| t.name == "emit_step_output") {
            // `{}` is missing the required `verdict` field → schema invalid.
            return Ok(MockLlmClient::tool_call_response(
                "coerce-fail",
                "emit_step_output",
                serde_json::json!({}),
            ));
        }
        Ok(text_response("some answer"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming unused")
    }
}

#[tokio::test]
async fn execute_step_with_schema_demotes_step_on_coercion_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaFailClient),
        workspace.path().to_string_lossy().to_string(),
    );
    let spec = AgentStepSpec::new("step-x", "general", "assess", "Assess the thing.")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(
        !outcome.success,
        "a step whose output can't satisfy the schema is demoted to failure"
    );
    assert_eq!(outcome.structured, None, "no validated object on failure");
    assert!(
        outcome.output.contains("[structured output failed"),
        "the demotion marker is appended: {}",
        outcome.output
    );
}

#[tokio::test]
async fn parallel_isolates_schema_coercion_failure_from_sibling() {
    let workspace = tempfile::tempdir().unwrap();
    let executor: Arc<dyn AgentExecutor> = Arc::new(TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaFailClient),
        workspace.path().to_string_lossy().to_string(),
    ));
    // A plain step (no schema → succeeds) alongside a schema'd step whose
    // coercion fails. The failure must not drop or fail the sibling.
    let specs = vec![
        AgentStepSpec::new("plain", "general", "d", "p"),
        AgentStepSpec::new("schemad", "general", "d", "p").with_output_schema(verdict_schema()),
    ];
    let out = crate::orchestration::execute_steps_parallel(executor, specs, None).await;

    assert_eq!(out.len(), 2);
    assert_eq!(out[0].task_id, "plain");
    assert!(out[0].success, "no-schema sibling unaffected");
    assert_eq!(out[0].structured, None);
    assert_eq!(out[1].task_id, "schemad");
    assert!(!out[1].success, "schema-failing step surfaces as failure");
    assert_eq!(out[1].structured, None);
    assert!(out[1].output.contains("[structured output failed"));
}

#[tokio::test]
async fn failed_step_with_schema_skips_coercion() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        Arc::new(SchemaCoercionClient),
        workspace.path().to_string_lossy().to_string(),
    );
    // Unknown agent → the run fails BEFORE coercion. The failure is the
    // run error, not a coercion failure — coercion must not run.
    let spec = AgentStepSpec::new("step-y", "no-such-agent", "d", "p")
        .with_output_schema(verdict_schema());

    let outcome = executor.execute_step(spec, None).await;

    assert!(!outcome.success);
    assert_eq!(outcome.structured, None);
    assert!(
        !outcome.output.contains("[structured output failed"),
        "coercion never ran — failure is the run error, not a coercion failure: {}",
        outcome.output
    );
}

struct StaticLlmClient {
    text: String,
}

impl StaticLlmClient {
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[async_trait::async_trait]
impl LlmClient for StaticLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }
        Ok(text_response(self.text.clone()))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by task executor tests")
    }
}

struct ConcurrentLlmClient {
    barrier: Arc<Barrier>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl ConcurrentLlmClient {
    fn new(task_count: usize) -> Self {
        Self {
            barrier: Arc::new(Barrier::new(task_count)),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    fn record_active(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let mut observed = self.max_active.load(Ordering::SeqCst);
        while active > observed {
            match self.max_active.compare_exchange(
                observed,
                active,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
    }
}

struct LimitedConcurrencyLlmClient {
    active: AtomicUsize,
    max_active: AtomicUsize,
    calls: AtomicUsize,
}

struct TransientThenSuccessLlmClient {
    calls: AtomicUsize,
    failures_before_success: usize,
}

struct SelectiveTransientLlmClient {
    attempts: Mutex<HashMap<&'static str, usize>>,
}

struct BlockingCapacityLlmClient {
    started: Notify,
    release: Notify,
    calls: AtomicUsize,
}

impl TransientThenSuccessLlmClient {
    fn new(failures_before_success: usize) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            failures_before_success,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl LimitedConcurrencyLlmClient {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }
    }

    fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn record_active(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
    }
}

impl SelectiveTransientLlmClient {
    fn new() -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
        }
    }

    fn attempts_for(&self, branch: &'static str) -> usize {
        self.attempts
            .lock()
            .unwrap()
            .get(branch)
            .copied()
            .unwrap_or_default()
    }
}

impl BlockingCapacityLlmClient {
    fn new() -> Self {
        Self {
            started: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LlmClient for LimitedConcurrencyLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }

        let prompt = last_text(messages);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.record_active();
        tokio::time::sleep(Duration::from_millis(40)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(text_response(format!("completed: {prompt}")))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by task executor tests")
    }
}

#[async_trait::async_trait]
impl LlmClient for SelectiveTransientLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }

        let prompt = last_text(messages);
        let branch = if prompt.contains("flaky branch") {
            "flaky"
        } else {
            "stable"
        };
        let attempt = {
            let mut attempts = self.attempts.lock().unwrap();
            let attempt = attempts.entry(branch).or_default();
            *attempt += 1;
            *attempt
        };
        if branch == "flaky" && attempt <= 2 {
            anyhow::bail!("provider overloaded: status 529; retry later");
        }
        Ok(text_response(format!("completed: {prompt}")))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("provider overloaded: status 529; retry later")
    }
}

#[async_trait::async_trait]
impl LlmClient for BlockingCapacityLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }

        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        self.release.notified().await;
        Ok(text_response(format!("completed: {}", last_text(messages))))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by capacity cancellation tests")
    }
}

#[async_trait::async_trait]
impl LlmClient for TransientThenSuccessLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }

        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call < self.failures_before_success {
            anyhow::bail!("provider overloaded: status 529; retry later");
        }
        Ok(text_response(format!("recovered: {}", last_text(messages))))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("provider overloaded: status 529; retry later")
    }
}

#[async_trait::async_trait]
impl LlmClient for ConcurrentLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if is_pre_analysis_system(system) {
            return Ok(pre_analysis_response(messages));
        }

        let prompt = last_text(messages);
        self.record_active();
        self.barrier.wait().await;
        if prompt.contains("slow") {
            tokio::time::sleep(Duration::from_millis(1_000)).await;
        } else {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(text_response(format!("completed: {prompt}")))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by task executor tests")
    }
}

fn test_registry_with_writer() -> Arc<AgentRegistry> {
    let registry = AgentRegistry::new();
    let spec = crate::subagent::WorkerAgentSpec::custom("writer", "Write files")
        .with_permissions(PermissionPolicy::new().allow("write(*)").allow("read(*)"))
        .with_prompt("Write files when asked.")
        .with_max_steps(3);
    registry.register(spec.into_agent_definition());
    Arc::new(registry)
}

fn test_registry_with_text_worker() -> Arc<AgentRegistry> {
    let registry = AgentRegistry::new();
    let spec = crate::subagent::WorkerAgentSpec::custom("worker", "Text worker")
        .with_prompt("Return a concise result.")
        .with_max_steps(1);
    registry.register(spec.into_agent_definition());
    Arc::new(registry)
}

struct RedactingSourceSecurityProvider;

impl crate::security::SecurityProvider for RedactingSourceSecurityProvider {
    fn sanitize_output(&self, text: &str) -> String {
        text.replace("private", "[redacted]")
    }
}

fn redacting_parent_context() -> crate::child_run::ChildRunContext {
    crate::child_run::ChildRunContext {
        security_provider: Some(Arc::new(RedactingSourceSecurityProvider)),
        hook_engine: None,
        skill_registry: None,
        permission_checker: None,
        permission_policy: None,
        tool_timeout_ms: None,
        llm_api_timeout_ms: None,
        max_parallel_tasks: None,
        max_execution_time_ms: None,
        circuit_breaker_threshold: None,
        duplicate_tool_call_threshold: None,
        confirmation_manager: None,
        enforce_active_skill_tool_restrictions: None,
        workspace_services: None,
        sandbox_handle: None,
        budget_guard: None,
    }
}

struct BlockingTaskClient {
    started: Arc<Notify>,
    calls: Arc<AtomicUsize>,
}

struct DelayedBackgroundWriteClient {
    started: Arc<Notify>,
    release: Arc<Notify>,
    calls: AtomicUsize,
}

impl DelayedBackgroundWriteClient {
    async fn next_response(&self) -> LlmResponse {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let release = self.release.notified();
            self.started.notify_one();
            release.await;
            MockLlmClient::tool_call_response(
                "background-write",
                "write",
                serde_json::json!({
                    "file_path": "run-snapshot.txt",
                    "content": "kept-original-run-authority"
                }),
            )
        } else {
            MockLlmClient::text_response("Background write completed.")
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for DelayedBackgroundWriteClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        Ok(self.next_response().await)
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let response = self.next_response().await;
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

struct MutableRunBoundary {
    deny_writes: Arc<AtomicBool>,
}

struct FrozenRunBoundary {
    deny_writes: bool,
}

impl PermissionChecker for MutableRunBoundary {
    fn snapshot_for_run(&self) -> Option<Arc<dyn PermissionChecker>> {
        Some(Arc::new(FrozenRunBoundary {
            deny_writes: self.deny_writes.load(Ordering::SeqCst),
        }))
    }

    fn check(&self, tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
        if tool_name.eq_ignore_ascii_case("write") && self.deny_writes.load(Ordering::SeqCst) {
            PermissionDecision::Deny
        } else {
            PermissionDecision::Allow
        }
    }
}

impl PermissionChecker for FrozenRunBoundary {
    fn check(&self, tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
        if tool_name.eq_ignore_ascii_case("write") && self.deny_writes {
            PermissionDecision::Deny
        } else {
            PermissionDecision::Allow
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for BlockingTaskClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        std::future::pending::<Result<LlmResponse>>().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by task cancellation tests")
    }
}

#[tokio::test]
async fn task_tool_parent_cancellation_reaches_child_llm_call() {
    let workspace = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(BlockingTaskClient {
            started: Arc::clone(&started),
            calls: Arc::clone(&calls),
        }),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = TaskTool::new(executor);
    let cancellation = CancellationToken::new();
    let ctx = ToolContext::new(workspace.path().to_path_buf())
        .with_session_id("parent-session")
        .with_cancellation(cancellation.clone());
    let started_wait = started.notified();
    let run = tokio::spawn(async move {
        tool.execute(
            &serde_json::json!({
                "agent": "worker",
                "description": "wait",
                "prompt": "wait forever"
            }),
            &ctx,
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(1), started_wait)
        .await
        .expect("child provider call should start");
    cancellation.cancel();
    let output = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("parent cancellation must stop the child run")
        .expect("task join should succeed")
        .expect("task tool should return a typed failed output");

    assert!(!output.success);
    assert!(output.content.contains("cancelled"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn background_task_updates_shared_tracker_without_an_event_receiver() {
    use crate::subagent_task_tracker::{InMemorySubagentTaskTracker, SubagentStatus};

    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.md"), "verified source").unwrap();
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_writer(),
            Arc::new(MockLlmClient::new(vec![
                MockLlmClient::tool_call_response(
                    "read-source",
                    "read",
                    serde_json::json!({"file_path": "source.md"}),
                ),
                MockLlmClient::text_response("background result"),
            ])),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_subagent_tracker(Arc::clone(&tracker)),
    );
    let tool = TaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf()).with_session_id("parent-session");

    let started = tool
        .execute(
            &serde_json::json!({
                "agent": "writer",
                "description": "background audit",
                "prompt": "Read source.md and return the background result",
                "background": true
            }),
            &ctx,
        )
        .await
        .unwrap();
    let task_id = started
        .content
        .split("Task ID: ")
        .nth(1)
        .expect("background result includes task id")
        .trim()
        .to_string();

    let snapshot = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(snapshot) = tracker.get(&task_id).await {
                if snapshot.status != SubagentStatus::Running {
                    break snapshot;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background tracker should reach a terminal state");

    assert_eq!(snapshot.parent_session_id, "parent-session");
    assert_eq!(snapshot.description, "background audit");
    assert_eq!(snapshot.status, SubagentStatus::Completed);
    assert_eq!(snapshot.success, Some(true));
    assert_eq!(snapshot.output.as_deref(), Some("background result"));
    assert_eq!(
        snapshot.source_anchors,
        vec![ToolSourceAnchor {
            tool: "read".to_string(),
            url_or_path: "source.md".to_string(),
        }]
    );
}

#[tokio::test]
async fn background_task_keeps_the_admitted_run_permission_snapshot_across_turn_changes() {
    use crate::subagent_task_tracker::{InMemorySubagentTaskTracker, SubagentStatus};

    let workspace = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let deny_writes = Arc::new(AtomicBool::new(false));
    let live_boundary = Arc::new(MutableRunBoundary {
        deny_writes: Arc::clone(&deny_writes),
    });
    let admitted_boundary = live_boundary
        .snapshot_for_run()
        .expect("mutable host boundary must produce a run snapshot");
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let parent_context = crate::child_run::ChildRunContext {
        security_provider: None,
        hook_engine: None,
        skill_registry: None,
        permission_checker: Some(live_boundary.clone()),
        permission_policy: None,
        tool_timeout_ms: None,
        llm_api_timeout_ms: None,
        max_parallel_tasks: None,
        max_execution_time_ms: None,
        circuit_breaker_threshold: None,
        duplicate_tool_call_threshold: None,
        confirmation_manager: None,
        enforce_active_skill_tool_restrictions: None,
        workspace_services: None,
        sandbox_handle: None,
        budget_guard: None,
    };
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_writer(),
            Arc::new(DelayedBackgroundWriteClient {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                calls: AtomicUsize::new(0),
            }),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_parent_context(parent_context)
        .with_subagent_tracker(Arc::clone(&tracker)),
    );
    let tool = TaskTool::new(executor);
    let context = ToolContext::new(workspace.path().to_path_buf())
        .with_session_id("auto-turn")
        .with_run_governance(Some(admitted_boundary), None);
    let started_wait = started.notified();

    let launch = tool
        .execute(
            &serde_json::json!({
                "agent": "writer",
                "description": "continue after parent turn",
                "prompt": "Write run-snapshot.txt after the parent turn ends.",
                "background": true
            }),
            &context,
        )
        .await
        .unwrap();
    let task_id = launch
        .content
        .split("Task ID: ")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();

    tokio::time::timeout(Duration::from_secs(1), started_wait)
        .await
        .expect("background child should reach the delayed model response");
    deny_writes.store(true, Ordering::SeqCst);
    assert_eq!(
        live_boundary.check("write", &serde_json::json!({})),
        PermissionDecision::Deny
    );
    release.notify_waiters();

    let snapshot = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(snapshot) = tracker.get(&task_id).await {
                if snapshot.status != SubagentStatus::Running {
                    break snapshot;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background child should finish");

    assert_eq!(snapshot.status, SubagentStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("run-snapshot.txt")).unwrap(),
        "kept-original-run-authority"
    );
}

#[tokio::test]
async fn failed_background_task_does_not_persist_source_anchors() {
    use crate::subagent_task_tracker::{InMemorySubagentTaskTracker, SubagentStatus};

    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.md"), "verified source").unwrap();
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_writer(),
            Arc::new(MockLlmClient::new(vec![MockLlmClient::tool_call_response(
                "read-source",
                "read",
                serde_json::json!({"file_path": "source.md"}),
            )])),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_subagent_tracker(Arc::clone(&tracker)),
    );
    let tool = TaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let started = tool
        .execute(
            &serde_json::json!({
                "agent": "writer",
                "description": "background failure",
                "prompt": "Read source.md, then fail before the final answer",
                "background": true
            }),
            &ctx,
        )
        .await
        .unwrap();
    let task_id = started
        .content
        .split("Task ID: ")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();

    let snapshot = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(snapshot) = tracker.get(&task_id).await {
                if snapshot.status != SubagentStatus::Running {
                    break snapshot;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background tracker should reach a failed state");

    assert_eq!(snapshot.status, SubagentStatus::Failed);
    assert_eq!(snapshot.success, Some(false));
    assert!(snapshot.source_anchors.is_empty());
}

#[tokio::test]
async fn background_setup_error_records_a_terminal_tracker_snapshot() {
    use crate::subagent_task_tracker::{InMemorySubagentTaskTracker, SubagentStatus};

    let workspace = tempfile::tempdir().unwrap();
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let executor = Arc::new(
        TaskExecutor::new(
            Arc::new(AgentRegistry::new()),
            Arc::new(MockLlmClient::new(Vec::new())),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_subagent_tracker(Arc::clone(&tracker)),
    );
    let tool = TaskTool::new(executor);
    let (event_tx, mut event_rx) = broadcast::channel(8);
    let ctx = ToolContext::new(workspace.path().to_path_buf())
        .with_session_id("parent-session")
        .with_agent_event_tx(event_tx);

    let started = tool
        .execute(
            &serde_json::json!({
                "agent": "missing-agent",
                "description": "cannot start",
                "prompt": "fail before running",
                "background": true
            }),
            &ctx,
        )
        .await
        .unwrap();
    let task_id = started
        .content
        .split("Task ID: ")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();

    let snapshot = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(snapshot) = tracker.get(&task_id).await {
                if snapshot.status != SubagentStatus::Running {
                    break snapshot;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("early background failure must become terminal");

    assert_eq!(snapshot.status, SubagentStatus::Failed);
    assert_eq!(snapshot.success, Some(false));
    assert!(
        snapshot
            .output
            .as_deref()
            .is_some_and(|output| output.contains("Unknown agent type")),
        "{:?}",
        snapshot.output
    );
    let terminal = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let AgentEvent::SubagentEnd {
                task_id,
                output,
                success,
                ..
            } = event_rx.recv().await.unwrap()
            {
                break (task_id, output, success);
            }
        }
    })
    .await
    .expect("early failure should also emit SubagentEnd");
    assert_eq!(terminal.0, task_id);
    assert!(terminal.1.contains("Unknown agent type"), "{}", terminal.1);
    assert!(!terminal.2);
}

#[tokio::test]
async fn task_child_run_permission_allow() {
    let workspace = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "t1",
            "write",
            serde_json::json!({
                "file_path": workspace.path().join("out.txt").to_string_lossy(),
                "content": "WRITTEN"
            }),
        ),
        MockLlmClient::text_response("Done."),
    ]));

    let executor = TaskExecutor::new(
        test_registry_with_writer(),
        mock,
        workspace.path().to_string_lossy().to_string(),
    );

    let result = executor
        .execute(
            TaskParams {
                agent: "writer".to_string(),
                description: "Write file".to_string(),
                prompt: "Write out.txt".to_string(),
                background: false,
                max_steps: Some(3),
                output_schema: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

    assert!(
        result.success,
        "child run should succeed: {}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "no permission denial: {}",
        result.output
    );
    let content = std::fs::read_to_string(workspace.path().join("out.txt")).unwrap();
    assert_eq!(content, "WRITTEN");
}

#[tokio::test]
async fn task_child_run_permission_deny() {
    let workspace = tempfile::tempdir().unwrap();
    let registry = AgentRegistry::new();
    let spec = crate::subagent::WorkerAgentSpec::custom("restricted", "Restricted agent")
        .with_permissions(PermissionPolicy::new().allow("read(*)").deny("bash(*)"))
        .with_max_steps(3);
    registry.register(spec.into_agent_definition());

    let mock = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "t1",
            "bash",
            serde_json::json!({"command": "echo hello"}),
        ),
        MockLlmClient::text_response("Could not run bash."),
    ]));

    let executor = TaskExecutor::new(
        Arc::new(registry),
        mock,
        workspace.path().to_string_lossy().to_string(),
    );

    let result = executor
        .execute(
            TaskParams {
                agent: "restricted".to_string(),
                description: "Try bash".to_string(),
                prompt: "Run echo hello".to_string(),
                background: false,
                max_steps: Some(3),
                output_schema: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

    // The agent completes (LLM responds after denial), but bash was denied.
    // The denial is sent as a tool result to the LLM, which then responds.
    assert!(result.success, "agent should complete: {}", result.output);
}

#[tokio::test]
async fn deep_research_child_agent_inherits_parent_permissions_for_bash() {
    struct RecordingSandbox {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl crate::sandbox::BashSandbox for RecordingSandbox {
        async fn exec_command(
            &self,
            command: &str,
            _guest_workspace: &str,
        ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
            self.called.store(true, Ordering::SeqCst);
            Ok(crate::sandbox::SandboxOutput {
                stdout: format!("sandboxed: {command}"),
                stderr: String::new(),
                exit_code: 0,
            })
        }

        async fn shutdown(&self) {}
    }

    let workspace = tempfile::tempdir().unwrap();
    let sandbox_called = Arc::new(AtomicBool::new(false));
    let mock = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "t1",
            "bash",
            serde_json::json!({"command": "echo inherited-deep-research-bash"}),
        ),
        MockLlmClient::text_response("Done."),
    ]));
    let parent_policy = PermissionPolicy::new().allow("bash(*)");
    let parent_context = crate::child_run::ChildRunContext {
        security_provider: None,
        hook_engine: None,
        skill_registry: None,
        permission_checker: Some(Arc::new(parent_policy.clone())),
        permission_policy: Some(parent_policy),
        tool_timeout_ms: None,
        llm_api_timeout_ms: None,
        max_parallel_tasks: None,
        max_execution_time_ms: None,
        circuit_breaker_threshold: None,
        duplicate_tool_call_threshold: None,
        confirmation_manager: None,
        enforce_active_skill_tool_restrictions: None,
        workspace_services: None,
        sandbox_handle: Some(Arc::new(RecordingSandbox {
            called: Arc::clone(&sandbox_called),
        })),
        budget_guard: None,
    };

    let executor = TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        mock,
        workspace.path().to_string_lossy().to_string(),
    )
    .with_parent_context(parent_context);

    let result = executor
        .execute(
            TaskParams {
                agent: "deep-research".to_string(),
                description: "Gather evidence".to_string(),
                prompt: "Run a harmless bash evidence command.".to_string(),
                background: false,
                max_steps: Some(3),
                output_schema: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

    assert!(
        result.success,
        "deep-research child should inherit parent bash permission: {}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "no inherited child permission denial: {}",
        result.output
    );
    assert!(
        sandbox_called.load(Ordering::SeqCst),
        "delegated bash must retain the parent sandbox instead of using the host runner"
    );
}

#[tokio::test]
async fn task_child_run_confirmation_auto_approve() {
    let workspace = tempfile::tempdir().unwrap();
    let registry = AgentRegistry::new();
    // Agent with allow("read(*)") — write is not in allow list, so it returns Ask.
    // With AutoApproveConfirmation (default for agents with permissions), Ask → approve.
    let spec = crate::subagent::WorkerAgentSpec::custom("reader-writer", "Read and write")
        .with_permissions(PermissionPolicy::new().allow("read(*)"))
        .with_max_steps(3);
    registry.register(spec.into_agent_definition());

    let mock = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "t1",
            "write",
            serde_json::json!({
                "file_path": workspace.path().join("auto.txt").to_string_lossy(),
                "content": "AUTO_APPROVED"
            }),
        ),
        MockLlmClient::text_response("Written."),
    ]));

    let executor = TaskExecutor::new(
        Arc::new(registry),
        mock,
        workspace.path().to_string_lossy().to_string(),
    );

    let result = executor
        .execute(
            TaskParams {
                agent: "reader-writer".to_string(),
                description: "Write via auto-approve".to_string(),
                prompt: "Write auto.txt".to_string(),
                background: false,
                max_steps: Some(3),
                output_schema: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

    assert!(
        result.success,
        "Ask should be auto-approved: {}",
        result.output
    );
    assert!(
        !result.output.contains("MissingConfirmationManager"),
        "no MissingConfirmationManager: {}",
        result.output
    );
}

#[tokio::test]
async fn task_child_run_step_budget_enforced() {
    let workspace = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "t1",
            "read",
            serde_json::json!({"file_path": "/tmp/a.txt"}),
        ),
        MockLlmClient::tool_call_response(
            "t2",
            "read",
            serde_json::json!({"file_path": "/tmp/b.txt"}),
        ),
        MockLlmClient::tool_call_response(
            "t3",
            "read",
            serde_json::json!({"file_path": "/tmp/c.txt"}),
        ),
        MockLlmClient::text_response("Should not reach here."),
    ]));

    let executor = TaskExecutor::new(
        test_registry_with_writer(),
        mock,
        workspace.path().to_string_lossy().to_string(),
    );

    let result = executor
        .execute(
            TaskParams {
                agent: "writer".to_string(),
                description: "Exceed budget".to_string(),
                prompt: "Read many files".to_string(),
                background: false,
                max_steps: Some(2),
                output_schema: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

    // The agent should fail after exceeding 2 tool rounds
    assert!(
        !result.success,
        "should fail when exceeding step budget: {}",
        result.output
    );
    assert!(
        result.output.contains("Max tool rounds") || result.output.contains("max tool rounds"),
        "error should mention tool rounds: {}",
        result.output
    );
}

#[tokio::test]
async fn parallel_task_executor_runs_children_concurrently_and_preserves_input_order() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(ConcurrentLlmClient::new(2));
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        client.clone(),
        workspace.path().to_string_lossy().to_string(),
    ));

    let tasks = vec![
        TaskParams {
            agent: "worker".to_string(),
            description: "Slow task".to_string(),
            prompt: "slow branch".to_string(),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        },
        TaskParams {
            agent: "worker".to_string(),
            description: "Fast task".to_string(),
            prompt: "fast branch".to_string(),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        },
    ];

    let results = tokio::time::timeout(
        Duration::from_secs(2),
        executor.execute_parallel(tasks, None, None),
    )
    .await
    .expect("parallel children should reach the barrier and complete");

    assert_eq!(results.len(), 2);
    assert!(
        client.max_active() >= 2,
        "expected concurrent child execution, max_active={}",
        client.max_active()
    );
    assert!(results[0].success);
    assert!(results[0].output.contains("slow branch"));
    assert!(results[1].success);
    assert!(results[1].output.contains("fast branch"));
}

#[tokio::test]
async fn parallel_task_executor_respects_configured_concurrency_limit() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(LimitedConcurrencyLlmClient::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_text_worker(),
            client.clone(),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(2),
    );

    let tasks = (0..5)
        .map(|idx| TaskParams {
            agent: "worker".to_string(),
            description: format!("Task {idx}"),
            prompt: format!("branch {idx}"),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        })
        .collect::<Vec<_>>();

    let results = executor.execute_parallel(tasks, None, None).await;

    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|result| result.success));
    assert_eq!(client.max_active(), 2);
}

#[tokio::test]
async fn concurrent_parallel_calls_share_one_provider_capacity_window() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(LimitedConcurrencyLlmClient::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_text_worker(),
            client.clone(),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(2),
    );
    let tasks = |prefix: &str| {
        (0..3)
            .map(|idx| TaskParams {
                agent: "worker".to_string(),
                description: format!("{prefix} task {idx}"),
                prompt: format!("{prefix} branch {idx}"),
                background: false,
                max_steps: Some(1),
                output_schema: None,
            })
            .collect::<Vec<_>>()
    };

    let (first, second) = tokio::join!(
        executor.execute_parallel(tasks("first"), None, None),
        executor.execute_parallel(tasks("second"), None, None),
    );

    assert!(first.iter().all(|result| result.success));
    assert!(second.iter().all(|result| result.success));
    assert_eq!(
        client.max_active(),
        2,
        "per-call limits must not multiply across concurrent workflow steps"
    );
}

#[tokio::test]
async fn concurrent_parallel_batches_share_eight_slots_and_execute_each_branch_once() {
    const BATCH_COUNT: usize = 4;
    const TASKS_PER_BATCH: usize = 7;
    const PROVIDER_CAPACITY: usize = 8;

    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(LimitedConcurrencyLlmClient::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_text_worker(),
            client.clone(),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(PROVIDER_CAPACITY),
    );
    let batches = (0..BATCH_COUNT).map(|batch| {
        let executor = Arc::clone(&executor);
        async move {
            let tasks = (0..TASKS_PER_BATCH)
                .map(|index| TaskParams {
                    agent: "worker".to_string(),
                    description: format!("Batch {batch} task {index}"),
                    prompt: format!("batch {batch} branch {index}"),
                    background: false,
                    max_steps: Some(1),
                    output_schema: None,
                })
                .collect::<Vec<_>>();
            executor.execute_parallel(tasks, None, None).await
        }
    });

    let results = tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(batches))
        .await
        .expect("bounded concurrent batches should finish");

    assert_eq!(results.len(), BATCH_COUNT);
    for (batch, batch_results) in results.iter().enumerate() {
        assert_eq!(batch_results.len(), TASKS_PER_BATCH);
        for (index, result) in batch_results.iter().enumerate() {
            assert!(result.success, "unexpected failure: {result:#?}");
            assert!(
                result
                    .output
                    .contains(&format!("batch {batch} branch {index}")),
                "input order or branch identity changed: {result:#?}"
            );
        }
    }
    assert_eq!(
        client.max_active(),
        PROVIDER_CAPACITY,
        "all batches must share one provider window without underutilizing it"
    );
    assert_eq!(
        client.calls(),
        BATCH_COUNT * TASKS_PER_BATCH,
        "successful branches must execute exactly once"
    );
}

#[tokio::test]
async fn parallel_task_tool_rejects_arguments_that_conflict_with_its_contract() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(StaticLlmClient::new("must not be used")),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let task = |description: &str| {
        serde_json::json!({
            "agent": "worker",
            "description": description,
            "prompt": format!("Run {description}")
        })
    };

    let invalid_arguments = [
        serde_json::json!({"tasks": [task("only branch")]}),
        serde_json::json!({
            "tasks": [
                task("foreground branch"),
                {
                    "agent": "worker",
                    "description": "background branch",
                    "prompt": "Run independently",
                    "background": true
                }
            ]
        }),
        serde_json::json!({
            "min_success_count": 1,
            "tasks": [task("first branch"), task("second branch")]
        }),
        serde_json::json!({
            "allow_partial_failure": true,
            "min_success_count": 3,
            "tasks": [task("first branch"), task("second branch")]
        }),
        serde_json::json!({
            "allow_partial_failure": true,
            "min_success_count": 0,
            "tasks": [task("first branch"), task("second branch")]
        }),
        serde_json::json!({
            "timeout_ms": 0,
            "tasks": [task("first branch"), task("second branch")]
        }),
    ];

    for args in invalid_arguments {
        let output = tool.execute(&args, &ctx).await.unwrap();
        assert!(!output.success, "arguments should fail: {args:#}");
        assert!(
            matches!(
                output.error_kind,
                Some(crate::tools::ToolErrorKind::InvalidArgument { .. })
            ),
            "expected InvalidArgument for {args:#}, got {output:#?}"
        );
    }
}

#[tokio::test]
async fn cancelling_a_batch_waiting_for_provider_capacity_returns_promptly() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(BlockingCapacityLlmClient::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_text_worker(),
            client.clone(),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(1),
    );
    let tool = Arc::new(ParallelTaskTool::new(executor));

    let first_tool = Arc::clone(&tool);
    let first_context = ToolContext::new(workspace.path().to_path_buf());
    let first = tokio::spawn(async move {
        first_tool
            .execute(
                &serde_json::json!({
                    "allow_partial_failure": true,
                    "tasks": [
                        {
                            "agent": "worker",
                            "description": "Hold provider capacity",
                            "prompt": "blocking branch"
                        },
                        {
                            "agent": "missing-agent",
                            "description": "Control branch",
                            "prompt": "Return without using provider capacity"
                        }
                    ]
                }),
                &first_context,
            )
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(1), client.started.notified())
        .await
        .expect("the first branch should acquire provider capacity");

    let cancellation = CancellationToken::new();
    let second_context =
        ToolContext::new(workspace.path().to_path_buf()).with_cancellation(cancellation.clone());
    let second_tool = Arc::clone(&tool);
    let second = tokio::spawn(async move {
        second_tool
            .execute(
                &serde_json::json!({
                    "tasks": [
                        {
                            "agent": "worker",
                            "description": "Wait for provider capacity",
                            "prompt": "waiting branch"
                        },
                        {
                            "agent": "missing-agent",
                            "description": "Cancelled control branch",
                            "prompt": "Return without using provider capacity"
                        }
                    ]
                }),
                &second_context,
            )
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancellation.cancel();

    let second_output = tokio::time::timeout(Duration::from_millis(500), second)
        .await
        .expect("a cancelled capacity waiter must not hang")
        .expect("capacity waiter task should join");
    assert!(!second_output.success);
    assert!(
        second_output
            .content
            .contains("cancelled while waiting for parallel provider capacity"),
        "{second_output:#?}"
    );
    assert_eq!(
        client.calls(),
        1,
        "the cancelled waiter must never reach the provider"
    );

    client.release.notify_one();
    let first_output = tokio::time::timeout(Duration::from_secs(1), first)
        .await
        .expect("the capacity holder should finish after release")
        .expect("capacity holder task should join");
    assert!(first_output.success, "{first_output:#?}");
}

#[tokio::test]
async fn parallel_task_does_not_replay_a_read_only_branch_from_error_prose() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(TransientThenSuccessLlmClient::new(2));
    let executor = Arc::new(TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        client.clone(),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "allow_partial_failure": true,
                "tasks": [
                    {
                        "agent": "explore",
                        "description": "Inspect retry behavior",
                        "prompt": "Read the relevant files and report evidence."
                    },
                    {
                        "agent": "missing-agent",
                        "description": "Independent control branch",
                        "prompt": "Return a control result."
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        !output.success,
        "untyped failure must remain failed: {output:?}"
    );
    let metadata = output.metadata.expect("parallel metadata");
    assert!(metadata.get("retry_attempt_count").is_none());
    assert!(metadata.get("retried_task_count").is_none());
    assert!(metadata.get("recovered_task_count").is_none());
    assert!(metadata["results"][0].get("retry_attempts").is_none());
    assert_eq!(
        client.calls(),
        2,
        "only the child LLM boundary may consume its configured attempt budget"
    );
}

#[tokio::test]
async fn parallel_task_preserves_successful_siblings_without_prose_driven_replay() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(SelectiveTransientLlmClient::new());
    let executor = Arc::new(TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        client.clone(),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let context = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "tasks": [
                    {
                        "agent": "explore",
                        "description": "Collect stable evidence",
                        "prompt": "Inspect the stable branch."
                    },
                    {
                        "agent": "explore",
                        "description": "Collect flaky evidence",
                        "prompt": "Inspect the flaky branch."
                    }
                ]
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(
        !output.success,
        "one branch must remain failed: {output:#?}"
    );
    let metadata = output.metadata.expect("parallel metadata");
    assert!(metadata.get("retry_attempt_count").is_none());
    assert!(metadata.get("retried_task_count").is_none());
    assert!(metadata.get("recovered_task_count").is_none());
    assert!(metadata["results"][0].get("retry_attempts").is_none());
    assert!(metadata["results"][1].get("retry_attempts").is_none());
    assert!(metadata["results"][0]["output_excerpt"]
        .as_str()
        .is_some_and(|output| output.contains("stable branch")));
    assert_eq!(metadata["results"][1]["success"], false);
    assert_eq!(
        client.attempts_for("stable"),
        1,
        "a successful sibling must never be replayed"
    );
    assert_eq!(
        client.attempts_for("flaky"),
        2,
        "the task layer must not create an extra attempt from rendered error text"
    );
}

#[tokio::test]
async fn parallel_task_does_not_retry_a_mutating_branch() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(TransientThenSuccessLlmClient::new(2));
    let executor = Arc::new(TaskExecutor::new(
        Arc::new(AgentRegistry::new()),
        client.clone(),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "tasks": [
                    {
                        "agent": "general",
                        "description": "Potentially mutate the workspace",
                        "prompt": "Implement the requested change."
                    },
                    {
                        "agent": "missing-agent",
                        "description": "Independent failed control",
                        "prompt": "Return a control result."
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!output.success, "a mutating branch must not be replayed");
    let metadata = output.metadata.expect("parallel metadata");
    assert!(metadata.get("retry_attempt_count").is_none());
    assert!(metadata.get("retried_task_count").is_none());
    assert!(metadata.get("recovered_task_count").is_none());
    assert!(metadata["results"][0].get("retry_attempts").is_none());
    assert_eq!(
        client.calls(),
        2,
        "only the child circuit breaker may retry"
    );
}

#[tokio::test]
async fn parallel_task_executor_isolates_unknown_agent_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(StaticLlmClient::new("valid branch done")),
        workspace.path().to_string_lossy().to_string(),
    ));

    let tasks = vec![
        TaskParams {
            agent: "missing-agent".to_string(),
            description: "Missing".to_string(),
            prompt: "should fail".to_string(),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        },
        TaskParams {
            agent: "worker".to_string(),
            description: "Valid".to_string(),
            prompt: "should succeed".to_string(),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        },
    ];

    let results = executor.execute_parallel(tasks, None, None).await;

    assert_eq!(results.len(), 2);
    assert!(!results[0].success);
    assert_eq!(results[0].agent, "missing-agent");
    assert!(results[0].output.contains("Unknown agent type"));
    assert!(results[1].success);
    assert_eq!(results[1].agent, "worker");
    assert!(results[1].output.contains("valid branch done"));
}

#[tokio::test]
async fn parallel_task_executor_emits_subagent_events_for_each_child() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(StaticLlmClient::new("done")),
        workspace.path().to_string_lossy().to_string(),
    ));
    let (tx, mut rx) = broadcast::channel(64);

    let tasks = vec![
        TaskParams {
            agent: "worker".to_string(),
            description: "One".to_string(),
            prompt: "first".to_string(),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        },
        TaskParams {
            agent: "worker".to_string(),
            description: "Two".to_string(),
            prompt: "second".to_string(),
            background: false,
            max_steps: Some(1),
            output_schema: None,
        },
    ];

    let results = executor.execute_parallel(tasks, Some(tx), None).await;
    assert_eq!(results.len(), 2);

    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut progress_statuses: Vec<String> = Vec::new();
    let mut ended_tasks = std::collections::HashSet::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::SubagentStart { description, .. } => starts.push(description),
            AgentEvent::SubagentEnd {
                task_id,
                agent,
                success,
                ..
            } => {
                ended_tasks.insert(task_id);
                ends.push((agent, success));
            }
            AgentEvent::SubagentProgress {
                task_id, status, ..
            } => {
                assert!(
                    !ended_tasks.contains(&task_id),
                    "subagent progress arrived after SubagentEnd for {task_id}"
                );
                progress_statuses.push(status);
            }
            _ => {}
        }
    }

    starts.sort();
    assert_eq!(starts, vec!["One".to_string(), "Two".to_string()]);
    assert_eq!(ends.len(), 2);
    assert!(ends
        .iter()
        .all(|(agent, success)| agent == "worker" && *success));
    // Each child loop emits at least one TurnEnd, so we expect at least
    // two synthesized turn_completed progress events across the run.
    assert!(
        progress_statuses
            .iter()
            .filter(|s| s == &"turn_completed")
            .count()
            >= 2,
        "expected at least two turn_completed progress events, got {:?}",
        progress_statuses
    );
}

#[tokio::test]
async fn parallel_task_tool_reports_error_when_any_child_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(StaticLlmClient::new("valid branch done")),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "tasks": [
                    {
                        "agent": "missing-agent",
                        "description": "Missing",
                        "prompt": "should fail"
                    },
                    {
                        "agent": "worker",
                        "description": "Valid",
                        "prompt": "should succeed"
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        !output.success,
        "parallel_task should fail when any child result fails"
    );
    assert!(output.content.contains("[ERR]"));
    assert!(output.content.contains("[OK]"));
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["task_count"], 2);
    assert_eq!(metadata["success_count"], 1);
    assert_eq!(metadata["failed_count"], 1);
    assert_eq!(metadata["all_success"], false);
    assert_eq!(metadata["partial_failure"], true);
    assert_eq!(metadata["allow_partial_failure"], false);
    assert_eq!(metadata["results"][0]["success"], false);
    assert_eq!(metadata["results"][1]["success"], true);
}

#[tokio::test]
async fn parallel_task_tool_allows_partial_failure_when_requested() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(StaticLlmClient::new("valid branch done")),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "allow_partial_failure": true,
                "tasks": [
                    {
                        "agent": "missing-agent",
                        "description": "Missing",
                        "prompt": "should fail"
                    },
                    {
                        "agent": "worker",
                        "description": "Valid",
                        "prompt": "should succeed"
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        output.success,
        "parallel_task should continue when partial failures are allowed"
    );
    assert!(output.content.contains("[ERR]"));
    assert!(output.content.contains("[OK]"));
    assert!(output.content.contains("Partial failure tolerated"));
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["task_count"], 2);
    assert_eq!(metadata["success_count"], 1);
    assert_eq!(metadata["failed_count"], 1);
    assert_eq!(metadata["all_success"], false);
    assert_eq!(metadata["partial_failure"], true);
    assert_eq!(metadata["allow_partial_failure"], true);
    assert_eq!(metadata["results"][0]["success"], false);
    assert_eq!(metadata["results"][1]["success"], true);
}

#[tokio::test]
async fn parallel_task_tool_timeout_returns_completed_partial_results() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(ConcurrentLlmClient::new(2));
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        client.clone(),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "allow_partial_failure": true,
                "timeout_ms": 500,
                "tasks": [
                    {
                        "agent": "worker",
                        "description": "Slow",
                        "prompt": "slow branch"
                    },
                    {
                        "agent": "worker",
                        "description": "Fast",
                        "prompt": "fast branch"
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        output.success,
        "parallel_task should return completed evidence on timeout: {}",
        output.content
    );
    assert!(output.content.contains("Parallel task timed out"));
    assert!(client.max_active() >= 2);
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["timed_out"], true);
    assert_eq!(metadata["returned_early"], false);
    assert!(
        metadata["duration_ms"]
            .as_u64()
            .is_some_and(|duration_ms| duration_ms >= 500),
        "{metadata}"
    );
    assert_eq!(metadata["success_count"], 1);
    assert_eq!(metadata["failed_count"], 1);
    assert_eq!(metadata["results"][0]["success"], false);
    assert!(
        metadata["results"][0]["error_message"]
            .as_str()
            .is_some_and(|text| text.contains("timed out")),
        "{metadata}"
    );
    assert_eq!(metadata["results"][1]["success"], true);
    assert_eq!(
        metadata["results"][1]["error_message"],
        serde_json::Value::Null
    );
    assert!(output.content.contains("fast branch"));
}

#[tokio::test]
async fn parallel_task_tool_timeout_path_respects_configured_concurrency_limit() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(LimitedConcurrencyLlmClient::new());
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_text_worker(),
            client.clone(),
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(2),
    );
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "allow_partial_failure": true,
                "timeout_ms": 1_000,
                "tasks": (0..5)
                    .map(|idx| serde_json::json!({
                        "agent": "worker",
                        "description": format!("Task {idx}"),
                        "prompt": format!("branch {idx}")
                    }))
                    .collect::<Vec<_>>()
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        output.success,
        "parallel_task should complete: {}",
        output.content
    );
    assert_eq!(client.max_active(), 2);
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["timed_out"], false);
    assert_eq!(metadata["success_count"], 5);
    assert_eq!(metadata["failed_count"], 0);
}

#[tokio::test]
async fn parallel_task_tool_can_return_after_min_success_count() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(ConcurrentLlmClient::new(2)),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "allow_partial_failure": true,
                "min_success_count": 1,
                "tasks": [
                    {
                        "agent": "worker",
                        "description": "Slow",
                        "prompt": "slow branch"
                    },
                    {
                        "agent": "worker",
                        "description": "Fast",
                        "prompt": "fast branch"
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        output.success,
        "parallel_task should return once enough branches succeed: {}",
        output.content
    );
    assert!(output.content.contains("min_success_count=1"));
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["timed_out"], false);
    assert_eq!(metadata["returned_early"], true);
    assert_eq!(metadata["success_count"], 1);
    assert_eq!(metadata["failed_count"], 1);
    assert_eq!(metadata["results"][0]["success"], false);
    assert_eq!(metadata["results"][1]["success"], true);
}

#[tokio::test]
async fn parallel_task_tool_returns_structured_child_output_when_schema_requested() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(SchemaCoercionClient),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "tasks": [
                    {
                        "agent": "worker",
                        "description": "Structured verdict",
                        "prompt": "Return a verdict.",
                        "output_schema": verdict_schema()
                    },
                    {
                        "agent": "worker",
                        "description": "Independent plain result",
                        "prompt": "Return a plain result."
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        output.success,
        "parallel_task should succeed: {}",
        output.content
    );
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["results"][0]["success"], true);
    assert_eq!(metadata["results"][0]["structured"]["verdict"], "ok");
}

struct BatchSourceTool {
    name: &'static str,
    source_anchor: &'static str,
}

#[async_trait::async_trait]
impl Tool for BatchSourceTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Returns a deterministic source anchor for nested batch event tests"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::success(format!("{} complete", self.name))
            .with_metadata(serde_json::json!({"source_anchors": [self.source_anchor]})))
    }
}

struct BatchFailureTool;

#[async_trait::async_trait]
impl Tool for BatchFailureTool {
    fn name(&self) -> &str {
        "nested_failure"
    }

    fn description(&self) -> &str {
        "Returns a deterministic nested failure"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::error("nested failure"))
    }
}

#[tokio::test]
async fn batch_nested_tool_events_are_paired_and_preserve_web_source_anchors() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(crate::tools::ToolExecutor::new(
        workspace.path().to_string_lossy().to_string(),
    ));
    // Replace network-backed builtins with deterministic fixtures while
    // retaining their production names and metadata contract.
    executor
        .registry()
        .register_builtin(Arc::new(BatchSourceTool {
            name: "web_search",
            source_anchor: "https://search.example.test/result",
        }));
    executor
        .registry()
        .register_builtin(Arc::new(BatchSourceTool {
            name: "web_fetch",
            source_anchor: "https://fetch.example.test/article",
        }));
    executor.register_dynamic_tool(Arc::new(BatchFailureTool));

    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "batch-source-call",
            "batch",
            serde_json::json!({
                "invocations": [
                    {"tool": "web_search", "args": {"query": "fixture"}},
                    {"tool": "web_fetch", "args": {"url": "https://fetch.example.test/article"}},
                    {"tool": "nested_failure", "args": {}}
                ]
            }),
        ),
        MockLlmClient::text_response("Batch observed."),
    ]));
    let permissions = PermissionPolicy::new()
        .allow("batch(*)")
        .allow("web_search(*)")
        .allow("web_fetch(*)")
        .allow("nested_failure(*)");
    let agent = AgentLoop::new(
        llm,
        Arc::clone(&executor),
        ToolContext::new(workspace.path().to_path_buf()),
        AgentConfig {
            tools: executor.definitions(),
            permission_checker: Some(Arc::new(permissions)),
            planning_mode: crate::prompts::PlanningMode::Disabled,
            continuation_enabled: false,
            ..Default::default()
        },
    );
    let (event_tx, mut event_rx) = mpsc::channel(64);

    let result = agent
        .execute_with_session(
            &[],
            "Run the source batch.",
            Some("batch-source-session"),
            Some(event_tx),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.text, "Batch observed.");

    let mut events = Vec::new();
    while let Some(event) = event_rx.recv().await {
        events.push(event);
    }

    let mut nested_starts = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionStart { id, name, .. } if id.starts_with("nested-") => {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut nested_ends = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolEnd { id, name, .. } if id.starts_with("nested-") => {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    nested_starts.sort();
    nested_ends.sort();
    assert_eq!(nested_starts.len(), 3, "{events:?}");
    assert_eq!(nested_ends, nested_starts, "{events:?}");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolEnd {
            name,
            exit_code: 1,
            output,
            ..
        } if name == "nested_failure" && output.contains("nested failure")
    )));
    assert_eq!(
        events
            .iter()
            .filter(
                |event| matches!(event, AgentEvent::ToolEnd { id, .. } if id == "batch-source-call")
            )
            .count(),
        1,
        "the outer model tool call must keep its existing single ToolEnd owner"
    );

    let source_ctx = ToolContext::new(workspace.path().to_path_buf());
    let mut source_anchors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut scanned = 0;
    for event in &events {
        collect_tool_source_anchors(
            event,
            &source_ctx,
            &mut source_anchors,
            &mut seen,
            &mut scanned,
        );
    }
    source_anchors.sort_by(|left, right| left.tool.cmp(&right.tool));
    assert_eq!(
        source_anchors,
        vec![
            ToolSourceAnchor {
                tool: "web_fetch".to_string(),
                url_or_path: "https://fetch.example.test/article".to_string(),
            },
            ToolSourceAnchor {
                tool: "web_search".to_string(),
                url_or_path: "https://search.example.test/result".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn parallel_task_metadata_carries_successful_child_read_anchor() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.md"), "verified source").unwrap();
    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "read-source",
            "read",
            serde_json::json!({"file_path": "source.md"}),
        ),
        MockLlmClient::text_response("Evidence came from source.md."),
        MockLlmClient::text_response("Independent source review complete."),
    ]));
    let executor = Arc::new(
        TaskExecutor::new(
            test_registry_with_writer(),
            llm,
            workspace.path().to_string_lossy().to_string(),
        )
        .with_max_parallel_tasks(1),
    );
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "tasks": [
                    {
                        "agent": "writer",
                        "description": "Read source",
                        "prompt": "Read source.md and report what it says."
                    },
                    {
                        "agent": "writer",
                        "description": "Review source independently",
                        "prompt": "Return a short independent review."
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(output.success, "{}", output.content);
    let metadata = output.metadata.expect("parallel task metadata");
    assert_eq!(
        metadata["results"][0]["source_anchors"],
        serde_json::json!([{
            "tool": "read",
            "url_or_path": "source.md"
        }])
    );
}

#[tokio::test]
async fn child_source_anchor_is_sanitized_before_task_metadata_persistence() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("private-source.md"),
        "sensitive fixture",
    )
    .unwrap();
    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "read-source",
            "read",
            serde_json::json!({"file_path": "private-source.md"}),
        ),
        MockLlmClient::text_response("Evidence collected."),
    ]));
    let parent_context = crate::child_run::ChildRunContext {
        security_provider: Some(Arc::new(RedactingSourceSecurityProvider)),
        hook_engine: None,
        skill_registry: None,
        permission_checker: None,
        permission_policy: None,
        tool_timeout_ms: None,
        llm_api_timeout_ms: None,
        max_parallel_tasks: None,
        max_execution_time_ms: None,
        circuit_breaker_threshold: None,
        duplicate_tool_call_threshold: None,
        confirmation_manager: None,
        enforce_active_skill_tool_restrictions: None,
        workspace_services: None,
        sandbox_handle: None,
        budget_guard: None,
    };
    let executor = TaskExecutor::new(
        test_registry_with_writer(),
        llm,
        workspace.path().to_string_lossy().to_string(),
    )
    .with_parent_context(parent_context);

    let result = executor
        .execute(
            TaskParams {
                agent: "writer".to_string(),
                description: "Read sensitive-named source".to_string(),
                prompt: "Read private-source.md.".to_string(),
                background: false,
                max_steps: Some(3),
                output_schema: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.source_anchors.len(), 1);
    assert_eq!(result.source_anchors[0].url_or_path, "[redacted]-source.md");
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("private-source"));
}

#[tokio::test]
async fn child_events_terminal_state_and_direct_result_are_sanitized() {
    use crate::subagent_task_tracker::InMemorySubagentTaskTracker;

    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("private-source.md"),
        "private fixture contents",
    )
    .unwrap();
    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "read-source",
            "read",
            serde_json::json!({"file_path": "private-source.md"}),
        ),
        MockLlmClient::text_response("private final result"),
    ]));
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let executor = TaskExecutor::new(
        test_registry_with_writer(),
        llm,
        workspace.path().to_string_lossy().to_string(),
    )
    .with_parent_context(redacting_parent_context())
    .with_subagent_tracker(Arc::clone(&tracker));
    let (event_tx, mut event_rx) = broadcast::channel(64);

    let result = executor
        .execute(
            TaskParams {
                agent: "writer".to_string(),
                description: "Read private source".to_string(),
                prompt: "Read private-source.md and report private details.".to_string(),
                background: false,
                max_steps: Some(3),
                output_schema: None,
            },
            Some(event_tx),
            Some("parent-session"),
        )
        .await
        .unwrap();

    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolEnd { name, .. } if name == "read")));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::SubagentProgress { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::SubagentEnd { .. })));
    let serialized_events = serde_json::to_string(&events).unwrap();
    assert!(
        !serialized_events.contains("private"),
        "{serialized_events}"
    );

    assert_eq!(result.output, "[redacted] final result");
    assert_eq!(result.source_anchors[0].url_or_path, "[redacted]-source.md");
    assert!(!serde_json::to_string(&result).unwrap().contains("private"));

    let snapshot = tracker.get(&result.task_id).await.unwrap();
    assert_eq!(snapshot.output.as_deref(), Some("[redacted] final result"));
    assert_eq!(
        snapshot.source_anchors,
        vec![ToolSourceAnchor {
            tool: "read".to_string(),
            url_or_path: "[redacted]-source.md".to_string(),
        }]
    );
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("private"));
}

#[tokio::test]
async fn task_tool_returns_structured_child_output_when_schema_requested() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(SchemaCoercionClient),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = TaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "agent": "worker",
                "description": "Structured verdict",
                "prompt": "Return a verdict.",
                "output_schema": verdict_schema()
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(output.success, "task should succeed: {}", output.content);
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["structured"]["verdict"], "ok");
}

#[tokio::test]
async fn bounded_child_tool_call_can_finalize_structured_output_without_a_live_child() {
    use crate::subagent_task_tracker::InMemorySubagentTaskTracker;

    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("evidence.md"), "traceable evidence").unwrap();
    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "read-evidence",
            "read",
            serde_json::json!({"file_path": "evidence.md"}),
        ),
        MockLlmClient::text_response(r#"{"verdict":"supported"}"#),
    ]));
    let tracker = Arc::new(InMemorySubagentTaskTracker::new());
    let executor = TaskExecutor::new(
        test_registry_with_writer(),
        llm,
        workspace.path().to_string_lossy().to_string(),
    )
    .with_subagent_tracker(Arc::clone(&tracker));

    let result = executor
        .execute(
            TaskParams {
                agent: "writer".to_string(),
                description: "Collect bounded evidence".to_string(),
                prompt: "Read evidence.md once, then return the structured verdict.".to_string(),
                background: false,
                max_steps: Some(2),
                output_schema: Some(verdict_schema()),
            },
            None,
            Some("parent-session"),
        )
        .await
        .unwrap();

    assert!(result.success, "unexpected child result: {result:#?}");
    assert_eq!(
        result.structured,
        Some(serde_json::json!({"verdict": "supported"}))
    );
    assert!(tracker.list_pending().await.is_empty());
}

#[tokio::test]
async fn parallel_task_tool_still_fails_when_all_children_fail() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_text_worker(),
        Arc::new(StaticLlmClient::new("unused")),
        workspace.path().to_string_lossy().to_string(),
    ));
    let tool = ParallelTaskTool::new(executor);
    let ctx = ToolContext::new(workspace.path().to_path_buf());

    let output = tool
        .execute(
            &serde_json::json!({
                "allow_partial_failure": true,
                "tasks": [
                    {
                        "agent": "missing-one",
                        "description": "Missing one",
                        "prompt": "should fail"
                    },
                    {
                        "agent": "missing-two",
                        "description": "Missing two",
                        "prompt": "should also fail"
                    }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(
        !output.success,
        "parallel_task should fail if every child task fails"
    );
    let metadata = output.metadata.expect("metadata");
    assert_eq!(metadata["success_count"], 0);
    assert_eq!(metadata["failed_count"], 2);
    assert_eq!(metadata["all_success"], false);
    assert_eq!(metadata["partial_failure"], false);
    assert_eq!(metadata["allow_partial_failure"], true);
}

#[tokio::test]
async fn parallel_task_both_inherit_permissions() {
    let workspace = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockLlmClient::new(vec![
        // Task 1 responses
        MockLlmClient::tool_call_response(
            "t1",
            "write",
            serde_json::json!({
                "file_path": workspace.path().join("p1.txt").to_string_lossy(),
                "content": "P1"
            }),
        ),
        MockLlmClient::text_response("Done 1."),
        // Task 2 responses
        MockLlmClient::tool_call_response(
            "t2",
            "write",
            serde_json::json!({
                "file_path": workspace.path().join("p2.txt").to_string_lossy(),
                "content": "P2"
            }),
        ),
        MockLlmClient::text_response("Done 2."),
    ]));

    let executor = Arc::new(TaskExecutor::new(
        test_registry_with_writer(),
        mock,
        workspace.path().to_string_lossy().to_string(),
    ));

    let tasks = vec![
        TaskParams {
            agent: "writer".to_string(),
            description: "Write p1".to_string(),
            prompt: "Write p1.txt".to_string(),
            background: false,
            max_steps: Some(3),
            output_schema: None,
        },
        TaskParams {
            agent: "writer".to_string(),
            description: "Write p2".to_string(),
            prompt: "Write p2.txt".to_string(),
            background: false,
            max_steps: Some(3),
            output_schema: None,
        },
    ];

    let results = executor.execute_parallel(tasks, None, None).await;
    assert_eq!(results.len(), 2);

    for result in &results {
        assert!(
            result.success,
            "parallel child should succeed: {}",
            result.output
        );
    }
}

#[test]
fn synthesize_progress_emits_tool_completed_for_tool_end() {
    let event = AgentEvent::ToolEnd {
        id: "call-1".to_string(),
        name: "bash".to_string(),
        args: None,
        output: "hello".to_string(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    };
    let progress = synthesize_subagent_progress(&event, "task-1", "task-run-task-1").expect("some");
    match progress {
        AgentEvent::SubagentProgress {
            task_id,
            session_id,
            status,
            metadata,
        } => {
            assert_eq!(task_id, "task-1");
            assert_eq!(session_id, "task-run-task-1");
            assert_eq!(status, "tool_completed");
            assert_eq!(metadata["tool"], "bash");
            assert_eq!(metadata["exit_code"], 0);
            assert_eq!(metadata["output_bytes"], 5);
            assert!(metadata.get("error_kind").is_none());
        }
        other => panic!("expected SubagentProgress, got {:?}", other),
    }
}

#[test]
fn synthesize_progress_includes_error_kind_when_present() {
    let event = AgentEvent::ToolEnd {
        id: "call-2".to_string(),
        name: "edit".to_string(),
        args: None,
        output: "boom".to_string(),
        exit_code: 1,
        metadata: None,
        error_kind: Some(crate::tools::ToolErrorKind::NotFound {
            path: "missing.txt".to_string(),
        }),
    };
    let progress = synthesize_subagent_progress(&event, "task-x", "task-run-task-x").expect("some");
    if let AgentEvent::SubagentProgress { metadata, .. } = progress {
        assert!(
            metadata.get("error_kind").is_some(),
            "error_kind should propagate into metadata"
        );
    } else {
        panic!("expected SubagentProgress");
    }
}

#[test]
fn synthesize_progress_emits_turn_completed_for_turn_end() {
    let event = AgentEvent::TurnEnd {
        turn: 3,
        usage: crate::llm::TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 25,
            total_tokens: 125,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
    };
    let progress = synthesize_subagent_progress(&event, "task-1", "task-run-task-1").expect("some");
    if let AgentEvent::SubagentProgress {
        status, metadata, ..
    } = progress
    {
        assert_eq!(status, "turn_completed");
        assert_eq!(metadata["turn"], 3);
        assert_eq!(metadata["total_tokens"], 125);
        assert_eq!(metadata["prompt_tokens"], 100);
        assert_eq!(metadata["completion_tokens"], 25);
    } else {
        panic!("expected SubagentProgress");
    }
}

#[test]
fn synthesize_progress_ignores_unrelated_events() {
    let ignored = [
        AgentEvent::TextDelta {
            text: "hi".to_string(),
        },
        AgentEvent::ToolStart {
            id: "x".to_string(),
            name: "bash".to_string(),
        },
        AgentEvent::TurnStart { turn: 1 },
        AgentEvent::SubagentStart {
            task_id: "nested".to_string(),
            session_id: "nested-run".to_string(),
            parent_session_id: "parent".to_string(),
            agent: "explore".to_string(),
            description: "nested".to_string(),
            started_ms: 0,
        },
    ];
    for event in &ignored {
        assert!(
            synthesize_subagent_progress(event, "task", "session").is_none(),
            "{:?} should not emit progress",
            event
        );
    }
}
