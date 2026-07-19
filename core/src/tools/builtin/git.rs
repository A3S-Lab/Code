//! Git tool backed by workspace Git services.
//!
//! The tool keeps the model-facing contract stable while the concrete Git
//! implementation can be local, remote, browser-backed, or DFS-backed.

use crate::tools::pagination::PageRequest;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use crate::workspace::{
    WorkspaceGit, WorkspaceGitCheckoutRequest, WorkspaceGitCreateBranchRequest,
    WorkspaceGitCreateWorktreeRequest, WorkspaceGitDiffRequest, WorkspaceGitRemote,
    WorkspaceGitRemoveWorktreeRequest, WorkspaceGitStashProvider, WorkspaceGitStashRequest,
    WorkspaceGitWorktreeProvider,
};
use anyhow::Result;
use async_trait::async_trait;

pub struct GitTool;

const DEFAULT_GIT_LIST_LIMIT: usize = 50;
const DEFAULT_GIT_LOG_LIMIT: usize = 10;
const MAX_GIT_PAGE_LIMIT: usize = 200;
const MAX_GIT_LOG_SCAN: usize = 5_000;
const DEFAULT_DIFF_BYTES: usize = 64 * 1024;
const MIN_DIFF_BYTES: usize = 256;

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Execute Git operations for the current workspace. Supports: status, log, branch, checkout, diff, stash, remote, and worktree management."
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
                "subcommand": {
                    "type": "string",
                    "enum": ["list", "create", "remove"],
                    "description": "Worktree subcommand. Defaults to list."
                },
                "name": {
                    "type": "string",
                    "description": "Branch name for branch/checkout/worktree operations."
                },
                "path": {
                    "type": "string",
                    "description": "Path for worktree operations."
                },
                "ref": {
                    "type": "string",
                    "description": "Reference (branch, tag, commit) for checkout."
                },
                "force": {
                    "type": "boolean",
                    "description": "Force checkout/create even if it loses changes."
                },
                "target": {
                    "type": "string",
                    "description": "Target ref for diff (e.g., HEAD~1, main). If omitted, diffs working tree."
                },
                "max_count": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Deprecated alias for limit when command=log."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_GIT_PAGE_LIMIT,
                    "description": "Maximum list or log entries to return. Defaults to 10 for log and 50 for other list commands; maximum 200."
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque continuation cursor returned by a previous log, branch, stash, remote, or worktree list call."
                },
                "byte_offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Byte offset for continuing a diff response."
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": MIN_DIFF_BYTES,
                    "maximum": MAX_OUTPUT_SIZE,
                    "description": "Maximum UTF-8 bytes returned by diff. Default 65536; maximum 102400."
                },
                "message": {
                    "type": "string",
                    "description": "Message for stash."
                },
                "include_untracked": {
                    "type": "boolean",
                    "description": "Include untracked files in stash (default false)."
                },
                "remote_name": {
                    "type": "string",
                    "description": "Remote name (reserved for provider-specific filtering)."
                },
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
                {"command": "worktree", "subcommand": "list"}
            ]
        })
    }

    fn capabilities(&self, args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        let command = args.get("command").and_then(|value| value.as_str());
        let is_read = match command {
            Some("status" | "log" | "diff" | "remote") => true,
            Some("branch") => args.get("name").is_none(),
            Some("stash") => {
                args.get("message").is_none()
                    && !args
                        .get("include_untracked")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
            }
            Some("worktree") => {
                args.get("subcommand")
                    .and_then(|value| value.as_str())
                    .unwrap_or("list")
                    == "list"
            }
            _ => false,
        };
        if !is_read {
            return crate::tools::ToolCapabilities::conservative();
        }

        let mut capabilities = crate::tools::ToolCapabilities::read_only_paginated(8);
        capabilities.supports_pagination = matches!(
            command,
            Some("log" | "branch" | "diff" | "stash" | "remote" | "worktree")
        );
        if command == Some("diff") {
            capabilities.output_kind = crate::tools::ToolOutputKind::Diff;
        }
        capabilities
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };

        let Some(git) = ctx.workspace_services.git() else {
            return Ok(ToolOutput::error(
                "Git is not available for this workspace backend",
            ));
        };

        match git.is_repository().await {
            Ok(true) => {}
            Ok(false) => {
                return Ok(ToolOutput::error(format!(
                    "Not a git repository: {}",
                    ctx.workspace_services.workspace_ref().display_root
                )))
            }
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to inspect git repository: {e}"
                )))
            }
        }

        match command {
            "status" => self.status(ctx, git.as_ref()).await,
            "log" => self.log(args, git.as_ref()).await,
            "branch" => self.branch(args, git.as_ref()).await,
            "checkout" => self.checkout(args, git.as_ref()).await,
            "diff" => self.diff(args, git.as_ref()).await,
            "stash" => {
                let Some(stash) = ctx.workspace_services.git_stash() else {
                    return Ok(ToolOutput::error(
                        "Stash operations are not supported by this workspace backend",
                    ));
                };
                self.stash(args, stash.as_ref()).await
            }
            "remote" => self.remote(args, git.as_ref()).await,
            "worktree" => {
                let Some(worktree) = ctx.workspace_services.git_worktree() else {
                    return Ok(ToolOutput::error(
                        "Worktree operations are not supported by this workspace backend",
                    ));
                };
                self.worktree(args, worktree.as_ref()).await
            }
            _ => Ok(ToolOutput::error(format!(
                "Unknown command: {command}. Use: status, log, branch, checkout, diff, stash, remote, worktree"
            ))),
        }
    }
}

