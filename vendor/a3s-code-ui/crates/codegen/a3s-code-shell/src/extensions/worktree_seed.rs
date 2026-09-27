//! Optional fetch-before and branch-after steps for
//! `x.ai/git/worktree/create_from_worktree_sync`. Best effort: the worktree
//! is created either way.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::worktree::WORKTREE_EXT_LOG as WORKTREE_SEED_LOG;

const FETCH_TIMEOUT: Duration = Duration::from_secs(180);
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(300);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// SIGTERM lets git drop its lock files; SIGKILL after this does not.
const TERM_GRACE: Duration = Duration::from_secs(5);
/// Per-pipe cap on captured git output; stdout here is a ref or a commit id.
const OUTPUT_CAP: u64 = 64 * 1024;

/// Parsed from the same params as `CreateWorktreeFromWorktreeRequest`.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeedOptions {
    /// Remote-tracking ref (`origin/main`): fetched in the source repository
    /// before the copy, and the start point of `branch`.
    #[serde(default)]
    pub base_ref: Option<String>,
    /// `git checkout -B` target in the new worktree.
    #[serde(default)]
    pub branch: Option<String>,
}

impl SeedOptions {
    /// Keeps option-like or malformed spellings off the git command line.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(base_ref) = &self.base_ref {
            validate_ref_spelling("baseRef", base_ref)?;
        }
        if let Some(branch) = &self.branch {
            validate_ref_spelling("branch", branch)?;
        }
        Ok(())
    }
}

fn validate_ref_spelling(field: &str, value: &str) -> Result<(), String> {
    let ok = !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.ends_with(".lock")
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!("{field} is not a valid ref name: {value:?}"))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BranchOutcome {
    pub branch: String,
    /// `baseRef`, or `HEAD` after the dirty-conflict fallback.
    pub start_point: String,
    /// HEAD after the checkout; the create-time `commit` is stale once the branch step moved it.
    pub commit: String,
}

/// Fetch failures are logged and swallowed: a stale base is still a usable base.
pub(crate) async fn fetch_base_ref(source_repo: &Path, base_ref: &str) -> bool {
    let Some((remote, branch)) = base_ref.split_once('/') else {
        tracing::info!(
            target: WORKTREE_SEED_LOG,
            base_ref,
            "worktree seed: baseRef has no remote component, skipping fetch"
        );
        return false;
    };
    match run_git(
        source_repo,
        &["fetch", "--no-tags", "--", remote, branch],
        FETCH_TIMEOUT,
    )
    .await
    {
        Ok(_) => {
            tracing::info!(target: WORKTREE_SEED_LOG, base_ref, source = %source_repo.display(), "worktree seed: fetched base ref");
            true
        }
        Err(err) => {
            tracing::warn!(target: WORKTREE_SEED_LOG, base_ref, source = %source_repo.display(), error = %err, "worktree seed: fetch failed, continuing with the local ref");
            false
        }
    }
}

