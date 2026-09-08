//! Filesystem-first serve daemon binding (requires the `serve` Cargo feature).

use super::*;

#[napi]
impl Agent {
    /// Serve a filesystem-first agent directory's cron schedules until stopped.
    ///
    /// Loads the directory by convention: `instructions.md` (required), optional
    /// `agent.acl`, `skills/`, `schedules/*.md` (cron jobs), and `tools/*.md`
    /// (`kind: mcp` servers or `kind: script` sandboxed QuickJS tools). It starts
    /// one durable session per enabled schedule (stable id `schedule:<name>`) with
    /// the agent dir's tools installed; each schedule fires as a FULL harness turn
    /// (context, tool visibility, safety gate, verification), never a raw model call.
    ///
    /// Resolves with a {@link ServeHandle} only after all enabled schedule
    /// sessions and tools have been prepared. Startup failures reject this call,
    /// so the returned handle is ready to accept scheduled work. The daemon then
    /// runs in the background until `handle.stop()` is called. The handle MUST be
    /// kept and stopped explicitly — dropping it does NOT cancel the daemon.
    ///
    /// ```js
    /// const handle = await agent.serveAgentDir('./my-agent', '/my-project');
    /// // ... later ...
    /// await handle.stop();
    /// ```
    ///
    /// @param dir - Path to the agent directory (prompt/skills/schedules/tools)
    /// @param workspace - Workspace directory each scheduled turn operates in
    /// @param options - Optional session overrides merged into every schedule session
    ///   (model, llmClient, sessionStore, …). `promptSlots` is honored when
    ///   provided; otherwise the AgentDir `instructions.md` slot is used.
    ///   `sessionId` is always owned by the daemon and set to `schedule:<name>`.
    #[napi]
    pub async fn serve_agent_dir(
        &self,
        dir: String,
        workspace: String,
        options: Option<SessionOptions>,
    ) -> napi::Result<ServeHandle> {
        let agent_dir = RustAgentDir::load(&dir)
            .map_err(|e| napi::Error::from_reason(format!("Failed to load agent dir: {e}")))?;
        let extra = js_session_options_to_rust(options)?;

        let agent = self.inner.clone();
        let handle = get_runtime()
            .spawn(async move {
                let handle =
                    match rust_spawn_agent_dir_daemon(agent, agent_dir, workspace, Some(extra)) {
                        Ok(handle) => handle,
                        Err(error) => return Err((None, error)),
                    };
                if let Err(error) = handle.wait_ready().await {
                    return Err((handle.failure_code(), error));
                }
                Ok(handle)
            })?
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|(failure_code, error)| node_serve_error_code(failure_code, error))?;

        Ok(ServeHandle {
            inner: Arc::new(handle),
        })
    }
}
