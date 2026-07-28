//! Security Module
//!
//! Provides a trait-based security interface for A3S Code sessions.
//! External consumers implement `SecurityProvider` to plug in their own
//! security logic (sanitization, taint tracking, injection detection, etc.).

pub mod config;
pub mod default;
mod event_sanitizer;

pub use config::{RedactionStrategy, SecurityConfig, SensitivityLevel};
pub use default::{DefaultSecurityConfig, DefaultSecurityProvider, SensitivePattern};
pub(crate) use event_sanitizer::AgentEventStreamSanitizer;

use crate::hooks::HookEngine;

/// Sanitize every data-bearing field of an agent event while preserving the
/// identifiers and discriminants used to correlate the event stream.
///
/// Runtime boundaries call this immediately before events are observed,
/// persisted, or exposed to an SDK. Implementations of [`SecurityProvider`]
/// should therefore make [`SecurityProvider::sanitize_output`] idempotent.
pub fn sanitize_agent_event(
    provider: &dyn SecurityProvider,
    event: &crate::agent::AgentEvent,
) -> crate::agent::AgentEvent {
    use crate::agent::AgentEvent;

    fn text(provider: &dyn SecurityProvider, value: &str) -> String {
        provider.sanitize_output(value)
    }

    fn optional_text(provider: &dyn SecurityProvider, value: &Option<String>) -> Option<String> {
        value.as_deref().map(|value| text(provider, value))
    }

    fn strings(provider: &dyn SecurityProvider, values: &[String]) -> Vec<String> {
        values.iter().map(|value| text(provider, value)).collect()
    }

    fn json(provider: &dyn SecurityProvider, value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::String(value) => serde_json::Value::String(text(provider, value)),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(|value| json(provider, value)).collect())
            }
            serde_json::Value::Object(values) => serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), json(provider, value)))
                    .collect(),
            ),
            value => value.clone(),
        }
    }

    fn optional_json(
        provider: &dyn SecurityProvider,
        value: &Option<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        value.as_ref().map(|value| json(provider, value))
    }

    fn tool_error_kind(
        provider: &dyn SecurityProvider,
        value: &Option<crate::tools::ToolErrorKind>,
    ) -> Option<crate::tools::ToolErrorKind> {
        use crate::tools::ToolErrorKind;

        value.as_ref().map(|kind| match kind {
            ToolErrorKind::VersionConflict {
                path,
                expected,
                actual,
            } => ToolErrorKind::VersionConflict {
                path: text(provider, path),
                expected: text(provider, expected),
                actual: optional_text(provider, actual),
            },
            ToolErrorKind::RemoteGitConflict { code, message } => {
                ToolErrorKind::RemoteGitConflict {
                    code: text(provider, code),
                    message: text(provider, message),
                }
            }
            ToolErrorKind::NotFound { path } => ToolErrorKind::NotFound {
                path: text(provider, path),
            },
            ToolErrorKind::InvalidArgument { message } => ToolErrorKind::InvalidArgument {
                message: text(provider, message),
            },
            ToolErrorKind::Unsupported { message } => ToolErrorKind::Unsupported {
                message: text(provider, message),
            },
            ToolErrorKind::Timeout { op, duration_ms } => ToolErrorKind::Timeout {
                op: text(provider, op),
                duration_ms: *duration_ms,
            },
            ToolErrorKind::Transport { op } => ToolErrorKind::Transport {
                op: text(provider, op),
            },
            ToolErrorKind::Cancelled { op } => ToolErrorKind::Cancelled {
                op: text(provider, op),
            },
            ToolErrorKind::PartialFailure { failed, total } => ToolErrorKind::PartialFailure {
                failed: *failed,
                total: *total,
            },
            ToolErrorKind::RateLimited { retry_after_ms } => ToolErrorKind::RateLimited {
                retry_after_ms: *retry_after_ms,
            },
        })
    }

    fn task(
        provider: &dyn SecurityProvider,
        task: &crate::planning::Task,
    ) -> crate::planning::Task {
        let mut task = task.clone();
        task.content = text(provider, &task.content);
        task.success_criteria = optional_text(provider, &task.success_criteria);
        task
    }

    fn plan(
        provider: &dyn SecurityProvider,
        plan: &crate::planning::ExecutionPlan,
    ) -> crate::planning::ExecutionPlan {
        let mut plan = plan.clone();
        plan.goal = text(provider, &plan.goal);
        plan.steps = plan.steps.iter().map(|item| task(provider, item)).collect();
        plan
    }

    fn goal(
        provider: &dyn SecurityProvider,
        goal: &crate::planning::AgentGoal,
    ) -> crate::planning::AgentGoal {
        let mut goal = goal.clone();
        goal.description = text(provider, &goal.description);
        goal.success_criteria = strings(provider, &goal.success_criteria);
        goal
    }

    fn verification_summary(
        provider: &dyn SecurityProvider,
        summary: &crate::verification::VerificationSummary,
    ) -> crate::verification::VerificationSummary {
        let mut summary = summary.clone();
        summary.pending_subjects = strings(provider, &summary.pending_subjects);
        summary.failed_subjects = strings(provider, &summary.failed_subjects);
        summary
    }

    fn response_meta(
        provider: &dyn SecurityProvider,
        meta: &Option<crate::llm::LlmResponseMeta>,
    ) -> Option<crate::llm::LlmResponseMeta> {
        meta.as_ref().map(|meta| {
            let mut meta = meta.clone();
            meta.request_url = optional_text(provider, &meta.request_url);
            meta
        })
    }

    match event {
        AgentEvent::Start { prompt } => AgentEvent::Start {
            prompt: text(provider, prompt),
        },
        AgentEvent::AgentModeChanged {
            mode,
            agent,
            description,
        } => AgentEvent::AgentModeChanged {
            mode: mode.clone(),
            agent: agent.clone(),
            description: text(provider, description),
        },
        AgentEvent::TextDelta { text: value } => AgentEvent::TextDelta {
            text: text(provider, value),
        },
        AgentEvent::ReasoningDelta { text: value } => AgentEvent::ReasoningDelta {
            text: text(provider, value),
        },
        AgentEvent::ToolInputDelta { id, delta } => AgentEvent::ToolInputDelta {
            id: id.clone(),
            delta: text(provider, delta),
        },
        AgentEvent::ToolExecutionStart { id, name, args } => AgentEvent::ToolExecutionStart {
            id: id.clone(),
            name: name.clone(),
            args: json(provider, args),
        },
        AgentEvent::ToolEnd {
            id,
            name,
            args,
            output,
            exit_code,
            metadata,
            error_kind,
        } => AgentEvent::ToolEnd {
            id: id.clone(),
            name: name.clone(),
            args: optional_json(provider, args),
            output: text(provider, output),
            exit_code: *exit_code,
            metadata: optional_json(provider, metadata),
            error_kind: tool_error_kind(provider, error_kind),
        },
        AgentEvent::ToolOutputDelta { id, name, delta } => AgentEvent::ToolOutputDelta {
            id: id.clone(),
            name: name.clone(),
            delta: text(provider, delta),
        },
        AgentEvent::End {
            text: value,
            usage,
            verification_summary: summary,
            meta,
        } => AgentEvent::End {
            text: text(provider, value),
            usage: usage.clone(),
            verification_summary: Box::new(verification_summary(provider, summary)),
            meta: response_meta(provider, meta),
        },
        AgentEvent::Error { message } => AgentEvent::Error {
            message: text(provider, message),
        },
        AgentEvent::ConfirmationRequired {
            tool_id,
            tool_name,
            args,
            timeout_ms,
        } => AgentEvent::ConfirmationRequired {
            tool_id: tool_id.clone(),
            tool_name: tool_name.clone(),
            args: json(provider, args),
            timeout_ms: *timeout_ms,
        },
        AgentEvent::ConfirmationReceived {
            tool_id,
            approved,
            reason,
        } => AgentEvent::ConfirmationReceived {
            tool_id: tool_id.clone(),
            approved: *approved,
            reason: optional_text(provider, reason),
        },
        AgentEvent::ConfirmationTimeout {
            tool_id,
            action_taken,
        } => AgentEvent::ConfirmationTimeout {
            tool_id: tool_id.clone(),
            action_taken: action_taken.clone(),
        },
        AgentEvent::ExternalTaskPending {
            task_id,
            session_id,
            lane,
            command_type,
            payload,
            timeout_ms,
        } => AgentEvent::ExternalTaskPending {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            lane: *lane,
            command_type: command_type.clone(),
            payload: json(provider, payload),
            timeout_ms: *timeout_ms,
        },
        AgentEvent::PermissionDenied {
            tool_id,
            tool_name,
            args,
            reason,
        } => AgentEvent::PermissionDenied {
            tool_id: tool_id.clone(),
            tool_name: tool_name.clone(),
            args: json(provider, args),
            reason: text(provider, reason),
        },
        AgentEvent::CommandDeadLettered {
            command_id,
            command_type,
            lane,
            error,
            attempts,
        } => AgentEvent::CommandDeadLettered {
            command_id: command_id.clone(),
            command_type: command_type.clone(),
            lane: lane.clone(),
            error: text(provider, error),
            attempts: *attempts,
        },
        AgentEvent::QueueAlert {
            level,
            alert_type,
            message,
        } => AgentEvent::QueueAlert {
            level: level.clone(),
            alert_type: alert_type.clone(),
            message: text(provider, message),
        },
        AgentEvent::TaskUpdated { session_id, tasks } => AgentEvent::TaskUpdated {
            session_id: session_id.clone(),
            tasks: tasks.iter().map(|item| task(provider, item)).collect(),
        },
        AgentEvent::MemoryStored {
            memory_id,
            memory_type,
            importance,
            tags,
        } => AgentEvent::MemoryStored {
            memory_id: memory_id.clone(),
            memory_type: memory_type.clone(),
            importance: *importance,
            tags: strings(provider, tags),
        },
        AgentEvent::MemoryRecalled {
            memory_id,
            content,
            relevance,
        } => AgentEvent::MemoryRecalled {
            memory_id: memory_id.clone(),
            content: text(provider, content),
            relevance: *relevance,
        },
        AgentEvent::MemoriesSearched {
            query,
            tags,
            result_count,
        } => AgentEvent::MemoriesSearched {
            query: optional_text(provider, query),
            tags: strings(provider, tags),
            result_count: *result_count,
        },
        AgentEvent::SubagentStart {
            task_id,
            session_id,
            parent_session_id,
            agent,
            description,
            started_ms,
        } => AgentEvent::SubagentStart {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            parent_session_id: parent_session_id.clone(),
            agent: agent.clone(),
            description: text(provider, description),
            started_ms: *started_ms,
        },
        AgentEvent::SubagentProgress {
            task_id,
            session_id,
            status,
            metadata,
        } => AgentEvent::SubagentProgress {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            status: text(provider, status),
            metadata: json(provider, metadata),
        },
        AgentEvent::SubagentEnd {
            task_id,
            session_id,
            agent,
            output,
            success,
            finished_ms,
        } => AgentEvent::SubagentEnd {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            agent: agent.clone(),
            output: text(provider, output),
            success: *success,
            finished_ms: *finished_ms,
        },
        AgentEvent::PlanningStart { prompt } => AgentEvent::PlanningStart {
            prompt: text(provider, prompt),
        },
        AgentEvent::PlanningEnd {
            plan: value,
            estimated_steps,
        } => AgentEvent::PlanningEnd {
            plan: plan(provider, value),
            estimated_steps: *estimated_steps,
        },
        AgentEvent::StepStart {
            step_id,
            description,
            step_number,
            total_steps,
        } => AgentEvent::StepStart {
            step_id: step_id.clone(),
            description: text(provider, description),
            step_number: *step_number,
            total_steps: *total_steps,
        },
        AgentEvent::GoalExtracted { goal: value } => AgentEvent::GoalExtracted {
            goal: goal(provider, value),
        },
        AgentEvent::GoalProgress {
            goal,
            progress,
            completed_steps,
            total_steps,
        } => AgentEvent::GoalProgress {
            goal: text(provider, goal),
            progress: *progress,
            completed_steps: *completed_steps,
            total_steps: *total_steps,
        },
        AgentEvent::GoalAchieved {
            goal,
            total_steps,
            duration_ms,
        } => AgentEvent::GoalAchieved {
            goal: text(provider, goal),
            total_steps: *total_steps,
            duration_ms: *duration_ms,
        },
        AgentEvent::PersistenceFailed {
            session_id,
            operation,
            error,
        } => AgentEvent::PersistenceFailed {
            session_id: session_id.clone(),
            operation: operation.clone(),
            error: text(provider, error),
        },
        AgentEvent::BudgetThresholdHit {
            resource,
            kind,
            consumed,
            limit,
            message,
        } => AgentEvent::BudgetThresholdHit {
            resource: resource.clone(),
            kind: kind.clone(),
            consumed: *consumed,
            limit: *limit,
            message: optional_text(provider, message),
        },
        AgentEvent::PassivationRequested {
            reason,
            deadline_ms,
        } => AgentEvent::PassivationRequested {
            reason: text(provider, reason),
            deadline_ms: *deadline_ms,
        },
        _ => event.clone(),
    }
}

