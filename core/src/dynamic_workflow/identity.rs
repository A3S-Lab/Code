use super::*;

/// Derive the stable identity for one dynamic Flow step admission.
///
/// The returned value is digest-only. The step input is included in the
/// derivation so retries with different arguments cannot share a lease, but no
/// input bytes are retained in the identity or emitted by the scheduler.
///
/// Large inputs (for example DeepResearch selector shards whose prompt envelope
/// is intentionally up to 96 KiB before schema wrapping) are folded to
/// `sha256` + `bytes` before derivation so the identity API does not impose a
/// tighter ceiling than the workflows it hosts.
pub fn dynamic_workflow_step_identity(
    run_id: &str,
    step_id: &str,
    step_name: &str,
    input: &Value,
) -> Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    for (field, value) in [
        ("run_id", run_id),
        ("step_id", step_id),
        ("step_name", step_name),
    ] {
        if value.is_empty()
            || value.len() > MAX_FLOW_STEP_ID_BYTES
            || value.contains('\0')
            || value.lines().count() != 1
        {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField(field),
            );
        }
    }
    let encoded_input = serde_json::to_vec(input).map_err(|error| {
        crate::execution_identity::ExecutionIdentityError::Serialization(error.to_string())
    })?;
    if encoded_input.len() > MAX_FLOW_STEP_INPUT_BYTES {
        return Err(
            crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                "dynamic Flow step input exceeds {} bytes",
                MAX_FLOW_STEP_INPUT_BYTES
            )),
        );
    }
    let input_for_identity = fold_flow_step_input_for_identity(input, &encoded_input);
    ExecutionIdentityV1::derive(
        FLOW_STEP_IDENTITY_DOMAIN_V1,
        &json!({
            "run_id": run_id,
            "step_id": step_id,
            "step_name": step_name,
            "input": input_for_identity,
        }),
    )
}

/// Soft-fold step input for identity derivation: inline below the retention
/// budget, otherwise mix only `sha256` + `bytes` (digest-only).
pub(super) fn fold_flow_step_input_for_identity(input: &Value, encoded_input: &[u8]) -> Value {
    if encoded_input.len() <= MAX_FLOW_STEP_INPUT_INLINE_BYTES {
        input.clone()
    } else {
        json!({
            "sha256": sha256::digest(encoded_input),
            "bytes": encoded_input.len(),
        })
    }
}

pub(super) fn dynamic_workflow_input_identity(
    input: &Value,
) -> Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    let encoded = serde_json::to_vec(input).map_err(|error| {
        crate::execution_identity::ExecutionIdentityError::Serialization(error.to_string())
    })?;
    if encoded.len() > MAX_DYNAMIC_WORKFLOW_INPUT_BYTES {
        return Err(
            crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                "dynamic workflow input exceeds {} bytes",
                MAX_DYNAMIC_WORKFLOW_INPUT_BYTES
            )),
        );
    }
    ExecutionIdentityV1::derive(DYNAMIC_WORKFLOW_INPUT_IDENTITY_DOMAIN_V1, input)
}

/// Derive the stable, digest-only claim identity for one dynamic workflow.
///
/// Unlike the continuation identity, this identity intentionally excludes the
/// evolving plan and step history. It therefore remains constant while a
/// worker replays, retries, or takes over the same durable run. The complete
/// continuation identity is still validated first, so malformed or
/// mixed-generation history can never acquire a worker lease.
pub fn dynamic_workflow_claim_identity(
    run_id: &str,
    source: &str,
    input: &Value,
    runtime_build_id: &str,
    history: &[FlowEventEnvelope],
) -> std::result::Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    dynamic_workflow_continuation_identity(run_id, source, input, runtime_build_id, history)?;
    let input_identity = dynamic_workflow_input_identity(input)?;
    let effective_runtime_build_id = history
        .iter()
        .find_map(|envelope| match &envelope.event {
            FlowEvent::RunCreated { spec, .. } => Some(
                spec.runtime_build_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| LEGACY_UNPINNED_RUNTIME_BUILD_ID.to_string()),
            ),
            _ => None,
        })
        .unwrap_or_else(|| runtime_build_id.to_string());
    ExecutionIdentityV1::derive(
        DYNAMIC_WORKFLOW_CLAIM_IDENTITY_DOMAIN_V1,
        &json!({
            "run_id": run_id,
            "source_hash": source_hash(source),
            "input_identity": input_identity.digest,
            "runtime_build_id": effective_runtime_build_id,
        }),
    )
}

