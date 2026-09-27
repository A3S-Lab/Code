//! ACP-shaped agent adapter over a3s-code-core 9.0.0.
//!
//! The a3s-build pager speaks Agent Client Protocol to its agent process.
//! A3S keeps that seam shape in-process: the TUI calls [`CodeAgentAdapter`],
//! which drives `Agent` / fact-log `session.stream`. No A3S Code agent runtime.

use std::path::PathBuf;
use std::sync::Arc;

use a3s_code_core::llm::LlmClient;
use a3s_code_core::{Agent, AgentEvent, AgentSession, PlanningMode, SessionOptions};
use anyhow::Context;

use crate::model::{merge_launch_layers, LaunchLayers, MergedLaunch};
use crate::transcript::Scrollback;

/// Permission posture for the next turn (Shift+Tab cycle in the product TUI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermissionMode {
    #[default]
    Default,
    Plan,
    Auto,
    Yolo,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::Yolo => "yolo",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Default => Self::Plan,
            Self::Plan => Self::Auto,
            Self::Auto => Self::Yolo,
            Self::Yolo => Self::Default,
        }
    }

    fn planning_mode(self) -> PlanningMode {
        match self {
            Self::Plan => PlanningMode::Enabled,
            Self::Auto => PlanningMode::Auto,
            Self::Default | Self::Yolo => PlanningMode::Disabled,
        }
    }

    fn auto_approve_tools(self) -> bool {
        matches!(self, Self::Auto | Self::Yolo)
    }
}

/// One completed prompt turn from the adapter.
#[derive(Debug, Clone)]
pub struct AgentTurn {
    pub text: String,
    pub model_name: String,
}

/// In-process agent the full-screen TUI drives instead of an A3S Code ACP child.
pub struct CodeAgentAdapter {
    layers: LaunchLayers,
    merged: MergedLaunch,
    permission: PermissionMode,
    session_id: String,
    responder: Option<Arc<dyn LlmClient>>,
    session: Option<AgentSession>,
}

impl CodeAgentAdapter {
    /// Build from the same ACL layer stack `a3s code` merges.
    pub async fn open(layers: LaunchLayers) -> anyhow::Result<Self> {
        let merged = merge_launch_layers(&layers).map_err(anyhow::Error::msg)?;
        Ok(Self {
            layers,
            merged,
            permission: PermissionMode::Default,
            session_id: "tui-session".to_string(),
            responder: None,
            session: None,
        })
    }

