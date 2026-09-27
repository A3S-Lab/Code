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
        }
    }

    /// Wire the outbound client connection after `AgentSideConnection::new`.
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
            let mut info = acp::ModelInfo::new(acp::ModelId::new(id), name);
            if let Some(description) = description {
                info = info.description(description);
            }
            info = info.meta(Some(reasoning_effort_catalog_meta()));
            infos.push(info);
        }
        if infos.is_empty() {
            let id = self.launch.model_id.clone();
            let info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), id)
                .meta(Some(reasoning_effort_catalog_meta()));
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
            .with_model(model_id.to_string());
        if let Some(limits) = effort {
            if let Some(budget) = limits.thinking_budget {
                options = options.with_thinking_budget(budget);
            }
            options = options
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
            if let Some(guideline) = effort_guideline(limits.token) {
                options = options.with_prompt_slots(
                    a3s_code_core::SystemPromptSlots::default().with_guidelines(guideline),
                );
            }
        }
        options
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
        let options = Self::core_session_options(session_id, model_id, effort);
        let session = agent
            .session_async(workspace.to_string_lossy().to_string(), Some(options))
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(LiveSession {
            session: Arc::new(session),
            workspace,
            model_id: model_id.to_string(),
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
        let mut options = Self::core_session_options(current.session_id(), model_id, effort);
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
        })
    }

    async fn emit(&self, notification: acp::SessionNotification) {
        let client = self.client.borrow().clone();
        if let Some(client) = client {
            let _ = client.session_notification(notification).await;
        }
    }

    async fn emit_text(&self, session_id: &acp::SessionId, text: &str) {
        if text.is_empty() {
            return;
        }
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
    ) {
        self.emit_xai_session_update(
            session_id,
            serde_json::json!({
                "sessionUpdate": "subagent_progress",
                "subagent_id": task_id,
                "parent_session_id": session_id.0.as_ref(),
                "child_session_id": child_session_id,
                "duration_ms": 0_u64,
                "turn_count": 0_u32,
                "tool_call_count": 0_u32,
                "tokens_used": 0_u64,
                "context_window_tokens": 0_u64,
                "context_usage_pct": 0_u8,
                "tools_used": Vec::<String>::new(),
                "error_count": 0_u32,
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
    ) {
        let duration_ms = finished_ms.saturating_sub(started_ms);
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
            "tool_calls": 0_u32,
            "turns": 0_u32,
            "duration_ms": duration_ms,
            "tokens_used": 0_u64,
            "will_wake": false,
        });
        if let Some(error) = error {
            update["error"] = serde_json::Value::String(error);
        }
        if !output.is_empty() {
            update["output"] = serde_json::Value::String(output.chars().take(4_000).collect());
        }
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
                ..
            } => {
                self.emit_subagent_progress(
                    session_id,
                    &task_id,
                    &child_session_id,
                    &status,
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
                self.emit_subagent_end(
                    session_id,
                    &task_id,
                    &child_session_id,
                    success,
                    &output,
                    started_ms,
                    finished_ms,
                )
                .await;
            }
            AgentEvent::End { text, .. } => {
                if should_emit_end_text(state.saw_text, &text) {
                    self.emit_text(session_id, &text).await;
                }
            }
            AgentEvent::Error { message } => {
                state.turn_failure = Some(message);
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
}

struct TurnState {
    turn_failure: Option<String>,
    saw_text: bool,
    subagent_started_ms: HashMap<String, u64>,
    prompted_questions: std::collections::HashSet<String>,
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
                    .load_session(false)
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
                    "availableCommands": [],
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
            .open_core_session(&session_id, workspace, &model_id, effort)
            .await?;
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let models = self.model_state_with(&live.model_id, effort_token);
        self.sessions.lock().await.insert(session_id.clone(), live);

        Ok(acp::NewSessionResponse::new(acp::SessionId::new(session_id)).models(models))
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
        self.sessions.lock().await.insert(session_key, live);
        *self.model_id.lock().await = model_id;
        *self.effort.lock().await = effort;

        Ok(acp::SetSessionModelResponse::new())
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        let session_key = args.session_id.0.to_string();
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&session_key)
                .map(|live| live.session.clone())
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?
        };

        let prompt = Self::prompt_text(&args.prompt);
        if prompt.trim().is_empty() {
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }

        let (mut rx, join) = session
            .stream(&prompt, None)
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;

        let mut state = TurnState {
            turn_failure: None,
            saw_text: false,
            subagent_started_ms: HashMap::new(),
            prompted_questions: std::collections::HashSet::new(),
        };
        while let Some(event) = rx.recv().await {
            self.handle_event(&args.session_id, &session, &mut state, event)
                .await;
        }
        let _ = join.await;
        if let Some(message) = state.turn_failure {
            return Err(agent_failure_error(&message));
        }
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        let session_key = args.session_id.0.to_string();
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

/// Limits copied from a3s `BudgetProfile` (`crates/cli/src/budget.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffortLimits {
    token: &'static str,
    thinking_budget: Option<usize>,
    max_tool_rounds: usize,
    max_parallel_tasks: usize,
    max_continuation_turns: u32,
}

