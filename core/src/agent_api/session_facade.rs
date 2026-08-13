use super::*;

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("session_id", &self.session_id)
            .field("workspace", &self.workspace.display().to_string())
            .field("task_priority", &self.task_priority)
            .field(
                "cognitive_package_binding",
                &self
                    .cognitive_context
                    .as_ref()
                    .map(crate::cognitive_context::CognitiveContextSession::binding),
            )
            .field("auto_save", &self.auto_save)
            .finish()
    }
}

impl AgentSession {
    /// Return current occupancy of the scheduler shared with sibling sessions.
    pub async fn task_scheduler_stats(
        &self,
    ) -> std::result::Result<
        crate::task_scheduler::TaskSchedulerStats,
        crate::task_scheduler::TaskSchedulerError,
    > {
        self.task_scheduler.stats().await
    }

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

    /// Observe asynchronous semantic workspace indexing for this session.
    ///
    /// Disabled sessions return a stable `disabled` status without starting a
    /// background task. Closing a configured session transitions the same
    /// status to `closed` and releases its in-memory vector index.
    pub fn workspace_retrieval_status(&self) -> crate::workspace::WorkspaceRetrievalStatus {
        self.workspace_retrieval
            .as_ref()
            .map(|runtime| runtime.status())
            .unwrap_or_else(crate::workspace::WorkspaceRetrievalStatus::disabled)
    }

    /// Run a structured semantic workspace query against this session.
    ///
    /// Returned source chunks are reread and digest-verified through the
    /// session's workspace backend. Partial indexing, degradation, and
    /// revision races are represented explicitly in the result's status and
    /// fallback fields.
    pub async fn semantic_search(
        &self,
        request: crate::workspace::WorkspaceSemanticSearchRequest,
    ) -> crate::workspace::WorkspaceRetrievalResult<crate::workspace::WorkspaceSemanticSearchResult>
    {
        if self.is_closed() {
            return Err(crate::workspace::WorkspaceRetrievalError::Unavailable);
        }
        self.tool_context
            .workspace_services
            .semantic_search(request, self.session_cancel.child_token())
            .await
    }

    /// Run structured hybrid workspace retrieval for this session.
    ///
    /// Exact literal, BM25, optional Code Intelligence symbols, and semantic
    /// candidates are fused by rank. Returned chunks are current-source
    /// verified and include per-channel status and fallback metadata.
    pub async fn hybrid_search(
        &self,
        request: crate::workspace::WorkspaceHybridSearchRequest,
    ) -> crate::workspace::WorkspaceRetrievalResult<crate::workspace::WorkspaceHybridSearchResult>
    {
        if self.is_closed() {
            return Err(crate::workspace::WorkspaceRetrievalError::Unavailable);
        }
        self.tool_context
            .workspace_services
            .hybrid_search(request, self.session_cancel.child_token())
            .await
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

    /// Return the exact cognitive-package generation bound to this session.
    pub fn cognitive_package_binding(
        &self,
    ) -> Option<&crate::cognitive_context::CognitivePackageBindingV1> {
        self.cognitive_context
            .as_ref()
            .map(crate::cognitive_context::CognitiveContextSession::binding)
    }

    /// Proactively close the session and release its in-flight work.
    ///
    /// On the first call this:
    /// 1. flips the session into the **closed** state so further `send`/`stream`
    ///    calls fast-fail with [`crate::error::CodeError::SessionClosed`];
    /// 2. stops the optional lane queue from accepting new commands;
    /// 3. waits for completed-turn memory extractions accepted before close,
    ///    up to the bounded shutdown deadline;
    /// 4. fires the session-level cancellation token so every derived
    ///    run/subagent token cascades to cancelled;
    /// 5. marks the active run `Cancelled` in the run store and notifies the
    ///    configured hook executor;
    /// 6. cancels every still-running delegated subagent task spawned from
    ///    this session;
    /// 7. cancels all pending human-in-the-loop tool confirmations and external
    ///    queue tasks, then drains commands admitted before close;
    /// 8. disconnects MCP servers owned by this session, without touching
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

    /// Start one detached run with an exact host-selected run id.
    ///
    /// Replaying the same id, session, and prompt returns the existing run
    /// without executing it again. Reusing the id for different immutable
    /// input fails with [`crate::error::CodeError::RunIdentityConflict`].
    /// This is the canonical entry point for headless `a3s code` hosts.
    pub async fn spawn_run_with_id(&self, run_id: &str, prompt: &str) -> Result<AgentRunSpawn> {
        conversation_runtime::spawn_run_with_id(self, run_id, prompt).await
    }

    /// Resume a durable checkpoint into one exact, fresh host-selected run id.
    ///
    /// The checkpoint run remains immutable. Replaying the same recovery run
    /// id returns its existing snapshot and never executes the checkpoint
    /// twice.
    pub async fn spawn_recovery_with_run_id(
        &self,
        checkpoint_run_id: &str,
        run_id: &str,
    ) -> Result<AgentRunSpawn> {
        conversation_runtime::spawn_recovery_with_run_id(self, checkpoint_run_id, run_id).await
    }

    pub(crate) async fn record_workspace_change_set(
        &self,
        run_id: &str,
        change_set: crate::run::RunWorkspaceChangeSet,
    ) -> Result<crate::run::RunSnapshot> {
        let snapshot = self
            .run_store
            .record_workspace_change_set(run_id, change_set)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        self.save().await?;
        Ok(snapshot)
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
        if !self.run_admission.abort_stream_workers() {
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

    /// Return the model-visible definitions for this session.
    ///
    /// The list reflects the live state of the tool executor — tools added via
    /// `add_mcp_server()` appear immediately; tools removed via
    /// `remove_mcp_server()` disappear immediately. Hidden host compatibility
    /// aliases remain executable by name but are omitted to avoid schema cost.
    pub fn tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        DirectToolRuntime::from_session(self).definitions()
    }

    /// Return the names of all model-visible tools on this session.
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
