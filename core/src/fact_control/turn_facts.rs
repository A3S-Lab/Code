//! Turn, steer, run-control, and checkpoint facts appended to the session log.

use super::*;

/// The fact appended when a run's completion gate Allows. Mutating tool
/// results after the last marker belong to runs that never settled
/// accountable — an interrupted or gate-refused run — and the next send's
/// gate inherits them (see [`replay_unsettled_mutations`]).
pub const TURN_COMPLETE_FACT_KIND: &str = "turn.complete";

/// This run's thread, read through the run's own log handle.
pub(super) fn surface_thread_facts(
    surface: &SessionSurface,
) -> Result<Vec<a3s_effect::Fact>, ActorError> {
    surface_fact_log(surface)?.read(&thread_for_session(&surface.session_id))
}

pub(super) fn surface_fact_log(surface: &SessionSurface) -> Result<Arc<FileLog>, ActorError> {
    let mut shared = surface
        .fact_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(log) = shared.as_ref() {
        return Ok(Arc::clone(log));
    }
    let workspace = &surface.agent.tool_context_handle().workspace;
    let log = Arc::new(FileLog::open(crate::session_folder::effect_log_dir(
        workspace,
        &surface.session_id,
    ))?);
    *shared = Some(Arc::clone(&log));
    Ok(log)
}

/// Mark exactly which committed receipts an accountable settlement covers.
/// Failure to retain this boundary refuses success rather than inventing it.
pub(crate) fn append_turn_complete_fact(
    surface: &SessionSurface,
    terminal: &crate::harness_loop::CompletionTerminal,
) -> Result<(), ActorError> {
    let log = surface_fact_log(surface)?;
    let facts = log.read(&thread_for_session(&surface.session_id))?;
    let debt = receipts::fold_debt(&facts);
    if !debt.unresolved.is_empty() {
        return Err(ActorError::Schema(
            "tool_effect_unresolved: settlement cannot cover unknown invocations".into(),
        ));
    }
    let receipt_keys = debt
        .results
        .iter()
        .map(|(_, result)| result.identity.key())
        .collect::<Result<Vec<_>, _>>()?;
    let (label, digest) = match terminal {
        crate::harness_loop::CompletionTerminal::Narrative => ("narrative", None),
        crate::harness_loop::CompletionTerminal::Waived { effect_digest } => {
            ("waived", Some(effect_digest.clone()))
        }
        crate::harness_loop::CompletionTerminal::Verified { effect_digest } => {
            ("verified", Some(effect_digest.clone()))
        }
        // Step closures did not share one effect digest; the run still
        // closed with every step bound, so the bookkeeping is settled.
        crate::harness_loop::CompletionTerminal::Distinct => ("distinct", None),
    };
    let cut = a3s_effect::cut_log(&facts);
    let payload = serde_json::json!({
        "terminal": label,
        "effect_digest": digest,
        "receipt_keys": receipt_keys,
        "background_receipt_keys": facts.iter().filter(|fact| fact.kind == "background.output" && fact.payload["event"] == "exited")
            .map(|fact| fact.key.clone()).collect::<Vec<_>>(),
        "log_cut": { "seq": cut.seq, "digest": cut.digest },
    });
    log.append(
        &thread_for_session(&surface.session_id),
        &[NewFact {
            kind: TURN_COMPLETE_FACT_KIND.into(),
            key: format!("turn-complete:{}:{}", cut.seq, cut.digest),
            payload,
        }],
        None,
    )?;
    Ok(())
}

/// Restore workspace changes and uncertain invocations from the actor log.
/// Explicit receipt identities bound verified changes; a marker or checkpoint
/// cannot forgive missing terminals, incomplete observation or external debt.
/// Legacy unsettled evidence remains unknown rather than gaining invented
/// byte identities from the current filesystem.
pub(crate) fn unsettled_mutation_paths(facts: &[a3s_effect::Fact]) -> Vec<String> {
    receipts::fold_debt(facts).paths
}

pub(crate) fn has_unsettled_effects(facts: &[a3s_effect::Fact]) -> bool {
    let debt = receipts::fold_debt(facts);
    !debt.paths.is_empty() || !debt.unresolved.is_empty()
}

/// A portable checkpoint only contributes conservative state unless its
/// positive evidence is covered by one coherent, validated original prefix.
/// Coverage and recovery use this one read; this does not append facts or
/// grant any tool execution authority.
/// The returned verdict must be committed through the original actor input
/// path by recovery orchestration before any resumed model/tool dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckpointMutationCacheCoverage {
    Covered,
    Unproved,
}

