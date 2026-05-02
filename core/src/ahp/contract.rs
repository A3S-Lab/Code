use crate::agent::AgentEvent;
use chrono::Utc;

/// Convert a runtime event into zero or more harness-facing AHP contract events.
///
/// The SDK event stream remains product/UI friendly. This mapper defines the
/// durable AHP-facing contract used by supervisors, replay, and audit.
pub fn agent_event_to_ahp_events(
    event: &AgentEvent,
    run_id: &str,
    default_session_id: &str,
    agent_id: &str,
    depth: u32,
) -> Vec<a3s_ahp::AhpEvent> {
    match event {
        AgentEvent::Start { prompt } => vec![run_lifecycle_event(
            run_id,
            default_session_id,
            agent_id,
            depth,
            a3s_ahp::RunStatus::Executing,
            Some(prompt.clone()),
            None,
            None,
        )],
        AgentEvent::PlanningStart { prompt } => vec![run_lifecycle_event(
            run_id,
            default_session_id,
            agent_id,
            depth,
            a3s_ahp::RunStatus::Planning,
            Some(prompt.clone()),
            None,
            None,
        )],
        AgentEvent::TaskUpdated { session_id, tasks } => {
            vec![task_list_event(run_id, session_id, agent_id, depth, tasks)]
        }
        AgentEvent::End {
            text,
            verification_summary,
            ..
        } => vec![
            run_lifecycle_event(
                run_id,
                default_session_id,
                agent_id,
                depth,
                a3s_ahp::RunStatus::Completed,
                None,
                Some(text.clone()),
                None,
            ),
            verification_event(
                run_id,
                default_session_id,
                agent_id,
                depth,
                verification_summary,
            ),
        ],
        AgentEvent::Error { message } => vec![run_lifecycle_event(
            run_id,
            default_session_id,
            agent_id,
            depth,
            a3s_ahp::RunStatus::Failed,
            None,
            None,
            Some(message.clone()),
        )],
        _ => Vec::new(),
    }
}

pub fn tasks_to_ahp_items(tasks: &[crate::planning::Task]) -> Vec<a3s_ahp::TaskItem> {
    tasks
        .iter()
        .map(|task| a3s_ahp::TaskItem {
            id: task.id.clone(),
            title: task.content.clone(),
            status: task_status_to_ahp(task.status),
            depends_on: task.dependencies.clone(),
            evidence: Vec::new(),
            artifacts: Vec::new(),
            error: (task.status == crate::planning::TaskStatus::Failed)
                .then(|| "task failed".to_string()),
            updated_at: Some(now()),
            metadata: task_metadata(task),
        })
        .collect()
}

pub fn cancelled_run_event(
    run_id: &str,
    session_id: &str,
    agent_id: &str,
    depth: u32,
    reason: Option<&str>,
) -> a3s_ahp::AhpEvent {
    run_lifecycle_event(
        run_id,
        session_id,
        agent_id,
        depth,
        a3s_ahp::RunStatus::Cancelled,
        None,
        None,
        reason.map(str::to_string),
    )
}

fn run_lifecycle_event(
    run_id: &str,
    session_id: &str,
    agent_id: &str,
    depth: u32,
    status: a3s_ahp::RunStatus,
    prompt: Option<String>,
    result_summary: Option<String>,
    error: Option<String>,
) -> a3s_ahp::AhpEvent {
    let updated_at = now();
    let payload = a3s_ahp::RunLifecycleEvent {
        run_id: run_id.to_string(),
        session_id: session_id.to_string(),
        status,
        prompt,
        result_summary,
        error,
        started_at: None,
        updated_at,
        metadata: None,
    };
    ahp_event(
        a3s_ahp::EventType::RunLifecycle,
        session_id,
        agent_id,
        depth,
        serde_json::to_value(payload).expect("run lifecycle payload serializes"),
    )
}

fn task_list_event(
    run_id: &str,
    session_id: &str,
    agent_id: &str,
    depth: u32,
    tasks: &[crate::planning::Task],
) -> a3s_ahp::AhpEvent {
    let payload = a3s_ahp::TaskListEvent {
        run_id: run_id.to_string(),
        session_id: session_id.to_string(),
        tasks: tasks_to_ahp_items(tasks),
        updated_at: now(),
        metadata: None,
    };
    ahp_event(
        a3s_ahp::EventType::TaskList,
        session_id,
        agent_id,
        depth,
        serde_json::to_value(payload).expect("task list payload serializes"),
    )
}

fn verification_event(
    run_id: &str,
    session_id: &str,
    agent_id: &str,
    depth: u32,
    summary: &crate::verification::VerificationSummary,
) -> a3s_ahp::AhpEvent {
    let mut residual_risks = Vec::new();
    if !summary.pending_subjects.is_empty() {
        residual_risks.push(format!(
            "pending verification subjects: {}",
            summary.pending_subjects.join(", ")
        ));
    }
    if !summary.failed_subjects.is_empty() {
        residual_risks.push(format!(
            "failed verification subjects: {}",
            summary.failed_subjects.join(", ")
        ));
    }

    let metadata = std::collections::HashMap::from([
        (
            "report_count".to_string(),
            serde_json::json!(summary.report_count),
        ),
        (
            "required_check_count".to_string(),
            serde_json::json!(summary.required_check_count),
        ),
        (
            "pending_required_check_count".to_string(),
            serde_json::json!(summary.pending_required_check_count),
        ),
        (
            "failed_check_count".to_string(),
            serde_json::json!(summary.failed_check_count),
        ),
    ]);

    let payload = a3s_ahp::VerificationEvent {
        run_id: run_id.to_string(),
        session_id: session_id.to_string(),
        status: verification_status_to_ahp(summary.status),
        checks: Vec::new(),
        residual_risks,
        updated_at: now(),
        metadata: Some(metadata),
    };
    ahp_event(
        a3s_ahp::EventType::Verification,
        session_id,
        agent_id,
        depth,
        serde_json::to_value(payload).expect("verification payload serializes"),
    )
}

