//! Bounded project instruction discovery for workspace sessions.
//!
//! A session gets one immutable instruction chain at construction time. The
//! chain starts at the nearest Git root and ends at the selected workspace;
//! the later, more local documents therefore have the final word.

use crate::config::CodeConfig;
use crate::context::{ContextItem, ContextType};
use std::io::Read;
use std::path::{Path, PathBuf};

const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 32 * 1024;
const MAX_PROJECT_DOC_MAX_BYTES: usize = 1024 * 1024;
const MAX_PROJECT_INSTRUCTION_DEPTH: usize = 256;
const PROJECT_DOC_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";
const PROJECT_DOC_SOURCES_METADATA: &str = "a3s.context.project_instruction_sources";

pub(super) fn load_context_item(code_config: &CodeConfig, workspace: &Path) -> Option<ContextItem> {
    let loaded = load_project_instructions(code_config, workspace)?;
    let source = if loaded.sources.len() == 1 {
        format!("file://{}", loaded.sources[0].display())
    } else {
        "a3s://workspace-instructions".to_string()
    };
    let source_uris = loaded
        .sources
        .iter()
        .map(|path| format!("file://{}", path.display()))
        .collect::<Vec<_>>();
    let content = format!(
        "# Project Instructions (AGENTS.md chain)\n\n{}",
        loaded.content
    );
    let token_count = content.split_whitespace().count().max(1);

    tracing::info!(
        files = loaded.sources.len(),
        bytes = loaded.loaded_bytes,
        workspace = %workspace.display(),
        "Auto-loaded hierarchical project instructions"
    );
    Some(
        ContextItem::new("agents_md", ContextType::Resource, content)
            .with_source(source)
            .with_metadata(PROJECT_DOC_SOURCES_METADATA, serde_json::json!(source_uris))
            .with_provenance("workspace_instructions")
            .with_priority(1.0)
            .with_trust(0.95)
            .with_freshness(1.0)
            .with_relevance(1.0)
            .with_token_count(token_count)
            .with_required(),
    )
}

#[derive(Debug)]
struct LoadedProjectInstructions {
    content: String,
    sources: Vec<PathBuf>,
    loaded_bytes: usize,
}

fn load_project_instructions(
    code_config: &CodeConfig,
    workspace: &Path,
) -> Option<LoadedProjectInstructions> {
    let max_bytes = code_config
        .project_doc_max_bytes
        .unwrap_or(DEFAULT_PROJECT_DOC_MAX_BYTES)
        .min(MAX_PROJECT_DOC_MAX_BYTES);
    if max_bytes == 0 {
        return None;
    }
    if code_config
        .project_doc_max_bytes
        .is_some_and(|value| value > max_bytes)
    {
        tracing::warn!(
            configured_bytes = code_config.project_doc_max_bytes.unwrap_or(max_bytes),
            effective_bytes = max_bytes,
            "project_doc_max_bytes exceeded the harness safety ceiling and was clamped"
        );
    }

    let search_dirs = project_instruction_directories(workspace);
    let project_root = search_dirs.first().cloned()?;
    let candidate_names = project_instruction_candidate_names(code_config);
    let mut remaining = max_bytes;
    let mut contents = Vec::new();
    let mut sources = Vec::new();
    let mut loaded_bytes = 0usize;

    for directory in search_dirs {
        if remaining == 0 {
            break;
        }
        let Some(path) =
            select_project_instruction_file(&directory, &project_root, &candidate_names)
        else {
            continue;
        };
        match read_project_instruction_file(&path, remaining) {
            Ok(Some((content, bytes_read, truncated))) => {
                if truncated {
                    tracing::warn!(
                        path = %path.display(),
                        remaining_bytes = remaining,
                        "Project instruction file exceeded the remaining budget and was truncated"
                    );
                }
                remaining = remaining.saturating_sub(bytes_read);
                loaded_bytes = loaded_bytes.saturating_add(bytes_read);
                contents.push(content);
                sources.push(path);
            }
            Ok(None) => {
                tracing::debug!(path = %path.display(), "Project instruction file is empty - skipping");
            }
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "Failed to read project instruction file - skipping"
                );
            }
        }
    }

    (!contents.is_empty()).then(|| LoadedProjectInstructions {
        content: contents.join(PROJECT_DOC_SEPARATOR),
        sources,
        loaded_bytes,
    })
}

fn project_instruction_directories(workspace: &Path) -> Vec<PathBuf> {
    let mut ancestors = Vec::new();
    let mut found_root = false;
    for directory in workspace.ancestors().take(MAX_PROJECT_INSTRUCTION_DEPTH) {
        ancestors.push(directory.to_path_buf());
        if is_project_root(directory) {
            found_root = true;
            break;
        }
    }
    if !found_root {
        return vec![workspace.to_path_buf()];
    }
    ancestors.reverse();
    ancestors
}

fn is_project_root(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory.join(".git"))
        .map(|metadata| {
            !metadata.file_type().is_symlink() && (metadata.is_file() || metadata.is_dir())
        })
        .unwrap_or(false)
}

fn project_instruction_candidate_names(code_config: &CodeConfig) -> Vec<String> {
    let mut names = vec!["AGENTS.override.md".to_string(), "AGENTS.md".to_string()];
    for configured in &code_config.project_doc_fallback_filenames {
        let candidate = configured.trim();
        if !is_safe_project_instruction_filename(candidate) {
            tracing::warn!(
                filename = configured,
                "Ignoring unsafe project instruction fallback filename"
            );
            continue;
        }
        if !names.iter().any(|existing| existing == candidate) {
            names.push(candidate.to_string());
        }
    }
    names
}

