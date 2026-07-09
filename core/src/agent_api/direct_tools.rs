//! Direct host tool access.
//!
//! These helpers expose the same tool executor used by the agent loop for host
//! control-plane calls. Keeping argument shaping and result projection here
//! prevents the public session facade from duplicating tool-specific knowledge.

use super::{AgentSession, ReadFileOptions, ToolCallResult};
use crate::agent::AgentEvent;
use crate::error::Result;
use crate::llm::ToolDefinition;
use crate::tools::{ToolArtifact, ToolContext, ToolExecutor, ToolStreamEvent};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

pub(super) struct DirectToolRuntime {
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
}

impl DirectToolRuntime {
    pub(super) fn from_session(session: &AgentSession) -> Self {
        Self {
            tool_executor: Arc::clone(&session.tool_executor),
            tool_context: session.tool_context.clone(),
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
        let result = self
            .tool_executor
            .execute_with_context("read", &args, &self.tool_context)
            .await?;
        Ok(result.output)
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
        let result = self
            .tool_executor
            .execute_with_context("bash", &args, &self.tool_context)
            .await?;
        Ok(result.output)
    }

    pub(super) async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool_executor.execute("glob", &args).await?;
        Ok(parse_glob_output(&result.output))
    }

    pub(super) async fn grep(&self, pattern: &str) -> Result<String> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool_executor.execute("grep", &args).await?;
        Ok(result.output)
    }

    pub(super) async fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolCallResult> {
        let result = self
            .tool_executor
            .execute_with_context(name, &args, &self.tool_context)
            .await?;
        Ok(ToolCallResult {
            name: name.to_string(),
            output: result.output,
            exit_code: result.exit_code,
            metadata: result.metadata,
            error_kind: result.error_kind,
        })
    }

    pub(super) fn spawn_call_with_agent_events(
        self,
        name: String,
        args: serde_json::Value,
    ) -> (
        mpsc::Receiver<AgentEvent>,
        JoinHandle<Result<ToolCallResult>>,
    ) {
        let (agent_tx, agent_rx) = mpsc::channel::<AgentEvent>(256);
        let (broadcast_tx, mut broadcast_rx) = broadcast::channel::<AgentEvent>(256);
        let forward_tx = agent_tx.clone();
        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(event) => {
                        if forward_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let executor = Arc::clone(&self.tool_executor);
        let mut ctx = self.tool_context;
        ctx.agent_event_tx = Some(broadcast_tx);
        let tool_name = name.clone();
        let (tool_tx, mut tool_rx) = mpsc::channel::<ToolStreamEvent>(64);
        ctx.event_tx = Some(tool_tx);
        let stream_agent_tx = agent_tx.clone();
        let stream_tool_id = tool_name.clone();
        let stream_tool_name = tool_name.clone();
        tokio::spawn(async move {
            while let Some(event) = tool_rx.recv().await {
                match event {
                    ToolStreamEvent::OutputDelta(delta) => {
                        if stream_agent_tx
                            .send(AgentEvent::ToolOutputDelta {
                                id: stream_tool_id.clone(),
                                name: stream_tool_name.clone(),
                                delta,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        let handle = tokio::spawn(async move {
            let result = executor
                .execute_with_context(&tool_name, &args, &ctx)
                .await?;
            Ok(ToolCallResult {
                name,
                output: result.output,
                exit_code: result.exit_code,
                metadata: result.metadata,
                error_kind: result.error_kind,
            })
        });

        drop(agent_tx);
        (agent_rx, handle)
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
mod tests {
    use super::*;
    use crate::tools::{Tool, ToolOutput};
    use anyhow::Result;
    use async_trait::async_trait;

    struct ContextProbeTool;
    struct StreamingProbeTool;

    #[async_trait]
    impl Tool for ContextProbeTool {
        fn name(&self) -> &str {
            "context_probe"
        }

        fn description(&self) -> &str {
            "Reports direct tool context for tests."
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success(
                ctx.session_id.as_deref().unwrap_or("missing-session"),
            ))
        }
    }

    #[async_trait]
    impl Tool for StreamingProbeTool {
        fn name(&self) -> &str {
            "streaming_probe"
        }

        fn description(&self) -> &str {
            "Streams direct tool progress for tests."
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            if let Some(tx) = &ctx.event_tx {
                let _ = tx
                    .send(ToolStreamEvent::OutputDelta(
                        "submitted 2 tasks\n".to_string(),
                    ))
                    .await;
                let _ = tx
                    .send(ToolStreamEvent::OutputDelta("2/2 done\n".to_string()))
                    .await;
            }
            Ok(ToolOutput::success("complete"))
        }
    }

    #[test]
    fn parse_glob_output_ignores_empty_lines() {
        assert_eq!(
            parse_glob_output("src/lib.rs\n\nsrc/main.rs\n"),
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn direct_tool_call_uses_session_tool_context() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(ContextProbeTool));
        let runtime = DirectToolRuntime {
            tool_executor,
            tool_context: ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
        };

        let output = runtime
            .call("context_probe", serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(output.output, "session-123");
    }

    #[tokio::test]
    async fn direct_read_file_passes_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        let runtime = DirectToolRuntime {
            tool_executor,
            tool_context: ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
        };

        let output = runtime
            .read_file(
                "notes.txt",
                ReadFileOptions {
                    offset: Some(1),
                    limit: Some(2),
                },
            )
            .await
            .unwrap();

        assert!(output.contains("two"));
        assert!(output.contains("three"));
        assert!(!output.contains("one"));
        assert!(!output.contains("four"));
    }

    #[tokio::test]
    async fn direct_tool_call_with_events_forwards_tool_stream_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(StreamingProbeTool));
        let runtime = DirectToolRuntime {
            tool_executor,
            tool_context: ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
        };

        let (mut rx, join) = runtime
            .spawn_call_with_agent_events("streaming_probe".to_string(), serde_json::json!({}));
        let output = join.await.unwrap().unwrap();
        assert_eq!(output.output, "complete");

        let mut deltas = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
        {
            if let AgentEvent::ToolOutputDelta { id, name, delta } = event {
                assert_eq!(id, "streaming_probe");
                assert_eq!(name, "streaming_probe");
                deltas.push(delta);
            }
        }
        assert_eq!(deltas.join(""), "submitted 2 tasks\n2/2 done\n");
    }
}
