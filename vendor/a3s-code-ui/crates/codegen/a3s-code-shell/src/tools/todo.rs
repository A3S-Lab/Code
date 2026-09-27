//! Types are canonical in `a3s-code-tools`.
//! This module adds conversions between ACP plan entries and `TodoItem` since `a3s-code-tools` is protocol-agnostic.

pub use a3s_code_tools::implementations::grok_build::todo::TodoId;
pub use a3s_code_tools::implementations::grok_build::todo::TodoItem;
pub use a3s_code_tools::implementations::grok_build::todo::TodoPriority;
pub use a3s_code_tools::implementations::grok_build::todo::TodoState;
pub use a3s_code_tools::implementations::grok_build::todo::TodoStatus;

use agent_client_protocol as acp;

/// Failed, skipped, and cancelled a3s tasks share ACP `Completed` and are distinguished in meta.
fn plan_entry_closed_without_success(entry: &acp::PlanEntry) -> bool {
    let Some(meta) = entry.meta.as_ref() else {
        return false;
    };
    if meta
        .get("cancelled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    matches!(
        meta.get("a3sStatus").and_then(|value| value.as_str()),
        Some("failed" | "skipped" | "cancelled" | "canceled")
    )
}

/// ACP has no `Cancelled` status, so cancelled items are stored as `Completed` with `{"cancelled": true}` in meta.
/// a3s-code also stamps failed, skipped, and cancelled tasks as Completed plus `a3sStatus`.
pub fn todo_item_from_plan_entry(entry: acp::PlanEntry) -> TodoItem {
    let status = match entry.status {
        acp::PlanEntryStatus::Pending => TodoStatus::Pending,
        acp::PlanEntryStatus::InProgress => TodoStatus::InProgress,
        acp::PlanEntryStatus::Completed => {
            if plan_entry_closed_without_success(&entry) {
                TodoStatus::Cancelled
            } else {
                TodoStatus::Completed
            }
        }
        // TODO(acp-0.10): `PlanEntryStatus` is #[non_exhaustive].
        _ => TodoStatus::Pending,
    };
    TodoItem {
        content: entry.content,
        priority: match entry.priority {
            acp::PlanEntryPriority::High => TodoPriority::High,
            acp::PlanEntryPriority::Medium => TodoPriority::Medium,
            acp::PlanEntryPriority::Low => TodoPriority::Low,
            // TODO(acp-0.10): `PlanEntryPriority` is #[non_exhaustive].
            _ => TodoPriority::Medium,
        },
        status,
        meta: entry.meta.map(serde_json::Value::Object),
    }
}

/// Cancelled items become `Completed` with `{"cancelled": true}` in meta.
pub(crate) fn plan_entry_from_todo_item(item: TodoItem) -> acp::PlanEntry {
    let status = match item.status {
        TodoStatus::Pending => acp::PlanEntryStatus::Pending,
        TodoStatus::InProgress => acp::PlanEntryStatus::InProgress,
        TodoStatus::Completed => acp::PlanEntryStatus::Completed,
        TodoStatus::Cancelled => acp::PlanEntryStatus::Completed,
    };
    let mut meta = item.meta;
    if item.status == TodoStatus::Cancelled {
        let mut m = meta.unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = m.as_object_mut() {
            obj.insert("cancelled".into(), true.into());
        }
        meta = Some(m);
    }
    acp::PlanEntry::new(
        item.content,
        match item.priority {
            TodoPriority::High => acp::PlanEntryPriority::High,
            TodoPriority::Medium => acp::PlanEntryPriority::Medium,
            TodoPriority::Low => acp::PlanEntryPriority::Low,
        },
        status,
    )
    .meta(meta.and_then(|v| v.as_object().cloned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(content: &str, meta: Option<acp::Meta>) -> TodoItem {
        let entry = acp::PlanEntry::new(
            content,
            acp::PlanEntryPriority::Medium,
            acp::PlanEntryStatus::Completed,
        );
        let entry = match meta {
            Some(meta) => entry.meta(Some(meta)),
            None => entry,
        };
        todo_item_from_plan_entry(entry)
    }

    #[test]
    fn a3s_status_failed_skipped_and_cancelled_are_not_completed() {
        let mut failed = acp::Meta::new();
        failed.insert("a3sStatus".into(), serde_json::json!("failed"));
        assert_eq!(
            completed("run tests", Some(failed)).status,
            TodoStatus::Cancelled
        );

        let mut skipped = acp::Meta::new();
        skipped.insert("a3sStatus".into(), serde_json::json!("skipped"));
        assert_eq!(
            completed("lint", Some(skipped)).status,
            TodoStatus::Cancelled
        );

        let mut cancelled = acp::Meta::new();
        cancelled.insert("a3sStatus".into(), serde_json::json!("cancelled"));
        assert_eq!(
            completed("dropped", Some(cancelled)).status,
            TodoStatus::Cancelled
        );

        let mut flag = acp::Meta::new();
        flag.insert("cancelled".into(), serde_json::json!(true));
        assert_eq!(
            completed("old wire", Some(flag)).status,
            TodoStatus::Cancelled
        );

        assert_eq!(completed("ship it", None).status, TodoStatus::Completed);
    }
}
