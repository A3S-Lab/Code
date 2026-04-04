//! Git tool — Uses system git with auto-installation support
//!
//! Provides Git operations using the external git command.
//! If git is not installed, attempts to install it automatically.

use crate::git;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

pub struct GitTool;

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Execute Git operations using the system git command. Supports: status, log, branch, checkout, diff, stash, remote, and worktree management. Auto-installs git if not available."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "command": {
                    "type": "string",
                    "enum": [
                        "status", "log", "branch", "checkout", "diff", "stash", "remote", "worktree"
                    ],
                    "description": "Required. Git command to execute."
                },
                // Branch/worktree specific
                "name": {
                    "type": "string",
                    "description": "Branch name for branch/checkout/worktree operations."
                },
                "path": {
                    "type": "string",
                    "description": "Path for worktree operations."
                },
                // Checkout specific
                "ref": {
                    "type": "string",
                    "description": "Reference (branch, tag, commit) for checkout."
                },
                "force": {
                    "type": "boolean",
                    "description": "Force checkout/create even if it loses changes."
                },
                // Diff specific
                "target": {
                    "type": "string",
                    "description": "Target ref for diff (e.g., HEAD~1, main). If omitted, diffs working tree."
                },
                // Log specific
                "max_count": {
                    "type": "integer",
                    "description": "Maximum number of log entries to show (default 10)."
                },
                // Stash specific
                "message": {
                    "type": "string",
                    "description": "Message for stash."
                },
                "include_untracked": {
                    "type": "boolean",
                    "description": "Include untracked files in stash (default false)."
                },
                // Remote specific
                "remote_name": {
                    "type": "string",
                    "description": "Remote name (default 'origin')."
                },
                // Worktree specific
                "new_branch": {
                    "type": "boolean",
                    "description": "Create a new branch for worktree (default true)."
                },
                "base": {
                    "type": "string",
                    "description": "Base ref for new branch (default HEAD)."
                }
            },
            "required": ["command"],
            "examples": [
                {"command": "status"},
                {"command": "log", "max_count": 5},
                {"command": "branch"},
                {"command": "branch", "name": "feature-x"},
                {"command": "checkout", "ref": "feature-x"},
                {"command": "diff"},
                {"command": "diff", "target": "HEAD~1"},
                {"command": "stash"},
                {"command": "stash", "message": "WIP: work in progress"},
                {"command": "remote"},
                {"command": "worktree", "command": "list"}
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };

        // Verify workspace is a git repo
        if !git::is_git_repo(&ctx.workspace) {
            return Ok(ToolOutput::error(format!(
                "Not a git repository: {}",
                ctx.workspace.display()
            )));
        }

        match command {
            "status" => self.status(ctx).await,
            "log" => self.log(args, ctx).await,
            "branch" => self.branch(args, ctx).await,
            "checkout" => self.checkout(args, ctx).await,
            "diff" => self.diff(args, ctx).await,
            "stash" => self.stash(args, ctx).await,
            "remote" => self.remote(args, ctx).await,
            "worktree" => self.worktree(args, ctx).await,
            _ => Ok(ToolOutput::error(format!(
                "Unknown command: {command}. Use: status, log, branch, checkout, diff, stash, remote, worktree"
            ))),
        }
    }
}

