//! `session/update` notifications for one ACP session.

use super::*;

impl A3sCodeAgent {
    pub(super) async fn emit(&self, notification: acp::SessionNotification) {
        let client = self.client.borrow().clone();
        if let Some(client) = client {
            let mut notification = notification;
            // Echo the in-flight turn's client prompt id on every
            // notification (the ACP prompt-ack contract: the pager disarms
            // its ack watch on the first update naming the prompt).
            if notification.meta.is_none() {
                let prompt_id = self.prompt_id_for_session(&notification.session_id);
                if let Some(prompt_id) = prompt_id {
                    let mut meta = acp::Meta::new();
                    meta.insert("promptId".to_string(), serde_json::json!(prompt_id));
                    notification.meta = Some(meta);
                }
            }
            let _ = client.session_notification(notification).await;
        }
    }

    pub(super) async fn emit_run_observation(
        &self,
        observation: Option<crate::run_observation::Observation>,
    ) {
        let Some(observation) = observation else {
            return;
        };
        let Some(client) = self.client.borrow().clone() else {
            return;
        };
        let params = match serde_json::value::to_raw_value(&observation) {
            Ok(params) => params,
            Err(error) => {
                tracing::warn!(%error, "Run observation serialization refused");
                return;
            }
        };
        if let Err(error) = client
            .ext_notification(acp::ExtNotification::new(
                crate::run_observation::METHOD,
                params.into(),
            ))
            .await
        {
            tracing::warn!(%error, "Run observation transport unavailable");
        }
    }

    pub(super) async fn close_run_observation(&self, state: &mut TurnState) {
        let observation = match state.observer.as_mut() {
            Some(observer) => observer.closed().await,
            None => None,
        };
        self.emit_run_observation(observation).await;
    }

