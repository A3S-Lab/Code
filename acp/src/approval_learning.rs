//! Persistent approval learning for interactive sessions.
//!
//! When the pager approves a `bash` tool call, the command's leading
//! executable token is learned as an approved prefix and persisted in an
//! ACL document at `<workspace>/.a3s/approvals.acl`. Later sessions skip
//! the confirmation round-trip for commands with the same prefix.
//!
//! Safety contract: only the first whitespace token is learned, validated
//! as a plain executable name (no path, no metacharacters, no shell
//! syntax); a never-learn floor excludes destructive or
//! interpreter/interpreter-like executables; denials are never recorded;
//! the list is capped and oldest-first evicted. The document is A3S ACL
//! (parsed with `a3s-acl`) — no second configuration format.

use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_LEARNED_PREFIXES: usize = 64;
const MAX_PREFIX_LEN: usize = 64;
/// Executables that must always keep their confirmation, whatever the user
/// approved once: destructive tools, privilege escalation, interpreters
/// (they make the "prefix" meaningless), and network fetchers (their
/// arguments are the risk, not the token).
const NEVER_LEARN: &[&str] = &[
    "rm",
    "rmdir",
    "sudo",
    "doas",
    "su",
    "dd",
    "mkfs",
    "fdisk",
    "reboot",
    "shutdown",
    "halt",
    "kill",
    "killall",
    "pkill",
    "chmod",
    "chown",
    "chflags",
    "mount",
    "umount",
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "env",
    "exec",
    "eval",
    "source",
    "xargs",
    "tee",
    "curl",
    "wget",
    "nc",
    "ncat",
    "ssh",
    "scp",
    "rsync",
    "git-push",
    "git-push-force",
];

pub fn approvals_path(workspace: &Path) -> PathBuf {
    workspace.join(".a3s").join("approvals.acl")
}

/// The learnable prefix of one command: its first whitespace token, when it
/// is a plain executable name outside the never-learn floor.
fn learnable_prefix(command: &str) -> Option<String> {
    let token = command.split_whitespace().next()?;
    if token.is_empty() || token.len() > MAX_PREFIX_LEN {
        return None;
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }
    if token.starts_with('.') {
        return None;
    }
    if NEVER_LEARN.contains(&token.to_ascii_lowercase().as_str()) {
        return None;
    }
    Some(token.to_owned())
}

