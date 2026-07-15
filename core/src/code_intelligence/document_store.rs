use std::collections::HashMap;

use tokio::sync::Mutex;

use super::{DocumentRevision, DocumentSnapshot};
use crate::workspace::WorkspacePath;

const INITIAL_LSP_VERSION: i32 = 1;

/// Saved-document transition the protocol runtime must apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentObservationKind {
    Opened,
    Changed,
    Unchanged,
    /// The previous protocol version could not be incremented. Callers must
    /// close and reopen the document with the returned reset version.
    Reopened,
}

/// Result of observing the latest saved contents of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocumentObservation {
    pub(crate) snapshot: DocumentSnapshot,
    pub(crate) lsp_version: i32,
    pub(crate) kind: DocumentObservationKind,
    pub(crate) evicted: Option<WorkspacePath>,
}

/// Metadata returned when a saved document is removed from the bounded store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemovedDocument {
    pub(crate) snapshot: DocumentSnapshot,
    pub(crate) lsp_version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DocumentStoreError {
    #[error("saved-document revision counter is exhausted")]
    RevisionExhausted,
}

#[derive(Debug, Clone)]
struct DocumentEntry {
    content_hash: String,
    revision: DocumentRevision,
    lsp_version: i32,
    last_touch: u64,
}

impl DocumentEntry {
    fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            revision: self.revision,
            content_hash: self.content_hash.clone(),
            stale: false,
        }
    }
}

#[derive(Debug)]
struct DocumentStoreState {
    entries: HashMap<WorkspacePath, DocumentEntry>,
    last_revision: u64,
    touch_clock: u64,
}

impl DocumentStoreState {
    fn next_revision(&mut self) -> Result<DocumentRevision, DocumentStoreError> {
        self.last_revision = self
            .last_revision
            .checked_add(1)
            .ok_or(DocumentStoreError::RevisionExhausted)?;
        Ok(DocumentRevision::new(self.last_revision))
    }

    fn next_touch(&mut self) -> u64 {
        // Touch ordering only chooses an eviction candidate. Renumbering keeps
        // that ordering intact without allowing the clock to wrap.
        if self.touch_clock == u64::MAX {
            let mut ordered = self
                .entries
                .iter()
                .map(|(path, entry)| (path.clone(), entry.last_touch))
                .collect::<Vec<_>>();
            ordered.sort_by_key(|(_, touch)| *touch);
            for (index, (path, _)) in ordered.into_iter().enumerate() {
                if let Some(entry) = self.entries.get_mut(&path) {
                    entry.last_touch = index as u64;
                }
            }
            self.touch_clock = self.entries.len() as u64;
        }
        self.touch_clock += 1;
        self.touch_clock
    }

    fn least_recent_path(&self) -> Option<WorkspacePath> {
        self.entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_touch)
            .map(|(path, _)| path.clone())
    }
}

/// Bounded, in-memory metadata for saved documents.
///
/// Source text is hashed before the mutex is acquired and is never retained.
#[derive(Debug)]
pub(crate) struct DocumentStore {
    capacity: usize,
    state: Mutex<DocumentStoreState>,
}