    pub(super) async fn emit_text(&self, session_id: &acp::SessionId, text: &str) {
        if text.is_empty() {
            return;
        }
        tracing::info!(target: "perf_probe", len = text.len(), preview = %text.chars().take(60).collect::<String>(), "chunk_emit");
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(text.to_string()),
            ))),
        ))
        .await;
    }

    pub(super) async fn emit_tool_call(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        args: Option<&serde_json::Value>,
    ) {
        let mut call = acp::ToolCall::new(acp::ToolCallId::new(id.to_string()), tool_title(name))
            .kind(tool_kind_for(name))
            .status(acp::ToolCallStatus::InProgress);
        if let Some(raw_input) = args.and_then(|args| prepared_raw_input(name, args)) {
            call = call.raw_input(raw_input);
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCall(call),
        ))
        .await;
    }

    /// Attach a3s tool arguments once execution starts so the pager can render
    /// the Execute / Read / Edit / Search / Fetch card instead of a generic row.
    pub(super) async fn emit_tool_call_started(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        args: &serde_json::Value,
    ) {
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(tool_title(name))
            .kind(tool_kind_for(name))
            .status(acp::ToolCallStatus::InProgress);
        if let Some(raw_input) = prepared_raw_input(name, args) {
            fields = fields.raw_input(raw_input);
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id.to_string()),
                fields,
            )),
        ))
        .await;
    }

    pub(super) async fn emit_tool_call_completed(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        args: Option<&serde_json::Value>,
        output: &str,
        exit_code: i32,
    ) {
        let status = if exit_code == 0 {
            acp::ToolCallStatus::Completed
        } else {
            acp::ToolCallStatus::Failed
        };
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(tool_title(name))
            .kind(tool_kind_for(name))
            .status(status)
            .content(tool_call_content(name, args, output));
        if let Some(raw_input) = args.and_then(|args| prepared_raw_input(name, args)) {
            fields = fields.raw_input(raw_input);
        }
        if let Some(raw_output) = tool_raw_output(name, args, output, exit_code) {
            fields = fields.raw_output(raw_output);
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id.to_string()),
                fields,
            )),
        ))
        .await;
    }

    pub(super) async fn emit_tool_call_output_delta(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        delta: &str,
    ) {
        if delta.is_empty() {
            return;
        }
        let preview: String = delta.chars().take(4_000).collect();
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(tool_title(name))
            .kind(tool_kind_for(name))
            .status(acp::ToolCallStatus::InProgress)
            .content(vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
                acp::TextContent::new(preview.clone()),
            ))]);
        if matches!(name, "bash" | "shell" | "execute") {
            fields = fields.raw_output(streaming_bash_delta(&preview));
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id.to_string()),
                fields,
            )),
        ))
        .await;
    }

    pub(super) async fn emit_thought(&self, session_id: &acp::SessionId, text: &str) {
        if text.is_empty() {
            return;
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(text.to_string()),
            ))),
        ))
        .await;
    }

    pub(super) async fn emit_plan(
        &self,
        session_id: &acp::SessionId,
        tasks: &[a3s_code_core::planning::Task],
    ) {
        if tasks.is_empty() {
            return;
        }
        let entries = tasks.iter().map(task_to_plan_entry).collect::<Vec<_>>();
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::Plan(acp::Plan::new(entries)),
        ))
        .await;
    }

    /// Forward xAI-shaped session updates the pager already understands
    /// (`SubagentSpawned` / `SubagentProgress` / `SubagentFinished`).
    pub(super) async fn emit_xai_session_update(
        &self,
        session_id: &acp::SessionId,
        update: serde_json::Value,
    ) {
        let payload = serde_json::json!({
            "sessionId": session_id.0.as_ref(),
            "update": update,
        });
        let Ok(raw) = serde_json::value::to_raw_value(&payload) else {
            return;
        };
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return;
        };
        let _ = client
            .ext_notification(acp::ExtNotification::new(
                "x.ai/session_notification",
                raw.into(),
            ))
            .await;
    }

    /// Notify the client how the run closed at the completion gate, on a
    /// dedicated method so it needs no ACP enum variant. Emitted before the
    /// prompt response resolves (same FIFO wire ⇒ it precedes the marker).
    pub(super) async fn emit_turn_outcome(
        &self,
        session_id: &acp::SessionId,
        outcome: serde_json::Value,
    ) {
        let payload = serde_json::json!({
            "sessionId": session_id.0.as_ref(),
            "outcome": outcome,
        });
        let Ok(raw) = serde_json::value::to_raw_value(&payload) else {
            return;
        };
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return;
        };
        let _ = client
            .ext_notification(acp::ExtNotification::new("x.ai/turn/outcome", raw.into()))
            .await;
    }

    pub(super) async fn emit_subagent_start(
        &self,
        session_id: &acp::SessionId,
        task_id: &str,
        child_session_id: &str,
        parent_session_id: &str,
        agent: &str,
        description: &str,
    ) {
        self.emit_xai_session_update(
            session_id,
            serde_json::json!({
                "sessionUpdate": "subagent_spawned",
                "subagent_id": task_id,
                "parent_session_id": parent_session_id,
                "child_session_id": child_session_id,
                "subagent_type": agent,
                "description": description,
            }),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)] // event surface arity is the ACP wire shape
    pub(super) async fn emit_subagent_progress(
        &self,
        session_id: &acp::SessionId,
        task_id: &str,
        child_session_id: &str,
        status: &str,
        window_tokens: u64,
        metadata: &serde_json::Value,
        started_ms: u64,
        progress: &ChildProgress,
    ) {
        let tokens_used = progress_tokens_used(metadata);
        let (context_window_tokens, context_usage_pct) = context_usage(window_tokens, tokens_used);
        self.emit_xai_session_update(
            session_id,
            serde_json::json!({
                "sessionUpdate": "subagent_progress",
                "subagent_id": task_id,
                "parent_session_id": session_id.0.as_ref(),
                "child_session_id": child_session_id,
                "duration_ms": progress_duration_ms(started_ms, unix_now_ms()),
                "turn_count": progress.turn_count,
                "tool_call_count": progress.tool_call_count,
                "tokens_used": tokens_used,
                "context_window_tokens": context_window_tokens,
                "context_usage_pct": context_usage_pct,
                "tools_used": progress.tools_used,
                "error_count": progress.error_count,
            }),
        )
        .await;
        // Surface the progress text as thought so the TUI still shows movement
        // when the typed SubagentProgress payload has no message field.
        if !status.is_empty() {
            self.emit_thought(session_id, &format!("[{task_id}] {status}\n"))
                .await;
        }
    }

    #[allow(clippy::too_many_arguments)] // event surface arity is the ACP wire shape
    pub(super) async fn emit_subagent_end(
        &self,
        session_id: &acp::SessionId,
        task_id: &str,
        child_session_id: &str,
        success: bool,
        output: &str,
        started_ms: u64,
        finished_ms: u64,
        progress: &ChildProgress,
    ) {
        let duration_ms = finished_ms.saturating_sub(started_ms);
        let update = subagent_finished_update(
            task_id,
            child_session_id,
            success,
            output,
            duration_ms,
            progress,
        );
        self.emit_xai_session_update(session_id, update).await;
    }
}

