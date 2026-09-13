//! Optional read-only producer of a LOOP-1 verification report.
//!
//! Off by default. The editor role cannot mark the report passed. A verifier
//! `Failed` or `NeedsReview` remains a non-pass under the completion gate.

use crate::verification::{VerificationCheck, VerificationReport, VerificationStatus};

const MUTATING_TOOLS: &[&str] = &["write", "edit", "patch", "download"];
/// Nested loops do not inherit the verifier flag, so they can write after this
/// turn has already been labeled read-only.
const NESTED_WRITER_TOOLS: &[&str] = &["skill", "task", "batch"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportAuthor {
    Verifier,
    Editor,
    Host,
}

pub const VERIFIER_TURN_INPUT: &str = "Verification role for this turn only. Presented tools exclude write, edit, patch, download, Skill, task, and batch. A sentence does not close the completion gate.";

pub fn should_invoke(enabled: bool, mutated: bool) -> bool {
    enabled && mutated
}

pub fn presentable(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    !MUTATING_TOOLS.contains(&tool.as_str()) && !NESTED_WRITER_TOOLS.contains(&tool.as_str())
}

pub fn tool_allowed(tool: &str, args: &serde_json::Value) -> bool {
    let tool = tool.to_ascii_lowercase();
    if MUTATING_TOOLS.contains(&tool.as_str()) || NESTED_WRITER_TOOLS.contains(&tool.as_str()) {
        return false;
    }
    if tool == "bash" {
        return !looks_mutating(args);
    }
    if tool == "git" {
        return !git_command_mutates(args);
    }
    true
}

/// Nested `program` calls do not re-enter the verifier tool list. A check
/// producer may run; anything else must have declared itself read-only.
pub fn nested_call_allowed(tool: &str, args: &serde_json::Value, declared_read_only: bool) -> bool {
    if !tool_allowed(tool, args) {
        return false;
    }
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "bash" | "git" | "program"
    ) || declared_read_only
}

pub fn accept_report(
    author: ReportAuthor,
    report: VerificationReport,
) -> Option<VerificationReport> {
    match author {
        ReportAuthor::Editor => None,
        ReportAuthor::Verifier | ReportAuthor::Host => Some(report),
    }
}

fn git_command_mutates(args: &serde_json::Value) -> bool {
    let command = args
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match command {
        "checkout" => true,
        "branch" => args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "stash" => {
            args.get("message")
                .and_then(serde_json::Value::as_str)
                .is_some()
                || args
                    .get("include_untracked")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        }
        "worktree" => matches!(
            args.get("subcommand").and_then(serde_json::Value::as_str),
            Some("create" | "remove")
        ),
        _ => false,
    }
}

fn looks_mutating(args: &serde_json::Value) -> bool {
    let command = args
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    command.contains('>')
        || command.contains("rm ")
        || command.contains("mv ")
        || command.contains("tee ")
        || args.get("changed_paths").is_some()
}

pub fn failed_report(digest: &str) -> VerificationReport {
    VerificationReport::new(
        "verifier",
        vec![
            VerificationCheck::required("review", "verifier", "rejected")
                .with_status(VerificationStatus::Failed),
        ],
    )
    .with_effect_digest(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_loop::{decide_completion, CompletionGate, MutationLedger};

    #[test]
    fn presented_tools_exclude_mutations() {
        assert!(!tool_allowed("write", &serde_json::json!({})));
        assert!(!tool_allowed("edit", &serde_json::json!({})));
        assert!(!tool_allowed("patch", &serde_json::json!({})));
        assert!(!tool_allowed(
            "bash",
            &serde_json::json!({"command": "echo hi > file"})
        ));
        assert!(tool_allowed(
            "read",
            &serde_json::json!({"file_path": "a.rs"})
        ));
        assert!(tool_allowed(
            "bash",
            &serde_json::json!({"command": "cargo test"})
        ));
        assert!(!tool_allowed(
            "git",
            &serde_json::json!({"command": "checkout", "ref": "main"})
        ));
        assert!(!tool_allowed(
            "git",
            &serde_json::json!({"command": "stash", "message": "wip"})
        ));
        assert!(!tool_allowed(
            "git",
            &serde_json::json!({"command": "worktree", "subcommand": "create"})
        ));
        assert!(!tool_allowed(
            "git",
            &serde_json::json!({"command": "branch", "name": "feature"})
        ));
        assert!(tool_allowed("git", &serde_json::json!({"command": "diff"})));
        assert!(tool_allowed(
            "git",
            &serde_json::json!({"command": "stash"})
        ));
        assert!(!tool_allowed(
            "Skill",
            &serde_json::json!({"skill_name": "review"})
        ));
        assert!(!tool_allowed(
            "task",
            &serde_json::json!({"prompt": "edit the file"})
        ));
        assert!(!tool_allowed("batch", &serde_json::json!({})));
        assert!(!presentable("Skill"));
        assert!(!presentable("task"));
        assert!(!presentable("batch"));
        assert!(!presentable("write"));
        assert!(!presentable("edit"));
        assert!(!presentable("patch"));
        assert!(!presentable("download"));
        assert!(presentable("read"));
        assert!(presentable("bash"));
        assert!(!should_invoke(false, true));
    }

    #[test]
    fn failed_verifier_report_blocks_loop_1_success() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "write",
            0,
            Some(&serde_json::json!({"file_path": "a.rs", "after": "x"})),
        );
        let report = accept_report(ReportAuthor::Verifier, failed_report(ledger.digest())).unwrap();
        assert!(matches!(
            decide_completion(&ledger, &[report], &[], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn editor_prose_cannot_flip_the_report_to_passed() {
        let report = VerificationReport::new(
            "editor",
            vec![
                VerificationCheck::required("review", "prose", "tests passed")
                    .with_status(VerificationStatus::Passed),
            ],
        );
        assert!(accept_report(ReportAuthor::Editor, report).is_none());
    }

    #[test]
    fn default_session_does_not_invoke_a_verifier() {
        assert!(!should_invoke(false, true));
        assert!(!should_invoke(true, false));
        assert!(should_invoke(true, true));
    }
}
