//! Blocking manifest grep work, owned independently of the request future.

use super::{
    candidate_indices_for_glob, dirty_order_can_change, glob_matches, order_grep_candidates,
    relative_to_base, CandidateIndices, ManifestSearchSnapshot,
};
use crate::workspace::{
    escape_control_chars_for_display, LocalWorkspaceBackend, SharedGrepCandidateIndex,
    WorkspaceGrepOutcome, WorkspaceGrepRequest, WorkspaceGrepResult, WorkspacePath,
};
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(feature = "grep-trigram")]
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) struct GrepWorker {
    pub(super) local: Arc<LocalWorkspaceBackend>,
    pub(super) search_snapshot: ManifestSearchSnapshot,
    pub(super) recent_ranks: HashMap<String, usize>,
    pub(super) grep_candidates: Option<SharedGrepCandidateIndex>,
    #[cfg(feature = "grep-trigram")]
    pub(super) auto_grep_candidates: Arc<Mutex<Option<(u64, SharedGrepCandidateIndex)>>>,
}

impl GrepWorker {
    pub(super) fn run(
        self,
        request: WorkspaceGrepRequest,
        cancellation: &CancellationToken,
    ) -> Result<WorkspaceGrepOutcome> {
        if cancellation.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        self.local.ensure_search_base_allowed(&request.base)?;
        let search_snapshot = &self.search_snapshot;
        let recent_ranks = &self.recent_ranks;
        let regex_pattern = if request.case_insensitive {
            format!("(?i){}", request.pattern)
        } else {
            request.pattern.clone()
        };
        let regex = regex::Regex::new(&regex_pattern)
            .map_err(|e| anyhow!("Invalid regex pattern '{}': {}", request.pattern, e))?;
        let glob = request
            .glob
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .map_err(|e| anyhow!("Invalid grep glob filter: {e}"))?;

        let mut output = String::new();
        let mut match_count = 0;
        let mut file_count = 0;
        let mut total_size = 0;
        let mut matched_paths = Vec::new();
        let mut skipped_oversize = Vec::new();
        let metadata_only = request.max_output_size == 0;

        let candidates = request
            .glob
            .as_deref()
            .map(|glob| candidate_indices_for_glob(&search_snapshot.index, &request.base, glob))
            .unwrap_or_else(|| CandidateIndices::Indexed(&search_snapshot.index.all));
        let content_filter = self.grep_content_candidate_paths(
            &request.pattern,
            request.case_insensitive,
            search_snapshot,
        );
        if cancellation.is_cancelled() {
            anyhow::bail!("cancelled");
        }
        // Dirty order only moves files outside the recent window.
        let dirty = if dirty_order_can_change(
            &candidates,
            &search_snapshot.snapshot.files,
            recent_ranks,
            content_filter.as_ref(),
        ) {
            crate::git::dirty_paths(&self.local.root).unwrap_or_default()
        } else {
            HashSet::new()
        };

        for file_index in
            order_grep_candidates(&candidates, &search_snapshot.index, recent_ranks, &dirty)
        {
            if cancellation.is_cancelled() {
                anyhow::bail!("cancelled");
            }
            let Some(file) = search_snapshot.snapshot.files.get(file_index) else {
                continue;
            };
            if file.binary {
                continue;
            }
            if let Some(ref allowed) = content_filter {
                if !allowed.contains(&file.path) {
                    continue;
                }
            }
            let Some(relative_to_base) = relative_to_base(&file.path, &request.base) else {
                continue;
            };
            if let Some(glob) = &glob {
                if !glob_matches(glob, relative_to_base) {
                    continue;
                }
            }

            let workspace_path = WorkspacePath::from_normalized(file.path.clone());
            let Some(content) = self.local.read_search_file(&workspace_path) else {
                if self.local.search_file_is_oversize(&workspace_path) {
                    skipped_oversize.push(workspace_path);
                }
                continue;
            };
            let lines: Vec<&str> = content.lines().collect();
            let mut file_matches = Vec::new();
            for (line_idx, line) in lines.iter().enumerate() {
                if cancellation.is_cancelled() {
                    anyhow::bail!("cancelled");
                }
                if regex.is_match(line) {
                    file_matches.push(line_idx);
                }
            }

            if file_matches.is_empty() {
                continue;
            }

            file_count += 1;
            let display_path = escape_control_chars_for_display(&file.path);
            let mut path_recorded = false;
            for &match_idx in &file_matches {
                if cancellation.is_cancelled() {
                    anyhow::bail!("cancelled");
                }
                if !metadata_only && total_size > request.max_output_size {
                    return Ok(WorkspaceGrepOutcome {
                        result: WorkspaceGrepResult {
                            output,
                            match_count,
                            file_count,
                            truncated: true,
                        },
                        matched_paths: Some(matched_paths),
                        skipped_oversize,
                    });
                }

                if !path_recorded {
                    matched_paths.push(workspace_path.clone());
                    path_recorded = true;
                }
                match_count += 1;
                if metadata_only {
                    continue;
                }
                let start = match_idx.saturating_sub(request.context_lines);
                let end = (match_idx + request.context_lines + 1).min(lines.len());

                for (i, line) in lines[start..end].iter().enumerate() {
                    let abs_i = start + i;
                    let prefix = if abs_i == match_idx { ">" } else { " " };
                    let line = format!("{}{}:{}: {}\n", prefix, display_path, abs_i + 1, line);
                    total_size += line.len();
                    output.push_str(&line);
                }

                if request.context_lines > 0 {
                    output.push_str("--\n");
                    total_size += 3;
                }
            }
        }

        Ok(WorkspaceGrepOutcome {
            result: WorkspaceGrepResult {
                output,
                match_count,
                file_count,
                truncated: false,
            },
            matched_paths: Some(matched_paths),
            skipped_oversize,
        })
    }