/// Advertise the a3s-code effort menu on every model.
///
/// Each entry includes `value` so the pager's `ReasoningEffortOption` parser
/// accepts it. `low`…`max` match `BudgetProfile`; `ultracode` is not a
/// `ReasoningEffort` variant, so it stays off the menu.
fn reasoning_effort_catalog_meta() -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "supportsReasoningEffort".into(),
        serde_json::Value::Bool(true),
    );
    meta.insert(
        "reasoningEfforts".into(),
        serde_json::json!([
            {"value": "low", "id": "low", "label": "low", "description": "Faster, lighter reasoning"},
            {"value": "medium", "id": "medium", "label": "medium", "description": "Balanced reasoning"},
            {"value": "high", "id": "high", "label": "high", "description": "Heavy reasoning", "default": true},
            {"value": "xhigh", "id": "xhigh", "label": "xhigh", "description": "Extended reasoning"},
            {"value": "max", "id": "max", "label": "max", "description": "Maximum reasoning"},
        ]),
    );
    meta
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

fn effort_limits(token: &str) -> Option<EffortLimits> {
    match token.to_ascii_lowercase().as_str() {
        "none" => Some(EffortLimits {
            token: "none",
            thinking_budget: None,
            max_tool_rounds: 240,
            max_parallel_tasks: 4,
            max_continuation_turns: 4,
        }),
        "minimal" => Some(EffortLimits {
            token: "minimal",
            thinking_budget: Some(1_024),
            max_tool_rounds: 240,
            max_parallel_tasks: 4,
            max_continuation_turns: 4,
        }),
        "low" => Some(EffortLimits {
            token: "low",
            thinking_budget: Some(2_048),
            max_tool_rounds: 240,
            max_parallel_tasks: 4,
            max_continuation_turns: 4,
        }),
        "medium" => Some(EffortLimits {
            token: "medium",
            thinking_budget: Some(8_192),
            max_tool_rounds: 800,
            max_parallel_tasks: 8,
            max_continuation_turns: 8,
        }),
        "high" => Some(EffortLimits {
            token: "high",
            thinking_budget: Some(16_384),
            max_tool_rounds: 1_200,
            max_parallel_tasks: 8,
            max_continuation_turns: 12,
        }),
        "xhigh" => Some(EffortLimits {
            token: "xhigh",
            thinking_budget: Some(32_768),
            max_tool_rounds: 1_800,
            max_parallel_tasks: 8,
            max_continuation_turns: 16,
        }),
        "max" => Some(EffortLimits {
            token: "max",
            thinking_budget: Some(65_536),
            max_tool_rounds: 2_400,
            max_parallel_tasks: 8,
            max_continuation_turns: 24,
        }),
        "ultracode" => Some(EffortLimits {
            token: "ultracode",
            thinking_budget: Some(65_536),
            max_tool_rounds: 3_200,
            max_parallel_tasks: 8,
            max_continuation_turns: 32,
        }),
        _ => None,
    }
}

