use std::path::{Path, PathBuf};

use super::{
    create_source_layout_after_probe, git_or_grove_root, inject_grove_covering_mount,
    inject_grove_parent, inject_grove_projected, inject_grove_status, jj_slug_and_source_git_root,
    layout_from_status, lock_grove_parent_inject, nested_git_inside_mount, probe_grove_parent_sync,
    resolve_create_source_layout,
};
use crate::LockedTestEnv;
use crate::worktree::{
    CreateWorktreeFromWorktreeRequest, CreateWorktreeRequest, CreateWorktreeResponse,
    WorktreeCopyMode, WorktreeNotificationSender, WorktreeStatus, WorktreeType,
    create_worktree_from_worktree_sync, create_worktree_streaming, prepare_worktree_creation,
    prepare_worktree_from_worktree, repo_slug,
};
use a3s_fast_worktree::NfsStatusView;

fn status_view(raw: serde_json::Value) -> NfsStatusView {
    NfsStatusView {
        hydration_percent: None,
        raw: Some(raw),
        port: None,
        mount_id: None,
        transport: None,
    }
}

/// Live Grove `MountStatus` keys: `mountpoint` is the kernel dest; `worktree` is backing.
fn mount_status_json(mountpoint: &str) -> serde_json::Value {
    serde_json::json!({
        "mounts":[{
            "kind": "worktree",
            "source_mode": "local",
            "mountpoint": mountpoint,
            "worktree": "/var/grove/store/abc/worktree",
            "git_dir": "/var/grove/store/abc/git",
            "store_id": "abc"
        }]
    })
}

fn write_partialclone_extension(repo: &Path) {
    a3s_test_utils::git::run_git(repo, &["config", "core.repositoryformatversion", "1"]);
    a3s_test_utils::git::run_git(repo, &["config", "extensions.partialclone", "origin"]);
}

fn create_req(
    session_id: String,
    source: &Path,
    grove: Option<bool>,
    label: Option<&str>,
) -> CreateWorktreeRequest {
    CreateWorktreeRequest {
        session_id,
        source_path: source.to_string_lossy().into_owned(),
        worktree_path: None,
        copy_mode: WorktreeCopyMode::Dirty,
        git_ref: None,
        copy_ignored_in_background: false,
        ignored_skip_patterns: vec![],
        worktree_type: Some(WorktreeType::Linked),
        label: label.map(str::to_owned),
        grove_worktree: grove,
        grove_gate_source: grove.and(Some("request".into())),
        resolved_source_git_root: None,
    }
}

fn fork_req(
    new_session_id: String,
    source: &Path,
    grove: Option<bool>,
    label: Option<&str>,
) -> CreateWorktreeFromWorktreeRequest {
    CreateWorktreeFromWorktreeRequest {
        source_worktree_path: source.to_string_lossy().into_owned(),
        new_session_id,
        copy_mode: WorktreeCopyMode::Dirty,
        git_ref: None,
        worktree_type: Some(WorktreeType::Linked),
        label: label.map(str::to_owned),
        grove_worktree: grove,
        grove_gate_source: grove.and(Some("request".into())),
        cancellation_token: None,
        resolved_dest_path: None,
        resolved_source_git_root: None,
    }
}

#[test]
fn layout_from_status_uses_mountpoint_not_backing() {
    let cwd = Path::new("/mnt/grove/acme/crates/foo");
    let status = status_view(mount_status_json("/mnt/grove/acme"));
    let layout = layout_from_status(cwd, Some(&status));
    assert_eq!(layout.slug_root, PathBuf::from("/mnt/grove/acme"));
    assert_eq!(layout.source_git_root.as_deref(), Some("/mnt/grove/acme"));
}

