//! Tool receipts are projections of the actor's existing immutable log.

use super::*;

mod cache_proof;
pub(super) use cache_proof::CacheProofCursor;

use a3s_effect::{
    Effect, PreparedToolExecution, ToolAdmissionIdentityV1, ToolEffectObservationV1,
    ToolExecutionOutcome, ToolExecutionResultV1, ToolObservationCompleteness,
    ToolPreparationRequest, ToolPreparedV1, ToolRecoveryPolicy, TOOL_PREPARED_SCHEMA,
    TOOL_RESULT_SCHEMA,
};

pub(super) fn runner_result_value(
    output: &str,
    is_error: bool,
    images: &[crate::llm::Attachment],
    exit_code: i32,
    metadata: Option<&serde_json::Value>,
) -> serde_json::Value {
    let images: Vec<_> = images.iter().map(|image| serde_json::json!({
        "media_type": image.media_type,
        "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image.data),
    })).collect();
    serde_json::Value::String(serde_json::json!({
        "kind": FACT_TOOL_RESULT_KIND,
        "text": output,
        "is_error": is_error,
        "images": images,
        "exit_code": exit_code,
        "outcome": if is_error { ToolExecutionOutcome::Failed } else { ToolExecutionOutcome::Succeeded },
        "workspace_child": metadata.and_then(|value| value.get("workspace_child")).and_then(|value| value.as_str()),
        "observation_incomplete": metadata.and_then(|value| value.get("observation_incomplete")).and_then(|value| value.as_bool()).unwrap_or(false),
        "metadata": metadata,
    }).to_string())
}

pub(super) fn runner_interrupted_value(output: &str) -> serde_json::Value {
    serde_json::Value::String(
        serde_json::json!({
            "kind": FACT_TOOL_RESULT_KIND,
            "text": output,
            "is_error": true,
            "images": [],
            "exit_code": null,
            "outcome": ToolExecutionOutcome::Cancelled,
            "observation_incomplete": true,
        })
        .to_string(),
    )
}

