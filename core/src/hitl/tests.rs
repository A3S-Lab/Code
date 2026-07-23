use super::*;

// ========================================================================
// SessionLane Tests
// ========================================================================

#[test]
fn test_session_lane() {
    assert_eq!(SessionLane::from_tool_name("read"), SessionLane::Query);
    assert_eq!(SessionLane::from_tool_name("grep"), SessionLane::Query);
    assert_eq!(
        SessionLane::from_tool_name("code_navigation"),
        SessionLane::Query
    );
    assert_eq!(SessionLane::from_tool_name("bash"), SessionLane::Execute);
    assert_eq!(SessionLane::from_tool_name("write"), SessionLane::Execute);
}

#[test]
fn test_session_lane_priority() {
    assert_eq!(SessionLane::Control.priority(), 0);
    assert_eq!(SessionLane::Query.priority(), 1);
    assert_eq!(SessionLane::Execute.priority(), 2);
    assert_eq!(SessionLane::Generate.priority(), 3);

    // Control has highest priority (lowest number)
    assert!(SessionLane::Control.priority() < SessionLane::Query.priority());
    assert!(SessionLane::Query.priority() < SessionLane::Execute.priority());
    assert!(SessionLane::Execute.priority() < SessionLane::Generate.priority());
}

#[test]
fn test_session_lane_all_query() {
    let query_tools = [
        "read",
        "glob",
        "ls",
        "grep",
        "list_files",
        "search",
        "code_symbols",
        "code_navigation",
        "code_diagnostics",
    ];
    for tool in query_tools {
        assert_eq!(
            SessionLane::from_tool_name(tool),
            SessionLane::Query,
            "Tool '{}' should be in Query lane",
            tool
        );
    }
}

#[test]
fn test_session_lane_all_execute() {
    let execute_tools = ["bash", "write", "edit", "delete", "move", "copy", "execute"];
    for tool in execute_tools {
        assert_eq!(
            SessionLane::from_tool_name(tool),
            SessionLane::Execute,
            "Tool '{}' should be in Execute lane",
            tool
        );
    }
}

// ========================================================================
// TimeoutAction Tests
// ========================================================================

// ========================================================================
// ConfirmationPolicy Tests
// ========================================================================

#[test]
fn test_confirmation_policy_default() {
    let policy = ConfirmationPolicy::default();
    assert!(!policy.enabled);
    // HITL disabled = everything is YOLO (no confirmation needed)
    assert!(!policy.requires_confirmation("bash"));
    assert!(!policy.requires_confirmation("write"));
    assert!(!policy.requires_confirmation("read"));
}

#[test]
fn test_confirmation_policy_enabled() {
    let policy = ConfirmationPolicy::enabled();
    assert!(policy.enabled);
    // All tools require confirmation when enabled with no YOLO lanes
    assert!(policy.requires_confirmation("bash"));
    assert!(policy.requires_confirmation("write"));
    assert!(policy.requires_confirmation("read"));
    assert!(policy.requires_confirmation("grep"));
}

#[test]
fn test_confirmation_policy_yolo_mode() {
    let policy = ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Execute]);

    assert!(!policy.requires_confirmation("bash")); // Execute lane in YOLO mode
    assert!(!policy.requires_confirmation("write")); // Execute lane in YOLO mode
    assert!(policy.requires_confirmation("read")); // Query lane NOT in YOLO
}

#[test]
fn test_confirmation_policy_yolo_multiple_lanes() {
    let policy =
        ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Query, SessionLane::Execute]);

    // All tools in YOLO lanes should be auto-approved
    assert!(!policy.requires_confirmation("bash")); // Execute
    assert!(!policy.requires_confirmation("read")); // Query
    assert!(!policy.requires_confirmation("grep")); // Query
}

#[test]
fn test_confirmation_policy_is_yolo() {
    let policy = ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Execute]);

    assert!(policy.is_yolo("bash")); // Execute lane
    assert!(policy.is_yolo("write")); // Execute lane
    assert!(!policy.is_yolo("read")); // Query lane, not YOLO
}

#[test]
fn test_confirmation_policy_disabled_is_always_yolo() {
    let policy = ConfirmationPolicy::default(); // disabled
    assert!(policy.is_yolo("bash"));
    assert!(policy.is_yolo("read"));
    assert!(policy.is_yolo("unknown_tool"));
}