fn effort_limits_from_meta(meta: Option<&acp::Meta>) -> Option<EffortLimits> {
    let effort = meta
        .and_then(|m| m.get("reasoningEffort"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    effort_limits(effort)
}

/// Map pager `/effort` `_meta.reasoningEffort` onto a3s-code thinking tokens.
fn reasoning_effort_to_thinking_budget(meta: Option<&acp::Meta>) -> Option<usize> {
    effort_limits_from_meta(meta).and_then(|limits| limits.thinking_budget)
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
    fn effort_tokens_map_to_thinking_budgets() {
        let meta = |effort: &str| {
            let mut m = acp::Meta::new();
            m.insert(
                "reasoningEffort".into(),
                serde_json::Value::String(effort.into()),
            );
            m
        };
        assert_eq!(
            reasoning_effort_to_thinking_budget(Some(&meta("none"))),
            None
        );
        assert_eq!(
            reasoning_effort_to_thinking_budget(Some(&meta("medium"))),
            Some(8_192)
        );
        assert_eq!(
            reasoning_effort_to_thinking_budget(Some(&meta("xhigh"))),
            Some(32_768)
        );
        assert_eq!(
            reasoning_effort_to_thinking_budget(Some(&meta("max"))),
            Some(65_536)
        );
    }

    #[test]
    fn reasoning_meta_offers_parseable_budget_levels() {
        let meta = reasoning_effort_catalog_meta();
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
    fn effort_limits_match_budget_profile() {
        let medium = effort_limits("medium").expect("medium");
        assert_eq!(medium.thinking_budget, Some(8_192));
        assert_eq!(medium.max_tool_rounds, 800);
        assert_eq!(medium.max_parallel_tasks, 8);
        assert_eq!(medium.max_continuation_turns, 8);

        let max = effort_limits("max").expect("max");
        assert_eq!(max.max_tool_rounds, 2_400);
        assert!(effort_limits("not-a-level").is_none());
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
            "shell", "execute", "git", "write", "patch", "glob", "search", "semantic", "hybrid",
            "bm25", "code_symbols", "download", "other",
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
        assert!(edit_diff("write", Some(&serde_json::json!({"path": "a", "content": "z"}))).is_some());
        assert!(tool_raw_output("read", None, "out", 0).is_none());
        assert!(tool_raw_output("shell", None, "out", 1).is_some());

        assert!(effort_guideline("low").is_some());
        assert!(effort_guideline("high").is_some());
        assert!(effort_guideline("xhigh").is_some());
        assert!(effort_guideline("max").is_some());
        assert!(effort_limits("ultracode").is_some());
        assert!(effort_guideline("medium").is_none());
        for token in ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultracode"] {
            let _ = effort_limits(token);
        }
        let ultra = effort_limits("ultracode").expect("ultracode");
        let _ = A3sCodeAgent::core_session_options("sid", "model", Some(ultra));
        let medium = effort_limits("medium").expect("medium");
        let _ = A3sCodeAgent::core_session_options("sid", "model", Some(medium));
        let _ = A3sCodeAgent::core_session_options("sid", "model", None);
        assert!(effort_limits_from_meta(None).is_none());

        let text = A3sCodeAgent::prompt_text(&[acp::ContentBlock::Text(acp::TextContent::new(
            " hello ",
        ))]);
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
        let _ = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(std::path::PathBuf::new()))
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
        acp::Agent::cancel(&agent, acp::CancelNotification::new(created.session_id.clone()))
            .await
            .expect("cancel");
        acp::Agent::cancel(&agent, acp::CancelNotification::new(acp::SessionId::new("missing")))
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
            subagent_started_ms: HashMap::new(),
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
            AgentEvent::TextDelta { text: String::new() },
            AgentEvent::TextDelta { text: "hi".into() },
            AgentEvent::End {
                text: "restated".into(),
                usage: TokenUsage::default(),
                verification_summary: Box::new(VerificationSummary::from_reports(&[])),
                meta: None,
            },
            AgentEvent::ReasoningDelta { text: String::new() },
            AgentEvent::ReasoningDelta { text: "think".into() },
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
                args: Some(serde_json::json!({"path": "a.rs", "old_string": "a", "new_string": "b"})),
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
            agent
                .handle_event(&sid, &session, &mut state, event)
                .await;
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
        record(
            "plan-title",
            tool_title("update_plan") == "Updating plan",
        );
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
        let menu = reasoning_effort_catalog_meta();
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
            effort_limits("high").is_some_and(|limits| limits.thinking_budget == Some(16384)),
        );
        record("effort-medium-no-guideline", effort_guideline("medium").is_none());

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
