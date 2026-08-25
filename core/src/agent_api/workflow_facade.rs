use super::*;

impl AgentSession {
    /// An [`AgentExecutor`](crate::orchestration::AgentExecutor) backed by this
    /// session — runs each orchestrated step as a child agent on this node,
    /// inheriting the session's agent registry, LLM client, workspace, MCP
    /// tools, and subagent tracker.
    ///
    /// This is what the orchestration combinators
    /// ([`execute_steps_parallel`](crate::orchestration::execute_steps_parallel),
    /// [`execute_pipeline`](crate::orchestration::execute_pipeline),
    /// [`execute_steps_parallel_resumable`](crate::orchestration::execute_steps_parallel_resumable))
    /// run against; a host can instead supply its own executor to place steps
    /// across a cluster.
    pub fn agent_executor(&self) -> Arc<dyn crate::orchestration::AgentExecutor> {
        Arc::new(self.build_task_executor(self.parent_run_context()))
    }

    /// Re-register `task`/`parallel_task` with the finalized session runtime.
    ///
    /// Session capability assembly happens before the per-session HITL manager is
    /// constructed from `confirmation_policy`. Refreshing after `AgentConfig` is
    /// final keeps model-driven delegation, workflow delegation, and
    /// `agent_executor()` on the same permission/HITL/workspace context.
    pub(crate) fn refresh_task_delegation_tools(&self) {
        if !self.config.auto_delegation.allow_manual_delegation {
            return;
        }
        crate::tools::register_task_with_mcp_managers_and_scheduler(
            self.tool_executor.registry(),
            Arc::clone(&self.llm_client),
            Arc::clone(&self.agent_registry),
            self.workspace.display().to_string(),
            self.mcp_managers.clone(),
            Some(self.parent_run_context()),
            Some(Arc::clone(&self.subagent_tasks)),
            Arc::clone(&self.task_scheduler),
        );
    }

    /// Re-register the model-calling skill tool with the effective runtime
    /// budget. SearchSkills is refreshed as part of the same registration so
    /// both tools continue to share the session's effective registry.
    pub(super) fn refresh_skill_tools(&self) {
        let Some(skill_registry) = self.config.skill_registry.clone() else {
            tracing::warn!(
                session_id = %self.session_id,
                "Cannot refresh skill tools without an effective skill registry"
            );
            return;
        };
        let mut config = self.config.clone();
        if let Some(runtime_guard) = self.budget_guard() {
            config.budget_guard = Some(runtime_guard);
        }
        crate::tools::register_skill(
            self.tool_executor.registry(),
            Arc::clone(&self.llm_client),
            skill_registry,
            Arc::clone(&self.tool_executor),
            config,
        );
    }

    /// Build the in-box [`TaskExecutor`](crate::tools::TaskExecutor) for this
    /// session, applying `parent` as the child-run capability context. Shared by
    /// [`agent_executor`](Self::agent_executor) and [`workflow`](Self::workflow)
    /// so both wire children identically.
    fn build_task_executor(
        &self,
        parent: crate::child_run::ChildRunContext,
    ) -> crate::tools::TaskExecutor {
        crate::tools::TaskExecutor::with_mcp_managers(
            Arc::clone(&self.agent_registry),
            Arc::clone(&self.llm_client),
            self.workspace.display().to_string(),
            self.mcp_managers.clone(),
        )
        .with_parent_context(parent)
        .with_parent_cancellation(self.session_cancel.child_token())
        .with_subagent_tracker(Arc::clone(&self.subagent_tasks))
        .with_max_parallel_tasks(self.config.max_parallel_tasks)
        // Host workflow/AgentExecutor calls have no enclosing conversation
        // lease, so each foreground step enters the global queue.
        .with_task_scheduler(Arc::clone(&self.task_scheduler), true)
    }

    /// A programmable [`Workflow`](crate::orchestration::Workflow) bound to this
    /// session.
    ///
    /// Pre-wired with this session's executor (inheriting the same governance as
    /// model-driven delegation), persistence store (so each
    /// [`phase`](crate::orchestration::Workflow::phase) is a resume boundary),
    /// per-step event stream, and a session-derived stable root id. Control flow
    /// is ordinary Rust: `await` a verb, inspect the outcomes, decide what runs
    /// next.
    pub fn workflow(&self) -> crate::orchestration::Workflow {
        self.workflow_with_token_budget(None)
    }

