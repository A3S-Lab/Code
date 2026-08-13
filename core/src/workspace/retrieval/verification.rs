//! Authoritative source verification shared by semantic and hybrid retrieval.

use super::{digest_content, WorkspaceChunk, WorkspaceRetrievalError};
use crate::workspace::{WorkspaceFileSystem, WorkspacePath};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Retain ranked candidates whose complete file digest and exact chunk byte
/// range still match the current workspace backend.
///
/// Candidate paths are grouped so each file is read at most once. Paths are
/// read in best-ranked candidate order and verification stops after finding
/// `limit + 1` current hits, which bounds remote I/O while preserving exact
/// truncation. Source text is released after that file's candidates have been
/// checked; the verifier does not retain a query-wide source cache.
pub(crate) async fn retain_verified<T>(
    candidates: Vec<T>,
    limit: usize,
    chunk_of: impl Fn(&T) -> &WorkspaceChunk,
    file_system: &dyn WorkspaceFileSystem,
    operation_timeout: Option<Duration>,
    runtime_cancellation: &CancellationToken,
    caller_cancellation: &CancellationToken,
) -> Result<(Vec<T>, bool, bool), WorkspaceRetrievalError> {
    let mut candidates_by_path = HashMap::<Arc<str>, Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        candidates_by_path
            .entry(Arc::clone(&chunk_of(candidate).path))
            .or_default()
            .push(index);
    }

    let mut verified = vec![None; candidates.len()];
    let mut verified_count = 0usize;
    for candidate_index in 0..candidates.len() {
        if verified[candidate_index].is_none() {
            let candidate = &candidates[candidate_index];
            let chunk = chunk_of(candidate);
            let path_key = Arc::clone(&chunk.path);
            let indices = candidates_by_path
                .remove(&path_key)
                .expect("candidate path must have a pending verification group");
            let path = WorkspacePath::from_normalized(path_key.as_ref());
            let read = async {
                match operation_timeout {
                    Some(timeout) => tokio::time::timeout(timeout, file_system.read_text(&path))
                        .await
                        .ok()
                        .and_then(Result::ok),
                    None => file_system.read_text(&path).await.ok(),
                }
            };
            let content = tokio::select! {
                biased;
                _ = caller_cancellation.cancelled() => {
                    return Err(WorkspaceRetrievalError::Cancelled);
                }
                _ = runtime_cancellation.cancelled() => {
                    return Err(WorkspaceRetrievalError::Cancelled);
                }
                content = read => content,
            };
            match content {
                Some(content) => {
                    let digest = digest_content(&content);
                    for index in indices {
                        let chunk = chunk_of(&candidates[index]);
                        verified[index] = Some(
                            digest.as_ref() == chunk.content_digest.as_ref()
                                && content
                                    .get(chunk.start_byte..chunk.end_byte)
                                    .is_some_and(|text| text == chunk.text.as_ref()),
                        );
                    }
                }
                None => {
                    for index in indices {
                        verified[index] = Some(false);
                    }
                }
            }
        }

        if verified[candidate_index] == Some(true) {
            verified_count = verified_count.saturating_add(1);
            if verified_count > limit {
                break;
            }
        }
    }

    let filtered = verified.contains(&Some(false));
    let retained = candidates
        .into_iter()
        .zip(verified)
        .filter_map(|(candidate, verified)| (verified == Some(true)).then_some(candidate))
        .take(limit)
        .collect();
    Ok((retained, filtered, verified_count > limit))
}