#[test]
fn test_confirmation_policy_with_timeout() {
    let policy = ConfirmationPolicy::enabled().with_timeout(5000, TimeoutAction::AutoApprove);

    assert_eq!(policy.default_timeout_ms, 5000);
    assert_eq!(policy.timeout_action, TimeoutAction::AutoApprove);
}

// ========================================================================
// ConfirmationManager Basic Tests
// ========================================================================

#[tokio::test]
async fn test_confirmation_manager_no_hitl() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::default(), event_tx);

    assert!(!manager.requires_confirmation("bash").await);
}

#[tokio::test]
async fn test_confirmation_manager_with_hitl() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // All tools require confirmation when HITL enabled with no YOLO lanes
    assert!(manager.requires_confirmation("bash").await);
    assert!(manager.requires_confirmation("read").await);
}

#[tokio::test]
async fn test_confirmation_manager_with_yolo() {
    let (event_tx, _) = broadcast::channel(100);
    let policy = ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Query]);
    let manager = ConfirmationManager::new(policy, event_tx);

    assert!(manager.requires_confirmation("bash").await); // Execute lane, not YOLO
    assert!(!manager.requires_confirmation("read").await); // Query lane, YOLO
}

#[tokio::test]
async fn test_confirmation_manager_policy_update() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::default(), event_tx);

    // Initially disabled
    assert!(!manager.requires_confirmation("bash").await);

    // Update policy to enabled
    manager.set_policy(ConfirmationPolicy::enabled()).await;
    assert!(manager.requires_confirmation("bash").await);

    // Update policy with YOLO mode
    manager
        .set_policy(ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Execute]))
        .await;
    assert!(!manager.requires_confirmation("bash").await);
}

// ========================================================================
// Confirmation Flow Tests
// ========================================================================

#[tokio::test]
async fn test_confirmation_flow_approve() {
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Request confirmation
    let rx = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({"command": "ls"}))
        .await;

    // Check event was emitted
    let event = event_rx.recv().await.unwrap();
    match event {
        AgentEvent::ConfirmationRequired {
            tool_id,
            tool_name,
            timeout_ms,
            ..
        } => {
            assert_eq!(tool_id, "tool-1");
            assert_eq!(tool_name, "bash");
            assert_eq!(timeout_ms, 30_000); // Default timeout
        }
        _ => panic!("Expected ConfirmationRequired event"),
    }

    // Approve the confirmation
    let result = manager.confirm("tool-1", true, None).await;
    assert!(result.is_ok());
    assert!(result.unwrap());

    // Check ConfirmationReceived event
    let event = event_rx.recv().await.unwrap();
    match event {
        AgentEvent::ConfirmationReceived {
            tool_id, approved, ..
        } => {
            assert_eq!(tool_id, "tool-1");
            assert!(approved);
        }
        _ => panic!("Expected ConfirmationReceived event"),
    }

    // Check response
    let response = rx.await.unwrap();
    assert!(response.approved);
    assert!(response.reason.is_none());
}

#[tokio::test]
async fn test_confirmation_flow_reject() {
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Request confirmation
    let rx = manager
        .request_confirmation(
            "tool-1",
            "bash",
            &serde_json::json!({"command": "rm -rf /"}),
        )
        .await;

    // Skip ConfirmationRequired event
    let _ = event_rx.recv().await.unwrap();

    // Reject the confirmation with reason
    let result = manager
        .confirm("tool-1", false, Some("Dangerous command".to_string()))
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap());

    // Check ConfirmationReceived event
    let event = event_rx.recv().await.unwrap();
    match event {
        AgentEvent::ConfirmationReceived {
            tool_id,
            approved,
            reason,
        } => {
            assert_eq!(tool_id, "tool-1");
            assert!(!approved);
            assert_eq!(reason, Some("Dangerous command".to_string()));
        }
        _ => panic!("Expected ConfirmationReceived event"),
    }

    // Check response
    let response = rx.await.unwrap();
    assert!(!response.approved);
    assert_eq!(response.reason, Some("Dangerous command".to_string()));
}

#[tokio::test]
async fn test_confirmation_not_found() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Try to confirm non-existent confirmation
    let result = manager.confirm("non-existent", true, None).await;
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Returns false for not found
}

// ========================================================================
// Multiple Confirmations Tests
// ========================================================================

