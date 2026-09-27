use std::fs::File;
use std::time::{Duration, Instant};

use fs2::FileExt;

use super::lock_file;
use crate::{LockError, LockOptions, SlotPolicy};

fn unguarded() -> LockOptions {
    LockOptions::new().with_slot(SlotPolicy::Unguarded)
}

#[test]
fn nowait_acquires_when_free_and_reports_contended_when_held() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("state.lock");

    let held = lock_file(&target, &unguarded()).unwrap();
    assert_eq!(target, held.path());
    let err = lock_file(&target, &unguarded()).unwrap_err();
    assert!(err.is_busy());
    assert_eq!(target, err.path());
    assert!(matches!(err, LockError::Contended { .. }), "{err:?}");
}

/// Un-migrated sites and external probes use `fs2`/raw `flock`; both directions must exclude.
#[test]
fn interop_with_fs2_flock() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("legacy.lock");

    let legacy = File::create(&target).unwrap();
    legacy.try_lock_exclusive().unwrap();
    let err = lock_file(&target, &unguarded()).unwrap_err();
    assert!(matches!(err, LockError::Contended { .. }), "{err:?}");
    drop(legacy);

    let held = lock_file(&target, &unguarded()).unwrap();
    let legacy = File::open(&target).unwrap();
    let err = legacy.try_lock_exclusive().unwrap_err();
    assert_eq!(fs2::lock_contended_error().kind(), err.kind());
    drop(held);
    legacy.try_lock_exclusive().unwrap();
}

#[test]
fn poll_times_out_with_elapsed() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("busy.lock");
    let _held = lock_file(&target, &unguarded()).unwrap();

    let timeout = Duration::from_millis(100);
    let started = Instant::now();
    let err = lock_file(
        &target,
        &unguarded().with_poll(timeout, Duration::from_millis(10)),
    )
    .unwrap_err();
    let elapsed = started.elapsed();
    match err {
        LockError::Timeout { path, waited } => {
            assert_eq!(target, path);
            assert!(waited >= timeout, "{waited:?}");
            assert!(waited <= elapsed, "{waited:?} > {elapsed:?}");
        }
        other => panic!("expected Timeout, got {other:?}"),
    }
}

#[test]
fn poll_acquires_after_release() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("state.lock");
    let held = lock_file(&target, &unguarded()).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        held.unlock().unwrap();
    });

    let acquired = lock_file(
        &target,
        &unguarded().with_poll(Duration::from_secs(5), Duration::from_millis(20)),
    )
    .unwrap();
    releaser.join().unwrap();
    assert_eq!(target, acquired.path());
}

/// An older holder's `Drop` unlinks the lock file while still holding its inode; a poller that kept one
/// descriptor open would wait on that anonymous inode forever.
#[cfg(unix)]
#[test]
fn poll_reopens_unlinked_recreated_path() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("leader.lock");
    let first = lock_file(&target, &unguarded()).unwrap();

    let unlink_target = target.clone();
    let unlinker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::remove_file(&unlink_target).unwrap();
        first
    });

    let second = lock_file(
        &target,
        &unguarded().with_poll(Duration::from_secs(5), Duration::from_millis(50)),
    )
    .unwrap();
    assert_eq!(target, second.path());
    let first = unlinker.join().unwrap();
    assert_eq!(target, first.path());
}

/// `flock(LOCK_UN)` releases the lock for every dup of the open file description, so a dup kept
/// past `unlock` does not keep the lock alive.
#[test]
fn unlock_releases_for_dup() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("state.lock");
    let held = lock_file(&target, &unguarded()).unwrap();
    let dup = held.try_clone().unwrap();

    held.unlock().unwrap();
    let reacquired = lock_file(&target, &unguarded()).unwrap();
    assert_eq!(target, reacquired.path());
    drop(dup);
}

/// Stand-in for a network filesystem stall: one attempt parks inside `open` while holding the
/// slot, and a second caller must fail fast without naming the target.
#[cfg(unix)]
#[test]
fn hung_open_wedges_only_one_caller() {
    use std::fs::{DirBuilder, OpenOptions};
    use std::os::unix::fs::DirBuilderExt;
    use std::path::Path;
    use std::sync::mpsc;

    use super::lock_file_with;

    let root = tempfile::tempdir().unwrap();
    // `tempfile::tempdir()` inherits the umask (0755), which a `GuardedIn` dir is refused for.
    let slot_dir = root.path().join("slots");
    DirBuilder::new().mode(0o700).create(&slot_dir).unwrap();
    let target = root.path().join("leader.lock");
    let guarded = |grace: Duration| {
        LockOptions::new().with_slot(SlotPolicy::GuardedIn {
            dir: slot_dir.clone(),
            grace,
        })
    };

    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let wedged_target = target.clone();
    let wedged_options = guarded(Duration::from_millis(200));
    let wedged = std::thread::spawn(move || {
        let mut open_fn = |path: &Path| {
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
        };
        lock_file_with(&wedged_target, &wedged_options, &mut open_fn)
    });

    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    let err = lock_file(&target, &guarded(Duration::from_millis(200))).unwrap_err();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    match err {
        LockError::AcquireInProgress { path, holder_pid } => {
            assert_eq!(target, path);
            assert_eq!(Some(std::process::id()), holder_pid);
        }
        other => panic!("expected AcquireInProgress, got {other:?}"),
    }
    assert!(!target.exists(), "target must not be opened or created");
    assert!(!wedged.is_finished());

    release_tx.send(()).unwrap();
    let held = wedged.join().unwrap().unwrap();
    assert_eq!(target, held.path());
}
