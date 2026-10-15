//! Direct host tool access.
//!
//! These helpers expose the same tool executor used by the agent loop for host
//! control-plane calls. Keeping argument shaping and result projection here
//! prevents the public session facade from duplicating tool-specific knowledge.

use super::{
    agent_loop_runtime::build_agent_loop, execution_coordinator::ExecutionCoordinator,
    AgentSession, ReadFileOptions, ToolCallResult,
};
use crate::agent::{AgentEvent, AgentLoop};
use crate::error::Result;
use crate::llm::ToolDefinition;
use crate::tools::{ToolArtifact, ToolContext, ToolExecutor, ToolInvocation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

mod event_stream;
#[cfg(test)]
mod governed_tests;

pub(crate) struct DirectToolRuntime {
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    agent_loop: AgentLoop,
    session_id: String,
    session_cancel: tokio_util::sync::CancellationToken,
    closed: Arc<AtomicBool>,
    task_scheduler: Option<Arc<crate::task_scheduler::TaskScheduler>>,
    task_priority: crate::task_scheduler::TaskPriority,
    provider_quota: Option<crate::task_scheduler::TaskSchedulerQuota>,
    security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
}

impl DirectToolRuntime {
    pub(crate) fn from_session(session: &AgentSession) -> Self {
        Self {
            tool_executor: Arc::clone(&session.tool_executor),
            tool_context: session.tool_context.clone(),
            agent_loop: build_agent_loop(session),
            session_id: session.session_id.clone(),
            session_cancel: session.session_cancel.clone(),
            closed: Arc::clone(&session.closed),
            task_scheduler: Some(Arc::clone(&session.task_scheduler)),
            task_priority: session.task_priority,
            // Session-owned model-generation permits already reserve this
            // provider quota through the shared scheduler. Avoid holding the
            // same dimension across the whole host-tool call, which would
            // recursively block a nested `generate_object` invocation.
            provider_quota: (!session.model_generation_admission.has_scheduler_quota())
                .then(|| session.llm_client.model_generation_pool())
                .flatten()
                .and_then(|pool| {
                    crate::task_scheduler::TaskSchedulerQuota::new(
                        pool.identity.clone(),
                        pool.max_concurrency().get(),
                    )
                    .ok()
                }),
            security_provider: session.config.security_provider.clone(),
        }
    }

    pub(super) fn definitions(&self) -> Vec<ToolDefinition> {
        self.tool_executor.definitions()
    }

    pub(super) fn names(&self) -> Vec<String> {
        self.tool_executor
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    pub(super) fn artifact(&self, artifact_uri: &str) -> Option<ToolArtifact> {
        self.tool_executor.get_artifact(artifact_uri)
    }

    pub(super) async fn read_file(&self, path: &str, options: ReadFileOptions) -> Result<String> {
        let mut args = serde_json::json!({ "file_path": path });
        if let serde_json::Value::Object(ref mut map) = args {
            if let Some(offset) = options.offset {
                map.insert("offset".to_string(), serde_json::json!(offset));
            }
            if let Some(limit) = options.limit {
                map.insert("limit".to_string(), serde_json::json!(limit));
            }
        }
        Ok(self.call("read", args).await?.output)
    }

    pub(super) async fn write_file(&self, path: &str, content: &str) -> Result<ToolCallResult> {
        let args = serde_json::json!({ "file_path": path, "content": content });
        self.call("write", args).await
    }

    pub(super) async fn ls(&self, path: Option<&str>) -> Result<ToolCallResult> {
        let args = match path {
            Some(path) => serde_json::json!({ "path": path }),
            None => serde_json::json!({}),
        };
        self.call("ls", args).await
    }

    pub(super) async fn edit_file(
        &self,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<ToolCallResult> {
        let args = serde_json::json!({
            "file_path": path,
            "old_string": old_string,
            "new_string": new_string,
            "replace_all": replace_all,
        });
        self.call("edit", args).await
    }

    pub(super) async fn patch_file(&self, path: &str, diff: &str) -> Result<ToolCallResult> {
        let args = serde_json::json!({ "file_path": path, "diff": diff });
        self.call("patch", args).await
    }

    pub(super) async fn bash(&self, command: &str) -> Result<String> {
        let args = serde_json::json!({ "command": command });
        Ok(self.call("bash", args).await?.output)
    }

    pub(super) async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let args = serde_json::json!({ "mode": "glob", "query": pattern });
        let result = self.call("search", args).await?;
        Ok(parse_glob_output(&result.output))
    }

    pub(super) async fn grep(&self, pattern: &str) -> Result<String> {
        let args = serde_json::json!({ "mode": "grep", "query": pattern });
        let result = self.call("search", args).await?;
        Ok(result.output)
    }

    pub(super) async fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolCallResult> {
        self.call_invocation(ToolInvocation::host_direct(
            format!("host-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
        ))
        .await
    }

    pub(crate) async fn call_governed(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<ToolCallResult> {
        self.call_invocation(ToolInvocation::host_governed(
            format!("host-governed-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
        ))
        .await
    }

    async fn call_invocation(&self, invocation: ToolInvocation) -> Result<ToolCallResult> {
        self.ensure_open()?;
        let cancel = self.session_cancel.child_token();
        let quotas = self.provider_quota.as_slice();
        let _task_lease = ExecutionCoordinator::acquire_optional_task_with_quotas(
            self.task_scheduler.as_deref(),
            self.task_priority,
            format!("{}:tool:{}", self.session_id, invocation.name),
            quotas,
            &self.session_id,
            &cancel,
            &self.closed,
        )
        .await?;
        self.ensure_open()?;
        let result = self
            .agent_loop
            .invoke_host_tool(
                invocation,
                &self.session_id,
                &None,
                &cancel,
                &self.tool_context,
            )
            .await;
        Ok(project_tool_result(result))
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        Ok(())
    }
}

fn project_tool_result(result: crate::tools::ToolResult) -> ToolCallResult {
    ToolCallResult {
        name: result.name,
        output: result.output,
        exit_code: result.exit_code,
        metadata: result.metadata,
        error_kind: result.error_kind,
    }
}

fn parse_glob_output(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests;
