use super::*;

/// Model-visible tool that executes a dynamic workflow through A3S Flow.
pub struct DynamicWorkflowTool {
    pub(super) registry: DynamicWorkflowRegistry,
    pub(super) flow_event_store: Option<Arc<dyn FlowEventStore>>,
    pub(super) graph_observer: Option<FlowGraphObserver>,
    pub(super) task_scheduler: Option<Arc<TaskScheduler>>,
    pub(super) admit_steps_globally: bool,
    pub(super) runtime_build_compatibility: Option<RuntimeBuildCompatibility>,
    pub(super) continuation_lease_ledger: Option<Arc<dyn FlowDecisionLedger>>,
    pub(super) continuation_lease_ms: u64,
    pub(super) memory_continuation_lease_ledger: Arc<MemoryFlowDecisionLedger>,
    pub(super) metrics: Arc<DynamicWorkflowMetrics>,
}

pub(super) enum DynamicWorkflowRegistry {
    Standalone(Arc<ToolRegistry>),
    RegistryBound(Weak<ToolRegistry>),
}

impl DynamicWorkflowRegistry {
    pub(super) fn resolve(&self) -> Option<Arc<ToolRegistry>> {
        match self {
            Self::Standalone(registry) => Some(Arc::clone(registry)),
            Self::RegistryBound(registry) => registry.upgrade(),
        }
    }
}

