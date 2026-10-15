use super::super::WorkspaceServices;
use super::*;

#[tokio::test]
async fn local_backend_reads_writes_and_lists() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let path = services.normalize_path("dir/file.txt").unwrap();

    let written = services
        .fs()
        .write_text(&path, "hello\nworld\n")
        .await
        .unwrap();
    assert_eq!(written.bytes, 12);
    assert_eq!(written.lines, 2);

    let content = services.fs().read_text(&path).await.unwrap();
    assert_eq!(content, "hello\nworld\n");

    let dir = services.normalize_path("dir").unwrap();
    let entries = services.fs().list_dir(&dir).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "file.txt");
}

#[tokio::test]
async fn local_backend_searches_glob_and_grep() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    services
        .fs()
        .write_text(
            &services.normalize_path("src/main.rs").unwrap(),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .await
        .unwrap();
    services
        .fs()
        .write_text(
            &services.normalize_path("README.md").unwrap(),
            "hello from docs\n",
        )
        .await
        .unwrap();

    let search = services.search().expect("local backend supports search");
    let glob = search
        .glob(WorkspaceGlobRequest {
            base: services.normalize_path("src").unwrap(),
            pattern: "*.rs".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(glob.matches[0].as_str(), "src/main.rs");

    let grep = search
        .grep(WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "hello".to_string(),
            glob: Some("**/*.rs".to_string()),
            context_lines: 0,
            case_insensitive: false,
            max_output_size: 1024,
        })
        .await
        .unwrap();
    assert_eq!(grep.match_count, 1);
    assert_eq!(grep.file_count, 1);
    assert!(grep.output.contains("src/main.rs:2"));
}

#[test]
fn grep_walk_stops_when_cancel_is_already_set() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a.txt"), "needle\n").unwrap();
    let regex = regex::Regex::new("needle").unwrap();
    let request = WorkspaceGrepRequest {
        base: WorkspacePath::root(),
        pattern: "needle".to_string(),
        glob: None,
        context_lines: 0,
        case_insensitive: false,
        max_output_size: 1024,
    };
    let cancel = AtomicBool::new(true);
    let error = grep_walk(temp.path(), None, temp.path(), &regex, &request, &cancel).unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn grep_poll_yields_before_the_walk_finishes() {
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("big.txt"), vec![b'x'; 8 * 1024 * 1024]).unwrap();
    let backend = LocalWorkspaceBackend::new(temp.path().to_path_buf());
    let request = WorkspaceGrepRequest {
        base: WorkspacePath::root(),
        pattern: "needle-not-in-file".to_string(),
        glob: None,
        context_lines: 0,
        case_insensitive: false,
        max_output_size: 1024,
    };
    let mut fut = std::pin::pin!(backend.grep_with_sources(request));
    let pending = poll_fn(|cx| match fut.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(true),
        Poll::Ready(_) => Poll::Ready(false),
    })
    .await;
    assert!(
        pending,
        "grep held the current-thread runtime for the whole walk"
    );
}

fn credential_boundary_backend(root: &Path) -> LocalWorkspaceBackend {
    LocalWorkspaceBackend::new_with_access_policy(
        root.to_path_buf(),
        LocalWorkspaceAccessPolicy::CredentialBoundary,
    )
}

#[tokio::test]
async fn credential_boundary_denies_direct_secret_reads_and_writes() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("apps/api")).unwrap();
    std::fs::write(temp.path().join("apps/api/.env.local"), "TOKEN=secret\n").unwrap();
    let backend = credential_boundary_backend(temp.path());
    let secret = backend.normalize("apps/api/.env.local").unwrap();

    let read_error = backend
        .read_text(&secret)
        .await
        .expect_err("direct secret reads must be denied");
    assert!(read_error.to_string().contains("credential boundary"));

    let range_error = backend
        .read_text_range(&secret, 0, 10)
        .await
        .expect_err("range reads must use the same boundary");
    assert!(range_error.to_string().contains("credential boundary"));

    let write_error = backend
        .write_text(&secret, "TOKEN=overwritten\n")
        .await
        .expect_err("direct secret writes must be denied");
    assert!(write_error.to_string().contains("credential boundary"));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("apps/api/.env.local")).unwrap(),
        "TOKEN=secret\n"
    );

    let new_secret = backend.normalize(".env.generated").unwrap();
    backend
        .write_text(&new_secret, "TOKEN=new\n")
        .await
        .expect_err("creating a new env file must be denied");
    assert!(!temp.path().join(".env.generated").exists());
}