fn ahp_event(
    event_type: a3s_ahp::EventType,
    session_id: &str,
    agent_id: &str,
    depth: u32,
    payload: serde_json::Value,
) -> a3s_ahp::AhpEvent {
    a3s_ahp::AhpEvent {
        event_type,
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        timestamp: now(),
        depth,
        payload,
        context: None,
        metadata: None,
    }
}

fn task_status_to_ahp(status: crate::planning::TaskStatus) -> a3s_ahp::TaskStatus {
    match status {
        crate::planning::TaskStatus::Pending => a3s_ahp::TaskStatus::Pending,
        crate::planning::TaskStatus::InProgress => a3s_ahp::TaskStatus::InProgress,
        crate::planning::TaskStatus::Completed => a3s_ahp::TaskStatus::Completed,
        crate::planning::TaskStatus::Failed => a3s_ahp::TaskStatus::Failed,
        crate::planning::TaskStatus::Skipped => a3s_ahp::TaskStatus::Skipped,
        crate::planning::TaskStatus::Cancelled => a3s_ahp::TaskStatus::Cancelled,
    }
}

fn verification_status_to_ahp(
    status: crate::verification::VerificationStatus,
) -> a3s_ahp::VerificationStatus {
    match status {
        crate::verification::VerificationStatus::Passed => a3s_ahp::VerificationStatus::Passed,
        crate::verification::VerificationStatus::Failed => a3s_ahp::VerificationStatus::Failed,
        crate::verification::VerificationStatus::NeedsReview => {
            a3s_ahp::VerificationStatus::NeedsReview
        }
        crate::verification::VerificationStatus::Skipped => a3s_ahp::VerificationStatus::Skipped,
    }
}

fn task_metadata(
    task: &crate::planning::Task,
) -> Option<std::collections::HashMap<String, serde_json::Value>> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("priority".to_string(), serde_json::json!(task.priority));
    if let Some(tool) = &task.tool {
        metadata.insert("tool".to_string(), serde_json::json!(tool));
    }
    if let Some(success_criteria) = &task.success_criteria {
        metadata.insert(
            "success_criteria".to_string(),
            serde_json::json!(success_criteria),
        );
    }
    (!metadata.is_empty()).then_some(metadata)
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::{Task, TaskStatus};
    use crate::verification::{VerificationCheck, VerificationReport};

    #[test]
    fn maps_task_updated_to_ahp_task_list_contract() {
        let tasks = vec![
            Task::new("s1", "inspect repo").with_status(TaskStatus::Completed),
            Task::new("s2", "run tests")
                .with_status(TaskStatus::InProgress)
                .with_dependencies(vec!["s1".to_string()]),
        ];
        let event = AgentEvent::TaskUpdated {
            session_id: "session-1".to_string(),
            tasks,
        };

        let mapped = agent_event_to_ahp_events(&event, "run-1", "fallback", "agent-1", 0);

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].event_type, a3s_ahp::EventType::TaskList);
        let payload: a3s_ahp::TaskListEvent =
            serde_json::from_value(mapped[0].payload.clone()).unwrap();
        assert_eq!(payload.run_id, "run-1");
        assert_eq!(payload.session_id, "session-1");
        assert_eq!(payload.tasks[0].status, a3s_ahp::TaskStatus::Completed);
        assert_eq!(payload.tasks[1].depends_on, vec!["s1".to_string()]);
    }

    #[test]
    fn maps_end_to_lifecycle_and_verification_contracts() {
        let report = VerificationReport::new(
            "program:test",
            vec![VerificationCheck::required("pytest", "test", "Run tests")],
        );
        let summary = crate::verification::VerificationSummary::from_reports(&[report]);
        let event = AgentEvent::End {
            text: "done".to_string(),
            usage: Default::default(),
            verification_summary: Box::new(summary),
            meta: None,
        };

        let mapped = agent_event_to_ahp_events(&event, "run-1", "session-1", "agent-1", 0);

        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].event_type, a3s_ahp::EventType::RunLifecycle);
        assert_eq!(mapped[1].event_type, a3s_ahp::EventType::Verification);
        let lifecycle: a3s_ahp::RunLifecycleEvent =
            serde_json::from_value(mapped[0].payload.clone()).unwrap();
        assert_eq!(lifecycle.status, a3s_ahp::RunStatus::Completed);
        let verification: a3s_ahp::VerificationEvent =
            serde_json::from_value(mapped[1].payload.clone()).unwrap();
        assert_eq!(
            verification.status,
            a3s_ahp::VerificationStatus::NeedsReview
        );
    }
}
