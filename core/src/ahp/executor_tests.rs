use super::*;
use crate::hooks::PreToolUseEvent;
use a3s_ahp::Decision;
use std::sync::Mutex;

struct NoopTransport;

#[async_trait]
impl a3s_ahp::transport::TransportLayer for NoopTransport {
    async fn send_request(
        &self,
        request: a3s_ahp::AhpRequest,
    ) -> a3s_ahp::Result<a3s_ahp::AhpResponse> {
        Ok(a3s_ahp::AhpResponse::success(
            request.id,
            serde_json::json!({}),
        ))
    }

    async fn send_notification(
        &self,
        _notification: a3s_ahp::AhpNotification,
    ) -> a3s_ahp::Result<()> {
        Ok(())
    }

    async fn close(&self) -> a3s_ahp::Result<()> {
        Ok(())
    }
}

struct RecordingTransport {
    notifications: Mutex<Vec<a3s_ahp::AhpNotification>>,
}

#[async_trait]
impl a3s_ahp::transport::TransportLayer for RecordingTransport {
    async fn send_request(
        &self,
        request: a3s_ahp::AhpRequest,
    ) -> a3s_ahp::Result<a3s_ahp::AhpResponse> {
        Ok(a3s_ahp::AhpResponse::success(
            request.id,
            serde_json::json!({"decision": "allow"}),
        ))
    }

    async fn send_notification(
        &self,
        notification: a3s_ahp::AhpNotification,
    ) -> a3s_ahp::Result<()> {
        self.notifications.lock().unwrap().push(notification);
        Ok(())
    }

    async fn close(&self) -> a3s_ahp::Result<()> {
        Ok(())
    }
}

fn make_test_executor() -> AhpHookExecutor {
    let client = Arc::new(AhpClient::new_for_testing(Arc::new(NoopTransport)));
    AhpHookExecutor::new_for_testing(client, 10_000)
}

fn usage(total_tokens: usize) -> crate::llm::TokenUsage {
    crate::llm::TokenUsage {
        prompt_tokens: total_tokens,
        completion_tokens: 0,
        total_tokens,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}

#[tokio::test]
async fn publish_agent_event_sends_runtime_contract_notifications() {
    let transport = Arc::new(RecordingTransport {
        notifications: Mutex::new(Vec::new()),
    });
    let client = Arc::new(AhpClient::new_for_testing(transport.clone()));
    let executor = AhpHookExecutor::new_for_testing(client, 10_000);

    executor
        .publish_agent_event(
            &AgentEvent::Start {
                prompt: "ship it".to_string(),
            },
            "run-1",
            "session-1",
        )
        .await;
    executor
        .publish_agent_event(
            &AgentEvent::End {
                text: "done".to_string(),
                usage: Default::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                meta: None,
            },
            "run-1",
            "session-1",
        )
        .await;

    let notifications = transport.notifications.lock().unwrap();
    assert_eq!(notifications.len(), 3);
    let event_types = notifications
        .iter()
        .map(|notification| {
            let event: a3s_ahp::AhpEvent =
                serde_json::from_value(notification.params.clone()).unwrap();
            event.event_type
        })
        .collect::<Vec<_>>();
    assert_eq!(event_types[0], EventType::RunLifecycle);
    assert_eq!(event_types[1], EventType::RunLifecycle);
    assert_eq!(event_types[2], EventType::Verification);

    let end_event: a3s_ahp::AhpEvent =
        serde_json::from_value(notifications[1].params.clone()).unwrap();
    assert!(end_event.context.is_some());
    assert_eq!(
        end_event
            .context
            .unwrap()
            .session_stats
            .unwrap()
            .total_actions,
        2
    );
}

#[tokio::test]
async fn publish_run_cancelled_sends_cancelled_lifecycle_notification() {
    let transport = Arc::new(RecordingTransport {
        notifications: Mutex::new(Vec::new()),
    });
    let client = Arc::new(AhpClient::new_for_testing(transport.clone()));
    let executor = AhpHookExecutor::new_for_testing(client, 10_000);

    executor
        .publish_run_cancelled("run-1", "session-1", Some("user cancelled"))
        .await;

    let notifications = transport.notifications.lock().unwrap();
    assert_eq!(notifications.len(), 1);
    let event: a3s_ahp::AhpEvent = serde_json::from_value(notifications[0].params.clone()).unwrap();
    assert_eq!(event.event_type, EventType::RunLifecycle);
    let payload: a3s_ahp::RunLifecycleEvent = serde_json::from_value(event.payload).unwrap();
    assert_eq!(payload.status, a3s_ahp::RunStatus::Cancelled);
    assert_eq!(payload.error.as_deref(), Some("user cancelled"));
}

#[tokio::test]
async fn runtime_snapshot_tracks_tools_pending_actions_and_tokens() {
    let executor = make_test_executor();

    executor
        .publish_agent_event(
            &AgentEvent::ToolStart {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
            },
            "run-1",
            "session-1",
        )
        .await;
    let snapshot = executor.runtime_snapshot();
    assert_eq!(snapshot.active_tools, 1);
    assert_eq!(snapshot.current_state, "running_tools");

    executor
        .publish_agent_event(
            &AgentEvent::ConfirmationRequired {
                tool_id: "tool-1".to_string(),
                tool_name: "bash".to_string(),
                args: serde_json::json!({"cmd": "rm -rf target"}),
                timeout_ms: 30_000,
            },
            "run-1",
            "session-1",
        )
        .await;
    let snapshot = executor.runtime_snapshot();
    assert_eq!(snapshot.active_tools, 0);
    assert_eq!(snapshot.pending_actions, 1);
    assert_eq!(snapshot.current_state, "waiting");

    executor
        .publish_agent_event(
            &AgentEvent::ConfirmationReceived {
                tool_id: "tool-1".to_string(),
                approved: true,
                reason: None,
            },
            "run-1",
            "session-1",
        )
        .await;
    executor
        .publish_agent_event(
            &AgentEvent::ExternalTaskPending {
                task_id: "task-1".to_string(),
                session_id: "session-1".to_string(),
                lane: crate::queue::SessionLane::Execute,
                command_type: "tool".to_string(),
                payload: serde_json::json!({}),
                timeout_ms: 30_000,
            },
            "run-1",
            "session-1",
        )
        .await;
    let snapshot = executor.runtime_snapshot();
    assert_eq!(snapshot.pending_actions, 1);
    assert_eq!(snapshot.queue_depth, 1);

    executor
        .publish_agent_event(
            &AgentEvent::TurnEnd {
                turn: 1,
                usage: usage(10),
            },
            "run-1",
            "session-1",
        )
        .await;
    executor
        .publish_agent_event(
            &AgentEvent::TurnEnd {
                turn: 2,
                usage: usage(7),
            },
            "run-1",
            "session-1",
        )
        .await;
    executor
        .publish_agent_event(
            &AgentEvent::End {
                text: "done".to_string(),
                usage: usage(17),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                meta: None,
            },
            "run-1",
            "session-1",
        )
        .await;

    let snapshot = executor.runtime_snapshot();
    assert_eq!(snapshot.tokens_used, 17);
    assert_eq!(snapshot.pending_actions, 0);
    assert_eq!(snapshot.queue_depth, 0);
}

#[test]
fn heartbeat_payload_includes_runtime_counters() {
    let executor = make_test_executor();
    executor.runtime_state.observe_agent_event(
        &AgentEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
        },
        "run-1",
    );
    executor.runtime_state.observe_agent_event(
        &AgentEvent::TurnEnd {
            turn: 1,
            usage: usage(42),
        },
        "run-1",
    );

    let heartbeat = executor.heartbeat_payload();
    assert_eq!(heartbeat.active_tools, Some(1));
    assert_eq!(heartbeat.pending_actions, Some(0));
    assert_eq!(heartbeat.queue_depth, Some(0));
    assert_eq!(heartbeat.tokens_used, Some(42));
    assert_eq!(heartbeat.current_state, "running_tools");
}