impl DocumentStore {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(DocumentStoreState {
                entries: HashMap::new(),
                last_revision: 0,
                touch_clock: 0,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) const fn capacity(&self) -> usize {
        self.capacity
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.state.lock().await.entries.len()
    }

    /// Observe saved text and return the protocol transition needed to make a
    /// language server reflect it.
    pub(crate) async fn observe(
        &self,
        path: &WorkspacePath,
        saved_content: &str,
    ) -> Result<DocumentObservation, DocumentStoreError> {
        let content_hash = sha256::digest(saved_content.as_bytes());
        let mut state = self.state.lock().await;
        let touch = state.next_touch();

        if let Some(existing) = state.entries.get(path) {
            if existing.content_hash == content_hash {
                let mut snapshot = existing.snapshot();
                let lsp_version = existing.lsp_version;
                if let Some(existing) = state.entries.get_mut(path) {
                    existing.last_touch = touch;
                    snapshot.stale = false;
                }
                return Ok(DocumentObservation {
                    snapshot,
                    lsp_version,
                    kind: DocumentObservationKind::Unchanged,
                    evicted: None,
                });
            }

            let previous_lsp_version = existing.lsp_version;
            let revision = state.next_revision()?;
            let (lsp_version, kind) = advance_lsp_version(previous_lsp_version);
            state.entries.insert(
                path.clone(),
                DocumentEntry {
                    content_hash: content_hash.clone(),
                    revision,
                    lsp_version,
                    last_touch: touch,
                },
            );
            return Ok(DocumentObservation {
                snapshot: DocumentSnapshot {
                    revision,
                    content_hash,
                    stale: false,
                },
                lsp_version,
                kind,
                evicted: None,
            });
        }

        // Allocate the monotonic revision before mutating the bounded set so
        // exhaustion cannot evict a valid entry on a failed observation.
        let revision = state.next_revision()?;
        let evicted = if state.entries.len() >= self.capacity {
            let evicted = state.least_recent_path();
            if let Some(path) = &evicted {
                state.entries.remove(path);
            }
            evicted
        } else {
            None
        };
        state.entries.insert(
            path.clone(),
            DocumentEntry {
                content_hash: content_hash.clone(),
                revision,
                lsp_version: INITIAL_LSP_VERSION,
                last_touch: touch,
            },
        );
        Ok(DocumentObservation {
            snapshot: DocumentSnapshot {
                revision,
                content_hash,
                stale: false,
            },
            lsp_version: INITIAL_LSP_VERSION,
            kind: DocumentObservationKind::Opened,
            evicted,
        })
    }

    /// Return the current saved snapshot and mark it as recently used.
    #[cfg(test)]
    pub(crate) async fn snapshot(&self, path: &WorkspacePath) -> Option<DocumentSnapshot> {
        let mut state = self.state.lock().await;
        let touch = state.next_touch();
        let entry = state.entries.get_mut(path)?;
        entry.last_touch = touch;
        Some(entry.snapshot())
    }

    #[cfg(test)]
    pub(crate) async fn touch(&self, path: &WorkspacePath) -> bool {
        let mut state = self.state.lock().await;
        let touch = state.next_touch();
        let Some(entry) = state.entries.get_mut(path) else {
            return false;
        };
        entry.last_touch = touch;
        true
    }

    /// Resolve a diagnostics version only when it still identifies the
    /// currently saved revision.
    pub(crate) async fn revision_for_lsp_version(
        &self,
        path: &WorkspacePath,
        lsp_version: i32,
    ) -> Option<DocumentRevision> {
        let mut state = self.state.lock().await;
        let touch = state.next_touch();
        let entry = state.entries.get_mut(path)?;
        if entry.lsp_version != lsp_version {
            return None;
        }
        entry.last_touch = touch;
        Some(entry.revision)
    }

    /// Compare a query's starting snapshot with the currently saved document.
    pub(crate) async fn complete_query(
        &self,
        path: &WorkspacePath,
        mut snapshot: DocumentSnapshot,
    ) -> DocumentSnapshot {
        let state = self.state.lock().await;
        let current_revision = state.entries.get(path).map(|entry| entry.revision);
        snapshot.stale |= current_revision != Some(snapshot.revision);
        snapshot
    }

    pub(crate) async fn remove(&self, path: &WorkspacePath) -> Option<RemovedDocument> {
        let mut state = self.state.lock().await;
        state.entries.remove(path).map(|entry| RemovedDocument {
            snapshot: entry.snapshot(),
            lsp_version: entry.lsp_version,
        })
    }

    /// Forget metadata after an external saved-file change. A later observe is
    /// treated as a fresh open.
    pub(crate) async fn invalidate(&self, path: &WorkspacePath) -> Option<RemovedDocument> {
        self.remove(path).await
    }
}

fn advance_lsp_version(current: i32) -> (i32, DocumentObservationKind) {
    match current.checked_add(1) {
        Some(version) => (version, DocumentObservationKind::Changed),
        None => (INITIAL_LSP_VERSION, DocumentObservationKind::Reopened),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> WorkspacePath {
        WorkspacePath::from_normalized(value)
    }

    #[tokio::test]
    async fn unchanged_hash_keeps_revision_and_change_advances_it() {
        let store = DocumentStore::new(4);
        let first = store.observe(&path("src/lib.rs"), "hello").await.unwrap();
        assert_eq!(first.kind, DocumentObservationKind::Opened);
        assert_eq!(first.lsp_version, INITIAL_LSP_VERSION);
        assert_eq!(
            first.snapshot.content_hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );

        let unchanged = store.observe(&path("src/lib.rs"), "hello").await.unwrap();
        assert_eq!(unchanged.kind, DocumentObservationKind::Unchanged);
        assert_eq!(unchanged.snapshot, first.snapshot);
        assert_eq!(unchanged.lsp_version, first.lsp_version);

        let changed = store
            .observe(&path("src/lib.rs"), "hello again")
            .await
            .unwrap();
        assert_eq!(changed.kind, DocumentObservationKind::Changed);
        assert!(changed.snapshot.revision > first.snapshot.revision);
        assert_eq!(changed.lsp_version, first.lsp_version + 1);

        let other = store.observe(&path("src/main.rs"), "main").await.unwrap();
        assert!(other.snapshot.revision > changed.snapshot.revision);
    }

    #[tokio::test]
    async fn least_recent_document_is_evicted_and_capacity_is_at_least_one() {
        let store = DocumentStore::new(2);
        store.observe(&path("a.rs"), "a").await.unwrap();
        store.observe(&path("b.rs"), "b").await.unwrap();
        assert!(store.touch(&path("a.rs")).await);

        let observed = store.observe(&path("c.rs"), "c").await.unwrap();
        assert_eq!(observed.evicted, Some(path("b.rs")));
        assert!(store.snapshot(&path("a.rs")).await.is_some());
        assert!(store.snapshot(&path("b.rs")).await.is_none());
        assert_eq!(store.len().await, 2);

        let minimum = DocumentStore::new(0);
        assert_eq!(minimum.capacity(), 1);
    }

    #[test]
    fn version_overflow_requires_reopen_instead_of_saturation() {
        assert_eq!(
            advance_lsp_version(i32::MAX - 1),
            (i32::MAX, DocumentObservationKind::Changed)
        );
        assert_eq!(
            advance_lsp_version(i32::MAX),
            (INITIAL_LSP_VERSION, DocumentObservationKind::Reopened)
        );
    }

    #[tokio::test]
    async fn lsp_version_only_resolves_the_current_revision() {
        let store = DocumentStore::new(2);
        let opened = store.observe(&path("a.rs"), "a").await.unwrap();
        assert_eq!(
            store
                .revision_for_lsp_version(&path("a.rs"), opened.lsp_version)
                .await,
            Some(opened.snapshot.revision)
        );

        let changed = store.observe(&path("a.rs"), "b").await.unwrap();
        assert_eq!(
            store
                .revision_for_lsp_version(&path("a.rs"), opened.lsp_version)
                .await,
            None
        );
        assert_eq!(
            store
                .revision_for_lsp_version(&path("a.rs"), changed.lsp_version)
                .await,
            Some(changed.snapshot.revision)
        );
    }

    #[tokio::test]
    async fn query_completion_marks_changed_or_removed_document_stale() {
        let store = DocumentStore::new(2);
        let first = store.observe(&path("a.rs"), "a").await.unwrap();
        let fresh = store
            .complete_query(&path("a.rs"), first.snapshot.clone())
            .await;
        assert!(!fresh.stale);

        store.observe(&path("a.rs"), "b").await.unwrap();
        let stale = store
            .complete_query(&path("a.rs"), first.snapshot.clone())
            .await;
        assert!(stale.stale);

        let current = store.snapshot(&path("a.rs")).await.unwrap();
        let removed = store.invalidate(&path("a.rs")).await.unwrap();
        assert_eq!(removed.snapshot, current);
        assert!(store.complete_query(&path("a.rs"), current).await.stale);
    }
}