pub(crate) fn restore_checkpoint_mutation_cache(
    surface: &SessionSurface,
    checkpoint: &crate::loop_checkpoint::LoopCheckpoint,
) -> CheckpointMutationCacheCoverage {
    let thread = thread_for_session(&surface.session_id);
    let facts = surface_thread_facts(surface);
    let covered = facts.as_ref().is_ok_and(|facts| {
        checkpoint_mutation_cache_covered(facts, checkpoint, &thread, &surface.session_id)
    });
    let needs_negative_input = facts.is_err()
        || (!covered
            && (checkpoint.convergence.mutations.has_checkpoint_evidence()
                || !checkpoint.verification_reports.is_empty()));
    {
        let mut ledger = surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.restore_checkpoint_cache(&checkpoint.convergence.mutations, covered);
        if !covered && !checkpoint.verification_reports.is_empty() {
            // A reports-only cache must not manufacture positive evidence
            // when its original receipt projection is absent either.
            ledger.restore_unresolved_tool_effects(
                std::collections::BTreeSet::from(
                    ["tool_effect_cache_evidence_missing".to_string()],
                ),
                &std::collections::BTreeSet::new(),
            );
        }
    }
    match facts {
        Ok(facts) => replay_unsettled_mutations_from_facts(surface, &facts),
        Err(error) => {
            surface
                .ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .restore_unresolved_tool_effects(
                    std::collections::BTreeSet::from([format!(
                        "tool_effect_log_unreadable:{error}"
                    )]),
                    &std::collections::BTreeSet::new(),
                );
        }
    }
    if needs_negative_input {
        CheckpointMutationCacheCoverage::Unproved
    } else {
        CheckpointMutationCacheCoverage::Covered
    }
}

pub(super) fn checkpoint_mutation_cache_covered(
    facts: &[a3s_effect::Fact],
    checkpoint: &crate::loop_checkpoint::LoopCheckpoint,
    thread: &str,
    session_id: &str,
) -> bool {
    if checkpoint.session_id != session_id {
        return false;
    }
    let expected_binding = match checkpoint.capability_binding.as_ref() {
        Some(binding) => {
            if binding.validate().is_err() {
                return false;
            }
            match crate::content_digest::digest_json("a3s.code.tool-admission-binding.v1", binding)
            {
                Ok(digest) => Some(digest),
                Err(_) => return false,
            }
        }
        None => None,
    };
    // Reuse preparation/terminal/background validation, including receipts
    // covered by old markers. A partial log cannot supply a missing pair.
    let debt = receipts::fold_cache_evidence(facts);
    if debt.integrity_invalid {
        return false;
    }
    let mut frozen_bindings = std::collections::BTreeMap::new();
    for (_, result) in &debt.results {
        let Some(admission) = result.identity.admission.as_ref() else {
            return false;
        };
        if admission.thread_id != thread
            || (admission.run_id == checkpoint.run_id
                && expected_binding.as_deref()
                    != Some(admission.capability_binding_digest.as_str()))
        {
            return false;
        }
        if frozen_bindings
            .insert(
                admission.run_id.as_str(),
                admission.capability_binding_digest.as_str(),
            )
            .is_some_and(|earlier| earlier != admission.capability_binding_digest.as_str())
        {
            return false;
        }
    }
    let (tool_keys, background_keys) = checkpoint.convergence.mutations.checkpoint_receipt_keys();
    let mut missing_tools = tool_keys.clone();
    let mut missing_background = background_keys.clone();
    let mut missing_reports = checkpoint
        .verification_reports
        .iter()
        .filter(|report| report.status == crate::verification::VerificationStatus::Passed)
        .collect::<Vec<_>>();
    if tool_keys.is_empty() && background_keys.is_empty() && !missing_reports.is_empty() {
        return false;
    }
    // The cached projection may precede a later known write. Project the
    // validated original events once in fact order. The first boundary that
    // covers every claimed key and positive report is the cache candidate;
    // compare its indexed states once, rather than folding every prefix or
    // repeatedly scanning a growing ledger.
    let mut original = crate::harness_loop::MutationLedger::default();
    let mut proof = debt.cache_proof_cursor(
        checkpoint
            .convergence
            .mutations
            .checkpoint_uncertainty_keys(),
        background_keys,
    );
    for projection in &debt.projection_order {
        let proof_ready = proof.advance_to(projection);
        match projection {
            receipts::ReceiptProjection::Tool(index) => {
                if let Some((tool, result)) = debt.results.get(*index) {
                    original.observe_tool_receipt(tool, result);
                    if let Ok(key) = result.identity.key() {
                        missing_tools.remove(&key);
                    }
                    if let Some(envelope) = result.output.as_str().and_then(decode_fact_tool_result)
                    {
                        let mut recorded = Vec::new();
                        crate::agent::AgentLoop::collect_verification_report(
                            &mut recorded,
                            &envelope.metadata,
                        );
                        recorded.retain(|report| {
                            report.effect_digest.as_deref()
                                == Some(checkpoint.convergence.mutations.digest())
                        });
                        missing_reports.retain(|cached| !recorded.contains(cached));
                    }
                }
            }
            receipts::ReceiptProjection::Background(index) => {
                if let Some((key, effects)) = debt.background_results.get(*index) {
                    original.observe_background_receipt(key, effects);
                    missing_background.remove(key);
                }
            }
        }
        if missing_tools.is_empty()
            && missing_background.is_empty()
            && missing_reports.is_empty()
            && proof_ready
        {
            let uncertainty = proof.into_proven_uncertainty();
            let negative_cache_covered = checkpoint
                .convergence
                .mutations
                .checkpoint_negative_cache_covered_by(&uncertainty);
            original
                .restore_unresolved_tool_effects(uncertainty, &std::collections::BTreeSet::new());
            return negative_cache_covered
                || checkpoint
                    .convergence
                    .mutations
                    .checkpoint_cache_covered_by(&original);
        }
    }
    // A final preparation or negative input may follow the last receipt.
    // Only this actual end-of-log boundary may include those later claims.
    if missing_tools.is_empty()
        && missing_background.is_empty()
        && missing_reports.is_empty()
        && proof.finish()
    {
        let uncertainty = proof.into_proven_uncertainty();
        let negative_cache_covered = checkpoint
            .convergence
            .mutations
            .checkpoint_negative_cache_covered_by(&uncertainty);
        original.restore_unresolved_tool_effects(uncertainty, &std::collections::BTreeSet::new());
        negative_cache_covered
            || checkpoint
                .convergence
                .mutations
                .checkpoint_cache_covered_by(&original)
    } else {
        false
    }
}

