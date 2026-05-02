//! Orchestrator tests.

use crate::orchestrator::{AgentOrchestrator, SubAgentConfig};

#[tokio::test]
async fn test_orchestrator_creation() {
    let orchestrator = AgentOrchestrator::new_memory();
    assert_eq!(orchestrator.active_count().await, 0);
}

#[tokio::test]
async fn test_spawn_without_agent_is_rejected() {
    let orchestrator = AgentOrchestrator::new_memory();
    let config = SubAgentConfig::new("general", "Test prompt")
        .with_description("Test agent")
        .with_max_steps(5);

    let error = orchestrator.spawn_subagent(config).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("SubAgent execution requires AgentOrchestrator::from_agent"));
    assert_eq!(orchestrator.active_count().await, 0);
}
