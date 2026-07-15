//! Filesystem watcher event batching and normalization.

use super::scanner::path_has_noise_component;
use super::scanner::scan_workspace_files_cancellable;
use super::{
    is_relevant_event, normalize_relative_path_lossy, update_state, LocalWorkspaceManifestSnapshot,
    ManifestState, WorkspaceFileChange, WorkspaceFileChangeKind, WorkspacePath,
};
use notify::{
    event::{ModifyKind, RenameMode},
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};
use std::thread;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const WATCH_STARTUP_SCAN_INTERVAL: Duration = Duration::from_secs(1);
const WATCH_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) async fn run_manifest_task(
    root: PathBuf,
    state: Arc<RwLock<ManifestState>>,
    snapshots: broadcast::Sender<LocalWorkspaceManifestSnapshot>,
    changes: broadcast::Sender<WorkspaceFileChange>,
    scan_cancelled: Arc<AtomicBool>,
) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    // Readiness must not depend on the platform watcher service. Watcher
    // construction is blocking on some platforms and can be slow or fail
    // under resource pressure, while the initial manifest is still useful.
    publish_scan(&root, &state, &snapshots, &scan_cancelled).await;
    if scan_cancelled.load(Ordering::Acquire) {
        return;
    }

    let watcher_root = root.clone();
    let watcher_cancelled = Arc::clone(&scan_cancelled);
    let (watcher_tx, mut watcher_rx) = oneshot::channel();
    // Watcher construction and recursive registration are synchronous platform
    // calls with no cancellation API. Keep them outside Tokio's blocking pool:
    // dropping a runtime waits for every spawn_blocking job, while a detached
    // setup thread cannot hold runtime teardown hostage.
    let _ = thread::Builder::new()
        .name("a3s-workspace-watcher".to_owned())
        .spawn(move || {
            if watcher_cancelled.load(Ordering::Acquire) {
                return;
            }
            let watcher = RecommendedWatcher::new(
                move |event| {
                    let _ = event_tx.send(event);
                },
                Config::default(),
            )
            .and_then(|mut watcher| {
                if watcher_cancelled.load(Ordering::Acquire) {
                    return Ok(watcher);
                }
                watcher.watch(&watcher_root, RecursiveMode::Recursive)?;
                Ok(watcher)
            });
            match watcher {
                Ok(watcher) => {
                    if watcher_cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    if watcher_tx.send(Ok(())).is_err() {
                        return;
                    }
                    // Keep construction, registration, ownership, and Drop on
                    // this detached thread. On macOS, dropping an FSEvents
                    // watcher can join its platform thread and must never hold
                    // a Tokio worker or runtime teardown hostage.
                    while !watcher_cancelled.load(Ordering::Acquire) {
                        thread::park_timeout(WATCH_SHUTDOWN_POLL_INTERVAL);
                    }
                    drop(watcher);
                }
                Err(error) => {
                    let _ = watcher_tx.send(Err(error));
                }
            }
        });
    let watcher_ready = loop {
        tokio::select! {
            result = &mut watcher_rx => break result,
            _ = tokio::time::sleep(WATCH_STARTUP_SCAN_INTERVAL) => {
                // Continue providing a fresh manifest while the platform
                // watcher service is slow to initialize.
                publish_scan(&root, &state, &snapshots, &scan_cancelled).await;
            }
        }
    };
    let Ok(Ok(())) = watcher_ready else {
        return;
    };
    // Close the scan-to-watch registration window: files changed while the
    // watcher was being constructed are captured by this second scan.
    publish_scan(&root, &state, &snapshots, &scan_cancelled).await;

    while let Some(event) = event_rx.recv().await {
        let Ok(event) = event else {
            continue;
        };
        if !is_relevant_event(&event, &root) {
            continue;
        }
        let mut events = vec![event];
        tokio::time::sleep(WATCH_DEBOUNCE).await;
        while let Ok(event) = event_rx.try_recv() {
            if let Ok(event) = event {
                if !is_relevant_event(&event, &root) {
                    continue;
                }
                events.push(event);
            }
        }
        let known_paths = state
            .read()
            .map(|state| state.index.by_path.keys().cloned().collect::<HashSet<_>>())
            .unwrap_or_default();
        let file_changes = normalize_file_changes(&root, &events, &known_paths);
        publish_scan(&root, &state, &snapshots, &scan_cancelled).await;
        for change in file_changes {
            let _ = changes.send(change);
        }
    }
}

pub(super) fn normalize_file_changes(
    root: &Path,
    events: &[Event],
    known_paths: &HashSet<String>,
) -> Vec<WorkspaceFileChange> {
    let mut changes = Vec::new();
    for event in events {
        match event.kind {
            EventKind::Access(_) => {}
            EventKind::Create(_) => {
                push_created_event_paths(&mut changes, root, &event.paths, known_paths);
            }
            EventKind::Remove(_) => {
                push_event_paths(
                    &mut changes,
                    root,
                    &event.paths,
                    WorkspaceFileChangeKind::Deleted,
                );
            }
            EventKind::Modify(ModifyKind::Name(mode)) => {
                push_rename_changes(&mut changes, root, &event.paths, mode);
            }
            EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                push_event_paths(
                    &mut changes,
                    root,
                    &event.paths,
                    WorkspaceFileChangeKind::Changed,
                );
            }
        }
    }
    changes
}

