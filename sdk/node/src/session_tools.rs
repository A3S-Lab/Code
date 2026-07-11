//! Node Session direct-tool methods.

use super::session::Session;
use super::*;

#[napi]
impl Session {
    /// Execute a tool by name, bypassing the LLM.
    #[napi]
    pub async fn tool(&self, name: String, args: serde_json::Value) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool(&name, args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Delegate a bounded task to a child agent through the built-in `task` tool.
    #[napi(ts_args_type = "options: DelegateTaskOptions")]
    pub async fn task(&self, options: DelegateTaskOptions) -> napi::Result<ToolResult> {
        let args = delegate_task_options_to_args(options);

        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool("task", args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Delegate a bounded task to a child agent through the built-in `task` tool.
    #[napi(ts_args_type = "options: DelegateTaskOptions")]
    pub async fn delegate_task(&self, options: DelegateTaskOptions) -> napi::Result<ToolResult> {
        self.task(options).await
    }

    /// Execute several delegated child-agent tasks concurrently through `parallel_task`.
    #[napi(ts_args_type = "tasks: DelegateTaskOptions[]")]
    pub async fn tasks(&self, tasks: Vec<DelegateTaskOptions>) -> napi::Result<ToolResult> {
        let args = parallel_task_options_to_args(tasks);

        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool("parallel_task", args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Execute several delegated child-agent tasks concurrently through `parallel_task`.
    #[napi(
        js_name = "parallelTask",
        ts_args_type = "tasks: DelegateTaskOptions[]"
    )]
    pub async fn parallel_task(&self, tasks: Vec<DelegateTaskOptions>) -> napi::Result<ToolResult> {
        self.tasks(tasks).await
    }

    /// Run a bounded JavaScript script through the embedded QuickJS `program` tool.
    #[napi(ts_args_type = "options: ProgramScriptOptions")]
    pub async fn program(&self, options: serde_json::Value) -> napi::Result<ToolResult> {
        let args = normalize_program_script_options(options)?;
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool("program", args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Read a file from the workspace.
    #[napi(ts_args_type = "path: string, options?: ReadFileOptions | null")]
    pub async fn read_file(
        &self,
        path: String,
        options: Option<ReadFileOptions>,
    ) -> napi::Result<String> {
        let session = self.inner.clone();
        let options = options
            .map(|options| a3s_code_core::ReadFileOptions {
                offset: options.offset.map(|value| value as usize),
                limit: options.limit.map(|value| value as usize),
            })
            .unwrap_or_default();
        get_runtime()
            .spawn(async move { session.read_file_with_options(&path, options).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    /// Write a file in the workspace.
    #[napi]
    pub async fn write_file(&self, path: String, content: String) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.write_file(&path, &content).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// List a directory in the workspace.
    #[napi]
    pub async fn ls(&self, path: Option<String>) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.ls(path.as_deref()).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Edit a file by replacing text in the workspace.
    #[napi]
    pub async fn edit_file(
        &self,
        path: String,
        old_string: String,
        new_string: String,
        replace_all: Option<bool>,
    ) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move {
                session
                    .edit_file(
                        &path,
                        &old_string,
                        &new_string,
                        replace_all.unwrap_or(false),
                    )
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Apply a unified diff patch to a workspace file.
    #[napi]
    pub async fn patch_file(&self, path: String, diff: String) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.patch_file(&path, &diff).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Execute a bash command in the workspace.
    #[napi]
    pub async fn bash(&self, command: String) -> napi::Result<String> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.bash(&command).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    /// Search for files matching a glob pattern.
    #[napi]
    pub async fn glob(&self, pattern: String) -> napi::Result<Vec<String>> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.glob(&pattern).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    /// Search file contents with a regex pattern.
    #[napi]
    pub async fn grep(&self, pattern: String) -> napi::Result<String> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.grep(&pattern).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    /// Search the web using multiple search engines.
    #[napi]
    pub async fn web_search(&self, params: JsWebSearchParams) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let args = serde_json::json!({
            "query": params.query,
            "engines": params.engines,
            "limit": params.limit,
            "timeout": params.timeout,
            "proxy": params.proxy,
            "format": params.format,
        });
        get_runtime()
            .spawn(async move {
                session.tool("web_search", args).await.map(|r| ToolResult {
                    name: r.name,
                    output: r.output,
                    exit_code: r.exit_code,
                    metadata_json: r.metadata.and_then(|m| serde_json::to_string(&m).ok()),
                    document_runtime_json: None,
                    error_kind_json: r
                        .error_kind
                        .as_ref()
                        .and_then(|k| serde_json::to_string(k).ok()),
                })
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)
    }

    /// Execute a git command.
    ///
    /// Prefer `git({ command: "status" })`; positional arguments remain for
    /// compatibility.
    #[allow(clippy::too_many_arguments)]
    #[napi(
        ts_args_type = "command: string | GitCommandOptions, subcommand?: string | null, name?: string | null, path?: string | null, newBranch?: boolean | null, base?: string | null, force?: boolean | null, maxCount?: number | null, message?: string | null, includeUntracked?: boolean | null, target?: string | null, reference?: string | null"
    )]
    pub async fn git(
        &self,
        command: Either<String, GitCommandOptions>,
        subcommand: Option<String>,
        name: Option<String>,
        path: Option<String>,
        new_branch: Option<bool>,
        base: Option<String>,
        force: Option<bool>,
        max_count: Option<u32>,
        message: Option<String>,
        include_untracked: Option<bool>,
        target: Option<String>,
        reference: Option<String>,
    ) -> napi::Result<ToolResult> {
        let mut args = match command {
            Either::A(command) => serde_json::json!({ "command": command }),
            Either::B(options) => git_command_options_to_args(options),
        };

        if args.is_object() {
            if let Some(sc) = subcommand {
                args["subcommand"] = serde_json::json!(sc);
            }
            if let Some(n) = name {
                args["name"] = serde_json::json!(n);
            }
            if let Some(p) = path {
                args["path"] = serde_json::json!(p);
            }
            if let Some(nb) = new_branch {
                args["new_branch"] = serde_json::json!(nb);
            }
            if let Some(b) = base {
                args["base"] = serde_json::json!(b);
            }
            if let Some(f) = force {
                args["force"] = serde_json::json!(f);
            }
            if let Some(mc) = max_count {
                args["max_count"] = serde_json::json!(mc);
            }
            if let Some(msg) = message {
                args["message"] = serde_json::json!(msg);
            }
            if let Some(iu) = include_untracked {
                args["include_untracked"] = serde_json::json!(iu);
            }
            if let Some(t) = target {
                args["target"] = serde_json::json!(t);
            }
            if let Some(r) = reference {
                args["ref"] = serde_json::json!(r);
            }
        }

        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool("git", args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }

    /// Execute a git command with an object-shaped API.
    ///
    /// Preferred over the positional `git(...)` overload for new callers.
    ///
    /// ```js
    /// await session.gitCommand({ command: 'status' })
    /// await session.gitCommand({ command: 'worktree', subcommand: 'list' })
    /// ```
    #[napi(js_name = "gitCommand", ts_args_type = "args: GitCommandOptions")]
    pub async fn git_command(&self, args: serde_json::Value) -> napi::Result<ToolResult> {
        let args = normalize_git_args(args)?;
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool("git", args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(tool_result_from_core(result))
    }
}