/// Judged by the resulting HEAD, not git's exit status: a failing
/// `post-checkout` hook (git-lfs installs one) exits non-zero after the
/// switch already happened. Retries at HEAD only when the checkout at
/// `base_ref` left HEAD unmoved (the copied dirty edits conflicted). Only for
/// a worktree this request created: `-B` resets an existing tree.
pub(crate) async fn checkout_branch(
    worktree: &Path,
    branch: &str,
    base_ref: Option<&str>,
) -> Option<BranchOutcome> {
    let head_before = match git_head(worktree).await {
        Ok(head) => head,
        Err(err) => {
            tracing::warn!(target: WORKTREE_SEED_LOG, branch, worktree = %worktree.display(), error = %err, "worktree seed: cannot read HEAD, branch step skipped");
            return None;
        }
    };

    let mut start_point = None;
    if let Some(base) = base_ref {
        let base_commit = format!("{base}^{{commit}}");
        match run_git(
            worktree,
            &["rev-parse", "--verify", &base_commit],
            READ_TIMEOUT,
        )
        .await
        {
            Ok(want) => {
                let exit = run_git(
                    worktree,
                    &checkout_args(branch, Some(base)),
                    CHECKOUT_TIMEOUT,
                )
                .await;
                match branch_state(worktree, branch).await {
                    Ok((true, head)) if head == want => start_point = Some(base),
                    Ok((_, head)) if head == head_before => {
                        tracing::warn!(target: WORKTREE_SEED_LOG, branch, base, worktree = %worktree.display(), error = ?exit.err(), "worktree seed: checkout at base ref left HEAD unmoved, retrying at HEAD");
                    }
                    Ok((on_branch, head)) => {
                        tracing::warn!(target: WORKTREE_SEED_LOG, branch, base, on_branch, head, worktree = %worktree.display(), error = ?exit.err(), "worktree seed: checkout at base ref left the tree in an unexpected state");
                        return None;
                    }
                    Err(err) => {
                        tracing::warn!(target: WORKTREE_SEED_LOG, branch, base, worktree = %worktree.display(), error = %err, "worktree seed: cannot read the tree state after checkout");
                        return None;
                    }
                }
            }
            Err(err) => {
                tracing::warn!(target: WORKTREE_SEED_LOG, branch, base, worktree = %worktree.display(), error = %err, "worktree seed: base ref does not resolve, branching at HEAD");
            }
        }
    }

    if start_point.is_none() {
        let exit = run_git(worktree, &checkout_args(branch, None), CHECKOUT_TIMEOUT).await;
        match branch_state(worktree, branch).await {
            Ok((true, head)) if head == head_before => start_point = Some("HEAD"),
            Ok((on_branch, head)) => {
                tracing::warn!(target: WORKTREE_SEED_LOG, branch, on_branch, head, worktree = %worktree.display(), error = ?exit.err(), "worktree seed: branch step failed, worktree stays detached");
                return None;
            }
            Err(err) => {
                tracing::warn!(target: WORKTREE_SEED_LOG, branch, worktree = %worktree.display(), error = %err, "worktree seed: cannot read the tree state after checkout");
                return None;
            }
        }
    }
    let start_point = start_point?;

    // A killed checkout leaves `index.lock` behind over a half-updated tree.
    match index_lock_present(worktree).await {
        Ok(false) => {}
        Ok(true) => {
            tracing::warn!(target: WORKTREE_SEED_LOG, branch, worktree = %worktree.display(), "worktree seed: index.lock left behind, not reporting the branch");
            return None;
        }
        Err(err) => {
            tracing::warn!(target: WORKTREE_SEED_LOG, branch, worktree = %worktree.display(), error = %err, "worktree seed: cannot locate index.lock");
            return None;
        }
    }
    let commit = match git_head(worktree).await {
        Ok(head) => head,
        Err(err) => {
            tracing::warn!(target: WORKTREE_SEED_LOG, branch, worktree = %worktree.display(), error = %err, "worktree seed: branch created but HEAD could not be read");
            return None;
        }
    };
    tracing::info!(target: WORKTREE_SEED_LOG, branch, start_point, commit, worktree = %worktree.display(), "worktree seed: branch created");
    Some(BranchOutcome {
        branch: branch.to_owned(),
        start_point: start_point.to_owned(),
        commit,
    })
}

fn checkout_args<'a>(branch: &'a str, base: Option<&'a str>) -> Vec<&'a str> {
    // `checkout.workers=0` is one worker per logical core.
    let mut args = vec!["-c", "checkout.workers=0", "checkout", "-B", branch];
    args.extend(base);
    args.push("--");
    args
}

async fn git_head(worktree: &Path) -> Result<String, String> {
    run_git(worktree, &["rev-parse", "--verify", "HEAD"], READ_TIMEOUT).await
}

/// `(HEAD is `branch`, HEAD commit)`.
async fn branch_state(worktree: &Path, branch: &str) -> Result<(bool, String), String> {
    let head = git_head(worktree).await?;
    let current = run_git(
        worktree,
        &["symbolic-ref", "-q", "--short", "HEAD"],
        READ_TIMEOUT,
    )
    .await
    .ok();
    Ok((current.as_deref() == Some(branch), head))
}

async fn index_lock_present(worktree: &Path) -> Result<bool, String> {
    let index = run_git(
        worktree,
        &["rev-parse", "--git-path", "index"],
        READ_TIMEOUT,
    )
    .await?;
    let lock = worktree.join(format!("{index}.lock"));
    Ok(lock.exists())
}