pub(super) async fn prepare_tools(
    tools: Arc<ExecutorTools>,
    mut request: ToolPreparationRequest,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<PreparedToolExecution, ActorError> {
    let original_digest = a3s_effect::tool_args_digest(&request.call.args)?;
    // Use the same declared-schema gateway as ordinary agent invocations,
    // against this Run's executor registry. Invalid original input must not
    // reach an input hook, which may itself have effects.
    let mut argument_error = tools
        .executor
        .registry()
        .validate_arguments(&request.call.name, &request.call.args)
        .err();
    let mut hook_evaluated = false;
    let mut denial = tools
        .agent
        .as_ref()
        .and_then(|agent| skill_denial(agent, &request.call.name))
        .map(|(output, _reason)| output);
    let original_permission = execution_permission(
        &tools.permission,
        tools.checker.as_deref(),
        &request.call.name,
        &request.call.args,
    );
    if original_permission == PermissionDecision::Deny {
        denial = Some(format!(
            "Permission denied: Tool '{}' is blocked by permission policy.",
            request.call.name
        ));
    }
    // A host backend may be unavailable before execution starts. Freeze that
    // refusal before a hook or workspace observer can create uncertainty; do
    // not infer non-execution from a later error message.
    if denial.is_none() && argument_error.is_none() && !cancel.is_cancelled() {
        if let Some(tool) = tools.executor.registry().get(&request.call.name) {
            if let Err(error) =
                tool.validate_execution_available(&request.call.args, &tools.context)
            {
                denial = Some(error.to_string());
            }
        }
    }
    // The fact path prepares tools directly instead of passing through
    // ToolInvoker::execute_budgeted. Consult the same captured host authority
    // once, before input hooks or the tool body can produce effects.
    if denial.is_none() && argument_error.is_none() && !cancel.is_cancelled() {
        if let Some(guard) = tools
            .agent
            .as_ref()
            .and_then(|agent| agent.tool_budget_guard())
        {
            let session = tools.context.session_id.as_deref().unwrap_or(THREAD);
            // This preparation owns the borrowed host callback future. Like
            // the existing pre-model budget check, cancellation drops that
            // exact future before any tool effects; it starts no detached task.
            let decision = tokio::select! {
                biased;
                _ = cancel.cancelled() => None,
                decision = guard.check_before_tool(session, &request.call.name) => Some(decision),
            };
            if let Some(decision) = decision {
                match decision {
                    crate::budget::BudgetDecision::Allow => {}
                    crate::budget::BudgetDecision::SoftLimit {
                        resource,
                        consumed,
                        limit,
                        message,
                    } => {
                        tools
                            .emit(crate::agent::AgentEvent::BudgetThresholdHit {
                                resource,
                                kind: "soft".into(),
                                consumed,
                                limit,
                                message,
                            })
                            .await;
                    }
                    crate::budget::BudgetDecision::Deny { resource, reason } => {
                        tools
                            .emit(crate::agent::AgentEvent::BudgetThresholdHit {
                                resource: resource.clone(),
                                kind: "hard".into(),
                                consumed: 0.0,
                                limit: 0.0,
                                message: Some(reason.clone()),
                            })
                            .await;
                        denial = Some(
                            crate::error::CodeError::BudgetExhausted { resource, reason }
                                .to_string(),
                        );
                    }
                }
            }
        }
    }
    if denial.is_none() && argument_error.is_none() && !cancel.is_cancelled() {
        if let Some(agent) = &tools.agent {
            let session = tools.context.session_id.clone().unwrap_or_default();
            hook_evaluated = true;
            let decision = agent
                .fire_pre_tool_use(&session, &request.call.name, &request.call.args, Vec::new())
                .await;
            if let Some(refused) = decision.denial {
                denial = Some(format!(
                    "Hook denied {}: {}",
                    request.call.name, refused.reason
                ));
            }
            if let Some(args) = decision.updated_args {
                request.call.args = args;
            }
        }
    }
    let effective_digest = a3s_effect::tool_args_digest(&request.call.args)?;
    if argument_error.is_none() && denial.is_none() {
        argument_error = tools
            .executor
            .registry()
            .validate_arguments(&request.call.name, &request.call.args)
            .err();
    }
    let mut availability_rejected_after_hook = false;
    if denial.is_none()
        && argument_error.is_none()
        && (hook_evaluated || effective_digest != original_digest)
    {
        if let Some(tool) = tools.executor.registry().get(&request.call.name) {
            if let Err(error) =
                tool.validate_execution_available(&request.call.args, &tools.context)
            {
                denial = Some(error.to_string());
                availability_rejected_after_hook = hook_evaluated;
            }
        }
    }
    let effective_permission = execution_permission(
        &tools.permission,
        tools.checker.as_deref(),
        &request.call.name,
        &request.call.args,
    );
    let read_only = tools
        .executor
        .registry()
        .capabilities(&request.call.name, &request.call.args)
        .is_some_and(|capabilities| capabilities.read_only);
    if !read_only
        && tools.ledger.as_ref().is_some_and(|ledger| {
            ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .has_uncertain_tool_invocations()
        })
    {
        denial = Some("tool_effect_unresolved: mutating calls require reconciliation of earlier uncertain invocations".into());
    }
    // The actor already settled any confirmation attached to this exact
    // original call. A false needs_confirmation also covers the host's
    // disabled-HITL lane or a manager that does not require this call to
    // park; it is not proof that the call was refused. Recheck that same
    // authority when no actor confirmation was required. Rewritten Ask
    // arguments never inherit approval of the original arguments.
    let unconfirmed_ask =
        if effective_permission == PermissionDecision::Ask && !request.call.needs_confirmation {
            match tools.agent.as_ref() {
                Some(agent) => match agent.confirmation_provider() {
                    Some(manager) => {
                        manager
                            .requires_confirmation_for(&request.call.name, &request.call.args)
                            .await
                    }
                    None => false,
                },
                None => true,
            }
        } else {
            false
        };
    if effective_permission == PermissionDecision::Deny
        || (effective_permission == PermissionDecision::Ask
            && (effective_digest != original_digest || unconfirmed_ask))
    {
        denial = Some("tool_effect_effective_arguments_unapproved: resolved input is not approved by the frozen permission policy".into());
    }
    if let Some(run_id) = &tools.run_id {
        let checkpoint_binding = tools
            .checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.capability_binding.clone());
        let stored_binding = match &tools.run_store {
            Some(store) => store
                .snapshot(run_id)
                .await
                .and_then(|snapshot| snapshot.capability_binding),
            None => None,
        };
        if let (Some(checkpoint), Some(stored)) = (&checkpoint_binding, &stored_binding) {
            if checkpoint != stored {
                return Err(ActorError::Schema(
                    "tool_effect_admission_drift: checkpoint and Run capability bindings differ"
                        .into(),
                ));
            }
        }
        let binding = checkpoint_binding.or(stored_binding).ok_or_else(|| {
            ActorError::Schema(
                "tool_effect_admission_missing: admitted Run has no frozen capability binding"
                    .into(),
            )
        })?;
        binding
            .validate()
            .map_err(|error| ActorError::Schema(error.to_string()))?;
        let digest =
            crate::content_digest::digest_json("a3s.code.tool-admission-binding.v1", &binding)
                .map_err(|error| ActorError::Schema(error.to_string()))?;
        request.identity.admission = Some(ToolAdmissionIdentityV1 {
            run_id: run_id.clone(),
            thread_id: thread_for_session(tools.context.session_id.as_deref().unwrap_or(THREAD)),
            capability_binding_digest: digest,
        });
    }
    let local = tools.context.workspace_services.local_root().is_some();
    let watch = if denial.is_none()
        && argument_error.is_none()
        && !cancel.is_cancelled()
        && local
        && !read_only
    {
        Some(crate::porcelain::Watch::start(tools.context.workspace.as_path()).await)
    } else {
        None
    };
    let prepared = ToolPreparedV1 {
        schema: TOOL_PREPARED_SCHEMA.into(),
        identity: request.identity,
        tool_name: request.call.name.clone(),
        original_args_digest: original_digest,
        effective_args_digest: effective_digest,
        recovery: ToolRecoveryPolicy::NeverReplay,
        baseline: watch
            .as_ref()
            .map(crate::porcelain::Watch::receipt_baseline),
    };
    prepared.validate()?;
    let hook_effects_unobserved =
        (hook_evaluated && argument_error.is_some()) || availability_rejected_after_hook;
    // Permission, skill and hook refusals remain final even if input also
    // violates the schema. Schema rejection never suppresses a denial event.
    let rejection = if let Some(output) = denial {
        tools
            .emit(crate::agent::AgentEvent::PermissionDenied {
                tool_id: request.call.id.clone(),
                tool_name: request.call.name.clone(),
                args: request.call.args.clone(),
                reason: output.clone(),
            })
            .await;
        Some((ToolExecutionOutcome::Denied, output))
    } else {
        argument_error.map(|message| {
            (
                ToolExecutionOutcome::Failed,
                format!(
                    "Invalid arguments for tool '{}': {} [permanent - change the arguments before retrying]",
                    request.call.name, message
                ),
            )
        })
    };
    if let Some((outcome, output)) = rejection {
        let mut result = result_without_execution(&prepared, outcome, output)?;
        if hook_effects_unobserved {
            // The tool body did not run, but an already evaluated input hook
            // is not an independently observed zero-effect boundary.
            result.effects = ToolEffectObservationV1::incomplete(
                "tool_effect_observation_incomplete: input hook ran before effective invocation was rejected",
            );
            result
                .effects
                .unresolved_external_operations
                .push(prepared.identity.key()?);
        }
        return Ok(PreparedToolExecution {
            prepared,
            execute: Effect::succeed(result),
        });
    }
    if cancel.is_cancelled() {
        return terminal_without_execution(
            prepared,
            ToolExecutionOutcome::Cancelled,
            "cancelled before tool execution".into(),
        );
    }
    let mut executor = tools.as_ref().clone();
    executor.pre_tool_hook_resolved = true;
    let executor = Arc::new(executor);
    let execution_prepared = prepared.clone();
    let consumed = Arc::new(AtomicBool::new(false));
    let execute = Effect::from_async_observed(move |_services, cancel| {
        let tools = Arc::clone(&executor);
        let call = request.call.clone();
        let prepared = execution_prepared.clone();
        let consumed = Arc::clone(&consumed);
        async move {
            if consumed.swap(true, Ordering::SeqCst) {
                return ToolExecutionResultV1::unknown(
                    &prepared,
                    "prepared continuation was already evaluated",
                )
                .map_err(Exit::Fail);
            }
            if cancel.is_cancelled() {
                return result_without_execution(
                    &prepared,
                    ToolExecutionOutcome::Cancelled,
                    "cancelled before tool execution".into(),
                )
                .map_err(Exit::Fail);
            }
            if !read_only
                && tools.ledger.as_ref().is_some_and(|ledger| {
                    ledger
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .has_uncertain_tool_invocations()
                })
            {
                return result_without_execution(&prepared, ToolExecutionOutcome::Denied,
                    "tool_effect_unresolved: an earlier call became uncertain after batch preparation".into())
                    .map_err(Exit::Fail);
            }
            // A batch is prepared durably before any call executes. Its
            // prepared baseline is recovery evidence, not a per-call delta:
            // take this live boundary after the preceding call has settled.
            // Read-only contracts never acquire another call's mutations.
            let observation = if local && !read_only {
                Some(crate::porcelain::Watch::start(tools.context.workspace.as_path()).await)
            } else {
                None
            };
            // Preserve a returned terminal result even if cancellation races
            // it. An interrupted execution remains explicitly uncertain.
            let output = tools.run(call.clone(), cancel.clone()).await;
            let delta = match observation {
                Some(watch) => {
                    watch
                        .finish_observed_with_revision(tools.context.workspace.as_path(), true)
                        .await
                }
                None => crate::porcelain::PathDelta {
                    paths: Vec::new(),
                    incomplete: !read_only,
                },
            };
            let mut effects =
                crate::porcelain::observe_path_effects(tools.context.workspace.as_path(), delta);
            let (output, outcome, exit_code, child, source_incomplete) = match output {
                Ok(value) => {
                    let envelope = value.as_str().and_then(decode_fact_tool_result);
                    match envelope {
                        Some(envelope) => (
                            value,
                            envelope.outcome.unwrap_or(if envelope.is_error {
                                ToolExecutionOutcome::Failed
                            } else {
                                ToolExecutionOutcome::Succeeded
                            }),
                            envelope.exit_code,
                            envelope.workspace_child,
                            envelope.observation_incomplete,
                        ),
                        None if cancel.is_cancelled() => {
                            (value, ToolExecutionOutcome::Cancelled, None, None, false)
                        }
                        None => (value, ToolExecutionOutcome::Denied, None, None, false),
                    }
                }
                Err(error) => (
                    serde_json::Value::String(error.to_string()),
                    ToolExecutionOutcome::Unknown,
                    None,
                    None,
                    true,
                ),
            };
            if source_incomplete {
                effects.completeness = ToolObservationCompleteness::Incomplete;
                effects.reason = Some("tool_effect_observation_incomplete: execution did not return complete evidence".into());
            }
            if matches!(
                outcome,
                ToolExecutionOutcome::Cancelled | ToolExecutionOutcome::Unknown
            ) && !read_only
            {
                effects
                    .unresolved_external_operations
                    .push(prepared.identity.key().map_err(Exit::Fail)?);
            }
            // A live workspace child is tracked separately until its durable
            // background terminal fact commits. Its id is exact bookkeeping.
            if let Some(child) = child {
                effects
                    .unresolved_external_operations
                    .push(format!("workspace-child:{child}"));
            }
            let mut result = ToolExecutionResultV1 {
                schema: TOOL_RESULT_SCHEMA.into(),
                identity: prepared.identity.clone(),
                prepared_digest: prepared.digest().map_err(Exit::Fail)?,
                outcome,
                exit_code,
                output,
                effects,
            };
            result.ensure_prepared(&prepared).map_err(Exit::Fail)?;
            if let Some(ledger) = &tools.ledger {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observe_tool_receipt(&prepared.tool_name, &result);
            }
            // A verifier must bind the file identity established above, not
            // the legacy metadata row produced before the byte observation.
            if let (Some(agent), Some(ledger), Some(code), Some(mut envelope)) = (
                tools.agent.as_ref(),
                tools.ledger.as_ref(),
                result.exit_code,
                result.output.as_str().and_then(decode_fact_tool_result),
            ) {
                let mut projected = ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let mut reports = Vec::new();
                let verification_call = crate::llm::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.clone(),
                };
                agent
                    .record_prepared_tool_verification(
                        &verification_call,
                        code,
                        &mut envelope.text,
                        &mut envelope.metadata,
                        &mut projected,
                        &mut reports,
                    )
                    .await;
                // Only reports from this execution may acquire this digest.
                // Do not rebind a previous call's reports to a later effect.
                if let Some(current) = &tools.reports {
                    let mut current = current
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    for report in reports {
                        if !current.contains(&report) {
                            current.push(report);
                        }
                    }
                }
                result.output = serde_json::Value::String(
                    serde_json::to_string(&envelope)
                        .map_err(|error| Exit::Fail(ActorError::Schema(error.to_string())))?,
                );
            }
            Ok(result)
        }
    });
    Ok(PreparedToolExecution { prepared, execute })
}

