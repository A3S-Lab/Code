//! Path-scoped instruction fragments. The `AGENTS.md` prefix stays stable.
//!
//! A fragment enters the next model input only when a tool or admitted plan
//! targets a matching path. No match is an empty injection, not an error.

use sha2::{Digest, Sha256};

const INJECTION_BYTE_BUDGET: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRule {
    pub glob: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInjection {
    pub prefix: String,
    pub prefix_digest: String,
    pub injected: String,
    pub injected_digest: String,
}

pub fn prefix_digest(prefix: &str) -> String {
    digest(prefix)
}

pub fn inject(prefix: &str, rules: &[PathRule], targeted_paths: &[String]) -> PathInjection {
    let mut injected = String::new();
    for rule in rules {
        if injected.len() >= INJECTION_BYTE_BUDGET {
            break;
        }
        if !targeted_paths
            .iter()
            .any(|path| path_matches(&rule.glob, path))
        {
            continue;
        }
        let remaining = INJECTION_BYTE_BUDGET.saturating_sub(injected.len());
        let text = truncate_utf8(&rule.text, remaining);
        if text.is_empty() {
            continue;
        }
        if !injected.is_empty() {
            injected.push('\n');
        }
        injected.push_str(&text);
    }
    PathInjection {
        prefix: prefix.to_string(),
        prefix_digest: digest(prefix),
        injected,
        injected_digest: digest(""),
    }
    .with_injected_digest()
}

impl PathInjection {
    fn with_injected_digest(mut self) -> Self {
        self.injected_digest = digest(&self.injected);
        self
    }

    /// Turn-scoped model input. Not part of the stable prefix.
    pub fn model_input_fragment(&self) -> Option<String> {
        if self.injected.is_empty() {
            return None;
        }
        Some(format!(
            "[path instructions digest={}] \n{}",
            self.injected_digest, self.injected
        ))
    }
}

fn path_matches(glob: &str, path: &str) -> bool {
    let glob = glob.trim_matches('/');
    let Some(path) = normalize_targeted_path(path) else {
        return false;
    };
    if let Some(prefix) = glob.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = glob.strip_suffix("/*") {
        return path.starts_with(&format!("{prefix}/")) && !path[prefix.len() + 1..].contains('/');
    }
    path == glob || path.starts_with(&format!("{glob}/"))
}

/// Collapse `.` and `..` without reading the filesystem. An absolute path or
/// a path that leaves the workspace prefix is not a rule target.
fn normalize_targeted_path(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    if path.starts_with('/') || windows_drive_prefix(&path) {
        return None;
    }
    let mut parts = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn windows_drive_prefix(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(drive), Some(':'), _) if drive.is_ascii_alphabetic()
    )
}

fn digest(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("sha256:{}", hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn truncate_utf8(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut boundary = max;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text[..boundary].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<PathRule> {
        vec![PathRule {
            glob: "crates/code/**".into(),
            text: "Prefer boring harness code.".into(),
        }]
    }

    #[test]
    fn docs_turn_omits_a_code_rule_and_keeps_the_prefix_digest() {
        let prefix = "# Instructions\nproject AGENTS.md";
        let before = prefix_digest(prefix);
        let injection = inject(prefix, &rules(), &["docs/readme.md".into()]);
        assert!(injection.injected.is_empty());
        assert!(injection.model_input_fragment().is_none());
        assert_eq!(injection.prefix_digest, before);
    }

    #[test]
    fn matching_path_injects_before_the_next_model_input() {
        let prefix = "# Instructions\nproject AGENTS.md";
        let before = prefix_digest(prefix);
        let injection = inject(prefix, &rules(), &["crates/code/core/src/lib.rs".into()]);
        let fragment = injection.model_input_fragment().unwrap();
        assert!(fragment.contains("Prefer boring harness code."));
        assert!(fragment.contains(&injection.injected_digest));
        assert_eq!(injection.prefix_digest, before);
        assert_eq!(prefix_digest(&injection.prefix), before);
    }

    #[test]
    fn dotted_spelling_follows_the_normalized_target() {
        let prefix = "# Instructions\nproject AGENTS.md";
        let before = prefix_digest(prefix);
        let rules = vec![
            PathRule {
                glob: "docs/**".into(),
                text: "Docs tone.".into(),
            },
            PathRule {
                glob: "crates/code/**".into(),
                text: "Prefer boring harness code.".into(),
            },
        ];
        let injection = inject(
            prefix,
            &rules,
            &["docs/../crates/code/core/src/lib.rs".into()],
        );
        let fragment = injection.model_input_fragment().unwrap();
        assert!(fragment.contains("Prefer boring harness code."));
        assert!(!fragment.contains("Docs tone."));
        assert_eq!(injection.prefix_digest, before);

        let dotted = inject(prefix, &rules, &["./crates/code/core/src/lib.rs".into()]);
        assert!(dotted
            .model_input_fragment()
            .unwrap()
            .contains("Prefer boring harness code."));
        assert!(!dotted
            .model_input_fragment()
            .unwrap()
            .contains("Docs tone."));

        let escaped = inject(prefix, &rules, &["../outside.md".into()]);
        assert!(escaped.injected.is_empty());
        assert_eq!(escaped.prefix_digest, before);
    }
}