#[tokio::test]
async fn credential_boundary_filters_grep_and_rejects_explicit_secret_base() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(".env"), "BOUNDARY_TOKEN=secret\n").unwrap();
    std::fs::write(
        temp.path().join("README.md"),
        "BOUNDARY_TOKEN is configured externally\n",
    )
    .unwrap();
    let backend = credential_boundary_backend(temp.path());

    let grep = backend
        .grep(WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "BOUNDARY_TOKEN".to_string(),
            glob: None,
            context_lines: 0,
            case_insensitive: false,
            max_output_size: 1024,
        })
        .await
        .unwrap();
    assert_eq!(grep.match_count, 1);
    assert_eq!(grep.file_count, 1);
    assert!(grep.output.contains("README.md"));
    assert!(!grep.output.contains("secret"));
    assert!(!grep.output.contains(".env"));

    let error = backend
        .grep(WorkspaceGrepRequest {
            base: backend.normalize(".env").unwrap(),
            pattern: "secret".to_string(),
            glob: None,
            context_lines: 0,
            case_insensitive: false,
            max_output_size: 1024,
        })
        .await
        .expect_err("an explicit secret grep must fail closed");
    assert!(error.to_string().contains("credential boundary"));
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn credential_boundary_denies_source_hardlinks_without_truncating_them() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    let alias = temp.path().join("alias.txt");
    std::fs::write(&source, "linked secret\n").unwrap();
    std::fs::hard_link(&source, &alias).unwrap();
    let backend = credential_boundary_backend(temp.path());
    let alias_path = backend.normalize("alias.txt").unwrap();

    backend
        .read_text(&alias_path)
        .await
        .expect_err("source-tree hardlink reads must be denied");
    backend
        .write_text(&alias_path, "overwritten\n")
        .await
        .expect_err("source-tree hardlink writes must be denied");
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "linked secret\n");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn source_egress_boundary_denies_control_paths_and_every_hardlink() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::create_dir_all(temp.path().join(".a3s")).unwrap();
    let source = temp.path().join("source.txt");
    let alias = temp.path().join("src/apparently-safe.txt");
    std::fs::write(&source, "linked source\n").unwrap();
    std::fs::hard_link(&source, &alias).unwrap();
    std::fs::write(temp.path().join(".a3s/config.acl"), "secret = true\n").unwrap();
    std::fs::write(temp.path().join("src/safe.rs"), "pub fn safe() {}\n").unwrap();
    let backend = LocalWorkspaceBackend::new_with_source_egress_policy(temp.path().to_path_buf());

    backend
        .read_text(&backend.normalize("src/apparently-safe.txt").unwrap())
        .await
        .expect_err("source egress must reject every multi-link file");
    backend
        .read_text(&backend.normalize(".a3s/config.acl").unwrap())
        .await
        .expect_err("source egress must reject control paths at read time");
    assert_eq!(
        backend
            .read_text(&backend.normalize("src/safe.rs").unwrap())
            .await
            .unwrap(),
        "pub fn safe() {}\n"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn credential_boundary_allows_package_store_hardlinks_but_denies_secret_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("node_modules/pkg");
    std::fs::create_dir_all(&package).unwrap();

    let package_source = package.join("source.js");
    let package_alias = package.join("alias.js");
    std::fs::write(&package_source, "export const value = 1;\n").unwrap();
    std::fs::hard_link(&package_source, &package_alias).unwrap();

    let env = temp.path().join(".env");
    let env_alias = package.join("credential.txt");
    std::fs::write(&env, "TOKEN=secret\n").unwrap();
    std::fs::hard_link(&env, &env_alias).unwrap();

    let backend = credential_boundary_backend(temp.path());
    let package_content = backend
        .read_text(&backend.normalize("node_modules/pkg/alias.js").unwrap())
        .await
        .expect("ordinary package-store hardlinks should remain readable");
    assert!(package_content.contains("value = 1"));

    let error = backend
        .read_text(
            &backend
                .normalize("node_modules/pkg/credential.txt")
                .unwrap(),
        )
        .await
        .expect_err("a package-tree alias of a known credential must be denied");
    assert!(error.to_string().contains("credential boundary"));
}