fn terminal_without_execution(
    prepared: ToolPreparedV1,
    outcome: ToolExecutionOutcome,
    output: String,
) -> Result<PreparedToolExecution, ActorError> {
    let result = result_without_execution(&prepared, outcome, output)?;
    Ok(PreparedToolExecution {
        prepared,
        execute: Effect::succeed(result),
    })
}

fn result_without_execution(
    prepared: &ToolPreparedV1,
    outcome: ToolExecutionOutcome,
    output: String,
) -> Result<ToolExecutionResultV1, ActorError> {
    Ok(ToolExecutionResultV1 {
        schema: TOOL_RESULT_SCHEMA.into(),
        identity: prepared.identity.clone(),
        prepared_digest: prepared.digest()?,
        outcome,
        exit_code: None,
        output: serde_json::Value::String(output),
        effects: ToolEffectObservationV1::complete(Vec::new()),
    })
}

#[derive(Default)]
pub(super) struct ReceiptDebt {
    pub paths: Vec<String>,
    pub results: Vec<(String, ToolExecutionResultV1)>,
    pub background_results: Vec<(String, ToolEffectObservationV1)>,
    pub projection_order: Vec<ReceiptProjection>,
    pub unresolved: std::collections::BTreeSet<String>,
    /// Malformed, conflicting, identity-mismatched or unsupported original
    /// evidence cannot prove a cache. This is independent of honest Unknown
    /// outcomes, which continue to block completion through `unresolved`.
    pub integrity_invalid: bool,
    /// Exact uncertainty asserted by validated original typed facts. Never
    /// derive this set by classifying the human-readable debt strings.
    pub validated_uncertainty: std::collections::BTreeSet<String>,
    cache_proof_events: Vec<CacheProofEvent>,
    validated_uncertainty_sources: Vec<ValidatedUncertaintySource>,
    prepared_fact_indices: std::collections::BTreeMap<String, usize>,
    terminal_fact_indices: std::collections::BTreeMap<String, usize>,
    background_required_proof_indices: std::collections::BTreeMap<String, usize>,
    settled_child_proof_indices: std::collections::BTreeMap<String, usize>,
    tool_fact_indices: Vec<usize>,
    background_fact_indices: Vec<usize>,
    pub settled_workspace_children: std::collections::BTreeSet<String>,
}

