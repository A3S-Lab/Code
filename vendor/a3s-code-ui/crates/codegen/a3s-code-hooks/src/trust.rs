use std::path::{Path, PathBuf};

// Project-hook trust is no longer stored here: the shell's folder-trust store
// (`~/.a3s/trusted_folders.toml`) is the single authority for whether a repo's project hooks run (the same gate as repo-local MCP/LSP). The helpers below exist only to migrate prior grants out of the legacy file.

/// Path to the legacy project-hook trust file (`<user_grok_home>/trusted-hook-projects`), or `None` when no user a3s home resolves.
/// It is retained only for the one-time migration into folder-trust.
pub fn legacy_trust_file_path() -> Option<PathBuf> {
    Some(a3s_code_config::user_grok_home()?.join(a3s_code_config::TRUSTED_HOOK_PROJECTS_FILENAME))
}

/// The legacy format is one canonical absolute path per line; blank and `#`-comment lines are skipped.
/// Any other read error is returned as `Err` so the caller does not mistake an unreadable file for an empty one and consume it.
/// The one-time migration that seeds folder-trust from prior grants consumes this list.
pub fn list_trusted_projects_with_file(trust_file: &Path) -> std::io::Result<Vec<PathBuf>> {
    let content = match std::fs::read_to_string(trust_file) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect())
}

// ── Hook enable/disable ─────────────────────────────────────────────────

/// Why a hook is skipped at dispatch and shown disabled in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSkipReason {
    /// Its `enabled` flag is off or its name is in `$GROK_HOME/disabled-hooks`.
    UserDisabled,
    /// `allow_managed_hooks_only` is pinned and the hook is not managed policy.
    ManagedOnly,
}

/// One-shot snapshot of the per-spec skip inputs: the disabled-hooks file and the `allow_managed_hooks_only` pin.
/// [`Self::skip_reason`] is the one rule the dispatcher, the stop-gate guard, and the modal apply, so they cannot disagree about what runs.
#[derive(Debug, Default)]
pub struct DisabledHooks {
    names: std::collections::HashSet<String>,
    managed_only: bool,
}

impl DisabledHooks {
    pub fn new<I: IntoIterator<Item = String>>(names: I, managed_only: bool) -> Self {
        Self {
            names: names.into_iter().collect(),
            managed_only,
        }
    }

    /// Read the disabled-hooks file; `managed_only` is the resolved `allow_managed_hooks_only` pin, which the caller reads from managed settings.
    pub fn load(managed_only: bool) -> Self {
        let names = disabled_hooks_file_path()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .map(|content| {
                content
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Self::new(names, managed_only)
    }

    pub fn contains(&self, hook_name: &str) -> bool {
        self.names.contains(hook_name)
    }

    pub fn managed_only(&self) -> bool {
        self.managed_only
    }

    /// Managed-policy hooks are never skipped; the lockdown outranks a user disable as the reported reason.
    pub fn skip_reason(&self, spec: &crate::config::HookSpec) -> Option<HookSkipReason> {
        if spec.is_managed_policy() {
            return None;
        }
        if self.managed_only {
            return Some(HookSkipReason::ManagedOnly);
        }
        (!spec.enabled || self.names.contains(&spec.name)).then_some(HookSkipReason::UserDisabled)
    }

    /// Whether dispatch skips `spec`; also what the modal shows as disabled.
    pub fn blocks(&self, spec: &crate::config::HookSpec) -> bool {
        self.skip_reason(spec).is_some()
    }
}

fn is_hook_disabled_with_file(hook_name: &str, file: &Path) -> bool {
    let content = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return false,
    };
    content
        .lines()
        .any(|l| !l.trim().is_empty() && !l.trim().starts_with('#') && l.trim() == hook_name)
}

/// Disable a hook by name (append to `$GROK_HOME/disabled-hooks`).
pub fn disable_hook(hook_name: &str) -> Result<(), String> {
    let file = disabled_hooks_file_path()
        .ok_or_else(|| "no user grok home (set $GROK_HOME or $HOME)".to_string())?;
    disable_hook_with_file(hook_name, &file)
}

fn disable_hook_with_file(hook_name: &str, file: &Path) -> Result<(), String> {
    if is_hook_disabled_with_file(hook_name, file) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|e| format!("failed to open disabled-hooks file: {e}"))?;
    writeln!(f, "{hook_name}").map_err(|e| format!("failed to write disabled-hooks file: {e}"))?;
    Ok(())
}

/// Enable a hook by name (remove from `$GROK_HOME/disabled-hooks`).
pub fn enable_hook(hook_name: &str) -> Result<bool, String> {
    match disabled_hooks_file_path() {
        Some(file) => enable_hook_with_file(hook_name, &file),
        None => Ok(false),
    }
}

fn enable_hook_with_file(hook_name: &str, file: &Path) -> Result<bool, String> {
    let content = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("failed to read disabled-hooks file: {e}")),
    };
    let mut found = false;
    let new_lines: Vec<&str> = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') && trimmed == hook_name {
                found = true;
                false
            } else {
                true
            }
        })
        .collect();
    if !found {
        return Ok(false);
    }
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    let mut f = std::fs::File::create(file)
        .map_err(|e| format!("failed to open disabled-hooks file: {e}"))?;
    for line in new_lines {
        writeln!(f, "{line}").map_err(|e| format!("failed to write disabled-hooks file: {e}"))?;
    }
    Ok(true)
}

/// Returns the path to `$GROK_HOME/disabled-hooks`, or `None` when no user a3s home resolves.
fn disabled_hooks_file_path() -> Option<PathBuf> {
    Some(a3s_code_config::user_grok_home()?.join("disabled-hooks"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test creates its own legacy file in its own temp dir, so no state is shared.
    fn trust_file_in(dir: &Path) -> PathBuf {
        let grok_dir = dir.join(".grok");
        std::fs::create_dir_all(&grok_dir).unwrap();
        grok_dir.join("trusted-hook-projects")
    }

    #[test]
    fn list_trusted_projects_parses_paths_skipping_comments_and_blanks() {
        let home = tempfile::tempdir().unwrap();
        let trust_file = trust_file_in(home.path());
        std::fs::write(
            &trust_file,
            "# comment\n\n/abs/project/one\n  /abs/project/two  \n# trailing\n",
        )
        .unwrap();

        let projects = list_trusted_projects_with_file(&trust_file).unwrap();
        assert_eq!(
            projects,
            vec![
                PathBuf::from("/abs/project/one"),
                PathBuf::from("/abs/project/two"),
            ]
        );
    }

    #[test]
    fn list_trusted_projects_missing_file_is_empty() {
        // The migration treats a missing file as "nothing to migrate", not as an unreadable file
        let projects =
            list_trusted_projects_with_file(Path::new("/nonexistent/trusted-hook-projects"))
                .expect("missing file resolves to Ok(empty)");
        assert!(projects.is_empty());
    }
}