impl GitTool {
    /// Show repository status.
    async fn status(&self, ctx: &ToolContext, git: &dyn WorkspaceGit) -> Result<ToolOutput> {
        match git.status().await {
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
                    ctx.workspace_services.workspace_ref().display_root,
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
    async fn log(&self, args: &serde_json::Value, git: &dyn WorkspaceGit) -> Result<ToolOutput> {
        let page_request = match git_page_request(args, DEFAULT_GIT_LOG_LIMIT) {
            Ok(request) => request,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        if page_request.offset > MAX_GIT_LOG_SCAN {
            return Ok(ToolOutput::error(format!(
                "cursor exceeds the maximum log scan of {MAX_GIT_LOG_SCAN} entries"
            )));
        }
        let fetch_count = page_request
            .offset
            .saturating_add(page_request.limit)
            .saturating_add(1)
            .min(MAX_GIT_LOG_SCAN.saturating_add(1));

        match git.log(fetch_count).await {
            Ok(commits) => {
                if page_request.offset > commits.len() {
                    return Ok(ToolOutput::error(format!(
                        "cursor offset {} exceeds available commit history",
                        page_request.offset
                    )));
                }
                let end = page_request
                    .offset
                    .saturating_add(page_request.limit)
                    .min(commits.len());
                let has_more = end < commits.len();
                let history_complete = commits.len() < fetch_count;
                let commits = &commits[page_request.offset..end];
                if commits.is_empty() && page_request.offset == 0 {
                    return Ok(ToolOutput::success("No commits found."));
                }

                let entries: Vec<String> = commits
                    .iter()
                    .map(|commit| {
                        format!(
                            "{} - {} ({})\n  {}",
                            short_commit_id(&commit.id),
                            commit.author,
                            commit.date,
                            commit.message
                        )
                    })
                    .collect();

                Ok(ToolOutput::success(format!(
                    "Commit log ({} entries from offset {}):\n\n{}{}",
                    commits.len(),
                    page_request.offset,
                    entries.join("\n\n"),
                    if has_more {
                        format!("\n\nMore commits available; continue with cursor={end}")
                    } else {
                        String::new()
                    }
                ))
                .with_metadata(serde_json::json!({
                    "count": commits.len(),
                    "page": git_page_metadata(
                        page_request,
                        commits.len(),
                        has_more.then(|| end.to_string()),
                        history_complete.then_some(end),
                    ),
                })))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to get log: {e}"))),
        }
    }

    /// Branch operations: list or create.
    async fn branch(&self, args: &serde_json::Value, git: &dyn WorkspaceGit) -> Result<ToolOutput> {
        let name = args.get("name").and_then(|v| v.as_str());

        if let Some(branch_name) = name {
            let base = args.get("base").and_then(|v| v.as_str()).unwrap_or("HEAD");

            match git
                .create_branch(WorkspaceGitCreateBranchRequest {
                    name: branch_name.to_string(),
                    base: base.to_string(),
                })
                .await
            {
                Ok(_) => Ok(ToolOutput::success(format!(
                    "Created branch: {} (based on {})",
                    branch_name, base
                ))
                .with_metadata(serde_json::json!({ "branch": branch_name, "base": base }))),
                Err(e) => Ok(ToolOutput::error(format!("Failed to create branch: {e}"))),
            }
        } else {
            match git.list_branches().await {
                Ok(branches) => {
                    let page = match git_page_request(args, DEFAULT_GIT_LIST_LIMIT)
                        .and_then(|request| request.page(branches))
                    {
                        Ok(page) => page,
                        Err(error) => return Ok(ToolOutput::error(error)),
                    };
                    if page.items.is_empty() && page.total_items == 0 {
                        return Ok(ToolOutput::success("No branches found."));
                    }

                    let entries: Vec<String> = page
                        .items
                        .iter()
                        .map(|branch| {
                            let prefix = if branch.is_current { "* " } else { "  " };
                            format!("{}{}", prefix, branch.name)
                        })
                        .collect();

                    let continuation = page
                        .next_cursor
                        .as_deref()
                        .map(|cursor| {
                            format!("\nMore branches available; continue with cursor={cursor}")
                        })
                        .unwrap_or_default();
                    Ok(ToolOutput::success(format!(
                        "Branches:\n{}{}",
                        entries.join("\n"),
                        continuation
                    ))
                    .with_metadata(serde_json::json!({ "page": page.metadata() })))
                }
                Err(e) => Ok(ToolOutput::error(format!("Failed to list branches: {e}"))),
            }
        }
    }

    /// Checkout a branch or commit.
    async fn checkout(
        &self,
        args: &serde_json::Value,
        git: &dyn WorkspaceGit,
    ) -> Result<ToolOutput> {
        let refspec = match args.get("ref").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => return Ok(ToolOutput::error("ref parameter is required for checkout")),
        };

        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

        match git
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: refspec.to_string(),
                force,
            })
            .await
        {
            Ok(output) => Ok(ToolOutput::success(format!(
                "Checked out: {}{}",
                refspec,
                if output.stdout.trim().is_empty() {
                    String::new()
                } else {
                    format!("\n{}", output.stdout)
                }
            ))),
            Err(e) => Ok(ToolOutput::error(format!("Failed to checkout: {e}"))),
        }
    }

    /// Show diff.
    async fn diff(&self, args: &serde_json::Value, git: &dyn WorkspaceGit) -> Result<ToolOutput> {
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        match git.diff(WorkspaceGitDiffRequest { target }).await {
            Ok(diff) => {
                if diff.trim().is_empty() {
                    return Ok(ToolOutput::success("No changes.".to_string()));
                }
                bounded_diff_output(args, diff)
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to get diff: {e}"))),
        }
    }

    /// Stash operations.
    async fn stash(
        &self,
        args: &serde_json::Value,
        stash: &dyn WorkspaceGitStashProvider,
    ) -> Result<ToolOutput> {
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let include_untracked = args
            .get("include_untracked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if message.is_some() || include_untracked {
            match stash
                .stash(WorkspaceGitStashRequest {
                    message,
                    include_untracked,
                })
                .await
            {
                Ok(_) => Ok(ToolOutput::success("Created stash".to_string())),
                Err(e) => Ok(ToolOutput::error(format!("Failed to stash: {e}"))),
            }
        } else {
            match stash.list_stashes().await {
                Ok(stashes) => {
                    let page = match git_page_request(args, DEFAULT_GIT_LIST_LIMIT)
                        .and_then(|request| request.page(stashes))
                    {
                        Ok(page) => page,
                        Err(error) => return Ok(ToolOutput::error(error)),
                    };
                    if page.items.is_empty() && page.total_items == 0 {
                        return Ok(ToolOutput::success("No stashes found.".to_string()));
                    }

                    let entries: Vec<String> = page
                        .items
                        .iter()
                        .map(|stash| format!("{}: {}", stash.index, stash.message))
                        .collect();

                    let continuation = page
                        .next_cursor
                        .as_deref()
                        .map(|cursor| {
                            format!("\nMore stashes available; continue with cursor={cursor}")
                        })
                        .unwrap_or_default();
                    Ok(ToolOutput::success(format!(
                        "Stashes:\n{}{}",
                        entries.join("\n"),
                        continuation
                    ))
                    .with_metadata(serde_json::json!({ "page": page.metadata() })))
                }
                Err(e) => Ok(ToolOutput::error(format!("Failed to list stashes: {e}"))),
            }
        }
    }

    /// Show remote information.
    async fn remote(&self, args: &serde_json::Value, git: &dyn WorkspaceGit) -> Result<ToolOutput> {
        match git.list_remotes().await {
            Ok(remotes) if remotes.is_empty() => {
                Ok(ToolOutput::success("No remotes configured.".to_string()))
            }
            Ok(remotes) => {
                let page = match git_page_request(args, DEFAULT_GIT_LIST_LIMIT)
                    .and_then(|request| request.page(remotes))
                {
                    Ok(page) => page,
                    Err(error) => return Ok(ToolOutput::error(error)),
                };
                let entries: Vec<String> = page.items.iter().map(format_remote).collect();
                let continuation = page
                    .next_cursor
                    .as_deref()
                    .map(|cursor| {
                        format!("\nMore remotes available; continue with cursor={cursor}")
                    })
                    .unwrap_or_default();
                Ok(
                    ToolOutput::success(format!(
                        "Remotes:\n{}{}",
                        entries.join("\n"),
                        continuation
                    ))
                    .with_metadata(serde_json::json!({ "page": page.metadata() })),
                )
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to list remotes: {e}"))),
        }
    }

    /// Worktree operations.
    async fn worktree(
        &self,
        args: &serde_json::Value,
        worktree: &dyn WorkspaceGitWorktreeProvider,
    ) -> Result<ToolOutput> {
        let subcommand = args
            .get("subcommand")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        match subcommand {
            "list" => self.list_worktrees(args, worktree).await,
            "create" => self.create_worktree(args, worktree).await,
            "remove" => self.remove_worktree(args, worktree).await,
            _ => Ok(ToolOutput::error(format!(
                "Unknown worktree subcommand: {subcommand}. Use: list, create, remove"
            ))),
        }
    }

    /// List all worktrees.
    async fn list_worktrees(
        &self,
        args: &serde_json::Value,
        worktree: &dyn WorkspaceGitWorktreeProvider,
    ) -> Result<ToolOutput> {
        match worktree.list_worktrees().await {
            Ok(worktrees) => {
                let page = match git_page_request(args, DEFAULT_GIT_LIST_LIMIT)
                    .and_then(|request| request.page(worktrees))
                {
                    Ok(page) => page,
                    Err(error) => return Ok(ToolOutput::error(error)),
                };
                if page.items.is_empty() && page.total_items == 0 {
                    return Ok(ToolOutput::success("No worktrees found."));
                }

                let entries: Vec<String> = page
                    .items
                    .iter()
                    .map(|worktree| {
                        let suffix = if worktree.is_bare {
                            " (bare)".to_string()
                        } else if worktree.is_detached {
                            " (detached)".to_string()
                        } else {
                            format!(" [{}]", worktree.branch)
                        };
                        format!("  {}{}", worktree.path, suffix)
                    })
                    .collect();

                Ok(ToolOutput::success(format!(
                    "Worktrees ({} of {}):\n{}{}",
                    page.items.len(),
                    page.total_items,
                    entries.join("\n"),
                    page.next_cursor
                        .as_deref()
                        .map(|cursor| format!(
                            "\nMore worktrees available; continue with cursor={cursor}"
                        ))
                        .unwrap_or_default()
                ))
                .with_metadata(serde_json::json!({ "page": page.metadata() })))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to list worktrees: {e}"))),
        }
    }

    /// Create a new worktree.
    async fn create_worktree(
        &self,
        args: &serde_json::Value,
        worktree: &dyn WorkspaceGitWorktreeProvider,
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
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        match worktree
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: branch.to_string(),
                path,
                new_branch,
            })
            .await
        {
            Ok(result) => Ok(ToolOutput::success(format!(
                "Created worktree at: {}\nBranch: {branch}",
                result.path
            ))
            .with_metadata(serde_json::json!({
                "path": result.path,
                "branch": branch,
            }))),
            Err(e) => Ok(ToolOutput::error(format!("Failed to create worktree: {e}"))),
        }
    }

    /// Remove a worktree.
    async fn remove_worktree(
        &self,
        args: &serde_json::Value,
        worktree: &dyn WorkspaceGitWorktreeProvider,
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

        match worktree
            .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
                path: path.to_string(),
                force,
            })
            .await
        {
            Ok(result) => Ok(ToolOutput::success(format!(
                "Removed worktree at: {}",
                result.path
            ))),
            Err(e) => Ok(ToolOutput::error(format!("Failed to remove worktree: {e}"))),
        }
    }
}