/// Runs git with the crate-wide prompt/LFS suppression (`git_command_locking`)
/// in its own process group; on timeout SIGTERM first, SIGKILL after
/// [`TERM_GRACE`]. Returns trimmed stdout.
async fn run_git(cwd: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let label = format!("git {}", args.join(" "));
    let mut cmd = tokio::process::Command::from(a3s_tty_utils::git_command_locking());
    cmd.args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[allow(clippy::disallowed_methods)] // enrolled in the global ProcessScope right below
    let mut child = cmd.spawn().map_err(|err| format!("{label}: {err}"))?;
    let group = match a3s_code_tools::util::global_process_scope().enroll(&child) {
        Ok(group) => group,
        Err(err) => {
            // The scope is closed (the child is already killed) or the child is gone.
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(format!("{label}: {err}"));
        }
    };
    let mut guard = GracefulStop {
        group,
        exited: false,
    };
    let stdout = tokio::spawn(read_to_end(child.stdout.take()));
    let stderr = tokio::spawn(read_to_end(child.stderr.take()));
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => return Err(format!("{label}: {err}")),
        Err(_) => {
            let _ = guard.group.terminate();
            if tokio::time::timeout(TERM_GRACE, child.wait())
                .await
                .is_err()
            {
                let _ = guard.group.kill();
                let _ = child.wait().await;
            }
            guard.exited = true;
            return Err(format!("{label} timed out after {}s", timeout.as_secs()));
        }
    };
    guard.exited = true;
    let stdout = stdout.await.unwrap_or_default();
    if status.success() {
        return Ok(stdout.trim().to_owned());
    }
    let stderr: String = stderr
        .await
        .unwrap_or_default()
        .trim()
        .chars()
        .take(400)
        .collect();
    // Git echoes the remote URL, credentials included, on transport failures.
    let stderr = a3s_code_workspace::session::git::scrub_git_output(&stderr);
    Err(format!("{label} exited {status}: {stderr}"))
}

/// A `run_git` future dropped mid-flight (the RPC was cancelled) still gives
/// git the SIGTERM grace, so lock files in the source repository are released;
/// `kill_on_drop` would SIGKILL the leader at once and leave them.
struct GracefulStop {
    group: std::sync::Arc<a3s_tty_utils::ProcessGroup>,
    exited: bool,
}

impl Drop for GracefulStop {
    fn drop(&mut self) {
        if self.exited {
            return;
        }
        let _ = self.group.terminate();
        let group = self.group.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    tokio::time::sleep(TERM_GRACE).await;
                    let _ = group.kill();
                });
            }
            Err(_) => {
                let _ = group.kill();
            }
        }
    }
}

