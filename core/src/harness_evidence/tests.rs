use super::*;
use crate::agent::AgentConfig;
use crate::llm::structured::{ResponseFormat, StructuredDirective};
use crate::llm::{ContentBlock, Message, TokenUsage, ToolDefinition, ToolResultContentField};
use crate::workspace::WorkspaceServices;

fn source(workspace: &std::path::Path) -> RunCapabilityEvidenceSource {
    let services = WorkspaceServices::local(workspace);
    RunCapabilityEvidenceSource::from_agent(&AgentConfig::default(), services, false, false)
}

fn search_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search".to_string(),
        description: "Search the workspace".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "mode": { "enum": ["semantic", "hybrid"] } }
        }),
    }
}

#[test]
fn public_evidence_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<HarnessEvidenceError>();
    assert_send_sync::<ModelInputKindV1>();
    assert_send_sync::<WorkspaceCapabilitySnapshotV1>();
    assert_send_sync::<RunPolicyCeilingSnapshotV1>();
    assert_send_sync::<WorkspaceRetrievalCapabilitySnapshotV1>();
    assert_send_sync::<RunCapabilitySnapshotV1>();
    assert_send_sync::<ModelInputSnapshotV1>();
    assert_send_sync::<ToolResultContextUsageV1>();
    assert_send_sync::<ModelUsageSnapshotV1>();
}

#[test]
fn evidence_is_stable_redacted_and_sensitive_to_actual_input() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let tools = vec![search_tool()];
    let messages = vec![Message::user("top-secret-model-input")];
    let (first_capability, first_input, _) = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &messages,
                Some("private-system"),
                &tools,
                None,
                17,
            ),
        )
        .unwrap();
    let (second_capability, second_input, _) = source
        .capture(
            2,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &messages,
                Some("private-system"),
                &tools,
                None,
                17,
            ),
        )
        .unwrap();

    first_capability.validate().unwrap();
    first_input.validate_against(&first_capability).unwrap();
    assert_eq!(
        first_capability.snapshot_digest,
        second_capability.snapshot_digest
    );
    assert_ne!(first_input.snapshot_digest, second_input.snapshot_digest);
    assert_eq!(first_input.input_digest, second_input.input_digest);
    assert_eq!(
        first_input.system_bytes,
        serde_json::to_vec("private-system").unwrap().len() as u64
    );
    assert_eq!(
        first_input.tool_definitions_digest,
        first_capability.model_visible_tools_digest
    );

    let encoded = serde_json::to_string(&(first_capability, first_input)).unwrap();
    assert!(!encoded.contains("top-secret-model-input"));
    assert!(!encoded.contains("private-system"));

    let changed = source
        .capture(
            3,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("different")],
                Some("private-system"),
                &tools,
                None,
                4,
            ),
        )
        .unwrap()
        .1;
    assert_ne!(second_input.input_digest, changed.input_digest);
}

#[test]
fn model_input_v1_wire_shape_stays_stable_when_usage_evidence_expands() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let (_, input, tool_results) = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::tool_result("tool-1", "private result", false)],
                None,
                &[],
                None,
                4,
            ),
        )
        .unwrap();
    let encoded = serde_json::to_value(&input).unwrap();
    let actual = encoded
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = [
        "schema",
        "callSequence",
        "kind",
        "messageCount",
        "contentBlockCount",
        "imageBlockCount",
        "toolResultCount",
        "toolCount",
        "retrievalResultCount",
        "retrievalResultBytes",
        "retrievalResultsDigest",
        "systemBytes",
        "messagePayloadBytes",
        "toolDefinitionBytes",
        "structuredOutputBytes",
        "payloadBytes",
        "estimatedPromptTokens",
        "messagesDigest",
        "systemDigest",
        "toolDefinitionsDigest",
        "structuredOutputDigest",
        "inputDigest",
        "capabilitySnapshotDigest",
        "snapshotDigest",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(actual, expected);
    let decoded: ModelInputSnapshotV1 = serde_json::from_value(encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, input);
    assert_eq!(tool_results.total_count, 1);
}

#[test]
fn model_input_identifies_semantic_tool_results_without_retaining_them() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "search-1".to_string(),
                content: ToolResultContentField::Text("result before call".to_string()),
                is_error: Some(false),
            }],
            reasoning_content: None,
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "search-1".to_string(),
                name: "search".to_string(),
                input: serde_json::json!({"mode": "hybrid", "query": "secret query"}),
            }],
            reasoning_content: None,
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "search-1".to_string(),
                content: ToolResultContentField::Text(
                    "private verified workspace source".to_string(),
                ),
                is_error: Some(false),
            }],
            reasoning_content: None,
        },
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "search-1".to_string(),
                name: "search".to_string(),
                input: serde_json::json!({"mode": "grep", "query": "reuse"}),
            }],
            reasoning_content: None,
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "search-1".to_string(),
                content: ToolResultContentField::Text("non-retrieval reused id".to_string()),
                is_error: Some(false),
            }],
            reasoning_content: None,
        },
    ];
    let input = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Streaming,
                &messages,
                None,
                &[search_tool()],
                None,
                24,
            ),
        )
        .unwrap()
        .1;

    assert_eq!(input.tool_result_count, 3);
    assert_eq!(input.retrieval_result_count, 1);
    assert!(input.retrieval_result_bytes > 0);
    assert!(input.retrieval_results_digest.is_some());
    let encoded = serde_json::to_string(&input).unwrap();
    assert!(!encoded.contains("private verified workspace source"));
    assert!(!encoded.contains("secret query"));
    assert!(!encoded.contains("result before call"));
    assert!(!encoded.contains("non-retrieval reused id"));
}

