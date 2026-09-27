use super::*;
fn counted(size: DirSize) -> u64 {
    size.measure.bytes().expect("measured on its own volume")
}
#[cfg(unix)]
#[test]
fn sizing_is_physical_not_logical() {
    let tmp = tempfile::TempDir::new().unwrap();
    const LOGICAL: u64 = 1 << 20;
    let sparse = tmp.path().join("sparse.bin");
    std::fs::File::create(&sparse)
        .unwrap()
        .set_len(LOGICAL)
        .unwrap();
    let sparse_meta = std::fs::symlink_metadata(&sparse).unwrap();
    assert_eq!(sparse_meta.len(), LOGICAL);
    assert!(
        physical_file_size(&sparse_meta) < LOGICAL,
        "a hole must cost fewer blocks than its logical length"
    );
    let linked = tmp.path().join("linked");
    std::fs::create_dir(&linked).unwrap();
    let original = linked.join("a.bin");
    std::fs::write(&original, vec![b'x'; 8192]).unwrap();
    std::fs::hard_link(&original, linked.join("b.bin")).unwrap();
    let one_link = physical_file_size(&std::fs::symlink_metadata(&original).unwrap());
    assert_eq!(
        counted(physical_dir_size(&linked, Volume::of(&linked))),
        one_link * 2,
        "each hard link to an inode counts at full size, as clones do"
    );
}
#[test]
fn physical_dir_size_sums_files_without_following_symlinks() {
    let tmp = tempfile::TempDir::new().unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir_all(target.join("sub")).unwrap();
    std::fs::write(target.join("a.bin"), vec![b'x'; 8192]).unwrap();
    std::fs::write(target.join("sub/b.bin"), vec![b'y'; 4096]).unwrap();
    let full = counted(physical_dir_size(&target, Volume::of(&target)));
    let expected: u64 = [target.join("a.bin"), target.join("sub/b.bin")]
        .iter()
        .map(|p| physical_file_size(&std::fs::symlink_metadata(p).unwrap()))
        .sum();
    assert_eq!(full, expected);
    #[cfg(unix)]
    {
        let linked = tmp.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&target, linked.join("escape")).unwrap();
        let link_only = counted(physical_dir_size(&linked, Volume::of(&linked)));
        let link_meta = std::fs::symlink_metadata(linked.join("escape")).unwrap();
        assert_eq!(
            link_only,
            physical_file_size(&link_meta),
            "a walk must cost exactly the symlink's own inode, never the target"
        );
    }
}
#[test]
fn missing_root_counts_one_unreadable_dir() {
    let missing = Path::new("/nonexistent-grok-du-root");
    let size = physical_dir_size(missing, Volume::of(missing));
    assert_eq!(size.measure.bytes(), Some(0));
    assert_eq!(size.issues.unreadable_dirs, 1);
    assert_eq!(size.issues.skipped(), 1);
}
#[test]
fn a_volume_holds_only_its_own_device() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path();
    assert!(Volume(None).holds(path), "unknown anchor excludes nothing");
    assert!(
        Volume::of(path).holds(Path::new("/nonexistent-grok-du-path")),
        "unknown entry device is not a proven crossing"
    );
    assert!(Volume::of(path).holds(path), "same device");
    #[cfg(unix)]
    {
        let elsewhere = Volume::of(path).other_device_for_test();
        assert!(!elsewhere.holds(path), "a different device is a crossing");
    }
}
#[cfg(unix)]
#[test]
fn a_root_off_the_anchor_is_measured_by_nobody() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("worktrees");
    let worktree = root.join("a3s/wt-a");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("payload.bin"), vec![b'x'; 65536]).unwrap();
    let elsewhere = Volume::of(tmp.path()).other_device_for_test();
    let home = physical_buckets(&root, Volume::of(&root));
    assert!(home.total.bytes().is_some_and(|bytes| bytes >= 65536));
    assert!(
        home.buckets
            .get(&worktree)
            .is_some_and(|m| m.bytes().is_some_and(|b| b >= 65536)),
        "on its own volume the worktree is a counted bucket"
    );
    let foreign = physical_buckets(&root, elsewhere);
    assert_eq!(foreign.total, Measure::Elsewhere, "no total exists for it");
    assert_eq!(foreign.total.bytes(), None);
    assert_eq!(foreign.issues.other_filesystems, 1);
    assert_eq!(
        physical_dir_size(&worktree, elsewhere).measure,
        Measure::Elsewhere,
        "the direct sizing route answers the same way, so no caller can differ"
    );
}
#[cfg(target_os = "linux")]
struct TmpfsMount {
    path: std::ffi::CString,
}
#[cfg(target_os = "linux")]
impl TmpfsMount {
    fn try_new(path: &Path) -> std::io::Result<Self> {
        let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let source = std::ffi::CString::new("tmpfs").unwrap();
        let fstype = std::ffi::CString::new("tmpfs").unwrap();
        let data = std::ffi::CString::new("size=1m,mode=0700").unwrap();
        let rc = unsafe {
            libc::mount(
                source.as_ptr(),
                path.as_ptr(),
                fstype.as_ptr(),
                0,
                data.as_ptr().cast(),
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { path })
    }
}
#[cfg(target_os = "linux")]
impl Drop for TmpfsMount {
    fn drop(&mut self) {
        let rc = unsafe { libc::umount(self.path.as_ptr()) };
        assert_eq!(
            0,
            rc,
            "tmpfs umount failed: {}",
            std::io::Error::last_os_error()
        );
    }
}
#[cfg(target_os = "linux")]
#[test]
fn later_sibling_is_visited_after_another_filesystem() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("worktrees");
    let other = root.join("other-fs");
    std::fs::create_dir_all(&other).unwrap();
    let _mount = match TmpfsMount::try_new(&other) {
        Ok(mount) => mount,
        Err(error) if matches!(error.raw_os_error(), Some(libc::EPERM | libc::EACCES)) => {
            return;
        }
        Err(error) => panic!("tmpfs mount failed: {error}"),
    };
    std::fs::write(other.join("hidden.bin"), vec![b'h'; 8192]).unwrap();
    assert_ne!(
        device_id(&root),
        device_id(&other),
        "tmpfs must be a different device"
    );
    let mut later = None;
    for i in 0..32 {
        let dir = root.join(format!("sib-{i}"));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("keep.bin"), vec![b'k'; 4096]).unwrap();
        let names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let other_at = names.iter().position(|name| name == "other-fs").unwrap();
        if names
            .iter()
            .skip(other_at + 1)
            .any(|name| name == dir.file_name().unwrap())
        {
            later = Some(dir);
            break;
        }
    }
    let later = later.expect("a sibling must be listed after the other filesystem");
    let same_fs_files: Vec<_> = (0..32)
        .map(|i| root.join(format!("sib-{i}/keep.bin")))
        .filter(|path| path.is_file())
        .collect();
    let expected: u64 = same_fs_files
        .iter()
        .map(|path| physical_file_size(&std::fs::symlink_metadata(path).unwrap()))
        .sum();
    let later_bytes =
        physical_file_size(&std::fs::symlink_metadata(later.join("keep.bin")).unwrap());
    let size = physical_dir_size(&root, Volume::of(&root));
    assert!(later_bytes > 0);
    assert_eq!(expected, counted(size));
    assert_eq!(1, size.issues.other_filesystems);
}
#[cfg(unix)]
#[test]
fn volume_bytes_reports_a_real_volume() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (capacity, available) = volume_bytes(tmp.path()).expect("statfs on a tempdir");
    assert!(capacity > 0, "capacity must be positive");
    assert!(
        available <= capacity,
        "available {available} exceeds capacity {capacity}"
    );
    let cpath = std::ffi::CString::new(tmp.path().to_string_lossy().as_bytes()).unwrap();
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::statvfs(cpath.as_ptr(), &mut st) }, 0);
    fn widen(v: impl TryInto<u64>) -> Option<u64> {
        v.try_into().ok()
    }
    let block = widen(st.f_frsize)
        .filter(|&size| size != 0)
        .or_else(|| widen(st.f_bsize))
        .unwrap();
    assert_eq!(
        capacity,
        block * widen(st.f_blocks).unwrap(),
        "statfs capacity must match statvfs blocks times the fundamental block size"
    );
}
