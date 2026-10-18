#![cfg(test)]

use super::*;
use crate::workspace::{GrepCandidateIndex, GrepCandidateSelection};
use std::sync::{mpsc, Mutex};

struct GatedCandidates {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    expired: Arc<AtomicBool>,
}

impl GrepCandidateIndex for GatedCandidates {
    fn select_paths(&self, _: &str, _: bool) -> GrepCandidateSelection {
        let started = self.started.lock().unwrap().take();
        if let Some(started) = started {
            let _ = started.send(());
            if self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            {
                self.expired.store(true, Ordering::Release);
            }
        }
        GrepCandidateSelection::Unconstrained
    }
}

async fn gated_backend() -> (
    tempfile::TempDir,
    Arc<ManifestWorkspaceBackend>,
    tokio::sync::oneshot::Receiver<()>,
    mpsc::Sender<()>,
    Arc<AtomicBool>,
) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hit.rs"), "responsive_token\n").unwrap();
    let backend = ManifestWorkspaceBackend::new(root.path());
    let mut snapshots = backend.manifest().subscribe();
    tokio::time::timeout(Duration::from_secs(5), snapshots.recv())
        .await
        .unwrap()
        .unwrap();
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, receiver) = mpsc::channel();
    let expired = Arc::new(AtomicBool::new(false));
    backend
        .configure_grep_candidate_index(Arc::new(GatedCandidates {
            started: Mutex::new(Some(started)),
            release: Mutex::new(receiver),
            expired: Arc::clone(&expired),
        }))
        .unwrap();
    (root, backend, entered, release, expired)
}

fn request() -> WorkspaceGrepRequest {
    WorkspaceGrepRequest {
        base: WorkspacePath::root(),
        pattern: "responsive_token".into(),
        glob: None,
        context_lines: 0,
        case_insensitive: false,
        max_output_size: 1024,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn grep_keeps_file_reads_responsive_while_a_candidate_provider_is_busy() {
    let (_root, backend, entered, release, expired) = gated_backend().await;
    let search = Arc::clone(&backend);
    let query = tokio::spawn(async move { search.grep_with_sources(request()).await });
    entered.await.unwrap();
    let content = backend
        .read_text(&WorkspacePath::from_normalized("hit.rs"))
        .await
        .unwrap();
    assert_eq!(content, "responsive_token\n");
    assert!(
        !expired.load(Ordering::Acquire),
        "a busy candidate provider prevented another workspace read"
    );
    release.send(()).unwrap();
    let result = query.await.unwrap().unwrap();
    assert_eq!(result.result.match_count, 1);
    assert_eq!(
        result.matched_paths.unwrap(),
        vec![WorkspacePath::from_normalized("hit.rs")]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_search_deadline_does_not_wait_for_the_candidate_provider() {
    let (_root, backend, entered, release, expired) = gated_backend().await;
    let search = Arc::clone(&backend);
    let query = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_millis(50),
            search.grep_with_sources(request()),
        )
        .await
    });
    entered.await.unwrap();
    let result = query.await.unwrap();
    let responsive = !expired.load(Ordering::Acquire);
    let queued = tokio::time::timeout(
        Duration::from_millis(50),
        backend.grep_with_sources(request()),
    )
    .await;
    let _ = release.send(());
    assert!(
        responsive,
        "the search prevented its own deadline from running"
    );
    assert!(
        result.is_err(),
        "a deadline must not report a completed search"
    );
    assert!(
        queued.is_err(),
        "the cancelled worker released its slot before exiting"
    );
    let next = tokio::time::timeout(Duration::from_secs(5), backend.grep_with_sources(request()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.result.match_count, 1);
    assert!(!next.result.truncated);
}
