use super::*;
use crate::llm::{LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use std::sync::Mutex;
use tokio::sync::mpsc;

struct RecordingSummaryClient {
    prompts: Mutex<Vec<String>>,
    systems: Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl LlmClient for RecordingSummaryClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.prompts.lock().unwrap().push(
            messages
                .iter()
                .map(Message::text)
                .collect::<Vec<_>>()
                .join("\n"),
        );
        self.systems
            .lock()
            .unwrap()
            .push(system.map(str::to_string));
        Ok(LlmResponse {
            message: Message::assistant("durable compact summary"),
            usage: TokenUsage::default(),
            stop_reason: Some("stop".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by compaction")
    }
}

#[tokio::test]
async fn folded_prefix_summary_is_model_text_and_prunes_old_tool_output() {
    let client = Arc::new(RecordingSummaryClient {
        prompts: Mutex::new(Vec::new()),
        systems: Mutex::new(Vec::new()),
    });
    let huge = "x".repeat(80_000);
    let lines = vec![
        "user\n## Goal\nship the parser".to_string(),
        format!("tool\n{huge}"),
        "tool\nrecent-output".to_string(),
    ];
    let summary = summarize_folded_prefix(client.as_ref(), &lines, 8_000, None)
        .await
        .expect("summary");
    assert!(summary.contains("durable compact summary"));
    assert!(summary.contains("ship the parser"));
    assert!(!summary.contains("[earlier transcript compacted]"));
    let prompt = client.prompts.lock().unwrap()[0].clone();
    assert!(prompt.contains("[output pruned"));
    assert!(prompt.contains("recent-output"));
    assert!(!prompt.contains(&huge));
}

// -- should_auto_compact tests --

#[test]
fn test_should_auto_compact_below_threshold() {
    assert!(!should_auto_compact(50_000, 200_000, 0.80));
}

#[test]
fn test_should_auto_compact_at_threshold() {
    assert!(should_auto_compact(160_000, 200_000, 0.80));
}

#[test]
fn test_should_auto_compact_above_threshold() {
    assert!(should_auto_compact(190_000, 200_000, 0.80));
}

#[test]
fn test_should_auto_compact_zero_max() {
    assert!(!should_auto_compact(100, 0, 0.80));
}

#[test]
fn test_should_auto_compact_exact_boundary() {
    // 80% of 100_000 = 80_000
    assert!(should_auto_compact(80_000, 100_000, 0.80));
    assert!(!should_auto_compact(79_999, 100_000, 0.80));
}

#[test]
fn test_should_auto_compact_custom_threshold() {
    assert!(should_auto_compact(95_000, 100_000, 0.95));
    assert!(!should_auto_compact(94_000, 100_000, 0.95));
}

// -- estimate_tokens tests --

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_short() {
    assert_eq!(estimate_tokens("hello world!"), 3); // 12 chars / 4
}

#[test]
fn test_estimate_tokens_code() {
    let code = "fn main() {\n    println!(\"Hello, world!\");\n}";
    let tokens = estimate_tokens(code);
    assert!(tokens > 5 && tokens < 20);
}

// -- prune_tool_outputs tests --

fn make_tool_result_msg(tool_id: &str, content: &str) -> Message {
    Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: tool_id.to_string(),
            content: ToolResultContentField::Text(content.to_string()),
            is_error: None,
            trust: crate::llm::ToolResultTrustV1::WorkspaceData,
            redaction_reviewed: false,
        }],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: Default::default(),
    }
}

fn make_text_msg(role: &str, text: &str) -> Message {
    Message {
        role: role.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: Default::default(),
    }
}

fn make_tool_use_msg(tool_id: &str) -> Message {
    Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: tool_id.to_string(),
            name: "test".to_string(),
            input: serde_json::json!({}),
        }],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: Default::default(),
    }
}

#[test]
fn test_prune_no_tool_outputs() {
    let messages = vec![
        make_text_msg("user", "hello"),
        make_text_msg("assistant", "hi there"),
    ];
    assert!(prune_tool_outputs(&messages, 200_000).is_none());
}

