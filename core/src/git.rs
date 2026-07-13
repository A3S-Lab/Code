//! Git tool — Uses system git with auto-installation support
//!
//! Provides Git operations using the external git command.
//! If git is not installed, downloads and installs it automatically on Windows, macOS, and Linux.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tokio::sync::OnceCell;

static GIT_AVAILABLE: OnceCell<bool> = OnceCell::const_new();

/// Check if git is available on the system.
pub fn is_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the git installation directory in user's home.
fn git_install_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/usr/local"))
        .join(".local")
        .join("git")
}

/// Check if a path is inside a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args(["-C", &path.display().to_string()])
        .args(["rev-parse", "--git-dir"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Ensure git is installed. Downloads and installs git if not found.
pub fn ensure_git_installed() -> Result<()> {
    // Fast path: already checked and available
    if GIT_AVAILABLE.get().copied().unwrap_or(false) {
        return Ok(());
    }

    if is_git_available() {
        let _ = GIT_AVAILABLE.set(true);
        return Ok(());
    }

    // Try to download and install git
    let install_result = if cfg!(target_os = "macos") {
        install_git_macos()
    } else if cfg!(target_os = "linux") {
        install_git_linux()
    } else if cfg!(target_os = "windows") {
        install_git_windows()
    } else {
        Err(anyhow!(
            "Unsupported platform: {}. Please install git manually from https://git-scm.com",
            std::env::consts::OS
        ))
    };

    if install_result.is_ok() {
        let _ = GIT_AVAILABLE.set(true);
    }

    install_result
}

fn install_git_macos() -> Result<()> {
    // Download official macOS git installer
    let install_dir = git_install_dir();
    let bin_dir = install_dir.join("bin");
    let git_path = bin_dir.join("git");

    // If already installed, just verify
    if git_path.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&bin_dir)?;

    // Download git-2.39.3-arm64-bin.tar.gz (Apple Silicon)
    // and git-2.39.3-x86_64-bin.tar.gz (Intel)
    let arch = if std::process::Command::new("uname")
        .arg("-m")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("arm"))
        .unwrap_or(false)
    {
        "arm64"
    } else {
        "x86_64"
    };

    let tarball_name = format!("git-2.39.3-{}-bin.tar.gz", arch);
    let download_url = format!("https://git-scm.com/download/mac/{}", tarball_name);

    let temp_tarball = std::env::temp_dir().join(&tarball_name);

    // Download with retry
    download_with_curl(&download_url, &temp_tarball)?;

    // Extract tarball
    let output = Command::new("tar")
        .args([
            "-xzf",
            &temp_tarball.display().to_string(),
            "-C",
            &bin_dir.display().to_string(),
        ])
        .output()?;

    if !output.status.success() {
        // Try alternative extraction
        let output = Command::new("bash")
            .args([
                "-c",
                &format!(
                    "cd {} && tar -xzf {}",
                    bin_dir.display(),
                    temp_tarball.display()
                ),
            ])
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to extract git tarball"));
        }
    }

    // Clean up
    let _ = std::fs::remove_file(&temp_tarball);

    // Verify installation
    if bin_dir.join("git").exists() {
        Ok(())
    } else {
        // Try extracting directly to install_dir
        let output = Command::new("tar")
            .args(["-xzf", &temp_tarball.display().to_string()])
            .current_dir(&install_dir)
            .output()?;

        if output.status.success() {
            // Move contents from bin to our bin dir
            let extracted_bin = install_dir.join("usr").join("bin");
            if extracted_bin.exists() {
                for entry in std::fs::read_dir(&extracted_bin)?.flatten() {
                    let _ = std::fs::rename(entry.path(), bin_dir.join(entry.file_name()));
                }
            }
        }

        if bin_dir.join("git").exists() {
            Ok(())
        } else {
            Err(anyhow!(
                "Failed to install git. Please download from https://git-scm.com/download/mac"
            ))
        }
    }
}

