use super::*;
use a3s_code_tools::bridge::BackgroundNoticeNames;
use a3s_code_tools::implementations::grok_build::task::backend::ChannelBackend;
use a3s_tool_types::ForegroundSpawnInterrupt;

const HAND_OFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

impl SessionActor {
    pub(super) async fn hand_off_foreground_subagents(
        &self,
        prompt_id: &str,
        interrupt: ForegroundSpawnInterrupt,
    ) -> HashMap<String, String> {
        let Some(event_tx) = self.tool_context.subagent_event_tx.clone() else {
            return HashMap::new();
        };
        let backend = ChannelBackend::for_session(event_tx, self.session_id_string());
        let handed_off = match tokio::time::timeout(
            HAND_OFF_TIMEOUT,
            backend.hand_off_foreground_for_prompt(prompt_id),
        )
        .await
        {
            Ok(handed_off) => handed_off,
            Err(_) => {
                tracing::warn!(
                    prompt_id,
                    "subagent coordinator did not answer the foreground handoff; keeping halt text"
                );
                return HashMap::new();
            }
        };
        if handed_off.is_empty() {
            return HashMap::new();
        }
        // The children are already backgrounded here; a slow resources lock
        // must not cost the honest answers, so fall back to canonical names
        // and under-promise on completion notices.
        let names = tokio::time::timeout(
            HAND_OFF_TIMEOUT,
            self.tool_bridge_handle().background_notice_naming(),
        )
        .await
        .unwrap_or_else(|_| {
            let canonical = a3s_tool_types::BackgroundNoticeNaming::CANONICAL;
            BackgroundNoticeNames {
                task_output_tool: canonical.task_output_tool.to_owned(),
                task_ids_param: canonical.task_ids_param.to_owned(),
                timeout_ms_param: canonical.timeout_ms_param.to_owned(),
                notified_on_completion: false,
            }
        });
        let subagent_ids: Vec<&str> = handed_off.iter().map(|c| c.subagent_id.as_str()).collect();
        let tool_call_ids: Vec<&str> = handed_off.iter().map(|c| c.tool_call_id.as_str()).collect();
        let states: Vec<&str> = handed_off.iter().map(|c| c.state.as_str()).collect();
        a3s_code_telemetry::unified_log::info(
            "shell.cancel.subagents_handed_off",
            Some(self.session_info.id.0.as_ref()),
            Some(serde_json::json!({
                "prompt_id": prompt_id,
                "subagent_ids": subagent_ids,
                "tool_call_ids": tool_call_ids,
                "states": states,
                "trigger": interrupt.as_str(),
            })),
        );
        handed_off
            .into_iter()
            .map(|child| {
                let text = a3s_tool_types::format_subagent_backgrounded_on_turn_end(
                    &child.subagent_id,
                    &child.description,
                    &names.naming(),
                    interrupt,
                    child.state,
                    names.notified_on_completion,
                );
                (child.tool_call_id, text)
            })
            .collect()
    }
}
