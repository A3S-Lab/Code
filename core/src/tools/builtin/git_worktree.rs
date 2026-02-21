//! Git Worktree tool — Create and manage isolated working trees
//!
//! Enables the AI agent to create parallel working trees for experimental
//! changes, multi-task work, or safe exploration without polluting the
//! main workspace.

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;

pub struct GitWorktreeTool;

#[async_trait]
impl Tool for GitWorktreeTool {
    fn name(&self) -> &str {
        "git_worktree"
    }

    fn description(&self) -> &str {
        "Manage git worktrees for isolated parallel work. Subcommands: create (new worktree on a branch), list (show all worktrees), remove (delete a worktree), status (show current worktree info)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["create", "list", "remove", "status"],
                    "description": "Subcommand: create, list, remove, or status"
                },
                "branch": {
                    "type": "string",
                    "description": "Branch name (for create)"
                },
                "path": {
                    "type": "string",
                    "description": "Worktree path (for create/remove)"
                },
                "new_branch": {
                    "type": "boolean",
                    "description": "Create a new branch (for create, default: true)"
                },
                "base": {
                    "type": "string",
                    "description": "Base ref for new branch (for create, default: HEAD)"
                },
                "force": {
                    "type": "boolean",
                    "description": "Force remove even if dirty (for remove)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };

        // Verify workspace is a git repo
        if !is_git_repo(&ctx.workspace).await {
            return Ok(ToolOutput::error(format!(
                "Not a git repository: {}",
                ctx.workspace.display()
            )));
        }

        match command {
            "create" => self.create(args, ctx).await,
            "list" => self.list(ctx).await,
            "remove" => self.remove(args, ctx).await,
            "status" => self.status(ctx).await,
            _ => Ok(ToolOutput::error(format!(
                "Unknown subcommand: {command}. Use: create, list, remove, status"
            ))),
        }
    }
}

impl GitWorktreeTool {
    /// Create a new worktree.
    async fn create(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let branch = match args.get("branch").and_then(|v| v.as_str()) {
            Some(b) => b,
            None => return Ok(ToolOutput::error("branch parameter is required for create")),
        };

        let new_branch = args
            .get("new_branch")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let base = args.get("base").and_then(|v| v.as_str()).unwrap_or("HEAD");

        // Default path: ../<repo-name>-<branch>
        let path = if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
            ctx.workspace.join(p)
        } else {
            let repo_name = ctx
                .workspace
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "repo".to_string());
            ctx.workspace
                .parent()
                .unwrap_or(&ctx.workspace)
                .join(format!("{repo_name}-{branch}"))
        };

        let path_str = path.display().to_string();
        let mut cmd_args: Vec<&str> = vec!["worktree", "add"];
        if new_branch {
            cmd_args.extend_from_slice(&["-b", branch, &path_str, base]);
        } else {
            cmd_args.extend_from_slice(&[&path_str, branch]);
        }

        let output = run_git(&ctx.workspace, &cmd_args).await?;
        if output.success {
            Ok(ToolOutput::success(format!(
                "Created worktree at: {}\nBranch: {branch}\n{}",
                path.display(),
                output.content
            ))
            .with_metadata(serde_json::json!({
                "path": path.display().to_string(),
                "branch": branch,
            })))
        } else {
            Ok(output)
        }
    }

    /// List all worktrees.
    async fn list(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        let output = run_git(&ctx.workspace, &["worktree", "list", "--porcelain"]).await?;
        if !output.success {
            return Ok(output);
        }

        let mut entries = Vec::new();
        let mut current_path = String::new();
        let mut current_branch = String::new();
        let mut is_bare = false;

        for line in output.content.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                if !current_path.is_empty() {
                    entries.push(format_worktree_entry(
                        &current_path,
                        &current_branch,
                        is_bare,
                    ));
                }
                current_path = p.to_string();
                current_branch.clear();
                is_bare = false;
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                current_branch = b.to_string();
            } else if line == "bare" {
                is_bare = true;
            } else if line == "detached" {
                current_branch = "(detached HEAD)".to_string();
            }
        }
        if !current_path.is_empty() {
            entries.push(format_worktree_entry(
                &current_path,
                &current_branch,
                is_bare,
            ));
        }

        if entries.is_empty() {
            Ok(ToolOutput::success("No worktrees found."))
        } else {
            Ok(ToolOutput::success(format!(
                "Worktrees ({}):\n{}",
                entries.len(),
                entries.join("\n")
            ))
            .with_metadata(serde_json::json!({ "count": entries.len() })))
        }
    }

    /// Remove a worktree.
    async fn remove(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("path parameter is required for remove")),
        };

        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut cmd_args = vec!["worktree", "remove"];
        if force {
            cmd_args.push("--force");
        }
        cmd_args.push(path);

        run_git(&ctx.workspace, &cmd_args).await
    }

    /// Show current worktree status.
    async fn status(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        let branch_output = run_git(&ctx.workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
        let branch = branch_output.content.trim().to_string();

        let git_dir = run_git(&ctx.workspace, &["rev-parse", "--git-dir"]).await?;
        let is_worktree = git_dir.content.trim().contains(".git/worktrees");

        let commit_output =
            run_git(&ctx.workspace, &["log", "--oneline", "-1", "--no-decorate"]).await?;

        let status_output = run_git(&ctx.workspace, &["status", "--porcelain", "--short"]).await?;
        let dirty_count = status_output
            .content
            .lines()
            .filter(|l| !l.is_empty())
            .count();

        let status = if dirty_count > 0 {
            format!("{dirty_count} uncommitted change(s)")
        } else {
            "clean".to_string()
        };

        Ok(ToolOutput::success(format!(
            "Workspace: {}\n\
             Branch:    {branch}\n\
             Commit:    {}\n\
             Status:    {status}\n\
             Worktree:  {}",
            ctx.workspace.display(),
            commit_output.content.trim(),
            if is_worktree {
                "yes (linked)"
            } else {
                "no (main)"
            }
        ))
        .with_metadata(serde_json::json!({
            "branch": branch,
            "is_worktree": is_worktree,
            "dirty_count": dirty_count,
        })))
    }
}