#[test]
fn test_map_pre_tool_use() {
    let executor = make_test_executor();

    let event = HookEvent::PreToolUse(PreToolUseEvent {
        session_id: "session-123".to_string(),
        tool: "Bash".to_string(),
        args: serde_json::json!({"command": "ls"}),
        working_directory: "/workspace".to_string(),
        recent_tools: vec![],
    });

    let ahp_event = executor.map_event(&event).unwrap();
    assert_eq!(ahp_event.event_type, EventType::PreAction);
    assert_eq!(ahp_event.session_id, "session-123");
    assert_eq!(ahp_event.depth, 0);
}

#[test]
fn test_map_decision_allow() {
    let executor = make_test_executor();

    let decision = Decision::Allow {
        modified_payload: None,
        metadata: None,
    };

    let result = executor.map_decision(
        EventType::PreAction,
        serde_json::to_value(decision).unwrap(),
    );
    assert!(matches!(result, HookResult::Continue(None)));
}

#[test]
fn test_map_decision_block() {
    let executor = make_test_executor();

    let decision = Decision::Block {
        reason: "Dangerous command".to_string(),
        metadata: None,
    };

    let result = executor.map_decision(
        EventType::PreAction,
        serde_json::to_value(decision).unwrap(),
    );
    assert!(matches!(result, HookResult::Block(_)));
}

#[test]
fn test_idle_detection_not_idle() {
    let executor = make_test_executor();
    // Should not be idle since we just created it
    let idle_event = executor.check_idle();
    assert!(idle_event.is_none());
}

#[test]
fn test_idle_detection_after_threshold() {
    let executor = make_test_executor();
    // Simulate old last activity (11 seconds ago)
    let old_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 11_000;
    executor.last_activity.store(old_time, Ordering::Relaxed);

    let idle_event = executor.check_idle();
    assert!(idle_event.is_some());
    let idle = idle_event.unwrap();
    assert!(idle.idle_duration_ms >= 10_000);
    assert_eq!(idle.idle_reason, "no_activity");
    assert_eq!(idle.suggested_action, Some("dream".to_string()));
}

#[test]
fn test_record_event_updates_activity() {
    let executor = make_test_executor();
    let old_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        - 1_000;
    executor.last_activity.store(old_time, Ordering::Relaxed);

    let before = executor.last_activity.load(Ordering::Relaxed);
    executor.record_event();
    let after = executor.last_activity.load(Ordering::Relaxed);

    assert!(after > before);
    assert!(executor.get_idle_duration_ms() < 1_000);
}
