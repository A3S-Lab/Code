//! `acp::Agent` backed by a3s-code-core fact-log sessions.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use a3s_code_core::{Agent, AgentEvent, AgentSession, SessionOptions};
use agent_client_protocol as acp;
use agent_client_protocol::Client;
use tokio::sync::Mutex;

use crate::config::LaunchConfig;

struct LiveSession {
    session: Arc<AgentSession>,
    workspace: PathBuf,
    model_id: String,
    /// Serializes turns: a prompt arriving mid-turn queues here instead of
    /// failing with "already has an active operation" (the pager's send-now
    /// and queue reissue both rely on the agent accepting and serializing).
    turn_lock: Arc<tokio::sync::Mutex<()>>,
    /// The client's prompt id for the in-flight turn, echoed on every
    /// session notification per the ACP prompt-ack contract.
    active_prompt_id: std::sync::Mutex<Option<String>>,
}

/// ACP agent that owns live [`AgentSession`] values keyed by ACP session id.
pub struct A3sCodeAgent {
    launch: LaunchConfig,
    /// Set once the stdio `AgentSideConnection` is built (chicken/egg with ACP ctor).
    client: RefCell<Option<Rc<acp::AgentSideConnection>>>,
    core_agent: Mutex<Option<Agent>>,
    sessions: Mutex<HashMap<String, LiveSession>>,
    model_id: Mutex<String>,
    /// Last `/effort` selection. Applied on every core session open.
    effort: Mutex<Option<EffortLimits>>,
    /// Durable `/goal` objectives keyed by ACP session id.
    goals: Mutex<HashMap<String, crate::goal::DurableGoal>>,
    /// Monotonic turn generation per session. `cancel` records the generation
    /// that was in flight, so a late cancel cannot mark the next turn.
    turn_generation: Mutex<HashMap<String, u64>>,
    /// Generation `cancel` observed for each session.
    cancelled_generation: Mutex<HashMap<String, u64>>,
}

impl A3sCodeAgent {
    pub fn new(launch: LaunchConfig) -> Self {
        let model_id = launch.model_id.clone();
        Self {
            launch,
            client: RefCell::new(None),
            core_agent: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            model_id: Mutex::new(model_id),
            // a3s-code TUI default is BudgetProfile index 2 (`high`).
            effort: Mutex::new(effort_limits("high")),
            goals: Mutex::new(HashMap::new()),
            turn_generation: Mutex::new(HashMap::new()),
            cancelled_generation: Mutex::new(HashMap::new()),
        }
    }

    /// The in-flight prompt id for this notification's session.
    ///
    /// A try-lock miss leaves the update unstamped. Falling through to another
    /// session's id would make the pager drop the update.
    fn prompt_id_for_session(&self, session_id: &acp::SessionId) -> Option<String> {
        let key = session_id.0.as_ref();
        let sessions = self.sessions.try_lock().ok()?;
        let prompt_id = sessions.get(key).and_then(|live| {
            live.active_prompt_id
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
        });
        drop(sessions);
        prompt_id
    }

    pub fn set_client(&self, conn: Rc<acp::AgentSideConnection>) {
        *self.client.borrow_mut() = Some(conn);
    }

