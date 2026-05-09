use super::{AgentEvent, AgentLoop};
use crate::prompts::AgentStyle;
use tokio::sync::mpsc;

pub(super) struct PromptMode {
    pub(super) system_prompt: String,
}

impl AgentLoop {
    /// Get the assembled system prompt from slots.
    #[allow(dead_code)]
    pub(super) fn system_prompt(&self) -> String {
        self.config.prompt_slots.build()
    }

    /// Get the assembled system prompt from slots with an explicit style.
    fn system_prompt_for_style(&self, style: AgentStyle) -> String {
        let mut slots = self.config.prompt_slots.clone();
        slots.style = Some(style);
        slots.build()
    }

    async fn resolve_effective_style(&self, prompt: &str) -> AgentStyle {
        if let Some(style) = self.config.prompt_slots.style {
            return style;
        }

        let (style, confidence) = AgentStyle::detect_with_confidence(prompt);
        tracing::debug!(
            intent.classification = ?style,
            intent.confidence = ?confidence,
            intent.source = "local",
            "Intent classified locally"
        );
        style
    }

    pub(super) async fn resolve_prompt_mode(
        &self,
        explicit_style: Option<AgentStyle>,
        style_prompt: &str,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> PromptMode {
        let style = match explicit_style {
            Some(style) => style,
            None => self.resolve_effective_style(style_prompt).await,
        };
        let system_prompt = self.system_prompt_for_style(style);
        self.emit_agent_mode_changed(style, event_tx).await;

        PromptMode { system_prompt }
    }

    async fn emit_agent_mode_changed(
        &self,
        style: AgentStyle,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        if let Some(tx) = event_tx {
            tx.send(AgentEvent::AgentModeChanged {
                mode: style.runtime_mode().to_string(),
                agent: style.builtin_agent_name().to_string(),
                description: style.description().to_string(),
            })
            .await
            .ok();
        }
    }
}