fn short_commit_id(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

fn git_page_request(
    args: &serde_json::Value,
    default_limit: usize,
) -> std::result::Result<PageRequest, String> {
    if args.get("limit").is_some() || args.get("max_count").is_none() {
        return PageRequest::parse(args, default_limit, MAX_GIT_PAGE_LIMIT);
    }

    let mut normalized = args.clone();
    if let (Some(object), Some(max_count)) = (normalized.as_object_mut(), args.get("max_count")) {
        object.insert("limit".to_string(), max_count.clone());
    }
    PageRequest::parse(&normalized, default_limit, MAX_GIT_PAGE_LIMIT)
}

fn git_page_metadata(
    request: PageRequest,
    returned_items: usize,
    next_cursor: Option<String>,
    total_items: Option<usize>,
) -> serde_json::Value {
    let truncated = next_cursor.is_some();
    serde_json::json!({
        "offset": request.offset,
        "requested_limit": request.requested_limit,
        "applied_limit": request.limit,
        "returned_items": returned_items,
        "total_items": total_items,
        "next_cursor": next_cursor,
        "truncated": truncated,
        "limit_clamped": request.requested_limit != request.limit,
    })
}

fn bounded_diff_output(args: &serde_json::Value, diff: String) -> Result<ToolOutput> {
    let byte_offset = match args.get("byte_offset") {
        Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
            Some(value) => value,
            None => {
                return Ok(ToolOutput::error(
                    "byte_offset must be a non-negative integer",
                ))
            }
        },
        None => 0,
    };
    let requested_max_bytes = match args.get("max_bytes") {
        Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
            Some(value) if value >= MIN_DIFF_BYTES => value,
            _ => {
                return Ok(ToolOutput::error(format!(
                    "max_bytes must be an integer of at least {MIN_DIFF_BYTES}"
                )))
            }
        },
        None => DEFAULT_DIFF_BYTES,
    };
    let max_bytes = requested_max_bytes.min(MAX_OUTPUT_SIZE);
    if byte_offset > diff.len() || !diff.is_char_boundary(byte_offset) {
        return Ok(ToolOutput::error(format!(
            "byte_offset {byte_offset} is outside the diff or not on a UTF-8 boundary"
        )));
    }

    let mut end = byte_offset.saturating_add(max_bytes).min(diff.len());
    while end > byte_offset && !diff.is_char_boundary(end) {
        end -= 1;
    }
    let next_offset = (end < diff.len()).then_some(end);
    let mut output = diff[byte_offset..end].to_string();
    if let Some(next_offset) = next_offset {
        output.push_str(&format!(
            "\n\n... (more diff available; continue with byte_offset={next_offset})\n"
        ));
    }

    Ok(
        ToolOutput::success(output).with_metadata(serde_json::json!({
            "range": {
                "byte_offset": byte_offset,
                "requested_max_bytes": requested_max_bytes,
                "applied_max_bytes": max_bytes,
                "returned_bytes": end - byte_offset,
                "total_bytes": diff.len(),
                "next_byte_offset": next_offset,
                "eof": next_offset.is_none(),
                "limit_clamped": requested_max_bytes != max_bytes,
            }
        })),
    )
}