/// Check if a path is inside a git repository.
async fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a git command and capture output.
async fn run_git(workspace: &Path, args: &[&str]) -> Result<ToolOutput> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute git: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        let content = if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}{stderr}")
        };
        Ok(ToolOutput::success(content))
    } else {
        let msg = if stderr.is_empty() { stdout } else { stderr };
        Ok(ToolOutput::error(msg.trim()))
    }
}

/// Format a worktree entry for display.
fn format_worktree_entry(path: &str, branch: &str, is_bare: bool) -> String {
    if is_bare {
        format!("  {path}  (bare)")
    } else if branch.is_empty() {
        format!("  {path}")
    } else {
        format!("  {path}  [{branch}]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_tool_metadata() {
        let tool = GitWorktreeTool;
        assert_eq!(tool.name(), "git_worktree");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert!(params["properties"]["command"].is_object());
        assert!(params["properties"]["branch"].is_object());
        assert!(params["properties"]["path"].is_object());
    }

    #[test]
    fn test_format_worktree_entry_variants() {
        assert_eq!(
            format_worktree_entry("/tmp/repo", "main", false),
            "  /tmp/repo  [main]"
        );
        assert_eq!(
            format_worktree_entry("/tmp/repo", "", true),
            "  /tmp/repo  (bare)"
        );
        assert_eq!(format_worktree_entry("/tmp/repo", "", false), "  /tmp/repo");
    }

    #[tokio::test]
    async fn test_not_a_git_repo() {
        let tool = GitWorktreeTool;
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "list"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("Not a git repository"));
    }

    #[tokio::test]
    async fn test_missing_command() {
        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let args = serde_json::json!({});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("command parameter is required"));
    }

    #[tokio::test]
    async fn test_unknown_subcommand() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "foobar"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("Unknown subcommand"));
    }

    /// Helper: init a git repo with one commit.
    async fn init_repo(path: &Path) {
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
        Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(path)
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_worktree_list() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "list"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(out.success);
        assert!(out.content.contains("Worktrees"));
    }

    #[tokio::test]
    async fn test_worktree_create_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(dir.path().to_path_buf());

        // Create worktree
        let wt_path = dir.path().join("wt-feature");
        let args = serde_json::json!({
            "command": "create",
            "branch": "feature-x",
            "path": wt_path.to_str().unwrap(),
        });
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(out.success, "create failed: {}", out.content);
        assert!(wt_path.exists());

        // List should show 2 worktrees
        let args = serde_json::json!({"command": "list"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(out.success);
        assert!(out.content.contains("feature-x"));

        // Status in the new worktree
        let wt_ctx = ToolContext::new(wt_path.clone());
        let args = serde_json::json!({"command": "status"});
        let out = tool.execute(&args, &wt_ctx).await.unwrap();
        assert!(out.success);
        assert!(out.content.contains("feature-x"));
        assert!(out.content.contains("yes (linked)"));

        // Remove worktree
        let args = serde_json::json!({
            "command": "remove",
            "path": wt_path.to_str().unwrap(),
        });
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(out.success, "remove failed: {}", out.content);
        assert!(!wt_path.exists());
    }

    #[tokio::test]
    async fn test_status_main_worktree() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "status"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(out.success);
        assert!(out.content.contains("no (main)"));
        assert!(out.content.contains("clean"));
    }

    #[tokio::test]
    async fn test_create_missing_branch() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "create"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("branch parameter is required"));
    }

    #[tokio::test]
    async fn test_remove_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;

        let tool = GitWorktreeTool;
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "remove"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("path parameter is required"));
    }
}