#[test]
fn safe_recent_boundary_keeps_every_matching_tool_call() {
    let messages = vec![
        make_tool_use_msg("first"),
        make_tool_use_msg("second"),
        Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "first".to_string(),
                    content: ToolResultContentField::Text("one".to_string()),
                    is_error: None,
                    trust: crate::llm::ToolResultTrustV1::WorkspaceData,
                    redaction_reviewed: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "second".to_string(),
                    content: ToolResultContentField::Text("two".to_string()),
                    is_error: None,
                    trust: crate::llm::ToolResultTrustV1::WorkspaceData,
                    redaction_reviewed: false,
                },
            ],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: Default::default(),
        },
        make_text_msg("assistant", "done"),
    ];

    assert_eq!(safe_recent_start(&messages, 2), 0);
}

#[test]
fn safe_recent_boundary_summarizes_an_orphaned_tool_result() {
    let messages = vec![
        make_text_msg("user", "request"),
        make_tool_result_msg("missing", "result"),
        make_text_msg("assistant", "done"),
    ];

    assert_eq!(safe_recent_start(&messages, 1), 2);
}

#[test]
fn safe_recent_boundary_keeps_a_tool_call_whose_result_is_after_the_cut() {
    let messages = vec![
        make_tool_use_msg("write-1"),
        make_text_msg("assistant", "between call and result"),
        make_tool_result_msg("write-1", "wrote guest.txt"),
        make_text_msg("assistant", "done"),
    ];

    let start = safe_recent_start(&messages, 1);
    let kept_call = messages[start..].iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "write-1"))
    });
    assert!(
        kept_call,
        "a retained tool result must keep its call; start={start}"
    );
}

#[test]
fn test_prune_small_tool_outputs() {
    let messages = vec![
        make_tool_result_msg("t1", "small output"),
        make_text_msg("assistant", "ok"),
    ];
    // Small output, no pruning needed
    assert!(prune_tool_outputs(&messages, 200_000).is_none());
}

#[test]
fn test_prune_large_tool_outputs() {
    // Create messages with large tool outputs that exceed protection threshold
    let large_content = "x".repeat(200_000); // ~50k tokens
    let large_content2 = "y".repeat(200_000); // ~50k tokens
    let small_recent = "z".repeat(40_000); // ~10k tokens (recent, protected)

    let messages = vec![
        make_tool_result_msg("t1", &large_content), // old, should be pruned
        make_text_msg("assistant", "processed t1"),
        make_tool_result_msg("t2", &large_content2), // old, should be pruned
        make_text_msg("assistant", "processed t2"),
        make_tool_result_msg("t3", &small_recent), // recent, protected
        make_text_msg("assistant", "done"),
    ];

    let result = prune_tool_outputs(&messages, 200_000);
    assert!(result.is_some());

    let pruned = result.unwrap();
    // t1 and/or t2 should be pruned (oldest first)
    let t1_content = match &pruned[0].content[0] {
        ContentBlock::ToolResult { content, .. } => content.as_text(),
        _ => panic!("Expected ToolResult"),
    };
    assert_eq!(t1_content, PRUNED_MARKER);
}

#[test]
fn test_prune_preserves_recent_outputs() {
    // Recent output alone fills the protection budget (~50k tokens)
    let large_old = "a".repeat(400_000); // ~100k tokens
    let recent = "b".repeat(200_000); // ~50k tokens (fills protection budget)

    let messages = vec![
        make_tool_result_msg("old", &large_old),
        make_text_msg("assistant", "ok"),
        make_tool_result_msg("recent", &recent),
        make_text_msg("assistant", "done"),
    ];

    let result = prune_tool_outputs(&messages, 200_000);
    assert!(result.is_some());

    let pruned = result.unwrap();
    // Old should be pruned
    let old_content = match &pruned[0].content[0] {
        ContentBlock::ToolResult { content, .. } => content.as_text(),
        _ => panic!("Expected ToolResult"),
    };
    assert_eq!(old_content, PRUNED_MARKER);

    // Recent should be preserved
    let recent_content = match &pruned[2].content[0] {
        ContentBlock::ToolResult { content, .. } => content.as_text(),
        _ => panic!("Expected ToolResult"),
    };
    assert_ne!(recent_content, PRUNED_MARKER);
}

#[test]
fn test_prune_marker_text() {
    assert!(PRUNED_MARKER.contains("pruned"));
}