fn run_test_git(root: &Path, args: &[&str]) -> bool {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=A3S Test",
            "-c",
            "user.email=test@a3s.local",
        ])
        .args(args);
    let started =
        crate::tools::process::status_std_child(&mut command).is_ok_and(|status| status.success());
    if started && args.first() == Some(&"init") {
        // Do not inherit the runner's `core.autocrlf`. A Windows checkout
        // that rewrites `\n` to `\r\n` dirties ordinary files and makes a
        // preserved credential file look overwritten.
        let _ = run_test_git(root, &["config", "core.autocrlf", "false"]);
        let _ = run_test_git(root, &["config", "core.eol", "lf"]);
    }
    started
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn credential_boundary_filters_git_diff_content_and_option_like_targets() {
    let temp = tempfile::tempdir().unwrap();
    if !run_test_git(temp.path(), &["init", "-q"]) {
        return;
    }
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join(".env"), "TOKEN=old-secret\n").unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 1;\n").unwrap();
    std::fs::write(temp.path().join("linked.txt"), "hardlink-old-secret\n").unwrap();
    std::fs::hard_link(
        temp.path().join("linked.txt"),
        temp.path().join("linked-alias.txt"),
    )
    .unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "baseline"]));

    std::fs::write(temp.path().join(".env"), "TOKEN=new-secret\n").unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 2;\n").unwrap();
    std::fs::write(
        temp.path().join("linked-alias.txt"),
        "hardlink-new-secret\n",
    )
    .unwrap();

    let backend = credential_boundary_backend(temp.path());
    let diff = backend
        .diff(WorkspaceGitDiffRequest { target: None })
        .await
        .unwrap();
    assert!(diff.contains("VALUE: u8 = 2"), "{diff}");
    for denied in [
        "old-secret",
        "new-secret",
        "hardlink-old-secret",
        "hardlink-new-secret",
        ".env",
        "linked.txt",
        "linked-alias.txt",
    ] {
        assert!(!diff.contains(denied), "{denied} leaked in {diff}");
    }

    let output = temp.path().join("injected-diff-output");
    let error = backend
        .diff(WorkspaceGitDiffRequest {
            target: Some(format!("--output={}", output.display())),
        })
        .await
        .expect_err("an option-like target must be parsed only as a revision");
    assert!(error.to_string().contains("Git diff"));
    assert!(!output.exists());
}

#[tokio::test]
async fn credential_boundary_checkout_does_not_overwrite_a_credential_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join(".env"), "TOKEN=checkout-secret-4e17\n").unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 1;\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "old"]));
    assert!(run_test_git(temp.path(), &["branch", "old"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=checkout-secret-new\n").unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 2;\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "new"]));
    let new_branch = if run_test_git(temp.path(), &["rev-parse", "--verify", "main"]) {
        "main"
    } else {
        "master"
    };
    assert!(run_test_git(temp.path(), &["checkout", "-q", "old"]));

    let backend = credential_boundary_backend(temp.path());
    let error = backend
        .checkout(WorkspaceGitCheckoutRequest {
            refspec: new_branch.to_string(),
            force: false,
        })
        .await
        .expect_err("checkout must not apply a credential path");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env")).unwrap(),
        "TOKEN=checkout-secret-4e17\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("src/lib.rs")).unwrap(),
        "pub const VALUE: u8 = 1;\n"
    );
}

#[tokio::test]
async fn credential_boundary_checkout_still_updates_an_ordinary_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "old"]));
    assert!(run_test_git(temp.path(), &["branch", "old"]));
    std::fs::write(temp.path().join("README.md"), "new\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "new"]));
    let new_branch = if run_test_git(temp.path(), &["rev-parse", "--verify", "main"]) {
        "main"
    } else {
        "master"
    };
    assert!(run_test_git(temp.path(), &["checkout", "-q", "old"]));

    let backend = credential_boundary_backend(temp.path());
    backend
        .checkout(WorkspaceGitCheckoutRequest {
            refspec: new_branch.to_string(),
            force: false,
        })
        .await
        .expect("ordinary checkout");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
        "new\n"
    );
}