pub(crate) fn replay_unsettled_mutations(surface: &SessionSurface) {
    let facts = match surface_thread_facts(surface) {
        Ok(facts) => facts,
        Err(error) => {
            let mut unresolved = std::collections::BTreeSet::new();
            unresolved.insert(format!("tool_effect_log_unreadable:{error}"));
            surface
                .ledger
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .restore_unresolved_tool_effects(unresolved, &std::collections::BTreeSet::new());
            return;
        }
    };
    replay_unsettled_mutations_from_facts(surface, &facts);
}

pub(super) fn replay_unsettled_mutations_from_facts(
    surface: &SessionSurface,
    facts: &[a3s_effect::Fact],
) {
    let workspace = surface.agent.tool_context_handle().workspace.clone();
    let debt = receipts::fold_debt(facts);
    let receipt_paths = debt
        .results
        .iter()
        .flat_map(|(_, result)| result.effects.changes.iter())
        .chain(
            debt.background_results
                .iter()
                .flat_map(|(_, effects)| effects.changes.iter()),
        )
        .map(|change| change.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let paths = debt
        .paths
        .iter()
        .filter(|path| !receipt_paths.contains(path.as_str()))
        .map(|path| crate::harness_loop::workspace_relative_path(&workspace, path))
        .collect::<Vec<_>>();
    let mut ledger = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Re-project committed facts in order even if a later live foreground
    // callback arrived before an earlier background terminal was observed.
    ledger.begin_receipt_replay();
    ledger.observe_unseen_paths(&paths);
    for projection in debt.projection_order {
        match projection {
            receipts::ReceiptProjection::Tool(index) => {
                if let Some((tool, result)) = debt.results.get(index) {
                    ledger.observe_tool_receipt(tool, result);
                }
            }
            receipts::ReceiptProjection::Background(index) => {
                if let Some((key, effects)) = debt.background_results.get(index) {
                    ledger.observe_background_receipt(key, effects);
                }
            }
        }
    }
    ledger.restore_unresolved_tool_effects(debt.unresolved, &debt.settled_workspace_children);
}

/// Record the `(provider, digest)` identity of every context section this
/// model call saw, so a replay of the fact log can verify which section
/// content each turn actually consumed (identical pairs across turns are
/// prompt byte-stability at the section level). Best-effort like the steer
/// record: a failed append never blocks the model call. Durable-memory
/// recall sections are excluded — their evidence lane is the
/// `MemoryRecalled` telemetry, not the provider contract.
pub(super) fn append_context_sections_fact(
    surface: &SessionSurface,
    sections: &[(String, String)],
) {
    let Ok(log) = surface_fact_log(surface) else {
        return;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let payload = serde_json::json!({
        "sections": sections
            .iter()
            .map(|(provider, digest)| serde_json::json!({
                "provider": provider,
                "digest": digest,
            }))
            .collect::<Vec<_>>(),
    });
    // The key carries a payload digest so two model calls inside one
    // millisecond (fast tool cycles, instant mock clients) cannot collide
    // into a DuplicateFact rejection — identical payloads in the same
    // millisecond dedupe to one record, which is their identity anyway.
    let payload_fingerprint = crate::content_digest::digest_bytes(
        "a3s.code.context.sections-key.v1",
        payload.to_string().as_bytes(),
    );
    let _ = log.append(
        &thread_for_session(&surface.session_id),
        &[NewFact {
            kind: "context.sections".into(),
            key: format!("context:{now_ms}:{payload_fingerprint}"),
            payload,
        }],
        None,
    );
}

pub(super) async fn apply_run_controls(
    surface: &SessionSurface,
    messages: &mut Vec<Message>,
) -> Result<(), ActorError> {
    persist_run_controls(surface, false, Some(messages)).await
}

/// Retain late accepted input without opening another actor/model turn.
/// All terminal outcomes use the same original inbox and pinned append seam.
pub(crate) async fn settle_terminal_run_controls(
    surface: &SessionSurface,
) -> Result<(), ActorError> {
    persist_run_controls(surface, true, None).await
}

pub(super) async fn persist_run_controls(
    surface: &SessionSurface,
    terminal: bool,
    mut messages: Option<&mut Vec<Message>>,
) -> Result<(), ActorError> {
    let Some(control) = &surface.run_control else {
        return Ok(());
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    let decided = control
        .persist_pending(terminal, now_ms, |item| append_steer_fact(surface, item))
        .await;
    control.observe_decisions(&decided.decisions).await;
    for item in decided.decisions {
        if item.receipt.state != crate::run_control::RunControlReceiptState::Applied {
            continue;
        }
        let (input, reason) = match &item.request.command {
            crate::run_control::RunControlCommand::Steer { input } => {
                if let Some(messages) = messages.as_deref_mut() {
                    messages.push(Message::user(input));
                }
                ((!terminal).then(|| input.clone()), None)
            }
            crate::run_control::RunControlCommand::Interrupt { reason, .. } => {
                (None, reason.clone())
            }
        };
        let receipt = item.receipt;
        record_run_event(
            surface,
            crate::agent::AgentEvent::RunControlApplied {
                request_id: receipt.request_id,
                operation: receipt.operation,
                turn_id: receipt.turn_id,
                turn_revision: receipt.turn_revision,
                input,
                reason,
            },
        )
        .await;
    }
    match decided.failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(super) fn append_steer_fact(
    surface: &SessionSurface,
    item: &crate::run_control::PendingRunControl,
) -> Result<(), ActorError> {
    if item.receipt.session_id != surface.session_id
        || surface.run_id.as_deref() != Some(item.receipt.run_id.as_str())
    {
        return Err(ActorError::Schema(
            "Steer does not belong to the original fact Run".into(),
        ));
    }
    let log = surface_fact_log(surface)?;
    append_steer_input(
        log.as_ref(),
        &item.receipt.session_id,
        &item.receipt.run_id,
        &item.receipt.request_id,
        &item.request.command,
    )
}

pub(super) fn append_steer_input(
    log: &dyn LogStore,
    session_id: &str,
    run_id: &str,
    request_id: &str,
    command: &crate::run_control::RunControlCommand,
) -> Result<(), ActorError> {
    let crate::run_control::RunControlCommand::Steer { input } = command else {
        return Ok(());
    };
    // A distinct prefix is outside the entire historical steer:{raw_id} set.
    // Fixed-array encoding distinguishes colons, Unicode and empty components.
    let digest = crate::content_digest::digest_json(
        "a3s.code.steer-input.identity.v1",
        &[session_id, run_id, request_id],
    )
    .map_err(|error| ActorError::Schema(error.to_string()))?;
    a3s_effect::append_input(
        log,
        &thread_for_session(session_id),
        message_fact(format!("run-steer-v1:{digest}"), input),
    )
}