#[test]
fn a_pruned_result_with_a_retained_artifact_points_at_it() {
    let uri = "a3s://tool-output/bash/abc123";
    let old = format!(
        "{}\n\n{}{uri} — page it with read]",
        "a".repeat(400_000),
        crate::tools::FULL_OUTPUT_ARTIFACT_MARKER
    );
    let messages = vec![
        make_tool_result_msg("old", &old),
        make_text_msg("assistant", "ok"),
        make_tool_result_msg("recent", &"b".repeat(200_000)),
        make_text_msg("assistant", "done"),
    ];
    let pruned = prune_tool_outputs(&messages, 200_000).expect("pruned");
    let ContentBlock::ToolResult { content, .. } = &pruned[0].content[0] else {
        panic!("Expected ToolResult");
    };
    let marker = content.as_text();
    assert!(marker.contains(uri), "{marker}");
    assert!(!marker.contains("re-run command"), "{marker}");

    // An adapter-held or foreign URI is not model-readable: keep the old hint.
    assert_eq!(
        pruned_marker("x\n\n[Full output artifact: cas://sha256/abc]"),
        PRUNED_MARKER
    );
}

#[test]
fn test_prune_bounds_a_single_oversized_recent_output() {
    let messages = vec![make_tool_result_msg("recent", &"x".repeat(200_000))];

    let pruned = prune_tool_outputs(&messages, 100_000).expect("large output should shrink");
    let content = match &pruned[0].content[0] {
        ContentBlock::ToolResult { content, .. } => content.as_text(),
        _ => panic!("Expected ToolResult"),
    };

    assert!(content.len() < 200_000);
    assert!(content.contains("output compacted"));
}

#[tokio::test]
async fn compact_summary_input_preserves_tool_calls_and_results() {
    let mut messages = (0..40)
        .map(|i| make_text_msg(if i % 2 == 0 { "user" } else { "assistant" }, "history"))
        .collect::<Vec<_>>();
    messages[2] = Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "cargo test -p a3s-code-core"}),
        }],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: Default::default(),
    };
    messages[3] = make_tool_result_msg("tool-1", "all 42 tests passed");
    messages[39] = make_text_msg(
        "assistant",
        "latest verified state must survive in the durable summary",
    );
    let client = Arc::new(RecordingSummaryClient {
        prompts: Mutex::new(Vec::new()),
        systems: Mutex::new(Vec::new()),
    });
    let llm_client: Arc<dyn LlmClient> = client.clone();

    let compacted = compact_messages(
        "tool-history",
        &messages,
        &llm_client,
        CompactionBudget::for_auto_compaction(128_000, 0.85, 0),
        None,
        None,
    )
    .await
    .unwrap()
    .expect("history should compact");

    assert!(
        compacted.summary.contains("## Goal"),
        "pinned product goal must survive even when the model omits it"
    );
    assert!(
        compacted.summary.contains("history"),
        "without a later revision, the first product user turn stays the pinned goal"
    );
    assert!(
        compacted.summary.contains("durable compact summary"),
        "model summary body must remain after Goal pinning"
    );
    assert_eq!(compacted.messages[0].role, "user");
    assert!(
        !compacted.messages[0].is_product_transcript(),
        "compaction summary is model-wire context, not a product user bubble"
    );
    let prompts = client.prompts.lock().unwrap();
    assert!(prompts[0].contains("cargo test -p a3s-code-core"));
    assert!(prompts[0].contains("all 42 tests passed"));
    assert!(prompts[0].contains("latest verified state must survive"));
    assert!(
        prompts[0].contains("Pinned original goal"),
        "compaction prompt must surface the pinned goal to the model"
    );
    let systems = client.systems.lock().unwrap();
    assert!(systems[0]
        .as_deref()
        .is_some_and(|system| system.contains("untrusted data")));
}