#[test]
fn layout_from_status_does_not_use_backing_or_store_id() {
    let cwd = Path::new("/mnt/grove/acme");
    let status = status_view(serde_json::json!({
        "mounts":[{
            "worktree": "/var/grove/store/abc/worktree",
            "git_dir": "/var/grove/store/abc/git",
            "store_id": "abc"
        }]
    }));
    let layout = layout_from_status(cwd, Some(&status));
    assert_eq!(layout.slug_root, cwd);
    assert_eq!(layout.source_git_root.as_deref(), Some("/mnt/grove/acme"));
}

#[test]
fn layout_from_status_falls_back_to_source_path() {
    let cwd = Path::new("/mnt/grove/acme");
    let layout = layout_from_status(cwd, None);
    assert_eq!(layout.slug_root, cwd);
}

#[cfg(target_os = "macos")]
#[test]
fn layout_from_status_source_git_root_uses_launch_spelling() {
    let source = Path::new("/tmp/grove/acme/crates/foo");
    let slug = "/private/tmp/grove/acme";
    let layout = layout_from_status(source, Some(&status_view(mount_status_json(slug))));
    assert_eq!(layout.slug_root, PathBuf::from(slug));
    assert_eq!(
        layout.source_git_root.as_deref(),
        Some("/tmp/grove/acme"),
        "launch-cwd dest spelling without canonicalize"
    );
    assert_eq!(
        Path::new(layout.source_git_root.as_deref().unwrap())
            .join("crates")
            .join("foo"),
        source
    );
}

#[test]
fn git_or_grove_root_prefers_nested_git_when_discover_works() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let outer = temp.path().join("outer");
    let nested = outer.join("vendor").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    a3s_test_utils::git::init_git_repo(&outer);
    a3s_test_utils::git::init_git_repo(&nested);
    std::fs::write(nested.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&nested, "nested");
    let got = git_or_grove_root(&nested, true).expect("nested git");
    assert_eq!(
        dunce::canonicalize(&nested).unwrap(),
        dunce::canonicalize(&got).unwrap()
    );
}

#[test]
fn dest_slug_from_mountpoint_groups_subdirectory_cwd() {
    let cwd = Path::new("/mnt/grove/acme/crates/foo");
    let status = status_view(mount_status_json("/mnt/grove/acme"));
    let layout =
        create_source_layout_after_probe(cwd, Some(layout_from_status(cwd, Some(&status))))
            .expect("grove parent");
    assert!(layout.skipped_libgit2);
    assert_eq!(layout.git_root, PathBuf::from("/mnt/grove/acme"));
    assert_eq!(layout.source_git_root.as_deref(), Some("/mnt/grove/acme"));
    let home = PathBuf::from("/tmp/grok-home");
    let dest = crate::worktree::resolve_worktree_path(
        &home,
        &create_req("s".into(), cwd, Some(true), Some("probe-wt")),
        &layout.git_root,
    );
    let expected_base = home
        .join("worktrees")
        .join(repo_slug(Path::new("/mnt/grove/acme")));
    assert!(
        Path::new(&dest).starts_with(&expected_base),
        "dest {dest} must be under {}",
        expected_base.display()
    );
}

#[test]
fn grove_parent_skips_libgit2_on_partialclone() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("org").join("acme-app");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    write_partialclone_extension(&repo);
    assert!(
        crate::session::git::find_main_repo_root_from_path(&repo).is_err(),
        "precondition: libgit2 must reject extensions.partialclone"
    );

    let layout = create_source_layout_after_probe(&repo, Some(super::layout_from_source(&repo)))
        .expect("grove parent");
    assert!(layout.skipped_libgit2);
    assert_eq!(layout.git_root, repo);
}

#[test]
fn non_grove_still_uses_git_discovery() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("org").join("acme-app");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    std::fs::write(repo.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&repo, "initial");

    let layout = create_source_layout_after_probe(&repo, None).expect("git discover");
    assert!(!layout.skipped_libgit2);
    assert_eq!(
        dunce::canonicalize(&layout.git_root).unwrap(),
        dunce::canonicalize(&repo).unwrap()
    );

    write_partialclone_extension(&repo);
    let err = create_source_layout_after_probe(&repo, None).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("partialclone") || msg.contains("unsupported"),
        "non-Grove must still hit libgit2: {err}"
    );
}

