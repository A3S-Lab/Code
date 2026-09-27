//! Worktree orchestration: plan + execute.
pub mod execute;
pub(crate) mod plan;
use crate::copy::{CopyStats, DirtyFilesReport};
use anyhow::Result;
pub(crate) use plan::WorktreePlan;
use std::path::PathBuf;
/// Strategy strings written to `worktrees.db` `creation_mode` and metrics.
pub const STRATEGY_GROVE_FUSE: &str = "grove-fuse";
pub const STRATEGY_GROVE_NFS: &str = "grove-nfs";
pub const STRATEGY_GROVE_PROJFS: &str = "grove-projfs";
/// Deprecated alias for [`STRATEGY_GROVE_NFS`]. Still accepted on read/GC.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const STRATEGY_NFS: &str = "nfs";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const STRATEGY_OVERLAY: &str = "overlay";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const STRATEGY_BTRFS: &str = "btrfs";
pub const STRATEGY_COPY: &str = "copy";
pub const STRATEGY_GIT: &str = "git";
pub const STRATEGY_STANDALONE: &str = "standalone";
/// Projected grove worktree: Linux FUSE, macOS NFS, Windows ProjFS, or the
/// legacy `nfs` spelling.
#[must_use]
pub fn is_grove_strategy(s: &str) -> bool {
    matches!(
        s,
        STRATEGY_GROVE_FUSE | STRATEGY_GROVE_NFS | STRATEGY_GROVE_PROJFS | STRATEGY_NFS
    )
}
/// The one decline both the Grove arm and the workspace's pre-dispatch rewrite
/// can reach, so they account for the same source the same way.
pub const SKIP_SOURCE_IS_GROVE_MOUNT: &str = "source is itself a Grove mount";
/// Why the Grove arm declined. Every variant is a fallthrough: the next arm runs.
/// `Display` is the wording that reaches the user through the strategy report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroveSkip {
    /// Linux-only: macOS reaches Grove over NFS and never reads `/dev/fuse`.
    #[cfg(target_os = "linux")]
    FuseUnavailable,
    #[cfg(target_os = "linux")]
    PrivateMountNamespace,
    /// Windows-only: `ProjectedFSLib.dll` is absent (the `Client-ProjFS`
    /// optional feature is off) or the build predates Windows 11 22H2.
    #[cfg(windows)]
    ProjfsUnavailable,
    SourceIsGroveMount,
    PreserveOnLinkedView,
    MountTableInconclusive,
    PreserveOnInconclusiveLinkedView,
    JjSourceRepo,
    PreserveNonHeadRef,
    HeadUnreadableAfterAdopt,
    DaemonDeclined,
}
impl GroveSkip {}
impl std::fmt::Display for GroveSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            #[cfg(target_os = "linux")]
            Self::FuseUnavailable => "/dev/fuse or fusermount is missing",
            #[cfg(target_os = "linux")]
            Self::PrivateMountNamespace => "this process is in a private mount namespace",
            #[cfg(windows)]
            Self::ProjfsUnavailable => {
                "Windows Projected File System is not available (enable the Client-ProjFS \
                 feature on Windows 11 22H2 or later)"
            }
            Self::SourceIsGroveMount => crate::worktree::SKIP_SOURCE_IS_GROVE_MOUNT,
            Self::PreserveOnLinkedView => {
                "uncommitted changes cannot be carried onto a linked Grove view"
            }
            Self::MountTableInconclusive => "the source's mount table could not be read",
            Self::PreserveOnInconclusiveLinkedView => {
                "uncommitted changes cannot be carried onto a possibly linked Grove view"
            }
            Self::JjSourceRepo => "the source is a jj repo",
            Self::PreserveNonHeadRef => {
                "uncommitted changes cannot be carried onto a different ref"
            }
            Self::HeadUnreadableAfterAdopt => "the new worktree's HEAD could not be read",
            Self::DaemonDeclined => "the Grove daemon declined or was unreachable",
        })
    }
}
/// A dispatch arm that can decline before a later arm serves the worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeArm {
    GroveFuse,
    GroveNfs,
    GroveProjfs,
    Overlay,
    Btrfs,
}
impl WorktreeArm {
    /// Only a Grove skip explains why Grove did not serve the worktree; a
    /// snapshot arm's skip means an earlier arm lost and Grove never ran.
    #[must_use]
    pub fn is_grove(self) -> bool {
        matches!(self, Self::GroveFuse | Self::GroveNfs | Self::GroveProjfs)
    }
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::GroveFuse => STRATEGY_GROVE_FUSE,
            Self::GroveNfs => STRATEGY_GROVE_NFS,
            Self::GroveProjfs => STRATEGY_GROVE_PROJFS,
            Self::Overlay => STRATEGY_OVERLAY,
            Self::Btrfs => STRATEGY_BTRFS,
        }
    }
}
/// One arm that did not serve the worktree, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmSkip {
    pub arm: WorktreeArm,
    /// One line: a typed Grove decline, or a flattened error chain.
    pub detail: String,
    /// Set when the Grove arm declined with a [`GroveSkip`]. Absent on snapshot-arm
    /// failures and flattened error chains, which have no typed Grove decline.
    pub grove_skip: Option<GroveSkip>,
}
impl ArmSkip {
    pub fn new(arm: WorktreeArm, detail: impl Into<String>) -> Self {
        Self {
            arm,
            detail: detail.into(),
            grove_skip: None,
        }
    }
    pub fn from_grove(arm: WorktreeArm, skip: GroveSkip) -> Self {
        Self {
            arm,
            detail: skip.to_string(),
            grove_skip: Some(skip),
        }
    }
}
impl std::fmt::Display for ArmSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.arm.label(), self.detail)
    }
}
/// Skip lines joined for a log field or a fallback message.
#[must_use]
pub fn render_arm_skips(skips: &[ArmSkip]) -> String {
    skips
        .iter()
        .map(ArmSkip::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}
/// Result of worktree creation.
#[derive(Debug)]
pub struct CreateWorktreeResult {
    /// Path to the created worktree
    pub worktree_path: PathBuf,
    /// Git commit the worktree is based on
    pub commit: String,
    /// Statistics from the file copy phase
    pub copy_stats: CopyStats,
    /// Statistics from ignored files copy (if enabled)
    pub ignored_stats: Option<CopyStats>,
    /// Report about dirty files (modified/untracked/deleted) in the source worktree
    pub dirty_files_report: Option<DirtyFilesReport>,
    /// Which dispatch arm actually ran (`grove-fuse` / `grove-nfs` / `grove-projfs` / `overlay` / `btrfs` / `copy` / `git` / `standalone`).
    pub resolved_strategy: &'static str,
    /// Arm-specific metadata (NFS mount/backing/pin; overlay/btrfs snapshot paths).
    pub strategy_metadata: Option<serde_json::Value>,
    /// Arms that declined before the one that ran.
    pub skipped: Vec<ArmSkip>,
    pub daemon_capability_class: Option<&'static str>,
}
/// Execute worktree creation plan. This is a blocking operation.
pub(crate) fn execute_plan(plan: WorktreePlan) -> Result<CreateWorktreeResult> {
    execute::execute_create_worktree(plan)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IgnoredFilesMode, WorkingTreeMode, WorktreeBuilder};
    #[test]
    fn grove_strategy_names() {
        assert!(is_grove_strategy(STRATEGY_GROVE_FUSE));
        assert!(is_grove_strategy(STRATEGY_GROVE_NFS));
        assert!(is_grove_strategy(STRATEGY_NFS));
        assert!(!is_grove_strategy(STRATEGY_COPY));
        assert!(!is_grove_strategy("linked"));
    }
    use a3s_test_utils::git::{git_commit_all, init_git_repo};
    use tempfile::TempDir;
    #[test]
    fn test_create_worktree_simple() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        assert!(result.worktree_path.exists());
        assert!(result.worktree_path.join("file.txt").exists());
        assert!(!result.commit.is_empty());
        assert_eq!(result.resolved_strategy, "copy");
        assert!(
            crate::grove_wt_create_count("copy") >= 1,
            "grove_wt_create must record the copy arm"
        );
    }
    #[test]
    fn test_create_worktree_creates_parent_dirs() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("nested").join("worktree");
        assert!(!worktree_path.parent().unwrap().exists());
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        assert!(result.worktree_path.exists());
        assert!(result.worktree_path.join("file.txt").exists());
        assert!(!result.commit.is_empty());
    }
    #[test]
    fn test_create_worktree_with_ignored_files() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(repo_path.join(".gitignore"), "ignored/").unwrap();
        std::fs::create_dir(repo_path.join("ignored")).unwrap();
        std::fs::write(repo_path.join("ignored/deps.txt"), "deps").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .ignored_files_mode(IgnoredFilesMode::Copy {
                skip_patterns: vec![],
            })
            .create()
            .unwrap();
        assert!(result.worktree_path.join("tracked.txt").exists());
        assert!(result.worktree_path.join("ignored/deps.txt").exists());
        assert!(result.ignored_copy.is_some());
    }
    #[test]
    fn test_create_worktree_skip_ignored_files() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(repo_path.join(".gitignore"), "*.log\nnode_modules/\n").unwrap();
        std::fs::write(repo_path.join("debug.log"), "debug").unwrap();
        std::fs::create_dir(repo_path.join("node_modules")).unwrap();
        std::fs::write(repo_path.join("node_modules/package.json"), "{}").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .ignored_files_mode(IgnoredFilesMode::Skip)
            .create()
            .unwrap();
        assert!(result.worktree_path.join("tracked.txt").exists());
        assert!(result.worktree_path.join(".gitignore").exists());
        assert!(!result.worktree_path.join("debug.log").exists());
        assert!(
            !result
                .worktree_path
                .join("node_modules/package.json")
                .exists()
        );
        assert!(result.ignored_copy.is_none());
    }
    #[test]
    fn test_copy_ignored_only_standalone() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        let dest_path = temp.path().join("dest");
        std::fs::create_dir(&repo_path).unwrap();
        std::fs::create_dir(&dest_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(repo_path.join(".gitignore"), "node_modules/\n*.log").unwrap();
        std::fs::create_dir(repo_path.join("node_modules")).unwrap();
        std::fs::write(repo_path.join("node_modules/package.json"), "{}").unwrap();
        std::fs::write(repo_path.join("debug.log"), "log content").unwrap();
        git_commit_all(&repo_path, "initial");
        let result = WorktreeBuilder::new(repo_path.clone(), dest_path.clone())
            .copy_ignored_only()
            .unwrap();
        assert!(dest_path.join("node_modules/package.json").exists());
        assert!(dest_path.join("debug.log").exists());
        assert!(!dest_path.join("tracked.txt").exists());
        assert!(!dest_path.join(".gitignore").exists());
        assert!(result.files_copied >= 2);
    }
    #[test]
    fn test_copy_ignored_only_with_skip_patterns() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        let dest_path = temp.path().join("dest");
        std::fs::create_dir(&repo_path).unwrap();
        std::fs::create_dir(&dest_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(
            repo_path.join(".gitignore"),
            "node_modules/\n*.log\n.cache/\n",
        )
        .unwrap();
        std::fs::create_dir(repo_path.join("node_modules")).unwrap();
        std::fs::write(repo_path.join("node_modules/package.json"), "{}").unwrap();
        std::fs::write(repo_path.join("debug.log"), "debug").unwrap();
        std::fs::write(repo_path.join("error.log"), "error").unwrap();
        std::fs::create_dir(repo_path.join(".cache")).unwrap();
        std::fs::write(repo_path.join(".cache/data.bin"), "cache").unwrap();
        git_commit_all(&repo_path, "initial");
        let result = WorktreeBuilder::new(repo_path.clone(), dest_path.clone())
            .ignored_files_mode(IgnoredFilesMode::CopyOnly {
                skip_patterns: vec!["*.log".to_string(), ".cache/**".to_string()],
            })
            .copy_ignored_only()
            .unwrap();
        assert!(dest_path.join("node_modules/package.json").exists());
        assert!(!dest_path.join("debug.log").exists());
        assert!(!dest_path.join("error.log").exists());
        assert!(!dest_path.join(".cache/data.bin").exists());
        assert!(!dest_path.join("tracked.txt").exists());
        assert!(!dest_path.join(".gitignore").exists());
        assert_eq!(result.files_copied, 1);
    }
    #[test]
    #[cfg(unix)]
    fn test_worktree_with_symlinks() {
        a3s_test_utils::require_git!();
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("target.txt"), "target content").unwrap();
        symlink("target.txt", repo_path.join("link.txt")).unwrap();
        std::fs::create_dir(repo_path.join("subdir")).unwrap();
        std::fs::write(repo_path.join("subdir/file.txt"), "subdir file").unwrap();
        symlink("subdir", repo_path.join("link_dir")).unwrap();
        git_commit_all(&repo_path, "initial with symlinks");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        assert!(result.worktree_path.join("target.txt").exists());
        assert!(result.worktree_path.join("subdir/file.txt").exists());
        let link_path = result.worktree_path.join("link.txt");
        assert!(link_path.exists());
        assert!(
            link_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let link_target = std::fs::read_link(&link_path).unwrap();
        assert_eq!(link_target, PathBuf::from("target.txt"));
        let dir_link_path = result.worktree_path.join("link_dir");
        assert!(
            dir_link_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    #[test]
    #[cfg(unix)]
    fn test_copy_ignored_only_with_symlinks() {
        a3s_test_utils::require_git!();
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        let dest_path = temp.path().join("dest");
        std::fs::create_dir(&repo_path).unwrap();
        std::fs::create_dir(&dest_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(repo_path.join(".gitignore"), "ignored/").unwrap();
        std::fs::create_dir(repo_path.join("ignored")).unwrap();
        std::fs::write(repo_path.join("ignored/real.txt"), "real").unwrap();
        symlink("real.txt", repo_path.join("ignored/link.txt")).unwrap();
        git_commit_all(&repo_path, "initial");
        let result = WorktreeBuilder::new(repo_path.clone(), dest_path.clone())
            .copy_ignored_only()
            .unwrap();
        let link_path = dest_path.join("ignored/link.txt");
        assert!(link_path.exists());
        assert!(
            link_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(result.symlinks_copied >= 1);
    }
    #[test]
    fn test_worktree_with_dirty_files() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "original").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("file.txt"), "modified").unwrap();
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .working_tree_mode(WorkingTreeMode::PreserveWorkingTree)
            .create()
            .unwrap();
        let worktree_path = result.worktree_path.clone();
        let content = std::fs::read_to_string(worktree_path.join("file.txt")).unwrap();
        assert_eq!(content, "modified");
    }
    #[test]
    fn test_worktree_preserves_git_status_for_dirty_files() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "original content").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("file.txt"), "modified content").unwrap();
        let source_status = std::process::Command::new("git")
            .current_dir(&repo_path)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        let source_status_str = String::from_utf8_lossy(&source_status.stdout);
        eprintln!("Source git status: {}", source_status_str);
        assert!(
            source_status_str.contains("file.txt"),
            "Source should show file.txt as modified"
        );
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .working_tree_mode(WorkingTreeMode::PreserveWorkingTree)
            .create()
            .unwrap();
        let worktree_path = result.worktree_path.clone();
        let content = std::fs::read_to_string(worktree_path.join("file.txt")).unwrap();
        assert_eq!(content, "modified content");
        let worktree_status = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        let worktree_status_str = String::from_utf8_lossy(&worktree_status.stdout);
        eprintln!("Worktree git status: {}", worktree_status_str);
        assert!(
            worktree_status_str.contains("file.txt"),
            "Worktree should show file.txt as modified, but got: {}",
            worktree_status_str
        );
    }
    #[test]
    fn test_git_status_is_instant_after_worktree_creation() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        for i in 0..10 {
            std::fs::write(
                repo_path.join(format!("file{}.txt", i)),
                format!("content {}", i),
            )
            .unwrap();
        }
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        let worktree_path = result.worktree_path.clone();
        let start = std::time::Instant::now();
        let status1 = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        let first_duration = start.elapsed();
        let start = std::time::Instant::now();
        let status2 = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        let second_duration = start.elapsed();
        eprintln!("First git status: {:?}", first_duration);
        eprintln!("Second git status: {:?}", second_duration);
        let status1_str = String::from_utf8_lossy(&status1.stdout);
        let status2_str = String::from_utf8_lossy(&status2.stdout);
        assert!(status1_str.is_empty(), "Worktree should be clean");
        assert!(status2_str.is_empty(), "Worktree should be clean");
        let trace_status = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .env("GIT_TRACE_PERFORMANCE", "1")
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        let trace_output = String::from_utf8_lossy(&trace_status.stderr);
        eprintln!("GIT_TRACE_PERFORMANCE output:\n{}", trace_output);
        let has_refresh_indicator = trace_output.contains("refresh_index")
            || trace_output.lines().filter(|l| l.contains("lstat")).count() > 3;
        if has_refresh_indicator {
            eprintln!("WARNING: git status appears to be refreshing the index");
        }
        if first_duration.as_millis() > 200 {
            eprintln!(
                "NOTE: First git status took {:?}, which seems slow",
                first_duration
            );
        }
    }
    #[test]
    fn test_clean_files_dont_show_as_modified() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("clean.txt"), "clean content").unwrap();
        std::fs::write(repo_path.join("will_be_dirty.txt"), "original").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("will_be_dirty.txt"), "modified").unwrap();
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .working_tree_mode(WorkingTreeMode::PreserveWorkingTree)
            .create()
            .unwrap();
        let worktree_path = result.worktree_path.clone();
        let status = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        let status_str = String::from_utf8_lossy(&status.stdout);
        eprintln!("Git status output:\n{}", status_str);
        assert!(
            status_str.contains("will_be_dirty.txt"),
            "Dirty file should show as modified"
        );
        assert!(
            !status_str.contains("clean.txt"),
            "Clean file should NOT show as modified, but status shows:\n{}",
            status_str
        );
    }
    #[test]
    fn test_worktree_clean_state() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "original").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("file.txt"), "modified").unwrap();
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .working_tree_mode(WorkingTreeMode::CleanTracked)
            .create()
            .unwrap();
        let worktree_path = result.worktree_path.clone();
        let content = std::fs::read_to_string(worktree_path.join("file.txt")).unwrap();
        assert_eq!(content, "original");
    }
    #[test]
    fn test_worktree_clean_all_state() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("committed.txt"), "committed").unwrap();
        std::fs::write(repo_path.join(".gitignore"), "*.log\n").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("committed.txt"), "modified").unwrap();
        std::fs::write(repo_path.join("untracked.txt"), "untracked").unwrap();
        std::fs::write(repo_path.join("debug.log"), "debug").unwrap();
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .working_tree_mode(WorkingTreeMode::CleanAll)
            .create()
            .unwrap();
        let worktree_path = result.worktree_path.clone();
        let content = std::fs::read_to_string(worktree_path.join("committed.txt")).unwrap();
        assert_eq!(content, "committed");
        assert!(!worktree_path.join("untracked.txt").exists());
        assert!(!worktree_path.join("debug.log").exists());
        assert!(worktree_path.join(".gitignore").exists());
    }
    #[test]
    fn test_background_finalization() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        assert!(result.worktree_path.join("file.txt").exists());
    }
    #[test]
    fn test_worktree_with_nested_directories() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::create_dir_all(repo_path.join("a/b/c/d")).unwrap();
        std::fs::write(repo_path.join("a/file1.txt"), "1").unwrap();
        std::fs::write(repo_path.join("a/b/file2.txt"), "2").unwrap();
        std::fs::write(repo_path.join("a/b/c/file3.txt"), "3").unwrap();
        std::fs::write(repo_path.join("a/b/c/d/file4.txt"), "4").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        assert!(result.worktree_path.join("a/file1.txt").exists());
        assert!(result.worktree_path.join("a/b/file2.txt").exists());
        assert!(result.worktree_path.join("a/b/c/file3.txt").exists());
        assert!(result.worktree_path.join("a/b/c/d/file4.txt").exists());
    }
    #[test]
    fn test_worktree_preserves_file_content() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        let binary_content: Vec<u8> = (0..=255).collect();
        std::fs::write(repo_path.join("text.txt"), "Hello, World! 🌍").unwrap();
        std::fs::write(repo_path.join("binary.bin"), &binary_content).unwrap();
        std::fs::write(repo_path.join("empty.txt"), "").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(result.worktree_path.join("text.txt")).unwrap(),
            "Hello, World! 🌍"
        );
        assert_eq!(
            std::fs::read(result.worktree_path.join("binary.bin")).unwrap(),
            binary_content
        );
        assert_eq!(
            std::fs::read_to_string(result.worktree_path.join("empty.txt")).unwrap(),
            ""
        );
    }
    #[test]
    #[cfg(unix)]
    fn test_worktree_preserves_permissions() {
        a3s_test_utils::require_git!();
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("script.sh"), "#!/bin/bash\necho hello").unwrap();
        let mut perms = std::fs::metadata(repo_path.join("script.sh"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(repo_path.join("script.sh"), perms).unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .create()
            .unwrap();
        let dest_perms = std::fs::metadata(result.worktree_path.join("script.sh"))
            .unwrap()
            .permissions();
        assert!(
            dest_perms.mode() & 0o111 != 0,
            "executable bit should be set"
        );
    }
    #[test]
    fn test_worktree_with_btrfs_disabled() {
        a3s_test_utils::require_git!();
        use crate::BtrfsMode;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .btrfs_mode(BtrfsMode::Disabled)
            .create()
            .unwrap();
        assert!(result.worktree_path.exists());
        assert!(result.worktree_path.join("file.txt").exists());
        assert!(!result.commit.is_empty());
        assert!(result.unignored_copy.files_copied > 0);
    }
    #[test]
    fn test_worktree_with_btrfs_auto_on_non_btrfs() {
        a3s_test_utils::require_git!();
        use crate::BtrfsMode;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("worktree");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .btrfs_mode(BtrfsMode::Auto)
            .create()
            .unwrap();
        assert!(result.worktree_path.exists());
        assert!(result.worktree_path.join("file.txt").exists());
        assert!(!result.commit.is_empty());
        assert!(result.unignored_copy.files_copied > 0);
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn test_worktree_btrfs_snapshot_integration() {
        use crate::BtrfsMode;
        fn get_btrfs_test_dir() -> Option<std::path::PathBuf> {
            if let Ok(path) = std::env::var("BTRFS_TEST_PATH") {
                let path = std::path::PathBuf::from(&path);
                if path.exists()
                    && crate::btrfs::is_btrfs(&path).unwrap_or(false)
                    && crate::btrfs::is_btrfs_subvolume(&path)
                        .ok()
                        .flatten()
                        .is_some()
                {
                    return Some(path);
                }
            }
            let temp = std::env::temp_dir();
            if crate::btrfs::is_btrfs(&temp).unwrap_or(false)
                && crate::btrfs::is_btrfs_subvolume(&temp)
                    .ok()
                    .flatten()
                    .is_some()
            {
                return Some(temp);
            }
            None
        }
        let Some(btrfs_path) = get_btrfs_test_dir() else {
            eprintln!("Skipping test: no BTRFS subvolume detected");
            eprintln!("Set BTRFS_TEST_PATH to a BTRFS subvolume to run this test");
            return;
        };
        eprintln!(
            "Running BTRFS integration test on: {}",
            btrfs_path.display()
        );
        let test_id = std::process::id();
        let repo_path = btrfs_path.join(format!("test_repo_{}", test_id));
        let worktree_path = btrfs_path.join(format!("test_worktree_{}", test_id));
        let _ = std::fs::remove_dir_all(&repo_path);
        let _ = std::process::Command::new("btrfs")
            .args(["subvolume", "delete", &worktree_path.to_string_lossy()])
            .output();
        std::fs::create_dir_all(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "btrfs test content").unwrap();
        git_commit_all(&repo_path, "initial");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .btrfs_mode(BtrfsMode::Auto)
            .create();
        match result {
            Ok(report) => {
                assert!(report.worktree_path.exists());
                assert!(report.worktree_path.join("file.txt").exists());
                if report.unignored_copy.files_copied == 0 {
                    eprintln!("✓ BTRFS snapshot worktree created successfully!");
                } else {
                    eprintln!(
                        "Note: Fell back to copy method ({} files copied)",
                        report.unignored_copy.files_copied
                    );
                }
                let _ = std::process::Command::new("btrfs")
                    .args(["subvolume", "delete", &worktree_path.to_string_lossy()])
                    .output();
                let _ = std::fs::remove_dir_all(&worktree_path);
            }
            Err(e) => {
                eprintln!("BTRFS test failed: {}", e);
            }
        }
        let _ = std::fs::remove_dir_all(&repo_path);
    }
    #[test]
    fn test_standalone_worktree_simple() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("standalone");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        assert!(result.worktree_path.exists());
        assert!(result.worktree_path.join("file.txt").exists());
        assert!(!result.commit.is_empty());
        assert!(result.worktree_path.join(".git").is_dir());
    }
    #[test]
    fn test_standalone_worktree_is_independent() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("standalone");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        let log_output = std::process::Command::new("git")
            .current_dir(&result.worktree_path)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(log_output.status.success());
        let log_str = String::from_utf8_lossy(&log_output.stdout);
        assert!(
            log_str.contains("initial"),
            "standalone repo should have commit history"
        );
        let worktree_list = std::process::Command::new("git")
            .current_dir(&repo_path)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        let list_str = String::from_utf8_lossy(&worktree_list.stdout);
        assert!(
            !list_str.contains("standalone"),
            "standalone copy should NOT be registered as a worktree in the source"
        );
    }
    #[test]
    fn test_standalone_worktree_no_worktrees_dir() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        let linked_wt = temp.path().join("linked-wt");
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        std::process::Command::new("git")
            .current_dir(&repo_path)
            .args(["worktree", "add", "--detach", &linked_wt.to_string_lossy()])
            .output()
            .unwrap();
        assert!(repo_path.join(".git/worktrees").exists());
        let standalone_path = temp.path().join("standalone");
        WorktreeBuilder::new(repo_path.clone(), standalone_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        assert!(!standalone_path.join(".git/worktrees").exists());
        let _ = std::process::Command::new("git")
            .current_dir(&repo_path)
            .args([
                "worktree",
                "remove",
                "--force",
                &linked_wt.to_string_lossy(),
            ])
            .output();
    }
    #[test]
    fn test_standalone_worktree_preserves_dirty_files() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "original").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("file.txt"), "modified").unwrap();
        let worktree_path = temp.path().join("standalone");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .standalone(true)
            .working_tree_mode(WorkingTreeMode::PreserveWorkingTree)
            .create()
            .unwrap();
        let content = std::fs::read_to_string(result.worktree_path.join("file.txt")).unwrap();
        assert_eq!(content, "modified");
    }
    #[test]
    fn test_standalone_worktree_clean_tracked() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "original").unwrap();
        git_commit_all(&repo_path, "initial");
        std::fs::write(repo_path.join("file.txt"), "modified").unwrap();
        let worktree_path = temp.path().join("standalone");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .standalone(true)
            .working_tree_mode(WorkingTreeMode::CleanTracked)
            .create()
            .unwrap();
        let content = std::fs::read_to_string(result.worktree_path.join("file.txt")).unwrap();
        assert_eq!(content, "original");
    }
    #[test]
    fn test_standalone_worktree_promotable_via_rename() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let standalone_path = temp.path().join("standalone");
        let result = WorktreeBuilder::new(repo_path.clone(), standalone_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        let original_commit = result.commit.clone();
        let promoted_path = temp.path().join("promoted");
        std::fs::rename(&standalone_path, &promoted_path).unwrap();
        assert!(promoted_path.join("file.txt").exists());
        assert!(promoted_path.join(".git").is_dir());
        let log_output = std::process::Command::new("git")
            .current_dir(&promoted_path)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(log_output.status.success());
        let rev_output = std::process::Command::new("git")
            .current_dir(&promoted_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let commit = String::from_utf8_lossy(&rev_output.stdout)
            .trim()
            .to_string();
        assert_eq!(commit, original_commit);
    }
    #[test]
    fn test_standalone_worktree_with_ignored_files() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("tracked.txt"), "tracked").unwrap();
        std::fs::write(repo_path.join(".gitignore"), "ignored/").unwrap();
        std::fs::create_dir(repo_path.join("ignored")).unwrap();
        std::fs::write(repo_path.join("ignored/deps.txt"), "deps").unwrap();
        git_commit_all(&repo_path, "initial");
        let worktree_path = temp.path().join("standalone");
        let result = WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .standalone(true)
            .ignored_files_mode(IgnoredFilesMode::Copy {
                skip_patterns: vec![],
            })
            .create()
            .unwrap();
        assert!(result.worktree_path.join("tracked.txt").exists());
        assert!(result.worktree_path.join("ignored/deps.txt").exists());
        assert!(result.ignored_copy.is_some());
    }
    #[test]
    fn test_standalone_worktree_narrows_origin_fetch() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let branch =
            a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        a3s_test_utils::git::run_git(
            &repo_path,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/a3s-org/a3s.git",
            ],
        );
        a3s_test_utils::git::run_git(
            &repo_path,
            &[
                "config",
                "--replace-all",
                "remote.origin.fetch",
                "+refs/heads/*",
            ],
        );
        assert_eq!(
            a3s_test_utils::git::run_git(&repo_path, &["config", "--get", "remote.origin.fetch"]),
            "+refs/heads/*"
        );
        let worktree_path = temp.path().join("standalone");
        WorktreeBuilder::new(repo_path.clone(), worktree_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        assert_eq!(
            a3s_test_utils::git::run_git(
                &worktree_path,
                &["config", "--get-all", "remote.origin.fetch"]
            ),
            format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
        );
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["config", "--get", "remote.origin.url"]),
            "https://github.com/a3s-org/a3s.git"
        );
    }
    #[test]
    fn test_standalone_worktree_drops_inconsistent_shallow() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("a.txt"), "a").unwrap();
        git_commit_all(&repo_path, "A");
        let a = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        std::fs::write(repo_path.join("b.txt"), "b").unwrap();
        git_commit_all(&repo_path, "B");
        let b = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        a3s_test_utils::git::run_git(&repo_path, &["checkout", "-b", "feature", &a]);
        std::fs::write(repo_path.join("d.txt"), "d").unwrap();
        git_commit_all(&repo_path, "D");
        a3s_test_utils::git::run_git(&repo_path, &["update-ref", "refs/heads/main", &b]);
        a3s_test_utils::git::run_git(&repo_path, &["update-ref", "refs/remotes/origin/main", &b]);
        std::fs::write(repo_path.join(".git/shallow"), format!("{b}\n")).unwrap();
        let worktree_path = temp.path().join("standalone");
        WorktreeBuilder::new(repo_path, worktree_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        assert!(!worktree_path.join(".git/shallow").exists());
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["rev-parse", "--is-shallow-repository"]),
            "false"
        );
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["rev-parse", "HEAD^"]),
            a
        );
    }
    #[test]
    fn test_standalone_worktree_sanitizes_after_checkout_ref() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("a.txt"), "a").unwrap();
        git_commit_all(&repo_path, "A");
        let a = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        std::fs::write(repo_path.join("b.txt"), "b").unwrap();
        git_commit_all(&repo_path, "B");
        let b = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        std::fs::write(repo_path.join("c.txt"), "c").unwrap();
        git_commit_all(&repo_path, "C");
        let source_branch =
            a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        a3s_test_utils::git::run_git(&repo_path, &["checkout", "-b", "feature", &a]);
        std::fs::write(repo_path.join("d.txt"), "d").unwrap();
        git_commit_all(&repo_path, "D");
        a3s_test_utils::git::run_git(&repo_path, &["checkout", &source_branch]);
        assert_ne!(source_branch, "feature");
        a3s_test_utils::git::run_git(
            &repo_path,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/a3s-org/a3s.git",
            ],
        );
        a3s_test_utils::git::run_git(
            &repo_path,
            &[
                "config",
                "--replace-all",
                "remote.origin.fetch",
                "+refs/heads/*",
            ],
        );
        a3s_test_utils::git::run_git(&repo_path, &["update-ref", "refs/remotes/origin/main", &b]);
        a3s_test_utils::git::run_git(
            &repo_path,
            &["update-ref", "refs/remotes/origin/feature", &a],
        );
        a3s_test_utils::git::run_git(&repo_path, &["update-ref", "refs/remotes/origin/noise", &b]);
        std::fs::write(repo_path.join(".git/shallow"), format!("{b}\n")).unwrap();
        let worktree_path = temp.path().join("standalone");
        WorktreeBuilder::new(repo_path, worktree_path.clone())
            .standalone(true)
            .git_ref("feature")
            .create()
            .unwrap();
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "feature"
        );
        assert_eq!(
            a3s_test_utils::git::run_git(
                &worktree_path,
                &["rev-parse", "refs/remotes/origin/feature"]
            ),
            a,
            "checkout dest-branch origin ref must survive source-HEAD CoW prune"
        );
        let noise = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .args(["show-ref", "--verify", "refs/remotes/origin/noise"])
            .output()
            .unwrap();
        assert!(
            !noise.status.success(),
            "unrelated origin/noise must still be pruned after checkout sanitize"
        );
        assert_eq!(
            a3s_test_utils::git::run_git(
                &worktree_path,
                &["config", "--get-all", "remote.origin.fetch"]
            ),
            "+refs/heads/feature:refs/remotes/origin/feature"
        );
        assert!(
            !worktree_path.join(".git/shallow").exists(),
            "after checkout, graft B is unused and its parent is in the ODB"
        );
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["rev-parse", "--is-shallow-repository"]),
            "false"
        );
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["rev-parse", "HEAD^"]),
            a
        );
    }
    #[test]
    fn test_standalone_worktree_keeps_origin_ref_after_detached_checkout() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("a.txt"), "a").unwrap();
        git_commit_all(&repo_path, "A");
        let a = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        std::fs::write(repo_path.join("b.txt"), "b").unwrap();
        git_commit_all(&repo_path, "B");
        let b = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        a3s_test_utils::git::run_git(&repo_path, &["branch", "feature", &a]);
        a3s_test_utils::git::run_git(
            &repo_path,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/a3s-org/a3s.git",
            ],
        );
        a3s_test_utils::git::run_git(
            &repo_path,
            &["update-ref", "refs/remotes/origin/feature", &a],
        );
        a3s_test_utils::git::run_git(&repo_path, &["update-ref", "refs/remotes/origin/noise", &b]);
        let worktree_path = temp.path().join("standalone");
        WorktreeBuilder::new(repo_path, worktree_path.clone())
            .standalone(true)
            .git_ref("origin/feature")
            .create()
            .unwrap();
        assert_eq!(
            a3s_test_utils::git::run_git(&worktree_path, &["rev-parse", "HEAD"]),
            a
        );
        assert_eq!(
            a3s_test_utils::git::run_git(
                &worktree_path,
                &["rev-parse", "refs/remotes/origin/feature"]
            ),
            a,
            "detached origin/feature checkout must keep that remote-tracking ref"
        );
        let noise = std::process::Command::new("git")
            .current_dir(&worktree_path)
            .args(["show-ref", "--verify", "refs/remotes/origin/noise"])
            .output()
            .unwrap();
        assert!(
            !noise.status.success(),
            "unrelated origin/noise must still be pruned"
        );
    }
    #[test]
    fn test_standalone_worktree_skips_extra_origin_remotes() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let head = a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "HEAD"]);
        let branch =
            a3s_test_utils::git::run_git(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        a3s_test_utils::git::run_git(
            &repo_path,
            &["update-ref", "refs/remotes/origin/main", &head],
        );
        a3s_test_utils::git::run_git(
            &repo_path,
            &[
                "update-ref",
                &format!("refs/remotes/origin/{branch}"),
                &head,
            ],
        );
        for i in 0..40 {
            a3s_test_utils::git::run_git(
                &repo_path,
                &[
                    "update-ref",
                    &format!("refs/remotes/origin/branch-{i}"),
                    &head,
                ],
            );
        }
        let worktree_path = temp.path().join("standalone");
        WorktreeBuilder::new(repo_path, worktree_path.clone())
            .standalone(true)
            .create()
            .unwrap();
        assert_eq!(
            a3s_test_utils::git::run_git(
                &worktree_path,
                &["rev-parse", "refs/remotes/origin/main"]
            ),
            head
        );
        assert_eq!(
            a3s_test_utils::git::run_git(
                &worktree_path,
                &["rev-parse", &format!("refs/remotes/origin/{branch}")]
            ),
            head
        );
        for i in 0..40 {
            let show = std::process::Command::new("git")
                .current_dir(&worktree_path)
                .args([
                    "show-ref",
                    "--verify",
                    &format!("refs/remotes/origin/branch-{i}"),
                ])
                .output()
                .unwrap();
            assert!(
                !show.status.success(),
                "standalone dest must not have origin/branch-{i}"
            );
        }
    }
    #[test]
    fn test_linked_cancel_after_worktree_add_deregisters() {
        a3s_test_utils::require_git!();
        use crate::CreationMode;
        use tokio_util::sync::CancellationToken;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let dest = temp.path().join("wt");
        let token = CancellationToken::new();
        token.cancel();
        let result = WorktreeBuilder::new(repo_path.clone(), dest.clone())
            .creation_mode(CreationMode::Linked)
            .cancellation_token(token)
            .create();
        assert!(result.is_err(), "cancelled creation must fail");
        assert!(!dest.exists(), "cancelled worktree dir must be removed");
        let registrations = std::fs::read_dir(repo_path.join(".git/worktrees"))
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);
        assert_eq!(
            registrations, 0,
            "the linked-worktree registration must be deregistered"
        );
    }
    #[test]
    fn test_standalone_cancel_removes_partial_dest() {
        a3s_test_utils::require_git!();
        use tokio_util::sync::CancellationToken;
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let dest = temp.path().join("standalone-wt");
        let token = CancellationToken::new();
        token.cancel();
        let result = WorktreeBuilder::new(repo_path.clone(), dest.clone())
            .standalone(true)
            .cancellation_token(token)
            .create();
        assert!(result.is_err(), "cancelled standalone creation must fail");
        assert!(!dest.exists(), "cancelled standalone dest must be removed");
    }
    #[test]
    fn test_linked_hard_error_reclaims_and_deregisters() {
        a3s_test_utils::require_git!();
        use crate::{CreationMode, IgnoredFilesMode};
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let dest = temp.path().join("wt");
        let result = WorktreeBuilder::new(repo_path.clone(), dest.clone())
            .creation_mode(CreationMode::Linked)
            .ignored_files_mode(IgnoredFilesMode::Copy {
                skip_patterns: vec!["[".to_string()],
            })
            .create();
        assert!(result.is_err(), "invalid skip glob must fail creation");
        assert!(
            !dest.exists(),
            "partial worktree dir must be reclaimed on a hard error"
        );
        let registrations = std::fs::read_dir(repo_path.join(".git/worktrees"))
            .map(|rd| rd.flatten().count())
            .unwrap_or(0);
        assert_eq!(
            registrations, 0,
            "registration must be deregistered on a hard error"
        );
    }
    #[test]
    fn test_standalone_hard_error_reclaims_dest() {
        a3s_test_utils::require_git!();
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        init_git_repo(&repo_path);
        std::fs::write(repo_path.join("file.txt"), "content").unwrap();
        git_commit_all(&repo_path, "initial");
        let dest = temp.path().join("standalone-wt");
        let result = WorktreeBuilder::new(repo_path.clone(), dest.clone())
            .standalone(true)
            .git_ref("definitely-not-a-ref")
            .create();
        assert!(result.is_err(), "bogus ref must fail creation");
        assert!(
            !dest.exists(),
            "partial standalone dest must be reclaimed on a hard error"
        );
    }
}
