//! Human-in-the-Loop (HITL) confirmation mechanism
//!
//! Provides the runtime confirmation flow for tool execution. Works with
//! `PermissionPolicy` (permissions.rs) which decides Allow/Deny/Ask.
//! When the permission decision is `Ask`, this module handles:
//! - Interactive confirmation request/response flow
//! - Timeout handling with configurable actions
//! - YOLO mode for lane-based auto-approval (skips confirmation for entire lanes)

use crate::agent::AgentEvent;
use crate::queue::SessionLane;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, oneshot, RwLock};

/// Action to take when confirmation times out
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeoutAction {
    /// Reject the tool execution on timeout
    #[default]
    Reject,
    /// Auto-approve the tool execution on timeout
    AutoApprove,
}

/// Confirmation policy configuration
///
/// Controls the runtime behavior of HITL confirmation flow.
/// The *decision* of whether to ask is made by `PermissionPolicy` (permissions.rs).
/// This policy controls *how* the confirmation works: timeouts, YOLO lanes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationPolicy {
    /// Whether HITL is enabled (default: false, all tools auto-approved)
    pub enabled: bool,

    /// Default timeout in milliseconds (default: 30000 = 30s)
    pub default_timeout_ms: u64,

    /// Action to take on timeout (default: Reject)
    pub timeout_action: TimeoutAction,

    /// YOLO mode: lanes that auto-approve without confirmation.
    /// When a lane is in this set, tools in that lane skip confirmation
    /// even if `PermissionPolicy` returns `Ask`.
    pub yolo_lanes: HashSet<SessionLane>,
}

impl Default for ConfirmationPolicy {
    fn default() -> Self {
        Self {
            enabled: false,             // HITL disabled by default
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

    /// Set timeout
    pub fn with_timeout(mut self, timeout_ms: u64, action: TimeoutAction) -> Self {
        self.default_timeout_ms = timeout_ms;
        self.timeout_action = action;
        self
    }

    /// Check if a tool should skip confirmation (YOLO lane check)
    ///
    /// Returns true if the tool's lane is in YOLO mode, meaning it should
    /// be auto-approved even when `PermissionPolicy` returns `Ask`.
    pub fn is_yolo(&self, tool_name: &str) -> bool {
        if !self.enabled {
            return true; // HITL disabled = everything auto-approved
        }
        let lane = SessionLane::from_tool_name(tool_name);
        self.yolo_lanes.contains(&lane)
    }

    /// Check if a tool requires confirmation
    ///
    /// This is the inverse of `is_yolo()` — returns true when HITL is enabled
    /// and the tool's lane is NOT in YOLO mode.
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        !self.is_yolo(tool_name)
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

/// Snapshot of a pending confirmation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConfirmationInfo {
    pub tool_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub remaining_ms: u64,
}

/// Trait for confirmation providers (HITL runtime behavior)
///
/// This trait abstracts the confirmation flow, allowing different implementations
/// (e.g., interactive, auto-approve, test mocks) while keeping the agent logic clean.
#[async_trait::async_trait]
pub trait ConfirmationProvider: Send + Sync {
    /// Check if a tool requires confirmation
    async fn requires_confirmation(&self, tool_name: &str) -> bool;

    /// Request confirmation for a tool execution
    ///
    /// Returns a receiver that will receive the confirmation response.
    async fn request_confirmation(
        &self,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> oneshot::Receiver<ConfirmationResponse>;

    /// Handle a confirmation response from the user
    ///
    /// Returns Ok(true) if the confirmation was found and processed,
    /// Ok(false) if no pending confirmation was found.
    async fn confirm(
        &self,
        tool_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<bool, String>;

    /// Get the current policy
    async fn policy(&self) -> ConfirmationPolicy;

    /// Update the confirmation policy
    async fn set_policy(&self, policy: ConfirmationPolicy);

    /// Check for and handle timed out confirmations
    async fn check_timeouts(&self) -> usize;

    /// Cancel all pending confirmations
    async fn cancel_all(&self) -> usize;

    /// Snapshot pending confirmations for status inspection.
    async fn pending_confirmations(&self) -> Vec<PendingConfirmationInfo> {
        Vec::new()
    }
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

    /// Get detailed pending confirmation snapshots.
    pub async fn pending_confirmation_details(&self) -> Vec<PendingConfirmationInfo> {
        let pending_map = self.pending.read().await;
        pending_map
            .values()
            .map(|p| PendingConfirmationInfo {
                tool_id: p.tool_id.clone(),
                tool_name: p.tool_name.clone(),
                args: p.args.clone(),
                remaining_ms: p.remaining_ms(),
            })
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

// Implement ConfirmationProvider trait for ConfirmationManager
#[async_trait::async_trait]
impl ConfirmationProvider for ConfirmationManager {
    async fn requires_confirmation(&self, tool_name: &str) -> bool {
        self.requires_confirmation(tool_name).await
    }

    async fn request_confirmation(
        &self,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> oneshot::Receiver<ConfirmationResponse> {
        self.request_confirmation(tool_id, tool_name, args).await
    }

    async fn confirm(
        &self,
        tool_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<bool, String> {
        self.confirm(tool_id, approved, reason).await
    }

    async fn policy(&self) -> ConfirmationPolicy {
        self.policy().await
    }

    async fn set_policy(&self, policy: ConfirmationPolicy) {
        self.set_policy(policy).await
    }

    async fn check_timeouts(&self) -> usize {
        self.check_timeouts().await
    }

    async fn cancel_all(&self) -> usize {
        self.cancel_all().await
    }

    async fn pending_confirmations(&self) -> Vec<PendingConfirmationInfo> {
        self.pending_confirmation_details().await
    }
}

/// A confirmation provider that never requires confirmation.
///
/// Used for child runs where the agent's permission policy already provides
/// the access control boundary. When permissions return `Ask` for a tool not
/// explicitly covered, this provider auto-approves instead of blocking.
pub struct AutoApproveConfirmation;

#[async_trait::async_trait]
impl ConfirmationProvider for AutoApproveConfirmation {
    async fn requires_confirmation(&self, _tool_name: &str) -> bool {
        false
    }

    async fn request_confirmation(
        &self,
        _tool_id: &str,
        _tool_name: &str,
        _args: &serde_json::Value,
    ) -> oneshot::Receiver<ConfirmationResponse> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(ConfirmationResponse {
            approved: true,
            reason: None,
        });
        rx
    }

    async fn confirm(
        &self,
        _tool_id: &str,
        _approved: bool,
        _reason: Option<String>,
    ) -> Result<bool, String> {
        Ok(false)
    }

    async fn policy(&self) -> ConfirmationPolicy {
        ConfirmationPolicy {
            enabled: false,
            ..ConfirmationPolicy::default()
        }
    }

    async fn set_policy(&self, _policy: ConfirmationPolicy) {}

    async fn check_timeouts(&self) -> usize {
        0
    }

    async fn cancel_all(&self) -> usize {
        0
    }
}

#[cfg(test)]
#[path = "hitl/tests.rs"]
mod tests;