#[test]
fn dest_slug_from_source_path_is_under_grok_worktrees() {
    let temp = tempfile::TempDir::new().unwrap();
    let source = temp.path().join("org").join("acme-app");
    std::fs::create_dir_all(&source).unwrap();
    let layout =
        create_source_layout_after_probe(&source, Some(super::layout_from_source(&source)))
            .unwrap();
    let home = temp.path().join("grok-home");
    let dest = crate::worktree::resolve_worktree_path(
        &home,
        &create_req("s".into(), &source, Some(true), Some("probe-wt")),
        &layout.git_root,
    );
    let expected_base = home.join("worktrees").join(repo_slug(&source));
    assert!(
        Path::new(&dest).starts_with(&expected_base),
        "dest {dest} must be under {}",
        expected_base.display()
    );
    assert!(dest.ends_with("probe-wt"), "{dest}");
}

#[tokio::test]
async fn prepare_grove_parent_skips_libgit2_discover() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let home = root.join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let source = root.join("org").join("acme-app");
    std::fs::create_dir_all(&source).unwrap();

    let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
    let _inject = inject_grove_parent();

    let session_id = format!("grove-skip-{}", std::process::id());
    let result = prepare_worktree_creation(&create_req(
        session_id.clone(),
        &source,
        Some(true),
        Some("probe-wt"),
    ))
    .await;
    assert!(result.spawn_task);
    let Ok(CreateWorktreeResponse::Creating {
        worktree_path,
        source_git_root,
        ..
    }) = result.response
    else {
        panic!("expected Creating, got {:?}", result.response.err());
    };
    let expected = home
        .join("worktrees")
        .join(repo_slug(&source))
        .join("probe-wt");
    assert_eq!(PathBuf::from(&worktree_path), expected);
    assert_eq!(
        source_git_root.as_deref(),
        Some(source.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn prepare_grove_off_still_discovers_git() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let home = root.join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let repo = root.join("org").join("acme-app");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    std::fs::write(repo.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&repo, "initial");

    let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
    let session_id = format!("grove-off-{}", std::process::id());
    let result =
        prepare_worktree_creation(&create_req(session_id, &repo, None, Some("probe-wt"))).await;
    assert!(result.spawn_task);
    let Ok(CreateWorktreeResponse::Creating { worktree_path, .. }) = result.response else {
        panic!("expected Creating");
    };
    let expected = home
        .join("worktrees")
        .join(repo_slug(&repo))
        .join("probe-wt");
    assert_eq!(PathBuf::from(worktree_path), expected);

    write_partialclone_extension(&repo);
    let bad = prepare_worktree_creation(&create_req(
        format!("grove-off-partial-{}", std::process::id()),
        &repo,
        None,
        Some("probe-wt-2"),
    ))
    .await;
    let err = bad
        .response
        .expect_err("Grove off must still fail libgit2 discover on partialclone");
    let msg = err.to_string();
    assert!(
        msg.contains("partialclone")
            || msg.contains("unsupported")
            || msg.contains("Invalid source path"),
        "expected discover failure, got {msg}"
    );
    assert!(!bad.spawn_task);
}

#[tokio::test]
async fn prepare_grove_probe_negative_still_discovers() {
    let _lock = lock_grove_parent_inject();
    let source = PathBuf::from(format!("/no/such/grove-probe-src-{}", std::process::id()));
    let discover_err = crate::session::git::find_main_repo_root_from_path(&source)
        .expect_err("precondition: path must not be inside a git repo");
    let result = prepare_worktree_creation(&create_req(
        format!("grove-neg-{}", std::process::id()),
        &source,
        Some(true),
        None,
    ))
    .await;
    let err = result
        .response
        .expect_err("negative Grove probe must fall through to libgit2 discover");
    let msg = err.to_string();
    assert!(
        msg.contains("Invalid source path") || msg.contains(&discover_err.to_string()),
        "expected wrapped discover error, got {msg}"
    );
    assert!(!result.spawn_task);
}

#[tokio::test]
async fn resolve_grove_parent_uses_spawn_blocking_inject() {
    let temp = tempfile::TempDir::new().unwrap();
    let source = temp.path().join("org").join("acme-app");
    std::fs::create_dir_all(&source).unwrap();
    let _inject = inject_grove_parent();
    let layout = resolve_create_source_layout(&source, true)
        .await
        .expect("injected grove parent");
    assert!(layout.skipped_libgit2);
    assert_eq!(layout.git_root, source);
}

#[tokio::test]
async fn resolve_injected_status_uses_mountpoint_for_dest_and_source_git_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let source = temp
        .path()
        .join("mnt")
        .join("grove")
        .join("acme")
        .join("crates")
        .join("foo");
    std::fs::create_dir_all(&source).unwrap();
    let mountpoint = temp.path().join("mnt").join("grove").join("acme");
    let _inject = inject_grove_status(mount_status_json(&mountpoint.to_string_lossy()));
    let layout = resolve_create_source_layout(&source, true)
        .await
        .expect("status grove parent");
    assert!(layout.skipped_libgit2);
    assert_eq!(layout.git_root, mountpoint);
    assert_eq!(
        layout.source_git_root.as_deref(),
        Some(mountpoint.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn projected_without_status_skips_discover_on_partialclone() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("org").join("acme-app");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    write_partialclone_extension(&repo);
    assert!(crate::session::git::find_main_repo_root_from_path(&repo).is_err());

    let _inject = inject_grove_projected();
    let layout = resolve_create_source_layout(&repo, true)
        .await
        .expect("projected grove parent");
    assert!(layout.skipped_libgit2);
    assert_eq!(layout.git_root, repo);
}

#[tokio::test]
async fn sync_fork_grove_parent_skips_libgit2_on_partialclone() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let home = root.join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let repo = root.join("org").join("acme-app");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    std::fs::write(repo.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&repo, "initial");
    write_partialclone_extension(&repo);
    assert!(
        crate::session::git::find_main_repo_root_from_path(&repo).is_err(),
        "precondition: libgit2 must reject extensions.partialclone"
    );
    assert!(crate::session::git::find_git_root_from_path(&repo).is_err());

    let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
    let _inject = inject_grove_parent();
    let req = fork_req(
        format!("grove-sync-{}", std::process::id()),
        &repo,
        Some(true),
        Some("probe-wt"),
    );
    let resp = create_worktree_from_worktree_sync(&req)
        .await
        .expect("sync fork after Grove skip must create, not fail at discover");
    let expected_base = home.join("worktrees").join(repo_slug(&repo));
    assert!(
        Path::new(&resp.worktree_path).starts_with(&expected_base),
        "dest {} must be under {}",
        resp.worktree_path,
        expected_base.display()
    );
    assert_eq!(
        resp.source_git_root.as_deref(),
        Some(repo.to_string_lossy().as_ref())
    );
}

struct NoopNotifier;

#[async_trait::async_trait]
impl WorktreeNotificationSender for NoopNotifier {
    async fn send_worktree_status(&self, _progress: WorktreeStatus) {}
}

#[tokio::test]
async fn pinned_streaming_keeps_prepare_source_git_root() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let home = root.join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let repo = root.join("mnt").join("grove").join("acme");
    let cwd = repo.join("crates").join("foo");
    std::fs::create_dir_all(&cwd).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    std::fs::write(cwd.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&repo, "initial");

    let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
    let _inject = inject_grove_status(mount_status_json(&repo.to_string_lossy()));
    let session_id = format!("grove-pin-sgr-{}", std::process::id());
    let prepared = prepare_worktree_creation(&create_req(
        session_id.clone(),
        &cwd,
        Some(true),
        Some("probe-wt"),
    ))
    .await;
    let Ok(CreateWorktreeResponse::Creating {
        worktree_path,
        source_git_root,
        ..
    }) = prepared.response
    else {
        panic!("expected Creating, got {:?}", prepared.response.err());
    };
    assert_eq!(
        source_git_root.as_deref(),
        Some(repo.to_string_lossy().as_ref()),
        "prepare must pin dest workdir, not launch cwd"
    );
    assert_ne!(
        source_git_root.as_deref(),
        Some(cwd.to_string_lossy().as_ref())
    );

    let mut req = create_req(session_id, &cwd, Some(true), Some("probe-wt"));
    req.worktree_path = Some(worktree_path);
    req.resolved_source_git_root = source_git_root.clone();
    let status = create_worktree_streaming(&req, &NoopNotifier).await;
    let WorktreeStatus::Created {
        source_git_root: created_sgr,
        ..
    } = status
    else {
        panic!("expected Created, got {status:?}");
    };
    assert_eq!(created_sgr, source_git_root);
    assert_eq!(
        created_sgr.as_deref(),
        Some(repo.to_string_lossy().as_ref()),
        "Created.source_git_root must stay the Grove dest, not cwd"
    );
}