/// Reconstruct the immutable identity of a dynamic workflow continuation.
///
/// The Flow journal already persists the run definition and every step
/// definition. This adapter binds those facts to the current runtime build,
/// source, and initial input without introducing a second journal. Progress,
/// retries, event sequence, and step outputs are intentionally excluded, so a
/// restart observes the same continuation identity before and after replay.
/// A malformed or mixed-generation history is rejected before a step body can
/// be admitted.
pub fn dynamic_workflow_continuation_identity(
    run_id: &str,
    source: &str,
    input: &Value,
    runtime_build_id: &str,
    history: &[FlowEventEnvelope],
) -> std::result::Result<ExecutionIdentityV1, crate::execution_identity::ExecutionIdentityError> {
    if !safe_workflow_run_id(run_id) {
        return Err(crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_id"));
    }
    if source.is_empty() {
        return Err(crate::execution_identity::ExecutionIdentityError::InvalidClaimField("source"));
    }
    RuntimeBuildId::new(runtime_build_id.to_string()).map_err(|error| {
        crate::execution_identity::ExecutionIdentityError::Serialization(format!(
            "invalid dynamic workflow runtime build id: {error}"
        ))
    })?;
    let input_identity = dynamic_workflow_input_identity(input)?;
    let expected_source_hash = source_hash(source);
    let expected_spec = WorkflowSpec::rust_embedded(
        "a3s-code.dynamic-workflow",
        expected_source_hash.as_str(),
        "ptc",
        "run",
    );
    let mut saw_run_created = false;
    let mut persisted_runtime_build: Option<String> = None;
    let mut step_identities = BTreeMap::<String, String>::new();
    let mut terminal_seen = false;

    for (index, envelope) in history.iter().enumerate() {
        if envelope.run_id != run_id {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_id"),
            );
        }
        if envelope.sequence != index as u64 + 1 {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("sequence"),
            );
        }
        if terminal_seen {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("terminal"),
            );
        }
        if index == 0 && !matches!(&envelope.event, FlowEvent::RunCreated { .. }) {
            return Err(
                crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_created"),
            );
        }

        match &envelope.event {
            FlowEvent::RunCreated {
                spec,
                input: persisted_input,
            } => {
                if saw_run_created {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "run_created",
                        ),
                    );
                }
                saw_run_created = true;
                if spec.name != expected_spec.name
                    || spec.runtime != expected_spec.runtime
                    || !spec.patch_markers.is_empty()
                    || !spec.signal_names.is_empty()
                {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "workflow_spec",
                        ),
                    );
                }
                if spec.version != expected_source_hash {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "source",
                        ),
                    );
                }
                let persisted_input_identity = dynamic_workflow_input_identity(persisted_input)?;
                if persisted_input_identity != input_identity {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "input",
                        ),
                    );
                }
                if let Some(build_id) = &spec.runtime_build_id {
                    RuntimeBuildId::new(build_id.as_str().to_string()).map_err(|error| {
                        crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                            "invalid persisted dynamic workflow runtime build id: {error}"
                        ))
                    })?;
                    persisted_runtime_build = Some(build_id.as_str().to_string());
                } else {
                    persisted_runtime_build = Some(LEGACY_UNPINNED_RUNTIME_BUILD_ID.to_string());
                }
            }
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                retry,
            } => {
                let admission_identity =
                    dynamic_workflow_step_identity(run_id, step_id, step_name, input)?;
                // Retry behavior is part of Flow's immutable step definition,
                // even though it is not needed for the scheduler admission
                // lease. Bind it here so a conflicting duplicate cannot hide
                // behind the digest-only admission identity.
                let identity = ExecutionIdentityV1::derive(
                    FLOW_STEP_IDENTITY_DOMAIN_V1,
                    &json!({
                        "admission": admission_identity.digest,
                        "retry": retry,
                    }),
                )?;
                if step_identities
                    .insert(step_id.clone(), identity.digest)
                    .is_some()
                {
                    return Err(
                        crate::execution_identity::ExecutionIdentityError::InvalidClaimField(
                            "step_definition",
                        ),
                    );
                }
            }
            _ => {}
        }

        terminal_seen = matches!(
            &envelope.event,
            FlowEvent::RunCompleted { .. }
                | FlowEvent::RunFailed { .. }
                | FlowEvent::RunCancelled { .. }
                | FlowEvent::RunTimedOut { .. }
                | FlowEvent::RunRetryExhausted { .. }
                | FlowEvent::RunHostShutdown { .. }
                | FlowEvent::RunContinuedAsNew { .. }
        );
    }

    if !history.is_empty() && !saw_run_created {
        return Err(
            crate::execution_identity::ExecutionIdentityError::InvalidClaimField("run_created"),
        );
    }
    let effective_runtime_build = persisted_runtime_build
        .as_deref()
        .unwrap_or(runtime_build_id);
    if effective_runtime_build != LEGACY_UNPINNED_RUNTIME_BUILD_ID {
        RuntimeBuildId::new(effective_runtime_build.to_string()).map_err(|error| {
            crate::execution_identity::ExecutionIdentityError::Serialization(format!(
                "invalid effective dynamic workflow runtime build id: {error}"
            ))
        })?;
    }
    let plan = dynamic_workflow_execution_plan(history);
    let plan_identity = plan.definition_identity()?;
    ExecutionIdentityV1::derive(
        DYNAMIC_WORKFLOW_CONTINUATION_IDENTITY_DOMAIN_V1,
        &json!({
            "run_id": run_id,
            "source_hash": expected_source_hash,
            "input_identity": input_identity.digest,
            "runtime_build_id": effective_runtime_build,
            "plan_identity": plan_identity.digest,
            "step_identities": step_identities,
        }),
    )
}

