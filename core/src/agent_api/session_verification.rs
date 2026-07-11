//! Verification runtime for host-controlled checks.
//!
//! Verification is completion evidence for a harness-driven coding session. This
//! module keeps command execution, report accumulation, summary formatting, and
//! autosave behavior together instead of scattering it across the public facade.

use super::{
    agent_loop_runtime::build_agent_loop, session_persistence::SessionPersistenceContext,
    AgentSession,
};
use crate::agent::AgentLoop;
use crate::error::{read_or_recover, write_or_recover, Result};
use crate::tools::{ToolContext, ToolInvocation};
use crate::verification::{
    format_verification_summary, verification_presets_for_workspace, VerificationCommand,
    VerificationPreset, VerificationReport, VerificationSummary,
};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub(super) struct VerificationRuntime {
    workspace: PathBuf,
    agent_loop: AgentLoop,
    tool_context: ToolContext,
    session_id: String,
    session_cancel: tokio_util::sync::CancellationToken,
    reports: Arc<RwLock<Vec<VerificationReport>>>,
    persistence: SessionPersistenceContext,
}

impl VerificationRuntime {
    pub(super) fn from_session(session: &AgentSession) -> Self {
        Self {
            workspace: session.workspace.clone(),
            agent_loop: build_agent_loop(session),
            tool_context: session.tool_context.clone(),
            session_id: session.session_id.clone(),
            session_cancel: session.session_cancel.clone(),
            reports: Arc::clone(&session.verification_reports),
            persistence: SessionPersistenceContext::from_session(session),
        }
    }

    pub(super) fn reports(&self) -> Vec<VerificationReport> {
        read_or_recover(&self.reports).clone()
    }

    pub(super) fn summary(&self) -> VerificationSummary {
        VerificationSummary::from_reports(&self.reports())
    }

    pub(super) fn summary_text(&self) -> String {
        format_verification_summary(&self.summary())
    }

    pub(super) fn record(&self, reports: impl IntoIterator<Item = VerificationReport>) {
        write_or_recover(&self.reports).extend(reports);
    }

    pub(super) async fn verify_commands(
        &self,
        subject: &str,
        commands: &[VerificationCommand],
    ) -> Result<VerificationReport> {
        let mut checks = Vec::with_capacity(commands.len());

        for command in commands {
            let mut args = serde_json::json!({ "command": command.command });
            if let Some(timeout_ms) = command.timeout_ms {
                args["timeout"] = serde_json::json!(timeout_ms);
            }

            let cancellation = self.session_cancel.child_token();
            let result = self
                .agent_loop
                .invoke_host_tool(
                    ToolInvocation::host_direct(
                        format!("verification-bash-{}", uuid::Uuid::new_v4()),
                        "bash",
                        args,
                    ),
                    &self.session_id,
                    &None,
                    &cancellation,
                    &self.tool_context,
                )
                .await;
            let exit_code = result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("exit_code"))
                .and_then(|value| value.as_i64())
                .and_then(|value| i32::try_from(value).ok())
                .unwrap_or(result.exit_code);
            let execution_error = governed_execution_error(&result);
            let check = command.check_from_execution(
                exit_code,
                result.metadata.as_ref(),
                execution_error.as_deref(),
            );
            checks.push(check);
        }

        let report = VerificationReport::new(subject, checks);
        self.record([report.clone()]);
        self.persistence.auto_save_if_enabled().await;

        Ok(report)
    }

    pub(super) fn presets(&self) -> Vec<VerificationPreset> {
        verification_presets_for_workspace(&self.workspace)
    }
}

/// Distinguish invocation-kernel failures from an ordinary non-zero command.
/// A real bash exit carries execution metadata and remains reported as a command
/// failure; pre-execution hook/budget/cancellation/queue failures do not.
fn governed_execution_error(result: &crate::tools::ToolResult) -> Option<String> {
    (result.exit_code != 0 && result.metadata.is_none()).then(|| result.output.clone())
}