    fn grep_content_candidate_paths(
        &self,
        pattern: &str,
        case_insensitive: bool,
        search_snapshot: &ManifestSearchSnapshot,
    ) -> Option<std::collections::BTreeSet<String>> {
        let index = self.resolve_grep_candidate_index(search_snapshot)?;
        match index.select_paths(pattern, case_insensitive) {
            crate::workspace::GrepCandidateSelection::Paths(paths) => Some(paths),
            crate::workspace::GrepCandidateSelection::Unconstrained
            | crate::workspace::GrepCandidateSelection::Unavailable => None,
        }
    }

    fn resolve_grep_candidate_index(
        &self,
        search_snapshot: &ManifestSearchSnapshot,
    ) -> Option<crate::workspace::SharedGrepCandidateIndex> {
        if let Some(index) = self.grep_candidates.as_ref() {
            return Some(Arc::clone(index));
        }
        #[cfg(feature = "grep-trigram")]
        {
            self.ensure_auto_trigram_candidate_index(search_snapshot)
        }
        #[cfg(not(feature = "grep-trigram"))]
        {
            let _ = search_snapshot;
            None
        }
    }

    #[cfg(feature = "grep-trigram")]
    fn ensure_auto_trigram_candidate_index(
        &self,
        search_snapshot: &ManifestSearchSnapshot,
    ) -> Option<crate::workspace::SharedGrepCandidateIndex> {
        use crate::workspace::TrigramGrepCandidateIndex;

        let version = search_snapshot.snapshot.version;
        let mut guard = self.auto_grep_candidates.lock().ok()?;
        if let Some((cached_version, index)) = guard.as_ref() {
            if *cached_version == version {
                return Some(Arc::clone(index));
            }
        }

        let relative_paths: Vec<String> = search_snapshot
            .snapshot
            .files
            .iter()
            .filter(|file| !file.binary)
            .map(|file| file.path.clone())
            .collect();
        if relative_paths.is_empty() {
            return None;
        }

        let root = self.local.root.clone();
        let index_dir = TrigramGrepCandidateIndex::default_index_dir(&root);
        // Drop the previous generation before rewriting the stamp directory.
        *guard = None;
        let built = TrigramGrepCandidateIndex::build_from_relative_paths(
            &root,
            &index_dir,
            &relative_paths,
        )
        .ok()?;
        let index: crate::workspace::SharedGrepCandidateIndex = Arc::new(built);
        *guard = Some((version, Arc::clone(&index)));
        Some(index)
    }
}
