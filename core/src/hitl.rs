//! Human-in-the-Loop (HITL) confirmation mechanism
//!
//! Provides tool execution confirmation before running potentially dangerous tools.
//! Supports:
//! - Tool categorization (ReadOnly, Mutating, Control)
//! - Configurable confirmation policies
//! - Timeout handling with configurable actions
//! - YOLO mode for lane-based auto-approval

use crate::agent::AgentEvent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, oneshot, RwLock};

/// Tool category for determining confirmation requirements
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCategory {
    /// Read-only operations (read, glob, ls, grep) - auto-approved by default
    ReadOnly,
    /// Mutating operations (bash, write, edit) - may require confirmation
    Mutating,
    /// Control operations (internal commands) - auto-approved
    Control,
}

impl ToolCategory {
    /// Classify a tool by its name
    pub fn from_tool_name(tool_name: &str) -> Self {
        match tool_name {
            "read" | "glob" | "ls" | "grep" | "list_files" | "search" => ToolCategory::ReadOnly,
            "bash" | "write" | "edit" | "delete" | "move" | "copy" | "execute" => {
                ToolCategory::Mutating
            }
            _ => ToolCategory::Mutating, // Default to mutating for unknown tools
        }
    }
}

/// Session lane for YOLO mode auto-approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionLane {
    /// Control operations (P0) - pause, resume, cancel
    Control,
    /// Query operations (P1) - read, glob, ls, grep
    Query,
    /// Execute operations (P2) - bash, write, edit
    Execute,
    /// Generate operations (P3) - LLM calls
    Generate,
}

impl SessionLane {
    /// Get the priority level (lower = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            SessionLane::Control => 0,
            SessionLane::Query => 1,
            SessionLane::Execute => 2,
            SessionLane::Generate => 3,
        }
    }

    /// Map a tool name to its lane
    pub fn from_tool_name(tool_name: &str) -> Self {
        match tool_name {
            "read" | "glob" | "ls" | "grep" | "list_files" | "search" => SessionLane::Query,
            "bash" | "write" | "edit" | "delete" | "move" | "copy" | "execute" => {
                SessionLane::Execute
            }
            _ => SessionLane::Execute, // Default to Execute lane
        }
    }
}

/// Action to take when confirmation times out
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeoutAction {
    /// Reject the tool execution on timeout
    #[default]
    Reject,
    /// Auto-approve the tool execution on timeout
    AutoApprove,
}

impl TimeoutAction {
    /// Convert to proto i32 value
    pub fn to_proto_i32(self) -> i32 {
        match self {
            TimeoutAction::Reject => 1,
            TimeoutAction::AutoApprove => 2,
        }
    }

    /// Create from proto i32 value
    pub fn from_proto_i32(value: i32) -> Self {
        match value {
            2 => TimeoutAction::AutoApprove,
            _ => TimeoutAction::Reject,
        }
    }
}

/// Confirmation policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationPolicy {
    /// Whether HITL is enabled (default: false, all tools auto-approved)
    pub enabled: bool,

    /// Tools that are always auto-approved (bypasses HITL)
    pub auto_approve_tools: HashSet<String>,

    /// Tools that always require confirmation (even in YOLO mode)
    pub require_confirm_tools: HashSet<String>,

    /// Default timeout in milliseconds (default: 30000 = 30s)
    pub default_timeout_ms: u64,

    /// Action to take on timeout (default: Reject)
    pub timeout_action: TimeoutAction,

    /// YOLO mode: lanes that auto-approve without confirmation
    /// When a lane is in this set, tools in that lane are auto-approved
    pub yolo_lanes: HashSet<SessionLane>,
}

impl Default for ConfirmationPolicy {
    fn default() -> Self {
        Self {
            enabled: false, // HITL disabled by default
            auto_approve_tools: HashSet::new(),
            require_confirm_tools: HashSet::new(),
            default_timeout_ms: 30_000, // 30 seconds
            timeout_action: TimeoutAction::Reject,
            yolo_lanes: HashSet::new(), // No YOLO lanes by default
        }
    }
}

impl ConfirmationPolicy {
    /// Create a new policy with HITL enabled
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Enable YOLO mode for specific lanes
    pub fn with_yolo_lanes(mut self, lanes: impl IntoIterator<Item = SessionLane>) -> Self {
        self.yolo_lanes = lanes.into_iter().collect();
        self
    }

