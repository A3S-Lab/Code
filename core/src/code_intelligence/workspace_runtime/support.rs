use std::path::Path;

use tokio::sync::watch;

use super::LanguageSlot;
use crate::code_intelligence::{
    language_runtime::LanguageRuntimeError, lsp::client::LspClientError,
    project_layout::ProjectLanguageProfile, CodeDiagnostic, CodeIntelligenceCapabilities,
    CodeIntelligenceError, CodeIntelligenceLanguageStatus, CodeIntelligenceState,
    CodeIntelligenceStatus, LanguageId, SymbolInformation,
};
use crate::workspace::{LocalWorkspaceManifestSnapshot, WorkspaceError, WorkspacePath};

pub(super) fn supported_source_paths(
    snapshot: &LocalWorkspaceManifestSnapshot,
    supports_path: impl Fn(&Path) -> bool,
) -> Vec<WorkspacePath> {
    let mut paths = snapshot
        .files
        .iter()
        .filter(|file| !file.binary && !file.generated)
        .filter(|file| supports_path(Path::new(&file.path)))
        .map(|file| WorkspacePath::from_normalized(file.path.clone()))
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    paths.dedup_by(|left, right| left.as_str() == right.as_str());
    paths
}

/// Select documents round-robin by language so a large profile cannot consume
/// the entire workspace query budget before another profile is represented.
pub(super) fn select_workspace_diagnostic_paths(
    slots: &[LanguageSlot],
    slot_indexes: &[usize],
    source_paths: &[WorkspacePath],
    limit: usize,
) -> (Vec<(usize, WorkspacePath)>, bool) {
    let mut buckets = slot_indexes
        .iter()
        .map(|slot_index| (*slot_index, Vec::new()))
        .collect::<Vec<_>>();
    for path in source_paths {
        let Some((_, bucket)) = buckets.iter_mut().find(|(slot_index, _)| {
            slots[*slot_index]
                .profile
                .supports_path(Path::new(path.as_str()))
        }) else {
            continue;
        };
        bucket.push(path.clone());
    }

    let candidate_count = buckets.iter().map(|(_, paths)| paths.len()).sum::<usize>();
    let mut offsets = vec![0_usize; buckets.len()];
    let mut selected = Vec::with_capacity(limit.min(candidate_count));
    'selection: loop {
        let mut progressed = false;
        for (bucket_index, (slot_index, paths)) in buckets.iter().enumerate() {
            let Some(path) = paths.get(offsets[bucket_index]) else {
                continue;
            };
            if selected.len() == limit {
                break 'selection;
            }
            selected.push((*slot_index, path.clone()));
            offsets[bucket_index] += 1;
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    let truncated = selected.len() < candidate_count;
    (selected, truncated)
}

pub(super) fn append_bounded<T>(target: &mut Vec<T>, incoming: Vec<T>, limit: usize) -> bool {
    let original_len = target.len();
    target.truncate(limit);
    let remaining = limit.saturating_sub(target.len());
    let truncated = original_len > limit || incoming.len() > remaining;
    target.extend(incoming.into_iter().take(remaining));
    truncated
}

pub(super) fn diagnostic_order(
    left: &CodeDiagnostic,
    right: &CodeDiagnostic,
) -> std::cmp::Ordering {
    left.location
        .path
        .as_str()
        .cmp(right.location.path.as_str())
        .then_with(|| {
            left.location
                .range
                .start
                .line
                .cmp(&right.location.range.start.line)
        })
        .then_with(|| {
            left.location
                .range
                .start
                .character
                .cmp(&right.location.range.start.character)
        })
        .then_with(|| {
            left.location
                .range
                .end
                .line
                .cmp(&right.location.range.end.line)
        })
        .then_with(|| {
            left.location
                .range
                .end
                .character
                .cmp(&right.location.range.end.character)
        })
        .then_with(|| left.message.cmp(&right.message))
        .then_with(|| left.severity.cmp(&right.severity))
        .then_with(|| left.code.cmp(&right.code))
        .then_with(|| left.source.cmp(&right.source))
}

pub(super) fn map_workspace_error(
    path: &WorkspacePath,
    error: WorkspaceError,
) -> CodeIntelligenceError {
    match error {
        WorkspaceError::NotFound { .. } | WorkspaceError::InvalidArgument { .. } => {
            CodeIntelligenceError::InvalidPath {
                path: path.clone(),
                message: error.to_string(),
            }
        }
        WorkspaceError::Timeout { op, duration } => CodeIntelligenceError::Timeout {
            operation: op,
            duration,
        },
        other => CodeIntelligenceError::Unavailable {
            message: format!("failed to read saved document {}: {other}", path.as_str()),
        },
    }
}

pub(super) fn map_language_error(
    profile: ProjectLanguageProfile,
    error: LanguageRuntimeError,
) -> CodeIntelligenceError {
    match error {
        LanguageRuntimeError::InvalidPath { path, message } => {
            CodeIntelligenceError::InvalidPath { path, message }
        }
        LanguageRuntimeError::UnsupportedPath { path } => CodeIntelligenceError::Unsupported {
            operation: "language".to_owned(),
            message: format!("saved document {} is not supported", path.as_str()),
        },
        LanguageRuntimeError::Unsupported { operation } => CodeIntelligenceError::Unsupported {
            operation: operation.to_owned(),
            message: "the active language runtime did not advertise this capability".to_owned(),
        },
        LanguageRuntimeError::InvalidPosition { path, position } => {
            CodeIntelligenceError::InvalidPosition { path, position }
        }
        LanguageRuntimeError::PendingDiagnostics { path } => CodeIntelligenceError::Unavailable {
            message: format!(
                "diagnostics for {} have not been received yet",
                path.as_str()
            ),
        },
        LanguageRuntimeError::Cancelled => CodeIntelligenceError::Cancelled,
        LanguageRuntimeError::Timeout {
            operation,
            duration,
        } => CodeIntelligenceError::Timeout {
            operation: operation.to_owned(),
            duration,
        },
        LanguageRuntimeError::Client {
            source: LspClientError::Cancelled,
            ..
        } => CodeIntelligenceError::Cancelled,
        LanguageRuntimeError::Client {
            source: LspClientError::Timeout { method, duration },
            ..
        } => CodeIntelligenceError::Timeout {
            operation: method,
            duration,
        },
        LanguageRuntimeError::Client {
            source: LspClientError::Closed { message },
            ..
        }
        | LanguageRuntimeError::Client {
            source: LspClientError::Transport { message },
            ..
        } => CodeIntelligenceError::ProcessExited {
            language: profile_language(profile),
            message,
        },
        LanguageRuntimeError::InvalidRoot { root, message } => CodeIntelligenceError::Unavailable {
            message: format!("invalid workspace root {root:?}: {message}"),
        },
        LanguageRuntimeError::Process { operation, source } => CodeIntelligenceError::Unavailable {
            message: format!("language runtime could not {operation}: {source}"),
        },
        other => CodeIntelligenceError::Protocol {
            message: other.to_string(),
        },
    }
}

pub(super) fn profile_language(profile: ProjectLanguageProfile) -> LanguageId {
    match profile {
        ProjectLanguageProfile::Rust => LanguageId::from("rust"),
        ProjectLanguageProfile::TypeScriptJavaScript => LanguageId::from("typescript-javascript"),
    }
}

pub(super) fn union_capabilities(
    target: &mut CodeIntelligenceCapabilities,
    source: CodeIntelligenceCapabilities,
) {
    target.document_symbols |= source.document_symbols;
    target.workspace_symbols |= source.workspace_symbols;
    target.definition |= source.definition;
    target.declaration |= source.declaration;
    target.references |= source.references;
    target.implementations |= source.implementations;
    target.diagnostics |= source.diagnostics;
}

pub(super) fn publish_stopped_language_status(
    sender: &watch::Sender<CodeIntelligenceStatus>,
    language: LanguageId,
    message: String,
) {
    let mut status = sender.borrow().clone();
    if let Some(current) = status
        .languages
        .iter_mut()
        .find(|current| current.language == language)
    {
        current.state = CodeIntelligenceState::Unavailable;
        current.capabilities = CodeIntelligenceCapabilities::default();
        current.message = Some(message);
    } else {
        status.languages.push(CodeIntelligenceLanguageStatus {
            language,
            state: CodeIntelligenceState::Unavailable,
            capabilities: CodeIntelligenceCapabilities::default(),
            message: Some(message),
        });
    }

    let mut capabilities = CodeIntelligenceCapabilities::default();
    let mut ready = 0_usize;
    let mut unavailable = 0_usize;
    let mut starting = 0_usize;
    for current in &status.languages {
        match current.state {
            CodeIntelligenceState::Ready => {
                ready += 1;
                union_capabilities(&mut capabilities, current.capabilities);
            }
            CodeIntelligenceState::Degraded => {
                ready += 1;
                unavailable += 1;
                union_capabilities(&mut capabilities, current.capabilities);
            }
            CodeIntelligenceState::Starting => starting += 1,
            CodeIntelligenceState::Unavailable => unavailable += 1,
        }
    }
    status.state = if ready > 0 && unavailable > 0 {
        CodeIntelligenceState::Degraded
    } else if ready > 0 {
        CodeIntelligenceState::Ready
    } else if unavailable > 0 && starting == 0 {
        CodeIntelligenceState::Unavailable
    } else if starting > 0 {
        CodeIntelligenceState::Starting
    } else {
        CodeIntelligenceState::Unavailable
    };
    status.capabilities = capabilities;
    status.message = Some("one or more language runtimes stopped unexpectedly".to_owned());
    sender.send_replace(status);
}

pub(super) fn symbol_key(symbol: &SymbolInformation) -> (String, u32, u32, String) {
    (
        symbol.location.path.as_str().to_owned(),
        symbol.location.range.start.line,
        symbol.location.range.start.character,
        symbol.name.clone(),
    )
}
