//! How a core tool call renders as an ACP tool call.

use super::*;

/// Map a3s-code planning tasks onto ACP `PlanEntry` so the pager plan strip updates.
pub(super) fn task_to_plan_entry(task: &a3s_code_core::planning::Task) -> acp::PlanEntry {
    use a3s_code_core::planning::{TaskPriority, TaskStatus};
    let priority = match task.priority {
        TaskPriority::High => acp::PlanEntryPriority::High,
        TaskPriority::Medium => acp::PlanEntryPriority::Medium,
        TaskPriority::Low => acp::PlanEntryPriority::Low,
    };
    // Failed/Skipped/Cancelled collapse to Completed so the strip stays within ACP's status set.
    let status = match task.status {
        TaskStatus::Pending => acp::PlanEntryStatus::Pending,
        TaskStatus::InProgress => acp::PlanEntryStatus::InProgress,
        TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Skipped
        | TaskStatus::Cancelled => acp::PlanEntryStatus::Completed,
    };
    let mut entry = acp::PlanEntry::new(task.content.clone(), priority, status);
    let mut meta = acp::Meta::new();
    meta.insert("id".into(), serde_json::Value::String(task.id.clone()));
    if matches!(
        task.status,
        TaskStatus::Failed | TaskStatus::Skipped | TaskStatus::Cancelled
    ) {
        meta.insert(
            "a3sStatus".into(),
            serde_json::Value::String(task.status.to_string()),
        );
    }
    entry = entry.meta(Some(meta));
    entry
}

/// ACP `ToolKind` the pager uses to pick Execute / Read / Edit / Search / Fetch cards.
pub(super) fn tool_kind_for(name: &str) -> acp::ToolKind {
    match name {
        "bash" | "shell" | "execute" | "git" => acp::ToolKind::Execute,
        "read" => acp::ToolKind::Read,
        "write" | "edit" | "patch" => acp::ToolKind::Edit,
        "grep" | "glob" | "search" | "semantic" | "hybrid" | "bm25" | "code_symbols"
        | "code_navigation" | "code_diagnostics" | "web_search" => acp::ToolKind::Search,
        "web_fetch" | "download" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

pub(super) fn tool_title(name: &str) -> String {
    match name {
        "update_plan" => "Updating plan".to_string(),
        other => other.to_string(),
    }
}

/// Copy a3s argument names onto the fields the pager card renderer reads.
pub(super) fn prepared_raw_input(
    name: &str,
    args: &serde_json::Value,
) -> Option<serde_json::Value> {
    if args.is_null() {
        return None;
    }
    let mut input = args.clone();
    let Some(obj) = input.as_object_mut() else {
        return Some(input);
    };
    match name {
        "ls" => {
            if let Some(path) = obj.get("path").cloned() {
                obj.entry("target_directory".to_string()).or_insert(path);
            }
        }
        "write" => {
            obj.insert("variant".into(), serde_json::json!("Write"));
        }
        "update_plan" => {
            obj.insert("variant".into(), serde_json::json!("TodoWrite"));
        }
        "web_search" => {
            obj.insert("variant".into(), serde_json::json!("WebSearch"));
        }
        "task" => {
            obj.insert("variant".into(), serde_json::json!("Task"));
        }
        "search" | "semantic" | "hybrid" | "bm25" => {
            // A let-chain form needs Rust 2024; keep the guard pair flat.
            #[allow(clippy::collapsible_if, clippy::single_match)]
            match (obj.get("pattern").is_none(), obj.get("query").cloned()) {
                (true, Some(query)) => {
                    obj.insert("pattern".into(), query);
                }
                _ => {}
            }
        }
        _ => {}
    }
    Some(input)
}

pub(super) fn tool_call_content(
    name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
) -> Vec<acp::ToolCallContent> {
    if let Some(diff) = edit_diff(name, args) {
        return vec![acp::ToolCallContent::from(diff)];
    }
    if output.is_empty() {
        return Vec::new();
    }
    let preview: String = output.chars().take(4_000).collect();
    vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
        acp::TextContent::new(preview),
    ))]
}

pub(super) fn edit_diff(name: &str, args: Option<&serde_json::Value>) -> Option<acp::Diff> {
    let args = args?;
    let path = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())?;
    match name {
        "edit" | "patch" => {
            let old_text = args.get("old_string").and_then(|v| v.as_str());
            let new_text = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut diff = acp::Diff::new(path, new_text);
            if let Some(old_text) = old_text {
                diff = diff.old_text(old_text.to_string());
            }
            Some(diff)
        }
        "write" => args
            .get("content")
            .and_then(|v| v.as_str())
            .map(|content| acp::Diff::new(path, content)),
        _ => None,
    }
}

/// Bash cards only show stdout when `raw_output` deserializes as `ToolOutput::Bash`.
/// Output fields are strings: a byte array costs a JSON number per byte.
/// Incremental stdout. `output` stays empty so the pager appends `output_delta`
/// instead of replacing the card with this slice.
pub(super) fn streaming_bash_delta(delta: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "Bash",
        "output": "",
        "output_for_prompt": "",
        "exit_code": 0,
        "command": "",
        "truncated": false,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": "",
        "total_bytes": delta.len(),
        "output_delta": delta,
    })
}

pub(super) fn tool_raw_output(
    name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
    exit_code: i32,
) -> Option<serde_json::Value> {
    if !matches!(name, "bash" | "shell" | "execute") {
        return None;
    }
    let command = args
        .and_then(|args| args.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Some(serde_json::json!({
        "type": "Bash",
        "output": output,
        "output_for_prompt": output,
        "exit_code": exit_code,
        "command": command,
        "truncated": false,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": "",
        "total_bytes": output.len(),
    }))
}