/// Trait for pluggable security providers.
///
/// Implement this trait to provide custom security logic for sessions.
/// The default `NoOpSecurityProvider` passes everything through unchanged.
pub trait SecurityProvider: Send + Sync {
    /// Classify and register sensitive data found in input text
    fn taint_input(&self, _text: &str) {}

    /// Sanitize output text by redacting sensitive data.
    /// Returns the sanitized text.
    fn sanitize_output(&self, text: &str) -> String {
        text.to_string()
    }

    /// Securely wipe all session security state
    fn wipe(&self) {}

    /// Register security hooks with the given engine
    fn register_hooks(&self, _hook_engine: &HookEngine) {}

    /// Unregister all hooks from the engine
    fn teardown(&self, _hook_engine: &HookEngine) {}
}

/// No-op security provider (default when security is disabled)
pub struct NoOpSecurityProvider;

impl SecurityProvider for NoOpSecurityProvider {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_provider_passthrough() {
        let provider = NoOpSecurityProvider;
        provider.taint_input("SSN: 123-45-6789");
        let output = provider.sanitize_output("SSN: 123-45-6789");
        assert_eq!(output, "SSN: 123-45-6789");
    }

    #[test]
    fn test_noop_provider_wipe() {
        let provider = NoOpSecurityProvider;
        provider.wipe(); // Should not panic
    }

