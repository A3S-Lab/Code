use super::*;

impl AgentSession {
    // Advanced optional Queue API
    // ========================================================================

    /// Returns whether this session has an advanced lane queue configured.
    pub fn has_queue(&self) -> bool {
        QueueControl::from_session(self).has_queue()
    }

    /// Configure a lane's handler mode for explicit external/hybrid dispatch.
    ///
    /// Only effective when a queue is configured via `SessionOptions::with_queue_config`.
    pub async fn set_lane_handler(
        &self,
        lane: SessionLane,
        config: LaneHandlerConfig,
    ) -> crate::error::Result<()> {
        let _mutation = self.close_handle.extension_mutation.lock().await;
        if self.is_closed() {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        QueueControl::from_session(self)
            .set_lane_handler(lane, config)
            .await;
        if self.is_closed() {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        Ok(())
    }

    /// Complete an external queue task by ID.
    ///
    /// Returns `true` if the task was found and completed, `false` if not found.
    pub async fn complete_external_task(&self, task_id: &str, result: ExternalTaskResult) -> bool {
        QueueControl::from_session(self)
            .complete_external_task(task_id, result)
            .await
    }

    /// Get pending external queue tasks awaiting completion by an external handler.
    pub async fn pending_external_tasks(&self) -> Vec<ExternalTask> {
        QueueControl::from_session(self)
            .pending_external_tasks()
            .await
    }

    /// Get optional queue statistics (pending, active, external counts per lane).
    pub async fn queue_stats(&self) -> SessionQueueStats {
        QueueControl::from_session(self).stats().await
    }

    /// Get a metrics snapshot from the optional queue (if metrics are enabled).
    pub async fn queue_metrics(&self) -> Option<MetricsSnapshot> {
        QueueControl::from_session(self).metrics().await
    }

    /// Get dead letters from the optional queue's DLQ (if DLQ is enabled).
    pub async fn dead_letters(&self) -> Vec<DeadLetter> {
        QueueControl::from_session(self).dead_letters().await
    }

    // ========================================================================
    // MCP API
    // ========================================================================

    /// Register all agents found in a directory with the live session.
    ///
    /// Scans `dir` for `*.yaml`, `*.yml`, and `*.md` agent definition files,
    /// parses them, and adds each one to the shared `AgentRegistry` used by the
    /// `task` tool. New agents are usable by the next admitted Run in the same
    /// Session; an already admitted Run retains its frozen registry.
    ///
    /// Returns the number of agents successfully loaded from the directory.
    pub fn register_agent_dir(&self, dir: &std::path::Path) -> crate::error::Result<usize> {
        let agents = crate::subagent::load_agents_from_dir(dir);
        self.close_handle.mutate_immediate(|| {
            for agent in &agents {
                self.ensure_compatibility_name_available(
                    crate::capability::CapabilityKind::Agent,
                    &agent.name,
                )?;
            }
            let count = agents.len();
            for agent in agents {
                tracing::info!(
                    session_id = %self.session_id,
                    agent = agent.name,
                    dir = %dir.display(),
                    "Dynamically registered agent"
                );
                self.agent_registry.register(agent);
            }
            Ok(count)
        })?
    }

    /// Register a disposable worker agent with the live session.
    ///
    /// The returned definition enters the `task` lookup and the model-facing
    /// `task` and `parallel_task` definitions on the next admitted Run. Callers
    /// can create discoverable reproducible workers without writing temporary
    /// agent files or restarting the Session, while an active Run remains
    /// generation-stable.
    pub fn register_worker_agent(
        &self,
        spec: crate::subagent::WorkerAgentSpec,
    ) -> crate::error::Result<crate::subagent::AgentDefinition> {
        self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Agent,
                &spec.name,
            )?;
            Ok(SessionExtensionRuntime::from_session(self).register_worker_agent(spec))
        })?
    }

    /// Register multiple disposable worker agents with the live session.
    pub fn register_worker_agents<I>(
        &self,
        specs: I,
    ) -> crate::error::Result<Vec<crate::subagent::AgentDefinition>>
    where
        I: IntoIterator<Item = crate::subagent::WorkerAgentSpec>,
    {
        let specs = specs.into_iter().collect::<Vec<_>>();
        self.close_handle.mutate_immediate(|| {
            for spec in &specs {
                self.ensure_compatibility_name_available(
                    crate::capability::CapabilityKind::Agent,
                    &spec.name,
                )?;
            }
            Ok(SessionExtensionRuntime::from_session(self).register_worker_agents(specs))
        })?
    }

    /// Add or replace a skill in this live session.
    ///
    /// The Skill and `search_skills` tools observe the new definition
    /// immediately, and the model-visible skills catalog observes it on the
    /// next turn. Removing the live definition restores any session skill it
    /// shadowed at installation time.
    pub fn add_skill(&self, skill: Arc<crate::skills::Skill>) -> crate::error::Result<()> {
        self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Skill,
                &skill.name,
            )?;
            SessionExtensionRuntime::from_session(self).add_skill(skill)
        })?
    }

    /// Remove a skill previously installed through [`Self::add_skill`].
    ///
    /// This is a no-op when the name is not owned by the live session API; base
    /// session skills and later host registrations are never removed.
    pub fn remove_skill(&self, name: &str) -> crate::error::Result<()> {
        self.close_handle
            .mutate_immediate(|| SessionExtensionRuntime::from_session(self).remove_skill(name))
    }

    /// Return the names in the session's current live skill registry.
    pub fn skill_names(&self) -> Vec<String> {
        self.close_handle.skill_registry.list()
    }

    /// Add an MCP server to this session.
    ///
    /// Registers, connects, and makes all tools immediately available for the
    /// agent to call. Tool names follow the convention `mcp__<name>__<tool>`.
    ///
    /// Returns the number of tools registered from the server.
    pub async fn add_mcp_server(
        &self,
        config: crate::mcp::McpServerConfig,
    ) -> crate::error::Result<usize> {
        SessionExtensionRuntime::from_session(self)
            .add_mcp_server(config)
            .await
    }

    /// The session's tool executor, for installing agent-dir `tools/` entries
    /// (e.g. a `kind = "script"` tool) into the live registry. Internal seam used
    /// by [`serve::install_agent_dir_tools`](crate::serve::install_agent_dir_tools)
    /// (the only caller, hence the `serve` gate).
    #[cfg(feature = "serve")]
    pub(crate) fn tool_executor(&self) -> &Arc<crate::tools::ToolExecutor> {
        &self.tool_executor
    }

    /// Register a host-provided dynamic tool into the live session. Enables an
    /// embedding app (e.g. the a3s-code CLI's login-gated `runtime` A3S Runtime
    /// offload tool) to add a native tool at runtime; it enters the LLM's toolset
    /// on the next run (`build_agent_loop` re-snapshots `definitions()` per run),
    /// the same way MCP tools surface after `add_mcp_server`. Idempotent by name.
    pub fn register_dynamic_tool(
        &self,
        tool: Arc<dyn crate::tools::Tool>,
    ) -> crate::error::Result<()> {
        self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Tool,
                tool.name(),
            )?;
            self.tool_executor.register_dynamic_tool(tool);
            Ok(())
        })?
    }

    /// Register the A3S Flow-backed dynamic workflow tool for this live session.
    ///
    /// The tool is named `dynamic_workflow`. It accepts a sandboxed JavaScript
    /// PTC workflow script and executes it through
    /// [`crate::DynamicWorkflowRuntime`], so A3S Flow owns workflow replay while
    /// the script can still call A3S Code tools.
    pub fn register_dynamic_workflow_runtime(&self) -> crate::error::Result<()> {
        self.close_handle.mutate_immediate(|| {
            self.ensure_compatibility_name_available(
                crate::capability::CapabilityKind::Tool,
                "dynamic_workflow",
            )?;
            crate::tools::register_dynamic_workflow(self.tool_executor.registry());
            Ok(())
        })?
    }

    /// Remove a previously host-registered dynamic tool by name (e.g. on logout).
    /// No-op if no tool of that name is registered.
    pub fn unregister_dynamic_tool(&self, name: &str) -> crate::error::Result<()> {
        self.close_handle
            .mutate_immediate(|| self.tool_executor.unregister_dynamic_tool(name))
    }

    /// Remove an MCP server from this session.
    ///
    /// Disconnects the server and unregisters all its tools from the executor.
    /// No-op if the server was never added.
    pub async fn remove_mcp_server(&self, server_name: &str) -> crate::error::Result<()> {
        SessionExtensionRuntime::from_session(self)
            .remove_mcp_server(server_name)
            .await
    }

    /// Return current projected and compatibility MCP server status.
    pub async fn mcp_status(
        &self,
    ) -> std::collections::HashMap<String, crate::mcp::McpServerStatus> {
        SessionExtensionRuntime::from_session(self)
            .mcp_status()
            .await
    }

    /// Return the exact generation and digest currently visible to new Runs.
    pub fn capability_catalog_stamp(&self) -> crate::capability::CapabilityCatalogStamp {
        self.capability_catalog.current_stamp()
    }

    /// Prepare and atomically publish one complete host capability generation.
    ///
    /// Preparation may perform asynchronous work, but no value becomes visible
    /// until every adapter has succeeded and the complete projection wins its
    /// generation-and-digest compare-and-swap. A Use-backed batch publishes its
    /// generation-specific lease provider in that same commit.
    pub async fn apply_capability_batch(
        &self,
        batch: crate::capability::SessionCapabilityBatch,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<
        crate::capability::CapabilityCommitReceipt,
        crate::capability::CapabilityRuntimeError,
    > {
        let _mutation = self.close_handle.extension_mutation.lock().await;
        if self.is_closed() {
            return Err(crate::capability::CapabilityRuntimeError::SessionClosed);
        }

        let preparation_cancellation = tokio_util::sync::CancellationToken::new();
        let prepared = tokio::select! {
            biased;
            _ = self.session_cancel.cancelled() => {
                preparation_cancellation.cancel();
                return Err(if self.is_closed() {
                    crate::capability::CapabilityRuntimeError::SessionClosed
                } else {
                    crate::capability::CapabilityRuntimeError::Cancelled
                });
            }
            _ = cancellation.cancelled() => {
                preparation_cancellation.cancel();
                return Err(crate::capability::CapabilityRuntimeError::Cancelled);
            }
            result = batch.prepare(
                &self.capability_catalog,
                preparation_cancellation.clone(),
            ) => result?,
        };
        self.ensure_projected_mcp_server_names_available(prepared.projection()?)
            .await?;

        // The close boundary and Run pinning use this same short mutex. The
        // prepared transaction holds no registry write lock while waiting.
        let _publication = self
            .close_handle
            .immediate_extension_mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_closed() {
            return Err(crate::capability::CapabilityRuntimeError::SessionClosed);
        }
        if cancellation.is_cancelled() || self.session_cancel.is_cancelled() {
            return Err(crate::capability::CapabilityRuntimeError::Cancelled);
        }
        // The legacy public registry guard does not participate in the
        // Session mutation gate. Keep its lock through catalog publication so
        // direct mutation linearizes wholly before validation or after CAS.
        let command_registry = self
            .command_registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::agent_loop_runtime::validate_capability_projection_runtime(
            self,
            prepared.projection()?,
            &command_registry,
        )?;
        prepared.commit()
    }

    /// Drain prepared effects from failed or retired host generations.
    pub async fn drain_capability_cleanup(&self) -> crate::capability::CapabilityCleanupReport {
        self.capability_catalog.drain_cleanup().await
    }

    #[cfg(test)]
    pub(crate) async fn admit_capability_run(
        &self,
    ) -> std::result::Result<
        crate::capability::SessionCapabilityRun,
        crate::capability::CapabilityRuntimeError,
    > {
        let projection = {
            let _admission = self
                .close_handle
                .immediate_extension_mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.is_closed() {
                return Err(crate::capability::CapabilityRuntimeError::SessionClosed);
            }
            self.capability_catalog.pin()
        };
        let ceiling = self.capability_run_ceiling(projection.projection().set())?;
        crate::capability::SessionCapabilityRun::admit(
            projection,
            "active",
            "active",
            ceiling,
            self.session_cancel.child_token(),
        )
        .await
    }

    pub(super) fn capability_run_ceiling(
        &self,
        set: &crate::capability::CapabilitySet,
    ) -> std::result::Result<
        crate::capability::CapabilityCeiling,
        crate::capability::CapabilityRuntimeError,
    > {
        let mut governance = crate::capability::GovernanceCapabilityCeiling::none_required();
        if self.config.permission_checker.is_some() || self.config.permission_policy.is_some() {
            governance = governance.require_permission_guard();
        }
        if self.config.confirmation_manager.is_some() || self.config.confirmation_policy.is_some() {
            governance = governance.require_confirmation_guard();
        }
        if self.config.security_provider.is_some() {
            governance = governance.require_security_guard();
        }
        if self.config.budget_guard.is_some() || self.budget_guard().is_some() {
            governance = governance.require_budget_guard();
        }
        if self.config.enforce_active_skill_tool_restrictions {
            governance = governance.require_active_skill_restrictions();
        }
        let execution = crate::capability::CapabilityExecutionCeiling::new(
            self.config.max_tool_rounds,
            self.config.max_parallel_tasks,
            self.config.tool_timeout_ms,
            self.config.llm_api_timeout_ms,
            self.config.max_execution_time_ms,
        )?;
        crate::capability::CapabilityCeiling::all(
            set,
            crate::capability::WorkspaceCapabilityCeiling::all(),
            governance,
            execution,
        )
        .map_err(Into::into)
    }

    pub(super) fn ensure_compatibility_name_available(
        &self,
        kind: crate::capability::CapabilityKind,
        public_name: &str,
    ) -> crate::error::Result<()> {
        let projection = self.capability_catalog.pin();
        if projection
            .projection()
            .iter()
            .any(|(_, value)| match value {
                crate::capability::CapabilityValue::Mcp(binding)
                    if kind == crate::capability::CapabilityKind::Tool =>
                {
                    binding.contains_public_tool_name(public_name)
                }
                crate::capability::CapabilityValue::Agent(agent) => {
                    kind == crate::capability::CapabilityKind::Agent
                        && crate::subagent::agent_names_conflict(&agent.name, public_name)
                }
                _ => value.kind() == kind && value.public_name() == Some(public_name),
            })
        {
            return Err(
                crate::capability::CapabilityRuntimeError::RuntimeNameConflict {
                    kind,
                    public_name: public_name.to_owned(),
                }
                .into(),
            );
        }
        Ok(())
    }

    async fn ensure_projected_mcp_server_names_available(
        &self,
        projection: &crate::capability::CapabilityProjection,
    ) -> std::result::Result<(), crate::capability::CapabilityRuntimeError> {
        let server_names = projection
            .iter()
            .filter_map(|(_, value)| match value {
                crate::capability::CapabilityValue::Mcp(binding) => {
                    Some(binding.server_name().to_owned())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for server_name in server_names {
            for manager in &self.mcp_managers {
                if manager.contains_server(&server_name).await {
                    return Err(
                        crate::capability::CapabilityRuntimeError::RuntimeNameConflict {
                            kind: crate::capability::CapabilityKind::Mcp,
                            public_name: server_name,
                        },
                    );
                }
            }
        }
        Ok(())
    }
}