#[tokio::test]
async fn fork_prepare_grove_parent_skips_git_dir_gate() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    let home = root.join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let source = root.join("org").join("acme-app");
    std::fs::create_dir_all(&source).unwrap();

    let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
    let _inject = inject_grove_parent();
    let result = prepare_worktree_from_worktree(&fork_req(
        format!("grove-fork-prep-{}", std::process::id()),
        &source,
        Some(true),
        Some("probe-wt"),
    ))
    .await;
    assert!(result.spawn_task);
    let Ok(CreateWorktreeResponse::Creating {
        worktree_path,
        source_git_root,
        ..
    }) = result.response
    else {
        panic!("expected Creating, got {:?}", result.response.err());
    };
    let expected = home
        .join("worktrees")
        .join(repo_slug(&source))
        .join("probe-wt");
    assert_eq!(PathBuf::from(&worktree_path), expected);
    assert_eq!(
        source_git_root.as_deref(),
        Some(source.to_string_lossy().as_ref())
    );
}

#[test]
fn probe_without_inject_is_none_without_daemon() {
    let _lock = lock_grove_parent_inject();
    let temp = tempfile::TempDir::new().unwrap();
    assert!(probe_grove_parent_sync(temp.path()).is_none());
}

#[test]
fn probe_status_fixture_keeps_grove_create() {
    let _inject = inject_grove_status(mount_status_json("/mnt/grove/acme"));
    let source = Path::new("/mnt/grove/acme/crates/foo");
    let layout = probe_grove_parent_sync(source).expect("status fixture");
    assert_eq!(layout.slug_root, PathBuf::from("/mnt/grove/acme"));
    assert_eq!(layout.source_git_root.as_deref(), Some("/mnt/grove/acme"));
}