#[tokio::test]
async fn rolling_compaction_reinserts_goal_when_model_drops_it() {
    let original_goal =
        "Write /app/build_part.py and save the part to /app/part.FCStd using PartDesign";
    let prior_summary = format!(
        "{}## Goal\n{original_goal}\n\n## Current State\nScanned drawings; ambiguities remain.",
        crate::prompts::CONTEXT_SUMMARY_PREFIX
    );
    let mut messages = vec![Message::user_wire(&prior_summary)];
    for i in 0..30 {
        messages.push(make_text_msg(
            if i % 2 == 0 { "assistant" } else { "user" },
            &format!("pixel scan step {i}"),
        ));
    }

    struct GoalDroppingClient;
    #[async_trait::async_trait]
    impl LlmClient for GoalDroppingClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            Ok(LlmResponse {
                message: Message::assistant(
                    "Looking at the full drawing, I can now see the overall layout clearly.",
                ),
                usage: TokenUsage::default(),
                stop_reason: Some("stop".to_string()),
                token_logprobs: Vec::new(),
                meta: None,
            })
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming is not used by compaction")
        }
    }

    let llm_client: Arc<dyn LlmClient> = Arc::new(GoalDroppingClient);
    let compacted = compact_messages(
        "goal-pin",
        &messages,
        &llm_client,
        CompactionBudget::for_auto_compaction(128_000, 0.85, 0),
        None,
        None,
    )
    .await
    .unwrap()
    .expect("history should compact");

    let goal = goal_section_body(&compacted.summary).expect("## Goal must be present");
    assert_eq!(goal, original_goal);
    assert!(compacted.summary.contains("/app/build_part.py"));
    assert!(compacted.summary.contains("/app/part.FCStd"));
    assert!(compacted.summary.contains("PartDesign"));
}

