use std::path::Path;

use super::{fnv1a64, slot_path_in};

#[test]
fn fnv1a64_matches_reference_vectors() {
    assert_eq!(0xcbf2_9ce4_8422_2325, fnv1a64(b""));
    assert_eq!(0xaf63_dc4c_8601_ec8c, fnv1a64(b"a"));
}

#[test]
fn slot_path_in_sanitizes_and_truncates_hint() {
    let target = Path::new("/tmp").join(format!("my lock$file-{}.lock", "a".repeat(100)));
    let slot = slot_path_in(Path::new("/s"), &target);
    let name = slot.file_name().unwrap().to_str().unwrap();
    let (hint, hash) = name
        .strip_suffix(".slot")
        .unwrap()
        .rsplit_once('.')
        .unwrap();
    assert_eq!(48, hint.len());
    assert!(hint.starts_with("my_lock_file-a"), "{hint}");
    assert!(
        hint.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "{hint}"
    );
    assert_eq!(16, hash.len());
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
}

#[test]
fn slot_path_in_is_pure_and_stable_across_spellings() {
    let dir = Path::new("/nonexistent/slot-dir");
    let canonical = slot_path_in(dir, Path::new("/a/b/c.lock"));
    assert_eq!(canonical, slot_path_in(dir, Path::new("/a//b/./c.lock")));
    assert_eq!(canonical, slot_path_in(dir, Path::new("/a/b/c.lock/")));
    assert_ne!(canonical, slot_path_in(dir, Path::new("/a/b/d.lock")));
    assert_ne!(canonical, slot_path_in(dir, Path::new("/a/c.lock")));

    assert_eq!(Some(dir), canonical.parent());
    let name = canonical.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("c.lock."), "{name}");
    assert!(name.ends_with(".slot"), "{name}");
}

#[cfg(unix)]
mod unix {
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use crate::slot::slot_path_in;
    use crate::slot::unix::{LooseMode, resolve_slot_dir, verify_slot_dir};
    use crate::{LockError, LockOptions, SlotPolicy, lock_file};

    fn guarded_in(dir: &Path, grace: Duration) -> LockOptions {
        LockOptions::new().with_slot(SlotPolicy::GuardedIn {
            dir: dir.to_path_buf(),
            grace,
        })
    }

    /// `tempfile::tempdir()` inherits the umask (0755), which a `GuardedIn` dir is refused for.
    fn slot_dir_in(root: &Path) -> PathBuf {
        let dir = root.join("slots");
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        dir
    }

    fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn held_slot_yields_acquire_in_progress_with_holder_pid_and_untouched_target() {
        let root = tempfile::tempdir().unwrap();
        let slot_dir = slot_dir_in(root.path());
        let target = root.path().join("leader.lock");
        let slot_path = slot_path_in(&slot_dir, &target);
        let mut holder = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&slot_path)
            .unwrap();
        holder.write_all(b"424242\n").unwrap();
        holder.try_lock().unwrap();