#[tokio::test]
async fn credential_boundary_force_checkout_does_not_reset_a_dirty_credential_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=committed\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=dirty-checkout-7c41\n").unwrap();

    let backend = credential_boundary_backend(temp.path());
    let error = backend
        .checkout(WorkspaceGitCheckoutRequest {
            refspec: "HEAD".to_string(),
            force: true,
        })
        .await
        .expect_err("force checkout must not reset a credential file");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env")).unwrap(),
        "TOKEN=dirty-checkout-7c41\n"
    );
}

#[tokio::test]
async fn credential_boundary_stash_does_not_reset_a_dirty_credential_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=committed\n").unwrap();
    std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=dirty-stash-7c41\n").unwrap();
    std::fs::write(temp.path().join("README.md"), "new\n").unwrap();

    let backend = credential_boundary_backend(temp.path());
    let error = backend
        .stash(WorkspaceGitStashRequest {
            message: Some("should-not-land".to_string()),
            include_untracked: false,
        })
        .await
        .expect_err("stash must not reset a credential file");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env")).unwrap(),
        "TOKEN=dirty-stash-7c41\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
        "new\n"
    );
    assert!(backend.list_stashes().await.unwrap().is_empty());
}

#[tokio::test]
async fn credential_boundary_stash_does_not_remove_an_untracked_credential_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=untracked-stash-91aa\n").unwrap();
    std::fs::write(temp.path().join("notes.txt"), "keep\n").unwrap();

    let backend = credential_boundary_backend(temp.path());
    let error = backend
        .stash(WorkspaceGitStashRequest {
            message: Some("should-not-land".to_string()),
            include_untracked: true,
        })
        .await
        .expect_err("stash -u must not remove an untracked credential file");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env")).unwrap(),
        "TOKEN=untracked-stash-91aa\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
        "keep\n"
    );
    assert!(backend.list_stashes().await.unwrap().is_empty());
}

#[tokio::test]
async fn credential_boundary_stash_still_saves_an_ordinary_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
    std::fs::write(temp.path().join("README.md"), "new\n").unwrap();

    let backend = credential_boundary_backend(temp.path());
    backend
        .stash(WorkspaceGitStashRequest {
            message: Some("ordinary".to_string()),
            include_untracked: false,
        })
        .await
        .expect("an ordinary dirty file must still be stashable");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
        "old\n"
    );
    let stashes = backend.list_stashes().await.unwrap();
    assert!(
        stashes
            .iter()
            .any(|stash| stash.message.contains("ordinary")),
        "{stashes:?}"
    );
}

#[tokio::test]
async fn create_branch_does_not_treat_an_option_like_base_as_a_git_flag() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=committed\n").unwrap();
    std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
    assert!(run_test_git(temp.path(), &["branch", "-M", "main"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=dirty-branch-4e18\n").unwrap();
    std::fs::write(temp.path().join("README.md"), "new\n").unwrap();

    let backend = LocalWorkspaceBackend::new(temp.path().to_path_buf());
    let error = backend
        .create_branch(WorkspaceGitCreateBranchRequest {
            name: "feature".to_string(),
            base: "-f".to_string(),
        })
        .await
        .expect_err("an option-like base must not force-discard the worktree");
    assert!(error.to_string().contains("create branch"), "{error}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env")).unwrap(),
        "TOKEN=dirty-branch-4e18\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
        "new\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".git/HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
}

#[tokio::test]
async fn credential_boundary_create_branch_does_not_rewrite_a_credential_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join(".env"), "TOKEN=branch-secret-4e18\n").unwrap();
    std::fs::write(temp.path().join("README.md"), "one\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "main"]));
    assert!(run_test_git(temp.path(), &["branch", "-M", "main"]));
    assert!(run_test_git(
        temp.path(),
        &["checkout", "-q", "-b", "other"]
    ));
    std::fs::write(temp.path().join(".env"), "TOKEN=other\n").unwrap();
    std::fs::write(temp.path().join("README.md"), "two\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "other"]));
    assert!(run_test_git(temp.path(), &["checkout", "-q", "main"]));

    let backend = credential_boundary_backend(temp.path());
    let error = backend
        .create_branch(WorkspaceGitCreateBranchRequest {
            name: "feature".to_string(),
            base: "other".to_string(),
        })
        .await
        .expect_err("creating a branch must not check out a credential path");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".env")).unwrap(),
        "TOKEN=branch-secret-4e18\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
        "one\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".git/HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
}

