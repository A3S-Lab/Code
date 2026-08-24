use super::*;

impl AgentSession {
    /// Install or replace a runtime budget guard. Takes effect on the
    /// next `send` / `stream` call (the guard is consulted at agent-
    /// loop build time, not on the live execution). Setting `None`
    /// clears the override so `config.budget_guard` takes over again.
    ///
    /// This is the entry point SDKs use to wire a host-supplied guard
    /// after the session has already been constructed — useful when
    /// the guard's transport (e.g. a JS callable) cannot live inside
    /// the value-typed `SessionOptions`.
    pub fn set_budget_guard(
        &self,
        guard: Option<Arc<dyn crate::budget::BudgetGuard>>,
    ) -> crate::error::Result<()> {
        self.close_handle.mutate_immediate(|| {
            let mut slot = self
                .runtime_budget_guard
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            *slot = guard;
            drop(slot);
            // Delegated children own a pre-built TaskExecutor. Refresh its parent
            // context so the next run cannot bypass a runtime-installed ledger.
            // SkillTool likewise owns a child AgentConfig captured at registration
            // time, so it must be refreshed from the same runtime source of truth.
            self.refresh_task_delegation_tools();
            self.refresh_skill_tools();
        })
    }

    /// Return the currently-installed runtime budget guard, if any.
    /// `None` means the loop falls back to `config.budget_guard`.
    pub fn budget_guard(&self) -> Option<Arc<dyn crate::budget::BudgetGuard>> {
        self.runtime_budget_guard
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Return pending HITL tool confirmations for this session.
    pub async fn pending_confirmations(&self) -> Vec<PendingConfirmationInfo> {
        HitlControl::from_session(self)
            .pending_confirmations()
            .await
    }

    /// Resolve a pending HITL tool confirmation.
    ///
    /// Returns `Ok(true)` when a pending confirmation was found and completed,
    /// `Ok(false)` when the tool ID is not pending or HITL is not configured.
    pub async fn confirm_tool_use(
        &self,
        tool_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<bool> {
        HitlControl::from_session(self)
            .confirm_tool_use(tool_id, approved, reason)
            .await
    }

    /// Cancel all pending HITL confirmations for this session.
    pub async fn cancel_confirmations(&self) -> usize {
        HitlControl::from_session(self).cancel_confirmations().await
    }

    /// Return structured verification reports recorded for this session.
    pub fn verification_reports(&self) -> Vec<crate::verification::VerificationReport> {
        VerificationRuntime::from_session(self).reports()
    }

    /// Return a structured summary of all verification reports recorded for this session.
    pub fn verification_summary(&self) -> crate::verification::VerificationSummary {
        VerificationRuntime::from_session(self).summary()
    }

    /// Return a concise human-readable verification summary for this session.
    pub fn verification_summary_text(&self) -> String {
        VerificationRuntime::from_session(self).summary_text()
    }

    /// Add externally produced verification reports to this session's completion evidence.
    pub fn record_verification_reports(
        &self,
        reports: impl IntoIterator<Item = crate::verification::VerificationReport>,
    ) {
        VerificationRuntime::from_session(self).record(reports);
    }

    /// Register a hook for lifecycle event interception.
    pub fn register_hook(&self, hook: crate::hooks::Hook) -> crate::error::Result<()> {
        self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Hook,
                &hook.id,
            )?;
            HookControl::from_session(self).register_hook(hook);
            Ok(())
        })?
    }

    /// Unregister a hook by ID.
    pub fn unregister_hook(
        &self,
        hook_id: &str,
    ) -> crate::error::Result<Option<crate::hooks::Hook>> {
        self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Hook,
                hook_id,
            )?;
            Ok(HookControl::from_session(self).unregister_hook(hook_id))
        })?
    }

    /// Register a handler for a specific hook.
    pub fn register_hook_handler(
        &self,
        hook_id: &str,
        handler: Arc<dyn crate::hooks::HookHandler>,
    ) -> crate::error::Result<()> {
        let mut pending_handler = Some(handler);
        let retired = self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Hook,
                hook_id,
            )?;
            let handler = pending_handler.take().ok_or_else(|| {
                crate::error::CodeError::Capability(
                    crate::capability::CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: crate::capability::CapabilityKind::Hook,
                        public_name: hook_id.to_owned(),
                        message: "Hook handler mutation was already consumed".to_owned(),
                    },
                )
            })?;
            Ok::<_, crate::error::CodeError>(
                HookControl::from_session(self).register_hook_handler(hook_id, handler),
            )
        })??;
        drop(retired);
        Ok(())
    }

    /// Unregister a hook handler by hook ID.
    pub fn unregister_hook_handler(&self, hook_id: &str) -> crate::error::Result<()> {
        let retired = self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Hook,
                hook_id,
            )?;
            Ok::<_, crate::error::CodeError>(
                HookControl::from_session(self).unregister_hook_handler(hook_id),
            )
        })??;
        drop(retired);
        Ok(())
    }

    /// Atomically add or replace a complete Hook definition and handler pair.
    ///
    /// Official SDK bridges use this method so a newly admitted Run cannot
    /// observe the definition without its matching callback. Passing `None`
    /// intentionally registers an event-only Hook with no gating handler.
    pub fn register_hook_registration(
        &self,
        hook: crate::hooks::Hook,
        handler: Option<Arc<dyn crate::hooks::HookHandler>>,
    ) -> crate::error::Result<()> {
        let mut pending_handler = Some(handler);
        let retired = self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Hook,
                &hook.id,
            )?;
            let handler = pending_handler.take().ok_or_else(|| {
                crate::error::CodeError::Capability(
                    crate::capability::CapabilityRuntimeError::RuntimeValueInvalid {
                        kind: crate::capability::CapabilityKind::Hook,
                        public_name: hook.id.clone(),
                        message: "Hook registration was already consumed".to_owned(),
                    },
                )
            })?;
            Ok::<_, crate::error::CodeError>(
                HookControl::from_session(self).register_hook_registration(hook, handler),
            )
        })??;
        drop(retired);
        Ok(())
    }

    /// Atomically remove a complete Hook definition and handler pair.
    pub fn unregister_hook_registration(
        &self,
        hook_id: &str,
    ) -> crate::error::Result<Option<crate::hooks::Hook>> {
        let (retired_hook, retired_handler) = self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Hook,
                hook_id,
            )?;
            Ok::<_, crate::error::CodeError>(
                HookControl::from_session(self).unregister_hook_registration(hook_id),
            )
        })??;
        let removed = retired_hook.as_deref().cloned();
        drop((retired_hook, retired_handler));
        Ok(removed)
    }

    /// Get the number of registered hooks.
    pub fn hook_count(&self) -> usize {
        HookControl::from_session(self).hook_count()
    }

    /// Run verification commands through the session's tool execution path.
    pub async fn verify_commands(
        &self,
        subject: &str,
        commands: &[crate::verification::VerificationCommand],
    ) -> Result<crate::verification::VerificationReport> {
        VerificationRuntime::from_session(self)
            .verify_commands(subject, commands)
            .await
    }

    /// Return project-aware verification command presets for this workspace.
    pub fn verification_presets(&self) -> Vec<crate::verification::VerificationPreset> {
        VerificationRuntime::from_session(self).presets()
    }
}
