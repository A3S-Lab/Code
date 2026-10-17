use super::*;

impl DynamicWorkflowControl {
    /// Return the immutable run id bound to this control handle.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Return bounded cumulative worker-claim diagnostics shared by this
    /// tool and all controls derived from it.
    pub fn health(&self) -> DynamicWorkflowHealthSnapshot {
        self.metrics.snapshot()
    }

    /// Compose workflow claim counters with the optional agent-wide scheduler
    /// health snapshot. This is a read-only diagnostic view; Flow history and
    /// the worker lease remain the only authorities for workflow state.
    pub async fn diagnostics(&self) -> Result<DynamicWorkflowControlDiagnostics> {
        let scheduler = match self.task_scheduler.as_ref() {
            Some(scheduler) => Some(scheduler.health().await.map_err(|error| {
                anyhow::anyhow!("read dynamic workflow scheduler health: {error}")
            })?),
            None => None,
        };
        let scheduler_quota = if self.admit_steps_globally {
            if let (Some(scheduler), Some(quota)) = (
                self.task_scheduler.as_ref(),
                // Diagnostics are valid before a control handle has started
                // its run. `allow_missing` keeps this read-only projection
                // from turning an observation into an implicit start/lookup
                // precondition while still deriving the same stable claim
                // identity that the first drive will use.
                self.prepare(true).await?.scheduler_quota,
            ) {
                Some(scheduler.quota_snapshot(&quota).await.map_err(|error| {
                    anyhow::anyhow!("read dynamic workflow scheduler quota: {error}")
                })?)
            } else {
                None
            }
        } else {
            None
        };
        Ok(DynamicWorkflowControlDiagnostics {
            workflow: self.metrics.snapshot(),
            scheduler,
            scheduler_quota,
        })
    }

    /// Replace the set of tools available to replayed script steps.
    pub fn with_allowed_tools(mut self, allowed_tools: impl IntoIterator<Item = String>) -> Self {
        self.allowed_tools = sanitize_allowed_tools(allowed_tools);
        self
    }