#[tokio::test]
async fn credential_boundary_create_branch_still_updates_an_ordinary_file() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join("README.md"), "one\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "main"]));
    assert!(run_test_git(temp.path(), &["branch", "-M", "main"]));
    assert!(run_test_git(
        temp.path(),
        &["checkout", "-q", "-b", "other"]
    ));
    std::fs::write(temp.path().join("README.md"), "two\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "other"]));
    assert!(run_test_git(temp.path(), &["checkout", "-q", "main"]));

    let backend = credential_boundary_backend(temp.path());
    backend
        .create_branch(WorkspaceGitCreateBranchRequest {
            name: "feature".to_string(),
            base: "other".to_string(),
        })
        .await
        .expect("a branch that only changes an ordinary file must still be created");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
        "two\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join(".git/HEAD")).unwrap(),
        "ref: refs/heads/feature\n"
    );
}

#[tokio::test]
async fn checkout_does_not_treat_an_option_like_ref_as_a_git_flag() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run_test_git(temp.path(), &["init", "-q"]));
    std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
    assert!(run_test_git(temp.path(), &["add", "."]));
    assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
    let output = temp.path().join("injected-checkout");
    let backend = LocalWorkspaceBackend::new(temp.path().to_path_buf());
    let error = backend
        .checkout(WorkspaceGitCheckoutRequest {
            refspec: format!("--output={}", output.display()),
            force: true,
        })
        .await
        .expect_err("an option-like ref must not become a Git flag");
    assert!(
        error.to_string().contains("option")
            || error.to_string().contains("checkout")
            || error.to_string().contains("Git"),
        "{error}"
    );
    assert!(!output.exists());
}

#[test]
fn local_backend_rejects_absolute_paths_outside_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let outside = temp.path().parent().unwrap().join("secret.txt");
    let err = services
        .normalize_path(outside.to_str().unwrap())
        .expect_err("outside absolute path should be rejected");
    assert!(err.to_string().contains("escapes workspace"));
}

#[test]
fn local_backend_rejects_backslash_parent_escape() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let err = services
        .normalize_path(r"..\secret.txt")
        .expect_err("backslash parent traversal should be rejected");
    assert!(err.to_string().contains("escapes workspace"));
}

#[test]
fn local_backend_allows_absolute_paths_inside_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let absolute = temp.path().join("src/main.rs");
    let path = services
        .normalize_path(absolute.to_str().unwrap())
        .expect("absolute path inside workspace should normalize");
    assert_eq!(path.as_str(), "src/main.rs");
}

#[cfg(unix)]
#[tokio::test]
async fn write_text_does_not_follow_a_symlink_file_out_of_the_workspace() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, "outside-token-4c91").unwrap();
    symlink(&outside_file, workspace.path().join("guest.txt")).unwrap();

    let services = WorkspaceServices::local(workspace.path());
    let path = services.normalize_path("guest.txt").unwrap();
    let error = services
        .fs()
        .write_text(&path, "written-through-link")
        .await
        .expect_err("a symlink destination must not be written");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&outside_file).unwrap(),
        "outside-token-4c91"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_text_does_not_create_directories_through_a_symlink() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), workspace.path().join("escape")).unwrap();

    let services = WorkspaceServices::local(workspace.path());
    let path = services.normalize_path("escape/nested/new.txt").unwrap();
    let error = services
        .fs()
        .write_text(&path, "created-outside")
        .await
        .expect_err("a symlink parent must not receive new directories");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert!(!outside.path().join("nested").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn read_text_does_not_return_bytes_through_a_symlink() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, "outside-token-b81e").unwrap();
    symlink(&outside_file, workspace.path().join("guest.txt")).unwrap();

    let services = WorkspaceServices::local(workspace.path());
    let path = services.normalize_path("guest.txt").unwrap();
    let error = services
        .fs()
        .read_text(&path)
        .await
        .expect_err("a symlink read must not return outside bytes");
    assert!(!error.to_string().contains("outside-token-b81e"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn list_dir_does_not_follow_a_symlink_directory() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("hidden.txt"), "outside-token-c44a").unwrap();
    symlink(outside.path(), workspace.path().join("escape")).unwrap();

    let services = WorkspaceServices::local(workspace.path());
    let path = services.normalize_path("escape").unwrap();
    let error = services
        .fs()
        .list_dir(&path)
        .await
        .expect_err("a symlink directory must not be listed");
    let rendered = error.to_string();
    assert!(!rendered.contains("hidden.txt"), "{rendered}");
    assert!(!rendered.contains("outside-token-c44a"), "{rendered}");
}

