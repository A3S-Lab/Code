use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::disk_usage_cmd::{DiskUsageReport, SCHEMA_VERSION};
use crate::fs_size::{Volume, physical_dir_size, physical_file_size};

#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    kind: &'static str,
    ino: u64,
    size: u64,
    mtime: i64,
    ctime: i64,
}

type TreeSnapshot = BTreeMap<PathBuf, TreeEntry>;

#[cfg(unix)]
fn snapshot(root: &Path) -> TreeSnapshot {
    use std::os::unix::fs::MetadataExt;

    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            let kind = if metadata.is_dir() {
                "dir"
            } else if metadata.is_symlink() {
                "symlink"
            } else {
                "file"
            };
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                TreeEntry {
                    kind,
                    ino: metadata.ino(),
                    size: metadata.size(),
                    mtime: metadata.mtime(),
                    ctime: metadata.ctime(),
                },
            )
        })
        .collect()
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().join("grove");
    let live = grove::prepare_escape_dest_root(&data_dir, "live").unwrap();
    let orphan = grove::prepare_escape_dest_root(&data_dir, "orphan").unwrap();
    let unattributed = data_dir.join(grove::ESCAPE_DESTS_DIR).join("foreign");
    fs::write(live.join("live.bin"), vec![b'l'; 8192]).unwrap();
    fs::write(orphan.join("orphan.bin"), vec![b'o'; 4096]).unwrap();
    fs::create_dir_all(&unattributed).unwrap();
    fs::write(unattributed.join("foreign.bin"), vec![b'f'; 2048]).unwrap();

    let mut registry = grove::MountRegistry::empty(data_dir.join(grove::MOUNTS_FILE));
    registry.upsert(grove::MountRecord {
        store_id: Some("live".to_owned()),
        ..grove::MountRecord::new(data_dir.join("repos/live"), tmp.path().join("mount/live"))
    });
    registry.save().unwrap();
    (tmp, data_dir)
}

#[test]
fn du_reports_redirections_orphaned_and_unattributed_rows_schema_v2_fixture() {
    let (_tmp, data_dir) = fixture();
    let usage = collect_redirect_usage_in(std::slice::from_ref(&data_dir));
    assert_eq!(0, usage.issues.unreadable_dirs);
    let report = DiskUsageReport {
        grok_home: "/tmp/grok-home".to_owned(),
        total_bytes: 17,
        redirections_bytes: usage.redirections_bytes,
        orphaned_redirections: usage.orphaned_redirections,
        unattributed_redirect_dirs: usage.unattributed_redirect_dirs,
        ..DiskUsageReport::default()
    };
    let json = serde_json::to_value(report).unwrap();

    assert_eq!(
        json.get("schema_version"),
        Some(&serde_json::json!(SCHEMA_VERSION))
    );
    assert_eq!(json.get("total_bytes"), Some(&serde_json::json!(17)));
    assert_eq!(
        json.get("redirections_bytes"),
        Some(&serde_json::json!(
            physical_dir_size(
                &data_dir.join(grove::ESCAPE_DESTS_DIR).join("live"),
                Volume::of(&data_dir.join(grove::ESCAPE_DESTS_DIR).join("live"))
            )
            .measure
            .bytes()
            .unwrap()
        ))
    );
    assert_eq!(
        json.get("orphaned_redirections"),
        Some(&serde_json::json!([{
            "id": "orphan",
            "bytes": physical_dir_size(
                &data_dir.join(grove::ESCAPE_DESTS_DIR).join("orphan"),
                Volume::of(&data_dir.join(grove::ESCAPE_DESTS_DIR).join("orphan"))
            ).measure.bytes().unwrap()
        }]))
    );
    assert_eq!(
        json.get("unattributed_redirect_dirs"),
        Some(&serde_json::json!([{
            "path": data_dir.join(grove::ESCAPE_DESTS_DIR).join("foreign"),
            "bytes": physical_dir_size(
                &data_dir.join(grove::ESCAPE_DESTS_DIR).join("foreign"),
                Volume::of(&data_dir.join(grove::ESCAPE_DESTS_DIR).join("foreign"))
            ).measure.bytes().unwrap()
        }]))
    );
}

#[cfg(unix)]
#[test]
fn du_rows_are_read_only_and_delete_nothing() {
    let (_tmp, data_dir) = fixture();
    let before = snapshot(&data_dir);

    let usage = collect_redirect_usage_in(std::slice::from_ref(&data_dir));

    assert!(usage.redirections_bytes > 0);
    assert_eq!(before, snapshot(&data_dir));
}