impl DynamicWorkflowTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry: DynamicWorkflowRegistry::Standalone(registry),
            flow_event_store: None,
            graph_observer: None,
            task_scheduler: None,
            admit_steps_globally: false,
            runtime_build_compatibility: None,
            continuation_lease_ledger: None,
            continuation_lease_ms: DEFAULT_DYNAMIC_WORKFLOW_LEASE_MS,
            memory_continuation_lease_ledger: Arc::new(MemoryFlowDecisionLedger::new()),
            metrics: Arc::new(DynamicWorkflowMetrics::default()),
        }
    }

    pub(super) fn new_registry_bound(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry: DynamicWorkflowRegistry::RegistryBound(Arc::downgrade(&registry)),
            flow_event_store: None,
            graph_observer: None,
            task_scheduler: None,
            admit_steps_globally: false,
            runtime_build_compatibility: None,
            continuation_lease_ledger: None,
            continuation_lease_ms: DEFAULT_DYNAMIC_WORKFLOW_LEASE_MS,
            memory_continuation_lease_ledger: Arc::new(MemoryFlowDecisionLedger::new()),
            metrics: Arc::new(DynamicWorkflowMetrics::default()),
        }
    }

    /// Return bounded cumulative worker-claim diagnostics for this tool
    /// instance. Durable per-run attempt state remains in the Flow ledger.
    pub fn health(&self) -> DynamicWorkflowHealthSnapshot {
        self.metrics.snapshot()
    }

    /// Project committed Flow events into an optional reactive state graph.
    /// A3S Flow remains the workflow execution source of truth.
    pub fn with_graph_observer(mut self, observer: FlowGraphObserver) -> Self {
        self.graph_observer = Some(observer);
        self
    }

    /// Use a host-owned Flow event store for workflow history and control.
    ///
    /// This is the extension point for remote or database-backed hosts. The
    /// store must provide its own durable append/sequence contract; Code does
    /// not mirror its events into a second journal or cache. When omitted,
    /// local workspaces use [`CrossProcessFlowEventStore`] and non-local
    /// contexts use a process-local in-memory store for compatibility.
    pub fn with_flow_event_store(mut self, store: Arc<dyn FlowEventStore>) -> Self {
        self.flow_event_store = Some(store);
        self
    }

    /// Configure optional global admission for direct script-backed Flow
    /// steps. Session-bound registrations should leave this disabled because
    /// the enclosing session operation already owns the global lease.
    pub fn with_task_scheduler(
        mut self,
        scheduler: Arc<TaskScheduler>,
        admit_steps_globally: bool,
    ) -> Self {
        self.task_scheduler = Some(scheduler);
        self.admit_steps_globally = admit_steps_globally;
        self
    }

    /// Fence new and resumed runs to an explicit runtime-build compatibility
    /// set. By default the tool pins new runs to
    /// [`DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID`] and temporarily accepts legacy
    /// unpinned histories; hosts that can replay older builds should add them
    /// to this compatibility set explicitly.
    pub fn with_runtime_build_compatibility(
        mut self,
        compatibility: RuntimeBuildCompatibility,
    ) -> Self {
        self.runtime_build_compatibility = Some(compatibility);
        self
    }

    /// Use a caller-owned durable lease ledger for worker admission.
    ///
    /// The ledger stores only claim metadata and digests; A3S Flow's event
    /// store remains the workflow source of truth. Hosts that run multiple
    /// workers should provide a shared ledger (for example a
    /// [`FileFlowDecisionLedger`]) and choose a lease long enough for one
    /// heartbeat interval. Without an explicit ledger, local workspaces use a
    /// sidecar file ledger and remote/in-memory contexts use a tool-scoped
    /// memory ledger.
    pub fn with_continuation_lease_ledger(
        mut self,
        ledger: Arc<dyn FlowDecisionLedger>,
        lease_ms: u64,
    ) -> Self {
        self.continuation_lease_ledger = Some(ledger);
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }

    /// Change the default worker lease while retaining automatic ledger
    /// selection.
    pub fn with_continuation_lease_ms(mut self, lease_ms: u64) -> Self {
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }

    /// Bind a host control handle to one durable run and its replay inputs.
    ///
    /// The handle is intentionally separate from the model-visible tool call:
    /// a host must provide the exact source and initial input before it can
    /// inspect, drive, or cancel a run. No operation is performed until a
    /// method on the returned handle is awaited.
    pub fn control(
        &self,
        run_id: impl Into<String>,
        source: impl Into<String>,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<DynamicWorkflowControl> {
        let registry = self
            .registry
            .resolve()
            .ok_or_else(|| anyhow::anyhow!("tool registry is closed"))?;
        let run_id = run_id.into();
        if !safe_workflow_run_id(&run_id) {
            anyhow::bail!(
                "dynamic workflow run_id must contain only ASCII letters, numbers, '-' or '_'"
            );
        }
        let source = source.into();
        if source.is_empty() {
            anyhow::bail!("dynamic workflow source must not be empty");
        }
        dynamic_workflow_input_identity(&input)
            .map_err(|error| anyhow::anyhow!("invalid dynamic workflow input: {error}"))?;
        let allowed_tools = default_allowed_tools(&registry);
        Ok(DynamicWorkflowControl {
            registry,
            context: ctx.clone(),
            flow_event_store: self.flow_event_store.clone(),
            run_id,
            source: Arc::from(source),
            input,
            allowed_tools,
            limits: DynamicWorkflowScriptLimits::default(),
            graph_observer: self.graph_observer.clone(),
            task_scheduler: self.task_scheduler.clone(),
            admit_steps_globally: self.admit_steps_globally,
            runtime_build_compatibility: self.runtime_build_compatibility.clone(),
            continuation_lease_ledger: self.continuation_lease_ledger.clone(),
            continuation_lease_ms: self.continuation_lease_ms,
            memory_continuation_lease_ledger: Arc::clone(&self.memory_continuation_lease_ledger),
            metrics: Arc::clone(&self.metrics),
        })
    }

    pub(super) async fn continuation_lease_ledger_for_context(
        &self,
        ctx: &ToolContext,
    ) -> Result<Arc<dyn FlowDecisionLedger>> {
        dynamic_workflow_lease_ledger_for_context(
            self.continuation_lease_ledger.as_ref(),
            &self.memory_continuation_lease_ledger,
            ctx,
        )
        .await
    }
}