fn install_git_linux() -> Result<()> {
    let install_dir = git_install_dir();
    let bin_dir = install_dir.join("bin");
    let git_path = bin_dir.join("git");

    // If already installed, verify and return
    if git_path.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&bin_dir)?;

    // Try to download static git binary for Linux
    // Use the official git releases from GitHub
    let version = "2.39.3";
    let arch = if std::process::Command::new("uname")
        .arg("-m")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("x86_64"))
        .unwrap_or(true)
    {
        "amd64"
    } else {
        "386"
    };

    // Try GitHub releases first (has portable binaries)
    let tarball_name = format!("git-{}-linux-{}.tar.gz", version, arch);
    let download_url = format!(
        "https://github.com/git/git/releases/download/v{}/{}",
        version, tarball_name
    );

    let temp_tarball = std::env::temp_dir().join(&tarball_name);

    // Try downloading from GitHub
    if download_with_curl(&download_url, &temp_tarball).is_err() {
        // Fallback: try official download page
        let fallback_url = format!("https://git-scm.com/downloads?file=git-{}", tarball_name);
        download_with_curl(&fallback_url, &temp_tarball)?;
    }

    // Extract
    let output = Command::new("tar")
        .args([
            "-xzf",
            &temp_tarball.display().to_string(),
            "-C",
            &bin_dir.display().to_string(),
        ])
        .output()?;

    if !output.status.success() {
        // Alternative: extract to temp and move
        let temp_dir = std::env::temp_dir().join("git-extract");
        let _ = std::fs::create_dir_all(&temp_dir);

        let output = Command::new("tar")
            .args([
                "-xzf",
                &temp_tarball.display().to_string(),
                "-C",
                &temp_dir.display().to_string(),
            ])
            .output()?;

        if output.status.success() {
            // Find and move git binary
            for path in walkdir(&temp_dir) {
                if let Some(name) = path.file_name() {
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("git-") && path.extension().is_none() {
                        let _ = std::fs::copy(&path, bin_dir.join("git"));
                    }
                }
            }
        }
    }

    let _ = std::fs::remove_file(&temp_tarball);

    if bin_dir.join("git").exists() {
        Ok(())
    } else {
        Err(anyhow!(
            "Failed to install git automatically.\n\n\
            Please install git via your system's package manager or download from:\n\
            https://git-scm.com/download/linux"
        ))
    }
}

fn install_git_windows() -> Result<()> {
    let install_dir = git_install_dir();
    let bin_dir = install_dir.join("bin");
    let git_exe = bin_dir.join("git.exe");

    // If already installed, verify and return
    if git_exe.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&bin_dir)?;

    // Download portable Git for Windows (MinGit)
    let version = "2.39.3.windows.1";
    let zip_name = format!("MinGit-{}-portable.zip", version);
    let download_url = format!(
        "https://github.com/git-for-windows/git/releases/download/{}/{}",
        version, zip_name
    );

    let temp_zip = std::env::temp_dir().join(&zip_name);

    download_with_curl(&download_url, &temp_zip)?;

    // Extract zip
    let output = Command::new("tar")
        .args([
            "-xf",
            &temp_zip.display().to_string(),
            "-C",
            &bin_dir.display().to_string(),
        ])
        .output()?;

    if !output.status.success() {
        // Try with PowerShell Expand-Archive on Windows
        let ps_script = format!(
            "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
            temp_zip.display(),
            bin_dir.display()
        );

        let output = Command::new("powershell")
            .args(["-Command", &ps_script])
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to extract git zip archive"));
        }
    }

    // Find git.exe in the extracted contents
    let extracted_dir = bin_dir.join(format!("MinGit-{}", version));
    if extracted_dir.exists() {
        // Move contents up one level
        for entry in std::fs::read_dir(&extracted_dir)?.flatten() {
            let dest = bin_dir.join(entry.file_name());
            let _ = std::fs::rename(entry.path(), dest);
        }
        let _ = std::fs::remove_dir(&extracted_dir);
    }

    let _ = std::fs::remove_file(&temp_zip);

    if git_exe.exists() {
        // Also copy git.exe to git-bash.exe and cmd/git.exe
        let cmd_dir = bin_dir.join("cmd");
        std::fs::create_dir_all(&cmd_dir)?;
        let _ = std::fs::copy(&git_exe, cmd_dir.join("git.exe"));
        Ok(())
    } else {
        Err(anyhow!(
            "Failed to install git automatically.\n\n\
            Please download and install Git from:\n\
            https://git-scm.com/download/win"
        ))
    }
}