struct CacheProofEvent {
    fact_index: usize,
    operation: CacheProofOperation,
}

enum CacheProofOperation {
    Prepared(String),
    Terminal(String),
    AssertedUncertainty(String),
    BackgroundReady(String),
    ChildSettled(String),
}

impl CacheProofOperation {
    fn phase(&self) -> u8 {
        match self {
            Self::Prepared(_) => 0,
            Self::Terminal(_) => 1,
            Self::AssertedUncertainty(_) | Self::BackgroundReady(_) => 2,
            Self::ChildSettled(_) => 3,
        }
    }
}

struct ValidatedUncertaintySource {
    fact_index: usize,
    key: String,
    background_fact_key: Option<String>,
}

impl ReceiptDebt {
    fn add_validated_uncertainty(&mut self, fact_index: usize, key: String) {
        self.unresolved.insert(key.clone());
        self.validated_uncertainty.insert(key.clone());
        self.validated_uncertainty_sources
            .push(ValidatedUncertaintySource {
                fact_index,
                key,
                background_fact_key: None,
            });
    }

    fn add_validated_background_uncertainty(
        &mut self,
        fact_index: usize,
        fact_key: &str,
        key: String,
    ) {
        self.unresolved.insert(key.clone());
        self.validated_uncertainty.insert(key.clone());
        self.validated_uncertainty_sources
            .push(ValidatedUncertaintySource {
                fact_index,
                key,
                background_fact_key: Some(fact_key.into()),
            });
    }

    fn projection_fact_index(&self, projection: &ReceiptProjection) -> Option<usize> {
        match projection {
            ReceiptProjection::Tool(index) => self.tool_fact_indices.get(*index).copied(),
            ReceiptProjection::Background(index) => {
                self.background_fact_indices.get(*index).copied()
            }
        }
    }