    /// Add tools to auto-approve list
    pub fn with_auto_approve_tools(mut self, tools: impl IntoIterator<Item = String>) -> Self {
        self.auto_approve_tools = tools.into_iter().collect();
        self
    }

    /// Add tools to require-confirm list
    pub fn with_require_confirm_tools(mut self, tools: impl IntoIterator<Item = String>) -> Self {
        self.require_confirm_tools = tools.into_iter().collect();
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout_ms: u64, action: TimeoutAction) -> Self {
        self.default_timeout_ms = timeout_ms;
        self.timeout_action = action;
        self
    }

    /// Check if a tool requires confirmation
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        // If HITL is disabled, no confirmation needed
        if !self.enabled {
            return false;
        }

        // Check explicit require list first
        if self.require_confirm_tools.contains(tool_name) {
            return true;
        }

        // Check explicit auto-approve list
        if self.auto_approve_tools.contains(tool_name) {
            return false;
        }

        // Check YOLO mode for the tool's lane
        let lane = SessionLane::from_tool_name(tool_name);
        if self.yolo_lanes.contains(&lane) {
            return false; // YOLO mode auto-approves this lane
        }

        // Check tool category - only Mutating tools need confirmation
        let category = ToolCategory::from_tool_name(tool_name);
        matches!(category, ToolCategory::Mutating)
    }
}

/// Confirmation response from user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationResponse {
    /// Whether the tool execution was approved
    pub approved: bool,
    /// Optional reason for rejection
    pub reason: Option<String>,
}

/// A pending confirmation request
pub struct PendingConfirmation {
    /// Tool call ID
    pub tool_id: String,
    /// Tool name
    pub tool_name: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// When the confirmation was requested
    pub created_at: Instant,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Channel to send the response
    response_tx: oneshot::Sender<ConfirmationResponse>,
}

impl PendingConfirmation {
    /// Check if this confirmation has timed out
    pub fn is_timed_out(&self) -> bool {
        self.created_at.elapsed() > Duration::from_millis(self.timeout_ms)
    }

    /// Get remaining time until timeout in milliseconds
    pub fn remaining_ms(&self) -> u64 {
        let elapsed = self.created_at.elapsed().as_millis() as u64;
        self.timeout_ms.saturating_sub(elapsed)
    }
}

/// Manages confirmation requests for a session
pub struct ConfirmationManager {
    /// Confirmation policy
    policy: RwLock<ConfirmationPolicy>,
    /// Pending confirmations by tool_id
    pending: Arc<RwLock<HashMap<String, PendingConfirmation>>>,
    /// Event broadcaster
    event_tx: broadcast::Sender<AgentEvent>,
}

