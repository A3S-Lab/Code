//! Node Session queue and governance methods.

use super::session::Session;
use super::*;

#[napi]
impl Session {
    // ========================================================================
    // Advanced optional Queue API
    // ========================================================================

    /// Check if this session has an advanced lane queue configured.
    #[napi]
    pub fn has_queue(&self) -> bool {
        self.inner.has_queue()
    }

    /// Configure a lane's handler mode for explicit external/hybrid dispatch.
    ///
    /// @param lane - "control", "query", "execute", or "generate"
    /// @param config - { mode: "internal"|"external"|"hybrid", timeoutMs?: number }
    #[napi]
    pub async fn set_lane_handler(
        &self,
        lane: String,
        config: LaneHandlerConfig,
    ) -> napi::Result<()> {
        let rust_lane = parse_lane(&lane)?;
        let rust_mode = parse_handler_mode(&config.mode)?;
        let rust_config = RustLaneHandlerConfig {
            mode: rust_mode,
            timeout_ms: config.timeout_ms.unwrap_or(60000) as u64,
        };
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.set_lane_handler(rust_lane, rust_config).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(())
    }

    /// Complete an external queue task by ID.
    ///
    /// @param taskId - The task identifier
    /// @param result - { success: boolean, result?: any, error?: string }
    /// @returns true if found, false if not found
    #[napi]
    pub async fn complete_external_task(
        &self,
        task_id: String,
        result: ExternalTaskResult,
    ) -> napi::Result<bool> {
        let ext_result = RustExternalTaskResult {
            success: result.success,
            result: result.result.unwrap_or(serde_json::json!({})),
            error: result.error,
        };
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.complete_external_task(&task_id, ext_result).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }

    /// Get pending external queue tasks.
    #[napi]
    pub async fn pending_external_tasks(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let tasks = get_runtime()
            .spawn(async move { session.pending_external_tasks().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(&tasks)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    // ========================================================================
    // HITL confirmation API
    // ========================================================================

    /// Return pending HITL tool confirmations for this session.
    #[napi]
    pub async fn pending_confirmations(&self) -> napi::Result<Vec<PendingConfirmation>> {
        let session = self.inner.clone();
        let pending = get_runtime()
            .spawn(async move { session.pending_confirmations().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(pending.into_iter().map(PendingConfirmation::from).collect())
    }

    /// Resolve a pending HITL tool confirmation.
    ///
    /// @param toolId - Tool call ID from a `confirmation_required` event.
    /// @param approved - Whether the tool execution should proceed.
    /// @param reason - Optional human-readable reason for audit/UI display.
    /// @returns true if a pending confirmation was found and completed.
    #[napi]
    pub async fn confirm_tool_use(
        &self,
        tool_id: String,
        approved: bool,
        reason: Option<String>,
    ) -> napi::Result<bool> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.confirm_tool_use(&tool_id, approved, reason).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    /// Cancel all pending HITL confirmations for this session.
    #[napi]
    pub async fn cancel_confirmations(&self) -> napi::Result<u32> {
        let session = self.inner.clone();
        let count = get_runtime()
            .spawn(async move { session.cancel_confirmations().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(count as u32)
    }

    /// Get optional queue statistics.
    #[napi]
    pub async fn queue_stats(&self) -> napi::Result<QueueStats> {
        let session = self.inner.clone();
        let stats = get_runtime()
            .spawn(async move { session.queue_stats().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(QueueStats {
            total_pending: stats.total_pending as u32,
            total_active: stats.total_active as u32,
            external_pending: stats.external_pending as u32,
        })
    }

    /// Return compact execution trace events recorded for this session.
    #[napi]
    pub fn trace_events(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.trace_events())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return structured verification reports recorded for this session.
    #[napi]
    pub fn verification_reports(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.verification_reports())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Add externally produced verification reports to this session.
    #[napi]
    pub fn record_verification_reports(&self, reports: serde_json::Value) -> napi::Result<()> {
        let reports = verification_reports_from_value(reports)?;
        self.inner.record_verification_reports(reports);
        Ok(())
    }

    /// Return a structured verification summary for this session.
    #[napi]
    pub fn verification_summary(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.verification_summary())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return a concise human-readable verification summary for this session.
    #[napi]
    pub fn verification_summary_text(&self) -> String {
        self.inner.verification_summary_text()
    }

    /// Run verification commands and return a structured verification report.
    #[napi]
    pub async fn verify_commands(
        &self,
        subject: String,
        commands: Vec<VerificationCommand>,
    ) -> napi::Result<serde_json::Value> {
        let rust_commands = commands
            .into_iter()
            .map(RustVerificationCommand::from)
            .collect::<Vec<_>>();
        let session = self.inner.clone();
        let report = get_runtime()
            .spawn(async move { session.verify_commands(&subject, &rust_commands).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        serde_json::to_value(report)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return project-aware verification command presets for this workspace.
    #[napi]
    pub fn verification_presets(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.verification_presets())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get dead letters from the optional queue's DLQ.
    #[napi]
    pub async fn dead_letters(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let letters = get_runtime()
            .spawn(async move { session.dead_letters().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(&letters)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get a detailed metrics snapshot from the queue.
    ///
    /// Returns `null` if metrics are not enabled (queue not configured or
    /// `enable_metrics` was not set in `SessionQueueConfig`).
    ///
    /// @returns Object with `counters`, `gauges`, and `histograms` maps, or null
    #[napi]
    pub async fn queue_metrics(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let snapshot = get_runtime()
            .spawn(async move { session.queue_metrics().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(metrics_snapshot_to_json(snapshot))
    }
}
