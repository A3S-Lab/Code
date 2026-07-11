use super::{AgentEvent, AgentLoop, ToolCommand};
use crate::llm::ToolCall;
use crate::tools::{ToolContext, ToolInvocation, ToolStreamEvent};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

impl AgentLoop {
    pub(super) async fn execute_delegated_plan_tool(
        &self,
        tool_name: &str,
        args: &Value,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> (String, i32, bool, Option<Value>) {
        let call_id = format!("plan-{}-{}", tool_name, uuid::Uuid::new_v4());
        let synthetic_call = ToolCall {
            id: call_id.clone(),
            name: tool_name.to_string(),
            args: args.clone(),
        };
        self.config.rl_trajectory_recorder.record_tool_call(
            session_id.unwrap_or(""),
            0,
            &synthetic_call,
        );
        let started = std::time::Instant::now();
        let normalized = self
            .invoke_model_tool(
                ToolInvocation::agent(
                    call_id.clone(),
                    tool_name.to_string(),
                    args.clone(),
                    Vec::new(),
                ),
                session_id,
                event_tx,
                cancel_token,
            )
            .await;
        self.config.rl_trajectory_recorder.record_tool_result(
            session_id.unwrap_or(""),
            0,
            &call_id,
            tool_name,
            &normalized.output,
            normalized.exit_code,
            started.elapsed().as_millis() as u64,
            &normalized.metadata,
            normalized
                .error_kind
                .as_ref()
                .map(|kind| format!("{kind:?}")),
        );

        if let Some(tx) = event_tx {
            tx.send(AgentEvent::ToolEnd {
                id: call_id,
                name: tool_name.to_string(),
                args: Some(args.clone()),
                output: normalized.output.clone(),
                exit_code: normalized.exit_code,
                metadata: normalized.metadata.clone(),
                error_kind: normalized.error_kind.clone(),
            })
            .await
            .ok();
        }

        (
            normalized.output,
            normalized.exit_code,
            normalized.is_error,
            normalized.metadata,
        )
    }

    /// Execute a tool, applying the configured timeout if set.
    ///
    /// On timeout, returns an error describing which tool timed out and after
    /// how many milliseconds. The caller converts this to a tool-result error
    /// message that is fed back to the LLM.
    async fn execute_tool_timed(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<crate::tools::ToolResult> {
        let fut = self.tool_executor.execute_with_context(name, args, ctx);
        let execute = async {
            if let Some(timeout_ms) = self.config.tool_timeout_ms {
                match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
                    Ok(result) => result,
                    Err(_) => Err(anyhow::anyhow!(
                        "Tool '{}' timed out after {}ms",
                        name,
                        timeout_ms
                    )),
                }
            } else {
                fut.await
            }
        };

        let cancellation = ctx.cancellation_token();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                Err(anyhow::anyhow!("Tool '{}' cancelled by caller", name))
            }
            result = execute => result,
        }
    }

    /// Execute a tool through the lane queue (if configured) or directly.
    pub(super) async fn execute_tool_queued_or_direct(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<crate::tools::ToolResult> {
        self.execute_tool_queued_or_direct_inner(name, args, ctx)
            .await
    }

    /// Inner execution without task lifecycle wrapping.
    async fn execute_tool_queued_or_direct_inner(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<crate::tools::ToolResult> {
        if ctx.is_cancelled() {
            anyhow::bail!("Tool '{}' cancelled by caller", name);
        }

        // A queue worker already owns the scheduling slot for this invocation
        // scope. Re-submitting an orchestrator's nested call to the same lane
        // can deadlock when that lane has a single worker. Nested calls still
        // pass through ToolInvoker before reaching this backend, so hooks,
        // budget, timeout, cancellation, and sanitization remain in force.
        if ctx.is_inside_tool_queue() {
            return self.execute_tool_timed(name, args, ctx).await;
        }

        if let Some(ref queue) = self.command_queue {
            let command = ToolCommand::new(
                Arc::clone(&self.tool_executor),
                name.to_string(),
                args.clone(),
                ctx.clone().with_tool_queue_scope(),
                self.config.tool_timeout_ms,
            );
            let cancellation = ctx.cancellation_token();
            let rx = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    anyhow::bail!("Tool '{}' cancelled while waiting for queue submission", name);
                }
                rx = queue.submit_by_tool(name, Box::new(command)) => rx,
            };
            let queued = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    anyhow::bail!("Tool '{}' cancelled while waiting in the queue", name);
                }
                result = rx => result,
            };
            match queued {
                Ok(Ok(value)) => {
                    let output = value["output"]
                        .as_str()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Queue result missing 'output' field for tool '{}'",
                                name
                            )
                        })?
                        .to_string();
                    let exit_code = value["exit_code"].as_i64().unwrap_or(0) as i32;
                    let metadata = value
                        .get("metadata")
                        .filter(|value| !value.is_null())
                        .cloned();
                    let images = value
                        .get("images")
                        .and_then(Value::as_array)
                        .map(|images| {
                            images
                                .iter()
                                .map(|image| {
                                    let data = image
                                        .get("data")
                                        .and_then(Value::as_str)
                                        .ok_or_else(|| {
                                            anyhow::anyhow!(
                                                "Queue result has an image without base64 data for tool '{}'",
                                                name
                                            )
                                        })?;
                                    let media_type = image
                                        .get("media_type")
                                        .and_then(Value::as_str)
                                        .ok_or_else(|| {
                                            anyhow::anyhow!(
                                                "Queue result has an image without media_type for tool '{}'",
                                                name
                                            )
                                        })?;
                                    let data = BASE64_STANDARD.decode(data).map_err(|error| {
                                        anyhow::anyhow!(
                                            "Queue result has invalid image data for tool '{}': {}",
                                            name,
                                            error
                                        )
                                    })?;
                                    Ok(crate::llm::Attachment::new(data, media_type))
                                })
                                .collect::<anyhow::Result<Vec<_>>>()
                        })
                        .transpose()?
                        .unwrap_or_default();
                    let error_kind = value
                        .get("error_kind")
                        .filter(|value| !value.is_null())
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "Queue result has invalid 'error_kind' for tool '{}': {}",
                                name,
                                error
                            )
                        })?;
                    return Ok(crate::tools::ToolResult {
                        name: name.to_string(),
                        output,
                        exit_code,
                        metadata,
                        images,
                        error_kind,
                    });
                }
                Ok(Err(e)) => {
                    return Err(anyhow::anyhow!("Queued tool '{}' failed: {}", name, e));
                }
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "Queued tool '{}' result channel closed",
                        name
                    ));
                }
            }
        }
        self.execute_tool_timed(name, args, ctx).await
    }
    /// Create a tool context with streaming support.
    ///
    /// When `event_tx` is Some, spawns a forwarder task that converts
    /// `ToolStreamEvent::OutputDelta` into `AgentEvent::ToolOutputDelta`
    /// and sends them to the agent event channel.
    ///
    /// Returns the augmented `ToolContext`. The forwarder task runs until
    /// the tool-side sender is dropped (i.e., tool execution finishes).
    pub(super) fn streaming_tool_context(
        &self,
        base_ctx: &ToolContext,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        tool_id: &str,
        tool_name: &str,
    ) -> ToolContext {
        let mut ctx = base_ctx.clone();
        if let Some(agent_tx) = event_tx {
            let (tool_tx, mut tool_rx) = mpsc::channel::<ToolStreamEvent>(64);
            ctx.event_tx = Some(tool_tx);

            let agent_tx = agent_tx.clone();
            let tool_id = tool_id.to_string();
            let tool_name = tool_name.to_string();
            tokio::spawn(async move {
                while let Some(event) = tool_rx.recv().await {
                    match event {
                        ToolStreamEvent::OutputDelta(delta) => {
                            agent_tx
                                .send(AgentEvent::ToolOutputDelta {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    delta,
                                })
                                .await
                                .ok();
                        }
                    }
                }
            });
        }
        ctx
    }
}