    pub(super) fn cache_proof_cursor(
        &self,
        claimed_uncertainty: &std::collections::BTreeSet<String>,
        claimed_background_keys: &std::collections::BTreeSet<String>,
    ) -> CacheProofCursor<'_> {
        CacheProofCursor::new(self, claimed_uncertainty, claimed_background_keys)
    }

    /// Compile the already-validated typed sources once. Source indices are
    /// proof boundaries, not a reordering of the original effect projection.
    fn build_cache_proof_events(&mut self) {
        for (key, fact_index) in std::mem::take(&mut self.prepared_fact_indices) {
            self.cache_proof_events.push(CacheProofEvent {
                fact_index,
                operation: CacheProofOperation::Prepared(key),
            });
        }
        for (key, fact_index) in std::mem::take(&mut self.terminal_fact_indices) {
            self.cache_proof_events.push(CacheProofEvent {
                fact_index,
                operation: CacheProofOperation::Terminal(key),
            });
        }
        for source in std::mem::take(&mut self.validated_uncertainty_sources) {
            let required_index = match source.background_fact_key.as_ref() {
                Some(key) => self.background_required_proof_indices.get(key).copied(),
                None => Some(source.fact_index),
            };
            if let Some(fact_index) = required_index {
                self.cache_proof_events.push(CacheProofEvent {
                    fact_index,
                    operation: CacheProofOperation::AssertedUncertainty(source.key),
                });
            }
        }
        for (key, fact_index) in std::mem::take(&mut self.background_required_proof_indices) {
            self.cache_proof_events.push(CacheProofEvent {
                fact_index,
                operation: CacheProofOperation::BackgroundReady(key),
            });
        }
        for (key, fact_index) in std::mem::take(&mut self.settled_child_proof_indices) {
            self.cache_proof_events.push(CacheProofEvent {
                fact_index,
                operation: CacheProofOperation::ChildSettled(key),
            });
        }
        self.cache_proof_events
            .sort_by_key(|event| (event.fact_index, event.operation.phase()));
    }

    // Small random-access diagnostic pins use the same production cursor;
    // recovery itself never rebuilds/scans this set at every prefix.
    #[cfg(test)]
    pub(super) fn validated_uncertainty_for(
        &self,
        projection: &ReceiptProjection,
    ) -> std::collections::BTreeSet<String> {
        let claims = self
            .cache_proof_events
            .iter()
            .filter_map(|event| match &event.operation {
                CacheProofOperation::Prepared(key)
                | CacheProofOperation::Terminal(key)
                | CacheProofOperation::AssertedUncertainty(key)
                | CacheProofOperation::ChildSettled(key) => Some(key.clone()),
                CacheProofOperation::BackgroundReady(_) => None,
            })
            .collect();
        let mut cursor = self.cache_proof_cursor(&claims, &std::collections::BTreeSet::new());
        cursor.advance_to(projection);
        cursor.into_proven_uncertainty()
    }

    #[cfg(test)]
    pub(super) fn background_proof_ready_for(
        &self,
        projection: &ReceiptProjection,
        claimed_background_keys: &std::collections::BTreeSet<String>,
    ) -> bool {
        self.cache_proof_cursor(&std::collections::BTreeSet::new(), claimed_background_keys)
            .advance_to(projection)
    }

    fn invalidate_evidence(&mut self, key: String) {
        self.integrity_invalid = true;
        self.unresolved.insert(key);
    }
}

pub(super) enum ReceiptProjection {
    Tool(usize),
    Background(usize),
}

/// A turn marker bounds verified local changes. It cannot erase uncertainty
/// about an invocation that never acquired an identity-bound terminal result.
pub(super) fn fold_debt(facts: &[a3s_effect::Fact]) -> ReceiptDebt {
    fold_receipts(facts, false)
}

/// The cache proof needs the same validation as recovery, including exact
/// typed terminals already covered by a turn marker. It never changes the
/// marker's ordinary unsettled-debt projection.
pub(super) fn fold_cache_evidence(facts: &[a3s_effect::Fact]) -> ReceiptDebt {
    fold_receipts(facts, true)
}