fn render_document(prefixes: &[String]) -> String {
    let body = prefixes
        .iter()
        .map(|prefix| format!("\"{prefix}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("learned_approvals = [{body}]\n")
}

fn parse_prefixes(text: &str) -> Option<Vec<String>> {
    let document = a3s_acl::parse_acl(text).ok()?;
    let block = document.blocks.first()?;
    let value = block.attributes.get("learned_approvals")?;
    let items = match value {
        a3s_acl::ast::Value::List(items) => items,
        _ => return None,
    };
    Some(
        items
            .iter()
            .filter_map(|item| match item {
                a3s_acl::ast::Value::String(prefix) => Some(prefix.clone()),
                _ => None,
            })
            .collect(),
    )
}

/// Load the learned prefixes; a missing or malformed document learns nothing.
pub fn load_approved_prefixes(workspace: &Path) -> Vec<String> {
    std::fs::read_to_string(approvals_path(workspace))
        .ok()
        .and_then(|text| parse_prefixes(&text))
        .unwrap_or_default()
}

/// Whether a tool call was already learned as approved (bash prefixes only).
pub fn is_previously_approved(workspace: &Path, tool_name: &str, args: &serde_json::Value) -> bool {
    if tool_name != "bash" {
        return false;
    }
    let Some(command) = args.get("command").and_then(|value| value.as_str()) else {
        return false;
    };
    let Some(prefix) = learnable_prefix(command) else {
        return false;
    };
    load_approved_prefixes(workspace).contains(&prefix)
}

/// Record one approved bash command. Returns the learned prefix when the
/// decision was persisted, `None` when the command is not learnable, is
/// already known, or the document cannot be written.
pub fn record_approved_bash(workspace: &Path, command: &str) -> Option<String> {
    let prefix = learnable_prefix(command)?;
    let path = approvals_path(workspace);
    let mut prefixes = load_approved_prefixes(workspace);
    if prefixes.contains(&prefix) {
        return None;
    }
    prefixes.push(prefix.clone());
    let overflow = prefixes.len().saturating_sub(MAX_LEARNED_PREFIXES);
    if overflow > 0 {
        prefixes.drain(..overflow);
    }
    let document = render_document(&prefixes);
    // The write must be a document a3s-acl can read back; validate before
    // touching disk so a rendering bug can never corrupt learned state.
    let reparsed = parse_prefixes(&document)?;
    if reparsed != prefixes {
        return None;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let temporary = path.with_extension("acl.tmp");
    {
        let mut file = std::fs::File::create(&temporary).ok()?;
        file.write_all(document.as_bytes()).ok()?;
    }
    std::fs::rename(&temporary, &path).ok()?;
    Some(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &str) -> serde_json::Value {
        serde_json::json!({ "command": command })
    }

    #[test]
    fn learns_plain_prefixes_and_round_trips_through_acl() {
        let workspace = tempfile::tempdir().unwrap();
        let learned = record_approved_bash(workspace.path(), "git status --short").unwrap();
        assert_eq!(learned, "git");
        let learned = record_approved_bash(workspace.path(), "cargo test -p a3s-code-acp").unwrap();
        assert_eq!(learned, "cargo");
        let loaded = load_approved_prefixes(workspace.path());
        assert_eq!(loaded, vec!["git".to_owned(), "cargo".to_owned()]);
        // The document on disk is ACL and parses.
        let text = std::fs::read_to_string(approvals_path(workspace.path())).unwrap();
        assert!(a3s_acl::parse_acl(&text).is_ok());
        assert!(is_previously_approved(
            workspace.path(),
            "bash",
            &args("git diff")
        ));
    }

    #[test]
    fn never_learns_floor_paths_or_metacharacters() {
        let workspace = tempfile::tempdir().unwrap();
        for command in [
            "rm -rf /",
            "sudo reboot",
            "sh -c 'anything'",
            "/usr/bin/git status",
            "./scripts/deploy.sh",
            "git;rm -rf /",
            "$(reboot)",
            "",
        ] {
            assert!(
                record_approved_bash(workspace.path(), command).is_none(),
                "must refuse to learn {command:?}"
            );
        }
        assert!(load_approved_prefixes(workspace.path()).is_empty());
        assert!(!is_previously_approved(
            workspace.path(),
            "bash",
            &args("rm -rf x")
        ));
    }

    #[test]
    fn non_bash_tools_and_denials_never_consult_or_record() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(!is_previously_approved(
            workspace.path(),
            "write",
            &serde_json::json!({"path": "x"})
        ));
        record_approved_bash(workspace.path(), "git push").unwrap();
        assert!(!is_previously_approved(
            workspace.path(),
            "bash",
            &args("write x")
        ));
    }

    #[test]
    fn caps_the_list_oldest_first() {
        let workspace = tempfile::tempdir().unwrap();
        for index in 0..(MAX_LEARNED_PREFIXES + 8) {
            record_approved_bash(workspace.path(), &format!("exe{index:03} run")).unwrap();
        }
        let loaded = load_approved_prefixes(workspace.path());
        assert_eq!(loaded.len(), MAX_LEARNED_PREFIXES);
        assert_eq!(loaded.first().map(String::as_str), Some("exe008"));
        assert_eq!(
            loaded.last().map(String::as_str),
            Some(format!("exe{:03}", MAX_LEARNED_PREFIXES + 7).as_str())
        );
    }

    #[test]
    fn malformed_document_learns_nothing() {
        let workspace = tempfile::tempdir().unwrap();
        let path = approvals_path(workspace.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not = [acl").unwrap();
        assert!(load_approved_prefixes(workspace.path()).is_empty());
        assert!(!is_previously_approved(
            workspace.path(),
            "bash",
            &args("git status")
        ));
    }
}