/// Download a file using curl with retry.
fn download_with_curl(url: &str, path: &std::path::Path) -> Result<()> {
    // Try curl first
    let output = Command::new("curl")
        .args([
            "-L",
            "--fail",
            "--retry",
            "3",
            "--retry-delay",
            "2",
            "-o",
            &path.display().to_string(),
            url,
        ])
        .output()?;

    if output.status.success() && path.exists() {
        return Ok(());
    }

    // Fallback: try wget
    let output = Command::new("wget")
        .args(["-O", &path.display().to_string(), url])
        .output()?;

    if output.status.success() && path.exists() {
        return Ok(());
    }

    Err(anyhow!(
        "Failed to download git from {}.\n\
        Please check your internet connection and try again.\n\
        Or download manually from https://git-scm.com",
        url
    ))
}

/// Walk directory recursively (simple implementation).
fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walkdir(&path));
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// Run a git command.
fn run_git(repo_path: &Path, args: &[&str]) -> Result<(bool, String, String)> {
    ensure_git_installed()?;

    let output = Command::new("git")
        .args(["-C", &repo_path.display().to_string()])
        .args(args)
        .output()
        .map_err(|e| anyhow!("Failed to execute git: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((output.status.success(), stdout, stderr))
}

// ==================== Git Operations ====================

/// Repository status information.
#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub branch: String,
    pub commit: String,
    pub is_worktree: bool,
    pub is_dirty: bool,
    pub dirty_count: usize,
}

/// Get repository status.
pub fn get_status(repo_path: &Path) -> Result<RepoStatus> {
    let (success, stdout, _) = run_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = if success {
        stdout.trim().to_string()
    } else {
        "(detached)".to_string()
    };

    let (_, commit, _) = run_git(repo_path, &["log", "--oneline", "-1", "--no-decorate"])?;
    let commit = commit.trim().to_string();

    let (_, git_dir, _) = run_git(repo_path, &["rev-parse", "--git-dir"])?;
    let is_worktree = git_dir.trim().contains(".git/worktrees");

    let (_, status_output, _) = run_git(repo_path, &["status", "--porcelain", "--short"])?;
    let dirty_count = status_output.lines().filter(|l| !l.is_empty()).count();
    let is_dirty = dirty_count > 0;

    Ok(RepoStatus {
        branch,
        commit,
        is_worktree,
        is_dirty,
        dirty_count,
    })
}

/// Commit information for log display.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// Get commit log.
pub fn get_log(repo_path: &Path, max_count: usize) -> Result<Vec<CommitInfo>> {
    let format = "%H|%s|%an|%ad";
    let date_format = "%Y-%m-%d %H:%M";
    let args = [
        "log",
        &format!("--format={}", format),
        &format!("--date=format:{}", date_format),
        &format!("-{}", max_count),
    ];

    let (_, stdout, _) = run_git(repo_path, &args)?;

    let commits: Vec<CommitInfo> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() >= 4 {
                Some(CommitInfo {
                    id: parts[0].to_string(),
                    message: parts[1].to_string(),
                    author: parts[2].to_string(),
                    date: parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(commits)
}

/// Branch information.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
}

/// List all local branches.
pub fn list_branches(repo_path: &Path) -> Result<Vec<BranchInfo>> {
    let (_, stdout, _) = run_git(repo_path, &["branch"])?;

    let branches: Vec<BranchInfo> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let is_current = line.starts_with('*');
            let name = line.trim_start_matches(['*', ' ']).to_string();
            Some(BranchInfo { name, is_current })
        })
        .collect();

    Ok(branches)
}

/// Create a new branch.
pub fn create_branch(repo_path: &Path, name: &str, base: &str) -> Result<()> {
    let (success, _, stderr) = run_git(repo_path, &["checkout", "-b", name, base])?;
    if !success && !stderr.is_empty() {
        return Err(anyhow!("Failed to create branch: {}", stderr));
    }
    Ok(())
}

/// Worktree information.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub is_bare: bool,
    pub is_detached: bool,
}