fn fold_receipts(facts: &[a3s_effect::Fact], include_covered: bool) -> ReceiptDebt {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == TURN_COMPLETE_FACT_KIND)
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut debt = ReceiptDebt::default();
    let covered_receipts = facts
        .iter()
        .filter(|fact| fact.kind == TURN_COMPLETE_FACT_KIND)
        .flat_map(|fact| {
            fact.payload
                .get("receipt_keys")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|value| value.as_str())
        .collect::<BTreeSet<_>>();
    let covered_background = facts
        .iter()
        .filter(|fact| fact.kind == TURN_COMPLETE_FACT_KIND)
        .flat_map(|fact| {
            fact.payload
                .get("background_receipt_keys")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|value| value.as_str())
        .collect::<BTreeSet<_>>();
    let mut prepared: BTreeMap<String, ToolPreparedV1> = BTreeMap::new();
    let mut terminals: BTreeMap<String, ToolExecutionResultV1> = BTreeMap::new();
    let mut legacy_calls: BTreeMap<String, RecordedCall> = BTreeMap::new();
    let mut legacy_results = BTreeSet::new();
    let mut active_calls = VecDeque::new();
    let mut confirmation_calls = BTreeSet::new();
    let mut refused_calls = BTreeSet::new();
    let mut protocol_one_calls = BTreeSet::new();
    let mut settled_children = BTreeSet::new();
    let mut child_bindings = BTreeMap::new();
    let mut child_binding_fact_indices = BTreeMap::new();
    let mut background_bindings = BTreeMap::new();
    let mut background_proof_requirements = Vec::new();
    let mut settled_background_claims = BTreeSet::new();
    for (index, fact) in facts.iter().enumerate() {
        match fact.kind.as_str() {
            super::recovery_unproved::RECOVERY_UNPROVED_FACT_KIND => {
                // A provenance gap is a host assertion, not execution or
                // verification evidence. Fold it over the entire log; no
                // turn marker or later receipt can discharge this v1 debt.
                let key = super::recovery_unproved::RecoveryUnprovedV1::validate_stored_fact(fact)
                    .and_then(|input| input.key());
                match key {
                    Ok(key) => debt.add_validated_uncertainty(index, key),
                    Err(error) => debt.invalidate_evidence(format!(
                        "invalid-recovery-unproved:{}:{}",
                        fact.seq,
                        error.code()
                    )),
                }
            }
            "tool.prepared" => {
                let parsed = fact
                    .payload
                    .get("prepared")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<ToolPreparedV1>(value).ok());
                match parsed.and_then(|value| value.validate().ok().map(|_| value)) {
                    Some(value) => match value.identity.key() {
                        Ok(key) => {
                            if prepared.get(&key).is_some_and(|earlier| earlier != &value) {
                                debt.invalidate_evidence(format!("conflicting-preparation:{key}"));
                            }
                            debt.prepared_fact_indices
                                .entry(key.clone())
                                .or_insert(index);
                            prepared.insert(key, value);
                        }
                        Err(_) => {
                            debt.invalidate_evidence(format!("invalid-preparation:{}", fact.seq));
                        }
                    },
                    None => {
                        debt.invalidate_evidence(format!("invalid-preparation:{}", fact.seq));
                    }
                }
            }
            "tool.result" if fact.payload.get("receipt").is_some() => {
                let parsed = serde_json::from_value::<ToolExecutionResultV1>(
                    fact.payload["receipt"].clone(),
                );
                match parsed {
                    Ok(result) => {
                        let key = result.identity.key();
                        let valid = key
                            .as_ref()
                            .ok()
                            .and_then(|key| prepared.get(key))
                            .filter(|value| result.ensure_prepared(value).is_ok());
                        if let (Ok(key), Some(value)) = (key, valid) {
                            let display_key = format!(
                                "tool-result:{}:{}:{}",
                                result.identity.turn,
                                result
                                    .identity
                                    .cycle
                                    .saturating_add(u64::from(result.identity.batch_index)),
                                result.identity.tool_call_id
                            );
                            active_calls.retain(|active| active != &display_key);
                            if terminals
                                .get(&key)
                                .is_some_and(|earlier| earlier != &result)
                            {
                                debt.invalidate_evidence(format!("conflicting-terminal:{key}"));
                            }
                            debt.terminal_fact_indices
                                .entry(key.clone())
                                .or_insert(index);
                            if result.outcome == ToolExecutionOutcome::Unknown
                                || result.effects.completeness
                                    == ToolObservationCompleteness::Incomplete
                            {
                                debt.add_validated_uncertainty(index, key.clone());
                            }
                            for operation in &result.effects.unresolved_external_operations {
                                debt.add_validated_uncertainty(index, operation.clone());
                            }
                            let envelope = result.output.as_str().and_then(decode_fact_tool_result);
                            let source_run = envelope
                                .as_ref()
                                .and_then(|envelope| envelope.metadata.as_ref())
                                .and_then(|metadata| metadata.get("source_run"))
                                .and_then(|value| value.as_str());
                            let effect_run = envelope
                                .as_ref()
                                .and_then(|envelope| envelope.metadata.as_ref())
                                .and_then(|metadata| metadata.get("run"))
                                .and_then(|value| value.as_str());
                            for child in result
                                .effects
                                .unresolved_external_operations
                                .iter()
                                .filter(|key| key.starts_with("workspace-child:"))
                            {
                                let binding = result
                                    .identity
                                    .admission
                                    .as_ref()
                                    .filter(|admission| {
                                        Some(admission.run_id.as_str()) == source_run
                                    })
                                    .filter(|_| {
                                        envelope.as_ref().and_then(|envelope| {
                                            envelope.workspace_child.as_deref()
                                        }) == child.strip_prefix("workspace-child:")
                                    })
                                    .and_then(|_| {
                                        Some((source_run?.to_string(), effect_run?.to_string()))
                                    });
                                if child_bindings
                                    .get(child)
                                    .is_some_and(|previous| previous != &binding)
                                {
                                    debt.invalidate_evidence(format!(
                                        "background-binding-conflict:{child}"
                                    ));
                                }
                                if binding.is_some() {
                                    child_binding_fact_indices
                                        .entry(child.clone())
                                        .or_insert(index);
                                }
                                child_bindings.insert(child.clone(), binding);
                            }
                            if include_covered || !covered_receipts.contains(key.as_str()) {
                                debt.paths.extend(
                                    result
                                        .effects
                                        .changes
                                        .iter()
                                        .map(|change| change.path.clone()),
                                );
                                debt.projection_order
                                    .push(ReceiptProjection::Tool(debt.results.len()));
                                debt.tool_fact_indices.push(index);
                                debt.results.push((value.tool_name.clone(), result.clone()));
                            }
                            terminals.insert(key, result);
                        } else {
                            debt.invalidate_evidence(format!(
                                "tool_effect_identity_mismatch:{}",
                                fact.seq
                            ));
                        }
                    }
                    Err(_) => {
                        debt.invalidate_evidence(format!(
                            "tool_effect_schema_unsupported:{}",
                            fact.seq
                        ));
                    }
                }
            }
            "model.turn" if index >= start && fact.payload["kind"] == "tool" => {
                // Mechanical legacy-key parsing never decides tool semantics.
                if let Some((turn, cycle)) = model_position(&fact.key) {
                    active_calls.clear();
                    let calls = fact.payload.get("call").into_iter().chain(
                        fact.payload
                            .get("also")
                            .and_then(|value| value.as_array())
                            .into_iter()
                            .flatten(),
                    );
                    for (offset, call) in calls.enumerate() {
                        let requires_confirmation = call
                            .get("needs_confirmation")
                            .and_then(|value| value.as_bool())
                            == Some(true);
                        let call = recorded_call_from(call);
                        let key = format!(
                            "tool-result:{turn}:{}:{}",
                            cycle.saturating_add(offset as u64),
                            call.id
                        );
                        active_calls.push_back(key.clone());
                        if fact
                            .payload
                            .get("tool_effect_protocol")
                            .and_then(|value| value.as_u64())
                            == Some(1)
                        {
                            protocol_one_calls.insert(key.clone());
                        }
                        if requires_confirmation {
                            confirmation_calls.insert(key.clone());
                        }
                        legacy_calls.insert(key, call);
                    }
                } else {
                    debt.invalidate_evidence(format!("legacy-invocation:{}", fact.seq));
                }
            }
            "tool.result"
                if index >= start
                    || fact
                        .payload
                        .get("unresolvedLegacy")
                        .and_then(|value| value.as_bool())
                        == Some(true) =>
            {
                active_calls.retain(|active| active != &fact.key);
                legacy_results.insert(fact.key.clone());
                debt.invalidate_evidence(format!("legacy-result:{}", fact.seq));
                if let Some(call) = legacy_calls.get(&fact.key) {
                    if fact.payload.get("ok").and_then(|value| value.as_bool()) == Some(true)
                        && crate::harness_loop::MUTATING_FILE_TOOLS.contains(&call.name.as_str())
                    {
                        if let Some(path) =
                            call.args.get("file_path").and_then(|value| value.as_str())
                        {
                            debt.paths.push(path.to_string());
                        }
                    }
                }
            }
            "confirmation.answered"
                if index >= start
                    && fact
                        .payload
                        .get("approved")
                        .and_then(|value| value.as_bool())
                        == Some(false) =>
            {
                if let Some(key) = active_calls.front() {
                    let exact = legacy_calls.get(key).is_some_and(|call| {
                        fact.payload
                            .get("tool_call_id")
                            .and_then(|value| value.as_str())
                            == Some(call.id.as_str())
                    }) && confirmation_calls.contains(key);
                    if exact {
                        if let Some(key) = active_calls.pop_front() {
                            refused_calls.insert(key);
                        }
                    }
                }
            }
            "budget.denied" if index >= start => {
                if let Some(key) = active_calls.front() {
                    let expected = key.replacen("tool-result:", "budget-denied:", 1);
                    let exact = fact.key == expected
                        && legacy_calls.get(key).is_some_and(|call| {
                            fact.payload
                                .get("toolCallId")
                                .and_then(|value| value.as_str())
                                == Some(call.id.as_str())
                        });
                    if exact {
                        // The scheduler stops this whole outstanding queue.
                        // These calls have a proven zero-execution boundary.
                        refused_calls.extend(active_calls.drain(..));
                    }
                }
            }
            "background.output" if fact.payload["event"] == "exited" => {
                let id = fact
                    .payload
                    .get("workspace_child")
                    .and_then(|value| value.as_str());
                let paths = fact
                    .payload
                    .get("changed_paths")
                    .and_then(|value| value.as_array());
                if let (Some(id), Some(paths)) = (id, paths) {
                    let observation = fact
                        .payload
                        .get("effects")
                        .cloned()
                        .and_then(|value| {
                            serde_json::from_value::<ToolEffectObservationV1>(value).ok()
                        })
                        .filter(|effects| effects.validate().is_ok());
                    let path_set = paths
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<BTreeSet<_>>();
                    let paths_match = observation.as_ref().is_some_and(|effects| {
                        path_set.len() == paths.len()
                            && effects
                                .changes
                                .iter()
                                .map(|change| change.path.as_str())
                                .collect::<BTreeSet<_>>()
                                == path_set
                    });
                    let evidence_valid = !id.is_empty()
                        && !fact.key.trim().is_empty()
                        && observation.is_some()
                        && paths_match
                        && fact.payload.get("task_id").and_then(|value| value.as_str()) == Some(id)
                        && background_terminal_fields_valid(&fact.payload);
                    if !evidence_valid {
                        debt.invalidate_evidence(format!("background-evidence:{id}"));
                    }
                    let complete = background_terminal_settled(&fact.payload)
                        && observation.as_ref().is_some_and(|effects| {
                            effects.completeness == ToolObservationCompleteness::Complete
                                && effects.unresolved_external_operations.is_empty()
                        })
                        && paths_match
                        && fact.payload.get("task_id").and_then(|value| value.as_str()) == Some(id);
                    if evidence_valid {
                        let binding = fact
                            .payload
                            .get("source_run")
                            .and_then(|value| value.as_str())
                            .zip(fact.payload.get("run").and_then(|value| value.as_str()))
                            .map(|(source, effect)| (source.to_string(), effect.to_string()));
                        let child = format!("workspace-child:{id}");
                        if background_bindings
                            .get(&child)
                            .is_some_and(|previous| previous != &binding)
                        {
                            debt.invalidate_evidence(format!("background-terminal-conflict:{id}"));
                        }
                        if complete {
                            settled_background_claims.insert(child.clone());
                        } else {
                            debt.add_validated_background_uncertainty(
                                index,
                                &fact.key,
                                format!("background-evidence:{id}"),
                            );
                        }
                        background_proof_requirements.push((
                            fact.key.clone(),
                            index,
                            child.clone(),
                            complete,
                        ));
                        background_bindings.insert(child, binding);
                        if let Some(effects) = observation.as_ref() {
                            if effects.completeness == ToolObservationCompleteness::Incomplete {
                                debt.add_validated_background_uncertainty(
                                    index,
                                    &fact.key,
                                    format!("background-receipt-incomplete:{}", fact.key),
                                );
                            }
                            for operation in &effects.unresolved_external_operations {
                                debt.add_validated_background_uncertainty(
                                    index,
                                    &fact.key,
                                    operation.clone(),
                                );
                            }
                        }
                    }
                    if include_covered || !covered_background.contains(fact.key.as_str()) {
                        if let Some(effects) = observation {
                            debt.projection_order
                                .push(ReceiptProjection::Background(debt.background_results.len()));
                            debt.background_fact_indices.push(index);
                            debt.background_results.push((fact.key.clone(), effects));
                        }
                        for path in paths {
                            match path.as_str() {
                                Some(path) => debt.paths.push(path.to_string()),
                                None => {
                                    debt.invalidate_evidence(format!("background-path:{id}"));
                                }
                            }
                        }
                    }
                } else {
                    debt.invalidate_evidence(format!("background-terminal-invalid:{}", fact.seq));
                }
            }
            _ => {}
        }
    }
    for (child, binding) in &background_bindings {
        if binding.is_some() && child_bindings.get(child) == Some(binding) {
            if settled_background_claims.contains(child) {
                settled_children.insert(child.clone());
            }
        } else {
            debt.invalidate_evidence(format!("background-terminal-unbound:{child}"));
        }
    }
    for (fact_key, terminal_at, child, complete) in background_proof_requirements {
        let binding_valid = background_bindings.get(&child).is_some_and(|binding| {
            binding.is_some() && child_bindings.get(&child) == Some(binding)
        });
        if let Some(spawn_at) = child_binding_fact_indices
            .get(&child)
            .filter(|_| binding_valid)
        {
            let required = terminal_at.max(*spawn_at);
            debt.background_required_proof_indices
                .insert(fact_key, required);
            if complete {
                debt.settled_child_proof_indices
                    .entry(child)
                    .and_modify(|earlier| *earlier = (*earlier).min(required))
                    .or_insert(required);
            }
        }
    }
    for (key, value) in &prepared {
        if !terminals.contains_key(key) {
            // The final debt remains sticky, while prefix proof uses the
            // actual preparation/terminal interval rather than an assertion.
            debt.unresolved.insert(key.clone());
            debt.validated_uncertainty.insert(key.clone());
        }
        // Typed preparations supersede only their exact legacy display key.
        let call_key = format!(
            "tool-result:{}:{}:{}",
            value.identity.turn,
            value
                .identity
                .cycle
                .saturating_add(u64::from(value.identity.batch_index)),
            value.identity.tool_call_id
        );
        legacy_calls.remove(&call_key);
        legacy_calls.remove(&format!(
            "tool-result:{}:{}:{}",
            value.identity.turn, value.identity.cycle, value.identity.tool_call_id
        ));
    }
    for key in legacy_calls.keys().filter(|key| {
        !legacy_results.contains(*key)
            && !refused_calls.contains(*key)
            && !protocol_one_calls.contains(*key)
    }) {
        debt.invalidate_evidence(format!("legacy-pending:{key}"));
    }
    for key in &settled_children {
        debt.unresolved.remove(key);
        debt.validated_uncertainty.remove(key);
    }
    debt.settled_workspace_children = settled_children;
    debt.paths
        .retain(|path| !crate::porcelain::is_harness_path(path));
    debt.paths.sort();
    debt.paths.dedup();
    if include_covered {
        debt.build_cache_proof_events();
    }
    debt
}

