use super::AgentEvent;
use crate::prompts::AgentStyle;
use tokio::sync::mpsc;

pub(super) struct PromptMode {
    pub(super) system_prompt: String,
}

impl crate::agent::TurnEngine {
    /// Get the assembled system prompt from slots.
    #[allow(dead_code)]
    pub(super) fn system_prompt(&self) -> String {
        if self.config.work_policy.is_some() {
            let mut slots = self.config.prompt_slots.clone();
            self.append_work_policy(&mut slots);
            return self.with_verification_context(slots.build());
        }
        self.with_verification_context(self.config.prompt_slots.build())
    }

    pub(crate) fn admitted_system_prompt(&self) -> String {
        self.system_prompt()
    }

    pub(crate) fn validate_durable_source(
        &self,
        session_id: &str,
        facts: &[a3s_effect::Fact],
    ) -> anyhow::Result<()> {
        if let Some(source) = &self.config.durable_source {
            source.verify(session_id, facts)?;
        }
        Ok(())
    }

    pub(crate) fn work_policy(&self) -> Option<&crate::WorkPolicyV1> {
        self.config.work_policy.as_ref()
    }

    pub(crate) fn with_work_policy(mut self, policy: Option<crate::WorkPolicyV1>) -> Self {
        self.tool_context.run_work_policy = policy.clone();
        self.config.work_policy = policy;
        self
    }

    fn append_work_policy(&self, slots: &mut crate::SystemPromptSlots) {
        if let Some(policy) = &self.config.work_policy {
            let guidelines = slots.guidelines.get_or_insert_with(String::new);
            if !guidelines.is_empty() {
                guidelines.push_str("\n\n");
            }
            guidelines.push_str(policy.instructions());
        }
    }

    /// Get the assembled system prompt from slots with an explicit style.
    fn system_prompt_for_style(&self, style: AgentStyle) -> String {
        let mut slots = self.config.prompt_slots.clone();
        slots.style = Some(style);
        self.append_work_policy(&mut slots);
        self.with_verification_context(slots.build())
    }

    fn with_verification_context(&self, mut system: String) -> String {
        if self.config.verification_contracts.is_empty() {
            return system;
        }
        // Project the admitted host manifest, never commands inferred from
        // repository text or the user's phrasing. This is model context, not
        // a grant of tool execution or authority to change the manifest.
        let contracts: Vec<_> = self
            .config
            .verification_contracts
            .iter()
            .map(|contract| {
                serde_json::json!({
                    "id": contract.id,
                    "command": contract.command.trim(),
                    "covers": contract.covers,
                })
            })
            .collect();
        system.push_str(
            "\n\nHost verification contracts for this workspace:\n\
             The following JSON is the admitted host manifest. When verifying covered edits, \
             use the command string as the entire bash command, with no extra arguments, \
             prefixes, suffixes or shell composition. Read-only inspection is not verification. \
             A successful checker must run after the latest edit; a failed check or a later edit \
             cannot yield Verified completion. These declarations do not authorize execution, \
             waive permission or sandbox checks, or allow changing the checker or the manifest. \
             Report a failed or unavailable check truthfully; never claim it passed.\n",
        );
        system.push_str(&serde_json::Value::Array(contracts).to_string());
        system
    }

    async fn resolve_effective_style(&self, prompt: &str) -> AgentStyle {
        if let Some(style) = self.config.prompt_slots.style {
            return style;
        }

        // Keep the primary session on GeneralPurpose unless the host explicitly
        // selected a specialty style. Keyword detection remains available for
        // planning heuristics and delegated agents, but must not silently
        // downgrade the writable coding prompt.
        let (detected, confidence) = AgentStyle::detect_with_confidence(prompt);
        tracing::debug!(
            intent.classification = ?detected,
            intent.confidence = ?confidence,
            intent.source = "local_nonbinding",
            "Intent classified locally without changing primary prompt style"
        );
        AgentStyle::GeneralPurpose
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