/// Keeps the first [`OUTPUT_CAP`] bytes and drains the rest so git never
/// blocks on a full pipe.
async fn read_to_end<R: tokio::io::AsyncRead + Unpin>(reader: Option<R>) -> String {
    use tokio::io::AsyncReadExt;
    let Some(reader) = reader else {
        return String::new();
    };
    let mut buf = Vec::new();
    let mut capped = reader.take(OUTPUT_CAP);
    let _ = capped.read_to_end(&mut buf).await;
    let _ = tokio::io::copy(&mut capped.into_inner(), &mut tokio::io::sink()).await;
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(cwd: &Path, args: &[&str]) {
        crate::test_support::ensure_hermetic_git_on_path();
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@x")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@x")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        crate::test_support::ensure_hermetic_git_on_path();
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn upstream_and_clone(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let upstream = dir.join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        std::fs::write(upstream.join("a.txt"), "one\n").unwrap();
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "one"]);
        let clone = dir.join("clone");
        git(
            dir,
            &[
                "clone",
                "-q",
                upstream.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        std::fs::write(upstream.join("a.txt"), "two\n").unwrap();
        git(&upstream, &["commit", "-q", "-am", "two"]);
        (upstream, clone)
    }

    #[test]
    fn seed_options_parse_beside_the_create_request() {
        let params = r#"{"sourceWorktreePath":"/src","newSessionId":"wt-1","copyMode":"dirty","baseRef":"origin/main","branch":"devbot/wt-1"}"#;
        let opts: SeedOptions = serde_json::from_str(params).unwrap();
        assert_eq!(opts.base_ref.as_deref(), Some("origin/main"));
        assert_eq!(opts.branch.as_deref(), Some("devbot/wt-1"));
        assert!(opts.validate().is_ok());

        let none: SeedOptions = serde_json::from_str(r#"{"sourceWorktreePath":"/src"}"#).unwrap();
        assert_eq!(none, SeedOptions::default());
    }

    #[test]
    fn seed_options_reject_option_like_and_malformed_refs() {
        for bad in [
            "-rf",
            "/abs",
            "trailing/",
            "a..b",
            "a//b",
            "x.lock",
            "a@{1}",
            "has space",
            "",
        ] {
            let opts = SeedOptions {
                base_ref: None,
                branch: Some(bad.to_owned()),
            };
            assert!(opts.validate().is_err(), "{bad:?} must be rejected");
        }
        let opts = SeedOptions {
            base_ref: Some("origin/main".to_owned()),
            branch: Some("devbot/wt-slack-1789750039.509579".to_owned()),
        };
        assert!(opts.validate().is_ok());
    }

    #[tokio::test]
    async fn fetch_base_ref_updates_the_remote_tracking_ref() {
        let dir = tempfile::tempdir().unwrap();
        let (upstream, clone) = upstream_and_clone(dir.path());
        let before = git_stdout(&clone, &["rev-parse", "origin/main"]);
        assert!(fetch_base_ref(&clone, "origin/main").await);
        let after = git_stdout(&clone, &["rev-parse", "origin/main"]);
        assert_ne!(before, after);
        assert_eq!(after, git_stdout(&upstream, &["rev-parse", "main"]));
    }

    #[tokio::test]
    async fn fetch_base_ref_without_remote_component_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        assert!(!fetch_base_ref(&clone, "main").await);
    }

    #[tokio::test]
    async fn fetch_base_ref_failure_is_swallowed() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        assert!(!fetch_base_ref(&clone, "nosuchremote/main").await);
    }

    #[tokio::test]
    async fn run_git_error_text_drops_url_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        git(
            &clone,
            &[
                "remote",
                "add",
                "leaky",
                "https://user:s3cret@127.0.0.1:9/repo.git",
            ],
        );
        let err = run_git(
            &clone,
            &["fetch", "--", "leaky", "main"],
            Duration::from_secs(30),
        )
        .await
        .expect_err("connection refused");
        assert!(!err.contains("s3cret"), "{err}");
    }

    #[tokio::test]
    async fn checkout_branch_starts_at_the_fetched_base_ref() {
        let dir = tempfile::tempdir().unwrap();
        let (upstream, clone) = upstream_and_clone(dir.path());
        assert!(fetch_base_ref(&clone, "origin/main").await);
        let wt = dir.path().join("wt");
        git(
            &clone,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        let outcome = checkout_branch(&wt, "devbot/wt-1", Some("origin/main"))
            .await
            .expect("branch created");
        assert_eq!(outcome.start_point, "origin/main");
        assert_eq!(
            git_stdout(&wt, &["symbolic-ref", "--short", "HEAD"]),
            "devbot/wt-1"
        );
        let upstream_head = git_stdout(&upstream, &["rev-parse", "main"]);
        assert_eq!(git_stdout(&wt, &["rev-parse", "HEAD"]), upstream_head);
        assert_eq!(outcome.commit, upstream_head);
    }

    #[tokio::test]
    async fn checkout_branch_falls_back_to_head_when_dirty_edits_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        assert!(fetch_base_ref(&clone, "origin/main").await);
        let wt = dir.path().join("wt");
        git(
            &clone,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        std::fs::write(wt.join("a.txt"), "local\n").unwrap();
        let head_before = git_stdout(&wt, &["rev-parse", "HEAD"]);
        let outcome = checkout_branch(&wt, "devbot/wt-2", Some("origin/main"))
            .await
            .expect("branch created at HEAD");
        assert_eq!(outcome.start_point, "HEAD");
        assert_eq!(
            git_stdout(&wt, &["symbolic-ref", "--short", "HEAD"]),
            "devbot/wt-2"
        );
        assert_eq!(git_stdout(&wt, &["rev-parse", "HEAD"]), head_before);
        assert_eq!(outcome.commit, head_before);
        assert_eq!(
            std::fs::read_to_string(wt.join("a.txt")).unwrap(),
            "local\n"
        );
    }

    #[tokio::test]
    async fn checkout_branch_without_base_ref_lands_on_head() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        let wt = dir.path().join("wt");
        git(
            &clone,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        let outcome = checkout_branch(&wt, "devbot/wt-3", None)
            .await
            .expect("branch created");
        assert_eq!(outcome.start_point, "HEAD");
        assert_eq!(
            git_stdout(&wt, &["symbolic-ref", "--short", "HEAD"]),
            "devbot/wt-3"
        );
    }

    #[tokio::test]
    async fn checkout_branch_reports_failure_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            checkout_branch(dir.path(), "devbot/wt-4", None)
                .await
                .is_none()
        );
    }

    fn install_post_checkout_hook(clone: &Path, script: &str) {
        let hooks =
            Path::new(&git_stdout(clone, &["rev-parse", "--git-path", "hooks"])).to_path_buf();
        let hooks = if hooks.is_absolute() {
            hooks
        } else {
            clone.join(hooks)
        };
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        std::fs::write(&hook, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checkout_branch_ignores_a_failing_post_checkout_hook() {
        let dir = tempfile::tempdir().unwrap();
        let (upstream, clone) = upstream_and_clone(dir.path());
        assert!(fetch_base_ref(&clone, "origin/main").await);
        let wt = dir.path().join("wt");
        git(
            &clone,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        install_post_checkout_hook(&clone, "#!/bin/sh\nexit 1\n");
        let outcome = checkout_branch(&wt, "devbot/wt-5", Some("origin/main"))
            .await
            .expect("the switch happened even though the hook failed");
        assert_eq!(outcome.start_point, "origin/main");
        assert_eq!(
            outcome.commit,
            git_stdout(&upstream, &["rev-parse", "main"])
        );
        assert_eq!(
            git_stdout(&wt, &["symbolic-ref", "--short", "HEAD"]),
            "devbot/wt-5"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checkout_branch_refuses_a_tree_with_index_lock_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        let wt = dir.path().join("wt");
        git(
            &clone,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
        // Stands in for a checkout killed mid-write.
        install_post_checkout_hook(
            &clone,
            "#!/bin/sh\ntouch \"$(git rev-parse --git-path index).lock\"\n",
        );
        assert!(checkout_branch(&wt, "devbot/wt-6", None).await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_git_caps_output_and_does_not_wedge_on_a_noisy_child() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        let out = run_git(
            &clone,
            &["-c", "alias.noisy=!head -c 5000000 /dev/zero | tr '\\0' x; head -c 5000000 /dev/zero | tr '\\0' y >&2", "noisy"],
            Duration::from_secs(30),
        )
        .await
        .expect("noisy git exits 0");
        assert_eq!(out.len() as u64, OUTPUT_CAP);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_git_dropped_mid_flight_terminates_the_child_group() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        let marker = dir.path().join("started");
        let alias = format!(
            "alias.hang=!touch {} && exec sleep 30",
            marker.to_str().unwrap()
        );
        let task = tokio::spawn(async move {
            let _ = run_git(&clone, &["-c", &alias, "hang"], Duration::from_secs(60)).await;
        });
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(marker.exists(), "git alias never started");
        task.abort();
        let _ = task.await;
        // SIGTERM reaches the whole group, so the `sleep` leader dies well within the grace.
        let pattern = format!("touch {}", marker.to_str().unwrap());
        let deadline = std::time::Instant::now() + TERM_GRACE;
        loop {
            let alive = std::process::Command::new("pgrep")
                .args(["-f", &pattern])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !alive {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "git group survived the SIGTERM grace"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_git_times_out_and_reaps_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let (_upstream, clone) = upstream_and_clone(dir.path());
        let started = std::time::Instant::now();
        let err = run_git(
            &clone,
            &["-c", "alias.hang=!sleep 30", "hang"],
            Duration::from_millis(500),
        )
        .await
        .expect_err("a hung git must time out");
        assert!(err.contains("timed out"), "{err}");
        assert!(
            started.elapsed() < TERM_GRACE + Duration::from_secs(5),
            "child was not reaped promptly: {:?}",
            started.elapsed()
        );
    }
}