#[tokio::test]
async fn test_multiple_confirmations() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Request multiple confirmations
    let rx1 = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({"cmd": "1"}))
        .await;
    let rx2 = manager
        .request_confirmation("tool-2", "write", &serde_json::json!({"cmd": "2"}))
        .await;
    let rx3 = manager
        .request_confirmation("tool-3", "edit", &serde_json::json!({"cmd": "3"}))
        .await;

    // Check pending count
    assert_eq!(manager.pending_count().await, 3);

    // Approve tool-1
    manager.confirm("tool-1", true, None).await.unwrap();
    let response1 = rx1.await.unwrap();
    assert!(response1.approved);

    // Reject tool-2
    manager.confirm("tool-2", false, None).await.unwrap();
    let response2 = rx2.await.unwrap();
    assert!(!response2.approved);

    // Approve tool-3
    manager.confirm("tool-3", true, None).await.unwrap();
    let response3 = rx3.await.unwrap();
    assert!(response3.approved);

    // All confirmations processed
    assert_eq!(manager.pending_count().await, 0);
}

#[tokio::test]
async fn duplicate_tool_ids_reject_both_requests_without_leaving_a_receiver() {
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    let first = manager
        .request_confirmation("duplicate-id", "bash", &serde_json::json!({"cmd": "first"}))
        .await;
    let duplicate = manager
        .request_confirmation(
            "duplicate-id",
            "write",
            &serde_json::json!({"path": "second"}),
        )
        .await;

    for receiver in [first, duplicate] {
        let response = receiver
            .await
            .expect("a duplicate id must return an explicit rejection");
        assert!(!response.approved);
        assert!(response
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Duplicate confirmation tool id")));
    }
    assert_eq!(manager.pending_count().await, 0);
    assert!(!manager.confirm("duplicate-id", true, None).await.unwrap());

    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::ConfirmationRequired {
            ref tool_id,
            ref tool_name,
            ..
        } if tool_id == "duplicate-id" && tool_name == "bash"
    ));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::ConfirmationReceived {
            ref tool_id,
            approved: false,
            ..
        } if tool_id == "duplicate-id"
    ));
    assert!(event_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_pending_confirmations_info() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Request confirmations
    let _rx1 = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({}))
        .await;
    let _rx2 = manager
        .request_confirmation("tool-2", "write", &serde_json::json!({}))
        .await;

    let pending = manager.pending_confirmations().await;
    assert_eq!(pending.len(), 2);

    // Check that both tools are in pending list
    let tool_ids: Vec<&str> = pending.iter().map(|(id, _, _)| id.as_str()).collect();
    assert!(tool_ids.contains(&"tool-1"));
    assert!(tool_ids.contains(&"tool-2"));
}

// ========================================================================
// Cancel Tests
// ========================================================================

#[tokio::test]
async fn test_cancel_confirmation() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Request confirmation
    let rx = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({}))
        .await;

    assert_eq!(manager.pending_count().await, 1);

    // Cancel confirmation
    let cancelled = manager.cancel("tool-1").await;
    assert!(cancelled);
    assert_eq!(manager.pending_count().await, 0);

    // Check response indicates cancellation
    let response = rx.await.unwrap();
    assert!(!response.approved);
    assert_eq!(response.reason, Some("Confirmation cancelled".to_string()));
}

#[tokio::test]
async fn test_cancel_nonexistent() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    let cancelled = manager.cancel("non-existent").await;
    assert!(!cancelled);
}

#[tokio::test]
async fn cancelling_one_confirmation_leaves_concurrent_confirmation_pending() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);
    let cancelled = manager
        .request_confirmation("tool-cancelled", "bash", &serde_json::json!({}))
        .await;
    let untouched = manager
        .request_confirmation("tool-untouched", "write", &serde_json::json!({}))
        .await;

    assert!(manager.cancel("tool-cancelled").await);
    assert!(!cancelled.await.unwrap().approved);
    let pending = manager.pending_confirmations().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, "tool-untouched");

    assert!(manager.confirm("tool-untouched", true, None).await.unwrap());
    assert!(untouched.await.unwrap().approved);
}

