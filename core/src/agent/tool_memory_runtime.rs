use super::{AgentEvent, AgentLoop};
use tokio::sync::mpsc;

impl AgentLoop {
    pub(super) async fn remember_tool_result(
        &self,
        effective_prompt: &str,
        tool_name: &str,
        output: &str,
        exit_code: i32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        let Some(ref memory) = self.config.memory else {
            return;
        };

        let tools_used = [tool_name.to_string()];
        let remember_result = if exit_code == 0 {
            memory
                .remember_success(effective_prompt, &tools_used, output)
                .await
        } else {
            memory
                .remember_failure(effective_prompt, output, &tools_used)
                .await
        };

        match remember_result {
            Ok(()) => {
                if let Some(tx) = event_tx {
                    let item_type = if exit_code == 0 { "success" } else { "failure" };
                    tx.send(AgentEvent::MemoryStored {
                        memory_id: uuid::Uuid::new_v4().to_string(),
                        memory_type: item_type.to_string(),
                        importance: if exit_code == 0 { 0.8 } else { 0.9 },
                        tags: vec![item_type.to_string(), tool_name.to_string()],
                    })
                    .await
                    .ok();
                }
            }
            Err(e) => {
                tracing::warn!("Failed to store memory after tool execution: {}", e);
            }
        }
    }
}