#[test]
fn model_usage_quantifies_repeated_tool_result_content_without_retaining_it() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let repeated = "private repeated tool output";
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "read-1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "one.rs"}),
            }],
            reasoning_content: None,
        },
        Message::tool_result("read-1", repeated, false),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "read-2".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "two.rs"}),
            }],
            reasoning_content: None,
        },
        Message::tool_result("read-2", repeated, false),
        Message::tool_result("read-3", "unique tool output", false),
    ];
    let (_, input, tool_results) = source
        .capture(
            1,
            ModelCallObservation::new(ModelInputKindV1::Completion, &messages, None, &[], None, 32),
        )
        .unwrap();
    let usage =
        ModelUsageSnapshotV1::from_input(&input, &tool_results, &TokenUsage::default()).unwrap();

    input.validate().unwrap();
    assert_eq!(input.tool_result_count, 3);
    assert_eq!(usage.tool_results.unique_count, 2);
    assert_eq!(usage.tool_results.repeated_count, 1);
    assert!(usage.tool_results.content_bytes > usage.tool_results.repeated_content_bytes);
    assert!(usage.tool_results.estimated_tokens > usage.tool_results.repeated_estimated_tokens);
    assert!(usage.tool_results.contents_digest.is_some());
    assert!(usage.tool_results.repeated_contents_digest.is_some());
    let encoded = serde_json::to_string(&(input, usage)).unwrap();
    assert!(!encoded.contains(repeated));
    assert!(!encoded.contains("unique tool output"));
}

#[test]
fn model_usage_binds_client_report_to_the_exact_input_snapshot() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let (capability, input, tool_results) = source
        .capture(
            7,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("private prompt")],
                None,
                &[],
                None,
                13,
            ),
        )
        .unwrap();
    let usage = TokenUsage {
        prompt_tokens: 11,
        completion_tokens: 5,
        total_tokens: 16,
        cache_read_tokens: Some(3),
        cache_write_tokens: Some(2),
    };
    let usage_snapshot = ModelUsageSnapshotV1::from_input(&input, &tool_results, &usage).unwrap();

    usage_snapshot.validate_against(&input).unwrap();
    input.validate_against(&capability).unwrap();
    assert_eq!(usage_snapshot.call_sequence, 7);
    assert_eq!(usage_snapshot.estimated_prompt_tokens, 13);
    assert_eq!(usage_snapshot.reported_prompt_tokens, 11);
    assert_eq!(usage_snapshot.reported_completion_tokens, 5);
    assert_eq!(usage_snapshot.reported_total_tokens, 16);
    assert_eq!(usage_snapshot.reported_cache_read_tokens, Some(3));
    assert_eq!(usage_snapshot.reported_cache_write_tokens, Some(2));

    let different_input = source
        .capture(
            8,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("private prompt")],
                None,
                &[],
                None,
                13,
            ),
        )
        .unwrap()
        .1;
    assert!(matches!(
        usage_snapshot.validate_against(&different_input),
        Err(HarnessEvidenceError::InvalidContents(
            "usage and input call sequences agree"
        ))
    ));

    let mut tampered = usage_snapshot;
    tampered.reported_total_tokens = 17;
    assert!(matches!(
        tampered.validate(),
        Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"))
    ));
}

#[test]
fn validation_rejects_snapshot_tampering() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let (mut capability, mut input, _) = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("hello")],
                None,
                &[],
                None,
                2,
            ),
        )
        .unwrap();
    capability.workspace.write = !capability.workspace.write;
    input.payload_bytes = input.payload_bytes.saturating_add(1);
    assert!(matches!(
        capability.validate(),
        Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"))
    ));
    assert!(matches!(
        input.validate(),
        Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"))
    ));
}