#[tokio::test]
async fn test_cancel_all() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);

    // Request multiple confirmations
    let rx1 = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({}))
        .await;
    let rx2 = manager
        .request_confirmation("tool-2", "write", &serde_json::json!({}))
        .await;
    let rx3 = manager
        .request_confirmation("tool-3", "edit", &serde_json::json!({}))
        .await;

    assert_eq!(manager.pending_count().await, 3);

    // Cancel all
    let cancelled_count = manager.cancel_all().await;
    assert_eq!(cancelled_count, 3);
    assert_eq!(manager.pending_count().await, 0);

    // All responses should indicate cancellation
    for rx in [rx1, rx2, rx3] {
        let response = rx.await.unwrap();
        assert!(!response.approved);
        assert_eq!(response.reason, Some("Confirmation cancelled".to_string()));
    }
}

// ========================================================================
// Timeout Tests
// ========================================================================

#[tokio::test]
async fn test_timeout_reject() {
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 50, // Very short timeout for testing
        timeout_action: TimeoutAction::Reject,
        ..Default::default()
    };
    let manager = ConfirmationManager::new(policy, event_tx);

    // Request confirmation
    let rx = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({}))
        .await;

    // Skip ConfirmationRequired event
    let _ = event_rx.recv().await.unwrap();

    // Wait for timeout
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check timeouts
    let timed_out = manager.check_timeouts().await;
    assert_eq!(timed_out, 1);

    // Check timeout event
    let event = event_rx.recv().await.unwrap();
    match event {
        AgentEvent::ConfirmationTimeout {
            tool_id,
            action_taken,
        } => {
            assert_eq!(tool_id, "tool-1");
            assert_eq!(action_taken, "rejected");
        }
        _ => panic!("Expected ConfirmationTimeout event"),
    }

    // Check response indicates timeout rejection
    let response = rx.await.unwrap();
    assert!(!response.approved);
    assert!(response.reason.as_ref().unwrap().contains("timed out"));
}

#[tokio::test]
async fn test_timeout_auto_approve() {
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 50, // Very short timeout for testing
        timeout_action: TimeoutAction::AutoApprove,
        ..Default::default()
    };
    let manager = ConfirmationManager::new(policy, event_tx);

    // Request confirmation
    let rx = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({}))
        .await;

    // Skip ConfirmationRequired event
    let _ = event_rx.recv().await.unwrap();

    // Wait for timeout
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check timeouts
    let timed_out = manager.check_timeouts().await;
    assert_eq!(timed_out, 1);

    // Check timeout event
    let event = event_rx.recv().await.unwrap();
    match event {
        AgentEvent::ConfirmationTimeout {
            tool_id,
            action_taken,
        } => {
            assert_eq!(tool_id, "tool-1");
            assert_eq!(action_taken, "auto_approved");
        }
        _ => panic!("Expected ConfirmationTimeout event"),
    }

    // Check response indicates timeout auto-approval
    let response = rx.await.unwrap();
    assert!(response.approved);
    assert!(response.reason.as_ref().unwrap().contains("auto_approved"));
}

#[tokio::test]
async fn expiring_one_confirmation_leaves_concurrent_confirmation_pending() {
    let (event_tx, _) = broadcast::channel(100);
    let manager = ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx);
    let expired = manager
        .request_confirmation("tool-expired", "bash", &serde_json::json!({}))
        .await;
    let untouched = manager
        .request_confirmation("tool-untouched", "write", &serde_json::json!({}))
        .await;

    assert!(manager.expire("tool-expired", TimeoutAction::Reject).await);
    assert!(!expired.await.unwrap().approved);
    let pending = manager.pending_confirmations().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, "tool-untouched");

    assert!(manager.confirm("tool-untouched", true, None).await.unwrap());
    assert!(untouched.await.unwrap().approved);
}

#[tokio::test]
async fn test_no_timeout_when_confirmed() {
    let (event_tx, _) = broadcast::channel(100);
    let policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 50,
        timeout_action: TimeoutAction::Reject,
        ..Default::default()
    };
    let manager = ConfirmationManager::new(policy, event_tx);

    // Request confirmation
    let rx = manager
        .request_confirmation("tool-1", "bash", &serde_json::json!({}))
        .await;

    // Confirm immediately
    manager.confirm("tool-1", true, None).await.unwrap();

    // Wait past timeout
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Check timeouts - should be 0 since already confirmed
    let timed_out = manager.check_timeouts().await;
    assert_eq!(timed_out, 0);

    // Response should be approval (not timeout)
    let response = rx.await.unwrap();
    assert!(response.approved);
    assert!(response.reason.is_none());
}
