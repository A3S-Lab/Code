use a3s_code_core::planning::{AgentGoal, Complexity, ExecutionPlan, TaskStatus};
use a3s_code_core::queue::SessionLane;
use a3s_code_core::verification::VerificationSummary;
use a3s_code_core::{
    run_event_envelope_v1, AgentEvent, AgentEventProjectionV1, CognitiveContextLimits,
    CognitiveKnowledgeBindingV1, CognitivePackageBindingV1, EventEnvelopeV1, TokenUsage,
    AGENT_EVENT_TYPES_V1,
};
use serde_json::json;

struct EventCase {
    event_type: &'static str,
    event: AgentEvent,
    payload_key: &'static str,
    payload_value: serde_json::Value,
}

fn case(
    event_type: &'static str,
    event: AgentEvent,
    payload_key: &'static str,
    payload_value: serde_json::Value,
) -> EventCase {
    EventCase {
        event_type,
        event,
        payload_key,
        payload_value,
    }
}

fn cognitive_binding() -> CognitivePackageBindingV1 {
    let generation_digest =
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
    let knowledge = CognitiveKnowledgeBindingV1::new(
        "domain-knowledge",
        "0.2",
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
        7,
        generation_digest,
    )
    .unwrap();
    CognitivePackageBindingV1::new(
        "contra-sense/handbook",
        "0.1.0",
        7,
        generation_digest,
        "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
        knowledge,
        CognitiveContextLimits::default(),
    )
    .unwrap()
}