#[test]
fn probe_status_fixture_without_keeps_grove_create_is_none() {
    let _inject = inject_grove_status(serde_json::json!({
        "mounts":[{"kind":"store"}]
    }));
    let source = Path::new("/mnt/grove/acme/crates/foo");
    assert!(
        probe_grove_parent_sync(source).is_none(),
        "store-only Status must not skip libgit2"
    );
}

#[test]
fn probe_covering_mount_subdir_skips_without_status() {
    let dest = PathBuf::from("/mnt/grove/acme");
    let source = dest.join("crates").join("foo");
    let _inject = inject_grove_covering_mount(dest.clone());
    let layout = probe_grove_parent_sync(&source).expect("covering dest subdir");
    assert_eq!(layout.slug_root, dest);
    assert_eq!(layout.source_git_root.as_deref(), Some("/mnt/grove/acme"));
}

#[test]
fn dest_root_gitfile_is_not_nested_clone() {
    let temp = tempfile::TempDir::new().unwrap();
    let dest = temp.path().join("acme");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join(".git"), "gitdir: ./x\n").unwrap();
    assert!(
        !nested_git_inside_mount(&dest, &dest),
        "dest root .git is the Grove dest, not a nested clone"
    );
    let sub = dest.join("crates").join("foo");
    std::fs::create_dir_all(&sub).unwrap();
    assert!(!nested_git_inside_mount(&sub, &dest));
}