    #[test]
    fn test_noop_provider_hooks() {
        let engine = HookEngine::new();
        let provider = NoOpSecurityProvider;
        provider.register_hooks(&engine);
        provider.teardown(&engine);
        assert_eq!(engine.hook_count(), 0);
    }

    #[test]
    fn agent_event_sanitization_redacts_streams_arguments_and_outputs() {
        let provider = DefaultSecurityProvider::new();
        let secret = "user@example.com";
        let events = [
            crate::agent::AgentEvent::TextDelta {
                text: secret.to_string(),
            },
            crate::agent::AgentEvent::ReasoningDelta {
                text: secret.to_string(),
            },
            crate::agent::AgentEvent::ToolInputDelta {
                id: Some("tool-1".to_string()),
                delta: format!(r#"{{"token":"{secret}"}}"#),
            },
            crate::agent::AgentEvent::ToolExecutionStart {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({"command": format!("echo {secret}")}),
            },
            crate::agent::AgentEvent::ToolOutputDelta {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                delta: secret.to_string(),
            },
            crate::agent::AgentEvent::ToolEnd {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                args: Some(serde_json::json!({"command": format!("echo {secret}")})),
                output: secret.to_string(),
                exit_code: 0,
                metadata: Some(serde_json::json!({"contact": secret})),
                error_kind: None,
            },
        ];

        for event in &events {
            let sanitized = sanitize_agent_event(&provider, event);
            let json = serde_json::to_string(&sanitized).unwrap();
            assert!(!json.contains(secret), "unsanitized event: {json}");
            assert!(json.contains("REDACTED:EMAIL"));
        }

        let sanitized = sanitize_agent_event(&provider, &events[3]);
        assert!(matches!(
            sanitized,
            crate::agent::AgentEvent::ToolExecutionStart { id, name, .. }
                if id == "tool-1" && name == "bash"
        ));
    }

    #[test]
    fn agent_event_sanitization_redacts_every_tool_error_kind_string() {
        use crate::agent::AgentEvent;
        use crate::tools::ToolErrorKind;

        let provider = DefaultSecurityProvider::new();
        let secret = "user@example.com";
        let redacted = "[REDACTED:EMAIL]";
        let cases = [
            (
                ToolErrorKind::VersionConflict {
                    path: format!("path/{secret}"),
                    expected: format!("expected: {secret}"),
                    actual: Some(format!("actual: {secret}")),
                },
                ToolErrorKind::VersionConflict {
                    path: format!("path/{redacted}"),
                    expected: format!("expected: {redacted}"),
                    actual: Some(format!("actual: {redacted}")),
                },
            ),
            (
                ToolErrorKind::RemoteGitConflict {
                    code: format!("code: {secret}"),
                    message: format!("message: {secret}"),
                },
                ToolErrorKind::RemoteGitConflict {
                    code: format!("code: {redacted}"),
                    message: format!("message: {redacted}"),
                },
            ),
            (
                ToolErrorKind::NotFound {
                    path: format!("path/{secret}"),
                },
                ToolErrorKind::NotFound {
                    path: format!("path/{redacted}"),
                },
            ),
            (
                ToolErrorKind::InvalidArgument {
                    message: format!("message: {secret}"),
                },
                ToolErrorKind::InvalidArgument {
                    message: format!("message: {redacted}"),
                },
            ),
            (
                ToolErrorKind::Unsupported {
                    message: format!("message: {secret}"),
                },
                ToolErrorKind::Unsupported {
                    message: format!("message: {redacted}"),
                },
            ),
            (
                ToolErrorKind::Timeout {
                    op: format!("operation: {secret}"),
                    duration_ms: 42,
                },
                ToolErrorKind::Timeout {
                    op: format!("operation: {redacted}"),
                    duration_ms: 42,
                },
            ),
            (
                ToolErrorKind::Transport {
                    op: format!("operation: {secret}"),
                },
                ToolErrorKind::Transport {
                    op: format!("operation: {redacted}"),
                },
            ),
        ];

        for (error_kind, expected) in cases {
            let event = AgentEvent::ToolEnd {
                id: "tool-1".to_string(),
                name: "test".to_string(),
                args: None,
                output: String::new(),
                exit_code: 1,
                metadata: None,
                error_kind: Some(error_kind),
            };

            let sanitized = sanitize_agent_event(&provider, &event);
            let AgentEvent::ToolEnd {
                id,
                name,
                exit_code,
                error_kind,
                ..
            } = sanitized
            else {
                panic!("sanitization changed the event variant");
            };

            assert_eq!(id, "tool-1");
            assert_eq!(name, "test");
            assert_eq!(exit_code, 1);
            assert_eq!(error_kind, Some(expected));
        }
    }
}