fn is_safe_project_instruction_filename(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate != "."
        && candidate != ".."
        && candidate.len() <= 255
        && !candidate
            .chars()
            .any(|character| matches!(character, '/' | '\\' | ':' | '\0'))
}

fn select_project_instruction_file(
    directory: &Path,
    project_root: &Path,
    candidate_names: &[String],
) -> Option<PathBuf> {
    for name in candidate_names {
        let candidate = directory.join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            tracing::warn!(
                path = %candidate.display(),
                "Ignoring project instruction candidate that is not a regular non-symlink file"
            );
            continue;
        }
        let resolved = super::safe_canonicalize(&candidate);
        let resolved_root = super::safe_canonicalize(project_root);
        if !resolved.starts_with(&resolved_root) {
            tracing::warn!(path = %resolved.display(), "Ignoring project instruction candidate outside the project root");
            continue;
        }
        return Some(resolved);
    }
    None
}

fn read_project_instruction_file(
    path: &Path,
    remaining: usize,
) -> std::io::Result<Option<(String, usize, bool)>> {
    let mut bytes = Vec::with_capacity(remaining.min(64 * 1024).saturating_add(1));
    std::fs::File::open(path)?
        .take(remaining.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > remaining;
    if truncated {
        bytes.truncate(remaining);
    }
    let content = match std::str::from_utf8(&bytes) {
        Ok(content) => content.to_string(),
        Err(error) if truncated && error.error_len().is_none() => {
            bytes.truncate(error.valid_up_to());
            String::from_utf8(bytes).expect("valid UTF-8 prefix after boundary truncation")
        }
        Err(error) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("project instruction file is not valid UTF-8: {error}"),
            ));
        }
    };
    if content.trim().is_empty() {
        return Ok(None);
    }
    let bytes_read = content.len();
    Ok(Some((content, bytes_read, truncated)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".git")).unwrap();
        directory
    }

    #[test]
    fn loads_root_to_workspace_and_prefers_override_per_directory() {
        let directory = repo();
        let nested = directory.path().join("crates").join("code");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(directory.path().join("AGENTS.md"), "root guidance").unwrap();
        std::fs::write(
            directory.path().join("crates/AGENTS.md"),
            "shadowed guidance",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("crates/AGENTS.override.md"),
            "crate override",
        )
        .unwrap();
        std::fs::write(nested.join("AGENTS.md"), "workspace guidance").unwrap();

        let loaded = load_project_instructions(&CodeConfig::default(), &nested).unwrap();
        assert_eq!(loaded.sources.len(), 3);
        assert_eq!(
            loaded.content,
            "root guidance\n\n--- project-doc ---\n\ncrate override\n\n--- project-doc ---\n\nworkspace guidance"
        );
        assert!(!loaded.content.contains("shadowed guidance"));
    }

    #[test]
    fn uses_safe_fallbacks_and_does_not_walk_above_a_missing_project_root() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(directory.path().join("AGENTS.md"), "parent guidance").unwrap();
        std::fs::write(workspace.join("TEAM_GUIDE.md"), "workspace fallback").unwrap();
        let config = CodeConfig {
            project_doc_fallback_filenames: vec![
                "../outside.md".to_string(),
                "TEAM_GUIDE.md".to_string(),
            ],
            ..Default::default()
        };

        let loaded = load_project_instructions(&config, &workspace).unwrap();
        assert_eq!(loaded.content, "workspace fallback");
        assert_eq!(loaded.sources.len(), 1);
    }

    #[test]
    fn enforces_combined_byte_budget_in_root_to_workspace_order() {
        let directory = repo();
        let nested = directory.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(directory.path().join("AGENTS.md"), "root").unwrap();
        std::fs::write(nested.join("AGENTS.md"), "deeper").unwrap();
        let config = CodeConfig {
            project_doc_max_bytes: Some(7),
            ..Default::default()
        };

        let loaded = load_project_instructions(&config, &nested).unwrap();
        assert_eq!(loaded.loaded_bytes, 7);
        assert_eq!(loaded.content, "root\n\n--- project-doc ---\n\ndee");
    }

    #[test]
    fn rejects_invalid_utf8_and_unsafe_symlink_candidates() {
        let directory = repo();
        let invalid = directory.path().join("INVALID.md");
        std::fs::write(&invalid, [0xff, 0xfe]).unwrap();
        let config = CodeConfig {
            project_doc_fallback_filenames: vec!["INVALID.md".to_string()],
            ..Default::default()
        };
        assert!(load_project_instructions(&config, directory.path()).is_none());

        std::fs::write(directory.path().join("AGENTS.md"), "safe guidance").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.md");
        std::fs::write(&outside_file, "outside guidance").unwrap();
        let override_path = directory.path().join("AGENTS.override.md");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside_file, &override_path).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&outside_file, &override_path).is_ok();
        if linked {
            let loaded =
                load_project_instructions(&CodeConfig::default(), directory.path()).unwrap();
            assert_eq!(loaded.content, "safe guidance");
            assert!(!loaded.content.contains("outside guidance"));
        }
    }
}