        let err =
            lock_file(&target, &guarded_in(&slot_dir, Duration::from_millis(50))).unwrap_err();
        match err {
            LockError::AcquireInProgress { path, holder_pid } => {
                assert_eq!(target, path);
                assert_eq!(Some(424_242), holder_pid);
            }
            other => panic!("expected AcquireInProgress, got {other:?}"),
        }
        assert!(!target.exists(), "target must not be opened or created");
    }

    #[test]
    fn slot_is_released_after_success_and_after_contention() {
        let root = tempfile::tempdir().unwrap();
        let slot_dir = slot_dir_in(root.path());
        let target = root.path().join("state.lock");
        let slot_path = slot_path_in(&slot_dir, &target);
        let options = guarded_in(&slot_dir, Duration::from_millis(100));
        // Dropping the probe releases the flock it just took.
        let assert_slot_free = || File::open(&slot_path).unwrap().try_lock().unwrap();

        let held = lock_file(&target, &options).unwrap();
        assert_slot_free();

        // The competing holder is `held`, in this thread, so the second call is contended for sure.
        let err = lock_file(&target, &options).unwrap_err();
        assert!(matches!(err, LockError::Contended { .. }), "{err:?}");
        assert_slot_free();
        drop(held);
    }

    #[test]
    fn guarded_in_creates_dir_0700() {
        let root = tempfile::tempdir().unwrap();
        let slot_dir = root.path().join("slots");
        let target = root.path().join("state.lock");

        let _held = lock_file(&target, &guarded_in(&slot_dir, Duration::from_millis(100))).unwrap();
        assert_eq!(0o700, mode_of(&slot_dir));
        assert!(slot_path_in(&slot_dir, &target).is_file());
    }

    /// A caller-supplied directory may be shared on purpose: a loose mode is refused, never
    /// tightened, and the lock is acquired without a slot (fail-open).
    #[test]
    fn guarded_in_refuses_loose_dir_mode() {
        let root = tempfile::tempdir().unwrap();
        let slot_dir = root.path().join("slots");
        fs::create_dir(&slot_dir).unwrap();
        fs::set_permissions(&slot_dir, fs::Permissions::from_mode(0o755)).unwrap();

        let target = root.path().join("state.lock");
        let held = lock_file(&target, &guarded_in(&slot_dir, Duration::from_millis(100))).unwrap();
        assert_eq!(target, held.path());
        assert_eq!(0o755, mode_of(&slot_dir));
        assert!(fs::read_dir(&slot_dir).unwrap().next().is_none());
    }

    /// The default directory is ours by construction, so a loose mode is tightened instead.
    #[test]
    fn verify_slot_dir_tightens_loose_default_mode() {
        let root = tempfile::tempdir().unwrap();
        let slot_dir = root.path().join("slots");
        fs::create_dir(&slot_dir).unwrap();
        fs::set_permissions(&slot_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let euid = fs::metadata(&slot_dir).unwrap().uid();

        let err = verify_slot_dir(&slot_dir, euid, LooseMode::Refuse).unwrap_err();
        assert!(err.to_string().contains("admits other users"), "{err}");
        verify_slot_dir(&slot_dir, euid, LooseMode::Tighten).unwrap();
        assert_eq!(0o700, mode_of(&slot_dir));
    }

    /// A symlinked slot dir is refused; the lock is still acquired, just without a slot (fail-open).
    #[test]
    fn guarded_in_rejects_symlink_dir() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let target = root.path().join("state.lock");
        let held = lock_file(&target, &guarded_in(&link, Duration::from_millis(100))).unwrap();
        assert_eq!(target, held.path());
        assert!(fs::read_dir(&real).unwrap().next().is_none());
    }

    #[test]
    fn slot_content_names_pid_and_target() {
        let root = tempfile::tempdir().unwrap();
        let slot_dir = slot_dir_in(root.path());
        let target = root.path().join("state.lock");

        let _held = lock_file(&target, &guarded_in(&slot_dir, Duration::from_millis(100))).unwrap();
        let stamp = fs::read_to_string(slot_path_in(&slot_dir, &target)).unwrap();
        assert_eq!(
            format!("{}\n{}\n", std::process::id(), target.display()),
            stamp
        );
    }

    #[test]
    fn resolve_slot_dir_prefers_override_then_tmp_euid() {
        let fallback = (
            PathBuf::from("/tmp/grok-file-lock-1000"),
            LooseMode::Tighten,
        );
        assert_eq!(
            (PathBuf::from("/opt/slots"), LooseMode::Refuse),
            resolve_slot_dir(Some(Path::new("/opt/slots")), 1000)
        );
        assert_eq!(
            fallback,
            resolve_slot_dir(Some(Path::new("relative/slots")), 1000)
        );
        assert_eq!(fallback, resolve_slot_dir(None, 1000));
    }
}