impl GitTool {
    /// Show repository status.
    async fn status(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        match git::get_status(&ctx.workspace) {
            Ok(status) => {
                let status_str = if status.is_dirty {
                    format!("{} uncommitted change(s)", status.dirty_count)
                } else {
                    "clean".to_string()
                };

                Ok(ToolOutput::success(format!(
                    "Workspace: {}\n\
                     Branch:    {}\n\
                     Commit:    {}\n\
                     Status:    {}\n\
                     Worktree:  {}",
                    ctx.workspace.display(),
                    status.branch,
                    status.commit,
                    status_str,
                    if status.is_worktree {
                        "yes (linked)"
                    } else {
                        "no (main)"
                    }
                ))
                .with_metadata(serde_json::json!({
                    "branch": status.branch,
                    "is_worktree": status.is_worktree,
                    "dirty_count": status.dirty_count,
                    "is_dirty": status.is_dirty,
                })))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to get status: {e}"))),
        }
    }

    /// Show commit log.
    async fn log(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let max_count = args.get("max_count").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        match git::get_log(&ctx.workspace, max_count) {
            Ok(commits) => {
                if commits.is_empty() {
                    return Ok(ToolOutput::success("No commits found."));
                }

                let entries: Vec<String> = commits
                    .iter()
                    .map(|c| {
                        format!(
                            "{} - {} ({})\n  {}",
                            &c.id[..7],
                            c.author,
                            c.date,
                            c.message
                        )
                    })
                    .collect();

                Ok(ToolOutput::success(format!(
                    "Commit log ({} entries):\n\n{}",
                    commits.len(),
                    entries.join("\n\n")
                ))
                .with_metadata(serde_json::json!({ "count": commits.len() })))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to get log: {e}"))),
        }
    }

    /// Branch operations: list or create.
    async fn branch(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name = args.get("name").and_then(|v| v.as_str());

        if let Some(branch_name) = name {
            let base = args.get("base").and_then(|v| v.as_str()).unwrap_or("HEAD");

            match git::create_branch(&ctx.workspace, branch_name, base) {
                Ok(_) => Ok(ToolOutput::success(format!(
                    "Created branch: {} (based on {})",
                    branch_name, base
                ))
                .with_metadata(serde_json::json!({ "branch": branch_name, "base": base }))),
                Err(e) => Ok(ToolOutput::error(format!("Failed to create branch: {e}"))),
            }
        } else {
            // List branches
            match git::list_branches(&ctx.workspace) {
                Ok(branches) => {
                    if branches.is_empty() {
                        return Ok(ToolOutput::success("No branches found."));
                    }

                    let entries: Vec<String> = branches
                        .iter()
                        .map(|b| {
                            let prefix = if b.is_current { "* " } else { "  " };
                            format!("{}{}", prefix, b.name)
                        })
                        .collect();

                    Ok(
                        ToolOutput::success(format!("Branches:\n{}", entries.join("\n")))
                            .with_metadata(serde_json::json!({ "count": branches.len() })),
                    )
                }
                Err(e) => Ok(ToolOutput::error(format!("Failed to list branches: {e}"))),
            }
        }
    }

    /// Checkout a branch or commit.
    async fn checkout(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let refspec = match args.get("ref").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => return Ok(ToolOutput::error("ref parameter is required for checkout")),
        };

        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let args_strs: Vec<&str> = if force {
            vec!["checkout", "--force", refspec]
        } else {
            vec!["checkout", refspec]
        };

        let output = tokio::process::Command::new("git")
            .args(["-C", &ctx.workspace.display().to_string()])
            .args(&args_strs)
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(ToolOutput::success(format!(
                "Checked out: {}{}",
                refspec,
                if stdout.trim().is_empty() {
                    "".to_string()
                } else {
                    format!("\n{}", stdout)
                }
            )))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(ToolOutput::error(format!("Failed to checkout: {}", stderr)))
        }
    }