#[test]
fn du_clean_orphaned_deletes_only_ownership_proven_orphans() {
    let (_tmp, data_dir) = fixture();
    let entries = grove::scan_escape_dests(&data_dir);
    for entry in entries {
        if let Some(orphan) = entry.into_proven_orphan() {
            grove::delete_orphan_jail(&data_dir, &orphan).unwrap();
        }
    }

    assert!(
        !data_dir
            .join(grove::ESCAPE_DESTS_DIR)
            .join("orphan")
            .exists()
    );
    assert!(data_dir.join(grove::ESCAPE_DESTS_DIR).join("live").is_dir());
    assert!(
        data_dir
            .join(grove::ESCAPE_DESTS_DIR)
            .join("foreign/foreign.bin")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn du_clean_never_touches_unattributed_dirs() {
    let (_tmp, data_dir) = fixture();
    let foreign = data_dir.join(grove::ESCAPE_DESTS_DIR).join("foreign");
    let before = snapshot(&foreign);
    for entry in grove::scan_escape_dests(&data_dir) {
        if let Some(orphan) = entry.into_proven_orphan() {
            grove::delete_orphan_jail(&data_dir, &orphan).unwrap();
        }
    }
    assert_eq!(before, snapshot(&foreign));
}

#[cfg(unix)]
#[test]
fn du_clean_and_clean_orphaned_never_delete_conflict_class_images() {
    let (_tmp, data_dir) = fixture();
    let live = data_dir.join(grove::ESCAPE_DESTS_DIR).join("live");
    let images = live.join(grove_git::IMAGES_DIR);
    fs::create_dir_all(&images).unwrap();
    fs::write(images.join("missing-marker.sparseimage"), b"missing").unwrap();
    fs::write(images.join("malformed.sparseimage"), b"malformed").unwrap();
    fs::write(images.join(".grove-image.json"), b"not json").unwrap();
    let before = snapshot(&images);

    let dest = data_dir.parent().unwrap().join("mount/live");
    fs::create_dir_all(data_dir.join(grove_git::WORKTREE_BACKING_DIR).join("live")).unwrap();
    grove::write_backing_marker(
        &data_dir,
        "live",
        &grove::BackingMarker {
            schema: 1,
            worktree_id: "live".to_owned(),
            dest: dest.clone(),
            source_repo: data_dir.join("repos/live"),
            pin_ref: "refs/grok/worktrees/live".to_owned(),
            mount_id: 1,
            created_at: 1,
        },
    )
    .unwrap();
    grove::clean_artifacts(&data_dir, "live", &dest, None).unwrap();

    assert_eq!(before, snapshot(&images));
}

#[cfg(unix)]
#[test]
fn du_clean_through_attached_image_preserves_marker_and_metadata() {
    let (_tmp, data_dir) = fixture();
    let live = data_dir.join(grove::ESCAPE_DESTS_DIR).join("live");
    let images = live.join(grove_git::IMAGES_DIR);
    fs::create_dir_all(&images).unwrap();
    for name in [
        ".grove-image.json",
        ".metadata_never_index",
        ".fseventsd",
        ".Trashes",
    ] {
        fs::write(images.join(name), name.as_bytes()).unwrap();
    }
    let before = snapshot(&images);
    let dest = data_dir.parent().unwrap().join("mount/live");
    fs::create_dir_all(data_dir.join(grove_git::WORKTREE_BACKING_DIR).join("live")).unwrap();
    grove::write_backing_marker(
        &data_dir,
        "live",
        &grove::BackingMarker {
            schema: 1,
            worktree_id: "live".to_owned(),
            dest: dest.clone(),
            source_repo: data_dir.join("repos/live"),
            pin_ref: "refs/grok/worktrees/live".to_owned(),
            mount_id: 1,
            created_at: 1,
        },
    )
    .unwrap();
    grove::clean_artifacts(&data_dir, "live", &dest, None).unwrap();

    assert_eq!(before, snapshot(&images));
    assert!(live.join(grove_git::ESCAPE_ROOT_MARKER).is_file());
}

#[test]
fn du_clean_purges_live_redirect_children_not_materialized_files() {
    let (tmp, data_dir) = fixture();
    let live = data_dir.join(grove::ESCAPE_DESTS_DIR).join("live");
    let dest = tmp.path().join("mount/live");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("materialized.txt"), b"keep").unwrap();
    fs::write(dest.join("ignored.bin"), b"keep").unwrap();
    fs::create_dir_all(data_dir.join(grove_git::WORKTREE_BACKING_DIR).join("live")).unwrap();
    grove::write_backing_marker(
        &data_dir,
        "live",
        &grove::BackingMarker {
            schema: 1,
            worktree_id: "live".to_owned(),
            dest: dest.clone(),
            source_repo: data_dir.join("repos/live"),
            pin_ref: "refs/grok/worktrees/live".to_owned(),
            mount_id: 1,
            created_at: 1,
        },
    )
    .unwrap();

    grove::clean_artifacts(&data_dir, "live", &dest, None).unwrap();

    assert!(!live.join("live.bin").exists());
    assert!(live.is_dir());
    assert!(dest.join("materialized.txt").is_file());
    assert!(dest.join("ignored.bin").is_file());
}

#[test]
fn redirections_bytes_include_images() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().join("grove");
    let jail = data_dir.join(grove::ESCAPE_DESTS_DIR).join("wt-image");
    fs::create_dir_all(jail.join(".grove-images/cache")).unwrap();
    let image = jail.join(".grove-images/cache/build.sparseimage");
    let regular = jail.join("artifact.bin");
    fs::write(&regular, vec![b'a'; 8192]).unwrap();
    let file = fs::File::create(&image).unwrap();
    file.set_len(1 << 20).unwrap();
    drop(file);
    fs::write(&image, vec![b'i'; 4096]).unwrap();

    let bytes = redirections_bytes_in(&[data_dir], &["wt-image".to_owned()]);
    let expected = physical_file_size(&fs::symlink_metadata(&regular).unwrap())
        + physical_file_size(&fs::symlink_metadata(&image).unwrap());

    assert_eq!(expected, bytes);
}