/// A later `Revision:` is the active obligation. The first user turn stays
/// history. This replaces the older "first product turn is always the goal"
/// pin when the user explicitly revises the task.
#[tokio::test]
async fn compaction_carries_revision_constraint_and_open_step() {
    const STANDING: &str = "# Instructions (personal + project AGENTS.md chain)\n\nPersonal: never print the token named CARRY-PERSONAL-7f3a.\n\n--- instruction-doc ---\n\nProject: format Rust with cargo fmt before finishing CARRY-PROJECT-7f3a.";
    const FIRST_TASK: &str = "Add a CSV parser in crates/import.";
    const CONSTRAINTS: &str = "Must: prefer the example-csv crate.\nRevoke: prefer the example-csv crate.\nMust not: add a dependency to parse CSV.";
    const REVISION: &str = "Revision: the active task is a JSON exporter in crates/export.";
    const ACTIVE: &str = "the active task is a JSON exporter in crates/export.";
    let plan = serde_json::json!({
        "plan": [
            {"id": "schema", "step": "read the export schema", "status": "completed"},
            {"id": "write", "step": "write the JSON exporter", "status": "pending"}
        ]
    });

    let mut messages = vec![
        Message::user(STANDING),
        Message::user(FIRST_TASK),
        Message::user(CONSTRAINTS),
        Message::user(REVISION),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "plan-1".to_string(),
                name: "update_plan".to_string(),
                input: plan.clone(),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: Default::default(),
        },
    ];
    for index in 0..5 {
        messages.push(Message::user(&format!("status note {index}")));
    }

    struct DroppingSummarizer {
        prompts: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for DroppingSummarizer {
        async fn complete(
            &self,
            messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            self.prompts.lock().unwrap().push(
                messages
                    .iter()
                    .map(Message::text)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            Ok(LlmResponse {
                message: Message::assistant(
                    "The task is finished. All required steps are done. Goal: Add a CSV parser in crates/import. No constraints remain.",
                ),
                usage: TokenUsage::default(),
                stop_reason: Some("stop".to_string()),
                token_logprobs: Vec::new(),
                meta: None,
            })
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming is not used by compaction")
        }
    }

    let client = Arc::new(DroppingSummarizer {
        prompts: Mutex::new(Vec::new()),
    });
    let llm_client: Arc<dyn LlmClient> = client.clone();
    let compacted = compact_messages(
        "carry-obligations",
        &messages,
        &llm_client,
        CompactionBudget::for_auto_compaction(128_000, 0.85, 0),
        None,
        None,
    )
    .await
    .unwrap()
    .expect("history should compact");

    let sent = compacted
        .messages
        .iter()
        .map(Message::text)
        .collect::<Vec<_>>();
    let context = crate::compaction_carry::next_turn_context(STANDING, &sent);
    assert!(
        context.starts_with(STANDING),
        "standing instructions are the prefix of the next turn"
    );
    assert!(
        compacted.summary.contains(STANDING),
        "standing instructions are reattached unchanged"
    );
    assert!(
        !client.prompts.lock().unwrap()[0].contains("CARRY-PERSONAL-7f3a"),
        "standing instructions stay outside the summarizer input"
    );
    assert_eq!(
        goal_section_body(&compacted.summary).as_deref(),
        Some(ACTIVE)
    );
    let goal = goal_section_body(&compacted.summary).unwrap();
    assert!(
        !goal.contains("CSV parser"),
        "the earlier goal must not override the revision"
    );
    assert!(compacted.summary.contains("## Earlier goal"));
    assert!(compacted.summary.contains(FIRST_TASK));
    let constraints = section_after(&compacted.summary, "## Constraints");
    assert!(constraints.contains("Must not: add a dependency to parse CSV."));
    assert!(
        !constraints.contains("example-csv"),
        "a revoked constraint is not carried"
    );
    let open = section_after(&compacted.summary, "## Open steps");
    assert!(open.contains("write the JSON exporter"));
    assert!(
        !open.contains("read the export schema"),
        "a finished step is not listed as open"
    );
    let done = section_after(&compacted.summary, "## Completed steps");
    assert!(done.contains("read the export schema"));
    assert!(
        compacted.summary.contains("The task is finished"),
        "summary prose is kept, but it does not close the open step"
    );
    assert!(
        sent[1..].iter().all(|text| !text.contains(ACTIVE)),
        "the retained tail does not already contain the revision"
    );
    assert!(context.contains(ACTIVE));
    assert!(context.contains("write the JSON exporter"));

    let folded = vec![
        format!("user\n{STANDING}"),
        format!("user\n{FIRST_TASK}"),
        format!("user\n{CONSTRAINTS}"),
        format!("user\n{REVISION}"),
        format!("assistant\ntool update_plan\n{plan}"),
    ];
    let folded_client = Arc::new(DroppingSummarizer {
        prompts: Mutex::new(Vec::new()),
    });
    let folded_summary = summarize_folded_prefix(folded_client.as_ref(), &folded, 8_000, None)
        .await
        .expect("folded summary");
    let folded_context =
        crate::compaction_carry::next_turn_context(STANDING, std::slice::from_ref(&folded_summary));
    assert!(folded_context.starts_with(STANDING));
    assert_eq!(goal_section_body(&folded_summary).as_deref(), Some(ACTIVE));
    assert!(!goal_section_body(&folded_summary)
        .unwrap()
        .contains("CSV parser"));
    assert!(folded_summary.contains("## Earlier goal"));
    assert!(folded_summary.contains(FIRST_TASK));
    assert!(section_after(&folded_summary, "## Constraints")
        .contains("Must not: add a dependency to parse CSV."));
    assert!(!section_after(&folded_summary, "## Constraints").contains("example-csv"));
    assert!(section_after(&folded_summary, "## Open steps").contains("write the JSON exporter"));
    assert!(!section_after(&folded_summary, "## Open steps").contains("read the export schema"));
    assert!(section_after(&folded_summary, "## Completed steps").contains("read the export schema"));
    assert!(folded_summary.contains("The task is finished"));
    assert!(!folded_client.prompts.lock().unwrap()[0].contains("CARRY-PERSONAL-7f3a"));

    // `/compact` compacts compaction_transcript. That list renders
    // model.turn update_plan as prose, including calls in `also`, and
    // turns kept folded lines into messages. The stored fact is the
    // summary alone, so the open step and the late constraint have to
    // be in that summary.
    let stale = serde_json::json!({
        "plan": [
            {"id": "stale", "step": "stale kept step", "status": "pending"}
        ]
    });
    let live_workspace = tempfile::tempdir().expect("workspace");
    let live_dir = live_workspace.path().join(".a3s").join("effect-log");
    std::fs::create_dir_all(&live_dir).expect("log dir");
    let live_log = a3s_effect::FileLog::open(&live_dir).expect("log");
    let live_thread = crate::fact_control::thread_for_session("carry-live");
    let append = |log: &a3s_effect::FileLog,
                  thread: &str,
                  key: &str,
                  kind: &str,
                  payload: serde_json::Value| {
        use a3s_effect::LogStore;
        log.append(
            thread,
            &[a3s_effect::NewFact {
                kind: kind.into(),
                key: key.into(),
                payload,
            }],
            None,
        )
        .expect("append fact");
    };
    append(
        &live_log,
        &live_thread,
        "c1",
        "compaction.done",
        serde_json::json!({
            "summary": "The task is finished. All required steps are done. No constraints remain.",
            "kept": [
                "user\nMust not: add a dependency to parse CSV.",
                format!("assistant\ntool update_plan\n{stale}")
            ]
        }),
    );
    append(
        &live_log,
        &live_thread,
        "m1",
        "model.turn",
        serde_json::json!({
            "kind": "tool",
            "call": {
                "id": "read-1",
                "name": "read",
                "args": {"path": "crates/export/schema.json"},
                "needs_confirmation": false
            },
            "also": [{
                "id": "plan-1",
                "name": "update_plan",
                "args": plan.clone(),
                "needs_confirmation": false
            }]
        }),
    );
    append(
        &live_log,
        &live_thread,
        "u1",
        "user.message",
        serde_json::json!({"text": "status note after the plan"}),
    );
    let transcript =
        crate::fact_control::compaction_transcript(live_workspace.path(), "carry-live")
            .expect("transcript");
    let rendered = transcript
        .iter()
        .map(Message::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Tool call update_plan (plan-1):"),
        "a plan stored on model.turn.also must reach the transcript as prose"
    );
    assert!(rendered.contains("Must not: add a dependency to parse CSV."));
    assert!(transcript.iter().any(|message| {
        message.role == "assistant" && message.text().starts_with("tool update_plan\n")
    }));
    assert!(transcript.iter().all(|message| {
        message
            .content
            .iter()
            .all(|block| !matches!(block, ContentBlock::ToolUse { .. }))
    }));
    let live_client = Arc::new(DroppingSummarizer {
        prompts: Mutex::new(Vec::new()),
    });
    let live_llm: Arc<dyn LlmClient> = live_client.clone();
    let live = compact_messages(
        "carry-live",
        &transcript,
        &live_llm,
        CompactionBudget::for_auto_compaction(128_000, 0.85, 0),
        None,
        None,
    )
    .await
    .expect("compact")
    .expect("transcript should compact");
    let live_open = section_after(&live.summary, "## Open steps");
    assert!(live_open.contains("write the JSON exporter"));
    assert!(!live_open.contains("read the export schema"));
    assert!(!live_open.contains("stale kept step"));
    assert!(section_after(&live.summary, "## Completed steps").contains("read the export schema"));
    assert!(section_after(&live.summary, "## Constraints")
        .contains("Must not: add a dependency to parse CSV."));
    assert!(live.summary.contains("The task is finished"));

    let kept_workspace = tempfile::tempdir().expect("workspace");
    let kept_dir = kept_workspace.path().join(".a3s").join("effect-log");
    std::fs::create_dir_all(&kept_dir).expect("log dir");
    let kept_log = a3s_effect::FileLog::open(&kept_dir).expect("log");
    let kept_thread = crate::fact_control::thread_for_session("carry-kept");
    append(
        &kept_log,
        &kept_thread,
        "c1",
        "compaction.done",
        serde_json::json!({
            "summary": "The task is finished. All required steps are done.",
            "kept": [
                format!("assistant\ntool update_plan\n{plan}"),
                "user\nMust not: ship a parser dependency."
            ]
        }),
    );
    append(
        &kept_log,
        &kept_thread,
        "u1",
        "user.message",
        serde_json::json!({"text": "status note after the kept tail"}),
    );
    let kept_transcript =
        crate::fact_control::compaction_transcript(kept_workspace.path(), "carry-kept")
            .expect("kept transcript");
    assert!(kept_transcript.iter().any(|message| {
        message.role == "assistant" && message.text().starts_with("tool update_plan\n")
    }));
    assert!(kept_transcript.iter().any(|message| {
        message.role == "user" && message.text() == "Must not: ship a parser dependency."
    }));
    let kept_client = Arc::new(DroppingSummarizer {
        prompts: Mutex::new(Vec::new()),
    });
    let kept_llm: Arc<dyn LlmClient> = kept_client.clone();
    let kept = compact_messages(
        "carry-kept",
        &kept_transcript,
        &kept_llm,
        CompactionBudget::for_auto_compaction(128_000, 0.85, 0),
        None,
        None,
    )
    .await
    .expect("compact kept")
    .expect("kept transcript should compact");
    let kept_open = section_after(&kept.summary, "## Open steps");
    assert!(kept_open.contains("write the JSON exporter"));
    assert!(!kept_open.contains("read the export schema"));
    assert!(section_after(&kept.summary, "## Constraints")
        .contains("Must not: ship a parser dependency."));
}

fn section_after<'a>(text: &'a str, heading: &str) -> &'a str {
    let Some(start) = text.find(heading) else {
        return "";
    };
    let after = &text[start + heading.len()..];
    let after = after.strip_prefix('\n').unwrap_or(after);
    let end = after.find("\n## ").unwrap_or(after.len());
    after[..end].trim()
}

