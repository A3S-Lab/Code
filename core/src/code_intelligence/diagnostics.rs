use std::collections::HashMap;

use tokio::sync::{futures::Notified, Mutex, Notify};

use super::{CodeDiagnostic, DocumentRevision};
use crate::workspace::WorkspacePath;

/// Result of looking up diagnostics for one saved document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiagnosticsLookup {
    /// No diagnostics notification has been received for this document.
    NotReceived,
    /// A notification was received. An empty vector explicitly clears prior
    /// diagnostics and must not be collapsed into [`Self::NotReceived`].
    Received {
        diagnostics: Vec<CodeDiagnostic>,
        revision: Option<DocumentRevision>,
    },
}

#[derive(Debug, Clone)]
struct DiagnosticsEntry {
    diagnostics: Vec<CodeDiagnostic>,
    revision: Option<DocumentRevision>,
    last_touch: u64,
}

#[derive(Debug)]
struct DiagnosticsState {
    entries: HashMap<WorkspacePath, DiagnosticsEntry>,
    clock: u64,
}

impl DiagnosticsState {
    fn next_tick(&mut self) -> u64 {
        if self.clock == u64::MAX {
            self.renumber_clocks();
        }
        self.clock += 1;
        self.clock
    }

    fn renumber_clocks(&mut self) {
        let mut touches = self
            .entries
            .iter()
            .map(|(path, entry)| (path.clone(), entry.last_touch))
            .collect::<Vec<_>>();
        touches.sort_by_key(|(_, touch)| *touch);
        for (index, (path, _)) in touches.into_iter().enumerate() {
            if let Some(entry) = self.entries.get_mut(&path) {
                entry.last_touch = index as u64;
            }
        }
        self.clock = self.entries.len() as u64;
    }

    fn least_recent_path(&self) -> Option<WorkspacePath> {
        self.entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_touch)
            .map(|(path, _)| path.clone())
    }
}

/// Bounded diagnostics received from language servers for saved documents.
#[derive(Debug)]
pub(crate) struct DiagnosticsStore {
    capacity: usize,
    state: Mutex<DiagnosticsState>,
    updates: Notify,
}

impl DiagnosticsStore {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(DiagnosticsState {
                entries: HashMap::new(),
                clock: 0,
            }),
            updates: Notify::new(),
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

    /// Publish the complete current diagnostics set for one document.
    ///
    /// Returns the least-recently-used path evicted to stay within capacity.
    pub(crate) async fn publish(
        &self,
        path: &WorkspacePath,
        diagnostics: Vec<CodeDiagnostic>,
        revision: Option<DocumentRevision>,
    ) -> Option<WorkspacePath> {
        let evicted = {
            let mut state = self.state.lock().await;
            let tick = state.next_tick();
            let evicted =
                if !state.entries.contains_key(path) && state.entries.len() >= self.capacity {
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
                DiagnosticsEntry {
                    diagnostics,
                    revision,
                    last_touch: tick,
                },
            );
            evicted
        };
        self.updates.notify_waiters();
        evicted
    }

    /// Subscribe before checking the store to avoid missing a concurrent
    /// publish. Callers must pin and enable the returned future before lookup.
    pub(crate) fn notified(&self) -> Notified<'_> {
        self.updates.notified()
    }

    pub(crate) async fn lookup(&self, path: &WorkspacePath) -> DiagnosticsLookup {
        let mut state = self.state.lock().await;
        let tick = state.next_tick();
        let Some(entry) = state.entries.get_mut(path) else {
            return DiagnosticsLookup::NotReceived;
        };
        entry.last_touch = tick;
        DiagnosticsLookup::Received {
            diagnostics: entry.diagnostics.clone(),
            revision: entry.revision,
        }
    }

    pub(crate) async fn remove(&self, path: &WorkspacePath) -> DiagnosticsLookup {
        let mut state = self.state.lock().await;
        match state.entries.remove(path) {
            Some(entry) => DiagnosticsLookup::Received {
                diagnostics: entry.diagnostics,
                revision: entry.revision,
            },
            None => DiagnosticsLookup::NotReceived,
        }
    }

    pub(crate) async fn invalidate(&self, path: &WorkspacePath) -> bool {
        !matches!(self.remove(path).await, DiagnosticsLookup::NotReceived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_intelligence::{CodeLocation, CodePosition, CodeRange};

    fn path(value: &str) -> WorkspacePath {
        WorkspacePath::from_normalized(value)
    }

    fn diagnostic(path: &WorkspacePath, message: &str) -> CodeDiagnostic {
        CodeDiagnostic {
            location: CodeLocation {
                path: path.clone(),
                range: CodeRange::new(CodePosition::new(0, 0), CodePosition::new(0, 1)),
            },
            severity: None,
            code: None,
            source: Some("test".to_owned()),
            message: message.to_owned(),
        }
    }

    #[tokio::test]
    async fn empty_received_is_distinct_from_not_received() {
        let store = DiagnosticsStore::new(2);
        let document = path("src/lib.rs");
        assert_eq!(
            store.lookup(&document).await,
            DiagnosticsLookup::NotReceived
        );

        store.publish(&document, Vec::new(), None).await;
        assert_eq!(
            store.lookup(&document).await,
            DiagnosticsLookup::Received {
                diagnostics: Vec::new(),
                revision: None,
            }
        );
    }

    #[tokio::test]
    async fn publish_preserves_document_revision() {
        let store = DiagnosticsStore::new(2);
        let document = path("src/lib.rs");
        let revision = DocumentRevision::new(42);
        let item = diagnostic(&document, "error");
        store
            .publish(&document, vec![item.clone()], Some(revision))
            .await;

        assert_eq!(
            store.lookup(&document).await,
            DiagnosticsLookup::Received {
                diagnostics: vec![item],
                revision: Some(revision),
            }
        );
    }

    #[tokio::test]
    async fn least_recent_entry_is_evicted() {
        let store = DiagnosticsStore::new(2);
        let a = path("a.rs");
        let b = path("b.rs");
        let c = path("c.rs");
        store.publish(&a, vec![diagnostic(&a, "a")], None).await;
        store.publish(&b, vec![diagnostic(&b, "b")], None).await;
        let _ = store.lookup(&a).await;

        assert_eq!(
            store.publish(&c, vec![diagnostic(&c, "c")], None).await,
            Some(b.clone())
        );
        assert_eq!(store.lookup(&b).await, DiagnosticsLookup::NotReceived);
        assert!(matches!(
            store.lookup(&a).await,
            DiagnosticsLookup::Received { .. }
        ));
        assert_eq!(store.len().await, 2);
        assert_eq!(DiagnosticsStore::new(0).capacity(), 1);
    }

    #[tokio::test]
    async fn enabled_notification_observes_a_concurrent_publish() {
        let store = DiagnosticsStore::new(1);
        let document = path("a.rs");
        let notified = store.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        store.publish(&document, Vec::new(), None).await;

        tokio::time::timeout(std::time::Duration::from_secs(1), notified)
            .await
            .expect("enabled subscribers must be notified");
    }

    #[tokio::test]
    async fn invalidate_restores_not_received_state() {
        let store = DiagnosticsStore::new(1);
        let document = path("a.rs");
        store
            .publish(&document, vec![diagnostic(&document, "old")], None)
            .await;
        assert!(store.invalidate(&document).await);
        assert!(!store.invalidate(&document).await);
        assert_eq!(
            store.lookup(&document).await,
            DiagnosticsLookup::NotReceived
        );
    }
}
