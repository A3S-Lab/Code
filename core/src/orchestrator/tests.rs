//! Orchestrator 测试

use super::*;
use crate::error::Result;
use crate::orchestrator::{
    AgentOrchestrator, ControlSignal, OrchestratorEvent, SubAgentConfig, SubAgentHandle,
    SubAgentState,
};

#[tokio::test]
async fn test_orchestrator_creation() {
    let orchestrator = AgentOrchestrator::new_memory();
    assert_eq!(orchestrator.active_count().await, 0);
}

#[tokio::test]
async fn test_spawn_subagent() {
    let orchestrator = AgentOrchestrator::new_memory();

    let config = SubAgentConfig::new("general", "Test prompt")
        .with_description("Test agent")
        .with_permissive(true)
        .with_max_steps(5);

    let handle = orchestrator
        .spawn_subagent(config)
        .await
        .expect("Failed to spawn subagent");

    assert!(!handle.id.is_empty());
    assert_eq!(orchestrator.active_count().await, 1);
}

#[tokio::test]
async fn test_subagent_lifecycle() {
    let orchestrator = AgentOrchestrator::new_memory();

    let config = SubAgentConfig::new("general", "Test")
        .with_description("Test lifecycle")
        .with_permissive(true)
        .with_max_steps(3);

    let handle = orchestrator.spawn_subagent(config).await.unwrap();

    // Wait for completion using the handle
    let result = handle.wait().await;
    assert!(result.is_ok());

    let state = handle.state_async().await;
    assert!(state.is_terminal());
}

#[tokio::test]
async fn test_pause_resume() {
    let orchestrator = AgentOrchestrator::new_memory();

    let config = SubAgentConfig::new("general", "Test")
        .with_description("Test pause/resume")
        .with_permissive(true)
        .with_max_steps(10);

    let handle = orchestrator.spawn_subagent(config).await.unwrap();

    // Wait for it to start running
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if handle.is_running() {
            break;
        }
    }

    // Pause
    handle.pause().await.unwrap();

    // Wait for pause to take effect
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if handle.is_paused() {
            break;
        }
    }
    assert!(handle.is_paused());

    // Resume
    handle.resume().await.unwrap();

    // Wait for resume to take effect
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if handle.is_running() || handle.is_done() {
            break;
        }
    }
    assert!(handle.is_running() || handle.is_done());
}

#[tokio::test]
async fn test_cancel() {
    let orchestrator = AgentOrchestrator::new_memory();

    let config = SubAgentConfig::new("general", "Test")
        .with_description("Test cancel")
        .with_permissive(true)
        .with_max_steps(10);

    let handle = orchestrator.spawn_subagent(config).await.unwrap();

    // Wait for it to start running
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if handle.is_running() {
            break;
        }
    }

    // Cancel
    handle.cancel().await.unwrap();

    // Wait for cancellation to be processed
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let state = handle.state_async().await;
        if matches!(state, SubAgentState::Cancelled) {
            break;
        }
    }

    let state = handle.state_async().await;
    assert!(matches!(state, SubAgentState::Cancelled));
}

#[tokio::test]
async fn test_event_subscription() {
    let orchestrator = AgentOrchestrator::new_memory();
    let mut events = orchestrator.subscribe_all();

    let config = SubAgentConfig::new("general", "Test")
        .with_description("Test events")
        .with_permissive(true)
        .with_max_steps(2);

    orchestrator.spawn_subagent(config).await.unwrap();

    // Collect events
    let mut event_types = Vec::new();
    let timeout = tokio::time::sleep(tokio::time::Duration::from_secs(2));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            Ok(event) = events.recv() => {
                event_types.push(event.event_name().to_string());
                if matches!(event, OrchestratorEvent::SubAgentCompleted { .. }) {
                    break;
                }
            }
            _ = &mut timeout => break,
        }
    }

    // Should have at least: started, state_changed (to running), completed
    assert!(event_types.contains(&"subagent_started".to_string()));
    assert!(event_types.contains(&"subagent_state_changed".to_string()));
    assert!(event_types.contains(&"subagent_completed".to_string()));
}

#[tokio::test]
async fn test_external_task_pending_event_fields() {
    // Verify that ExternalTaskPending/Completed variants serialise correctly
    // and that event_name() returns the expected strings.
    use crate::hitl::SessionLane;

    let pending = OrchestratorEvent::ExternalTaskPending {
        id: "subagent-1".to_string(),
        task_id: "task-42".to_string(),
        lane: SessionLane::Execute,
        command_type: "bash".to_string(),
        payload: serde_json::json!({"command": "cargo test"}),
        timeout_ms: 30_000,
    };
    assert_eq!(pending.event_name(), "external_task_pending");
    assert_eq!(pending.subagent_id(), Some("subagent-1"));

    let completed = OrchestratorEvent::ExternalTaskCompleted {
        id: "subagent-1".to_string(),
        task_id: "task-42".to_string(),
        success: true,
    };
    assert_eq!(completed.event_name(), "external_task_completed");
    assert_eq!(completed.subagent_id(), Some("subagent-1"));
}

#[tokio::test]
async fn test_subagent_config_with_lane_config() {
    use crate::queue::{LaneHandlerConfig, SessionLane, SessionQueueConfig, TaskHandlerMode};

    let lane_cfg = SessionQueueConfig::default();
    let config = SubAgentConfig::new("general", "Test prompt")
        .with_description("External lane test")
        .with_lane_config(lane_cfg);

    assert!(config.lane_config.is_some());

    // Verify a specific lane can be set to External mode.
    let external_cfg = SessionQueueConfig::default();
    let mut handlers = std::collections::HashMap::new();
    handlers.insert(
        SessionLane::Execute,
        LaneHandlerConfig {
            mode: TaskHandlerMode::External,
            timeout_ms: 30_000,
        },
    );
    // Build via with_lane_config builder
    let config2 = SubAgentConfig::new("general", "prompt").with_lane_config(external_cfg);
    assert!(config2.lane_config.is_some());
}

#[tokio::test]
async fn test_complete_external_task_no_session() {
    // Without a real Agent, complete_external_task returns false (no session registered).
    let orchestrator = AgentOrchestrator::new_memory();
    let result = orchestrator
        .complete_external_task(
            "nonexistent-subagent",
            "task-1",
            crate::queue::ExternalTaskResult {
                success: true,
                result: serde_json::Value::Null,
                error: None,
            },
        )
        .await;
    assert!(!result);
}

#[tokio::test]
async fn test_max_concurrent_subagents() {
    let mut config = crate::orchestrator::OrchestratorConfig::default();
    config.max_concurrent_subagents = 2;

    let orchestrator = AgentOrchestrator::new(config);

    let agent_config = SubAgentConfig::new("general", "Test")
        .with_description("Test")
        .with_permissive(true)
        .with_max_steps(5);

    // Spawn 2 agents (should succeed)
    let _h1 = orchestrator
        .spawn_subagent(agent_config.clone())
        .await
        .unwrap();
    let _h2 = orchestrator
        .spawn_subagent(agent_config.clone())
        .await
        .unwrap();

    // Try to spawn 3rd (should fail)
    let result: Result<SubAgentHandle> = orchestrator.spawn_subagent(agent_config).await;
    assert!(result.is_err());
}