fn representative_events() -> Vec<EventCase> {
    let goal = AgentGoal::new("ship");
    let goal_payload = serde_json::to_value(&goal).expect("representative goal must serialize");
    vec![
        case(
            "agent_start",
            AgentEvent::Start {
                prompt: "hello".into(),
            },
            "prompt",
            json!("hello"),
        ),
        case(
            "agent_mode_changed",
            AgentEvent::AgentModeChanged {
                mode: "planning".into(),
                agent: "planner".into(),
                description: "plans".into(),
            },
            "mode",
            json!("planning"),
        ),
        case(
            "turn_start",
            AgentEvent::TurnStart { turn: 2 },
            "turn",
            json!(2),
        ),
        case(
            "text_delta",
            AgentEvent::TextDelta {
                text: "chunk".into(),
            },
            "text",
            json!("chunk"),
        ),
        case(
            "reasoning_delta",
            AgentEvent::ReasoningDelta {
                text: "because".into(),
            },
            "text",
            json!("because"),
        ),
        case(
            "tool_start",
            AgentEvent::ToolStart {
                id: "tool-1".into(),
                name: "read".into(),
            },
            "name",
            json!("read"),
        ),
        case(
            "tool_input_delta",
            AgentEvent::ToolInputDelta {
                id: Some("tool-1".into()),
                delta: r#"{"path":""#.into(),
            },
            "delta",
            json!(r#"{"path":""#),
        ),
        case(
            "tool_execution_start",
            AgentEvent::ToolExecutionStart {
                id: "tool-1".into(),
                name: "read".into(),
                args: json!({ "path": "README.md" }),
            },
            "args",
            json!({ "path": "README.md" }),
        ),
        case(
            "tool_end",
            AgentEvent::ToolEnd {
                id: "tool-1".into(),
                name: "read".into(),
                args: Some(json!({ "path": "README.md" })),
                output: "contents".into(),
                exit_code: 0,
                metadata: Some(json!({ "artifact": "a1" })),
                error_kind: None,
            },
            "metadata",
            json!({ "artifact": "a1" }),
        ),
        case(
            "tool_output_delta",
            AgentEvent::ToolOutputDelta {
                id: "tool-1".into(),
                name: "bash".into(),
                delta: "line".into(),
            },
            "delta",
            json!("line"),
        ),
        case(
            "turn_end",
            AgentEvent::TurnEnd {
                turn: 2,
                usage: TokenUsage {
                    total_tokens: 12,
                    ..TokenUsage::default()
                },
            },
            "turn",
            json!(2),
        ),
        case(
            "agent_end",
            AgentEvent::End {
                text: "done".into(),
                usage: TokenUsage {
                    total_tokens: 20,
                    ..TokenUsage::default()
                },
                verification_summary: Box::new(VerificationSummary::from_reports(&[])),
                meta: None,
            },
            "text",
            json!("done"),
        ),
        case(
            "error",
            AgentEvent::Error {
                message: "failed".into(),
            },
            "message",
            json!("failed"),
        ),
        case(
            "confirmation_required",
            AgentEvent::ConfirmationRequired {
                tool_id: "tool-1".into(),
                tool_name: "bash".into(),
                args: json!({ "command": "rm" }),
                timeout_ms: 30_000,
            },
            "timeout_ms",
            json!(30_000),
        ),
        case(
            "confirmation_received",
            AgentEvent::ConfirmationReceived {
                tool_id: "tool-1".into(),
                approved: true,
                reason: Some("reviewed".into()),
            },
            "approved",
            json!(true),
        ),
        case(
            "confirmation_timeout",
            AgentEvent::ConfirmationTimeout {
                tool_id: "tool-1".into(),
                action_taken: "rejected".into(),
            },
            "action_taken",
            json!("rejected"),
        ),
        case(
            "external_task_pending",
            AgentEvent::ExternalTaskPending {
                task_id: "task-1".into(),
                session_id: "session-1".into(),
                lane: SessionLane::Execute,
                command_type: "bash".into(),
                payload: json!({ "command": "pwd" }),
                timeout_ms: 60_000,
            },
            "lane",
            json!("Execute"),
        ),
        case(
            "external_task_completed",
            AgentEvent::ExternalTaskCompleted {
                task_id: "task-1".into(),
                session_id: "session-1".into(),
                success: true,
            },
            "success",
            json!(true),
        ),
        case(
            "permission_denied",
            AgentEvent::PermissionDenied {
                tool_id: "tool-1".into(),
                tool_name: "bash".into(),
                args: json!({ "command": "sudo" }),
                reason: "policy".into(),
            },
            "reason",
            json!("policy"),
        ),
        case(
            "context_resolving",
            AgentEvent::ContextResolving {
                providers: vec!["memory".into()],
            },
            "providers",
            json!(["memory"]),
        ),
        case(
            "context_resolved",
            AgentEvent::ContextResolved {
                total_items: 3,
                total_tokens: 40,
            },
            "total_items",
            json!(3),
        ),
        case(
            "cognitive_context_bound",
            AgentEvent::CognitiveContextBound {
                binding: cognitive_binding(),
            },
            "binding",
            serde_json::to_value(cognitive_binding()).unwrap(),
        ),
        case(
            "command_dead_lettered",
            AgentEvent::CommandDeadLettered {
                command_id: "command-1".into(),
                command_type: "bash".into(),
                lane: "execute".into(),
                error: "timeout".into(),
                attempts: 3,
            },
            "attempts",
            json!(3),
        ),
        case(
            "command_retry",
            AgentEvent::CommandRetry {
                command_id: "command-1".into(),
                command_type: "bash".into(),
                lane: "execute".into(),
                attempt: 2,
                delay_ms: 100,
            },
            "delay_ms",
            json!(100),
        ),
        case(
            "queue_alert",
            AgentEvent::QueueAlert {
                level: "warning".into(),
                alert_type: "depth".into(),
                message: "queue is deep".into(),
            },
            "level",
            json!("warning"),
        ),
        case(
            "task_updated",
            AgentEvent::TaskUpdated {
                session_id: "session-1".into(),
                tasks: vec![],
            },
            "session_id",
            json!("session-1"),
        ),
        case(
            "memory_stored",
            AgentEvent::MemoryStored {
                memory_id: "memory-1".into(),
                memory_type: "fact".into(),
                importance: 0.75,
                tags: vec!["rust".into()],
            },
            "memory_id",
            json!("memory-1"),
        ),
        case(
            "memory_recalled",
            AgentEvent::MemoryRecalled {
                memory_id: "memory-1".into(),
                content: "remembered".into(),
                relevance: 0.9,
            },
            "content",
            json!("remembered"),
        ),
        case(
            "memories_searched",
            AgentEvent::MemoriesSearched {
                query: Some("rust".into()),
                tags: vec!["language".into()],
                result_count: 4,
            },
            "result_count",
            json!(4),
        ),
        case(
            "memory_cleared",
            AgentEvent::MemoryCleared {
                tier: "working".into(),
                count: 2,
            },
            "count",
            json!(2),
        ),
        case(
            "subagent_start",
            AgentEvent::SubagentStart {
                task_id: "task-1".into(),
                session_id: "child-1".into(),
                parent_session_id: "parent-1".into(),
                agent: "explore".into(),
                description: "inspect".into(),
                started_ms: 123,
            },
            "parent_session_id",
            json!("parent-1"),
        ),
        case(
            "subagent_progress",
            AgentEvent::SubagentProgress {
                task_id: "task-1".into(),
                session_id: "child-1".into(),
                status: "working".into(),
                metadata: json!({ "percent": 50 }),
            },
            "metadata",
            json!({ "percent": 50 }),
        ),
        case(
            "subagent_end",
            AgentEvent::SubagentEnd {
                task_id: "task-1".into(),
                session_id: "child-1".into(),
                agent: "explore".into(),
                output: "result".into(),
                success: true,
                finished_ms: 456,
            },
            "finished_ms",
            json!(456),
        ),
        case(
            "planning_start",
            AgentEvent::PlanningStart {
                prompt: "make a plan".into(),
            },
            "prompt",
            json!("make a plan"),
        ),
        case(
            "planning_end",
            AgentEvent::PlanningEnd {
                plan: ExecutionPlan::new("ship", Complexity::Simple),
                estimated_steps: 1,
            },
            "estimated_steps",
            json!(1),
        ),
        case(
            "step_start",
            AgentEvent::StepStart {
                step_id: "step-1".into(),
                description: "implement".into(),
                step_number: 1,
                total_steps: 2,
            },
            "description",
            json!("implement"),
        ),
        case(
            "step_end",
            AgentEvent::StepEnd {
                step_id: "step-1".into(),
                status: TaskStatus::Completed,
                step_number: 1,
                total_steps: 2,
            },
            "status",
            json!("completed"),
        ),
        case(
            "goal_extracted",
            AgentEvent::GoalExtracted { goal },
            "goal",
            goal_payload,
        ),
        case(
            "goal_progress",
            AgentEvent::GoalProgress {
                goal: "ship".into(),
                progress: 0.5,
                completed_steps: 1,
                total_steps: 2,
            },
            "progress",
            json!(0.5),
        ),
        case(
            "goal_achieved",
            AgentEvent::GoalAchieved {
                goal: "ship".into(),
                total_steps: 2,
                duration_ms: 500,
            },
            "duration_ms",
            json!(500),
        ),
        case(
            "context_compacted",
            AgentEvent::ContextCompacted {
                session_id: "session-1".into(),
                before_messages: 20,
                after_messages: 8,
                percent_before: 0.9,
                summary: Some("continue from the verified state".into()),
            },
            "after_messages",
            json!(8),
        ),
        case(
            "persistence_failed",
            AgentEvent::PersistenceFailed {
                session_id: "session-1".into(),
                operation: "save".into(),
                error: "disk full".into(),
            },
            "operation",
            json!("save"),
        ),
        case(
            "budget_threshold_hit",
            AgentEvent::BudgetThresholdHit {
                resource: "llm_tokens".into(),
                kind: "soft".into(),
                consumed: 80.0,
                limit: 100.0,
                message: Some("near limit".into()),
            },
            "kind",
            json!("soft"),
        ),
        case(
            "passivation_requested",
            AgentEvent::PassivationRequested {
                reason: "node_drain".into(),
                deadline_ms: Some(1_000),
            },
            "deadline_ms",
            json!(1_000),
        ),
        case(
            "peer_invocation",
            AgentEvent::PeerInvocation {
                from_session_id: "peer-1".into(),
                from_tenant_id: Some("tenant-1".into()),
                correlation_id: Some("correlation-1".into()),
            },
            "from_session_id",
            json!("peer-1"),
        ),
    ]
}

#[test]
fn envelope_v1_uses_the_canonical_agent_event_name_and_payload() {
    let event = AgentEvent::Start {
        prompt: "hello".to_string(),
    };

    let envelope = EventEnvelopeV1::try_from(&event).expect("event should project to wire v1");

    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.event_type, "agent_start");
    assert_eq!(envelope.payload, json!({ "prompt": "hello" }));
    assert_eq!(envelope.metadata, None);
}