/// List all worktrees.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeInfo>> {
    let (_, stdout, _) = run_git(repo_path, &["worktree", "list", "--porcelain"])?;

    let mut worktrees = Vec::new();
    let mut current_path = String::new();
    let mut current_branch = String::new();
    let mut is_bare = false;
    let mut is_detached = false;

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if !current_path.is_empty() {
                worktrees.push(WorktreeInfo {
                    path: current_path.clone(),
                    branch: current_branch.clone(),
                    is_bare,
                    is_detached,
                });
            }
            current_path = path.to_string();
            current_branch.clear();
            is_bare = false;
            is_detached = false;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = branch.to_string();
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }

    if !current_path.is_empty() {
        worktrees.push(WorktreeInfo {
            path: current_path,
            branch: current_branch,
            is_bare,
            is_detached,
        });
    }

    Ok(worktrees)
}

/// Create a new worktree.
pub fn create_worktree(
    repo_path: &Path,
    branch: &str,
    path: &Path,
    new_branch: bool,
) -> Result<()> {
    let path_str = path.display().to_string();
    let args: Vec<&str> = if new_branch {
        vec!["worktree", "add", "-b", branch, &path_str]
    } else {
        vec!["worktree", "add", &path_str, branch]
    };

    let (success, _, stderr) = run_git(repo_path, &args)?;
    if !success {
        return Err(anyhow!("Failed to create worktree: {}", stderr));
    }
    Ok(())
}

/// Remove a worktree.
pub fn remove_worktree(repo_path: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.display().to_string();
    let args: Vec<&str> = if force {
        vec!["worktree", "remove", "--force", &path_str]
    } else {
        vec!["worktree", "remove", &path_str]
    };

    let (success, _, stderr) = run_git(repo_path, &args)?;
    if !success {
        return Err(anyhow!("Failed to remove worktree: {}", stderr));
    }
    Ok(())
}

/// Get diff output.
pub fn get_diff(repo_path: &Path, target: Option<&str>) -> Result<String> {
    let args: Vec<&str> = if let Some(t) = target {
        vec!["diff", t]
    } else {
        vec!["diff"]
    };

    let (_, stdout, _) = run_git(repo_path, &args)?;
    Ok(stdout)
}

/// Stash information.
#[derive(Debug, Clone)]
pub struct StashInfo {
    pub index: usize,
    pub message: String,
}

/// List stashes.
pub fn list_stashes(repo_path: &Path) -> Result<Vec<StashInfo>> {
    let (_, stdout, _) = run_git(repo_path, &["stash", "list", "--format=%H|%gd|%s"])?;

    let stashes: Vec<StashInfo> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() >= 3 {
                Some(StashInfo {
                    index: parts[1].parse().unwrap_or(0),
                    message: parts[2].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(stashes)
}

/// Create a stash.
pub fn stash(repo_path: &Path, message: Option<&str>, include_untracked: bool) -> Result<()> {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("-u");
    }
    if let Some(msg) = message {
        args.push("-m");
        args.push(msg);
    }

    let (success, _, stderr) = run_git(repo_path, &args)?;
    if !success {
        return Err(anyhow!("Failed to stash: {}", stderr));
    }
    Ok(())
}