#[test]
fn model_input_excludes_host_only_validation_schema() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let messages = [Message::user("hello")];
    let tools = [search_tool()];
    let directive = StructuredDirective {
        force_tool: Some("search".to_string()),
        response_format: Some(ResponseFormat::JsonSchema {
            name: "answer".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }),
        validation_schema: Some(serde_json::json!({"const": "host-secret-one"})),
    };
    let mut changed_host_schema = directive.clone();
    changed_host_schema.validation_schema = Some(serde_json::json!({"const": "host-secret-two"}));

    let first = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Structured,
                &messages,
                None,
                &tools,
                Some(&directive),
                2,
            ),
        )
        .unwrap()
        .1;
    let changed_host_only = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Structured,
                &messages,
                None,
                &tools,
                Some(&changed_host_schema),
                2,
            ),
        )
        .unwrap()
        .1;

    assert_eq!(first.input_digest, changed_host_only.input_digest);
    assert_eq!(
        first.structured_output_digest,
        changed_host_only.structured_output_digest
    );
    assert_eq!(first.snapshot_digest, changed_host_only.snapshot_digest);

    let mut changed_provider_schema = directive;
    changed_provider_schema.response_format = Some(ResponseFormat::JsonObject);
    let changed_provider_input = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Structured,
                &messages,
                None,
                &tools,
                Some(&changed_provider_schema),
                2,
            ),
        )
        .unwrap()
        .1;
    assert_ne!(first.input_digest, changed_provider_input.input_digest);
}

#[test]
fn validation_reports_shape_invariants_before_digest_mismatch() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let (mut capability, mut input, _) = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("hello")],
                None,
                &[],
                None,
                2,
            ),
        )
        .unwrap();

    capability.retrieval.coverage_bps = 10_001;
    input.call_sequence = 0;
    assert!(matches!(
        capability.validate(),
        Err(HarnessEvidenceError::InvalidContents(
            "retrieval.coverage_bps <= 10_000"
        ))
    ));
    assert!(matches!(
        input.validate(),
        Err(HarnessEvidenceError::InvalidContents(
            "call_sequence is positive"
        ))
    ));

    let mut inconsistent_system = source
        .capture(
            2,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("hello")],
                None,
                &[],
                None,
                2,
            ),
        )
        .unwrap()
        .1;
    inconsistent_system.system_digest = Some(digest_for_test('5'));
    assert!(matches!(
        inconsistent_system.validate(),
        Err(HarnessEvidenceError::InvalidContents(
            "system bytes and digest agree"
        ))
    ));

    let (_, _, mut inconsistent_tool_results) = source
        .capture(
            3,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::tool_result("tool-1", "result", false)],
                None,
                &[],
                None,
                4,
            ),
        )
        .unwrap();
    inconsistent_tool_results.repeated_count = 1;
    assert!(matches!(
        inconsistent_tool_results.validate(),
        Err(HarnessEvidenceError::InvalidContents(
            "unique and repeated Tool-result counts partition Tool results"
        ))
    ));
}

#[test]
fn pair_validation_rejects_a_different_capability_snapshot() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let messages = [Message::user("hello")];
    let tools = [search_tool()];
    let (capability, input, _) = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &messages,
                None,
                &tools,
                None,
                2,
            ),
        )
        .unwrap();
    let different_capability = source
        .capture(
            2,
            ModelCallObservation::new(ModelInputKindV1::Completion, &messages, None, &[], None, 2),
        )
        .unwrap()
        .0;

    input.validate_against(&capability).unwrap();
    assert!(matches!(
        input.validate_against(&different_capability),
        Err(HarnessEvidenceError::DigestMismatch(
            "capability_snapshot_digest"
        ))
    ));
}

#[test]
fn capability_digest_changes_on_readiness_and_generation_drift() {
    let workspace = tempfile::tempdir().unwrap();
    let source = source(workspace.path());
    let baseline = source
        .capture(
            1,
            ModelCallObservation::new(
                ModelInputKindV1::Completion,
                &[Message::user("hello")],
                None,
                &[search_tool()],
                None,
                2,
            ),
        )
        .unwrap()
        .0;
    let building = RunCapabilitySnapshotV1::new(
        baseline.model_visible_tool_count,
        baseline.model_visible_tools_digest.clone(),
        baseline.workspace.clone(),
        baseline.policy.clone(),
        WorkspaceRetrievalCapabilitySnapshotV1 {
            enabled: true,
            phase: WorkspaceRetrievalPhase::Building,
            catalog_revision: 7,
            source_revision: 8,
            vector_revision: 0,
            coverage_bps: 0,
            model_digest: Some(digest_for_test('4')),
        },
    )
    .unwrap();
    let ready = RunCapabilitySnapshotV1::new(
        building.model_visible_tool_count,
        building.model_visible_tools_digest.clone(),
        building.workspace.clone(),
        building.policy.clone(),
        WorkspaceRetrievalCapabilitySnapshotV1 {
            phase: WorkspaceRetrievalPhase::Ready,
            vector_revision: 9,
            coverage_bps: 10_000,
            ..building.retrieval.clone()
        },
    )
    .unwrap();

    assert_ne!(baseline.snapshot_digest, building.snapshot_digest);
    assert_ne!(building.snapshot_digest, ready.snapshot_digest);
    building.validate().unwrap();
    ready.validate().unwrap();
}

fn digest_for_test(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}