    /// Replace the bounded script and orchestration limits used during replay.
    pub fn with_limits(mut self, limits: DynamicWorkflowScriptLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Project control-driven Flow events into a graph observer.
    pub fn with_graph_observer(mut self, observer: FlowGraphObserver) -> Self {
        self.graph_observer = Some(observer);
        self
    }

    /// Configure optional global admission for direct script-backed steps.
    pub fn with_task_scheduler(
        mut self,
        scheduler: Arc<TaskScheduler>,
        admit_steps_globally: bool,
    ) -> Self {
        self.task_scheduler = Some(scheduler);
        self.admit_steps_globally = admit_steps_globally;
        self
    }

    /// Set the runtime-build compatibility policy used by control replay.
    pub fn with_runtime_build_compatibility(
        mut self,
        compatibility: RuntimeBuildCompatibility,
    ) -> Self {
        self.runtime_build_compatibility = Some(compatibility);
        self
    }

    /// Use a caller-owned worker lease ledger for control operations.
    pub fn with_continuation_lease_ledger(
        mut self,
        ledger: Arc<dyn FlowDecisionLedger>,
        lease_ms: u64,
    ) -> Self {
        self.continuation_lease_ledger = Some(ledger);
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }

    /// Change the worker lease while retaining automatic ledger selection.
    pub fn with_continuation_lease_ms(mut self, lease_ms: u64) -> Self {
        self.continuation_lease_ms = lease_ms.max(1);
        self
    }
}

pub(super) struct DynamicWorkflowControlPreparation {
    pub(super) store: Arc<dyn FlowEventStore>,
    pub(super) prior_history: Vec<FlowEventEnvelope>,
    pub(super) spec: WorkflowSpec,
    pub(super) effective_runtime_build_id: Option<String>,
    pub(super) runtime_build_id: RuntimeBuildId,
    pub(super) runtime_build_compatibility: RuntimeBuildCompatibility,
    pub(super) claim_identity: ExecutionIdentityV1,
    pub(super) scheduler_quota: Option<TaskSchedulerQuota>,
    pub(super) lease_ledger: Arc<dyn FlowDecisionLedger>,
}

pub(super) fn dynamic_workflow_runtime_configuration(
    configured: Option<&RuntimeBuildCompatibility>,
) -> Result<(RuntimeBuildId, RuntimeBuildCompatibility)> {
    let runtime_build_id =
        match configured {
            Some(compatibility) => compatibility.current_build_id().clone(),
            None => RuntimeBuildId::new(DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID.to_string()).map_err(
                |error| anyhow::anyhow!("invalid dynamic workflow runtime build identity: {error}"),
            )?,
        };
    let compatibility = configured.cloned().unwrap_or_else(|| {
        RuntimeBuildCompatibility::new(runtime_build_id.clone()).accept_unpinned()
    });
    Ok((runtime_build_id, compatibility))
}

impl DynamicWorkflowControl {
    pub(super) async fn prepare(
        &self,
        allow_missing: bool,
    ) -> Result<DynamicWorkflowControlPreparation> {
        let (runtime_build_id, runtime_build_compatibility) =
            dynamic_workflow_runtime_configuration(self.runtime_build_compatibility.as_ref())?;
        let source_hash = source_hash(self.source.as_ref());
        let base_spec = WorkflowSpec::rust_embedded(
            "a3s-code.dynamic-workflow",
            source_hash.as_str(),
            "ptc",
            "run",
        );
        let store = match self.flow_event_store.clone() {
            Some(store) => store,
            None => flow_store_for_context(&self.context, Some(&self.run_id)).await?,
        };
        let prior_history = match store.list(&self.run_id).await {
            Ok(history) => history,
            Err(a3s_flow::FlowError::RunNotFound(_)) if allow_missing => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        if !allow_missing && prior_history.is_empty() {
            return Err(a3s_flow::FlowError::RunNotFound(self.run_id.clone()).into());
        }
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
        let claim_identity = dynamic_workflow_claim_identity(
            &self.run_id,
            self.source.as_ref(),
            &self.input,
            runtime_build_id.as_str(),
            &prior_history,
        )
        .map_err(|error| {
            anyhow::anyhow!("dynamic workflow continuation identity rejected: {error}")
        })?;
        let scheduler_quota = if self.admit_steps_globally && self.task_scheduler.is_some() {
            Some(
                TaskSchedulerQuota::new(
                    claim_identity.clone(),
                    dynamic_workflow_scheduler_quota_limit(&self.limits),
                )
                .map_err(|error| {
                    anyhow::anyhow!("build dynamic workflow scheduler quota: {error}")
                })?,
            )
        } else {
            None
        };
        let lease_ledger = dynamic_workflow_lease_ledger_for_context(
            self.continuation_lease_ledger.as_ref(),
            &self.memory_continuation_lease_ledger,
            &self.context,
        )
        .await?;
        Ok(DynamicWorkflowControlPreparation {
            store,
            prior_history,
            spec,
            effective_runtime_build_id,
            runtime_build_id,
            runtime_build_compatibility,
            claim_identity,
            scheduler_quota,
            lease_ledger,
        })
    }

    pub(super) fn build_engine(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
        workflow_context: ToolContext,
        lease: Option<&DynamicWorkflowLease>,
    ) -> FlowEngine {
        let mut runtime = DynamicWorkflowRuntime::new(
            Arc::clone(&self.registry),
            workflow_context,
            self.source.as_ref(),
        )
        .with_allowed_tools(self.allowed_tools.clone())
        .with_limits(self.limits.clone());
        if let Some(scheduler) = &self.task_scheduler {
            runtime = runtime.with_task_scheduler(Arc::clone(scheduler), self.admit_steps_globally);
            if let Some(quota) = preparation.scheduler_quota.clone() {
                runtime = runtime.with_task_scheduler_quota(quota);
            }
        }
        if let Some(lease) = lease {
            runtime = runtime.with_continuation_lease(Arc::new(lease.clone()));
        }
        let runtime = Arc::new(runtime);
        let initial_plan = if preparation.prior_history.is_empty() {
            ExecutionPlan::new("dynamic workflow", Complexity::Medium)
        } else {
            dynamic_workflow_execution_plan(&preparation.prior_history)
        };
        let mut observers: Vec<Arc<dyn FlowEventObserver>> = Vec::new();
        if let Some(tx) = self.context.agent_event_tx.clone() {
            observers.push(Arc::new(AgentEventFlowObserver::new(
                tx,
                self.context.session_id.clone().unwrap_or_default(),
                initial_plan,
            )));
        }
        if let Some(observer) = &self.graph_observer {
            observers.push(Arc::new(observer.clone()));
        }
        let mut builder = FlowEngine::builder(runtime)
            .with_store(Arc::clone(&preparation.store))
            .with_runtime_build_compatibility(preparation.runtime_build_compatibility.clone());
        if !observers.is_empty() {
            builder =
                builder.with_observer(Arc::new(FanoutFlowEventObserver::from_observers(observers)));
        }
        builder.build()
    }

    pub(super) async fn read_state(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
        engine: &FlowEngine,
    ) -> Result<(WorkflowRunSnapshot, Vec<FlowEventEnvelope>)> {
        for _ in 0..3 {
            // Flow exposes history and its projection as separate reads. Read
            // the journal on both sides of the projection so a concurrent
            // append cannot be mistaken for an atomic snapshot; retry a
            // bounded number of times when the sequences disagree.
            let before_history = engine.history(&self.run_id).await?;
            let snapshot = engine.snapshot(&self.run_id).await?;
            let history = engine.history(&self.run_id).await?;
            let before_sequence = before_history
                .last()
                .map(|event| event.sequence)
                .unwrap_or(0);
            let last_sequence = history.last().map(|event| event.sequence).unwrap_or(0);
            if before_sequence == last_sequence && snapshot.last_sequence == last_sequence {
                return Ok((snapshot, history));
            }
        }
        let expected = preparation
            .prior_history
            .last()
            .map(|event| event.sequence)
            .unwrap_or(0);
        anyhow::bail!(
            "dynamic workflow state changed repeatedly while inspecting run `{}` (initial sequence {expected})",
            self.run_id
        )
    }

    pub(super) async fn summarize(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
        snapshot: WorkflowRunSnapshot,
        history: Vec<FlowEventEnvelope>,
    ) -> Result<DynamicWorkflowControlSnapshot> {
        let continuation_identity = dynamic_workflow_continuation_identity(
            &self.run_id,
            self.source.as_ref(),
            &self.input,
            preparation.runtime_build_id.as_str(),
            &history,
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "dynamic workflow continuation identity rejected during inspection: {error}"
            )
        })?;
        let plan = dynamic_workflow_execution_plan(&history);
        let plan_identity = plan.definition_identity()?;
        let (decision_id, request_hash) = dynamic_workflow_lease_key(&preparation.claim_identity);
        let worker_lease = preparation
            .lease_ledger
            .inspect_with_identity(&decision_id, &request_hash, &preparation.claim_identity)
            .await?;
        let completed_steps = snapshot
            .steps
            .values()
            .filter(|step| step.status == StepStatus::Completed)
            .count();
        let open_steps = snapshot
            .steps
            .values()
            .filter(|step| {
                !matches!(
                    step.status,
                    StepStatus::Completed | StepStatus::Failed | StepStatus::Cancelled
                )
            })
            .count();
        Ok(DynamicWorkflowControlSnapshot {
            run_id: self.run_id.clone(),
            status: snapshot.status,
            last_sequence: snapshot.last_sequence,
            step_count: snapshot.steps.len(),
            completed_steps,
            open_steps,
            cancellation_requested: snapshot.cancellation.is_some(),
            continuation_identity,
            plan_identity,
            runtime_build_id: preparation.effective_runtime_build_id.clone(),
            worker_lease,
        })
    }

