//! Python Session direct-tool methods.

use super::session::PySession;
use super::*;

#[pymethods]
impl PySession {
    /// Execute a tool by name, bypassing the LLM and treating the host as the
    /// authority that already approved permission and HITL requirements.
    fn tool(
        &self,
        py: Python<'_>,
        name: String,
        args: &Bound<'_, pyo3::types::PyDict>,
    ) -> PyResult<PyToolResult> {
        let json_str = py_dict_to_json(args)?;
        let json_value: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid JSON args: {e}")))?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool(&name, json_value)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Execute a tool without an LLM while retaining the session's permission
    /// and HITL gates.
    fn governed_tool(
        &self,
        py: Python<'_>,
        name: String,
        args: &Bound<'_, pyo3::types::PyDict>,
    ) -> PyResult<PyToolResult> {
        let json_str = py_dict_to_json(args)?;
        let json_value: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid JSON args: {e}")))?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.governed_tool(&name, json_value)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Return an asyncio Future for a trusted host direct-tool call.
    fn tool_async<'py>(
        &self,
        py: Python<'py>,
        name: String,
        args: &Bound<'_, PyDict>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let json = py_dict_to_json(args)?;
        let args = serde_json::from_str(&json)
            .map_err(|error| PyValueError::new_err(format!("Invalid JSON args: {error}")))?;
        let callable = Bound::new(
            py,
            AsyncSessionToolCall {
                session: Arc::clone(&self.inner),
                name: Some(name),
                args: Some(args),
                mode: AsyncSessionToolMode::Trusted,
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Return an asyncio Future for a direct-tool call that retains the
    /// session's permission and HITL gates.
    fn governed_tool_async<'py>(
        &self,
        py: Python<'py>,
        name: String,
        args: &Bound<'_, PyDict>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let json = py_dict_to_json(args)?;
        let args = serde_json::from_str(&json)
            .map_err(|error| PyValueError::new_err(format!("Invalid JSON args: {error}")))?;
        let callable = Bound::new(
            py,
            AsyncSessionToolCall {
                session: Arc::clone(&self.inner),
                name: Some(name),
                args: Some(args),
                mode: AsyncSessionToolMode::Governed,
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Delegate a bounded task with the compact object-shaped API.
    fn task(&self, py: Python<'_>, options: &Bound<'_, PyDict>) -> PyResult<PyToolResult> {
        let json_str = py_dict_to_json(options)?;
        let args: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid task options: {e}")))?;
        let args = normalize_task_options(args)?;
        let args = delegated_tasks_args(serde_json::json!([args]))?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("task", args)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Delegate a bounded task to a child agent through the built-in ``task`` tool.
    #[pyo3(signature = (agent, description, prompt, background=false, max_steps=None))]
    fn delegate_task(
        &self,
        py: Python<'_>,
        agent: String,
        description: String,
        prompt: String,
        background: bool,
        max_steps: Option<u32>,
    ) -> PyResult<PyToolResult> {
        let args = delegate_task_args(agent, description, prompt, background, max_steps);
        let args = delegated_tasks_args(serde_json::json!([args]))?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("task", args)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Execute several delegated child-agent tasks with the compact API.
    fn tasks(&self, py: Python<'_>, tasks: &Bound<'_, PyAny>) -> PyResult<PyToolResult> {
        let json_mod = py.import("json")?;
        let json_str: String = json_mod.call_method1("dumps", (tasks,))?.extract()?;
        let task_values: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid task list: {e}")))?;
        let args = delegated_tasks_args(task_values)?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("task", args)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Compatibility helper for the legacy hidden ``parallel_task`` host tool.
    /// Prefer ``tasks()`` for new code.
    fn parallel_task(&self, py: Python<'_>, tasks: &Bound<'_, PyAny>) -> PyResult<PyToolResult> {
        let json_mod = py.import("json")?;
        let json_str: String = json_mod.call_method1("dumps", (tasks,))?.extract()?;
        let task_values: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid task list: {e}")))?;
        let args = delegated_tasks_args(task_values)?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("parallel_task", args)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Run a bounded JavaScript script through the embedded QuickJS `program` tool.
    fn program(
        &self,
        py: Python<'_>,
        options: &Bound<'_, pyo3::types::PyDict>,
    ) -> PyResult<PyToolResult> {
        let args = normalize_program_script_options(options)?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("program", args)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Read a file from the workspace.
    #[pyo3(signature = (path, offset=None, limit=None))]
    fn read_file(
        &self,
        py: Python<'_>,
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> PyResult<String> {
        let session = self.inner.clone();
        let options = a3s_code_core::ReadFileOptions { offset, limit };
        py.allow_threads(move || {
            get_runtime().block_on(session.read_file_with_options(&path, options))
        })
        .map_err(py_code_error)
    }

    /// Write a file in the workspace.
    fn write_file(&self, py: Python<'_>, path: String, content: String) -> PyResult<PyToolResult> {
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.write_file(&path, &content)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// List a directory in the workspace.
    #[pyo3(signature = (path=None))]
    fn ls(&self, py: Python<'_>, path: Option<String>) -> PyResult<PyToolResult> {
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.ls(path.as_deref())))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Edit a file by replacing text in the workspace.
    #[pyo3(signature = (path, old_string, new_string, replace_all=false))]
    fn edit_file(
        &self,
        py: Python<'_>,
        path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    ) -> PyResult<PyToolResult> {
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || {
                get_runtime().block_on(session.edit_file(
                    &path,
                    &old_string,
                    &new_string,
                    replace_all,
                ))
            })
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Apply a unified diff patch to a workspace file.
    fn patch_file(&self, py: Python<'_>, path: String, diff: String) -> PyResult<PyToolResult> {
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.patch_file(&path, &diff)))
            .map_err(py_code_error)?;

        Ok(PyToolResult::from(result))
    }

    /// Execute a bash command in the workspace.
    fn bash(&self, py: Python<'_>, command: String) -> PyResult<String> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.bash(&command)))
            .map_err(py_code_error)
    }

    /// Search for files matching a glob pattern.
    fn glob(&self, py: Python<'_>, pattern: String) -> PyResult<Vec<String>> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.glob(&pattern)))
            .map_err(py_code_error)
    }

    /// Search file contents with a regex pattern.
    fn grep(&self, py: Python<'_>, pattern: String) -> PyResult<String> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.grep(&pattern)))
            .map_err(py_code_error)
    }

    /// Search the web using multiple search engines.
    fn web_search(&self, py: Python<'_>, params: PyWebSearchParams) -> PyResult<PyToolResult> {
        let session = self.inner.clone();
        let mut args = serde_json::json!({
            "query": params.query,
        });
        if let Some(ref engines) = params.engines {
            args["engines"] = serde_json::json!(engines);
        }
        if let Some(limit) = params.limit {
            args["limit"] = serde_json::json!(limit);
        }
        if let Some(timeout) = params.timeout {
            args["timeout"] = serde_json::json!(timeout);
        }
        if let Some(ref proxy) = params.proxy {
            args["proxy"] = serde_json::json!(proxy);
        }
        if let Some(ref format) = params.format {
            args["format"] = serde_json::json!(format);
        }
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("web_search", args)))
            .map_err(py_code_error)?;
        Ok(PyToolResult::from(result))
    }

    /// Execute a git command.
    ///
    /// Prefer ``git({"command": "status"})``; positional arguments remain for
    /// compatibility.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (command, subcommand=None, name=None, path=None, new_branch=true, base=None, force=false, max_count=None, message=None, include_untracked=false, target=None, reference=None))]
    fn git(
        &self,
        py: Python<'_>,
        command: &Bound<'_, PyAny>,
        subcommand: Option<String>,
        name: Option<String>,
        path: Option<String>,
        new_branch: bool,
        base: Option<String>,
        force: bool,
        max_count: Option<usize>,
        message: Option<String>,
        include_untracked: bool,
        target: Option<String>,
        reference: Option<String>,
    ) -> PyResult<PyToolResult> {
        let mut args = if let Ok(command) = command.extract::<String>() {
            serde_json::json!({ "command": command })
        } else if let Ok(config) = command.downcast::<PyDict>() {
            let json_str = py_dict_to_json(config)?;
            let args: serde_json::Value = serde_json::from_str(&json_str)
                .map_err(|e| PyValueError::new_err(format!("Invalid git args: {e}")))?;
            normalize_git_args(args)?
        } else {
            return Err(PyTypeError::new_err(
                "git command must be a command string or options dict",
            ));
        };

        if let Some(sc) = subcommand {
            args["subcommand"] = serde_json::json!(sc);
        }
        if let Some(n) = name {
            args["name"] = serde_json::json!(n);
        }
        if let Some(p) = path {
            args["path"] = serde_json::json!(p);
        }
        if !new_branch {
            args["new_branch"] = serde_json::json!(new_branch);
        }
        if let Some(b) = base {
            args["base"] = serde_json::json!(b);
        }
        if force {
            args["force"] = serde_json::json!(force);
        }
        if let Some(mc) = max_count {
            args["max_count"] = serde_json::json!(mc);
        }
        if let Some(msg) = message {
            args["message"] = serde_json::json!(msg);
        }
        if include_untracked {
            args["include_untracked"] = serde_json::json!(include_untracked);
        }
        if let Some(t) = target {
            args["target"] = serde_json::json!(t);
        }
        if let Some(r) = reference {
            args["ref"] = serde_json::json!(r);
        }

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("git", args)))
            .map_err(py_code_error)?;
        Ok(PyToolResult::from(result))
    }

    /// Execute a git command with an object-shaped API.
    ///
    /// Preferred over the positional ``git(...)`` overload for new callers.
    ///
    /// Example:
    ///     session.git_command({"command": "status"})
    ///     session.git_command({"command": "worktree", "subcommand": "list"})
    fn git_command(&self, py: Python<'_>, args: &Bound<'_, PyDict>) -> PyResult<PyToolResult> {
        let json_str = py_dict_to_json(args)?;
        let args: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid git args: {e}")))?;
        let args = normalize_git_args(args)?;
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("git", args)))
            .map_err(py_code_error)?;
        Ok(PyToolResult::from(result))
    }
}