/// Schema integrity is independent of whether these typed flags prove
/// settlement. A valid `unknown` disposition or `unproven` basis is honest
/// uncertainty; missing/unknown fields cannot prove cached provenance.
fn background_terminal_fields_valid(payload: &serde_json::Value) -> bool {
    use crate::tools::builtin::background::{BackgroundDisposition, ProcessSettlementBasis};
    [
        "process_settled",
        "execution_future_released",
        "output_facts_retained",
        "observation_incomplete",
        "timed_out",
        "cancelled",
        "output_complete",
    ]
    .iter()
    .all(|name| {
        payload
            .get(name)
            .and_then(|value| value.as_bool())
            .is_some()
    }) && payload
        .get("disposition")
        .and_then(|value| value.as_str())
        .is_some_and(|value| BackgroundDisposition::parse(value).is_ok())
        && payload
            .get("process_settlement_basis")
            .and_then(|value| value.as_str())
            .is_some_and(|value| ProcessSettlementBasis::parse(value).is_ok())
        && payload.get("exit_code").is_some_and(|value| {
            value.is_null()
                || value
                    .as_i64()
                    .is_some_and(|code| code >= 0 && i32::try_from(code).is_ok())
        })
        && ["source_run", "run"].iter().all(|name| {
            payload
                .get(name)
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty())
        })
}