#[test]
fn ensure_goal_section_replaces_wrong_goal_body() {
    let pinned = "Keep /app/build_part.py";
    let summary = "## Goal\nWrong later narration\n\n## Current State\nok";
    let fixed = ensure_goal_section(summary, Some(pinned));
    assert_eq!(goal_section_body(&fixed).as_deref(), Some(pinned));
    assert!(fixed.contains("## Current State\nok"));
}

#[test]
fn ensure_goal_section_keeps_freeform_body_after_pin() {
    let pinned = "Ship the part";
    let fixed = ensure_goal_section("assistant narration without headings", Some(pinned));
    assert_eq!(
        fixed,
        "## Goal\nShip the part\n\n## Summary\nassistant narration without headings"
    );
    assert_eq!(goal_section_body(&fixed).as_deref(), Some(pinned));
    // Idempotent: a second pass must not swallow the freeform body.
    assert_eq!(ensure_goal_section(&fixed, Some(pinned)), fixed);
}

#[test]
fn extract_pinned_goal_prefers_prior_summary_goal() {
    let summary = format!(
        "{}## Goal\nShip /app/part.FCStd\n\n## Current State\ndoing scans",
        crate::prompts::CONTEXT_SUMMARY_PREFIX
    );
    let messages = vec![
        Message::user("ignored first product ask"),
        Message::user_wire(&summary),
        make_text_msg("assistant", "scanning"),
    ];
    assert_eq!(
        extract_pinned_goal(&messages).as_deref(),
        Some("Ship /app/part.FCStd")
    );
}