pub(super) async fn dynamic_workflow_lease_ledger_for_context(
    explicit: Option<&Arc<dyn FlowDecisionLedger>>,
    memory: &Arc<MemoryFlowDecisionLedger>,
    ctx: &ToolContext,
) -> Result<Arc<dyn FlowDecisionLedger>> {
    if let Some(ledger) = explicit {
        return Ok(Arc::clone(ledger));
    }
    let Some(root) = ctx.workspace_services.local_root() else {
        let ledger: Arc<dyn FlowDecisionLedger> = memory.clone();
        return Ok(ledger);
    };
    let workflow_root = dynamic_workflow_store_path(root);
    validate_dynamic_workflow_directory(&root.join(".a3s"), ".a3s").await?;
    validate_dynamic_workflow_directory(&workflow_root, ".a3s/workflow").await?;
    let lease_root = workflow_root.join(DYNAMIC_WORKFLOW_LEASE_RELATIVE_PATH);
    validate_dynamic_workflow_directory(&lease_root, ".a3s/workflow/leases").await?;
    Ok(Arc::new(FileFlowDecisionLedger::new(lease_root)))
}

#[async_trait]
impl Tool for DynamicWorkflowTool {
    fn name(&self) -> &str {
        DYNAMIC_WORKFLOW_TOOL
    }