fn push_rename_changes(
    changes: &mut Vec<WorkspaceFileChange>,
    root: &Path,
    paths: &[PathBuf],
    mode: RenameMode,
) {
    match mode {
        RenameMode::Both if paths.len() >= 2 => {
            push_file_change(changes, root, &paths[0], WorkspaceFileChangeKind::Deleted);
            push_file_change(
                changes,
                root,
                &paths[paths.len() - 1],
                WorkspaceFileChangeKind::Created,
            );
        }
        RenameMode::From => {
            push_event_paths(changes, root, paths, WorkspaceFileChangeKind::Deleted)
        }
        RenameMode::To => push_event_paths(changes, root, paths, WorkspaceFileChangeKind::Created),
        RenameMode::Any | RenameMode::Other if paths.len() >= 2 => {
            push_file_change(changes, root, &paths[0], WorkspaceFileChangeKind::Deleted);
            push_file_change(
                changes,
                root,
                &paths[paths.len() - 1],
                WorkspaceFileChangeKind::Created,
            );
        }
        RenameMode::Any | RenameMode::Other => {
            push_event_paths(changes, root, paths, WorkspaceFileChangeKind::Changed)
        }
        RenameMode::Both => {}
    }
}

fn push_created_event_paths(
    changes: &mut Vec<WorkspaceFileChange>,
    root: &Path,
    paths: &[PathBuf],
    known_paths: &HashSet<String>,
) {
    for path in paths {
        let Some(path) = normalize_event_path(root, path) else {
            continue;
        };
        let kind = if known_paths.contains(path.as_str())
            && !changes.iter().any(|change| {
                change.path == path && change.kind == WorkspaceFileChangeKind::Deleted
            }) {
            WorkspaceFileChangeKind::Changed
        } else {
            WorkspaceFileChangeKind::Created
        };
        push_normalized_file_change(changes, path, kind);
    }
}

fn push_event_paths(
    changes: &mut Vec<WorkspaceFileChange>,
    root: &Path,
    paths: &[PathBuf],
    kind: WorkspaceFileChangeKind,
) {
    for path in paths {
        push_file_change(changes, root, path, kind);
    }
}

fn push_file_change(
    changes: &mut Vec<WorkspaceFileChange>,
    root: &Path,
    path: &Path,
    kind: WorkspaceFileChangeKind,
) {
    let Some(path) = normalize_event_path(root, path) else {
        return;
    };
    push_normalized_file_change(changes, path, kind);
}

fn push_normalized_file_change(
    changes: &mut Vec<WorkspaceFileChange>,
    path: WorkspacePath,
    kind: WorkspaceFileChangeKind,
) {
    let Some(existing_index) = changes.iter().position(|change| change.path == path) else {
        changes.push(WorkspaceFileChange { path, kind });
        return;
    };
    let existing = changes[existing_index].kind;
    match merge_file_change_kinds(existing, kind) {
        Some(kind) => changes[existing_index].kind = kind,
        None => {
            changes.remove(existing_index);
        }
    }
}

fn normalize_event_path(root: &Path, path: &Path) -> Option<WorkspacePath> {
    let relative = path.strip_prefix(root).ok()?;
    if path_has_noise_component(relative) {
        return None;
    }
    let normalized = normalize_relative_path_lossy(relative)?;
    (!normalized.is_empty()).then(|| WorkspacePath::from_normalized(normalized))
}

fn merge_file_change_kinds(
    existing: WorkspaceFileChangeKind,
    incoming: WorkspaceFileChangeKind,
) -> Option<WorkspaceFileChangeKind> {
    use WorkspaceFileChangeKind::{Changed, Created, Deleted};

    match (existing, incoming) {
        (Created, Deleted) => None,
        (Deleted, Created) => Some(Changed),
        (Created, _) => Some(Created),
        (Changed, Deleted) => Some(Deleted),
        (Changed, _) => Some(Changed),
        (Deleted, _) => Some(Deleted),
    }
}

async fn publish_scan(
    root: &Path,
    state: &Arc<RwLock<ManifestState>>,
    snapshots: &broadcast::Sender<LocalWorkspaceManifestSnapshot>,
    scan_cancelled: &Arc<AtomicBool>,
) {
    let root = root.to_path_buf();
    let scan_cancelled_for_task = Arc::clone(scan_cancelled);
    let (scan_tx, scan_rx) = oneshot::channel();
    // Filesystem traversal can block inside one platform call even after the
    // cooperative flag is set. A detached standard thread keeps that call out
    // of Tokio's blocking pool, so runtime teardown never waits for it.
    if thread::Builder::new()
        .name("a3s-workspace-scan".to_owned())
        .spawn(move || {
            let files = scan_workspace_files_cancellable(&root, &scan_cancelled_for_task);
            let _ = scan_tx.send(files);
        })
        .is_err()
    {
        return;
    }
    let Ok(Some(files)) = scan_rx.await else {
        return;
    };
    if scan_cancelled.load(Ordering::Acquire) {
        return;
    }
    let Some(snapshot) = update_state(state, files) else {
        return;
    };
    let _ = snapshots.send(snapshot);
}