/// Project the complete durable Flow history into Code's canonical plan model.
///
/// Flow remains the execution authority; this is a read-only adapter used by
/// progress events and metadata. Replaying the full history (rather than only
/// observing newly appended events) makes resumed runs expose the same plan as
/// fresh runs.
pub fn dynamic_workflow_execution_plan(history: &[FlowEventEnvelope]) -> ExecutionPlan {
    let mut plan = ExecutionPlan::new("dynamic workflow", Complexity::Medium);
    for envelope in history {
        match &envelope.event {
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                ..
            } => {
                let task = Task::new(
                    step_id.clone(),
                    workflow_step_description(step_id, step_name, Some(input)),
                )
                .with_tool(step_name.clone());
                plan.upsert_step(task);
            }
            FlowEvent::StepStarted { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::InProgress);
            }
            FlowEvent::StepRetrying { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::InProgress);
            }
            FlowEvent::StepCompleted { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::Completed);
            }
            FlowEvent::StepFailed { step_id, .. } => {
                plan.mark_status(step_id, TaskStatus::Failed);
            }
            FlowEvent::RunFailed { .. }
            | FlowEvent::RunTimedOut { .. }
            | FlowEvent::RunRetryExhausted { .. } => {
                for task in &mut plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Failed;
                    }
                }
            }
            FlowEvent::RunCancelled { .. } | FlowEvent::RunHostShutdown { .. } => {
                for task in &mut plan.steps {
                    if task.status.is_active() {
                        task.status = TaskStatus::Cancelled;
                    }
                }
            }
            _ => {}
        }
    }
    plan
}