impl ConfirmationManager {
    /// Create a new confirmation manager
    pub fn new(policy: ConfirmationPolicy, event_tx: broadcast::Sender<AgentEvent>) -> Self {
        Self {
            policy: RwLock::new(policy),
            pending: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    /// Get the current policy
    pub async fn policy(&self) -> ConfirmationPolicy {
        self.policy.read().await.clone()
    }

    /// Update the confirmation policy
    pub async fn set_policy(&self, policy: ConfirmationPolicy) {
        *self.policy.write().await = policy;
    }

    /// Check if a tool requires confirmation
    pub async fn requires_confirmation(&self, tool_name: &str) -> bool {
        self.policy.read().await.requires_confirmation(tool_name)
    }

    /// Request confirmation for a tool execution
    ///
    /// Returns a receiver that will receive the confirmation response.
    /// Emits a ConfirmationRequired event.
    pub async fn request_confirmation(
        &self,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> oneshot::Receiver<ConfirmationResponse> {
        let (tx, rx) = oneshot::channel();

        let policy = self.policy.read().await;
        let timeout_ms = policy.default_timeout_ms;
        drop(policy);

        let pending = PendingConfirmation {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            args: args.clone(),
            created_at: Instant::now(),
            timeout_ms,
            response_tx: tx,
        };

        // Store the pending confirmation
        {
            let mut pending_map = self.pending.write().await;
            pending_map.insert(tool_id.to_string(), pending);
        }

        // Emit confirmation required event
        let _ = self.event_tx.send(AgentEvent::ConfirmationRequired {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            args: args.clone(),
            timeout_ms,
        });

        rx
    }

    /// Handle a confirmation response from the user
    ///
    /// Returns Ok(true) if the confirmation was found and processed,
    /// Ok(false) if no pending confirmation was found.
    pub async fn confirm(
        &self,
        tool_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<bool, String> {
        let pending = {
            let mut pending_map = self.pending.write().await;
            pending_map.remove(tool_id)
        };

        if let Some(confirmation) = pending {
            // Emit confirmation received event
            let _ = self.event_tx.send(AgentEvent::ConfirmationReceived {
                tool_id: tool_id.to_string(),
                approved,
                reason: reason.clone(),
            });

            // Send the response
            let response = ConfirmationResponse { approved, reason };
            let _ = confirmation.response_tx.send(response);

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Check for and handle timed out confirmations
    ///
    /// Returns the number of confirmations that timed out.
    pub async fn check_timeouts(&self) -> usize {
        let policy = self.policy.read().await;
        let timeout_action = policy.timeout_action;
        drop(policy);

        let mut timed_out = Vec::new();

        // Find timed out confirmations
        {
            let pending_map = self.pending.read().await;
            for (tool_id, pending) in pending_map.iter() {
                if pending.is_timed_out() {
                    timed_out.push(tool_id.clone());
                }
            }
        }

        // Handle timed out confirmations
        for tool_id in &timed_out {
            let pending = {
                let mut pending_map = self.pending.write().await;
                pending_map.remove(tool_id)
            };

            if let Some(confirmation) = pending {
                let (approved, action_taken) = match timeout_action {
                    TimeoutAction::Reject => (false, "rejected"),
                    TimeoutAction::AutoApprove => (true, "auto_approved"),
                };

                // Emit timeout event
                let _ = self.event_tx.send(AgentEvent::ConfirmationTimeout {
                    tool_id: tool_id.clone(),
                    action_taken: action_taken.to_string(),
                });

                // Send the response
                let response = ConfirmationResponse {
                    approved,
                    reason: Some(format!("Confirmation timed out, action: {}", action_taken)),
                };
                let _ = confirmation.response_tx.send(response);
            }
        }

        timed_out.len()
    }

    /// Get the number of pending confirmations
    pub async fn pending_count(&self) -> usize {
        self.pending.read().await.len()
    }

    /// Get pending confirmation details (for debugging/status)
    pub async fn pending_confirmations(&self) -> Vec<(String, String, u64)> {
        let pending_map = self.pending.read().await;
        pending_map
            .values()
            .map(|p| (p.tool_id.clone(), p.tool_name.clone(), p.remaining_ms()))
            .collect()
    }

    /// Cancel a pending confirmation
    pub async fn cancel(&self, tool_id: &str) -> bool {
        let pending = {
            let mut pending_map = self.pending.write().await;
            pending_map.remove(tool_id)
        };

        if let Some(confirmation) = pending {
            let response = ConfirmationResponse {
                approved: false,
                reason: Some("Confirmation cancelled".to_string()),
            };
            let _ = confirmation.response_tx.send(response);
            true
        } else {
            false
        }
    }

    /// Cancel all pending confirmations
    pub async fn cancel_all(&self) -> usize {
        let pending_list: Vec<_> = {
            let mut pending_map = self.pending.write().await;
            pending_map.drain().collect()
        };

        let count = pending_list.len();

        for (_, confirmation) in pending_list {
            let response = ConfirmationResponse {
                approved: false,
                reason: Some("Confirmation cancelled".to_string()),
            };
            let _ = confirmation.response_tx.send(response);
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // ToolCategory Tests
    // ========================================================================

    #[test]
    fn test_tool_category() {
        assert_eq!(ToolCategory::from_tool_name("read"), ToolCategory::ReadOnly);
        assert_eq!(ToolCategory::from_tool_name("glob"), ToolCategory::ReadOnly);
        assert_eq!(ToolCategory::from_tool_name("bash"), ToolCategory::Mutating);
        assert_eq!(
            ToolCategory::from_tool_name("write"),
            ToolCategory::Mutating
        );
        assert_eq!(
            ToolCategory::from_tool_name("unknown"),
            ToolCategory::Mutating
        );
    }

    #[test]
    fn test_tool_category_all_readonly() {
        let readonly_tools = ["read", "glob", "ls", "grep", "list_files", "search"];
        for tool in readonly_tools {
            assert_eq!(
                ToolCategory::from_tool_name(tool),
                ToolCategory::ReadOnly,
                "Tool '{}' should be ReadOnly",
                tool
            );
        }
    }

    #[test]
    fn test_tool_category_all_mutating() {
        let mutating_tools = ["bash", "write", "edit", "delete", "move", "copy", "execute"];
        for tool in mutating_tools {
            assert_eq!(
                ToolCategory::from_tool_name(tool),
                ToolCategory::Mutating,
                "Tool '{}' should be Mutating",
                tool
            );
        }
    }

    // ========================================================================
    // SessionLane Tests
    // ========================================================================

    #[test]
    fn test_session_lane() {
        assert_eq!(SessionLane::from_tool_name("read"), SessionLane::Query);
        assert_eq!(SessionLane::from_tool_name("grep"), SessionLane::Query);
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
        let query_tools = ["read", "glob", "ls", "grep", "list_files", "search"];
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

    #[test]
    fn test_timeout_action_proto_conversion() {
        assert_eq!(TimeoutAction::Reject.to_proto_i32(), 1);
        assert_eq!(TimeoutAction::AutoApprove.to_proto_i32(), 2);

        assert_eq!(TimeoutAction::from_proto_i32(1), TimeoutAction::Reject);
        assert_eq!(TimeoutAction::from_proto_i32(2), TimeoutAction::AutoApprove);
        assert_eq!(TimeoutAction::from_proto_i32(0), TimeoutAction::Reject); // Unknown defaults to Reject
        assert_eq!(TimeoutAction::from_proto_i32(99), TimeoutAction::Reject);
    }

    // ========================================================================
    // ConfirmationPolicy Tests
    // ========================================================================

    #[test]
    fn test_confirmation_policy_default() {
        let policy = ConfirmationPolicy::default();
        assert!(!policy.enabled);
        assert!(!policy.requires_confirmation("bash")); // HITL disabled
        assert!(!policy.requires_confirmation("write"));
        assert!(!policy.requires_confirmation("read"));
    }

    #[test]
    fn test_confirmation_policy_enabled() {
        let policy = ConfirmationPolicy::enabled();
        assert!(policy.enabled);
        assert!(policy.requires_confirmation("bash")); // Mutating tool
        assert!(policy.requires_confirmation("write")); // Mutating tool
        assert!(!policy.requires_confirmation("read")); // ReadOnly tool
        assert!(!policy.requires_confirmation("grep")); // ReadOnly tool
    }

    #[test]
    fn test_confirmation_policy_yolo_mode() {
        let policy = ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Execute]);

        assert!(!policy.requires_confirmation("bash")); // Execute lane in YOLO mode
        assert!(!policy.requires_confirmation("write")); // Execute lane in YOLO mode
        assert!(!policy.requires_confirmation("read")); // ReadOnly
    }

    #[test]
    fn test_confirmation_policy_yolo_multiple_lanes() {
        let policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Query, SessionLane::Execute]);

        // All tools in YOLO lanes should be auto-approved
        assert!(!policy.requires_confirmation("bash")); // Execute
        assert!(!policy.requires_confirmation("read")); // Query
        assert!(!policy.requires_confirmation("grep")); // Query
    }

    #[test]
    fn test_confirmation_policy_explicit_lists() {
        let policy = ConfirmationPolicy::enabled()
            .with_auto_approve_tools(["bash".to_string()])
            .with_require_confirm_tools(["read".to_string()]);

        assert!(!policy.requires_confirmation("bash")); // Explicitly auto-approved
        assert!(policy.requires_confirmation("read")); // Explicitly required
        assert!(policy.requires_confirmation("write")); // Default for Mutating
    }

    #[test]
    fn test_confirmation_policy_explicit_overrides_yolo() {
        // require_confirm_tools should override YOLO mode
        let policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Execute])
            .with_require_confirm_tools(["bash".to_string()]);

        assert!(policy.requires_confirmation("bash")); // Explicitly required, overrides YOLO
        assert!(!policy.requires_confirmation("write")); // Still in YOLO mode
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

        assert!(manager.requires_confirmation("bash").await);
        assert!(!manager.requires_confirmation("read").await);
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
}