#[test]
fn every_agent_event_variant_has_a_versioned_wire_case_with_representative_payload() {
    let cases = representative_events();
    let case_types: Vec<_> = cases.iter().map(|case| case.event_type).collect();

    assert_eq!(case_types, AGENT_EVENT_TYPES_V1);

    for case in cases {
        let envelope =
            EventEnvelopeV1::try_from(&case.event).expect("every runtime variant must serialize");
        assert_eq!(envelope.version, 1, "{}", case.event_type);
        assert_eq!(envelope.event_type, case.event_type);
        assert_eq!(
            envelope.payload.get(case.payload_key),
            Some(&case.payload_value),
            "representative payload mismatch for {}",
            case.event_type
        );
        assert!(envelope.payload.get("type").is_none());

        let encoded = serde_json::to_value(&envelope).expect("envelope should serialize");
        assert_eq!(encoded["type"], case.event_type);
        assert_eq!(encoded["version"], 1);
        assert_eq!(encoded["payload"][case.payload_key], case.payload_value);

        let decoded: EventEnvelopeV1 =
            serde_json::from_value(encoded).expect("envelope should round-trip");
        assert_eq!(decoded, envelope);
    }
}

#[test]
fn sdk_projection_derives_reasoning_and_terminal_fields_from_the_envelope() {
    let reasoning = AgentEventProjectionV1::try_from(&AgentEvent::ReasoningDelta {
        text: "chain".into(),
    })
    .expect("reasoning event should project");
    assert_eq!(reasoning.event_type, "reasoning_delta");
    assert_eq!(reasoning.text.as_deref(), Some("chain"));

    let terminal = AgentEventProjectionV1::try_from(&AgentEvent::End {
        text: "done".into(),
        usage: TokenUsage {
            total_tokens: 42,
            ..TokenUsage::default()
        },
        verification_summary: Box::new(VerificationSummary::from_reports(&[])),
        meta: None,
    })
    .expect("terminal event should project");
    assert_eq!(terminal.event_type, "agent_end");
    assert_eq!(terminal.text.as_deref(), Some("done"));
    assert_eq!(terminal.total_tokens, Some(42));
    assert!(terminal.verification_summary_json.is_some());
}