#[cfg(unix)]
#[tokio::test]
async fn grep_and_glob_do_not_surface_a_symlink_target() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("hidden.txt"), "outside-token-e90d").unwrap();
    symlink(outside.path(), workspace.path().join("escape")).unwrap();
    std::fs::write(workspace.path().join("local.txt"), "local-only\n").unwrap();

    let services = WorkspaceServices::local(workspace.path());
    let search = services.search().expect("local backend supports search");
    let grep = search
        .grep(WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "outside-token-e90d".to_string(),
            glob: None,
            context_lines: 0,
            case_insensitive: false,
            max_output_size: 4096,
        })
        .await
        .unwrap();
    assert_eq!(grep.match_count, 0, "{}", grep.output);
    assert!(!grep.output.contains("outside-token-e90d"));

    let glob = search
        .glob(WorkspaceGlobRequest {
            base: WorkspacePath::root(),
            pattern: "**/*".to_string(),
        })
        .await
        .unwrap();
    assert!(
        glob.matches
            .iter()
            .all(|path| !path.as_str().contains("hidden.txt")),
        "{:?}",
        glob.matches
    );
}

#[cfg(unix)]
#[tokio::test]
async fn create_worktree_does_not_follow_a_symlink_directory() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("keep.txt"),
        "outside-worktree-token-6a41",
    )
    .unwrap();
    symlink(outside.path(), workspace.path().join("escape")).unwrap();
    init_worktree_repo(workspace.path());

    let backend = LocalWorkspaceBackend::new(workspace.path().to_path_buf());
    let error = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "feature".to_string(),
            path: Some("escape/wt".to_string()),
            new_branch: true,
        })
        .await
        .expect_err("worktree create must not follow a symlink out of the workspace");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert!(!outside.path().join("wt").exists());
    assert_eq!(
        std::fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
        "outside-worktree-token-6a41"
    );

    let absolute = workspace.path().join("escape/wt-abs");
    let error = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "feature-abs".to_string(),
            path: Some(absolute.display().to_string()),
            new_branch: true,
        })
        .await
        .expect_err("an absolute path must not follow a symlink out of the workspace");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert!(!outside.path().join("wt-abs").exists());
    assert_eq!(
        std::fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
        "outside-worktree-token-6a41"
    );

    let sibling = tempfile::tempdir().unwrap();
    let error = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "sibling".to_string(),
            path: Some(format!(
                "../{}",
                sibling.path().file_name().unwrap().to_string_lossy()
            )),
            new_branch: true,
        })
        .await
        .expect_err("a relative parent path must not select a sibling worktree");
    assert!(
        error.to_string().contains("unsupported component"),
        "{error}"
    );
    assert!(std::fs::read_dir(sibling.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn create_worktree_still_creates_an_ordinary_worktree() {
    let workspace = tempfile::tempdir().unwrap();
    init_worktree_repo(workspace.path());
    let backend = LocalWorkspaceBackend::new(workspace.path().to_path_buf());
    let created = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "feature".to_string(),
            path: Some("wt".to_string()),
            new_branch: true,
        })
        .await
        .expect("an ordinary worktree path must still be created");
    assert!(created.path.ends_with("wt"), "{}", created.path);
    assert!(workspace.path().join("wt/README.md").is_file());
    backend
        .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
            path: workspace.path().join("wt").display().to_string(),
            force: true,
        })
        .await
        .expect("the ordinary worktree must still be removable");
}