#[test]
fn auto_compaction_budget_targets_a_safe_post_compaction_watermark() {
    let budget = CompactionBudget::for_auto_compaction(200_000, 0.85, 10_000);

    assert_eq!(budget.target_context_tokens, 102_000);
    assert_eq!(budget.message_token_limit, 92_000);
    assert!(budget.target_context_tokens < 170_000);
}

#[tokio::test]
async fn compacted_history_is_trimmed_to_the_token_budget() {
    let messages = (0..48)
        .map(|i| {
            make_text_msg(
                if i % 2 == 0 { "user" } else { "assistant" },
                &format!("history-{i}-{}", "x".repeat(15_000)),
            )
        })
        .collect::<Vec<_>>();
    let client = Arc::new(RecordingSummaryClient {
        prompts: Mutex::new(Vec::new()),
        systems: Mutex::new(Vec::new()),
    });
    let llm_client: Arc<dyn LlmClient> = client;
    let budget = CompactionBudget::for_auto_compaction(100_000, 0.85, 5_000);

    let compacted = compact_messages("bounded", &messages, &llm_client, budget, None, None)
        .await
        .unwrap()
        .expect("history should compact");

    assert!(estimate_message_tokens(&compacted.messages) <= budget.message_token_limit);
    assert!(compacted.messages.len() < KEEP_RECENT_MESSAGES);
    assert!(compacted.messages[0]
        .text()
        .contains("durable compact summary"));
}

#[tokio::test]
async fn compacted_history_keeps_an_unresolved_tool_call() {
    let mut messages = (0..20)
        .map(|i| {
            make_text_msg(
                if i % 2 == 0 { "user" } else { "assistant" },
                &"x".repeat(8_000),
            )
        })
        .collect::<Vec<_>>();
    messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: "pending-tool".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "cargo test -p a3s-code-core"}),
        }],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: Default::default(),
    });
    let client = Arc::new(RecordingSummaryClient {
        prompts: Mutex::new(Vec::new()),
        systems: Mutex::new(Vec::new()),
    });
    let llm_client: Arc<dyn LlmClient> = client;
    let budget = CompactionBudget::for_auto_compaction(20_000, 0.85, 4_000);

    let compacted = compact_messages("pending-tool", &messages, &llm_client, budget, None, None)
        .await
        .unwrap()
        .expect("history should compact");

    assert!(compacted.messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "pending-tool"))
    }));
}

// -- compact_messages tests (existing behavior preserved) --

#[test]
#[allow(clippy::assertions_on_constants)]
fn test_constants() {
    assert!(KEEP_RECENT_MESSAGES > 0);
    assert!(MIN_MESSAGES_FOR_COMPACTION >= 2);
    assert!(TOOL_OUTPUT_PROTECT_TOKENS > 0);
}