fn format_remote(remote: &WorkspaceGitRemote) -> String {
    let url = sanitize_remote_url(&remote.url);
    if remote.direction.is_empty() {
        format!("{}\t{url}", remote.name)
    } else {
        format!("{}\t{url} ({})", remote.name, remote.direction)
    }
}

fn sanitize_remote_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    if matches!(url.scheme(), "http" | "https") {
        let _ = url.set_username("");
        let _ = url.set_password(None);
    } else if url.password().is_some() {
        let _ = url.set_password(None);
    }
    url.set_query(None);
    url.set_fragment(None);
    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn run_git(path: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository_with_commits(count: usize) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        run_git(temp.path(), &["init", "-q"]);
        run_git(temp.path(), &["config", "user.email", "tests@a3s.local"]);
        run_git(temp.path(), &["config", "user.name", "A3S Tests"]);
        for index in 0..count {
            std::fs::write(temp.path().join("history.txt"), format!("{index}\n")).unwrap();
            run_git(temp.path(), &["add", "history.txt"]);
            run_git(
                temp.path(),
                &["commit", "-q", "-m", &format!("commit {index}")],
            );
        }
        temp
    }

    #[tokio::test]
    async fn test_git_not_installed() {
        // This test checks that the local provider handles non-git repos properly.
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

    #[tokio::test]
    async fn test_log_paginates_without_repeating_entries() {
        let repository = repository_with_commits(3);
        let tool = GitTool;
        let ctx = ToolContext::new(repository.path().to_path_buf());

        let first = tool
            .execute(&serde_json::json!({"command": "log", "limit": 2}), &ctx)
            .await
            .unwrap();
        assert!(first.success, "{}", first.content);
        assert!(first.content.contains("commit 2"));
        assert!(first.content.contains("commit 1"));
        assert!(!first.content.contains("commit 0"));
        assert_eq!(first.metadata.as_ref().unwrap()["page"]["next_cursor"], "2");

        let second = tool
            .execute(
                &serde_json::json!({"command": "log", "limit": 2, "cursor": "2"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(second.success, "{}", second.content);
        assert!(second.content.contains("commit 0"));
        assert!(!second.content.contains("commit 2"));
        assert_eq!(
            second.metadata.as_ref().unwrap()["page"]["next_cursor"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn test_diff_is_utf8_safe_and_resumable_by_byte_offset() {
        let repository = repository_with_commits(1);
        std::fs::write(repository.path().join("history.txt"), "界".repeat(2_000)).unwrap();
        let tool = GitTool;
        let ctx = ToolContext::new(repository.path().to_path_buf());

        let first = tool
            .execute(
                &serde_json::json!({"command": "diff", "max_bytes": 256}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(first.success, "{}", first.content);
        let metadata = first.metadata.as_ref().unwrap();
        assert!(metadata["range"]["returned_bytes"].as_u64().unwrap() <= 256);
        let next = metadata["range"]["next_byte_offset"].as_u64().unwrap();
        assert!(first.content.contains("more diff available"));

        let second = tool
            .execute(
                &serde_json::json!({
                    "command": "diff",
                    "max_bytes": 256,
                    "byte_offset": next,
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(second.success, "{}", second.content);
        assert_eq!(
            second.metadata.as_ref().unwrap()["range"]["byte_offset"],
            next
        );
        assert!(!second.content.contains("diff --git"));
    }

    #[test]
    fn remote_urls_do_not_expose_embedded_credentials_or_query_tokens() {
        let remote = WorkspaceGitRemote {
            name: "origin".to_string(),
            url: "https://user:secret@example.com/org/repo.git?access_token=hidden#fragment"
                .to_string(),
            direction: "fetch".to_string(),
        };

        let rendered = format_remote(&remote);

        assert_eq!(rendered, "origin\thttps://example.com/org/repo.git (fetch)");
        for secret in ["user", "secret", "access_token", "hidden", "fragment"] {
            assert!(!rendered.contains(secret), "{secret} leaked in {rendered}");
        }
    }
}