#[tokio::test]
async fn credential_boundary_create_worktree_does_not_check_out_a_credential_file() {
    let workspace = tempfile::tempdir().unwrap();
    init_worktree_repo(workspace.path());
    std::fs::write(
        workspace.path().join(".env"),
        "TOKEN=worktree-secret-2c91\n",
    )
    .unwrap();
    assert!(run_test_git(workspace.path(), &["add", ".env"]));
    assert!(run_test_git(workspace.path(), &["commit", "-qm", "secret"]));

    let backend = credential_boundary_backend(workspace.path());
    let error = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "feature".to_string(),
            path: Some("wt".to_string()),
            new_branch: true,
        })
        .await
        .expect_err("worktree create must not check out a credential file");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert!(!workspace.path().join("wt").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".env")).unwrap(),
        "TOKEN=worktree-secret-2c91\n"
    );

    assert!(run_test_git(
        workspace.path(),
        &["checkout", "-q", "-b", "other"]
    ));
    std::fs::write(workspace.path().join(".env"), "TOKEN=other-worktree-8f12\n").unwrap();
    assert!(run_test_git(workspace.path(), &["add", ".env"]));
    assert!(run_test_git(workspace.path(), &["commit", "-qm", "other"]));
    assert!(run_test_git(workspace.path(), &["checkout", "-q", "main"]));
    let error = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "other".to_string(),
            path: Some("wt2".to_string()),
            new_branch: false,
        })
        .await
        .expect_err("checking out an existing branch must not write a credential file");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert!(!workspace.path().join("wt2").exists());

    let alias = workspace.path().join("wt-alias");
    let error = backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "alias-feature".to_string(),
            path: Some(alias.display().to_string()),
            new_branch: true,
        })
        .await
        .expect_err("a non-canonical absolute path must not skip the credential check");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert!(!alias.exists());
    assert!(!workspace
        .path()
        .canonicalize()
        .unwrap()
        .join("wt-alias")
        .exists());
}

#[tokio::test]
async fn credential_boundary_create_worktree_still_creates_an_ordinary_tree() {
    let workspace = tempfile::tempdir().unwrap();
    init_worktree_repo(workspace.path());
    let backend = credential_boundary_backend(workspace.path());
    backend
        .create_worktree(WorkspaceGitCreateWorktreeRequest {
            branch: "feature".to_string(),
            path: Some("wt".to_string()),
            new_branch: true,
        })
        .await
        .expect("a tree without credential files must still create a worktree");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("wt/README.md")).unwrap(),
        "v1\n"
    );
    assert!(!workspace.path().join("wt/.env").exists());
    backend
        .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
            path: workspace.path().join("wt").display().to_string(),
            force: true,
        })
        .await
        .expect("an ordinary worktree must still be removable");
    assert!(!workspace.path().join("wt").exists());
}

#[tokio::test]
async fn credential_boundary_remove_worktree_does_not_delete_a_credential_file() {
    let workspace = tempfile::tempdir().unwrap();
    init_worktree_repo(workspace.path());
    std::fs::write(workspace.path().join(".env"), "TOKEN=remove-wt-6b20\n").unwrap();
    assert!(run_test_git(workspace.path(), &["add", ".env"]));
    assert!(run_test_git(workspace.path(), &["commit", "-qm", "secret"]));
    assert!(run_test_git(
        workspace.path(),
        &["worktree", "add", "wt", "-b", "feature"]
    ));

    let backend = credential_boundary_backend(workspace.path());
    let error = backend
        .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
            path: workspace.path().join("wt").display().to_string(),
            force: true,
        })
        .await
        .expect_err("worktree remove must not delete a credential file");
    assert!(error.to_string().contains("credential boundary"), "{error}");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("wt/.env")).unwrap(),
        "TOKEN=remove-wt-6b20\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".env")).unwrap(),
        "TOKEN=remove-wt-6b20\n"
    );
}

fn init_worktree_repo(root: &Path) {
    // Name the branch explicitly. `git init` follows the runner's
    // `init.defaultBranch`, which is not always `main`.
    assert!(run_test_git(root, &["init", "-b", "main"]));
    assert!(run_test_git(
        root,
        &["config", "user.email", "a3s@example.com"]
    ));
    assert!(run_test_git(root, &["config", "user.name", "a3s"]));
    std::fs::write(root.join("README.md"), "v1\n").unwrap();
    assert!(run_test_git(root, &["add", "README.md"]));
    assert!(run_test_git(root, &["commit", "-m", "one"]));
}

#[tokio::test]
async fn read_text_refuses_a_file_over_the_byte_bound() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let path = services.normalize_path("big.log").unwrap();
    // One byte past the bound; a no-newline body also pins that the check
    // fires before any read is attempted, not after a failed materialize.
    let oversized = "a".repeat(MAX_TEXT_FILE_BYTES as usize + 1);
    std::fs::write(temp.path().join("big.log"), &oversized).unwrap();

    match services.fs().read_text(&path).await {
        Err(WorkspaceError::TooLarge {
            path,
            size_bytes,
            max_bytes,
        }) => {
            assert_eq!(path, "big.log");
            assert_eq!(size_bytes, MAX_TEXT_FILE_BYTES + 1);
            assert_eq!(max_bytes, MAX_TEXT_FILE_BYTES);
        }
        other => panic!("expected TooLarge, got {other:?}",),
    }
}