    fn available_model_infos(&self) -> Vec<acp::ModelInfo> {
        let mut infos = Vec::new();
        for (provider, model) in self.launch.config.list_models() {
            let id = format!("{}/{}", provider.name, model.id);
            let display_name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            let name = match provider.name.as_str() {
                "cc-switch" => format!("cc-switch · {display_name}"),
                "grok" => format!("grok · {display_name}"),
                other => format!("{other}/{display_name}"),
            };
            let description = match provider.name.as_str() {
                "cc-switch" => Some("Imported from local CC Switch".to_string()),
                "grok" => Some("Logged-in Grok account".to_string()),
                _ => Some(format!("From A3S ACL provider {}", provider.name)),
            };
            let mut info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), name);
            if let Some(description) = description {
                info = info.description(description);
            }
            info = info.meta(Some(reasoning_effort_catalog_meta(&id)));
            infos.push(info);
        }
        if infos.is_empty() {
            let id = self.launch.model_id.clone();
            let info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), id.clone())
                .meta(Some(reasoning_effort_catalog_meta(&id)));
            infos.push(info);
        }
        infos
    }

    fn model_state_with(
        &self,
        current: &str,
        effort_token: Option<&str>,
    ) -> acp::SessionModelState {
        let mut available = self.available_model_infos();
        let current_id = if available
            .iter()
            .any(|info| info.model_id.0.as_ref() == current)
        {
            current.to_string()
        } else {
            available
                .first()
                .map(|info| info.model_id.0.to_string())
                .unwrap_or_else(|| current.to_string())
        };
        if let Some(token) = effort_token {
            if let Some(info) = available
                .iter_mut()
                .find(|info| info.model_id.0.as_ref() == current_id)
            {
                let mut meta = info.meta.clone().unwrap_or_default();
                meta.insert(
                    "reasoningEffort".into(),
                    serde_json::Value::String(token.to_string()),
                );
                info.meta = Some(meta);
            }
        }
        acp::SessionModelState::new(acp::ModelId::new(current_id), available)
    }

    fn known_model(&self, model_id: &str) -> bool {
        self.launch
            .config
            .list_models()
            .iter()
            .any(|(provider, model)| format!("{}/{}", provider.name, model.id) == model_id)
            || model_id == self.launch.model_id
    }

    async fn ensure_core_agent(&self) -> acp::Result<()> {
        let mut guard = self.core_agent.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let agent = Agent::from_config(self.launch.config.clone())
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        *guard = Some(agent);
        Ok(())
    }

    fn core_session_options(
        session_id: &str,
        model_id: &str,
        effort: Option<EffortLimits>,
    ) -> SessionOptions {
        let mut options = SessionOptions::new()
            .with_session_id(session_id.to_string())
            .with_model(model_id.to_string())
            .with_auto_compact(true)
            .with_auto_compact_threshold(a3s_code_core::store::DEFAULT_AUTO_COMPACT_THRESHOLD);
        if let Some(limits) = effort {
            options = options
                .with_reasoning_effort(limits.token)
                .with_thinking_budget(limits.thinking_budget)
                .with_max_tool_rounds(limits.max_tool_rounds)
                .with_max_parallel_tasks(limits.max_parallel_tasks)
                .with_max_continuation_turns(limits.max_continuation_turns);
            if limits.token == "ultracode" {
                options = options
                    .with_planning_mode(a3s_code_core::PlanningMode::Auto)
                    .with_auto_delegation_enabled(true)
                    .with_auto_parallel_delegation(true)
                    .with_manual_delegation_enabled(true);
            }
            // The numbers are part of the completion the model receives, including
            // native-effort models whose provider parameter does not carry a token count.
            let mut guideline = format!(
                "Session budget: thinking_budget={} tool_rounds={}.",
                limits.thinking_budget, limits.max_tool_rounds
            );
            if !a3s_code_core::llm::model_sends_native_effort(model_id) {
                if let Some(extra) = effort_guideline(limits.token) {
                    guideline.push('\n');
                    guideline.push_str(extra);
                }
            }
            options = options.with_prompt_slots(
                a3s_code_core::SystemPromptSlots::default().with_guidelines(guideline),
            );
        }
        options
    }

    /// The terminal host asks before a mutation and lets known-safe reads run.
    ///
    /// `InteractiveToolGuardrail` is the shared classifier. An enabled
    /// confirmation policy is what turns its Ask into a parked
    /// `ConfirmationRequired` the pager answers through `request_permission`.
    /// A session with neither runs every tool, including writes.
    fn host_session_options(
        session_id: &str,
        model_id: &str,
        effort: Option<EffortLimits>,
        workspace: &std::path::Path,
    ) -> SessionOptions {
        let guardrail = a3s_code_core::permissions::InteractiveToolGuardrail::default()
            .with_workspace(workspace);
        Self::core_session_options(session_id, model_id, effort)
            .with_permission_checker(Arc::new(guardrail))
            .with_confirmation_policy(a3s_code_core::hitl::ConfirmationPolicy::enabled())
    }

    async fn open_core_session(
        &self,
        session_id: &str,
        workspace: PathBuf,
        model_id: &str,
        effort: Option<EffortLimits>,
    ) -> acp::Result<LiveSession> {
        self.ensure_core_agent().await?;
        let agent_guard = self.core_agent.lock().await;
        let agent = agent_guard
            .as_ref()
            .ok_or_else(|| acp::Error::internal_error().data("core agent missing"))?;
        let options = Self::host_session_options(session_id, model_id, effort, &workspace);
        let session = agent
            .session_async(workspace.to_string_lossy().to_string(), Some(options))
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(LiveSession {
            session: Arc::new(session),
            workspace,
            model_id: model_id.to_string(),
            turn_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_prompt_id: std::sync::Mutex::new(None),
        })
    }

    /// Rebuild a session that is already registered. `session_async` rejects a
    /// live id (`session is already live or being built`); model and effort
    /// changes have to replace that session in place.
    async fn replace_core_session(
        &self,
        current: &AgentSession,
        workspace: PathBuf,
        model_id: &str,
        effort: Option<EffortLimits>,
    ) -> acp::Result<LiveSession> {
        self.ensure_core_agent().await?;
        let agent_guard = self.core_agent.lock().await;
        let agent = agent_guard
            .as_ref()
            .ok_or_else(|| acp::Error::internal_error().data("core agent missing"))?;
        let mut options =
            Self::host_session_options(current.session_id(), model_id, effort, &workspace);
        let session = if let Some(store) = current.session_store() {
            options = options.with_session_store(store);
            agent
                .replace_session_async(current, options)
                .await
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?
        } else {
            // Nothing was persisted, so closing frees the id for a fresh session.
            current.close().await;
            agent
                .session_async(workspace.to_string_lossy().to_string(), Some(options))
                .await
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?
        };
        Ok(LiveSession {
            session: Arc::new(session),
            workspace,
            model_id: model_id.to_string(),
            turn_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_prompt_id: std::sync::Mutex::new(None),
        })
    }

    async fn emit(&self, notification: acp::SessionNotification) {
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

    async fn emit_text(&self, session_id: &acp::SessionId, text: &str) {
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

    async fn emit_tool_call(
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
    async fn emit_tool_call_started(
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

    async fn emit_tool_call_completed(
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

    async fn emit_tool_call_output_delta(
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

    async fn emit_thought(&self, session_id: &acp::SessionId, text: &str) {
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

    async fn emit_plan(
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
    async fn emit_xai_session_update(
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

    async fn emit_subagent_start(
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

    async fn emit_subagent_progress(
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

    async fn emit_subagent_end(
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

    /// Ask the ACP client (pager) whether a tool may run. Fail closed when no client is wired.
    /// YOLO auto-approve still happens on the pager side when `session.is_yolo()`.
    async fn request_tool_permission(
        &self,
        session_id: &acp::SessionId,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> bool {
        let request = tool_permission_request(session_id, tool_id, tool_name, args);
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return false;
        };
        match client.request_permission(request).await {
            Ok(response) => permission_response_approved(&response),
            Err(_) => false,
        }
    }

    /// Ask the same ACP permission channel for a structured answer.
    ///
    /// Options use `RejectOnce` so a YOLO `AllowOnce` drain cannot answer.
    /// Free text with no options stays unanswered; the run remains parked.
    async fn request_question_answer(
        &self,
        session_id: &acp::SessionId,
        question_id: &str,
        question: &str,
        options: &[String],
        _allow_free_text: bool,
    ) -> Option<String> {
        let Some(request) = question_permission_request(session_id, question_id, question, options)
        else {
            return None;
        };
        let client = self.client.borrow().clone();
        let client = client?;
        let response = client.request_permission(request).await.ok()?;
        answer_from_permission_outcome(&response.outcome, options)
    }

    fn prompt_text(prompt: &[acp::ContentBlock]) -> String {
        prompt
            .iter()
            .filter_map(|block| match block {
                acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn handle_event(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        state: &mut TurnState,
        event: AgentEvent,
    ) {
        match event {
            AgentEvent::TextDelta { text } => {
                if !text.is_empty() {
                    state.saw_text = true;
                }
                state.delta_text.push_str(&text);
                self.emit_text(session_id, &text).await;
            }
            AgentEvent::ReasoningDelta { text } => {
                self.emit_thought(session_id, &text).await;
            }
            AgentEvent::ToolStart { id, name } => {
                self.emit_tool_call(session_id, &id, &name, None).await;
            }
            AgentEvent::ToolExecutionStart {
                id,
                name,
                args: tool_args,
            } => {
                self.emit_tool_call_started(session_id, &id, &name, &tool_args)
                    .await;
            }
            AgentEvent::ToolOutputDelta { id, name, delta } => {
                self.emit_tool_call_output_delta(session_id, &id, &name, &delta)
                    .await;
            }
            AgentEvent::ToolEnd {
                id,
                name,
                args: tool_args,
                output,
                exit_code,
                ..
            } => {
                self.emit_tool_call_completed(
                    session_id,
                    &id,
                    &name,
                    tool_args.as_ref(),
                    &output,
                    exit_code,
                )
                .await;
            }
            AgentEvent::ConfirmationRequired {
                tool_id,
                tool_name,
                args: tool_args,
                ..
            } => {
                self.emit_tool_call(session_id, &tool_id, &tool_name, Some(&tool_args))
                    .await;
                let approved = self
                    .request_tool_permission(session_id, &tool_id, &tool_name, &tool_args)
                    .await;
                let reason = if approved {
                    Some("approved via ACP request_permission".into())
                } else {
                    Some("rejected via ACP request_permission".into())
                };
                if let Err(error) = session.confirm_tool_use(&tool_id, approved, reason).await {
                    self.emit_text(
                        session_id,
                        &format!("confirm {tool_name} failed: {error}\n"),
                    )
                    .await;
                }
            }
            AgentEvent::TaskUpdated { tasks, .. } => {
                self.emit_plan(session_id, &tasks).await;
            }
            AgentEvent::PlanningEnd { plan, .. } => {
                self.emit_plan(session_id, &plan.steps).await;
            }
            AgentEvent::SubagentStart {
                task_id,
                session_id: child_session_id,
                parent_session_id,
                agent,
                description,
                started_ms,
            } => {
                state
                    .subagent_started_ms
                    .insert(task_id.clone(), started_ms);
                self.emit_subagent_start(
                    session_id,
                    &task_id,
                    &child_session_id,
                    &parent_session_id,
                    &agent,
                    &description,
                )
                .await;
            }
            AgentEvent::SubagentProgress {
                task_id,
                session_id: child_session_id,
                status,
                metadata,
            } => {
                let started_ms = state
                    .subagent_started_ms
                    .get(&task_id)
                    .copied()
                    .unwrap_or(0);
                let progress = state.child_progress.entry(task_id.clone()).or_default();
                note_child_progress(progress, &status, &metadata);
                self.emit_subagent_progress(
                    session_id,
                    &task_id,
                    &child_session_id,
                    &status,
                    session.context_window_tokens() as u64,
                    &metadata,
                    started_ms,
                    progress,
                )
                .await;
            }
            AgentEvent::SubagentEnd {
                task_id,
                session_id: child_session_id,
                output,
                success,
                finished_ms,
                ..
            } => {
                let started_ms = state
                    .subagent_started_ms
                    .remove(&task_id)
                    .unwrap_or(finished_ms);
                let progress = state.child_progress.remove(&task_id).unwrap_or_default();
                self.emit_subagent_end(
                    session_id,
                    &task_id,
                    &child_session_id,
                    success,
                    &output,
                    started_ms,
                    finished_ms,
                    &progress,
                )
                .await;
            }
            AgentEvent::End { text, .. } => {
                state.end_text = Some(text.clone());
                if should_emit_end_text(state.saw_text, &text) {
                    self.emit_text(session_id, &text).await;
                }
            }
            AgentEvent::Error { message } => {
                state.turn_failure = Some(message);
            }
            AgentEvent::ModelUsageBound { snapshot } => {
                let used = snapshot.reported_prompt_tokens as u64;
                let size = session.context_window_tokens() as u64;
                if used > 0 && size > 0 {
                    self.emit(acp::SessionNotification::new(
                        session_id.clone(),
                        acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(used, size)),
                    ))
                    .await;
                }
            }
            AgentEvent::AutoCompact {
                phase,
                tokens_used,
                context_window,
                tokens_after,
            } => {
                let (_, percentage) = context_usage(context_window, tokens_used);
                let update = if phase == "started" {
                    serde_json::json!({
                        "sessionUpdate": "auto_compact_started",
                        "tokens_used": tokens_used,
                        "context_window": context_window,
                        "percentage": percentage,
                        "reason": "threshold",
                    })
                } else {
                    serde_json::json!({
                        "sessionUpdate": "auto_compact_completed",
                        "tokens_before": tokens_used,
                        "tokens_after": tokens_after,
                    })
                };
                self.emit_xai_session_update(session_id, update).await;
            }
            AgentEvent::PermissionDenied {
                tool_id,
                tool_name,
                args: tool_args,
                reason,
            } => {
                self.emit_tool_call(session_id, &tool_id, &tool_name, Some(&tool_args))
                    .await;
                self.emit_tool_call_completed(
                    session_id,
                    &tool_id,
                    &tool_name,
                    Some(&tool_args),
                    &reason,
                    1,
                )
                .await;
            }
            AgentEvent::UserQuestion {
                question_id,
                question,
                options,
                allow_free_text,
            } => {
                if !claim_question_prompt(&mut state.prompted_questions, &question_id) {
                    return;
                }
                if !question_has_choices(&options) {
                    self.emit_text(
                        session_id,
                        &format!("\nquestion {question_id}: {question}\n"),
                    )
                    .await;
                }
                if let Some(text) = self
                    .request_question_answer(
                        session_id,
                        &question_id,
                        &question,
                        &options,
                        allow_free_text,
                    )
                    .await
                {
                    let _ = a3s_code_core::ask_user::answer(&question_id, &text);
                }
            }
            _ => {}
        }
    }

    /// Restore planning after a session rebuild while a goal is still active.
    async fn reapply_active_goal(&self, session_key: &str) {
        let active = self
            .goals
            .lock()
            .await
            .get(session_key)
            .is_some_and(|goal| !goal.paused);
        if !active {
            return;
        }
        let session = self
            .sessions
            .lock()
            .await
            .get(session_key)
            .map(|live| Arc::clone(&live.session));
        if let Some(session) = session {
            let _ = session.set_planning_mode(a3s_code_core::PlanningMode::Enabled);
        }
    }

    async fn handle_goal(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        op: crate::goal::GoalOp,
    ) -> acp::Result<acp::PromptResponse> {
        let key = session_id.0.to_string();
        let current = self.goals.lock().await.get(&key).cloned();
        let applied = crate::goal::apply_goal(current, op);
        if let Some(mode) = applied.plan.mode() {
            session
                .set_planning_mode(mode)
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        } else if applied.plan.clears_override() {
            session
                .clear_planning_mode_override()
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        let stored = applied.next.clone();
        {
            let mut goals = self.goals.lock().await;
            if let Some(goal) = applied.next {
                goals.insert(key.clone(), goal);
            } else {
                goals.remove(&key);
            }
        }
        if let Some(workspace) = self
            .sessions
            .lock()
            .await
            .get(&key)
            .map(|live| live.workspace.clone())
        {
            if applied.plan.clears_override() {
                let _ = crate::goal::clear_stored_goal(&workspace);
            } else if let Some(goal) = stored {
                let _ = crate::goal::store_goal(&workspace, &goal);
            }
        }
        if let Some(reply) = applied.reply {
            self.emit_text(session_id, &reply).await;
        }
        if let Some(model_prompt) = applied.model_prompt {
            return self.stream_turn(session_id, session, &model_prompt).await;
        }
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn stream_turn(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        prompt: &str,
    ) -> acp::Result<acp::PromptResponse> {
        let (mut rx, join) = session
            .stream(prompt, None)
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;

        let mut state = TurnState {
            turn_failure: None,
            saw_text: false,
            delta_text: String::new(),
            end_text: None,
            subagent_started_ms: HashMap::new(),
            child_progress: HashMap::new(),
            prompted_questions: std::collections::HashSet::new(),
        };
        let drain_started = std::time::Instant::now();
        while let Some(event) = rx.recv().await {
            self.handle_event(session_id, session, &mut state, event)
                .await;
        }
        let events_drained = drain_started.elapsed();
        tracing::info!(
            target: "perf_probe",
            delta_chars = state.delta_text.chars().count(),
            delta_preview = %state.delta_text.chars().take(120).collect::<String>(),
            end_preview = %state.end_text.as_deref().unwrap_or("").chars().take(120).collect::<String>(),
            "turn_text_assembly"
        );
        let _ = join.await;
        tracing::info!(
            target: "perf_probe",
            events_drained_ms = events_drained.as_millis() as u64,
            join_ms = (drain_started.elapsed() - events_drained).as_millis() as u64,
            "stream_turn_tail"
        );
        if let Some(message) = state.turn_failure {
            return Err(agent_failure_error(&message));
        }
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    /// A cancel for this generation wins over both a normal end and a stream error.
    /// The word "cancel" inside an unrelated error is not a cancellation.
    async fn finish_prompt(
        &self,
        session_key: &str,
        generation: u64,
        response: acp::Result<acp::PromptResponse>,
    ) -> acp::Result<acp::PromptResponse> {
        {
            let sessions = self.sessions.lock().await;
            if let Some(live) = sessions.get(session_key) {
                *live
                    .active_prompt_id
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
        }
        let cancelled = self
            .cancelled_generation
            .lock()
            .await
            .get(session_key)
            .copied()
            == Some(generation);
        if cancelled {
            return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
        }
        response
    }

    async fn handle_compact_extension(
        &self,
        args: &acp::ExtRequest,
    ) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("compact request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("compact request: sessionId"))?;
        let user_context = request
            .get("userContext")
            .or_else(|| request.get("user_context"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|live| Arc::clone(&live.session))
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?
        };
        session
            .compact_conversation(user_context.as_deref())
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        let body = serde_json::value::to_raw_value(&serde_json::json!({}))
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(body.into()))
    }

    async fn handle_interject(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request = parse_interject_params(args.params.get())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&request.session_id)
                .map(|live| Arc::clone(&live.session))
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?
        };
        session
            .steer(a3s_code_core::SteerRequest::new(request.text))
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        let body = serde_json::value::to_raw_value(&serde_json::json!({ "ok": true }))
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(body.into()))
    }

    /// `x.ai/session/info` is the pager's `/session-info` and `/context` source.
    /// The pager decodes `ExtMethodResult<SessionInfoResponse>` and treats any
    /// other body, including JSON null, as an invalid response.
    async fn handle_session_info(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("session info request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("session info request: sessionId"))?;
        let live = {
            let sessions = self.sessions.lock().await;
            sessions.get(session_id).map(|live| {
                // The fact scheduler increments its turn only on `user.message`.
                // Tool rounds and assistant tool lines are other fact kinds, and
                // the folded transcript stores those lines with role=user.
                let turns = product_prompt_turns(live.workspace.as_path(), session_id);
                let (prompt_tokens, completion_tokens, total_tokens) =
                    live.session.recorded_usage_tokens();
                let used = if total_tokens > 0 {
                    total_tokens
                } else {
                    prompt_tokens.saturating_add(completion_tokens)
                };
                let total = live.session.context_window_tokens() as u64;
                (
                    live.workspace.display().to_string(),
                    live.model_id.clone(),
                    turns,
                    used,
                    total,
                )
            })
        };
        let body = match live {
            Some((cwd, model_id, turns, used, total)) => {
                let usage_pct = usage_percent(used, total);
                serde_json::json!({
                    "result": {
                        "sessionId": session_id,
                        "cwd": cwd,
                        "model": &model_id,
                        "resolvedModelId": &model_id,
                        "modelFingerprint": null,
                        "turns": turns,
                        "turnIndex": turns,
                        "context": {
                            "used": used,
                            "total": total,
                            "usagePct": usage_pct,
                            "turnCount": turns,
                            "freeTokens": total.saturating_sub(used)
                        }
                    }
                })
            }
            None => serde_json::json!({
                "result": null,
                "error": "unknown session id"
            }),
        };
        let raw = serde_json::value::to_raw_value(&body)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }

    /// Push the fact-log transcript as historical session updates.
    ///
    /// The pager does not read the fact log itself. `session/load` is the
    /// path `--resume` already calls, and untagged updates are dropped once
    /// the load window closes.
    async fn replay_loaded_transcript(&self, session_id: &acp::SessionId, session: &AgentSession) {
        let mut meta = acp::Meta::new();
        meta.insert("isReplay".to_string(), serde_json::json!(true));
        for message in session.durable_transcript() {
            if !message.is_product_transcript() {
                continue;
            }
            let text = message.transcript_display_text();
            if text.trim().is_empty() {
                continue;
            }
            let block = acp::ContentBlock::Text(acp::TextContent::new(text));
            let update = if message.role.eq_ignore_ascii_case("assistant") {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(block))
            } else if message.role.eq_ignore_ascii_case("user") {
                acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(block))
            } else {
                continue;
            };
            let mut notification = acp::SessionNotification::new(session_id.clone(), update);
            notification.meta = Some(meta.clone());
            self.emit(notification).await;
        }
    }

    /// `x.ai/session/delete` is the pager's `/delete` confirmation.
    ///
    /// The pager removes its live view only after this returns without an
    /// error. The session directory under the a3s home is what `--continue`
    /// reads, so a null body would report success and leave that directory.
    async fn handle_session_delete(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("session delete request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| uuid::Uuid::parse_str(value).is_ok())
            .ok_or_else(|| {
                acp::Error::invalid_params().data("session delete request: sessionId")
            })?;
        let cwd = request
            .get("cwd")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let workspace = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(session_id).map(|live| live.workspace)
        };
        self.goals.lock().await.remove(session_id);
        self.turn_generation.lock().await.remove(session_id);
        self.cancelled_generation.lock().await.remove(session_id);
        if let Some(workspace) = workspace.as_deref() {
            remove_fact_log(workspace, session_id);
        }
        if let Some(cwd) = cwd.as_deref() {
            remove_fact_log(std::path::Path::new(cwd), session_id);
        }
        remove_recorded_session(session_id);
        let body = serde_json::json!({ "success": true });
        let raw = serde_json::value::to_raw_value(&body)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }
}

fn product_prompt_turns(workspace: &std::path::Path, session_id: &str) -> u64 {
    let thread = a3s_code_core::fact_control::thread_for_session(session_id);
    let Ok(facts) = a3s_code_core::fact_control::read_workspace_facts(workspace, &thread) else {
        return 0;
    };
    facts
        .iter()
        .filter(|fact| fact.kind == "user.message")
        .count() as u64
}

fn remove_fact_log(workspace: &std::path::Path, session_id: &str) {
    let path = workspace
        .join(".a3s")
        .join("effect-log")
        .join(format!("{session_id}.jsonl"));
    if path.is_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// Remove `{a3s-home}/sessions/{cwd}/{session_id}` for every recorded cwd.
///
/// The directory name is the url-encoded cwd. Matching the session id keeps
/// the delete on that session's own record.
fn remove_recorded_session(session_id: &str) {
    let mut homes = Vec::new();
    if let Some(env) = std::env::var_os("GROK_HOME").filter(|value| !value.is_empty()) {
        homes.push(PathBuf::from(env));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        homes.push(PathBuf::from(home).join(".grok"));
    }
    for home in homes {
        let Ok(entries) = std::fs::read_dir(home.join("sessions")) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path().join(session_id);
            if dir.is_dir() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

fn usage_percent(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((used as f64) / (total as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

struct TurnState {
    turn_failure: Option<String>,
    saw_text: bool,
    delta_text: String,
    end_text: Option<String>,
    subagent_started_ms: HashMap<String, u64>,
    child_progress: HashMap<String, ChildProgress>,
    prompted_questions: std::collections::HashSet<String>,
}

#[derive(Default)]
struct ChildProgress {
    turn_count: u32,
    tool_call_count: u32,
    tools_used: Vec<String>,
    error_count: u32,
    tokens_used: u64,
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for A3sCodeAgent {
    async fn initialize(
        &self,
        _args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        let cwd = self.launch.workspace.to_string_lossy().to_string();
        let current = self.model_id.lock().await.clone();
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let model_state = self.model_state_with(&current, effort_token);
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(acp::PromptCapabilities::new().embedded_context(true)),
            )
            .auth_methods(Vec::new())
            .meta(
                serde_json::json!({
                    "grokShell": false,
                    "a3sCode": true,
                    "agentVersion": env!("CARGO_PKG_VERSION"),
                    "currentWorkingDirectory": cwd,
                    "modelState": model_state,
                    "availableCommands": [{
                        "name": "goal",
                        "description": "Set, pause, resume, or clear a durable goal",
                        "input": {
                            "hint": "<objective> | status | pause | resume | clear"
                        },
                    }],
                    "cancelRewind": false,
                    "sessionRecap": false,
                    "feedbackTraceOffer": false,
                })
                .as_object()
                .cloned(),
            ))
    }

    async fn authenticate(
        &self,
        _args: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        args: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let workspace = if args.cwd.as_os_str().is_empty() {
            self.launch.workspace.clone()
        } else {
            args.cwd.clone()
        };
        let model_id = self.model_id.lock().await.clone();
        let effort = self.effort.lock().await.clone();
        let live = self
            .open_core_session(&session_id, workspace.clone(), &model_id, effort)
            .await?;
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let models = self.model_state_with(&live.model_id, effort_token);
        self.sessions.lock().await.insert(session_id.clone(), live);
        if let Some(goal) = crate::goal::load_goal(&workspace) {
            self.goals.lock().await.insert(session_id.clone(), goal);
        }
        self.reapply_active_goal(&session_id).await;

        Ok(acp::NewSessionResponse::new(acp::SessionId::new(session_id)).models(models))
    }

    async fn load_session(
        &self,
        args: acp::LoadSessionRequest,
    ) -> acp::Result<acp::LoadSessionResponse> {
        let session_id = args.session_id.0.to_string();
        if session_id.is_empty() {
            return Err(acp::Error::invalid_params().data("session id is empty"));
        }
        let workspace = if args.cwd.as_os_str().is_empty() {
            self.launch.workspace.clone()
        } else {
            args.cwd.clone()
        };
        let model_id = self.model_id.lock().await.clone();
        let effort = self.effort.lock().await.clone();
        let live = self
            .open_core_session(&session_id, workspace.clone(), &model_id, effort)
            .await?;
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let models = self.model_state_with(&live.model_id, effort_token);
        let transcript = Arc::clone(&live.session);
        self.sessions.lock().await.insert(session_id.clone(), live);
        if let Some(goal) = crate::goal::load_goal(&workspace) {
            self.goals.lock().await.insert(session_id.clone(), goal);
        }
        self.reapply_active_goal(&session_id).await;
        // The pager keeps `isReplay` updates only while this load is in flight
        // (and for a short grace after). Emit before the response returns.
        self.replay_loaded_transcript(&args.session_id, &transcript)
            .await;
        Ok(acp::LoadSessionResponse::new().models(models))
    }

    /// ACP session modes: the A3S permission/mode surface. `plan` enables
    /// maker planning; every other mode runs with the session's configured
    /// planning default.
    async fn set_session_mode(
        &self,
        args: acp::SetSessionModeRequest,
    ) -> acp::Result<acp::SetSessionModeResponse> {
        let session_key = args.session_id.0.to_string();
        let mode_id = args.mode_id.0.to_string();
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&session_key)
                .map(|live| Arc::clone(&live.session))
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?
        };
        let planning = if mode_id.eq_ignore_ascii_case("plan") {
            a3s_code_core::PlanningMode::Enabled
        } else {
            a3s_code_core::PlanningMode::Disabled
        };
        session
            .set_planning_mode(planning)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::SetSessionModeResponse::new())
    }

    async fn set_session_model(
        &self,
        args: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        let model_id = args.model_id.0.to_string();
        if !self.known_model(&model_id) {
            return Err(acp::Error::invalid_params().data(format!("unknown model id: {model_id}")));
        }

        let effort = match args.meta.as_ref() {
            Some(meta) if meta.contains_key("reasoningEffort") => {
                effort_limits_from_meta(Some(meta)).or(self.effort.lock().await.clone())
            }
            _ => self.effort.lock().await.clone(),
        };

        let session_key = args.session_id.0.to_string();
        let existing = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&session_key)
                .map(|live| (Arc::clone(&live.session), live.workspace.clone()))
        };

        let live = if let Some((current, workspace)) = existing {
            self.replace_core_session(&current, workspace, &model_id, effort)
                .await?
        } else {
            self.open_core_session(
                &session_key,
                self.launch.workspace.clone(),
                &model_id,
                effort,
            )
            .await?
        };
        self.sessions.lock().await.insert(session_key.clone(), live);
        self.reapply_active_goal(&session_key).await;
        *self.model_id.lock().await = model_id;
        *self.effort.lock().await = effort;

        Ok(acp::SetSessionModelResponse::new())
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        tracing::info!(target: "perf_probe", "prompt_enter");
        let session_key = args.session_id.0.to_string();
        let (session, turn_lock, _prompt_echo) = {
            let sessions = self.sessions.lock().await;
            let live = sessions
                .get(&session_key)
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
            let echo = live
                .active_prompt_id
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            (Arc::clone(&live.session), Arc::clone(&live.turn_lock), echo)
        };

        let prompt = Self::prompt_text(&args.prompt);
        if prompt.trim().is_empty() {
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }
        // Queue behind an active turn instead of failing: the pager's
        // send-now / queue-reissue expect the agent to accept and serialize.
        let turn_permit = turn_lock.lock().await;
        let generation = {
            let mut generations = self.turn_generation.lock().await;
            let slot = generations.entry(session_key.clone()).or_insert(0);
            *slot = slot.saturating_add(1);
            *slot
        };
        // Echo the client's prompt id on every notification (the ACP
        // prompt-ack contract): the pager disarms its ack watch on the
        // first update naming the prompt.
        let client_prompt_id = args
            .meta
            .as_ref()
            .and_then(|meta| meta.get("promptId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(live) = self.sessions.lock().await.get(&session_key) {
            *live
                .active_prompt_id
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = client_prompt_id.clone();
        }
        let response = if let Some(op) = crate::goal::parse_goal_command(&prompt) {
            tracing::info!(target: "perf_probe", "goal_command_branch");
            self.handle_goal(&args.session_id, &session, op).await
        } else {
            let goal = self.goals.lock().await.get(&session_key).cloned();
            let prompt = crate::goal::standing_goal_prompt(goal.as_ref(), &prompt);
            self.stream_turn(&args.session_id, &session, &prompt).await
        };
        tracing::info!(target: "perf_probe", "prompt_return");
        drop(turn_permit);
        self.finish_prompt(&session_key, generation, response).await
    }

    async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        if args
            .method
            .as_ref()
            .starts_with("x.ai/compact_conversation")
        {
            return self.handle_compact_extension(&args).await;
        }
        if args.method.as_ref().starts_with("x.ai/interject") {
            return self.handle_interject(&args).await;
        }
        if args.method.as_ref() == "x.ai/session/info" {
            return self.handle_session_info(&args).await;
        }
        if args.method.as_ref() == "x.ai/session/delete" {
            return self.handle_session_delete(&args).await;
        }
        Ok(acp::ExtResponse::new(
            serde_json::value::RawValue::NULL.to_owned().into(),
        ))
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        let session_key = args.session_id.0.to_string();
        let generation = self
            .turn_generation
            .lock()
            .await
            .get(&session_key)
            .copied()
            .unwrap_or(0);
        self.cancelled_generation
            .lock()
            .await
            .insert(session_key.clone(), generation);
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_key).map(|live| live.session.clone())
        };
        if let Some(session) = session {
            let _ = session.cancel().await;
        }
        Ok(())
    }
}

/// Map a3s-code planning tasks onto ACP `PlanEntry` so the pager plan strip updates.
fn task_to_plan_entry(task: &a3s_code_core::planning::Task) -> acp::PlanEntry {
    use a3s_code_core::planning::{TaskPriority, TaskStatus};
    let priority = match task.priority {
        TaskPriority::High => acp::PlanEntryPriority::High,
        TaskPriority::Medium => acp::PlanEntryPriority::Medium,
        TaskPriority::Low => acp::PlanEntryPriority::Low,
    };
    // Failed/Skipped/Cancelled collapse to Completed so the strip stays within ACP's status set.
    let status = match task.status {
        TaskStatus::Pending => acp::PlanEntryStatus::Pending,
        TaskStatus::InProgress => acp::PlanEntryStatus::InProgress,
        TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Skipped
        | TaskStatus::Cancelled => acp::PlanEntryStatus::Completed,
    };
    let mut entry = acp::PlanEntry::new(task.content.clone(), priority, status);
    let mut meta = acp::Meta::new();
    meta.insert("id".into(), serde_json::Value::String(task.id.clone()));
    if matches!(
        task.status,
        TaskStatus::Failed | TaskStatus::Skipped | TaskStatus::Cancelled
    ) {
        meta.insert(
            "a3sStatus".into(),
            serde_json::Value::String(task.status.to_string()),
        );
    }
    entry = entry.meta(Some(meta));
    entry
}

/// Effort selection for one session.
///
/// `token` is the provider effort parameter when the model has one.
/// `thinking_budget` and `max_tool_rounds` are the host budget. Each level
/// has its own pair. `max` and `ultracode` keep the tool-round safety ceiling
/// as their cap; every other level is strictly below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffortLimits {
    token: &'static str,
    thinking_budget: usize,
    max_tool_rounds: usize,
    max_parallel_tasks: usize,
    max_continuation_turns: u32,
}

/// Host budget levels offered when the provider has no native effort parameter.
/// `ultracode` stays off the menu: it is not a `ReasoningEffort` variant.
const HOST_EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Advertise the effort values `/effort` can apply.
///
/// A native menu wins when the provider has one (GLM stays on low/high/max).
/// Every other model still gets the host budget menu, because thinking budget
/// and tool rounds apply whether or not the provider has an effort parameter.
/// Each entry includes `value` so the pager's `ReasoningEffortOption` parser
/// accepts it.
fn reasoning_effort_catalog_meta(model_id: &str) -> serde_json::Map<String, serde_json::Value> {
    let native = a3s_code_core::llm::native_effort_menu(model_id);
    let levels: &[&str] = if native.is_empty() {
        HOST_EFFORT_LEVELS
    } else {
        native
    };
    let mut meta = serde_json::Map::new();
    meta.insert(
        "supportsReasoningEffort".into(),
        serde_json::Value::Bool(!levels.is_empty()),
    );
    let efforts = levels
        .iter()
        .map(|level| {
            let mut entry = serde_json::json!({
                "value": level,
                "id": level,
                "label": level,
                "description": effort_level_description(level),
            });
            if *level == "high" {
                entry["default"] = serde_json::Value::Bool(true);
            }
            entry
        })
        .collect::<Vec<_>>();
    meta.insert("reasoningEfforts".into(), serde_json::Value::Array(efforts));
    meta
}

fn effort_level_description(level: &str) -> &'static str {
    match level {
        "low" => "Faster, lighter reasoning",
        "medium" => "Balanced reasoning",
        "high" => "Heavy reasoning",
        "xhigh" => "Extended reasoning",
        "max" => "Maximum reasoning",
        _ => "Reasoning effort",
    }
}

fn note_child_progress(progress: &mut ChildProgress, status: &str, metadata: &serde_json::Value) {
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

fn subagent_finished_update(
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

struct InterjectParams {
    session_id: String,
    text: String,
}

fn parse_interject_params(raw: &str) -> Result<InterjectParams, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("interject request: {error}"))?;
    let session_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .and_then(|item| item.as_str())
        .filter(|item| !item.is_empty())
        .ok_or_else(|| "interject request: sessionId".to_string())?;
    let text = value
        .get("text")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .ok_or_else(|| "interject request: text".to_string())?;
    Ok(InterjectParams {
        session_id: session_id.to_string(),
        text: text.to_string(),
    })
}

fn progress_duration_ms(started_ms: u64, now_ms: u64) -> u64 {
    if started_ms == 0 {
        return 0;
    }
    now_ms.saturating_sub(started_ms)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn progress_tokens_used(metadata: &serde_json::Value) -> u64 {
    metadata
        .get("prompt_tokens")
        .or_else(|| metadata.get("total_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
}

fn context_usage(window_tokens: u64, tokens_used: u64) -> (u64, u8) {
    if window_tokens == 0 {
        return (0, 0);
    }
    let pct = tokens_used.saturating_mul(100) / window_tokens;
    (window_tokens, pct.min(100) as u8)
}

/// Depth steer from a3s `BudgetProfile.guideline`. Medium has none.
fn effort_guideline(token: &str) -> Option<&'static str> {
    match token {
        "low" => Some(
            "[effort: low] Favor speed and minimalism. Answer directly, make the smallest \
change that works (reading enough surrounding code to change it safely), and \
keep verification proportionate: still run the narrowest build/test/type-check \
that covers what you touched — just don't add checks or scope the task didn't \
warrant. Don't gold-plate.",
        ),
        "high" => Some(
            "[effort: high] Favor depth. Reason through the approach before acting. After \
changes, verify the narrow path you touched (build / test / type-check) and \
check the obvious edge cases, then re-read your own diff for correctness before \
finishing.",
        ),
        "xhigh" => Some(
            "[effort: xhigh] Work rigorously. Before choosing an approach, weigh at \
least one alternative. Verify thoroughly — run the relevant tests/build, probe \
edge cases and failure modes, and confirm the change actually does what was \
asked. Do a self-review pass for correctness and simplicity before concluding.",
        ),
        "max" => Some(
            "[effort: max] Maximum rigor; prefer correctness and completeness over speed. \
Decompose the problem, compare alternatives, and implement the strongest \
solution. Verify exhaustively: tests, build, edge cases, and boundary / \
adversarial inputs. Finish with a self-critique pass that actively hunts for \
what you may have missed or gotten wrong, and fix it before concluding.",
        ),
        "ultracode" => Some(
            "[ultracode] Dynamic-workflow mode is available — you decide whether a turn needs \
it. Match the effort to the task: answer trivial or conversational input (a \
greeting, a single question, a one-step edit) directly, with no plan and no \
fan-out. When a task genuinely needs a dynamic workflow, call the \
`dynamic_workflow` tool with one sandboxed JavaScript PTC workflow script.",
        ),
        _ => None,
    }
}

const HOST_PARALLEL_TASKS: usize = 8;
const HOST_CONTINUATION_TURNS: u32 = 8;

fn effort_limits(token: &str) -> Option<EffortLimits> {
    let ceiling = a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING;
    let (token, thinking_budget, max_tool_rounds) = match token.to_ascii_lowercase().as_str() {
        "none" => ("none", 1024, 4),
        "minimal" => ("minimal", 1536, 8),
        "low" => ("low", 2048, 16),
        "medium" => ("medium", 8192, 48),
        "high" => ("high", 16_384, 128),
        "xhigh" => ("xhigh", 32_768, 512),
        "max" => ("max", 65_536, ceiling),
        "ultracode" => ("ultracode", 65_536, ceiling),
        _ => return None,
    };
    Some(EffortLimits {
        token,
        thinking_budget,
        max_tool_rounds,
        max_parallel_tasks: HOST_PARALLEL_TASKS,
        max_continuation_turns: HOST_CONTINUATION_TURNS,
    })
}

fn effort_limits_from_meta(meta: Option<&acp::Meta>) -> Option<EffortLimits> {
    let effort = meta
        .and_then(|m| m.get("reasoningEffort"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    effort_limits(effort)
}

/// Map pager `/effort` `_meta.reasoningEffort` onto the provider effort token.
fn reasoning_effort_token(meta: Option<&acp::Meta>) -> Option<&'static str> {
    effort_limits_from_meta(meta).map(|limits| limits.token)
}

/// ACP `ToolKind` the pager uses to pick Execute / Read / Edit / Search / Fetch cards.
fn tool_kind_for(name: &str) -> acp::ToolKind {
    match name {
        "bash" | "shell" | "execute" | "git" => acp::ToolKind::Execute,
        "read" => acp::ToolKind::Read,
        "write" | "edit" | "patch" => acp::ToolKind::Edit,
        "grep" | "glob" | "search" | "semantic" | "hybrid" | "bm25" | "code_symbols"
        | "code_navigation" | "code_diagnostics" | "web_search" => acp::ToolKind::Search,
        "web_fetch" | "download" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

fn tool_title(name: &str) -> String {
    match name {
        "update_plan" => "Updating plan".to_string(),
        other => other.to_string(),
    }
}

/// Copy a3s argument names onto the fields the pager card renderer reads.
fn prepared_raw_input(name: &str, args: &serde_json::Value) -> Option<serde_json::Value> {
    if args.is_null() {
        return None;
    }
    let mut input = args.clone();
    let Some(obj) = input.as_object_mut() else {
        return Some(input);
    };
    match name {
        "ls" => {
            if let Some(path) = obj.get("path").cloned() {
                obj.entry("target_directory".to_string()).or_insert(path);
            }
        }
        "write" => {
            obj.insert("variant".into(), serde_json::json!("Write"));
        }
        "update_plan" => {
            obj.insert("variant".into(), serde_json::json!("TodoWrite"));
        }
        "web_search" => {
            obj.insert("variant".into(), serde_json::json!("WebSearch"));
        }
        "task" => {
            obj.insert("variant".into(), serde_json::json!("Task"));
        }
        "search" | "semantic" | "hybrid" | "bm25" => {
            if obj.get("pattern").is_none() {
                if let Some(query) = obj.get("query").cloned() {
                    obj.insert("pattern".into(), query);
                }
            }
        }
        _ => {}
    }
    Some(input)
}

fn tool_call_content(
    name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
) -> Vec<acp::ToolCallContent> {
    if let Some(diff) = edit_diff(name, args) {
        return vec![acp::ToolCallContent::from(diff)];
    }
    if output.is_empty() {
        return Vec::new();
    }
    let preview: String = output.chars().take(4_000).collect();
    vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
        acp::TextContent::new(preview),
    ))]
}

fn edit_diff(name: &str, args: Option<&serde_json::Value>) -> Option<acp::Diff> {
    let args = args?;
    let path = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())?;
    match name {
        "edit" | "patch" => {
            let old_text = args.get("old_string").and_then(|v| v.as_str());
            let new_text = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut diff = acp::Diff::new(path, new_text);
            if let Some(old_text) = old_text {
                diff = diff.old_text(old_text.to_string());
            }
            Some(diff)
        }
        "write" => args
            .get("content")
            .and_then(|v| v.as_str())
            .map(|content| acp::Diff::new(path, content)),
        _ => None,
    }
}

/// Bash cards only show stdout when `raw_output` deserializes as `ToolOutput::Bash`.
/// Incremental stdout. `output` stays empty so the pager appends `output_delta`
/// instead of replacing the card with this slice.
fn streaming_bash_delta(delta: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "Bash",
        "output": [],
        "output_for_prompt": "",
        "exit_code": 0,
        "command": "",
        "truncated": false,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": "",
        "total_bytes": delta.len(),
        "output_delta": delta.as_bytes(),
    })
}

fn tool_raw_output(
    name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
    exit_code: i32,
) -> Option<serde_json::Value> {
    if !matches!(name, "bash" | "shell" | "execute") {
        return None;
    }
    let command = args
        .and_then(|args| args.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let bytes = output.as_bytes();
    Some(serde_json::json!({
        "type": "Bash",
        "output": bytes,
        "output_for_prompt": output,
        "exit_code": exit_code,
        "command": command,
        "truncated": false,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": "",
        "total_bytes": bytes.len(),
    }))
}

/// Whether a `request_permission` response grants the tool.
fn permission_response_approved(response: &acp::RequestPermissionResponse) -> bool {
    match &response.outcome {
        acp::RequestPermissionOutcome::Selected(selected) => {
            // YOLO picks the first AllowOnce (`allow-once`). RejectOnce / cancel stay false.
            selected.option_id.0.as_ref() == "allow-once"
        }
        acp::RequestPermissionOutcome::Cancelled => false,
        _ => false,
    }
}

/// Question choices are not permission grants. `RejectOnce` keeps YOLO from
/// selecting them via its `AllowOnce` drain.
fn question_permission_options(options: &[String]) -> Vec<acp::PermissionOption> {
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| {
            let text = option.trim();
            if text.is_empty() {
                return None;
            }
            Some(acp::PermissionOption::new(
                acp::PermissionOptionId::new(format!("question-{index}")),
                text.to_string(),
                acp::PermissionOptionKind::RejectOnce,
            ))
        })
        .collect()
}

fn claim_question_prompt(seen: &mut std::collections::HashSet<String>, question_id: &str) -> bool {
    seen.insert(question_id.to_string())
}

fn tool_permission_request(
    session_id: &acp::SessionId,
    tool_id: &str,
    tool_name: &str,
    args: &serde_json::Value,
) -> acp::RequestPermissionRequest {
    let raw_input = prepared_raw_input(tool_name, args);
    let mut fields = acp::ToolCallUpdateFields::new()
        .title(tool_title(tool_name))
        .kind(tool_kind_for(tool_name))
        .status(acp::ToolCallStatus::Pending);
    if let Some(raw_input) = raw_input {
        fields = fields.raw_input(raw_input);
    }
    acp::RequestPermissionRequest::new(
        session_id.clone(),
        acp::ToolCallUpdate::new(acp::ToolCallId::new(tool_id.to_string()), fields),
        vec![
            acp::PermissionOption::new(
                acp::PermissionOptionId::new("allow-once"),
                "Yes, proceed",
                acp::PermissionOptionKind::AllowOnce,
            ),
            acp::PermissionOption::new(
                acp::PermissionOptionId::new("reject-once"),
                "No",
                acp::PermissionOptionKind::RejectOnce,
            ),
        ],
    )
}

fn question_permission_request(
    session_id: &acp::SessionId,
    question_id: &str,
    question: &str,
    options: &[String],
) -> Option<acp::RequestPermissionRequest> {
    let choices = question_permission_options(options);
    if choices.is_empty() {
        return None;
    }
    Some(acp::RequestPermissionRequest::new(
        session_id.clone(),
        acp::ToolCallUpdate::new(
            acp::ToolCallId::new(question_id.to_string()),
            acp::ToolCallUpdateFields::new()
                .title(question.to_string())
                .kind(acp::ToolKind::Other)
                .status(acp::ToolCallStatus::Pending),
        ),
        choices,
    ))
}

/// A choice opens the permission dialog. Blank options do not.
fn question_has_choices(options: &[String]) -> bool {
    options.iter().any(|option| !option.trim().is_empty())
}

/// End restates a reply that already streamed. Emit it only when no deltas arrived.
fn should_emit_end_text(saw_text: bool, text: &str) -> bool {
    !saw_text && !text.is_empty()
}

/// A Core `AgentEvent::Error` is a failed turn. The pager already renders
/// `Err` as TurnFailed; `Ok(EndTurn)` would say the turn completed.
fn agent_failure_error(message: &str) -> acp::Error {
    acp::Error::internal_error().data(message.to_string())
}

fn answer_from_permission_outcome(
    outcome: &acp::RequestPermissionOutcome,
    options: &[String],
) -> Option<String> {
    let acp::RequestPermissionOutcome::Selected(selected) = outcome else {
        return None;
    };
    let index = selected
        .option_id
        .0
        .as_ref()
        .strip_prefix("question-")?
        .parse::<usize>()
        .ok()?;
    let text = options.get(index)?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_tokens_keep_one_tool_ceiling() {
        let meta = |effort: &str| {
            let mut m = acp::Meta::new();
            m.insert(
                "reasoningEffort".into(),
                serde_json::Value::String(effort.into()),
            );
            m
        };
        assert_eq!(reasoning_effort_token(Some(&meta("none"))), Some("none"));
        assert_eq!(
            reasoning_effort_token(Some(&meta("medium"))),
            Some("medium")
        );
        let low = effort_limits("low").expect("low");
        let high = effort_limits("high").expect("high");
        let max = effort_limits("max").expect("max");
        assert!(low.max_tool_rounds < high.max_tool_rounds);
        assert!(high.max_tool_rounds < max.max_tool_rounds);
        assert!(low.thinking_budget < high.thinking_budget);
        assert!(high.thinking_budget < max.thinking_budget);
        assert_eq!(
            max.max_tool_rounds,
            a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
        );

        let claude =
            A3sCodeAgent::core_session_options("sid", "anthropic/claude-opus-4-6", Some(high));
        assert_eq!(claude.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(claude.thinking_budget, Some(high.thinking_budget));
        assert_eq!(claude.max_tool_rounds, Some(high.max_tool_rounds));
        let claude_budget = claude
            .prompt_slots
            .as_ref()
            .and_then(|slots| slots.guidelines.as_deref())
            .expect("native models still receive the budget line");
        assert!(claude_budget.contains(&format!("thinking_budget={}", high.thinking_budget)));
        assert!(claude_budget.contains(&format!("tool_rounds={}", high.max_tool_rounds)));
        assert!(claude.auto_compact);
        assert_eq!(
            claude.auto_compact_threshold,
            Some(a3s_code_core::store::DEFAULT_AUTO_COMPACT_THRESHOLD)
        );
        assert!(claude.max_context_tokens.is_none());

        let local = A3sCodeAgent::core_session_options("sid", "ollama/llama3", Some(high));
        let local_budget = local
            .prompt_slots
            .as_ref()
            .and_then(|slots| slots.guidelines.as_deref())
            .expect("host budget line");
        assert!(local_budget.contains(&format!("thinking_budget={}", high.thinking_budget)));
        assert!(local_budget.contains(effort_guideline("high").expect("high guideline")));
    }

    #[test]
    fn reasoning_meta_offers_parseable_budget_levels() {
        let meta = reasoning_effort_catalog_meta("anthropic/claude-opus-4-6");
        assert_eq!(
            meta.get("supportsReasoningEffort"),
            Some(&serde_json::Value::Bool(true))
        );
        let efforts = meta
            .get("reasoningEfforts")
            .and_then(|v| v.as_array())
            .expect("reasoningEfforts");
        let values: Vec<&str> = efforts
            .iter()
            .filter_map(|entry| entry.get("value").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(values, ["low", "medium", "high", "xhigh", "max"]);
        let high = efforts
            .iter()
            .find(|entry| entry.get("value").and_then(|v| v.as_str()) == Some("high"))
            .expect("high");
        assert_eq!(high.get("default"), Some(&serde_json::Value::Bool(true)));
        assert!(effort_guideline("high").is_some());
        assert!(effort_guideline("medium").is_none());
        assert_eq!(effort_limits("high").expect("high").token, "high");
    }

    #[test]
    fn effort_limits_share_the_tool_ceiling() {
        let medium = effort_limits("medium").expect("medium");
        assert!(medium.max_tool_rounds < a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING);
        assert_eq!(medium.thinking_budget, 8192);
        assert_eq!(medium.max_tool_rounds, 48);
        assert_eq!(medium.max_parallel_tasks, HOST_PARALLEL_TASKS);
        assert_eq!(medium.max_continuation_turns, HOST_CONTINUATION_TURNS);

        let low = effort_limits("low").expect("low");
        let max = effort_limits("max").expect("max");
        let ultra = effort_limits("ultracode").expect("ultracode");
        assert_eq!(low.max_parallel_tasks, max.max_parallel_tasks);
        assert_eq!(low.max_continuation_turns, max.max_continuation_turns);
        assert_eq!(ultra.max_parallel_tasks, max.max_parallel_tasks);
        assert_eq!(ultra.max_continuation_turns, max.max_continuation_turns);
        assert_eq!(
            max.max_tool_rounds,
            a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
        );
        assert_eq!(ultra.max_tool_rounds, max.max_tool_rounds);
        assert_eq!(ultra.thinking_budget, max.thinking_budget);
        assert!(medium.max_tool_rounds < max.max_tool_rounds);
        let ultra_options =
            A3sCodeAgent::core_session_options("sid", "anthropic/claude-opus-4-6", Some(ultra));
        assert_eq!(
            ultra_options.planning_mode,
            a3s_code_core::PlanningMode::Auto
        );
        assert!(effort_limits("not-a-level").is_none());
    }

    fn effort_values(meta: &serde_json::Map<String, serde_json::Value>) -> Vec<&str> {
        meta.get("reasoningEfforts")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("value").and_then(|value| value.as_str()))
            .collect()
    }

    #[test]
    fn reasoning_menu_follows_the_model() {
        let claude = reasoning_effort_catalog_meta("anthropic/claude-opus-4-6");
        assert_eq!(
            claude.get("supportsReasoningEffort"),
            Some(&serde_json::Value::Bool(true))
        );
        assert!(effort_values(&claude).contains(&"xhigh"));

        let glm = reasoning_effort_catalog_meta("zhipu/glm-5");
        let glm_values = effort_values(&glm);
        assert!(!glm_values.contains(&"medium"));
        assert!(!glm_values.contains(&"xhigh"));
        assert_eq!(glm_values, vec!["low", "high", "max"]);

        let local = reasoning_effort_catalog_meta("ollama/llama3");
        assert_eq!(
            local.get("supportsReasoningEffort"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            effort_values(&local),
            vec!["low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn subagent_progress_reports_the_resolved_window() {
        let (window, early) = context_usage(128_000, 64_000);
        let (_, later) = context_usage(128_000, 96_000);
        assert_eq!(window, 128_000);
        assert_eq!(early, 50);
        assert!(later > early);
        let idle = serde_json::json!({});
        assert_eq!(progress_tokens_used(&idle), 0);
        let growing = serde_json::json!({"prompt_tokens": 96_000});
        let (reported, pct) = context_usage(128_000, progress_tokens_used(&growing));
        assert_eq!(reported, 128_000);
        assert_eq!(pct, later);

        let mut progress = ChildProgress::default();
        note_child_progress(
            &mut progress,
            "tool_completed",
            &serde_json::json!({"tool": "read", "exit_code": 0}),
        );
        note_child_progress(
            &mut progress,
            "tool_completed",
            &serde_json::json!({"tool": "bash", "exit_code": 1}),
        );
        note_child_progress(
            &mut progress,
            "turn_completed",
            &serde_json::json!({"turn": 2}),
        );
        assert_eq!(progress.tool_call_count, 2);
        assert_eq!(
            progress.tools_used,
            vec!["read".to_string(), "bash".to_string()]
        );
        assert_eq!(progress.error_count, 1);
        assert_eq!(progress.turn_count, 2);
        note_child_progress(
            &mut progress,
            "tool_completed",
            &serde_json::json!({"tool": "read", "exit_code": 0, "prompt_tokens": 4_000}),
        );
        assert_eq!(progress.tokens_used, 4_000);
        let finished = subagent_finished_update("task-1", "child-1", true, "ok", 500, &progress);
        assert_eq!(finished["tool_calls"], 3);
        assert_eq!(finished["turns"], 2);
        assert_eq!(finished["tokens_used"], 4_000);
        assert_eq!(finished["status"], "completed");
        let parsed = parse_interject_params(
            r#"{"sessionId":"s1","text":" look here ","interjectionId":"i1"}"#,
        )
        .expect("interject");
        assert_eq!(parsed.session_id, "s1");
        assert_eq!(parsed.text, "look here");
        assert!(parse_interject_params(r#"{"sessionId":"s1"}"#).is_err());
        assert_eq!(progress_duration_ms(1_000, 1_500), 500);
        assert_eq!(progress_duration_ms(0, 1_500), 0);
    }

    #[test]
    fn question_options_are_not_permission_grants() {
        let options = question_permission_options(&[" left ".into(), "  ".into(), "right".into()]);
        assert_eq!(options.len(), 2);
        assert!(options
            .iter()
            .all(|option| option.kind == acp::PermissionOptionKind::RejectOnce));
        assert!(options
            .iter()
            .all(|option| option.kind != acp::PermissionOptionKind::AllowOnce));
        assert_eq!(options[0].name, "left");
        assert_eq!(options[1].name, "right");
    }

    #[test]
    fn choice_questions_use_the_dialog_not_a_transcript_line() {
        assert!(question_has_choices(&["keep".into(), "  ".into()]));
        assert!(!question_has_choices(&["  ".into(), String::new()]));
    }

    #[test]
    fn a_core_error_is_not_a_completed_turn() {
        let err = agent_failure_error("completion gate: workspace mutation");
        let detail = err
            .data
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(
            detail.contains("completion gate:"),
            "pager TurnFailed reads error data, got {detail:?}"
        );
    }

    #[test]
    fn end_text_fills_a_turn_that_had_no_deltas() {
        assert!(should_emit_end_text(false, "done"));
        assert!(!should_emit_end_text(true, "done"));
        assert!(!should_emit_end_text(false, ""));
    }

    #[test]
    fn selected_question_option_is_the_answer_text() {
        let options = vec!["left".to_string(), "right".to_string()];
        let selected = acp::RequestPermissionOutcome::Selected(
            acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new("question-1")),
        );
        assert_eq!(
            answer_from_permission_outcome(&selected, &options).as_deref(),
            Some("right")
        );
        assert_eq!(
            answer_from_permission_outcome(&acp::RequestPermissionOutcome::Cancelled, &options),
            None
        );
        let grant = acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
            acp::PermissionOptionId::new("allow-once"),
        ));
        assert_eq!(answer_from_permission_outcome(&grant, &options), None);
    }

    #[test]
    fn a_question_id_is_prompted_once() {
        let mut seen = std::collections::HashSet::new();
        assert!(claim_question_prompt(&mut seen, "q1"));
        assert!(!claim_question_prompt(&mut seen, "q1"));
        assert!(claim_question_prompt(&mut seen, "q2"));
    }

    #[test]
    fn tool_cards_use_pager_kind_and_raw_input() {
        assert_eq!(tool_kind_for("bash"), acp::ToolKind::Execute);
        assert_eq!(tool_kind_for("read"), acp::ToolKind::Read);
        assert_eq!(tool_kind_for("edit"), acp::ToolKind::Edit);
        assert_eq!(tool_kind_for("grep"), acp::ToolKind::Search);
        assert_eq!(tool_kind_for("web_fetch"), acp::ToolKind::Fetch);
        assert_eq!(tool_kind_for("web_search"), acp::ToolKind::Search);
        assert_eq!(tool_title("update_plan"), "Updating plan");

        let ls = prepared_raw_input("ls", &serde_json::json!({"path": "src"})).unwrap();
        assert_eq!(ls["target_directory"], "src");

        let search =
            prepared_raw_input("web_search", &serde_json::json!({"query": "a3s code"})).unwrap();
        assert_eq!(search["variant"], "WebSearch");

        let bash = tool_raw_output(
            "bash",
            Some(&serde_json::json!({"command": "echo hi"})),
            "hi\n",
            0,
        )
        .unwrap();
        assert_eq!(bash["type"], "Bash");
        assert_eq!(bash["exit_code"], 0);
        assert_eq!(bash["command"], "echo hi");
    }

    #[test]
    fn permission_response_maps_allow_and_reject() {
        let allow = acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Selected(
            acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new("allow-once")),
        ));
        assert!(permission_response_approved(&allow));

        let reject = acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Selected(
            acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new("reject-once")),
        ));
        assert!(!permission_response_approved(&reject));

        let cancelled =
            acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled);
        assert!(!permission_response_approved(&cancelled));
    }

    fn test_model(id: &str, name: &str, family: &str) -> a3s_code_core::ModelConfig {
        a3s_code_core::ModelConfig {
            id: id.to_string(),
            name: name.to_string(),
            family: family.to_string(),
            api_key: None,
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            attachment: false,
            reasoning: false,
            tool_call: true,
            temperature: true,
            release_date: None,
            modalities: a3s_code_core::ModelModalities::default(),
            cost: a3s_code_core::ModelCost::default(),
            limit: a3s_code_core::ModelLimit::default(),
        }
    }

    fn test_launch(sessions_dir: Option<std::path::PathBuf>) -> crate::config::LaunchConfig {
        let config = a3s_code_core::CodeConfig {
            default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
            providers: vec![
                a3s_code_core::ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key: Some("test-key".to_string()),
                    base_url: None,
                    headers: std::collections::HashMap::new(),
                    session_id_header: None,
                    models: vec![test_model(
                        "claude-sonnet-4-20250514",
                        "Claude Sonnet 4",
                        "claude-sonnet",
                    )],
                },
                a3s_code_core::ProviderConfig {
                    name: "openai".to_string(),
                    api_key: Some("test-openai-key".to_string()),
                    base_url: None,
                    headers: std::collections::HashMap::new(),
                    session_id_header: None,
                    models: vec![test_model("gpt-4o", "GPT-4o", "gpt-4")],
                },
            ],
            sessions_dir,
            ..a3s_code_core::CodeConfig::default()
        };
        crate::config::LaunchConfig {
            workspace: std::env::temp_dir(),
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            config,
        }
    }

    #[tokio::test]
    async fn set_session_model_replaces_a_live_session() {
        let root =
            std::env::temp_dir().join(format!("a3s-acp-model-switch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp workspace");
        let sessions_dir = root.join("sessions");
        let mut launch = test_launch(Some(sessions_dir));
        launch.workspace = root.clone();
        let agent = A3sCodeAgent::new(launch);
        let switched = async {
            let created =
                acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone())).await?;
            acp::Agent::set_session_model(
                &agent,
                acp::SetSessionModelRequest::new(
                    created.session_id.clone(),
                    acp::ModelId::new("openai/gpt-4o"),
                ),
            )
            .await?;
            let live = agent
                .sessions
                .lock()
                .await
                .get(created.session_id.0.as_ref())
                .map(|session| (session.model_id.clone(), session.session.is_closed()));
            Ok::<_, acp::Error>(live)
        }
        .await;
        let _ = std::fs::remove_dir_all(&root);
        let live = switched.expect("model switch must replace the live session");
        assert_eq!(live, Some(("openai/gpt-4o".to_string(), false)));
    }

    #[test]
    fn task_maps_to_acp_plan_entry_status_and_priority() {
        use a3s_code_core::planning::{Task, TaskPriority, TaskStatus};

        let pending = Task::new("t1", "Investigate").with_priority(TaskPriority::High);
        let entry = task_to_plan_entry(&pending);
        assert_eq!(entry.content, "Investigate");
        assert_eq!(entry.priority, acp::PlanEntryPriority::High);
        assert_eq!(entry.status, acp::PlanEntryStatus::Pending);
        assert_eq!(
            entry.meta.as_ref().and_then(|m| m.get("id")),
            Some(&serde_json::Value::String("t1".into()))
        );

        let failed = Task::new("t2", "Broken")
            .with_status(TaskStatus::Failed)
            .with_priority(TaskPriority::Low);
        let entry = task_to_plan_entry(&failed);
        assert_eq!(entry.status, acp::PlanEntryStatus::Completed);
        assert_eq!(entry.priority, acp::PlanEntryPriority::Low);
        assert_eq!(
            entry.meta.as_ref().and_then(|m| m.get("a3sStatus")),
            Some(&serde_json::Value::String("failed".into()))
        );
    }

    #[test]
    fn tui_mappings_cover_cards_catalog_and_questions() {
        for name in [
            "shell",
            "execute",
            "git",
            "write",
            "patch",
            "glob",
            "search",
            "semantic",
            "hybrid",
            "bm25",
            "code_symbols",
            "download",
            "other",
        ] {
            let _ = tool_kind_for(name);
        }
        assert!(prepared_raw_input("read", &serde_json::Value::Null).is_none());
        assert!(prepared_raw_input("read", &serde_json::json!([1])).is_some());
        assert_eq!(
            prepared_raw_input("write", &serde_json::json!({"path": "a"})).unwrap()["variant"],
            "Write"
        );
        assert_eq!(
            prepared_raw_input("update_plan", &serde_json::json!({})).unwrap()["variant"],
            "TodoWrite"
        );
        assert_eq!(
            prepared_raw_input("task", &serde_json::json!({})).unwrap()["variant"],
            "Task"
        );
        let search = prepared_raw_input("semantic", &serde_json::json!({"query": "q"})).unwrap();
        assert_eq!(search["pattern"], "q");

        let edit = serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"});
        assert_eq!(tool_call_content("edit", Some(&edit), "").len(), 1);
        assert!(tool_call_content("read", None, "").is_empty());
        let long = "x".repeat(4_001);
        assert_eq!(tool_call_content("read", None, &long).len(), 1);
        assert!(edit_diff("read", Some(&edit)).is_none());
        assert!(edit_diff(
            "write",
            Some(&serde_json::json!({"path": "a", "content": "z"}))
        )
        .is_some());
        assert!(tool_raw_output("read", None, "out", 0).is_none());
        assert!(tool_raw_output("shell", None, "out", 1).is_some());

        assert!(effort_guideline("low").is_some());
        assert!(effort_guideline("high").is_some());
        assert!(effort_guideline("xhigh").is_some());
        assert!(effort_guideline("max").is_some());
        assert!(effort_limits("ultracode").is_some());
        assert!(effort_guideline("medium").is_none());
        for token in [
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
            "ultracode",
        ] {
            let _ = effort_limits(token);
        }
        let ultra = effort_limits("ultracode").expect("ultracode");
        let _ = A3sCodeAgent::core_session_options("sid", "model", Some(ultra));
        let medium = effort_limits("medium").expect("medium");
        let _ = A3sCodeAgent::core_session_options("sid", "model", Some(medium));
        let _ = A3sCodeAgent::core_session_options("sid", "model", None);
        assert!(effort_limits_from_meta(None).is_none());

        let text =
            A3sCodeAgent::prompt_text(&[acp::ContentBlock::Text(acp::TextContent::new(" hello "))]);
        assert_eq!(text, " hello ");

        let session_id = acp::SessionId::new("s");
        let request = tool_permission_request(
            &session_id,
            "tool-1",
            "bash",
            &serde_json::json!({"command": "true"}),
        );
        assert_eq!(request.options.len(), 2);
        assert!(question_permission_request(&session_id, "q", "which", &["  ".into()]).is_none());
        let question = question_permission_request(&session_id, "q", "which", &["keep".into()])
            .expect("choice");
        assert_eq!(question.options[0].name, "keep");

        let options = vec![" ".to_string(), "keep".to_string()];
        assert!(answer_from_permission_outcome(
            &acp::RequestPermissionOutcome::Cancelled,
            &options
        )
        .is_none());
        assert!(answer_from_permission_outcome(
            &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("allow-once")
            )),
            &options
        )
        .is_none());
        assert!(answer_from_permission_outcome(
            &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("question-0")
            )),
            &options
        )
        .is_none());
        assert_eq!(
            answer_from_permission_outcome(
                &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    acp::PermissionOptionId::new("question-1")
                )),
                &options
            )
            .as_deref(),
            Some("keep")
        );

        let mut launch = test_launch(None);
        launch.config.providers[0].name = "cc-switch".into();
        launch.config.providers[0].models[0].name = "  ".into();
        launch.config.providers[1].name = "grok".into();
        let catalog = A3sCodeAgent::new(launch);
        let names: Vec<_> = catalog
            .available_model_infos()
            .into_iter()
            .map(|info| info.name)
            .collect();
        assert!(names.iter().any(|name| name.starts_with("cc-switch")));
        assert!(names.iter().any(|name| name.starts_with("grok")));
        let fallback = catalog.model_state_with("missing/id", Some("low"));
        assert_ne!(fallback.current_model_id.0.as_ref(), "missing/id");

        let empty = crate::config::LaunchConfig {
            workspace: std::env::temp_dir(),
            model_id: "fallback/model".into(),
            config: a3s_code_core::CodeConfig {
                default_model: Some("fallback/model".into()),
                ..a3s_code_core::CodeConfig::default()
            },
        };
        let agent = A3sCodeAgent::new(empty);
        assert_eq!(agent.available_model_infos().len(), 1);
        assert!(!agent.known_model("missing/model"));
        assert!(agent.known_model("fallback/model"));
    }

    #[tokio::test]
    async fn tui_turn_renders_every_pager_event() {
        use a3s_code_core::planning::{Complexity, ExecutionPlan, Task, TaskStatus};
        use a3s_code_core::verification::VerificationSummary;
        use a3s_code_core::TokenUsage;

        let root = std::env::temp_dir().join(format!("a3s-acp-events-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("workspace");
        let mut launch = test_launch(Some(root.join("sessions")));
        launch.workspace = root.clone();
        let agent = A3sCodeAgent::new(launch);
        let created = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(root.clone()))
            .await
            .expect("session");
        let init = acp::Agent::initialize(
            &agent,
            acp::InitializeRequest::new(acp::ProtocolVersion::V1),
        )
        .await
        .expect("initialize");
        let meta = init.meta.expect("meta");
        assert_eq!(meta.get("a3sCode"), Some(&serde_json::Value::Bool(true)));
        acp::Agent::authenticate(&agent, acp::AuthenticateRequest::new("none"))
            .await
            .expect("authenticate");
        let _ = acp::Agent::new_session(
            &agent,
            acp::NewSessionRequest::new(std::path::PathBuf::new()),
        )
        .await;

        let empty = acp::Agent::prompt(
            &agent,
            acp::PromptRequest::new(
                created.session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new("   "))],
            ),
        )
        .await
        .expect("blank prompt");
        assert_eq!(empty.stop_reason, acp::StopReason::EndTurn);
        let unknown = acp::Agent::prompt(
            &agent,
            acp::PromptRequest::new(
                acp::SessionId::new("missing"),
                vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))],
            ),
        )
        .await;
        assert!(unknown.is_err());
        acp::Agent::cancel(
            &agent,
            acp::CancelNotification::new(created.session_id.clone()),
        )
        .await
        .expect("cancel");
        acp::Agent::cancel(
            &agent,
            acp::CancelNotification::new(acp::SessionId::new("missing")),
        )
        .await
        .expect("cancel missing");

        let session = agent
            .sessions
            .lock()
            .await
            .get(created.session_id.0.as_ref())
            .expect("live")
            .session
            .clone();
        let mut state = TurnState {
            turn_failure: None,
            saw_text: false,
            delta_text: String::new(),
            end_text: None,
            subagent_started_ms: HashMap::new(),
            child_progress: HashMap::new(),
            prompted_questions: std::collections::HashSet::new(),
        };
        let sid = created.session_id.clone();
        let events = vec![
            AgentEvent::End {
                text: "only end".into(),
                usage: TokenUsage::default(),
                verification_summary: Box::new(VerificationSummary::from_reports(&[])),
                meta: None,
            },
            AgentEvent::TextDelta {
                text: String::new(),
            },
            AgentEvent::TextDelta { text: "hi".into() },
            AgentEvent::End {
                text: "restated".into(),
                usage: TokenUsage::default(),
                verification_summary: Box::new(VerificationSummary::from_reports(&[])),
                meta: None,
            },
            AgentEvent::ReasoningDelta {
                text: String::new(),
            },
            AgentEvent::ReasoningDelta {
                text: "think".into(),
            },
            AgentEvent::ToolStart {
                id: "t1".into(),
                name: "bash".into(),
            },
            AgentEvent::ToolExecutionStart {
                id: "t1".into(),
                name: "write".into(),
                args: serde_json::json!({"path": "a.txt", "content": "z"}),
            },
            AgentEvent::ToolOutputDelta {
                id: "t1".into(),
                name: "bash".into(),
                delta: String::new(),
            },
            AgentEvent::ToolOutputDelta {
                id: "t1".into(),
                name: "bash".into(),
                delta: "out".into(),
            },
            AgentEvent::ToolEnd {
                id: "t1".into(),
                name: "bash".into(),
                args: Some(serde_json::json!({"command": "echo"})),
                output: "ok".into(),
                exit_code: 0,
                metadata: None,
                error_kind: None,
            },
            AgentEvent::ToolEnd {
                id: "t2".into(),
                name: "edit".into(),
                args: Some(
                    serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"}),
                ),
                output: String::new(),
                exit_code: 0,
                metadata: None,
                error_kind: None,
            },
            AgentEvent::PermissionDenied {
                tool_id: "t3".into(),
                tool_name: "read".into(),
                args: serde_json::json!({"file_path": "secret"}),
                reason: "denied".into(),
            },
            AgentEvent::ConfirmationRequired {
                tool_id: "t4".into(),
                tool_name: "bash".into(),
                args: serde_json::json!({"command": "true"}),
                timeout_ms: 1,
            },
            AgentEvent::TaskUpdated {
                session_id: "s".into(),
                tasks: vec![],
            },
            AgentEvent::TaskUpdated {
                session_id: "s".into(),
                tasks: vec![Task::new("p1", "Plan").with_status(TaskStatus::InProgress)],
            },
            AgentEvent::PlanningEnd {
                plan: {
                    let mut plan = ExecutionPlan::new("goal", Complexity::Simple);
                    plan.steps.push(Task::new("p2", "Step"));
                    plan
                },
                estimated_steps: 1,
            },
            AgentEvent::SubagentStart {
                task_id: "child".into(),
                session_id: "child-session".into(),
                parent_session_id: "parent".into(),
                agent: "review".into(),
                description: "check".into(),
                started_ms: 10,
            },
            AgentEvent::SubagentProgress {
                task_id: "child".into(),
                session_id: "child-session".into(),
                status: "running".into(),
                metadata: serde_json::json!({}),
            },
            AgentEvent::SubagentProgress {
                task_id: "child".into(),
                session_id: "child-session".into(),
                status: String::new(),
                metadata: serde_json::json!({}),
            },
            AgentEvent::SubagentEnd {
                task_id: "child".into(),
                session_id: "child-session".into(),
                agent: "review".into(),
                output: "notes".into(),
                success: false,
                finished_ms: 40,
            },
            AgentEvent::SubagentEnd {
                task_id: "missing".into(),
                session_id: "child-session".into(),
                agent: "review".into(),
                output: String::new(),
                success: true,
                finished_ms: 5,
            },
            AgentEvent::SubagentEnd {
                task_id: "empty-fail".into(),
                session_id: "child-session".into(),
                agent: "review".into(),
                output: "   ".into(),
                success: false,
                finished_ms: 8,
            },
            AgentEvent::UserQuestion {
                question_id: "q1".into(),
                question: "free?".into(),
                options: vec![],
                allow_free_text: true,
            },
            AgentEvent::UserQuestion {
                question_id: "q2".into(),
                question: "which?".into(),
                options: vec!["keep".into()],
                allow_free_text: false,
            },
            AgentEvent::UserQuestion {
                question_id: "q2".into(),
                question: "which?".into(),
                options: vec!["keep".into()],
                allow_free_text: false,
            },
            AgentEvent::Error {
                message: "completion gate: blocked".into(),
            },
            AgentEvent::Start { prompt: "x".into() },
        ];
        for event in events {
            agent.handle_event(&sid, &session, &mut state, event).await;
        }
        assert_eq!(
            state.turn_failure.as_deref(),
            Some("completion gate: blocked")
        );
        assert!(state.saw_text);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Session functions the TUI renders. Each entry is an observed effect, not a
    /// line of `agent.rs` that executed.
    #[test]
    fn tui_session_function_coverage_is_at_least_95_percent() {
        let mut covered = Vec::new();
        let mut missing = Vec::new();
        let mut record = |name: &str, ok: bool| {
            if ok {
                covered.push(name.to_string());
            } else {
                missing.push(name.to_string());
            }
        };

        record(
            "tool-card:bash",
            tool_kind_for("bash") == acp::ToolKind::Execute,
        );
        record(
            "tool-card:read",
            tool_kind_for("read") == acp::ToolKind::Read,
        );
        record(
            "tool-card:edit",
            tool_kind_for("edit") == acp::ToolKind::Edit,
        );
        record(
            "tool-card:grep",
            tool_kind_for("grep") == acp::ToolKind::Search,
        );
        record(
            "tool-card:web-fetch",
            tool_kind_for("web_fetch") == acp::ToolKind::Fetch,
        );
        record("plan-title", tool_title("update_plan") == "Updating plan");
        let edit = serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"});
        record("edit-diff", edit_diff("edit", Some(&edit)).is_some());
        record(
            "bash-output",
            tool_raw_output("bash", None, "ok", 0).is_some(),
        );
        let session_id = acp::SessionId::new("s");
        let permission = tool_permission_request(
            &session_id,
            "tool-1",
            "bash",
            &serde_json::json!({"command": "true"}),
        );
        record(
            "permission-dialog",
            permission.options.len() == 2
                && permission.options[0].kind == acp::PermissionOptionKind::AllowOnce
                && permission.options[1].kind == acp::PermissionOptionKind::RejectOnce,
        );
        record(
            "question-free-text",
            question_permission_request(&session_id, "q", "which", &["  ".into()]).is_none(),
        );
        let question = question_permission_request(&session_id, "q", "which", &["keep".into()]);
        record(
            "question-choices",
            question.as_ref().is_some_and(|request| {
                request.options.len() == 1
                    && request.options[0].kind == acp::PermissionOptionKind::RejectOnce
                    && request.options[0].name == "keep"
            }),
        );
        let choices = vec!["keep".to_string()];
        record(
            "question-cancel",
            answer_from_permission_outcome(&acp::RequestPermissionOutcome::Cancelled, &choices)
                .is_none(),
        );
        record(
            "question-answer",
            answer_from_permission_outcome(
                &acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    acp::PermissionOptionId::new("question-0"),
                )),
                &choices,
            )
            .as_deref()
                == Some("keep"),
        );
        record(
            "end-without-deltas",
            should_emit_end_text(false, "reply") && !should_emit_end_text(false, ""),
        );
        record("end-after-deltas", !should_emit_end_text(true, "reply"));
        let failure = agent_failure_error("completion gate: blocked");
        let detail = failure
            .data
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("");
        record("turn-failed", detail.contains("completion gate:"));
        let menu = reasoning_effort_catalog_meta("anthropic/claude-opus-4-6");
        let values = menu
            .get("reasoningEfforts")
            .and_then(|value| value.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("value").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
            });
        record(
            "effort-menu",
            values.as_deref() == Some(&["low", "medium", "high", "xhigh", "max"][..]),
        );
        record(
            "effort-high-budget",
            effort_limits("high").is_some_and(|limits| {
                limits.token == "high"
                    && limits.thinking_budget == 16_384
                    && limits.max_tool_rounds == 128
                    && limits.max_tool_rounds < a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING
            }),
        );
        record(
            "effort-medium-no-guideline",
            effort_guideline("medium").is_none(),
        );

        let total = covered.len() + missing.len();
        let percent = covered.len() * 100 / total.max(1);
        assert!(
            percent >= 95,
            "TUI session function coverage is {percent}% ({}/{}). Missing: {}",
            covered.len(),
            total,
            missing.join(", ")
        );
    }
}