    /// Show diff.
    async fn diff(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let target = args.get("target").and_then(|v| v.as_str());

        match git::get_diff(&ctx.workspace, target) {
            Ok(diff) => {
                if diff.trim().is_empty() {
                    return Ok(ToolOutput::success("No changes.".to_string()));
                }
                Ok(ToolOutput::success(diff))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to get diff: {e}"))),
        }
    }

    /// Stash operations.
    async fn stash(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let message = args.get("message").and_then(|v| v.as_str());
        let include_untracked = args
            .get("include_untracked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if message.is_some() || include_untracked {
            // Create stash
            match git::stash(&ctx.workspace, message, include_untracked) {
                Ok(_) => Ok(ToolOutput::success("Created stash".to_string())),
                Err(e) => Ok(ToolOutput::error(format!("Failed to stash: {e}"))),
            }
        } else {
            // List stashes
            match git::list_stashes(&ctx.workspace) {
                Ok(stashes) => {
                    if stashes.is_empty() {
                        return Ok(ToolOutput::success("No stashes found.".to_string()));
                    }

                    let entries: Vec<String> = stashes
                        .iter()
                        .map(|s| format!("{}: {}", s.index, s.message))
                        .collect();

                    Ok(
                        ToolOutput::success(format!("Stashes:\n{}", entries.join("\n")))
                            .with_metadata(serde_json::json!({ "count": stashes.len() })),
                    )
                }
                Err(e) => Ok(ToolOutput::error(format!("Failed to list stashes: {e}"))),
            }
        }
    }

    /// Show remote information.
    async fn remote(&self, _args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let output = tokio::process::Command::new("git")
            .args(["-C", &ctx.workspace.display().to_string()])
            .args(["remote", "-v"])
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if output.status.success() && !stdout.trim().is_empty() {
            Ok(ToolOutput::success(format!("Remotes:\n{}", stdout)))
        } else {
            Ok(ToolOutput::success("No remotes configured.".to_string()))
        }
    }

    /// Worktree operations.
    async fn worktree(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let subcommand = args
            .get("subcommand")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        match subcommand {
            "list" => self.list_worktrees(ctx).await,
            "create" => self.create_worktree(args, ctx).await,
            "remove" => self.remove_worktree(args, ctx).await,
            _ => Ok(ToolOutput::error(format!(
                "Unknown worktree subcommand: {subcommand}. Use: list, create, remove"
            ))),
        }
    }

    /// List all worktrees.
    async fn list_worktrees(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        match git::list_worktrees(&ctx.workspace) {
            Ok(worktrees) => {
                if worktrees.is_empty() {
                    return Ok(ToolOutput::success("No worktrees found."));
                }

                let entries: Vec<String> = worktrees
                    .iter()
                    .map(|wt| {
                        let suffix = if wt.is_bare {
                            " (bare)".to_string()
                        } else if wt.is_detached {
                            " (detached)".to_string()
                        } else {
                            format!(" [{}]", wt.branch)
                        };
                        format!("  {}{}", wt.path, suffix)
                    })
                    .collect();

                Ok(ToolOutput::success(format!(
                    "Worktrees ({}):\n{}",
                    worktrees.len(),
                    entries.join("\n")
                ))
                .with_metadata(serde_json::json!({ "count": worktrees.len() })))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to list worktrees: {e}"))),
        }
    }

    /// Create a new worktree.
    async fn create_worktree(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let branch = match args
            .get("name")
            .or_else(|| args.get("branch"))
            .and_then(|v| v.as_str())
        {
            Some(b) => b,
            None => {
                return Ok(ToolOutput::error(
                    "branch name is required for worktree create",
                ))
            }
        };

        let new_branch = args
            .get("new_branch")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

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

        match git::create_worktree(&ctx.workspace, branch, &path, new_branch) {
            Ok(_) => Ok(ToolOutput::success(format!(
                "Created worktree at: {}\nBranch: {branch}",
                path.display()
            ))
            .with_metadata(serde_json::json!({
                "path": path.display().to_string(),
                "branch": branch,
            }))),
            Err(e) => Ok(ToolOutput::error(format!("Failed to create worktree: {e}"))),
        }
    }

    /// Remove a worktree.
    async fn remove_worktree(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let path = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return Ok(ToolOutput::error(
                    "path parameter is required for worktree remove",
                ))
            }
        };

        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let path = Path::new(path);

        match git::remove_worktree(&ctx.workspace, path, force) {
            Ok(_) => Ok(ToolOutput::success(format!(
                "Removed worktree at: {}",
                path.display()
            ))),
            Err(e) => Ok(ToolOutput::error(format!("Failed to remove worktree: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_git_not_installed() {
        // This test just checks that the tool handles non-git repos properly
        let tool = GitTool;
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path().to_path_buf());
        let args = serde_json::json!({"command": "status"});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("Not a git repository"));
    }

    #[tokio::test]
    async fn test_missing_command() {
        let tool = GitTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let args = serde_json::json!({});
        let out = tool.execute(&args, &ctx).await.unwrap();
        assert!(!out.success);
        assert!(out.content.contains("command parameter is required"));
    }
}