    fn description(&self) -> &str {
        "Run a local dynamic workflow with A3S Flow. The workflow source is a sandboxed JavaScript PTC script that may call allowed ctx tools; A3S Flow records workflow and step history."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "source": {
                    "type": "string",
                    "description": "JavaScript PTC source defining async function run(ctx, inputs). For inputs.kind='workflow', return a Flow command: {type:'complete', output}, {type:'fail', error}, {type:'schedule_step', step_id, step_name, input, retry?}, or {type:'schedule_steps', steps:[...]}. For inputs.kind='step', return the step JSON output. A scheduled step with step_name='task' bypasses QuickJS and calls the host task tool directly with input as its arguments; legacy parallel_task steps remain readable."
                },
                "input": {
                    "type": "object",
                    "description": "Initial workflow input."
                },
                "run_id": {
                    "type": "string",
                    "description": "Optional durable run id. Reusing it with the same source and input is idempotent."
                },
                "allowed_tools": {
                    "type": "array",
                    "description": "Tool names the workflow script may call through ctx. Defaults to all registered tools except program, dynamic_workflow, and the legacy parallel_task alias. Direct task fan-out is blocked inside QuickJS; schedule a host task step instead. Login-registered tools such as runtime are allowed when present.",
                    "items": { "type": "string" }
                },
                "limits": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "timeoutMs": { "type": "integer", "minimum": 1 },
                        "maxToolCalls": { "type": "integer", "minimum": 1 },
                        "maxOutputBytes": { "type": "integer", "minimum": 1 },
                        "maxConcurrentGenerations": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 4,
                            "description": "Optional bounded fan-out for independently session-bound generate_object steps. Providers without session forking remain single-flight."
                        },
                        "maxConcurrentSteps": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 32,
                            "description": "Optional per-workflow limit for concurrently executing Flow step bodies."
                        }
                    }
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(registry) = self.registry.resolve() else {
            return Ok(ToolOutput::error("Tool registry is closed"));
        };
        let Some(source) = args.get("source").and_then(Value::as_str) else {
            return Ok(ToolOutput::error("dynamic_workflow requires source"));
        };
        let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
        let allowed_tools = args
            .get("allowed_tools")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| default_allowed_tools(&registry));
        let limits: DynamicWorkflowScriptLimits = args
            .get("limits")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();

        let (runtime_build_id, runtime_build_compatibility) =
            match dynamic_workflow_runtime_configuration(self.runtime_build_compatibility.as_ref())
            {
                Ok(configuration) => configuration,
                Err(error) => {
                    return Ok(ToolOutput::error(format!(
                        "invalid dynamic workflow runtime build identity: {error}"
                    )))
                }
            };
        let source_hash = source_hash(source);
        let base_spec = WorkflowSpec::rust_embedded(
            "a3s-code.dynamic-workflow",
            source_hash.as_str(),
            "ptc",
            "run",
        );

        if ctx.is_cancelled() {
            return Ok(ToolOutput::error(
                "dynamic_workflow was cancelled before admission",
            ));
        }
        let requested_run_id = args.get("run_id").and_then(Value::as_str);
        if requested_run_id.is_some_and(|run_id| !safe_workflow_run_id(run_id)) {
            return Ok(ToolOutput::error(
                "dynamic_workflow run_id must contain only ASCII letters, numbers, '-' or '_'",
            ));
        }
        let run_id = requested_run_id
            .map(ToString::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let store = match self.flow_event_store.clone() {
            Some(store) => Ok(store),
            None => flow_store_for_context(ctx, Some(&run_id)).await,
        };
        let store = match store {
            Ok(store) => store,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let prior_history = match store.list(&run_id).await {
            Ok(history) => history,
            Err(a3s_flow::FlowError::RunNotFound(_)) => Vec::new(),
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        // Preserve the immutable build pin already persisted in a resumed
        // history. A new run is pinned to this worker's current build; a
        // legacy run remains intentionally unpinned during migration. Keeping
        // the exact persisted spec is required because Flow treats the whole
        // workflow definition as the idempotent start contract.
        let (spec, effective_runtime_build_id) = prior_history
            .iter()
            .find_map(|envelope| match &envelope.event {
                FlowEvent::RunCreated { spec, .. } => Some(spec.clone()),
                _ => None,
            })
            .map(|persisted_spec| {
                let runtime_build_id = persisted_spec
                    .runtime_build_id
                    .as_ref()
                    .map(ToString::to_string);
                (persisted_spec, runtime_build_id)
            })
            .unwrap_or_else(|| {
                let spec = base_spec.with_runtime_build(runtime_build_id.clone());
                (spec, Some(runtime_build_id.to_string()))
            });
        let claim_identity = match dynamic_workflow_claim_identity(
            &run_id,
            source,
            &input,
            runtime_build_id.as_str(),
            &prior_history,
        ) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "dynamic workflow continuation identity rejected: {error}"
                )))
            }
        };
        let lease_ledger = match self.continuation_lease_ledger_for_context(ctx).await {
            Ok(ledger) => ledger,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let lease_claim = match claim_dynamic_workflow_lease(
            lease_ledger,
            claim_identity.clone(),
            self.continuation_lease_ms,
            Arc::clone(&self.metrics),
        )
        .await
        {
            Ok(claim) => claim,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let (lease, mut lease_state) = match lease_claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => (Some(lease), "claimed"),
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                if !history_is_terminal(&prior_history) {
                    return Ok(ToolOutput::error(
                        "dynamic workflow worker claim is completed but its durable run is not terminal",
                    ));
                }
                (None, "already_completed")
            }
        };
        let parent_cancellation = ctx.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = ctx
            .clone()
            .with_run_id(run_id.clone())
            .with_cancellation(child_cancellation.clone());
        let scheduler_quota = if self.admit_steps_globally && self.task_scheduler.is_some() {
            Some(
                TaskSchedulerQuota::new(
                    lease
                        .as_ref()
                        .map(|lease| lease.identity.clone())
                        .unwrap_or_else(|| claim_identity.clone()),
                    dynamic_workflow_scheduler_quota_limit(&limits),
                )
                .map_err(|error| {
                    anyhow::anyhow!("build dynamic workflow scheduler quota: {error}")
                })?,
            )
        } else {
            None
        };
        let mut runtime = DynamicWorkflowRuntime::new(registry, workflow_context.clone(), source)
            .with_allowed_tools(allowed_tools)
            .with_limits(limits);
        if let Some(scheduler) = &self.task_scheduler {
            runtime = runtime.with_task_scheduler(Arc::clone(scheduler), self.admit_steps_globally);
            if let Some(quota) = scheduler_quota {
                runtime = runtime.with_task_scheduler_quota(quota);
            }
        }
        if let Some(lease) = lease.as_ref() {
            runtime = runtime.with_continuation_lease(Arc::new(lease.clone()));
        }
        let runtime = Arc::new(runtime);
        let runtime_for_metadata = Arc::clone(&runtime);
        let initial_plan = if prior_history.is_empty() {
            ExecutionPlan::new("dynamic workflow", Complexity::Medium)
        } else {
            dynamic_workflow_execution_plan(&prior_history)
        };
        let mut observers: Vec<Arc<dyn FlowEventObserver>> = Vec::new();
        if let Some(tx) = ctx.agent_event_tx.clone() {
            observers.push(Arc::new(AgentEventFlowObserver::new(
                tx,
                ctx.session_id.clone().unwrap_or_default(),
                initial_plan,
            )));
        }
        if let Some(observer) = &self.graph_observer {
            observers.push(Arc::new(observer.clone()));
        }
        let mut engine_builder = FlowEngine::builder(runtime)
            .with_store(store)
            .with_runtime_build_compatibility(runtime_build_compatibility);
        if !observers.is_empty() {
            engine_builder = engine_builder
                .with_observer(Arc::new(FanoutFlowEventObserver::from_observers(observers)));
        }
        let engine = engine_builder.build();
        let input_for_identity = input.clone();

        let execution_result = if let Some(lease) = lease.as_ref() {
            drive_dynamic_workflow_with_lease(
                &engine,
                &run_id,
                spec,
                input,
                DynamicWorkflowDriveContext {
                    source,
                    input_for_identity: &input_for_identity,
                    runtime_build_id: runtime_build_id.as_str(),
                    workflow_context: &workflow_context,
                    parent_cancellation,
                    child_cancellation,
                },
                lease,
            )
            .await
        } else {
            let started_run_id = match engine.start_with_id(&run_id, spec, input).await {
                Ok(run_id) => run_id,
                Err(error) => return Ok(ToolOutput::error(error.to_string())),
            };
            if started_run_id != run_id {
                return Ok(ToolOutput::error(format!(
                    "dynamic workflow engine returned unexpected run id `{started_run_id}` for `{run_id}`"
                )));
            }
            let snapshot = match drive_inline_retries(&engine, &run_id, &workflow_context).await {
                Ok(snapshot) => snapshot,
                Err(error) => return Ok(ToolOutput::error(error.to_string())),
            };
            collect_dynamic_workflow_execution(
                &engine,
                &run_id,
                snapshot,
                source,
                &input_for_identity,
                runtime_build_id.as_str(),
            )
            .await
        };
        let execution = match execution_result {
            Ok(execution) => execution,
            Err(err) => return Ok(ToolOutput::error(err.to_string())),
        };
        let DynamicWorkflowExecution {
            snapshot,
            history,
            continuation_identity,
        } = execution;
        if lease.is_some() {
            lease_state = if snapshot.status.is_terminal() {
                "completed"
            } else {
                "released"
            };
        }

        let output = match &snapshot.output {
            Some(output) => {
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string())
            }
            None => snapshot
                .error
                .clone()
                .unwrap_or_else(|| format!("workflow status: {:?}", snapshot.status)),
        };

        let status = snapshot.status;
        let plan = dynamic_workflow_execution_plan(&history);
        let plan_identity = match plan.definition_identity() {
            Ok(identity) => identity,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let metadata = json!({
            "dynamic_workflow": {
                "run_id": run_id,
                "status": format!("{:?}", snapshot.status),
                "last_sequence": snapshot.last_sequence,
                "source_hash": source_hash,
                "runtime_build_id": effective_runtime_build_id,
                "snapshot": snapshot,
                "history": history,
                "plan": plan,
                "plan_identity": plan_identity,
                "continuation_identity": continuation_identity,
                "worker_lease": {
                    "state": lease_state,
                    "attempt": lease.as_ref().map(|lease| lease.attempt),
                },
                "admission": runtime_for_metadata.admission_stats(),
            }
        });
        let output = match status {
            WorkflowRunStatus::Completed => ToolOutput::success(output),
            WorkflowRunStatus::Failed | WorkflowRunStatus::Cancelled => ToolOutput::error(output),
            _ => ToolOutput::error(format!(
                "dynamic_workflow ended without a terminal result: {status:?}; {output}"
            )),
        };

        Ok(output.with_metadata(metadata))
    }
}
