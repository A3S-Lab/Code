use super::*;

// ============================================================================
// Session
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
#[pyclass(name = "Session")]
pub(super) struct PySession {
    pub(super) inner: Arc<RustAgentSession>,
}

#[pymethods]
impl PySession {
    /// Send a prompt or request and wait for the complete response.
    ///
    /// Args:
    ///     prompt: Prompt string, or {"prompt": str, "history": list, "attachments": list}
    ///     history: Optional conversation history as list of dicts
    ///              `[{"role": "user", "content": [{"type": "text", "text": "..."}]}]`
    #[pyo3(signature = (prompt, history=None))]
    fn send(
        &self,
        py: Python<'_>,
        prompt: &Bound<'_, PyAny>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAgentResult> {
        let (prompt, rust_history, rust_attachments) = py_session_input_to_parts(prompt, history)?;
        let session = self.inner.clone();
        let result = if rust_attachments.is_empty() {
            py.allow_threads(move || {
                get_runtime().block_on(session.send(&prompt, rust_history.as_deref()))
            })
        } else {
            py.allow_threads(move || {
                get_runtime().block_on(session.send_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
        }
        .map_err(py_code_error)?;
        Ok(PyAgentResult::from(result))
    }

    /// Return an asyncio Future that sends a request without blocking the
    /// current event-loop task. Input and result shapes match ``send()``.
    #[pyo3(signature = (prompt, history=None))]
    fn send_async<'py>(
        &self,
        py: Python<'py>,
        prompt: &Bound<'_, PyAny>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let (prompt, history, attachments) = py_session_input_to_parts(prompt, history)?;
        let callable = Bound::new(
            py,
            AsyncSessionSendCall {
                session: Arc::clone(&self.inner),
                prompt: Some(prompt),
                history,
                attachments: Some(attachments),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Alias for ``send(...)`` with a name that matches run/replay terminology.
    #[pyo3(signature = (prompt, history=None))]
    fn run(
        &self,
        py: Python<'_>,
        prompt: &Bound<'_, PyAny>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAgentResult> {
        self.send(py, prompt, history)
    }

    /// Resume a previously-checkpointed run on this session.
    ///
    /// Loads the latest loop checkpoint stored under ``checkpoint_run_id``
    /// and replays the agent loop from that boundary. A new run id is
    /// allocated for the resumed work.
    ///
    /// Raises ``RuntimeError`` when no ``session_store`` is configured,
    /// or when no checkpoint exists for the given id.
    fn resume_run(&self, py: Python<'_>, checkpoint_run_id: String) -> PyResult<PyAgentResult> {
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.resume_run(&checkpoint_run_id)))
            .map_err(py_code_error)?;
        Ok(PyAgentResult::from(result))
    }

    /// Run `specs` as a fan-out of agent steps and return each step's outcome
    /// (a dict) in input order. Each spec is a dict with snake_case keys:
    /// `task_id`, `agent`, `description`, `prompt`, optional `max_steps`,
    /// `parent_session_id`, `output_schema`. A failed step surfaces as
    /// `success: False` without failing the batch.
    ///
    /// Pass `budget_tokens` to run the fan-out under one shared token budget:
    /// every child agent feeds a single ledger and, once the cap is reached,
    /// further child LLM calls are denied (a soft cap; the in-flight fan-out is
    /// never force-killed). With a budget the result is a dict
    /// `{"outcomes": [...], "budget": {"consumed_tokens", "limit_tokens"}}`;
    /// without one it is the plain list of outcome dicts, unchanged.
    #[pyo3(signature = (specs, budget_tokens=None))]
    fn parallel(
        &self,
        py: Python<'_>,
        specs: Vec<Bound<'_, PyAny>>,
        budget_tokens: Option<u64>,
    ) -> PyResult<PyObject> {
        let rust_specs = specs
            .iter()
            .map(|s| py_to_step_spec(py, s))
            .collect::<PyResult<Vec<_>>>()?;
        let session = self.inner.clone();

        // No budget → unchanged behavior: a plain list of outcome dicts.
        let Some(limit) = budget_tokens else {
            let outcomes = py.allow_threads(move || {
                get_runtime().block_on(async move {
                    let executor = session.agent_executor();
                    execute_steps_parallel(executor, rust_specs, None).await
                })
            });
            let items = outcomes
                .iter()
                .map(|o| step_outcome_to_py(py, o))
                .collect::<PyResult<Vec<_>>>()?;
            return Ok(PyList::new(py, items)?.into_any().unbind());
        };

        // Budget → shared ledger across the fan-out; return {"outcomes", "budget"}.
        let (outcomes, snapshot) = py.allow_threads(move || {
            get_runtime().block_on(async move {
                let wf = session.workflow_with_token_budget(Some(limit));
                let outcomes = wf.parallel(rust_specs).await;
                (outcomes, wf.budget_snapshot())
            })
        });
        let outcomes_py = outcomes
            .iter()
            .map(|o| step_outcome_to_py(py, o))
            .collect::<PyResult<Vec<_>>>()?;
        let budget = PyDict::new(py);
        budget.set_item(
            "consumed_tokens",
            snapshot.as_ref().map(|b| b.consumed_tokens).unwrap_or(0),
        )?;
        budget.set_item(
            "limit_tokens",
            snapshot
                .as_ref()
                .and_then(|b| b.limit_tokens)
                .or(Some(limit)),
        )?;
        let result = PyDict::new(py);
        result.set_item("outcomes", outcomes_py)?;
        result.set_item("budget", budget)?;
        Ok(result.into_any().unbind())
    }

    /// Like `parallel`, but resumable: progress is journaled under
    /// `workflow_id` via the session's store, so an interrupted run skips
    /// already-completed steps. Raises if no `session_store` is configured.
    fn parallel_resumable(
        &self,
        py: Python<'_>,
        specs: Vec<Bound<'_, PyAny>>,
        workflow_id: String,
    ) -> PyResult<Vec<PyObject>> {
        let rust_specs = specs
            .iter()
            .map(|s| py_to_step_spec(py, s))
            .collect::<PyResult<Vec<_>>>()?;
        let session = self.inner.clone();
        let outcomes = py
            .allow_threads(move || {
                get_runtime().block_on(async move {
                    let Some(store) = session.session_store() else {
                        return Err("parallel_resumable requires a session_store on the session");
                    };
                    let executor = session.agent_executor();
                    Ok(execute_steps_parallel_resumable(
                        executor,
                        rust_specs,
                        &workflow_id,
                        store,
                        None,
                    )
                    .await)
                })
            })
            .map_err(PyRuntimeError::new_err)?;
        outcomes.iter().map(|o| step_outcome_to_py(py, o)).collect()
    }

    /// Run each item through a chain of `stages`, with no barrier between
    /// stages. Each stage is a callable `stage(ctx) -> spec_dict | None`, where
    /// `ctx = {"previous": <outcome dict or None>, "item": <item>}`. Return a
    /// spec dict (snake_case keys) to run that step, or `None` to stop the
    /// item's chain. A chain also stops when a step fails. Returns one entry
    /// per item (the last outcome dict, or `None`), in input order.
    ///
    /// A stage callable that raises is caught and treated as `None` (stops that
    /// chain). Per-stage `output_schema` is not supported here — use `parallel`
    /// for schema-validated steps.
    fn pipeline(
        &self,
        py: Python<'_>,
        items: Vec<Bound<'_, PyAny>>,
        stages: Vec<Bound<'_, PyAny>>,
    ) -> PyResult<Vec<Option<PyObject>>> {
        let rust_items = items
            .iter()
            .map(|i| py_to_json_value(py, i))
            .collect::<PyResult<Vec<_>>>()?;
        let rust_stages: Vec<RustPipelineStage<serde_json::Value>> = stages
            .into_iter()
            .map(|s| {
                let stage = std::sync::Arc::new(PythonPipelineStage {
                    callback: s.unbind(),
                });
                let ps: RustPipelineStage<serde_json::Value> =
                    std::sync::Arc::new(move |prev, item| stage.invoke(prev, item));
                ps
            })
            .collect();

        let session = self.inner.clone();
        let outcomes = py.allow_threads(move || {
            get_runtime().block_on(async move {
                let executor = session.agent_executor();
                execute_pipeline(executor, rust_items, rust_stages, None).await
            })
        });

        outcomes
            .iter()
            .map(|o| match o {
                Some(outcome) => step_outcome_to_py(py, outcome).map(Some),
                None => Ok(None),
            })
            .collect()
    }

    /// Send a prompt or request and get a streaming iterator of events.
    ///
    /// When ``history`` is omitted, session history and verification evidence are
    /// updated after the stream completes. Supplying ``history`` keeps the stream isolated.
    ///
    /// Args:
    ///     prompt: Prompt string, or {"prompt": str, "history": list, "attachments": list}
    ///     history: Optional conversation history (same format as send)
    #[pyo3(signature = (prompt, history=None))]
    fn stream(
        &self,
        py: Python<'_>,
        prompt: &Bound<'_, PyAny>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyEventStream> {
        let (prompt, rust_history, rust_attachments) = py_session_input_to_parts(prompt, history)?;
        let session = self.inner.clone();
        let (rx, handle) = if rust_attachments.is_empty() {
            py.allow_threads(move || {
                get_runtime().block_on(session.stream(&prompt, rust_history.as_deref()))
            })
        } else {
            py.allow_threads(move || {
                get_runtime().block_on(session.stream_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
        }
        .map_err(py_code_error)?;

        Ok(PyEventStream {
            rx: Arc::new(Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(Mutex::new(Some(handle))),
        })
    }

    /// Send a request using the long-lived object-shaped API.
    ///
    /// Prefer this for new integrations when the call may need history,
    /// attachments, or future request options.
    fn send_request(&self, py: Python<'_>, request: &Bound<'_, PyDict>) -> PyResult<PyAgentResult> {
        let (prompt, rust_history, rust_attachments) = py_session_request_to_parts(request)?;
        let session = self.inner.clone();

        let result = if rust_attachments.is_empty() {
            py.allow_threads(move || {
                get_runtime().block_on(session.send(&prompt, rust_history.as_deref()))
            })
        } else {
            py.allow_threads(move || {
                get_runtime().block_on(session.send_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
        }
        .map_err(py_code_error)?;

        Ok(PyAgentResult::from(result))
    }

    /// Stream a request using the long-lived object-shaped API.
    fn stream_request(
        &self,
        py: Python<'_>,
        request: &Bound<'_, PyDict>,
    ) -> PyResult<PyEventStream> {
        let (prompt, rust_history, rust_attachments) = py_session_request_to_parts(request)?;
        let session = self.inner.clone();

        let (rx, handle) = if rust_attachments.is_empty() {
            py.allow_threads(move || {
                get_runtime().block_on(session.stream(&prompt, rust_history.as_deref()))
            })
        } else {
            py.allow_threads(move || {
                get_runtime().block_on(session.stream_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
        }
        .map_err(py_code_error)?;

        Ok(PyEventStream {
            rx: Arc::new(Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(Mutex::new(Some(handle))),
        })
    }

    /// Send a prompt with image attachments and wait for the complete response.
    ///
    /// Args:
    ///     prompt: The prompt to send
    ///     attachments: List of dicts with `{"data": bytes, "media_type": str}`
    ///     history: Optional conversation history
    #[pyo3(signature = (prompt, attachments, history=None))]
    fn send_with_attachments(
        &self,
        py: Python<'_>,
        prompt: String,
        attachments: Vec<Bound<'_, PyDict>>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAgentResult> {
        let rust_attachments = py_attachments_to_rust(&attachments)?;
        let rust_history = history.map(|h| py_list_to_messages(h)).transpose()?;
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || {
                get_runtime().block_on(session.send_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
            .map_err(py_code_error)?;
        Ok(PyAgentResult::from(result))
    }

    /// Stream a prompt with image attachments.
    ///
    /// When ``history`` is omitted, session history and verification evidence are
    /// updated after the stream completes. Supplying ``history`` keeps the stream isolated.
    ///
    /// Args:
    ///     prompt: The prompt to send
    ///     attachments: List of dicts with `{"data": bytes, "media_type": str}`
    ///     history: Optional conversation history
    #[pyo3(signature = (prompt, attachments, history=None))]
    fn stream_with_attachments(
        &self,
        py: Python<'_>,
        prompt: String,
        attachments: Vec<Bound<'_, PyDict>>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyEventStream> {
        let rust_attachments = py_attachments_to_rust(&attachments)?;
        let rust_history = history.map(|h| py_list_to_messages(h)).transpose()?;
        let session = self.inner.clone();
        let (rx, handle) = py
            .allow_threads(move || {
                get_runtime().block_on(session.stream_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
            .map_err(py_code_error)?;
        Ok(PyEventStream {
            rx: Arc::new(Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(Mutex::new(Some(handle))),
        })
    }

    /// Return the session's conversation history as a list of dicts.
    ///
    /// Each dict has `{"role": str, "content": [{"type": "text", "text": str}, ...]}`.
    fn history<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let messages = self.inner.history();
        messages_to_py_list(py, &messages)
    }

    /// Return run snapshots recorded by this session.
    fn runs(&self, py: Python<'_>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let runs = py.allow_threads(move || get_runtime().block_on(session.runs()));
        let json = serde_json::to_string(&runs)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize runs: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return an asyncio Future for ``runs()``.
    fn runs_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        async_run_query_future(py, Arc::clone(&self.inner), AsyncRunQueryOperation::Runs)
    }

    /// Return a run snapshot by ID, or None when it is unknown.
    fn run_snapshot(&self, py: Python<'_>, run_id: String) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let snapshot =
            py.allow_threads(move || get_runtime().block_on(session.run_snapshot(&run_id)));
        let json = serde_json::to_string(&snapshot).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize run snapshot: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return an asyncio Future for ``run_snapshot()``.
    fn run_snapshot_async<'py>(
        &self,
        py: Python<'py>,
        run_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        async_run_query_future(
            py,
            Arc::clone(&self.inner),
            AsyncRunQueryOperation::Snapshot { run_id },
        )
    }

    /// Return recorded runtime events for a run.
    fn run_events(&self, py: Python<'_>, run_id: String) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let session_id = self.inner.session_id().to_string();
        let requested_run_id = run_id.clone();
        let events =
            py.allow_threads(move || get_runtime().block_on(session.run_events(&requested_run_id)));
        let envelopes = events
            .iter()
            .map(|record| rust_run_event_envelope_v1(record, &run_id, &session_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PyRuntimeError::new_err(format!("Event protocol error: {e}")))?;
        let json = serde_json::to_string(&envelopes)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize run events: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return an asyncio Future for ``run_events()``.
    fn run_events_async<'py>(
        &self,
        py: Python<'py>,
        run_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        async_run_query_future(
            py,
            Arc::clone(&self.inner),
            AsyncRunQueryOperation::Events { run_id },
        )
    }

    /// Return a cursor-based page from the retained event window.
    #[pyo3(signature = (run_id, after_sequence=None, limit=256))]
    fn run_event_page(
        &self,
        py: Python<'_>,
        run_id: String,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let session_id = self.inner.session_id().to_string();
        let requested_run_id = run_id.clone();
        let page = py.allow_threads(move || {
            get_runtime().block_on(session.run_event_page(&requested_run_id, after_sequence, limit))
        });
        let Some(page) = page else {
            return Ok(py.None());
        };
        let events = page
            .events
            .iter()
            .map(|record| rust_run_event_envelope_v1(record, &run_id, &session_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PyRuntimeError::new_err(format!("Event protocol error: {e}")))?;
        let value = serde_json::json!({
            "events": events,
            "first_available_sequence": page.first_available_sequence,
            "latest_sequence_exclusive": page.latest_sequence_exclusive,
            "next_after_sequence": page.next_after_sequence,
            "retention_gap": page.retention_gap,
            "has_more": page.has_more,
        });
        let json = serde_json::to_string(&value)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize event page: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return an asyncio Future for ``run_event_page()``.
    #[pyo3(signature = (run_id, after_sequence=None, limit=256))]
    fn run_event_page_async<'py>(
        &self,
        py: Python<'py>,
        run_id: String,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        async_run_query_future(
            py,
            Arc::clone(&self.inner),
            AsyncRunQueryOperation::EventPage {
                run_id,
                after_sequence,
                limit,
            },
        )
    }

    /// Return the currently running operation, or None when idle.
    fn current_run(&self, py: Python<'_>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let snapshot = py.allow_threads(move || {
            get_runtime().block_on(async move {
                match session.current_run().await {
                    Some(run) => run.snapshot().await,
                    None => None,
                }
            })
        });
        let json = serde_json::to_string(&snapshot)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize run: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return active tool calls observed for the currently running operation.
    fn active_tools(&self, py: Python<'_>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let active_tools = py.allow_threads(move || get_runtime().block_on(session.active_tools()));
        let json = serde_json::to_string(&active_tools).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize active tools: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Look up a delegated subagent task by id. Returns None when no such
    /// task has been observed in this session.
    fn subagent_task(&self, py: Python<'_>, task_id: String) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let snapshot =
            py.allow_threads(move || get_runtime().block_on(session.subagent_task(&task_id)));
        let json = serde_json::to_string(&snapshot).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize subagent task: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return snapshots of every delegated subagent task observed in this
    /// session (including completed and failed ones), oldest first.
    fn subagent_tasks(&self, py: Python<'_>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let tasks = py.allow_threads(move || get_runtime().block_on(session.subagent_tasks()));
        let json = serde_json::to_string(&tasks).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize subagent tasks: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return snapshots of subagent tasks still in `running` state.
    fn pending_subagent_tasks(&self, py: Python<'_>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let tasks =
            py.allow_threads(move || get_runtime().block_on(session.pending_subagent_tasks()));
        let json = serde_json::to_string(&tasks).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize pending subagent tasks: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Cancel an in-flight subagent task by id. Returns True when a
    /// cancellation token was found and fired, False when the task id is
    /// unknown or the task already finished.
    fn cancel_subagent_task(&self, py: Python<'_>, task_id: String) -> bool {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.cancel_subagent_task(&task_id)))
    }

    /// Cancel a specific run only if it is still the active run.
    fn cancel_run(&self, py: Python<'_>, run_id: String) -> bool {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.cancel_run(&run_id)))
    }

    // ========================================================================
    // Hook API
    // ========================================================================

    /// Register a hook for lifecycle event interception.
    ///
    /// Hooks registered on a session are automatically propagated to all sub-agents
    /// spawned by the `task` tool, including grandchild agents at arbitrary depth.
    /// This ensures security hooks (e.g. a sentinel) apply across the full agent tree
    /// without requiring explicit registration on each sub-agent session.
    ///
    /// Args:
    ///     hook_id: Unique hook identifier
    ///     event_type: Event type string — one of:
    ///         "pre_tool_use", "post_tool_use", "generate_start", "generate_end",
    ///         "session_start", "session_end", "skill_load", "skill_unload",
    ///         "pre_prompt", "post_response", "on_error"
    ///     matcher: Optional dict with keys: tool, path_pattern, command_pattern, session_id, skill
    ///     config: Optional dict with keys: priority, timeout_ms, async_execution, max_retries
    ///     handler: Optional callable ``(event: dict) -> dict | None``. When provided, it is called
    ///         for every matching event and its return value controls execution:
    ///         ``{"action": "block", "reason": "…"}`` cancels the operation,
    ///         ``{"action": "skip"}`` skips remaining hooks, ``None`` or
    ///         ``{"action": "continue"}`` allows execution to proceed.
    #[pyo3(signature = (hook_id, event_type, matcher=None, config=None, handler=None))]
    fn register_hook(
        &self,
        hook_id: String,
        event_type: String,
        matcher: Option<&Bound<'_, PyDict>>,
        config: Option<&Bound<'_, PyDict>>,
        handler: Option<pyo3::Py<pyo3::PyAny>>,
    ) -> PyResult<()> {
        let rust_event_type = py_parse_hook_event_type(&event_type)?;
        let mut hook = RustHook::new(&hook_id, rust_event_type);

        if let Some(m) = matcher {
            let mut rust_matcher = RustHookMatcher::new();
            if let Some(tool) = m.get_item("tool")? {
                rust_matcher = rust_matcher.with_tool(tool.extract::<String>()?);
            }
            if let Some(path) = m.get_item("path_pattern")? {
                rust_matcher = rust_matcher.with_path(path.extract::<String>()?);
            }
            if let Some(cmd) = m.get_item("command_pattern")? {
                rust_matcher = rust_matcher.with_command(cmd.extract::<String>()?);
            }
            if let Some(sid) = m.get_item("session_id")? {
                rust_matcher = rust_matcher.with_session(sid.extract::<String>()?);
            }
            if let Some(skill) = m.get_item("skill")? {
                rust_matcher = rust_matcher.with_skill(skill.extract::<String>()?);
            }
            hook = hook.with_matcher(rust_matcher);
        }

        if let Some(c) = config {
            let priority = c
                .get_item("priority")?
                .map(|v| v.extract::<i32>())
                .transpose()?
                .unwrap_or(100);
            let timeout_ms = c
                .get_item("timeout_ms")?
                .map(|v| v.extract::<u64>())
                .transpose()?
                .unwrap_or(30000);
            let async_execution = c
                .get_item("async_execution")?
                .map(|v| v.extract::<bool>())
                .transpose()?
                .unwrap_or(false);
            let max_retries = c
                .get_item("max_retries")?
                .map(|v| v.extract::<u32>())
                .transpose()?
                .unwrap_or(0);
            hook = hook.with_config(RustHookConfig {
                priority,
                timeout_ms,
                async_execution,
                max_retries,
            });
        }

        self.inner.register_hook(hook).map_err(py_code_error)?;

        if let Some(py_fn) = handler {
            self.inner
                .register_hook_handler(
                    &hook_id,
                    Arc::new(PythonCallbackHandler { callback: py_fn }),
                )
                .map_err(py_code_error)?;
        } else {
            // Re-registering an existing hook without a handler must clear any
            // callback previously associated with the same ID.
            self.inner
                .unregister_hook_handler(&hook_id)
                .map_err(py_code_error)?;
        }

        Ok(())
    }

    /// Unregister a hook by ID.
    ///
    /// Returns True if the hook was found and removed, False otherwise.
    fn unregister_hook(&self, hook_id: String) -> PyResult<bool> {
        self.inner
            .unregister_hook_handler(&hook_id)
            .map_err(py_code_error)?;
        self.inner
            .unregister_hook(&hook_id)
            .map(|hook| hook.is_some())
            .map_err(py_code_error)
    }

    /// Get the number of registered hooks.
    fn hook_count(&self) -> usize {
        self.inner.hook_count()
    }

    // ========================================================================
    // Session Metadata API
    // ========================================================================

    /// Return the session ID.
    #[getter]
    fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Return the workspace path.
    #[getter]
    fn workspace(&self) -> String {
        self.inner.workspace().display().to_string()
    }

    /// Return any deferred init warning (e.g. memory store failed to initialize).
    #[getter]
    fn init_warning(&self) -> Option<String> {
        self.inner.init_warning().map(|s| s.to_string())
    }

    /// Host-defined tenant id attached at session creation, if any.
    #[getter]
    fn tenant_id(&self) -> Option<String> {
        self.inner.tenant_id().map(|s| s.to_string())
    }

    /// Identity of the principal that triggered the session, if any.
    #[getter]
    fn principal(&self) -> Option<String> {
        self.inner.principal().map(|s| s.to_string())
    }

    /// Logical agent template / definition id, if any.
    #[getter]
    fn agent_template_id(&self) -> Option<String> {
        self.inner.agent_template_id().map(|s| s.to_string())
    }

    /// Distributed-trace correlation id propagated through this session,
    /// if any.
    #[getter]
    fn correlation_id(&self) -> Option<String> {
        self.inner.correlation_id().map(|s| s.to_string())
    }

    // ========================================================================
    // Session Persistence API
    // ========================================================================

    /// Save the session to the configured store.
    ///
    /// Returns None if no store is configured (no-op).
    fn save(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.save()))
            .map_err(py_code_error)
    }

    /// Return an asyncio Future that saves the session to its configured store.
    fn save_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncSessionControlCall {
                session: Arc::clone(&self.inner),
                operation: Some(AsyncSessionControlOperation::Save),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    // ========================================================================
    // Slash Command & Scheduler API
    // ========================================================================

    /// List all registered slash commands.
    ///
    /// Returns a list of dicts with keys: `name`, `description`, `usage` (or `None`).
    /// Slash commands can be invoked via `session.send("/command args")`.
    fn list_commands<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let commands = self.inner.command_registry().list_full();
        let items: Vec<_> = commands
            .into_iter()
            .map(|(name, description, usage)| {
                let d = PyDict::new(py);
                let _ = d.set_item("name", &name);
                let _ = d.set_item("description", &description);
                let _ = d.set_item("usage", usage.as_deref());
                d.into_any()
            })
            .collect();
        PyList::new(py, &items)
    }

    /// Register a custom slash command with a Python callback.
    ///
    /// The `handler` receives two arguments: `args: str` (everything after the command name)
    /// and `ctx: dict` (session context with keys: `session_id`, `workspace`, `model`,
    /// `history_len`, `total_tokens`, `total_cost`, `tool_names`).
    /// It must return a `str` — the text displayed to the user.
    ///
    /// Example::
    ///
    ///     def ping_handler(args, ctx):
    ///         return f"pong! session={ctx['session_id']}"
    ///
    ///     session.register_command("ping", "Pong!", ping_handler)
    ///     result = await session.send("/ping hello")
    #[pyo3(signature = (name, description, handler))]
    fn register_command(
        &self,
        name: String,
        description: String,
        handler: pyo3::Py<pyo3::PyAny>,
    ) -> PyResult<()> {
        let cmd = Arc::new(PySlashCommand {
            name,
            description,
            handler,
        });
        self.inner
            .clone()
            .register_command(cmd)
            .map_err(py_code_error)?;
        Ok(())
    }

    /// Cancel the current ongoing operation (send/stream).
    ///
    /// If an operation is in progress, this will trigger cancellation of the LLM streaming
    /// and tool execution. The operation will terminate as soon as possible.
    ///
    /// :returns: ``True`` if an operation was cancelled, ``False`` if no operation was in progress.
    fn cancel(&self, py: Python<'_>) -> bool {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.cancel()))
    }

    /// Return an asyncio Future that cancels the active operation.
    fn cancel_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncSessionControlCall {
                session: Arc::clone(&self.inner),
                operation: Some(AsyncSessionControlOperation::Cancel),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Cancel the active operation and wait until the session is safe to reuse.
    ///
    /// A streaming worker that does not settle during ``grace_ms`` is aborted
    /// and receives ``abort_grace_ms`` for cleanup.
    #[pyo3(signature = (grace_ms=2000, abort_grace_ms=1000))]
    fn cancel_and_settle(&self, py: Python<'_>, grace_ms: u64, abort_grace_ms: u64) -> bool {
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(session.cancel_and_settle(
                std::time::Duration::from_millis(grace_ms),
                std::time::Duration::from_millis(abort_grace_ms),
            ))
        })
    }

    /// Close the session and cancel any active operation.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.close()));
        Ok(())
    }

    /// Return an asyncio Future that closes the session and its children.
    fn close_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncSessionControlCall {
                session: Arc::clone(&self.inner),
                operation: Some(AsyncSessionControlOperation::Close),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Whether ``close()`` has been called on this session.
    ///
    /// Once ``True``, calls to ``send`` / ``stream`` raise ``RuntimeError``
    /// with a "Session closed" message instead of starting a new run.
    #[getter]
    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Install or clear a host-supplied BudgetGuard on this session.
    ///
    /// The guard object may define ``check_before_llm(session_id, estimated_tokens)``,
    /// ``record_after_llm(session_id, usage_dict)``, and
    /// ``check_before_tool(session_id, tool_name)``. Missing methods behave as
    /// Allow / no-op. Pass ``None`` to clear the runtime override.
    #[pyo3(signature = (guard=None))]
    fn set_budget_guard(&self, guard: Option<pyo3::PyObject>) -> PyResult<()> {
        let Some(guard) = guard else {
            return self.inner.set_budget_guard(None).map_err(py_code_error);
        };
        self.inner
            .set_budget_guard(Some(Arc::new(PyBudgetGuard::new(guard))))
            .map_err(py_code_error)
    }

    fn __repr__(&self) -> String {
        format!(
            "Session(id='{}', workspace='{}')",
            self.inner.session_id(),
            self.inner.workspace().display()
        )
    }
}
