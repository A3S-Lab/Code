use std::path::{Path, PathBuf};

use grove_git::{BACKING_MARKER_FILE, BackingMarker, ESCAPE_DESTS_DIR, WORKTREE_BACKING_DIR};
use tempfile::TempDir;

use super::{
    JailLayout, MarkerHit, PathFlavor, dest_key, local_clean_artifacts_in, resolve_marker,
};
use crate::confined::tests::plant_journal;

fn marker(id: &str, dest: &Path, mount_id: i64) -> BackingMarker {
    BackingMarker {
        schema: 1,
        worktree_id: id.to_owned(),
        dest: dest.to_path_buf(),
        source_repo: PathBuf::from("repo"),
        pin_ref: format!("refs/grok/worktrees/{id}"),
        mount_id,
        created_at: 1,
    }
}

/// backing directory is returned so a journal row can name its identity.
fn plant_marker(data: &Path, dirent: &str, marker: &BackingMarker) -> PathBuf {
    let backing = data.join(WORKTREE_BACKING_DIR).join(dirent);
    std::fs::create_dir_all(&backing).unwrap();
    std::fs::write(
        backing.join(BACKING_MARKER_FILE),
        serde_json::to_vec(marker).unwrap(),
    )
    .unwrap();
    backing
}

#[test]
fn windows_dest_key_folds_verbatim_prefix_separators_case_and_dots() {
    let key = |p: &str| dest_key(p, PathFlavor::Windows);
    let want = r"c:\users\u\wt";
    for spelling in [
        r"C:\Users\u\wt",
        r"c:\users\U\WT\",
        "C:/Users/u/wt",
        r"\\?\C:\Users\u\wt",
        r"C:\Users\u\.\build\..\wt",
        r"\\?\C:\Users\..\Users\u\wt",
    ] {
        assert_eq!(want, key(spelling), "{spelling}");
    }
    assert_eq!(r"c:\", key(r"C:\"), "the root keeps its separator");
    assert_eq!(r"c:\", key(r"C:\..\..\"), ".. never climbs past the drive");
    assert_eq!(r"\\srv\share\wt", key(r"\\?\UNC\srv\share\wt"));
    assert_eq!(r"\\srv\share\wt", key(r"\\srv\share\.\wt\"));
    assert_ne!(key(r"C:\Users\u\wt"), key(r"D:\Users\u\wt"));
}

#[test]
fn unix_dest_key_collapses_dots_and_trailing_separators_and_keeps_case() {
    let key = |p: &str| dest_key(p, PathFlavor::Unix);
    for spelling in [
        "/home/u/wt",
        "/home/u/wt/",
        "/home/u/./wt",
        "/home/u/x/../wt",
    ] {
        assert_eq!("/home/u/wt", key(spelling), "{spelling}");
    }
    assert_eq!("/", key("/"));
    assert_eq!("/", key("/.."), ".. never climbs past the root");
    assert_eq!("/wt", key("//wt"));
    assert_ne!(key("/home/u/wt"), key("/home/u/WT"));
}

#[test]
fn darwin_dest_key_rewrites_firmlinks_to_private_and_leaves_lookalikes() {
    let key = |p: &str| dest_key(p, PathFlavor::Darwin);
    assert_eq!("/private/tmp/wt", key("/tmp/wt"));
    assert_eq!("/private/tmp/wt", key("/private/tmp/wt"));
    assert_eq!("/private/var/folders/x", key("/var/folders/x/"));
    assert_eq!("/private/etc", key("/etc"));
    assert_eq!("/tmpfoo/wt", key("/tmpfoo/wt"));
    assert_eq!("/Users/u/wt", key("/Users/u/wt"));
}

#[test]
fn jail_id_is_the_worktree_id_per_worktree_and_suffixed_by_the_mount_id_per_mount() {
    let hit = MarkerHit {
        data_dir: PathBuf::from("data"),
        worktree_id: "wt-1".to_owned(),
        mount_id: 7,
    };
    assert_eq!("wt-1", hit.jail_id(JailLayout::PerWorktree));
    assert_eq!("wt-1-7", hit.jail_id(JailLayout::PerMount));
}

#[test]
fn resolve_marker_binds_the_dirent_and_skips_planted_unsafe_and_foreign_markers() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("grove");
    let other = tmp.path().join("other");
    let dest = tmp.path().join("wt");
    plant_marker(&data, "wt-real", &marker("wt-real", &dest, 7));
    // A marker whose own field names a different id than its dirent.
    plant_marker(&data, "wt-decoy", &marker("wt-real", &dest, 9));
    // An unconfined dirent is never an id, however its marker reads.
    plant_marker(&data, ".hidden", &marker(".hidden", &dest, 9));
    // Another worktree in another data dir.
    plant_marker(
        &other,
        "wt-other",
        &marker("wt-other", &tmp.path().join("elsewhere"), 3),
    );

    let hit = resolve_marker(&dest, &[other.clone(), data.clone()]).unwrap();
    assert_eq!(
        MarkerHit {
            data_dir: data.clone(),
            worktree_id: "wt-real".to_owned(),
            mount_id: 7,
        },
        hit
    );
    let spelled = dest.join(".").join("sub").join("..");
    assert_eq!(
        hit,
        resolve_marker(&spelled, std::slice::from_ref(&data)).unwrap()
    );

    let missing =
        resolve_marker(&tmp.path().join("nowhere"), std::slice::from_ref(&data)).unwrap_err();
    assert!(
        missing.to_string().contains("no backing marker"),
        "{missing}"
    );
    plant_marker(&other, "wt-twin", &marker("wt-twin", &dest, 8));
    let twin = resolve_marker(&dest, &[data, other]).unwrap_err();
    assert!(twin.to_string().contains("ambiguous"), "{twin}");
}

#[cfg(unix)]
#[test]
fn resolve_marker_does_not_follow_a_symlinked_backing_slot() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("grove");
    let dest = tmp.path().join("wt");
    let real = plant_marker(
        &tmp.path().join("victim"),
        "wt-1",
        &marker("wt-1", &dest, 1),
    );
    std::fs::create_dir_all(data.join(WORKTREE_BACKING_DIR)).unwrap();
    std::os::unix::fs::symlink(&real, data.join(WORKTREE_BACKING_DIR).join("wt-1")).unwrap();

    let err = resolve_marker(&dest, &[data]).unwrap_err();
    assert!(err.to_string().contains("no backing marker"), "{err}");
}

#[test]
fn local_clean_without_a_jail_is_a_noop() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("grove");
    let dest = tmp.path().join("wt");
    std::fs::create_dir(&dest).unwrap();
    let backing = plant_marker(&data, "wt-art", &marker("wt-art", &dest, 1));
    plant_journal(&data, "wt-art", &backing, None);

    let report =
        local_clean_artifacts_in(&dest, std::slice::from_ref(&data), JailLayout::PerWorktree)
            .unwrap();
    assert!(report.no_escapes);
    assert_eq!(0, report.purged_entries);
}

/// The ProjFS layout: the jail is `<id>-<mount_id>` and its identity is
/// journaled under that id. The per-worktree lookup finds no jail there and
/// purges nothing; the per-mount lookup empties it and keeps the jail and
/// its ownership marker.
#[test]
fn local_clean_per_mount_purges_the_suffixed_jail_the_journal_proves() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("grove");
    let dest = tmp.path().join("wt");
    std::fs::create_dir(&dest).unwrap();
    let backing = plant_marker(&data, "wt-p", &marker("wt-p", &dest, 7));
    let jail = grove::prepare_escape_dest_root(&data, "wt-p-7").unwrap();
    std::fs::create_dir_all(jail.join("target")).unwrap();
    std::fs::write(jail.join("target").join("obj.o"), b"artifact").unwrap();
    plant_journal(&data, "wt-p", &backing, None);
    plant_journal(&data, "wt-p-7", &backing, Some(&jail));

    let per_worktree =
        local_clean_artifacts_in(&dest, std::slice::from_ref(&data), JailLayout::PerWorktree)
            .unwrap();
    assert!(per_worktree.no_escapes);
    assert!(jail.join("target").join("obj.o").is_file());

    let per_mount =
        local_clean_artifacts_in(&dest, std::slice::from_ref(&data), JailLayout::PerMount).unwrap();
    assert_eq!((1, false), (per_mount.purged_entries, per_mount.no_escapes));
    assert!(!jail.join("target").exists());
    assert!(jail.join(grove_git::ESCAPE_ROOT_MARKER).is_file());
}