    pub(super) async fn claim(
        &self,
        preparation: &DynamicWorkflowControlPreparation,
    ) -> Result<DynamicWorkflowLeaseClaim> {
        claim_dynamic_workflow_lease(
            Arc::clone(&preparation.lease_ledger),
            preparation.claim_identity.clone(),
            self.continuation_lease_ms,
            Arc::clone(&self.metrics),
        )
        .await
    }

    /// Inspect the durable run and return a bounded, digest-only projection.
    pub async fn inspect(&self) -> Result<DynamicWorkflowControlSnapshot> {
        let preparation = self.prepare(false).await?;
        let engine = self.build_engine(&preparation, self.context.clone(), None);
        let (snapshot, history) = self.read_state(&preparation, &engine).await?;
        self.summarize(&preparation, snapshot, history).await
    }

    /// Read the complete durable Flow history for trusted host diagnostics.
    ///
    /// Unlike [`Self::inspect`], this method intentionally returns persisted
    /// input and step output values. Callers should apply their own redaction
    /// policy before exposing the result outside a trusted control plane.
    pub async fn history(&self) -> Result<Vec<FlowEventEnvelope>> {
        let preparation = self.prepare(false).await?;
        let history = preparation.store.list(&self.run_id).await?;
        dynamic_workflow_continuation_identity(
            &self.run_id,
            self.source.as_ref(),
            &self.input,
            preparation.runtime_build_id.as_str(),
            &history,
        )
        .map_err(|error| anyhow::anyhow!("dynamic workflow history rejected: {error}"))?;
        Ok(history)
    }

