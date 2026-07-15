use super::*;

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("session_id", &self.session_id)
            .field("workspace", &self.workspace.display().to_string())
            .field("auto_save", &self.auto_save)
            .finish()
    }
}

impl AgentSession {
    /// Get a snapshot of command entries (name, description, optional usage).
    ///
    /// Acquires the command registry lock briefly and returns owned data.
    pub fn command_registry(&self) -> std::sync::MutexGuard<'_, CommandRegistry> {
        session_commands::registry(self)
    }

    /// Register a custom slash command.
    ///
    /// Takes `&self` so it can be called on a shared `Arc<AgentSession>`.
    pub fn register_command(
        &self,
        cmd: Arc<dyn crate::commands::SlashCommand>,
    ) -> crate::error::Result<()> {
        self.close_handle
            .mutate_immediate(|| session_commands::register(self, cmd))
    }

    /// Return whether [`close`](Self::close) has been called on this session.
    ///
    /// Once closed, `send`/`stream` and their attachment variants fast-fail
    /// with [`crate::error::CodeError::SessionClosed`] instead of starting a
    /// new run.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Clone the session-level [`CancellationToken`](tokio_util::sync::CancellationToken).
    ///
    /// All in-flight runs derive their per-operation token from this one via
    /// `child_token()`, so embedders can:
    ///
    /// - Observe the token (e.g. wire it into a host-side `select!`) to
    ///   react to session shutdown without polling [`is_closed`](Self::is_closed);
    /// - Call `.cancel()` on it to abort every operation in the session
    ///   without going through `close()` (no run-store / hook side effects).
    ///
    /// For graceful shutdown prefer [`close`](Self::close), which also marks
    /// runs as cancelled in the store and notifies the configured hook executor.
    pub fn session_cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.session_cancel.clone()
    }

    /// Return the host-defined tenant id, if any.
    ///
    /// The framework only transports this string — it never interprets
    /// or enforces tenant boundaries itself. Use this from custom
    /// `HookExecutor` / `PermissionChecker` / `BudgetGuard` impls to route logic
    /// by tenant.
    pub fn tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref()
    }

    /// Return the principal that triggered the session, if any.
    pub fn principal(&self) -> Option<&str> {
        self.principal.as_deref()
    }

    /// Return the id of the agent template/definition the session was
    /// instantiated from, if any.
    pub fn agent_template_id(&self) -> Option<&str> {
        self.agent_template_id.as_deref()
    }

    /// Return the distributed-trace correlation id propagated through
    /// this session's events, if any.
    pub fn correlation_id(&self) -> Option<&str> {
        self.correlation_id.as_deref()
    }

    /// Proactively close the session and release its in-flight work.
    ///
    /// On the first call this:
    /// 1. flips the session into the **closed** state so further `send`/`stream`
    ///    calls fast-fail with [`crate::error::CodeError::SessionClosed`];
    /// 2. stops the optional lane queue from accepting new commands;
    /// 3. fires the session-level cancellation token so every derived
    ///    run/subagent token cascades to cancelled;
    /// 4. marks the active run `Cancelled` in the run store and notifies the
    ///    configured hook executor;
    /// 5. cancels every still-running delegated subagent task spawned from
    ///    this session;
    /// 6. cancels all pending human-in-the-loop tool confirmations and external
    ///    queue tasks, then drains commands admitted before close;
    /// 7. disconnects MCP servers owned by this session, without touching
    ///    inherited agent- or host-owned managers.
    ///
    /// Subsequent calls are no-ops and are guaranteed not to panic.
    pub async fn close(&self) {
        // Delegate to the shared handle so this entry point and
        // `Agent::close_session(id)` cannot drift in behaviour.
        self.close_handle.close().await;
    }

    /// Send a prompt and wait for the complete response.
    ///
    /// When `history` is `None`, uses (and auto-updates) the session's
    /// internal conversation history. When `Some`, uses the provided
    /// history instead (the internal history is **not** modified).
    ///
    /// If the prompt starts with `/`, it is dispatched as a slash command
    /// and the result is returned without calling the LLM.
    pub async fn send(&self, prompt: &str, history: Option<&[Message]>) -> Result<AgentResult> {
        conversation_runtime::send(self, prompt, history).await
    }

    /// Resume a previously-checkpointed run on this session.
    ///
    /// Loads the latest [`LoopCheckpoint`](crate::loop_checkpoint::LoopCheckpoint)
    /// stored under `checkpoint_run_id` and replays the agent loop from
    /// that boundary state. A **new** run id is allocated for the
    /// resumed work; the relationship between the old and new run is
    /// host-tracked — the framework does not interpret
    /// it.
    ///
    /// Returns an error when no `SessionStore` is configured on this
    /// session, or when no checkpoint exists for `checkpoint_run_id`.
    pub async fn resume_run(&self, checkpoint_run_id: &str) -> Result<AgentResult> {
        conversation_runtime::resume_run(self, checkpoint_run_id).await
    }

    /// Send a prompt with image attachments and wait for the complete response.
    ///
    /// Images are included as multi-modal content blocks in the user message.
    /// Requires a vision-capable model (e.g., Claude Sonnet, GPT-4o).
    pub async fn send_with_attachments(
        &self,
        prompt: &str,
        attachments: &[crate::llm::Attachment],
        history: Option<&[Message]>,
    ) -> Result<AgentResult> {
        conversation_runtime::send_with_attachments(self, prompt, attachments, history).await
    }

    /// Stream a prompt with image attachments.
    ///
    /// Images are included as multi-modal content blocks in the user message.
    /// Requires a vision-capable model (e.g., Claude Sonnet, GPT-4o).
    pub async fn stream_with_attachments(
        &self,
        prompt: &str,
        attachments: &[crate::llm::Attachment],
        history: Option<&[Message]>,
    ) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
        conversation_runtime::stream_with_attachments(self, prompt, attachments, history).await
    }

    /// Send a prompt and stream events back.
    ///
    /// When `history` is `None`, uses the session's internal history
    /// and updates it when the stream completes.
    /// When `Some`, uses the provided history instead.
    ///
    /// If the prompt starts with `/`, it is dispatched as a slash command
    /// and the result is emitted as a single `TextDelta` + `End` event.
    pub async fn stream(
        &self,
        prompt: &str,
        history: Option<&[Message]>,
    ) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
        conversation_runtime::stream(self, prompt, history).await
    }

    /// Cancel the current ongoing operation (send/stream).
    ///
    /// If an operation is in progress, this will trigger cancellation of the LLM streaming
    /// and tool execution. The operation will terminate as soon as possible.
    ///
    /// Returns `true` if an operation was cancelled, `false` if no operation was in progress.
    pub async fn cancel(&self) -> bool {
        RunControl::from_session(self).cancel_current().await
    }

    /// Cancel the current operation and wait for its single-flight lease to be
    /// released. Streaming workers first receive cooperative cancellation; a
    /// worker that exceeds `grace` is aborted, then given `abort_grace` to run
    /// destructors and release admission ownership.
    ///
    /// Returns `true` once the session is safe to reuse. A blocking `send`
    /// future cannot be force-aborted by the session and may return `false` if
    /// its caller does not poll it to completion.
    pub async fn cancel_and_settle(
        &self,
        grace: std::time::Duration,
        abort_grace: std::time::Duration,
    ) -> bool {
        let _ = self.cancel().await;
        if self.run_admission.wait_until_idle(grace).await {
            return true;
        }
        if !self.run_admission.abort_stream_worker() {
            return false;
        }
        self.run_admission.wait_until_idle(abort_grace).await
    }

    /// Return a snapshot of the session's conversation history.
    pub fn history(&self) -> Vec<Message> {
        SessionView::from_session(self).history()
    }

    /// Return a reference to the session's memory.
    ///
    /// Normal sessions always have memory; `None` is reserved for
    /// lower-level/manual construction compatibility.
    pub fn memory(&self) -> Option<&Arc<crate::memory::AgentMemory>> {
        SessionView::from_session(self).memory()
    }

    /// Return the session ID.
    pub fn id(&self) -> &str {
        SessionView::from_session(self).id()
    }

    /// Return the session workspace path.
    pub fn workspace(&self) -> &std::path::Path {
        SessionView::from_session(self).workspace()
    }

    /// Return any deferred init warning (e.g. memory store failed to initialize).
    pub fn init_warning(&self) -> Option<&str> {
        SessionView::from_session(self).init_warning()
    }

    /// Return the session ID.
    pub fn session_id(&self) -> &str {
        SessionView::from_session(self).id()
    }

    /// The session's persistence store, if one is configured — needed by the
    /// resumable orchestration combinator to journal workflow progress.
    pub fn session_store(&self) -> Option<Arc<dyn crate::store::SessionStore>> {
        self.session_store.clone()
    }

    /// Return the definitions of all tools currently registered in this session.
    ///
    /// The list reflects the live state of the tool executor — tools added via
    /// `add_mcp_server()` appear immediately; tools removed via
    /// `remove_mcp_server()` disappear immediately.
    pub fn tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        DirectToolRuntime::from_session(self).definitions()
    }

    /// Return the names of all tools currently registered on this session.
    ///
    /// Equivalent to `tool_definitions().into_iter().map(|t| t.name).collect()`.
    /// Tools added via [`Self::add_mcp_server`] appear immediately; tools
    /// removed via [`Self::remove_mcp_server`] disappear immediately.
    pub fn tool_names(&self) -> Vec<String> {
        DirectToolRuntime::from_session(self).names()
    }

    /// Return a stored tool artifact by URI, if it exists in this session.
    pub fn get_artifact(&self, artifact_uri: &str) -> Option<crate::tools::ToolArtifact> {
        DirectToolRuntime::from_session(self).artifact(artifact_uri)
    }

    /// Return compact execution trace events recorded for this session.
    pub fn trace_events(&self) -> Vec<crate::trace::TraceEvent> {
        SessionView::from_session(self).trace_events()
    }

    /// Save the session to the configured store.
    ///
    /// Returns `Ok(())` if saved successfully, or if no store is configured (no-op).
    pub async fn save(&self) -> Result<()> {
        session_save::save(self).await
    }
}