/// A marker planted under another slot that names a harmless dest must not
/// let the clean reach a jail that has no durable identity of its own.
#[test]
fn local_clean_planted_marker_does_not_purge_victim_jail() {
    let tmp = TempDir::new().unwrap();
    let data = tmp.path().join("grove");
    let victim_dest = tmp.path().join("real-dest");
    let harmless = tmp.path().join("harmless");
    std::fs::create_dir_all(&victim_dest).unwrap();
    std::fs::create_dir_all(&harmless).unwrap();
    let victim_backing = plant_marker(
        &data,
        "wt-esc-victim",
        &marker("wt-esc-victim", &victim_dest, 1),
    );
    plant_marker(
        &data,
        "wt-esc-decoy",
        &marker("wt-esc-victim", &harmless, 1),
    );
    let victim_jail = data.join(ESCAPE_DESTS_DIR).join("wt-esc-victim");
    let decoy_jail = data.join(ESCAPE_DESTS_DIR).join("wt-esc-decoy");
    for jail in [&victim_jail, &decoy_jail] {
        std::fs::create_dir_all(jail.join("target")).unwrap();
        std::fs::write(jail.join("target").join("obj.o"), b"artifact").unwrap();
    }
    plant_journal(&data, "wt-esc-victim", &victim_backing, Some(&victim_jail));

    let report = local_clean_artifacts_in(
        &harmless,
        std::slice::from_ref(&data),
        JailLayout::PerWorktree,
    );
    assert!(report.is_err(), "{report:?}");
    assert!(victim_jail.join("target").join("obj.o").is_file());
    assert!(decoy_jail.join("target").join("obj.o").is_file());
}