    /// Like [`workflow`](Self::workflow) but with a hard token ceiling shared
    /// across every step. The cap is a best-effort *soft* cost ceiling — under a
    /// wide fan-out a few in-flight turns can race past it before the shared
    /// ledger catches up (see [`WorkflowBudget`](crate::orchestration::WorkflowBudget)).
    pub fn workflow_with_token_budget(
        &self,
        limit_tokens: Option<u64>,
    ) -> crate::orchestration::Workflow {
        use crate::budget::BudgetGuard;

        // One shared ledger for the whole workflow, wrapping the session's own
        // budget guard (if any) so a host's per-tenant accounting keeps working.
        let mut budget = crate::orchestration::WorkflowBudget::new(limit_tokens);
        if let Some(inner) = self
            .budget_guard()
            .or_else(|| self.config.budget_guard.clone())
        {
            budget = budget.with_inner(inner);
        }
        let budget = Arc::new(budget);

        // Install the shared ledger as the child runs' budget guard so every
        // step's per-turn LLM accounting feeds it.
        let mut parent = self.parent_run_context();
        parent.budget_guard = Some(Arc::clone(&budget) as Arc<dyn BudgetGuard>);
        let executor: Arc<dyn crate::orchestration::AgentExecutor> =
            Arc::new(self.build_task_executor(parent));

        let mut builder = crate::orchestration::Workflow::builder(executor)
            .with_root_id(format!("wf-{}", self.session_id))
            .with_budget(Arc::clone(&budget));
        if let Some(store) = self.session_store.clone() {
            builder = builder.with_store(store);
        }
        if let Some(step_events) = self.tool_context.agent_event_tx.clone() {
            builder = builder.with_step_events(step_events);
        }
        builder.build()
    }

    /// Build the [`ChildRunContext`](crate::child_run::ChildRunContext) that
    /// orchestrated / delegated child runs inherit from this session.
    ///
    /// Mirrors the context the model-driven `task` path
    /// installs (see `register_task_capability` in `agent_api/capabilities.rs`)
    /// so a step run through [`agent_executor`](Self::agent_executor) carries the
    /// SAME governance — hooks, security provider, skill restrictions,
    /// confirmation, the shared workspace, and safety limits — instead of
    /// weaker ambient authority.
    pub(crate) fn parent_run_context(&self) -> crate::child_run::ChildRunContext {
        crate::child_run::ChildRunContext {
            security_provider: self.config.security_provider.clone(),
            hook_engine: Some(match &self.hook_executor {
                Some(executor) => executor.clone(),
                None => Arc::clone(&self.hook_engine) as Arc<dyn crate::hooks::HookExecutor>,
            }),
            skill_registry: self.config.skill_registry.clone(),
            permission_checker: self.config.permission_checker.clone(),
            permission_policy: self.config.permission_policy.clone(),
            tool_timeout_ms: self.config.tool_timeout_ms,
            llm_api_timeout_ms: self.config.llm_api_timeout_ms,
            max_parallel_tasks: Some(self.config.max_parallel_tasks),
            max_execution_time_ms: self.config.max_execution_time_ms,
            circuit_breaker_threshold: Some(self.config.circuit_breaker_threshold),
            duplicate_tool_call_threshold: Some(self.config.duplicate_tool_call_threshold),
            confirmation_manager: self.config.confirmation_manager.clone(),
            enforce_active_skill_tool_restrictions: Some(
                self.config.enforce_active_skill_tool_restrictions,
            ),
            workspace_services: Some(Arc::clone(&self.tool_context.workspace_services)),
            immutable_content_adapter: self
                .tool_executor
                .registry()
                .immutable_content_adapter()
                .cloned(),
            sandbox_handle: self.tool_context.sandbox.clone(),
            tool_presentation_profile: Some(self.config.tool_presentation_profile.clone()),
            budget_guard: self
                .budget_guard()
                .or_else(|| self.config.budget_guard.clone()),
        }
    }
}