#[tokio::test]
async fn read_text_reads_a_file_at_exactly_the_byte_bound() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let path = services.normalize_path("edge.txt").unwrap();
    let at_bound = "a".repeat(MAX_TEXT_FILE_BYTES as usize);
    std::fs::write(temp.path().join("edge.txt"), &at_bound).unwrap();

    let content = services.fs().read_text(&path).await.unwrap();
    assert_eq!(content.len(), MAX_TEXT_FILE_BYTES as usize);
}

#[tokio::test]
async fn read_text_range_refuses_the_same_oversized_file() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    let path = services.normalize_path("one-line.log").unwrap();
    // A single line with no newlines is the ranged reader's unbounded
    // case; the whole-file bound must refuse it before any line lands.
    let oversized = "b".repeat(MAX_TEXT_FILE_BYTES as usize + 512);
    std::fs::write(temp.path().join("one-line.log"), &oversized).unwrap();

    let reader = services
        .text_reader()
        .expect("local backend has a text reader");
    match reader.read_text_range(&path, 0, 10).await {
        Err(WorkspaceError::TooLarge { path, .. }) => assert_eq!(path, "one-line.log"),
        other => panic!("expected TooLarge, got {other:?}",),
    }
}

#[cfg(unix)]
fn make_fifo(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let fifo = dir.join(name);
    let fifo_c =
        std::ffi::CString::new(fifo.to_str().expect("utf-8 fifo path")).expect("fifo path cstring");
    // SAFETY: mkfifo only creates a named pipe inside the temp workspace.
    let status = unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o644) };
    assert_eq!(status, 0, "mkfifo must succeed for the fixture");
    fifo
}

#[cfg(unix)]
#[tokio::test]
async fn read_text_refuses_a_nonregular_file_without_opening_it() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    // A writer-less FIFO would block the open itself — the refusal must
    // return before that, proving the gate runs pre-open.
    make_fifo(temp.path(), "pipe.fifo");
    let path = services.normalize_path("pipe.fifo").unwrap();

    match services.fs().read_text(&path).await {
        Err(WorkspaceError::Backend(error)) => {
            let message = error.to_string();
            assert!(
                message.contains("not a regular file"),
                "refusal names the reason: {message}"
            );
        }
        other => panic!("expected a not-a-regular-file refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn read_text_range_refuses_a_nonregular_file_without_opening_it() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    make_fifo(temp.path(), "pipe.fifo");
    let path = services.normalize_path("pipe.fifo").unwrap();

    let reader = services
        .text_reader()
        .expect("local backend has a text reader");
    match reader.read_text_range(&path, 0, 10).await {
        Err(WorkspaceError::Backend(error)) => {
            let message = error.to_string();
            assert!(
                message.contains("not a regular file"),
                "refusal names the reason: {message}"
            );
        }
        other => panic!("expected a not-a-regular-file refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn grep_skips_oversize_and_nonregular_files_and_still_matches_the_rest() {
    let temp = tempfile::tempdir().unwrap();
    let services = WorkspaceServices::local(temp.path());
    std::fs::write(temp.path().join("hit.txt"), "needle in the normal file\n").unwrap();
    // Oversize: past the text bound, outside the searchable domain.
    let oversized = "n".repeat(MAX_TEXT_FILE_BYTES as usize + 1);
    std::fs::write(temp.path().join("big.log"), &oversized).unwrap();
    // Non-regular: a writer-less FIFO must not hang or fail the walk.
    make_fifo(temp.path(), "pipe.fifo");

    let search = services.search().expect("local backend supports search");
    let grep = search
        .grep(WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "needle".to_string(),
            glob: None,
            context_lines: 0,
            case_insensitive: false,
            max_output_size: 1024,
        })
        .await
        .expect("the walk tolerates oversize and non-regular files");
    assert_eq!(grep.match_count, 1, "only the normal file matches");
    assert_eq!(grep.file_count, 1);
    assert!(grep.output.contains("hit.txt:1"));
    assert!(!grep.output.contains("big.log"));
}