    /// Request cleanup-aware durable cancellation and settle the worker lease.
    ///
    /// A live worker keeps ownership and causes this method to return a busy
    /// error; the caller can retry after the lease expires or the worker
    /// releases it. This preserves one executor for side-effecting steps.
    pub async fn request_cancellation(
        &self,
        reason: Option<String>,
    ) -> Result<DynamicWorkflowControlSnapshot> {
        validate_dynamic_workflow_control_reason(reason.as_deref())?;
        if self.context.is_cancelled() {
            anyhow::bail!("dynamic workflow control was cancelled before admission");
        }
        let preparation = self.prepare(false).await?;
        let claim = self.claim(&preparation).await?;
        let lease = match claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => lease,
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                let engine = self.build_engine(&preparation, self.context.clone(), None);
                let (snapshot, history) = self.read_state(&preparation, &engine).await?;
                if !snapshot.status.is_terminal() {
                    anyhow::bail!(
                        "dynamic workflow worker claim is completed but its durable run is not terminal"
                    );
                }
                return self.summarize(&preparation, snapshot, history).await;
            }
        };
        let parent_cancellation = self.context.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = self
            .context
            .clone()
            .with_cancellation(child_cancellation.clone());
        let engine = self.build_engine(&preparation, workflow_context.clone(), Some(&lease));
        let future = engine.request_cancellation(&self.run_id, CancellationRequest::new(reason));
        let execution = drive_dynamic_workflow_control_future(
            &engine,
            &self.run_id,
            DynamicWorkflowDriveContext {
                source: self.source.as_ref(),
                input_for_identity: &self.input,
                runtime_build_id: preparation.runtime_build_id.as_str(),
                workflow_context: &workflow_context,
                parent_cancellation,
                child_cancellation,
            },
            &lease,
            future,
        )
        .await?;
        self.summarize(&preparation, execution.snapshot, execution.history)
            .await
    }

    /// Immediately terminate the run through Flow's durable cancellation
    /// transition while still fencing the worker claim.
    pub async fn force_cancel(
        &self,
        reason: Option<String>,
    ) -> Result<DynamicWorkflowControlSnapshot> {
        validate_dynamic_workflow_control_reason(reason.as_deref())?;
        if self.context.is_cancelled() {
            anyhow::bail!("dynamic workflow control was cancelled before admission");
        }
        let preparation = self.prepare(false).await?;
        let claim = self.claim(&preparation).await?;
        let lease = match claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => lease,
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                let engine = self.build_engine(&preparation, self.context.clone(), None);
                let (snapshot, history) = self.read_state(&preparation, &engine).await?;
                if !snapshot.status.is_terminal() {
                    anyhow::bail!(
                        "dynamic workflow worker claim is completed but its durable run is not terminal"
                    );
                }
                return self.summarize(&preparation, snapshot, history).await;
            }
        };
        let parent_cancellation = self.context.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = self
            .context
            .clone()
            .with_cancellation(child_cancellation.clone());
        let engine = self.build_engine(&preparation, workflow_context.clone(), Some(&lease));
        let future = async {
            engine.force_cancel(&self.run_id, reason).await?;
            engine.snapshot(&self.run_id).await
        };
        let execution = drive_dynamic_workflow_control_future(
            &engine,
            &self.run_id,
            DynamicWorkflowDriveContext {
                source: self.source.as_ref(),
                input_for_identity: &self.input,
                runtime_build_id: preparation.runtime_build_id.as_str(),
                workflow_context: &workflow_context,
                parent_cancellation,
                child_cancellation,
            },
            &lease,
            future,
        )
        .await?;
        self.summarize(&preparation, execution.snapshot, execution.history)
            .await
    }

    /// Resume or start the bound run under the same worker fencing contract as
    /// the model-visible dynamic workflow tool.
    pub async fn drive(&self) -> Result<DynamicWorkflowControlSnapshot> {
        if self.context.is_cancelled() {
            anyhow::bail!("dynamic workflow control was cancelled before admission");
        }
        let preparation = self.prepare(true).await?;
        let claim = self.claim(&preparation).await?;
        let lease = match claim {
            DynamicWorkflowLeaseClaim::Owned(lease) => lease,
            DynamicWorkflowLeaseClaim::AlreadyCompleted => {
                let engine = self.build_engine(&preparation, self.context.clone(), None);
                let (snapshot, history) = self.read_state(&preparation, &engine).await?;
                if !snapshot.status.is_terminal() {
                    anyhow::bail!(
                        "dynamic workflow worker claim is completed but its durable run is not terminal"
                    );
                }
                return self.summarize(&preparation, snapshot, history).await;
            }
        };
        let parent_cancellation = self.context.cancellation_token();
        let child_cancellation = parent_cancellation.child_token();
        let workflow_context = self
            .context
            .clone()
            .with_cancellation(child_cancellation.clone());
        let engine = self.build_engine(&preparation, workflow_context.clone(), Some(&lease));
        let execution = drive_dynamic_workflow_with_lease(
            &engine,
            &self.run_id,
            preparation.spec.clone(),
            self.input.clone(),
            DynamicWorkflowDriveContext {
                source: self.source.as_ref(),
                input_for_identity: &self.input,
                runtime_build_id: preparation.runtime_build_id.as_str(),
                workflow_context: &workflow_context,
                parent_cancellation,
                child_cancellation,
            },
            &lease,
        )
        .await?;
        self.summarize(&preparation, execution.snapshot, execution.history)
            .await
    }
}