/// Output capture and process settlement are separate facts. A cancelled
/// process may have partial output, but only the opted-in backend's explicit
/// drop contract plus complete workspace/evidence settlement can clear it.
fn background_terminal_settled(payload: &serde_json::Value) -> bool {
    use crate::tools::builtin::background::{BackgroundDisposition, ProcessSettlementBasis};
    let disposition = payload
        .get("disposition")
        .and_then(|value| value.as_str())
        .and_then(|value| BackgroundDisposition::parse(value).ok());
    let basis = payload
        .get("process_settlement_basis")
        .and_then(|value| value.as_str())
        .and_then(|value| ProcessSettlementBasis::parse(value).ok());
    let flag = |name| payload.get(name).and_then(|value| value.as_bool());
    if flag("process_settled") != Some(true)
        || flag("execution_future_released") != Some(true)
        || flag("output_facts_retained") != Some(true)
        || flag("observation_incomplete") != Some(false)
        || flag("timed_out") != Some(false)
    {
        return false;
    }
    match (disposition, basis) {
        (
            Some(BackgroundDisposition::Completed),
            Some(ProcessSettlementBasis::ExecutionReturned),
        ) => {
            flag("cancelled") == Some(false)
                && flag("output_complete") == Some(true)
                && payload
                    .get("exit_code")
                    .and_then(|value| value.as_i64())
                    .is_some_and(|code| code >= 0 && i32::try_from(code).is_ok())
        }
        (
            Some(BackgroundDisposition::Cancelled),
            Some(ProcessSettlementBasis::BackendDropContract),
        ) => {
            flag("cancelled") == Some(true)
                && flag("output_complete") == Some(false)
                && payload
                    .get("exit_code")
                    .is_some_and(|value| value.is_null())
        }
        (Some(BackgroundDisposition::Failed), Some(ProcessSettlementBasis::AdmissionRefused)) => {
            // A typed pre-spawn refusal has no exit status to report.
            flag("cancelled") == Some(false)
                && flag("output_complete") == Some(false)
                && payload
                    .get("exit_code")
                    .is_some_and(|value| value.is_null())
        }
        _ => false,
    }
}

fn model_position(key: &str) -> Option<(u64, u64)> {
    let mut parts = key.split(':');
    if parts.next()? != "model" {
        return None;
    }
    let turn = parts.next()?.parse().ok()?;
    let cycle = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((turn, cycle))
}

#[cfg(test)]
mod tests;

pub(super) fn observe_live_child(
    ledger: &Arc<Mutex<crate::harness_loop::MutationLedger>>,
    metadata: Option<&serde_json::Value>,
) {
    if let Some(child) = metadata
        .and_then(|value| value.get("workspace_child"))
        .and_then(|value| value.as_str())
    {
        ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observe_tool(
                "background",
                0,
                Some(&serde_json::json!({"workspace_child": child})),
            );
    }
}
