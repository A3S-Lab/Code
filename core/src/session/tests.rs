#[cfg(test)]
mod tests {
    use crate::context::{ContextItem, ContextProvider, ContextQuery, ContextResult};
    use crate::hitl::{ConfirmationPolicy, SessionLane, TimeoutAction};
    use crate::llm::ContentBlock;
    use crate::permissions::{PermissionDecision, PermissionPolicy};
    use crate::prompts::PlanningMode;
    use crate::queue::SessionQueueConfig;
    use crate::session::manager::*;
    use crate::session::*;
    use crate::tools::ToolExecutor;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// SessionConfig with workspace set to OS temp dir to avoid creating
    /// `.a3s/` inside the crate directory when running `cargo test`.
    fn test_config() -> SessionConfig {
        SessionConfig {
            workspace: std::env::temp_dir().to_string_lossy().into_owned(),
            ..SessionConfig::default()
        }
    }

    /// Mock LLM client that returns a summary for compaction tests.
    struct MockSummaryLlmClient;

    #[async_trait::async_trait]
    impl LlmClient for MockSummaryLlmClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            Ok(crate::llm::LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "This is a summary of the conversation.".to_string(),
                    }],
                    reasoning_content: None,
                },
                usage: crate::llm::TokenUsage::default(),
                stop_reason: Some("end_turn".to_string()),
                meta: None,
            })
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            Ok(rx)
        }
    }

    // ========================================================================
    // Basic Session Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_creation() {
        let config = SessionConfig {
            name: "test".to_string(),
            workspace: "/tmp".to_string(),
            system_prompt: Some("You are helpful.".to_string()),
            max_context_length: 0,
            auto_compact: false,
            auto_compact_threshold: DEFAULT_AUTO_COMPACT_THRESHOLD,
            storage_type: crate::config::StorageBackend::Memory,
            queue_config: None,
            confirmation_policy: None,
            permission_policy: None,
            parent_id: None,
            security_config: None,
            hook_engine: None,
            planning_mode: PlanningMode::default(),
            goal_tracking: false,
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.id, "test-1");
        assert_eq!(session.system(), Some("You are helpful."));
        assert!(session.messages.is_empty());
        assert_eq!(session.state, SessionState::Active);
        assert!(session.created_at > 0);
    }

    #[tokio::test]
    async fn test_session_creation_with_queue_config() {
        let queue_config = SessionQueueConfig {
            control_max_concurrency: 1,
            query_max_concurrency: 2,
            execute_max_concurrency: 3,
            generate_max_concurrency: 4,
            lane_handlers: std::collections::HashMap::new(),
            ..Default::default()
        };
        let config = SessionConfig {
            queue_config: Some(queue_config),
            ..test_config()
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.id, "test-1");
    }

    #[tokio::test]
    async fn test_session_creation_with_confirmation_policy() {
        let policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Query])
            .with_timeout(5000, TimeoutAction::AutoApprove);

        let config = SessionConfig {
            confirmation_policy: Some(policy),
            ..test_config()
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.id, "test-1");
    }

    #[test]
    fn test_context_usage_default() {
        let usage = ContextUsage::default();
        assert_eq!(usage.used_tokens, 0);
        assert_eq!(usage.max_tokens, 200_000);
        assert_eq!(usage.percent, 0.0);
    }

    #[tokio::test]
    async fn test_session_pause_resume() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        assert_eq!(session.state, SessionState::Active);

        // Pause
        assert!(session.pause());
        assert_eq!(session.state, SessionState::Paused);

        // Can't pause again
        assert!(!session.pause());

        // Resume
        assert!(session.resume());
        assert_eq!(session.state, SessionState::Active);

        // Can't resume again
        assert!(!session.resume());
    }

    // ========================================================================
    // Session HITL Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_confirmation_policy() {
        let config = test_config();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Default policy (HITL enabled — secure default)
        let policy = session.confirmation_policy().await;
        assert!(policy.enabled);

        // Update policy
        let new_policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Execute])
            .with_timeout(10000, TimeoutAction::Reject);

        session.set_confirmation_policy(new_policy).await;

        let policy = session.confirmation_policy().await;
        assert!(policy.enabled);
        assert!(policy.yolo_lanes.contains(&SessionLane::Execute));
        assert_eq!(policy.default_timeout_ms, 10000);
        assert_eq!(policy.timeout_action, TimeoutAction::Reject);
    }

    #[tokio::test]
    async fn test_session_permission_policy() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        assert_eq!(
            session.check_permission("bash", &serde_json::json!({})),
            PermissionDecision::Ask
        );

        session.set_permission_policy(PermissionPolicy::permissive());

        assert_eq!(
            session.check_permission("bash", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        let stored_policy = session
            .config
            .permission_policy
            .as_ref()
            .expect("permission policy should be stored in config");
        assert_eq!(
            stored_policy.check("bash", &serde_json::json!({})),
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn test_session_subscribe_events() {
        let config = test_config();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Subscribe to events
        let mut rx = session.subscribe_events();

        // Send an event through the broadcaster
        let tx = session.event_tx();
        tx.send(crate::agent::AgentEvent::Start {
            prompt: "test".to_string(),
        })
        .unwrap();

        // Should receive the event
        let event = rx.recv().await.unwrap();
        match event {
            crate::agent::AgentEvent::Start { prompt } => {
                assert_eq!(prompt, "test");
            }
            _ => panic!("Expected Start event"),
        }
    }

    // ========================================================================
    // SessionManager Tests
    // ========================================================================

    fn create_test_session_manager() -> SessionManager {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        SessionManager::new(None, tool_executor)
    }

    #[tokio::test]
    async fn test_session_manager_create_session() {
        let manager = create_test_session_manager();

        let config = SessionConfig {
            name: "test-session".to_string(),
            ..test_config()
        };

        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        let session_lock = manager.get_session("session-1").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.id, "session-1");
        assert_eq!(session.config.name, "test-session");
    }

    #[tokio::test]
    async fn test_session_manager_destroy_session() {
        let manager = create_test_session_manager();

        let config = test_config();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Session exists
        assert!(manager.get_session("session-1").await.is_ok());

        // Destroy session
        manager.destroy_session("session-1").await.unwrap();

        // Session no longer exists
        assert!(manager.get_session("session-1").await.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_list_sessions() {
        let manager = create_test_session_manager();

        // Create multiple sessions
        for i in 0..3 {
            let config = SessionConfig {
                name: format!("session-{}", i),
                ..test_config()
            };
            manager
                .create_session(format!("session-{}", i), config)
                .await
                .unwrap();
        }

        let sessions = manager.get_all_sessions().await;
        assert_eq!(sessions.len(), 3);
    }

    #[tokio::test]
    async fn test_session_manager_pause_resume() {
        let manager = create_test_session_manager();

        let config = test_config();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Pause
        assert!(manager.pause_session("session-1").await.unwrap());

        // Resume
        assert!(manager.resume_session("session-1").await.unwrap());
    }

    // ========================================================================
    // SessionManager HITL Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_confirmation_policy() {
        let manager = create_test_session_manager();

        let config = test_config();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Get default policy (HITL enabled — secure default)
        let policy = manager.get_confirmation_policy("session-1").await.unwrap();
        assert!(policy.enabled);

        // Set new policy
        let new_policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Query, SessionLane::Execute]);

        let result = manager
            .set_confirmation_policy("session-1", new_policy)
            .await
            .unwrap();
        assert!(result.enabled);
        assert!(result.yolo_lanes.contains(&SessionLane::Query));
        assert!(result.yolo_lanes.contains(&SessionLane::Execute));

        // Verify policy was persisted
        let policy = manager.get_confirmation_policy("session-1").await.unwrap();
        assert!(policy.enabled);
    }

    #[tokio::test]
    async fn test_session_manager_permission_policy() {
        let manager = create_test_session_manager();

        let config = test_config();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        let result = manager
            .set_permission_policy("session-1", PermissionPolicy::permissive())
            .await
            .unwrap();
        assert_eq!(
            result.check("bash", &serde_json::json!({})),
            PermissionDecision::Allow
        );

        let session_lock = manager.get_session("session-1").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(
            session.check_permission("bash", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        let stored_policy = session
            .config
            .permission_policy
            .as_ref()
            .expect("permission policy should be stored in config");
        assert_eq!(
            stored_policy.check("bash", &serde_json::json!({})),
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn test_session_manager_confirm_tool_not_found() {
        let manager = create_test_session_manager();

        let config = test_config();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Confirm non-existent tool
        let result = manager
            .confirm_tool("session-1", "non-existent", true, None)
            .await
            .unwrap();
        assert!(!result); // Not found
    }

    #[tokio::test]
    async fn test_session_manager_confirm_tool_session_not_found() {
        let manager = create_test_session_manager();

        // Session doesn't exist
        let result = manager
            .confirm_tool("non-existent-session", "tool-1", true, None)
            .await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Integration Tests: Multiple Sessions
    // ========================================================================

    #[tokio::test]
    async fn test_multiple_sessions_independent_policies() {
        let manager = create_test_session_manager();

        // Create two sessions with different policies
        let config1 = SessionConfig {
            confirmation_policy: Some(ConfirmationPolicy::enabled()),
            ..test_config()
        };
        let config2 = SessionConfig {
            confirmation_policy: Some(
                ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Execute]),
            ),
            ..test_config()
        };

        manager
            .create_session("session-1".to_string(), config1)
            .await
            .unwrap();
        manager
            .create_session("session-2".to_string(), config2)
            .await
            .unwrap();

        // Verify policies are independent
        let policy1 = manager.get_confirmation_policy("session-1").await.unwrap();
        let policy2 = manager.get_confirmation_policy("session-2").await.unwrap();

        assert!(policy1.enabled);
        assert!(policy1.yolo_lanes.is_empty());

        assert!(policy2.enabled);
        assert!(policy2.yolo_lanes.contains(&SessionLane::Execute));

        // Update session-1 policy
        manager
            .set_confirmation_policy(
                "session-1",
                ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Query]),
            )
            .await
            .unwrap();

        // session-2 should be unchanged
        let policy2 = manager.get_confirmation_policy("session-2").await.unwrap();
        assert!(!policy2.yolo_lanes.contains(&SessionLane::Query));
        assert!(policy2.yolo_lanes.contains(&SessionLane::Execute));
    }

    /// Mock context provider for testing
    #[allow(dead_code)]
    struct MockContextProvider {
        name: String,
        items: Vec<ContextItem>,
    }

    impl MockContextProvider {
        #[allow(dead_code)]
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                items: Vec::new(),
            }
        }

        #[allow(dead_code)]
        fn with_items(mut self, items: Vec<ContextItem>) -> Self {
            self.items = items;
            self
        }
    }

    #[async_trait::async_trait]
    impl ContextProvider for MockContextProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
            let mut result = ContextResult::new(&self.name);
            for item in &self.items {
                result.add_item(item.clone());
            }
            Ok(result)
        }
    }

    // ========================================================================
    // Cancellation Tests
    // ========================================================================

    #[tokio::test]
    async fn test_cancel_operation_no_ongoing() {
        let manager = create_test_session_manager();
        let config = test_config();
        manager
            .create_session("test-session".to_string(), config)
            .await
            .unwrap();

        // Cancel when no operation is running
        let result = manager.cancel_operation("test-session").await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // No operation was cancelled
    }

    #[tokio::test]
    async fn test_cancel_operation_session_not_found() {
        let manager = create_test_session_manager();

        let result = manager.cancel_operation("non-existent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_operation_with_pending_confirmations() {
        let manager = create_test_session_manager();
        let config = SessionConfig {
            confirmation_policy: Some(ConfirmationPolicy::enabled()),
            ..test_config()
        };
        manager
            .create_session("test-session".to_string(), config)
            .await
            .unwrap();

        // Add a pending confirmation
        let session_lock = manager.get_session("test-session").await.unwrap();
        {
            let session = session_lock.read().await;
            let args = serde_json::json!({});
            session
                .confirmation_manager
                .request_confirmation("tool-1", "test_tool", &args)
                .await;
        }

        // Cancel should cancel the pending confirmation
        let result = manager.cancel_operation("test-session").await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // Confirmation was cancelled
    }

    // ========================================================================
    // Context Compaction Tests
    // ========================================================================

    #[tokio::test]
    async fn test_compact_not_needed() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add only a few messages (less than threshold)
        for i in 0..10 {
            session
                .messages
                .push(Message::user(&format!("Message {}", i)));
        }

        // Create a mock LLM client that should NOT be called
        struct NeverCalledLlmClient;

        #[async_trait::async_trait]
        impl LlmClient for NeverCalledLlmClient {
            async fn complete(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                panic!("LLM should not be called when compaction is not needed");
            }

            async fn complete_streaming(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                panic!("LLM should not be called when compaction is not needed");
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(NeverCalledLlmClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());
        assert_eq!(session.messages.len(), 10); // Messages unchanged
    }

    #[tokio::test]
    async fn test_compact_with_many_messages() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add many messages (more than threshold of 30)
        for i in 0..50 {
            session
                .messages
                .push(Message::user(&format!("Message {}", i)));
        }

        let client: Arc<dyn LlmClient> = Arc::new(MockSummaryLlmClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());

        // Should have: 2 initial + 1 summary + 20 recent = 23 messages
        assert_eq!(session.messages.len(), 23);

        // Check that the summary message is present
        let summary_msg = &session.messages[2];
        assert!(summary_msg.text().contains("[Context Summary:"));
    }

    #[tokio::test]
    async fn test_compact_then_add_tool_results() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add 50 messages to trigger compaction
        for i in 0..50 {
            session
                .messages
                .push(Message::user(&format!("Message {}", i)));
        }

        // Compact
        let client: Arc<dyn LlmClient> = Arc::new(MockSummaryLlmClient);
        session.compact(&client).await.unwrap();
        let count_after_compact = session.messages.len();

        // Add tool results after compaction
        session.messages.push(Message::tool_result(
            "tool-1",
            "{\"result\": \"success\"}",
            false,
        ));
        session.messages.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Done".to_string(),
            }],
            reasoning_content: None,
        });

        assert_eq!(session.messages.len(), count_after_compact + 2);
    }

    #[tokio::test]
    async fn test_multiple_compactions() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // First compaction
        for i in 0..50 {
            session
                .messages
                .push(Message::user(&format!("Round 1 - {}", i)));
        }
        let client: Arc<dyn LlmClient> = Arc::new(MockSummaryLlmClient);
        session.compact(&client).await.unwrap();
        let count_after_first = session.messages.len();

        // Add more messages and compact again
        for i in 0..50 {
            session
                .messages
                .push(Message::user(&format!("Round 2 - {}", i)));
        }
        session.compact(&client).await.unwrap();
        let count_after_second = session.messages.len();

        // Second compaction should also reduce message count
        assert!(count_after_second < count_after_first + 50);
    }

    #[tokio::test]
    async fn test_prune_then_compact() {
        let config = test_config();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add 50 messages
        for i in 0..50 {
            session
                .messages
                .push(Message::user(&format!("Message {}", i)));
        }

        // Prune (remove old messages manually)
        session.messages.drain(0..10);
        assert_eq!(session.messages.len(), 40);

        // Compact after pruning
        let client: Arc<dyn LlmClient> = Arc::new(MockSummaryLlmClient);
        session.compact(&client).await.unwrap();

        // Compaction should still work after manual pruning
        assert!(session.messages.len() < 40);
    }

    // ========================================================================
    // Child Session Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_is_child_session() {
        // Create a regular session (no parent)
        let config = test_config();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(!session.is_child_session());
        assert!(session.parent_session_id().is_none());

        // Create a child session (with parent)
        let child_config = SessionConfig {
            parent_id: Some("parent-1".to_string()),
            ..test_config()
        };
        let child_session = Session::new("child-1".to_string(), child_config, vec![])
            .await
            .unwrap();
        assert!(child_session.is_child_session());
        assert_eq!(child_session.parent_session_id(), Some("parent-1"));
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Create parent session
        let parent_config = test_config();
        manager
            .create_session("parent-1".to_string(), parent_config)
            .await
            .unwrap();

        // Create child session
        let child_config = SessionConfig {
            name: "Child Session".to_string(),
            ..test_config()
        };
        let child_id = manager
            .create_child_session("parent-1", "child-1".to_string(), child_config)
            .await
            .unwrap();

        assert_eq!(child_id, "child-1");

        // Verify child session has parent_id set
        let child_lock = manager.get_session("child-1").await.unwrap();
        let child = child_lock.read().await;
        assert!(child.is_child_session());
        assert_eq!(child.parent_session_id(), Some("parent-1"));
    }

    #[tokio::test]
    async fn test_session_manager_get_child_sessions() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Create parent session
        let parent_config = test_config();
        manager
            .create_session("parent-1".to_string(), parent_config)
            .await
            .unwrap();

        // Create multiple child sessions
        for i in 1..=3 {
            let child_config = test_config();
            manager
                .create_child_session("parent-1", format!("child-{}", i), child_config)
                .await
                .unwrap();
        }

        // Get child sessions
        let children = manager.get_child_sessions("parent-1").await;
        assert_eq!(children.len(), 3);
        assert!(children.contains(&"child-1".to_string()));
        assert!(children.contains(&"child-2".to_string()));
        assert!(children.contains(&"child-3".to_string()));

        // Non-existent parent should return empty list
        let no_children = manager.get_child_sessions("nonexistent").await;
        assert!(no_children.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_is_child_session() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Create parent session
        let parent_config = test_config();
        manager
            .create_session("parent-1".to_string(), parent_config)
            .await
            .unwrap();

        // Create child session
        let child_config = test_config();
        manager
            .create_child_session("parent-1", "child-1".to_string(), child_config)
            .await
            .unwrap();

        // Check is_child_session
        assert!(!manager.is_child_session("parent-1").await.unwrap());
        assert!(manager.is_child_session("child-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session_parent_not_found() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Try to create child session with non-existent parent
        let child_config = test_config();
        let result = manager
            .create_child_session("nonexistent", "child-1".to_string(), child_config)
            .await;

        assert!(result.is_err());
    }

    // ========================================================================
    // LLM Resolution Tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_llm_for_session_no_client() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = test_config();
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // No LLM client configured at any level
        let result = manager.get_llm_for_session("test-1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_llm_for_session_default_client() {
        struct DummyLlmClient;

        #[async_trait::async_trait]
        impl LlmClient for DummyLlmClient {
            async fn complete(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                unimplemented!()
            }

            async fn complete_streaming(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                unimplemented!()
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(DummyLlmClient);
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(Some(client), tool_executor);

        let config = test_config();
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Should resolve to default client
        let result = manager.get_llm_for_session("test-1").await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_get_llm_for_session_not_found() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let result = manager.get_llm_for_session("nonexistent").await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Auto-Compact Tests
    // ========================================================================

    #[test]
    fn test_session_config_default_auto_compact_threshold() {
        let config = test_config();
        assert!(!config.auto_compact);
        assert_eq!(
            config.auto_compact_threshold,
            DEFAULT_AUTO_COMPACT_THRESHOLD
        );
        assert_eq!(config.auto_compact_threshold, 0.80);
    }

    #[test]
    fn test_session_config_custom_auto_compact_threshold() {
        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.90,
            ..test_config()
        };
        assert!(config.auto_compact);
        assert_eq!(config.auto_compact_threshold, 0.90);
    }

    #[test]
    fn test_session_config_auto_compact_threshold_serde() {
        // Serialize with threshold
        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.75,
            ..test_config()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.auto_compact_threshold, 0.75);

        // Deserialize without threshold (should use default)
        let json_no_threshold = r#"{"name":"","workspace":"","system_prompt":null,"max_context_length":0,"auto_compact":true}"#;
        let parsed: SessionConfig = serde_json::from_str(json_no_threshold).unwrap();
        assert_eq!(
            parsed.auto_compact_threshold,
            DEFAULT_AUTO_COMPACT_THRESHOLD
        );
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_disabled() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig {
            auto_compact: false,
            ..test_config()
        };
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Should return false (auto_compact disabled)
        let result = manager.maybe_auto_compact("test-1").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_below_threshold() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.80,
            ..test_config()
        };
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Set context usage below threshold
        {
            let session_lock = manager.get_session("test-1").await.unwrap();
            let mut session = session_lock.write().await;
            session.context_usage.percent = 0.50; // 50% < 80%
        }

        // Should return false (below threshold)
        let result = manager.maybe_auto_compact("test-1").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_triggers_at_threshold() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.80,
            ..test_config()
        };
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Add enough messages and set high context usage
        {
            let session_lock = manager.get_session("test-1").await.unwrap();
            let mut session = session_lock.write().await;
            // Add 35 messages (above MIN_MESSAGES_FOR_COMPACTION = 30)
            for i in 0..35 {
                session.messages.push(Message::user(&format!("msg {}", i)));
            }
            session.context_usage.percent = 0.85; // 85% > 80%
            session.context_usage.used_tokens = 170_000;
            session.context_usage.max_tokens = 200_000;
        }

        // Should trigger compaction (no LLM client, so it falls back to simple truncation)
        let result = manager.maybe_auto_compact("test-1").await.unwrap();
        assert!(result);

        // Verify messages were reduced
        let session_lock = manager.get_session("test-1").await.unwrap();
        let session = session_lock.read().await;
        assert!(session.messages.len() < 35);
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_session_not_found() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let result = manager.maybe_auto_compact("nonexistent").await;
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod extra_session_tests {
    use crate::planning::{Task, TaskPriority, TaskStatus};
    use crate::session::manager::*;
    use crate::session::*;
    use crate::store::MemorySessionStore;
    use crate::tools::ToolExecutor;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// SessionConfig with workspace set to OS temp dir to avoid creating
    /// `.a3s/` inside the crate directory when running `cargo test`.
    fn default_config() -> SessionConfig {
        SessionConfig {
            workspace: std::env::temp_dir().to_string_lossy().into_owned(),
            ..SessionConfig::default()
        }
    }

    async fn make_session(id: &str) -> Session {
        Session::new(id.to_string(), default_config(), vec![])
            .await
            .unwrap()
    }

    fn make_manager() -> SessionManager {
        let store = Arc::new(MemorySessionStore::new());
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        SessionManager::with_store(
            None,
            tool_executor,
            store,
            crate::config::StorageBackend::File,
        )
    }

    // ========================================================================
    // Session state transitions
    // ========================================================================

    #[tokio::test]
    async fn test_session_set_error() {
        let mut session = make_session("s1").await;
        assert_eq!(session.state, SessionState::Active);
        session.set_error();
        assert_eq!(session.state, SessionState::Error);
    }

    #[tokio::test]
    async fn test_session_set_completed() {
        let mut session = make_session("s1").await;
        session.set_completed();
        assert_eq!(session.state, SessionState::Completed);
    }

    #[tokio::test]
    async fn test_session_set_completed_stops_queue() {
        let config = SessionConfig {
            queue_config: Some(crate::queue::SessionQueueConfig::default()),
            ..default_config()
        };
        let mut session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        session.start_queue().await.unwrap();
        session.set_completed();
        assert_eq!(session.state, SessionState::Completed);
    }

    // ========================================================================
    // Session message management
    // ========================================================================

    #[tokio::test]
    async fn test_session_add_message() {
        let mut session = make_session("s1").await;
        assert!(session.history().is_empty());

        session.add_message(Message::user("Hello"));
        assert_eq!(session.history().len(), 1);
        assert_eq!(session.context_usage.turns, 1);

        session.add_message(Message::user("World"));
        assert_eq!(session.history().len(), 2);
        assert_eq!(session.context_usage.turns, 2);
    }

    #[tokio::test]
    async fn test_session_update_usage() {
        let mut session = make_session("s1").await;
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);
        assert_eq!(session.total_usage.prompt_tokens, 100);
        assert_eq!(session.total_usage.completion_tokens, 50);
        assert_eq!(session.total_usage.total_tokens, 150);
        assert_eq!(session.context_usage.used_tokens, 100);
        assert!(session.context_usage.percent > 0.0);

        // Update again - should accumulate
        session.update_usage(&usage);
        assert_eq!(session.total_usage.prompt_tokens, 200);
        assert_eq!(session.total_usage.total_tokens, 300);
    }

    #[tokio::test]
    async fn test_session_clear() {
        let mut session = make_session("s1").await;
        session.add_message(Message::user("Hello"));
        session.add_message(Message::user("World"));
        assert_eq!(session.history().len(), 2);

        session.clear();
        assert!(session.history().is_empty());
        assert_eq!(session.context_usage.used_tokens, 0);
        assert_eq!(session.context_usage.turns, 0);
    }

    // ========================================================================
    // Session tasks
    // ========================================================================

    #[tokio::test]
    async fn test_session_tasks() {
        let mut session = make_session("s1").await;
        assert!(session.get_tasks().is_empty());

        let tasks = vec![Task::new("t1", "Fix bug"), Task::new("t2", "Write tests")];
        session.set_tasks(tasks);
        assert_eq!(session.get_tasks().len(), 2);
    }

    #[tokio::test]
    async fn test_session_active_task_count() {
        let mut session = make_session("s1").await;
        let tasks = vec![
            Task::new("t1", "Fix bug")
                .with_status(TaskStatus::Pending)
                .with_priority(TaskPriority::High),
            Task::new("t2", "Write tests")
                .with_status(TaskStatus::InProgress)
                .with_priority(TaskPriority::Medium),
            Task::new("t3", "Done task")
                .with_status(TaskStatus::Completed)
                .with_priority(TaskPriority::Low),
            Task::new("t4", "Cancelled")
                .with_status(TaskStatus::Cancelled)
                .with_priority(TaskPriority::Low),
        ];
        session.set_tasks(tasks);
        // Active = Pending + InProgress
        assert_eq!(session.active_task_count(), 2);
    }

    // ========================================================================
    // SessionState conversions
    // ========================================================================

    // ========================================================================
    // Session system prompt
    // ========================================================================

    #[tokio::test]
    async fn test_session_system_prompt() {
        let config = SessionConfig {
            system_prompt: Some("You are helpful".to_string()),
            ..default_config()
        };
        let session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.system(), Some("You are helpful"));
    }

    #[tokio::test]
    async fn test_session_no_system_prompt() {
        let session = make_session("s1").await;
        assert!(session.system().is_none());
    }

    // ========================================================================
    // Session child/parent
    // ========================================================================

    #[tokio::test]
    async fn test_session_not_child() {
        let session = make_session("s1").await;
        assert!(!session.is_child_session());
        assert!(session.parent_session_id().is_none());
    }

    #[tokio::test]
    async fn test_session_is_child() {
        let config = SessionConfig {
            parent_id: Some("parent-1".to_string()),
            ..default_config()
        };
        let session = Session::new("child-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.is_child_session());
        assert_eq!(session.parent_session_id(), Some("parent-1"));
    }

    // ========================================================================
    // SessionManager - session access via get_session
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_add_and_read_messages() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // Add messages via get_session
        {
            let session_lock = sm.get_session(&id).await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Hello"));
        }

        // Read messages
        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.history().len(), 1);
    }

    #[tokio::test]
    async fn test_session_manager_context_usage_via_session() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.context_usage.used_tokens, 0);
        assert_eq!(session.context_usage.turns, 0);
    }

    #[tokio::test]
    async fn test_session_manager_clear_via_session() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // Add then clear
        {
            let session_lock = sm.get_session(&id).await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Hello"));
            session.add_message(Message::user("World"));
            assert_eq!(session.history().len(), 2);
            session.clear();
        }

        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert!(session.history().is_empty());
    }

    // ========================================================================
    // SessionManager - configure
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_configure_thinking() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        sm.configure(&id, Some(true), Some(1000), None)
            .await
            .unwrap();

        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert!(session.thinking_enabled);
        assert_eq!(session.thinking_budget, Some(1000));
    }

    #[tokio::test]
    async fn test_session_manager_configure_with_llm() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let llm_config =
            crate::llm::LlmConfig::new("anthropic", "claude-sonnet-4-20250514", "sk-key");
        sm.configure(&id, None, None, Some(llm_config))
            .await
            .unwrap();

        // Verify LLM config was stored
        let configs = sm.llm_configs.read().await;
        assert!(configs.contains_key(&id));
    }

    #[tokio::test]
    async fn test_session_manager_configure_not_found() {
        let sm = make_manager();
        let result = sm.configure("nonexistent", Some(true), None, None).await;
        assert!(result.is_err());
    }

    // ========================================================================
    // SessionManager - get_session
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_get_session_ok() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        let session_lock = sm.get_session(&id).await;
        assert!(session_lock.is_ok());
    }

    #[tokio::test]
    async fn test_session_manager_get_session_not_found() {
        let sm = make_manager();
        let result = sm.get_session("nonexistent").await;
        assert!(result.is_err());
    }

    // ========================================================================
    // SessionManager - session_count
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_session_count() {
        let sm = make_manager();
        assert_eq!(sm.session_count().await, 0);
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        assert_eq!(sm.session_count().await, 1);
        sm.create_session("s2".to_string(), default_config())
            .await
            .unwrap();
        assert_eq!(sm.session_count().await, 2);
        sm.destroy_session("s1").await.unwrap();
        assert_eq!(sm.session_count().await, 1);
    }

    // ========================================================================
    // Session event subscription
    // ========================================================================

    #[tokio::test]
    async fn test_session_event_tx() {
        let session = make_session("s1").await;
        let tx = session.event_tx();
        let mut rx = session.subscribe_events();

        tx.send(AgentEvent::Start {
            prompt: "test".to_string(),
        })
        .unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, AgentEvent::Start { .. }));
    }

    // ========================================================================
    // ContextUsage
    // ========================================================================

    #[test]
    fn test_context_usage_default_values() {
        let usage = ContextUsage::default();
        assert_eq!(usage.used_tokens, 0);
        assert_eq!(usage.max_tokens, 200_000);
        assert_eq!(usage.percent, 0.0);
        assert_eq!(usage.turns, 0);
    }

    // ========================================================================
    // SessionConfig default
    // ========================================================================

    #[test]
    fn test_session_config_default() {
        let config = SessionConfig::default();
        assert!(config.name.is_empty());
        assert!(config.workspace.is_empty());
        assert!(config.system_prompt.is_none());
        assert_eq!(config.max_context_length, 0);
        assert!(!config.auto_compact);
        assert_eq!(
            config.auto_compact_threshold,
            DEFAULT_AUTO_COMPACT_THRESHOLD
        );
        assert!(config.queue_config.is_none());
        assert!(config.confirmation_policy.is_none());
        assert!(config.permission_policy.is_none());
        assert!(config.parent_id.is_none());
    }

    // ========================================================================
    // Additional Coverage Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_update_usage_with_cost() {
        let mut session = make_session("s1").await;
        session.model_name = Some("claude-3-5-sonnet-20241022".to_string());

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        session.update_usage(&usage);
        assert_eq!(session.total_usage.prompt_tokens, 1000);
        assert_eq!(session.total_usage.completion_tokens, 500);
        assert!(session.total_cost > 0.0);
    }

    #[tokio::test]
    async fn test_session_update_usage_no_model() {
        let mut session = make_session("s1").await;
        session.model_name = None;

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        session.update_usage(&usage);
        assert_eq!(session.total_cost, 0.0);
    }

    #[tokio::test]
    async fn test_session_update_usage_unknown_model() {
        let mut session = make_session("s1").await;
        session.model_name = Some("unknown-model-xyz".to_string());

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        session.update_usage(&usage);
        assert_eq!(session.total_cost, 0.0);
    }

    #[tokio::test]
    async fn test_session_to_session_data() {
        let mut session = make_session("s1").await;
        session.add_message(Message::user("test"));
        session.model_name = Some("test-model".to_string());

        let data = session.to_session_data(None);
        assert_eq!(data.id, "s1");
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.model_name, Some("test-model".to_string()));
        assert!(data.llm_config.is_none());
    }

    #[tokio::test]
    async fn test_session_to_session_data_with_llm_config() {
        let session = make_session("s1").await;
        let llm_config = LlmConfigData {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key: None,
            base_url: None,
        };

        let data = session.to_session_data(Some(llm_config.clone()));
        assert!(data.llm_config.is_some());
        assert_eq!(data.llm_config.unwrap().model, "claude-sonnet-4-20250514");
    }

    #[tokio::test]
    async fn test_session_restore_from_data() {
        let mut session = make_session("s1").await;

        let data = SessionData {
            id: "s1".to_string(),
            config: default_config(),
            state: SessionState::Paused,
            messages: vec![Message::user("restored")],
            context_usage: ContextUsage {
                used_tokens: 100,
                max_tokens: 200_000,
                percent: 0.0005,
                turns: 1,
            },
            total_usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            total_cost: 0.05,
            model_name: Some("test-model".to_string()),
            tool_names: vec![],
            thinking_enabled: true,
            thinking_budget: Some(5000),
            created_at: 1000,
            updated_at: 2000,
            llm_config: None,
            tasks: vec![Task::new("t1", "test task")],
            parent_id: Some("parent".to_string()),
            cost_records: Vec::new(),
        };

        session.restore_from_data(&data);

        assert_eq!(session.state, SessionState::Paused);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.context_usage.used_tokens, 100);
        assert_eq!(session.total_usage.prompt_tokens, 100);
        assert_eq!(session.total_cost, 0.05);
        assert_eq!(session.model_name, Some("test-model".to_string()));
        assert!(session.thinking_enabled);
        assert_eq!(session.thinking_budget, Some(5000));
        assert_eq!(session.created_at, 1000);
        assert_eq!(session.updated_at, 2000);
        assert_eq!(session.tasks.len(), 1);
        assert_eq!(session.parent_id, Some("parent".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_get_child_sessions() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let mut child_config = default_config();
        child_config.parent_id = Some("parent".to_string());
        sm.create_session("child1".to_string(), child_config.clone())
            .await
            .unwrap();
        sm.create_session("child2".to_string(), child_config)
            .await
            .unwrap();

        let children = sm.get_child_sessions("parent").await;
        assert_eq!(children.len(), 2);
        assert!(children.contains(&"child1".to_string()));
        assert!(children.contains(&"child2".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_get_child_sessions_none() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let children = sm.get_child_sessions("parent").await;
        assert!(children.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_is_child_session() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let mut child_config = default_config();
        child_config.parent_id = Some("parent".to_string());
        sm.create_session("child".to_string(), child_config)
            .await
            .unwrap();

        assert!(!sm.is_child_session("parent").await.unwrap());
        assert!(sm.is_child_session("child").await.unwrap());
    }

    #[tokio::test]
    async fn test_session_manager_is_child_session_not_found() {
        let sm = make_manager();
        let result = sm.is_child_session("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_list_tools() {
        let sm = make_manager();
        let tools = sm.list_tools();
        assert!(!tools.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_tool_executor() {
        let sm = make_manager();
        let executor = sm.tool_executor();
        assert_eq!(executor.workspace().to_str().unwrap(), "/tmp");
    }

    #[tokio::test]
    async fn test_session_manager_context_usage() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let usage = sm.context_usage("s1").await.unwrap();
        assert_eq!(usage.used_tokens, 0);
        assert_eq!(usage.turns, 0);
    }

    #[tokio::test]
    async fn test_session_manager_context_usage_not_found() {
        let sm = make_manager();
        let result = sm.context_usage("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_history() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        {
            let session_lock = sm.get_session("s1").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("test"));
        }

        let history = sm.history("s1").await.unwrap();
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn test_session_manager_history_not_found() {
        let sm = make_manager();
        let result = sm.history("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_clear() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        {
            let session_lock = sm.get_session("s1").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("test"));
        }

        sm.clear("s1").await.unwrap();

        let history = sm.history("s1").await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_clear_not_found() {
        let sm = make_manager();
        let result = sm.clear("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_cancel_operation_no_operation() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let cancelled = sm.cancel_operation("s1").await.unwrap();
        assert!(!cancelled);
    }

    #[tokio::test]
    async fn test_session_manager_cancel_operation_not_found() {
        let sm = make_manager();
        let result = sm.cancel_operation("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_get_all_sessions() {
        let sm = make_manager();
        assert!(sm.get_all_sessions().await.is_empty());

        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        sm.create_session("s2".to_string(), default_config())
            .await
            .unwrap();

        let sessions = sm.get_all_sessions().await;
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_session_queue_metrics() {
        let session = make_session("s1").await;
        session.start_queue().await.unwrap();
        let _metrics = session.queue_metrics().await;
        // Metrics may or may not be available depending on queue state
        // Just verify the call doesn't panic
    }

    #[tokio::test]
    async fn test_session_queue_stats() {
        let session = make_session("s1").await;
        let stats = session.queue_stats().await;
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
    }

    #[tokio::test]
    async fn test_session_dead_letters() {
        let session = make_session("s1").await;
        let dead_letters = session.dead_letters().await;
        assert!(dead_letters.is_empty());
    }

    #[tokio::test]
    async fn test_session_stop_queue() {
        let session = make_session("s1").await;
        session.stop_queue().await;
        // No assertion needed, just verify it doesn't panic
    }

    #[tokio::test]
    async fn test_session_config_with_security() {
        let security_config = crate::security::SecurityConfig {
            enabled: true,
            ..Default::default()
        };

        let config = SessionConfig {
            security_config: Some(security_config),
            ..default_config()
        };

        let session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.security_provider.is_some());
    }

    #[tokio::test]
    async fn test_session_config_security_disabled() {
        let security_config = crate::security::SecurityConfig {
            enabled: false,
            ..Default::default()
        };

        let config = SessionConfig {
            security_config: Some(security_config),
            ..default_config()
        };

        let session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.security_provider.is_none());
    }

    #[tokio::test]
    async fn test_session_pause_from_non_active() {
        let mut session = make_session("s1").await;
        session.set_completed();

        let paused = session.pause();
        assert!(!paused);
        assert_eq!(session.state, SessionState::Completed);
    }

    #[tokio::test]
    async fn test_session_resume_from_non_paused() {
        let mut session = make_session("s1").await;
        assert_eq!(session.state, SessionState::Active);

        let resumed = session.resume();
        assert!(!resumed);
        assert_eq!(session.state, SessionState::Active);
    }

    #[tokio::test]
    async fn test_session_manager_pause_session_not_found() {
        let sm = make_manager();
        let result = sm.pause_session("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_resume_session_not_found() {
        let sm = make_manager();
        let result = sm.resume_session("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_compact_not_enough_messages() {
        let mut session = make_session("s1").await;

        // Add only a few messages (below MIN_MESSAGES_FOR_COMPACTION = 30)
        for i in 0..10 {
            session.add_message(Message::user(&format!("msg {}", i)));
        }

        struct StubClient;
        #[async_trait::async_trait]
        impl LlmClient for StubClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                unimplemented!()
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(StubClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());
        assert_eq!(session.messages.len(), 10); // No compaction happened
    }

    #[tokio::test]
    async fn test_session_compact_nothing_to_summarize() {
        let mut session = make_session("s1").await;

        // Add 31 messages so compaction triggers
        for i in 0..31 {
            session.add_message(Message::user(&format!("msg {}", i)));
        }

        struct SummaryClient;
        #[async_trait::async_trait]
        impl LlmClient for SummaryClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                Ok(crate::llm::LlmResponse {
                    message: Message {
                        role: "assistant".to_string(),
                        content: vec![crate::llm::ContentBlock::Text {
                            text: "Summary of conversation".to_string(),
                        }],
                        reasoning_content: None,
                    },
                    usage: crate::llm::TokenUsage::default(),
                    stop_reason: None,
                    meta: None,
                })
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                unimplemented!()
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(SummaryClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());
        // After compaction, message count should be reduced
        assert!(session.messages.len() <= 31);
    }

    #[test]
    fn test_default_auto_compact_threshold_function() {
        assert_eq!(default_auto_compact_threshold(), 0.80);
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let child_id = sm
            .create_child_session("parent", "child".to_string(), default_config())
            .await
            .unwrap();

        assert_eq!(child_id, "child");

        let session_lock = sm.get_session("child").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.parent_id, Some("parent".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session_parent_not_found() {
        let sm = make_manager();
        let result = sm
            .create_child_session("nonexistent", "child".to_string(), default_config())
            .await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Cost Records Tests
    // ========================================================================

    #[tokio::test]
    async fn test_update_usage_records_cost_record() {
        let config = default_config();
        let mut session = Session::new("test-cost".to_string(), config, vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        let usage = crate::llm::TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);

        assert_eq!(session.cost_records.len(), 1);
        assert_eq!(session.cost_records[0].model, "gpt-4o");
        assert_eq!(session.cost_records[0].prompt_tokens, 1000);
        assert_eq!(session.cost_records[0].completion_tokens, 500);
        assert!(session.cost_records[0].cost_usd.is_some());
    }

    #[tokio::test]
    async fn test_update_usage_accumulates_cost_records() {
        let config = default_config();
        let mut session = Session::new("test-cost-2".to_string(), config, vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        for _ in 0..3 {
            let usage = crate::llm::TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            };
            session.update_usage(&usage);
        }

        assert_eq!(session.cost_records.len(), 3);
    }

    #[tokio::test]
    async fn test_to_session_data_includes_cost_records() {
        let config = default_config();
        let mut session = Session::new("test-cost-3".to_string(), config, vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        let usage = crate::llm::TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);

        let data = session.to_session_data(None);
        assert_eq!(data.cost_records.len(), 1);
        assert_eq!(data.cost_records[0].model, "gpt-4o");
    }

    #[tokio::test]
    async fn test_restore_from_data_restores_cost_records() {
        let config = default_config();
        let mut session = Session::new("test-cost-4".to_string(), config.clone(), vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        let usage = crate::llm::TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);

        let data = session.to_session_data(None);

        // Create a fresh session and restore
        let mut fresh_session = Session::new("test-cost-4".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(fresh_session.cost_records.is_empty());

        fresh_session.restore_from_data(&data);
        assert_eq!(fresh_session.cost_records.len(), 1);
        assert_eq!(fresh_session.cost_records[0].model, "gpt-4o");
    }
}