#[test]
fn sdk_projection_preserves_authoritative_tool_arguments() {
    let args = json!({ "path": "README.md" });
    let execution = AgentEventProjectionV1::try_from(&AgentEvent::ToolExecutionStart {
        id: "tool-1".into(),
        name: "read".into(),
        args: args.clone(),
    })
    .expect("tool execution event should project");
    assert_eq!(execution.tool_id.as_deref(), Some("tool-1"));
    assert_eq!(execution.tool_name.as_deref(), Some("read"));
    assert_eq!(execution.payload["args"], args);
    assert_eq!(execution.data_json, Some(execution.payload_json.clone()));

    let completed = AgentEventProjectionV1::try_from(&AgentEvent::ToolEnd {
        id: "tool-1".into(),
        name: "read".into(),
        args: Some(args.clone()),
        output: "contents".into(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    })
    .expect("tool completion event should project");
    assert_eq!(completed.payload["args"], args);
    assert_eq!(completed.data_json, Some(completed.payload_json.clone()));
}

#[test]
fn tool_protocol_additions_accept_pre_upgrade_events() {
    let input_delta: AgentEvent = serde_json::from_value(json!({
        "type": "tool_input_delta",
        "delta": "{\"path\":"
    }))
    .expect("tool input events persisted before call ids must remain readable");
    assert!(matches!(
        input_delta,
        AgentEvent::ToolInputDelta { id: None, .. }
    ));

    let completed: AgentEvent = serde_json::from_value(json!({
        "type": "tool_end",
        "id": "tool-1",
        "name": "read",
        "output": "contents",
        "exit_code": 0,
        "metadata": null,
        "error_kind": null
    }))
    .expect("tool completions persisted before authoritative args must remain readable");
    assert!(matches!(completed, AgentEvent::ToolEnd { args: None, .. }));
}

#[test]
fn projection_v1_preserves_unknown_future_events_without_relabeling() {
    let envelope: EventEnvelopeV1 = serde_json::from_value(json!({
        "version": 1,
        "type": "future_event",
        "payload": { "opaque": [1, 2, 3] },
        "metadata": { "correlation_id": "future-1" }
    }))
    .expect("future event should remain valid inside envelope v1");

    let projection = AgentEventProjectionV1::from(envelope);

    assert_eq!(projection.event_type, "future_event");
    assert_eq!(projection.payload, json!({ "opaque": [1, 2, 3] }));
    assert_eq!(
        projection.metadata,
        Some(json!({ "correlation_id": "future-1" }))
    );
    assert_eq!(projection.payload_json, r#"{"opaque":[1,2,3]}"#);
    assert_eq!(
        projection.metadata_json.as_deref(),
        Some(r#"{"correlation_id":"future-1"}"#)
    );
    assert_eq!(projection.data_json, Some(projection.payload_json.clone()));
}

#[test]
fn envelope_v1_rejects_a_different_protocol_version() {
    let error = serde_json::from_value::<EventEnvelopeV1>(json!({
        "version": 2,
        "type": "future_event",
        "payload": { "opaque": true }
    }))
    .expect_err("a v2 envelope must not silently deserialize as v1");

    assert!(error.to_string().contains("expected 1"));
}

#[test]
fn persisted_run_event_uses_the_live_v1_envelope_shape() {
    let record = a3s_code_core::run::RunEventRecord {
        sequence: 7,
        timestamp_ms: 1_700_000_000_000,
        event: AgentEvent::TextDelta {
            text: "hello".to_string(),
        },
    };

    let envelope = run_event_envelope_v1(&record, "run-1", "session-1").unwrap();
    assert_eq!(envelope.version, 1);
    assert_eq!(envelope.event_type, "text_delta");
    assert_eq!(envelope.payload, json!({"text": "hello"}));
    assert_eq!(
        envelope.metadata,
        Some(json!({
            "run_id": "run-1",
            "session_id": "session-1",
            "sequence": 7,
            "timestamp_ms": 1_700_000_000_000u64,
        }))
    );
}
