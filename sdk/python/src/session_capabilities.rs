//! Python Session capability and verification methods.

use super::session::PySession;
use super::*;

#[pymethods]
impl PySession {
    /// Add or replace a Skill in this live session.
    ///
    /// The Skill tools and model-visible catalog observe it immediately.
    #[pyo3(signature = (name, content, kind="instruction"))]
    fn add_skill(&self, name: String, content: String, kind: &str) -> PyResult<()> {
        self.inner
            .add_skill(inline_skill_to_rust(name, content, kind)?)
            .map_err(py_code_error)
    }

    /// Remove a Skill installed through ``add_skill``.
    fn remove_skill(&self, name: String) -> PyResult<()> {
        self.inner.remove_skill(&name).map_err(py_code_error)
    }

    /// Return the names in the session's current live Skill registry.
    fn skill_names(&self) -> Vec<String> {
        self.inner.skill_names()
    }

    /// Add an MCP server to this live session.
    ///
    /// Connects the server and registers all its tools immediately so the agent
    /// can call them. Tool names follow the convention ``mcp__<name>__<tool>``.
    ///
    /// Args:
    ///     name: Server identifier (used as prefix in tool names)
    ///     transport: Transport type — ``"stdio"`` (default), ``"http"``, or ``"streamable-http"``
    ///     command: Executable to launch (stdio only, e.g. ``"npx"``)
    ///     args: Arguments for the command (stdio only)
    ///     url: Server URL (http / streamable-http only)
    ///     headers: HTTP headers dict (http / streamable-http only, e.g. ``{"Authorization": "Bearer ..."}``))
    ///     env: Optional dict of extra environment variables (stdio only)
    ///
    /// Returns:
    ///     Number of tools registered from the server
    ///
    /// Raises:
    ///     RuntimeError: If the server fails to connect
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (name, transport="stdio", command=None, args=None, url=None, headers=None, env=None, timeout_ms=None))]
    fn add_mcp_server(
        &self,
        py: Python<'_>,
        name: &str,
        transport: &str,
        command: Option<&str>,
        args: Option<Vec<String>>,
        url: Option<&str>,
        headers: Option<std::collections::HashMap<String, String>>,
        env: Option<std::collections::HashMap<String, String>>,
        timeout_ms: Option<u64>,
    ) -> PyResult<usize> {
        use a3s_code_core::mcp::protocol::{McpServerConfig, McpTransportConfig};

        let transport_config = match transport {
            "stdio" => {
                let command = command.ok_or_else(|| {
                    PyRuntimeError::new_err("'command' is required for stdio transport")
                })?;
                McpTransportConfig::Stdio {
                    command: command.to_string(),
                    args: args.unwrap_or_default(),
                }
            }
            "http" => {
                let url = url.ok_or_else(|| {
                    PyRuntimeError::new_err("'url' is required for http transport")
                })?;
                McpTransportConfig::Http {
                    url: url.to_string(),
                    headers: headers.unwrap_or_default(),
                }
            }
            "streamable-http" | "streamable_http" => {
                let url = url.ok_or_else(|| {
                    PyRuntimeError::new_err("'url' is required for streamable-http transport")
                })?;
                McpTransportConfig::StreamableHttp {
                    url: url.to_string(),
                    headers: headers.unwrap_or_default(),
                }
            }
            other => {
                return Err(PyRuntimeError::new_err(format!(
                    "Unknown transport '{}'. Use 'stdio', 'http', or 'streamable-http'",
                    other
                )))
            }
        };

        let tool_timeout_secs = timeout_ms.map(|ms| (ms / 1000).max(1)).unwrap_or(60);
        let config = McpServerConfig {
            name: name.to_string(),
            transport: transport_config,
            enabled: true,
            env: env.unwrap_or_default(),
            oauth: None,
            tool_timeout_secs,
        };
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async { session.add_mcp_server(config).await.map_err(py_code_error) })
        })
    }

    /// Add an MCP server with an object config.
    ///
    /// Preferred for new SDK callers because the transport is typed as a nested
    /// object instead of split across positional parameters.
    ///
    /// Example:
    ///     session.add_mcp_server_config({
    ///         "name": "github",
    ///         "transport": {
    ///             "type": "stdio",
    ///             "command": "npx",
    ///             "args": ["-y", "@modelcontextprotocol/server-github"],
    ///         },
    ///         "env": {"GITHUB_TOKEN": "..."},
    ///         "timeout_ms": 30000,
    ///     })
    fn add_mcp_server_config(&self, py: Python<'_>, config: &Bound<'_, PyDict>) -> PyResult<usize> {
        let json_str = py_dict_to_json(config)?;
        let value: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid MCP server config: {e}")))?;
        let config = normalize_mcp_server_config(value)?;
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async { session.add_mcp_server(config).await.map_err(py_code_error) })
        })
    }

    /// Add an MCP server with the compact object-shaped API.
    fn add_mcp(&self, py: Python<'_>, config: &Bound<'_, PyDict>) -> PyResult<usize> {
        self.add_mcp_server_config(py, config)
    }

    /// Dynamically register agents from a directory with the live session.
    ///
    /// Scans the given directory for ``*.yaml``, ``*.yml``, and ``*.md`` agent
    /// definition files and adds each to the shared agent registry used by the
    /// ``task`` tool.  New agents become immediately callable via
    /// ``task(agent="…")`` without restarting the session.
    ///
    /// Args:
    ///     path: Directory path to scan for agent definition files
    ///
    /// Returns:
    ///     Number of agents successfully loaded from the directory
    #[pyo3(signature = (path))]
    fn register_agent_dir(&self, py: Python<'_>, path: &str) -> PyResult<usize> {
        let dir = std::path::PathBuf::from(path);
        let session = self.inner.clone();
        py.allow_threads(move || {
            let count = session.register_agent_dir(&dir).map_err(py_code_error)?;
            Ok(count)
        })
    }

    /// Register a disposable worker agent into the live session.
    fn register_worker_agent(&self, worker: PyWorkerAgentSpec) -> PyResult<PyAgentDefinition> {
        let worker = py_worker_agent_spec_to_rust(worker)?;
        Ok(rust_agent_definition_to_py(
            self.inner
                .register_worker_agent(worker)
                .map_err(py_code_error)?,
        ))
    }

    /// Register many disposable worker agents into the live session.
    fn register_worker_agents(
        &self,
        workers: Vec<PyWorkerAgentSpec>,
    ) -> PyResult<Vec<PyAgentDefinition>> {
        let workers = workers
            .into_iter()
            .map(py_worker_agent_spec_to_rust)
            .collect::<PyResult<Vec<_>>>()?;
        Ok(self
            .inner
            .register_worker_agents(workers)
            .map_err(py_code_error)?
            .into_iter()
            .map(rust_agent_definition_to_py)
            .collect())
    }

    /// Register the built-in A3S Flow-backed ``dynamic_workflow`` tool into this live session.
    ///
    /// The tool becomes visible in ``tool_names()`` immediately and can be invoked
    /// through the ordinary ``tool("dynamic_workflow", ...)`` direct-call path or
    /// selected by the model on subsequent runs.
    fn register_dynamic_workflow_runtime(&self) -> PyResult<()> {
        self.inner
            .register_dynamic_workflow_runtime()
            .map_err(py_code_error)
    }

    /// Remove a previously registered dynamic tool from this live session.
    ///
    /// This is primarily used to unregister host/runtime-added tools such as
    /// ``dynamic_workflow`` when a capability is disabled.
    fn unregister_dynamic_tool(&self, name: String) -> PyResult<()> {
        self.inner
            .unregister_dynamic_tool(&name)
            .map_err(py_code_error)
    }

    /// Remove an MCP server from this session.
    ///
    /// Disconnects the server and unregisters all its tools.
    /// No-op if the server was never added.
    ///
    /// Args:
    ///     name: Server identifier used when it was added
    #[pyo3(signature = (name))]
    fn remove_mcp_server(&self, py: Python<'_>, name: &str) -> PyResult<()> {
        let name = name.to_string();
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async {
                session
                    .remove_mcp_server(&name)
                    .await
                    .map_err(py_code_error)
            })
        })
    }

    /// Remove an MCP server with the compact API.
    fn remove_mcp(&self, py: Python<'_>, name: &str) -> PyResult<()> {
        self.remove_mcp_server(py, name)
    }

    /// Return the connection status of all MCP servers for this session.
    ///
    /// Returns:
    ///     Dict mapping server name to status dict with keys:
    ///     ``connected`` (bool), ``tool_count`` (int).
    fn mcp_status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let session = self.inner.clone();
        let status = py.allow_threads(move || get_runtime().block_on(session.mcp_status()));
        let dict = PyDict::new(py);
        for (name, s) in status {
            let entry = PyDict::new(py);
            entry.set_item("connected", s.connected)?;
            entry.set_item("tool_count", s.tool_count)?;
            entry.set_item("error", s.error.as_deref())?;
            dict.set_item(name, entry)?;
        }
        Ok(dict)
    }

    /// Return MCP server status with the compact API.
    fn mcps<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.mcp_status(py)
    }

    /// Return the names of all tools currently available in this session.
    ///
    /// Reflects the live state — MCP tools appear after ``add_mcp_server()``
    /// or ``add_mcp_server_config()``
    /// and disappear after ``remove_mcp_server()``.
    ///
    /// Returns:
    ///     List of tool name strings
    fn tool_names<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let names = self.inner.tool_names();
        let list = PyList::new(py, names)?;
        Ok(list)
    }

    /// Return full model-visible tool definitions currently registered on this session.
    fn tool_definitions(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.tool_definitions()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize tool definitions: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return a stored tool artifact by URI, or ``None`` if it is not retained.
    pub(super) fn get_artifact(&self, py: Python<'_>, artifact_uri: &str) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.get_artifact(artifact_uri))
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize artifact: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return compact execution trace events recorded for this session.
    fn trace_events(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.trace_events())
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize traces: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return structured verification reports recorded for this session.
    fn verification_reports(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.verification_reports()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification reports: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Add externally produced verification reports to this session.
    pub(super) fn record_verification_reports(
        &self,
        py: Python<'_>,
        reports: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let reports = py_verification_reports_to_rust(py, reports)?;
        self.inner.record_verification_reports(reports);
        Ok(())
    }

    /// Return a structured verification summary for this session.
    fn verification_summary(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.verification_summary()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification summary: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return a concise human-readable verification summary for this session.
    fn verification_summary_text(&self) -> String {
        self.inner.verification_summary_text()
    }

    /// Run verification commands and return a structured verification report.
    fn verify_commands(
        &self,
        py: Python<'_>,
        subject: &str,
        commands: &Bound<'_, PyList>,
    ) -> PyResult<PyObject> {
        let rust_commands = py_list_to_verification_commands(commands)?;
        let session = self.inner.clone();
        let subject = subject.to_string();
        let report = py
            .allow_threads(move || {
                get_runtime().block_on(session.verify_commands(&subject, &rust_commands))
            })
            .map_err(py_code_error)?;
        let json = serde_json::to_string(&report).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification report: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return project-aware verification command presets for this workspace.
    fn verification_presets(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.verification_presets()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification presets: {e}"))
        })?;
        json_string_to_py(py, &json)
    }
}