pub(super) fn note_child_progress(
    progress: &mut ChildProgress,
    status: &str,
    metadata: &serde_json::Value,
) {
    let tokens = progress_tokens_used(metadata);
    if tokens > progress.tokens_used {
        progress.tokens_used = tokens;
    }
    if status == "tool_completed" {
        progress.tool_call_count = progress.tool_call_count.saturating_add(1);
        if let Some(name) = metadata.get("tool").and_then(|value| value.as_str()) {
            if !progress.tools_used.iter().any(|existing| existing == name) {
                progress.tools_used.push(name.to_string());
            }
        }
        let exit_code = metadata
            .get("exit_code")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if exit_code != 0 || metadata.get("error_kind").is_some() {
            progress.error_count = progress.error_count.saturating_add(1);
        }
    } else if status == "turn_completed" {
        if let Some(turn) = metadata.get("turn").and_then(|value| value.as_u64()) {
            progress.turn_count = progress.turn_count.max(turn as u32);
        }
    }
}

pub(super) fn subagent_finished_update(
    task_id: &str,
    child_session_id: &str,
    success: bool,
    output: &str,
    duration_ms: u64,
    progress: &ChildProgress,
) -> serde_json::Value {
    let status = if success { "completed" } else { "failed" };
    let error = if success {
        None
    } else if output.trim().is_empty() {
        Some("subagent failed".to_string())
    } else {
        Some(output.chars().take(2_000).collect::<String>())
    };
    let mut update = serde_json::json!({
        "sessionUpdate": "subagent_finished",
        "subagent_id": task_id,
        "child_session_id": child_session_id,
        "status": status,
        "tool_calls": progress.tool_call_count,
        "turns": progress.turn_count,
        "duration_ms": duration_ms,
        "tokens_used": progress.tokens_used,
        "will_wake": false,
    });
    if let Some(error) = error {
        update["error"] = serde_json::Value::String(error);
    }
    if !output.is_empty() {
        update["output"] = serde_json::Value::String(output.chars().take(4_000).collect());
    }
    update
}

pub(super) fn progress_duration_ms(started_ms: u64, now_ms: u64) -> u64 {
    if started_ms == 0 {
        return 0;
    }
    now_ms.saturating_sub(started_ms)
}

pub(super) fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn progress_tokens_used(metadata: &serde_json::Value) -> u64 {
    metadata
        .get("prompt_tokens")
        .or_else(|| metadata.get("total_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
}

pub(super) fn context_usage(window_tokens: u64, tokens_used: u64) -> (u64, u8) {
    if window_tokens == 0 {
        return (0, 0);
    }
    let pct = tokens_used.saturating_mul(100) / window_tokens;
    (window_tokens, pct.min(100) as u8)
}
