use super::*;

impl AgentSession {
    /// Read a file from the workspace.
    pub async fn read_file(&self, path: &str) -> Result<String> {
        self.read_file_with_options(path, ReadFileOptions::default())
            .await
    }

    /// Read a file from the workspace with optional line-range controls.
    pub async fn read_file_with_options(
        &self,
        path: &str,
        options: ReadFileOptions,
    ) -> Result<String> {
        DirectToolRuntime::from_session(self)
            .read_file(path, options)
            .await
    }

    /// Write a file in the workspace.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<ToolCallResult> {
        DirectToolRuntime::from_session(self)
            .write_file(path, content)
            .await
    }

    /// List a directory in the workspace.
    pub async fn ls(&self, path: Option<&str>) -> Result<ToolCallResult> {
        DirectToolRuntime::from_session(self).ls(path).await
    }

    /// Edit a file by replacing text in the workspace.
    pub async fn edit_file(
        &self,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<ToolCallResult> {
        DirectToolRuntime::from_session(self)
            .edit_file(path, old_string, new_string, replace_all)
            .await
    }

    /// Apply a unified diff patch to a workspace file.
    pub async fn patch_file(&self, path: &str, diff: &str) -> Result<ToolCallResult> {
        DirectToolRuntime::from_session(self)
            .patch_file(path, diff)
            .await
    }

    /// Execute a bash command in the workspace.
    ///
    /// When a sandbox handle is configured via
    /// [`SessionOptions::with_sandbox_handle()`], the command is routed through
    /// that sandbox.
    pub async fn bash(&self, command: &str) -> Result<String> {
        DirectToolRuntime::from_session(self).bash(command).await
    }

    /// Search for files matching a glob pattern.
    pub async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        DirectToolRuntime::from_session(self).glob(pattern).await
    }

    /// Search file contents with a regex pattern.
    pub async fn grep(&self, pattern: &str) -> Result<String> {
        DirectToolRuntime::from_session(self).grep(pattern).await
    }

    /// Execute a tool by name, bypassing the LLM and treating the host as the
    /// authority that already approved permission and HITL requirements.
    ///
    /// Use [`Self::governed_tool`] when the host is coordinating an invocation
    /// that must still pass the session's permission and confirmation gates.
    pub async fn tool(&self, name: &str, args: serde_json::Value) -> Result<ToolCallResult> {
        DirectToolRuntime::from_session(self).call(name, args).await
    }

    /// Execute a tool by name without an LLM while retaining the session's
    /// permission and HITL gates.
    ///
    /// Use this entry point when the host coordinates work but has not already
    /// authorized the tool invocation. Unlike [`Self::tool`], a permission
    /// `Ask` decision or a registered tool's `requires_confirmation` hook must
    /// resolve through the session confirmation provider before execution.
    pub async fn governed_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<ToolCallResult> {
        DirectToolRuntime::from_session(self)
            .call_governed(name, args)
            .await
    }

    /// Execute a tool by name and expose high-level agent events emitted by that
    /// tool while it runs.
    ///
    /// This keeps the normal direct `tool()` API unchanged while giving embedded
    /// hosts a live progress stream for tools such as `dynamic_workflow` and
    /// `parallel_task`.
    pub fn tool_with_events(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> (
        mpsc::Receiver<AgentEvent>,
        JoinHandle<Result<ToolCallResult>>,
    ) {
        DirectToolRuntime::from_session(self).spawn_call_with_agent_events(name.to_string(), args)
    }
}
