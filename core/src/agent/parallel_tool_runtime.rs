use super::execution_state::ExecutionLoopState;
use super::tool_result_runtime::{push_tool_result_message, NormalizedToolResult};
use super::{AgentEvent, AgentLoop};
use crate::llm::ToolCall;
use futures::future::join_all;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

impl AgentLoop {
    pub(super) async fn execute_parallel_write_batch(
        &self,
        tool_calls: &[ToolCall],
        state: &mut ExecutionLoopState,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        tracing::info!(
            count = tool_calls.len(),
            "Parallel write batch: executing {} independent file writes concurrently",
            tool_calls.len()
        );

        let futures = tool_calls
            .iter()
            .map(|tc| {
                let ctx = self.tool_context.clone();
                let executor = Arc::clone(&self.tool_executor);
                let name = tc.name.clone();
                let args = tc.args.clone();
                async move { executor.execute_with_context(&name, &args, &ctx).await }
            })
            .collect::<Vec<_>>();

        let results = join_all(futures).await;

        for (tc, result) in tool_calls.iter().zip(results) {
            state.record_tool_call();
            let normalized = NormalizedToolResult::from_execution(result);
            Self::collect_verification_report(
                &mut state.verification_reports,
                &normalized.metadata,
            );
            self.track_tool_result(&tc.name, &tc.args, normalized.exit_code);

            let output = if let Some(ref sp) = self.config.security_provider {
                sp.sanitize_output(&normalized.output)
            } else {
                normalized.output.clone()
            };

            if let Some(tx) = event_tx {
                tx.send(AgentEvent::ToolEnd {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    output: output.clone(),
                    exit_code: normalized.exit_code,
                    metadata: normalized.metadata.clone(),
                    error_kind: normalized.error_kind.clone(),
                })
                .await
                .ok();
            }

            push_tool_result_message(
                state,
                &tc.id,
                &output,
                normalized.is_error,
                normalized.images,
            );
        }
    }

    pub(super) fn can_run_parallel_write_batch(&self, tool_calls: &[ToolCall]) -> bool {
        if self.config.hook_engine.is_some()
            || self.config.confirmation_manager.is_some()
            || tool_calls.len() <= 1
        {
            return false;
        }

        if !tool_calls
            .iter()
            .all(|tc| is_parallel_safe_write(&tc.name, &tc.args))
        {
            return false;
        }

        let paths = tool_calls
            .iter()
            .filter_map(|tc| extract_write_path(&tc.args))
            .collect::<Vec<_>>();
        paths.len() == tool_calls.len() && paths.iter().collect::<HashSet<_>>().len() == paths.len()
    }
}

fn is_parallel_safe_write(name: &str, _args: &Value) -> bool {
    matches!(
        name,
        "write_file" | "edit_file" | "create_file" | "append_to_file" | "replace_in_file"
    )
}

fn extract_write_path(args: &Value) -> Option<String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parallel_write_safety_requires_known_write_tool_and_path() {
        assert!(is_parallel_safe_write(
            "write_file",
            &json!({"path":"a.txt"})
        ));
        assert!(!is_parallel_safe_write(
            "read_file",
            &json!({"path":"a.txt"})
        ));
        assert_eq!(
            extract_write_path(&json!({"path":"a.txt"})),
            Some("a.txt".to_string())
        );
        assert_eq!(extract_write_path(&json!({"file":"a.txt"})), None);
    }
}