#[cfg(windows)]
#[test]
fn dest_root_gitfile_is_not_nested_when_mountpoint_case_differs() {
    let temp = tempfile::TempDir::new().unwrap();
    let dest = temp.path().join("acme");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join(".git"), "gitdir: ./x\n").unwrap();
    let flipped = PathBuf::from(dest.to_string_lossy().to_ascii_uppercase());
    assert!(
        !nested_git_inside_mount(&dest, &flipped),
        "Windows Status mountpoint case must not treat the dest as a nested clone"
    );
}

#[test]
fn jj_slug_uses_main_repo_for_linked_worktree() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("main");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    std::fs::write(repo.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&repo, "initial");
    let wt = temp.path().join("linked");
    let wt_s = wt.to_string_lossy();
    a3s_test_utils::git::run_git(&repo, &["worktree", "add", wt_s.as_ref(), "HEAD"]);
    let (slug, nearest) = jj_slug_and_source_git_root(&wt, false);
    assert_eq!(
        dunce::canonicalize(&repo).unwrap(),
        dunce::canonicalize(slug.as_ref().unwrap()).unwrap(),
        "jj dest slug must group under the main repo"
    );
    assert_eq!(
        dunce::canonicalize(&wt).unwrap(),
        dunce::canonicalize(nearest.as_ref().unwrap()).unwrap(),
        "source_git_root stays the linked worktree"
    );
}

#[test]
fn probe_nested_git_inside_covering_mount_is_none() {
    let temp = tempfile::TempDir::new().unwrap();
    let dest = temp.path().join("acme");
    let nested = dest.join("vendor").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join(".git"), "gitdir: ./not-grove\n").unwrap();
    let _inject = inject_grove_covering_mount(dest);
    assert!(
        probe_grove_parent_sync(&nested).is_none(),
        "nested clone inside a Grove dest must still use git"
    );
}

#[test]
fn probe_status_nested_git_inside_dest_is_none() {
    let temp = tempfile::TempDir::new().unwrap();
    let dest = temp.path().join("acme");
    let nested = dest.join("vendor").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join(".git"), "gitdir: ./not-grove\n").unwrap();
    let _inject = inject_grove_status(mount_status_json(&dest.to_string_lossy()));
    assert!(
        probe_grove_parent_sync(&nested).is_none(),
        "Status keeps_grove_create must not skip libgit2 for a nested clone"
    );
}

#[test]
fn pinned_grove_on_ordinary_git_falls_back_to_discover() {
    a3s_test_utils::require_git!();
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().join("org").join("acme-app");
    std::fs::create_dir_all(&repo).unwrap();
    a3s_test_utils::git::init_git_repo(&repo);
    std::fs::write(repo.join("tracked.txt"), "x").unwrap();
    a3s_test_utils::git::git_commit_all(&repo, "initial");
    let cwd = repo.join("crates").join("foo");
    std::fs::create_dir_all(&cwd).unwrap();

    let got = crate::worktree::source_git_root_for_pinned_dest(&cwd, true, None);
    assert_eq!(
        Some(dunce::canonicalize(&repo).unwrap()),
        got.as_deref().map(|p| dunce::canonicalize(p).unwrap()),
        "mount-table miss on Grove-on ordinary git must still discover, got {got:?}"
    );
}