#[cfg(unix)]
#[test]
fn symlink_jail_is_not_sized_or_listed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().join("grove");
    let real = tmp.path().join("elsewhere");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("big.bin"), vec![b'x'; 8192]).unwrap();
    let dests = data_dir.join(grove::ESCAPE_DESTS_DIR);
    fs::create_dir_all(&dests).unwrap();
    std::os::unix::fs::symlink(&real, dests.join("linked")).unwrap();

    let usage = collect_redirect_usage_in(std::slice::from_ref(&data_dir));

    assert_eq!(0, usage.redirections_bytes);
    assert!(usage.orphaned_redirections.is_empty());
    assert!(usage.unattributed_redirect_dirs.is_empty());
    assert_eq!(1, usage.unfollowed_dir_symlinks);
    assert_eq!(
        0,
        redirections_bytes_in(&[data_dir], &["linked".to_owned()])
    );
}

#[test]
fn missing_jail_root_keeps_the_unreadable_count() {
    let missing = Path::new("/nonexistent-grok-du-jail");
    let size = crate::fs_size::physical_jail_size(missing);
    let mut usage = RedirectUsage::default();
    usage.issues.merge(size.issues);
    assert_eq!(1, usage.issues.unreadable_dirs);
    assert_eq!(1, usage.issues.skipped());
}

#[test]
fn missing_escape_dests_does_not_increment_unreadable() {
    let tmp = tempfile::TempDir::new().unwrap();
    let missing_data = tmp.path().join("missing-data");
    let data_dir = tmp.path().join("grove");
    fs::create_dir_all(&data_dir).unwrap();

    let usage = collect_redirect_usage_in(&[missing_data, data_dir]);

    assert_eq!(0, usage.issues.unreadable_dirs);
    assert_eq!(0, usage.redirections_bytes);
    assert!(usage.orphaned_redirections.is_empty());
    assert!(usage.unattributed_redirect_dirs.is_empty());
}

#[test]
fn escape_dests_file_increments_unreadable_and_adds_no_jail_rows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().join("grove");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join(grove::ESCAPE_DESTS_DIR), b"not-a-directory").unwrap();

    let usage = collect_redirect_usage_in(std::slice::from_ref(&data_dir));

    assert_eq!(1, usage.issues.unreadable_dirs);
    assert_eq!(0, usage.redirections_bytes);
    assert!(usage.orphaned_redirections.is_empty());
    assert!(usage.unattributed_redirect_dirs.is_empty());
    assert_eq!(0, usage.unfollowed_dir_symlinks);
}

fn record_with_grove(
    creation_mode: &str,
    grove: serde_json::Value,
) -> a3s_fast_worktree::WorktreeRecord {
    let mut record = crate::test_util::make_worktree_record("wt", Path::new("/no/such"), "label");
    record.creation_mode = creation_mode.to_owned();
    record.metadata = Some(serde_json::json!({ "grove": grove }));
    record
}

#[test]
fn grove_projfs_creation_mode_uses_projfs_jail_id_without_transport() {
    let record = record_with_grove("grove-projfs", serde_json::json!({ "mount_id": 7 }));
    assert_eq!(
        vec![
            grove::mounts::projfs_escape_jail_id("wt", 7),
            "wt".to_owned()
        ],
        jail_ids_for(&record),
    );
}

#[test]
fn projfs_transport_uses_projfs_jail_id() {
    let record = record_with_grove(
        "linked",
        serde_json::json!({ "transport": "projfs", "mount_id": 7 }),
    );
    assert_eq!(
        vec![
            grove::mounts::projfs_escape_jail_id("wt", 7),
            "wt".to_owned()
        ],
        jail_ids_for(&record),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn show_redirection_bytes_match_sync_walk() {
    let record = crate::test_util::make_worktree_record("wt", Path::new("/no/such"), "label");
    let runtime_thread = std::thread::current().id();
    let expected = redirections_bytes_for(&record);
    let actual = redirections_bytes_for_show(&record).await.unwrap();
    let worker = SHOW_WALK_THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    assert_eq!(expected, actual);
    assert_ne!(Some(runtime_thread), worker);
}