    pub fn with_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.responder = Some(client);
        self
    }

    pub fn model_id(&self) -> &str {
        &self.merged.model_id
    }

    pub fn provider_names(&self) -> &[String] {
        &self.merged.provider_names
    }

    pub fn permission_mode(&self) -> PermissionMode {
        self.permission
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission = mode;
    }

    pub fn cycle_permission_mode(&mut self) -> PermissionMode {
        self.permission = self.permission.cycle();
        self.permission
    }

    pub fn workspace(&self) -> &PathBuf {
        &self.layers.workspace
    }

    /// Ensure a live [`AgentSession`] exists (lazy on first prompt).
    pub async fn ensure_session(&mut self) -> anyhow::Result<&mut AgentSession> {
        if self.session.is_none() {
            let agent = Agent::from_config(self.merged.config.clone())
                .await
                .context("Agent::from_config")?;
            let mut options = SessionOptions::new()
                .with_session_id(self.session_id.clone())
                .with_model(self.merged.model_id.clone())
                .with_planning_mode(self.permission.planning_mode())
                .with_auto_delegation_enabled(matches!(
                    self.permission,
                    PermissionMode::Auto | PermissionMode::Yolo
                ));
            if let Some(responder) = self.responder.clone() {
                options = options.with_llm_client(responder);
            }
            let session = agent
                .session_async(
                    self.layers.workspace.to_string_lossy().to_string(),
                    Some(options),
                )
                .await
                .context("session_async")?;
            self.session = Some(session);
        }
        Ok(self.session.as_mut().expect("session just inserted"))
    }

    /// Blocking collect via `session.send` (tests / simple hosts).
    pub async fn prompt(&mut self, text: &str) -> anyhow::Result<AgentTurn> {
        let session = self.ensure_session().await?;
        let model_name = session.model_name().to_string();
        let result = session.send(text, None).await.context("session.send")?;
        Ok(AgentTurn {
            text: result.text,
            model_name,
        })
    }

    /// ACP-shaped `session/prompt` with streaming into scrollback.
    ///
    /// Text deltas update the stream line. When a tool needs confirmation,
    /// `ask_confirm(tool_name)` decides approval (`true`/`false`). Callers in
    /// auto/yolo typically return `true` without prompting.
    pub async fn prompt_streaming(
        &mut self,
        text: &str,
        scrollback: &mut Scrollback,
        mut on_delta: impl FnMut(&Scrollback),
        mut ask_confirm: impl FnMut(&str) -> bool,
    ) -> anyhow::Result<AgentTurn> {
        let auto_approve = self.permission.auto_approve_tools();
        let session = self.ensure_session().await?;
        let model_name = session.model_name().to_string();
        let (mut rx, join) = session.stream(text, None).await.context("session.stream")?;

        scrollback.push_assistant("");
        let stream_line = scrollback.len().saturating_sub(1);
        let mut assembled = String::new();

        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::TextDelta { text } => {
                    assembled.push_str(&text);
                    // Show raw deltas while streaming; flatten when End arrives.
                    scrollback.set_assistant_line(stream_line, &assembled);
                    on_delta(scrollback);
                }
                AgentEvent::ToolStart { name, .. } => {
                    scrollback.push_assistant(&format!("tool: {name}…"));
                    on_delta(scrollback);
                }
                AgentEvent::ToolEnd { name, output, .. } => {
                    let preview: String = output.chars().take(200).collect();
                    scrollback.push_assistant(&format!("tool:{name} → {preview}"));
                    on_delta(scrollback);
                }
                AgentEvent::ConfirmationRequired {
                    tool_id,
                    tool_name,
                    ..
                } => {
                    let approved = if auto_approve {
                        true
                    } else {
                        scrollback.push_assistant(&format!(
                            "allow tool `{tool_name}`? [y/N]"
                        ));
                        on_delta(scrollback);
                        tokio::task::block_in_place(|| ask_confirm(&tool_name))
                    };
                    let reason = if approved {
                        Some("approved".to_string())
                    } else {
                        Some("denied by user".to_string())
                    };
                    if let Err(error) = session
                        .confirm_tool_use(&tool_id, approved, reason)
                        .await
                    {
                        scrollback.push_assistant(&format!("confirm {tool_name}: {error}"));
                        on_delta(scrollback);
                    } else if !approved {
                        scrollback.push_assistant(&format!("denied {tool_name}"));
                        on_delta(scrollback);
                    }
                }
                AgentEvent::UserQuestion {
                    question,
                    options,
                    ..
                } => {
                    scrollback.push_assistant(&format!(
                        "question: {question} · options: {}",
                        options.join(" | ")
                    ));
                    on_delta(scrollback);
                }
                AgentEvent::End { text, .. } => {
                    assembled = text;
                    let plain = crate::markdown_to_plain(&assembled);
                    scrollback.set_assistant_line(stream_line, &plain);
                    on_delta(scrollback);
                }
                AgentEvent::Error { message } => {
                    scrollback.set_assistant_line(stream_line, &message);
                    on_delta(scrollback);
                    let _ = join.await;
                    return Err(anyhow::anyhow!(message));
                }
                _ => {}
            }
        }
        let _ = join.await;
        Ok(AgentTurn {
            text: assembled,
            model_name,
        })
    }

    /// Drop the live session so the next prompt rebuilds with current permission mode.
    pub fn cancel_session(&mut self) {
        self.session = None;
    }
}
