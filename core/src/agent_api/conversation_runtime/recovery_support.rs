use super::*;

pub(super) async fn prepare_pre_analysis(
    parts: &FactSession,
    prompt: &str,
    current_goal: Option<&crate::goal_control::DurableGoalDefinitionV1>,
) -> anyhow::Result<()> {
    let Some(surface) = &parts.surface else {
        return Ok(());
    };
    // `Auto` pays one structured pre-analysis call and plans only when it
    // returns `requires_planning`; `Disabled` makes no call.
    surface
        .agent
        .fact_publish_plan(
            prompt,
            &surface.session_id,
            &surface.events,
            &surface.cancel,
            (surface.run_store.as_deref(), surface.run_id.as_deref()),
            current_goal,
        )
        .await
        .map_err(pre_analysis_error)
}

pub(super) fn pre_analysis_error(error: anyhow::Error) -> anyhow::Error {
    if let Some(message) = crate::llm::non_retryable_llm_error_message(&error) {
        anyhow::anyhow!(message.to_string())
    } else {
        error
    }
}

pub(super) async fn settle_confirmations(
    run: &crate::fact_control::FactRun,
    settled: a3s_effect::Settlement<a3s_effect::CodingView>,
    parts: &FactSession,
) -> anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>> {
    match &parts.surface {
        Some(surface) => settle_host_input(run, settled, surface).await,
        None => Ok(settled),
    }
}

/// Answer pending confirmations and questions until the run leaves both phases.
pub(crate) async fn settle_host_input(
    run: &crate::fact_control::FactRun,
    mut settled: a3s_effect::Settlement<a3s_effect::CodingView>,
    surface: &crate::fact_control::SessionSurface,
) -> anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>> {
    while settled.view.phase == a3s_effect::CodingPhase::Confirm {
        let Some(pending) = settled.view.pending_confirmation.clone() else {
            break;
        };
        let approved = await_host_confirmation(surface, &pending).await;
        if surface.cancel.is_cancelled() {
            let _ = run.confirm(&pending.tool_call_id, false).await;
            break;
        }
        settled = run.confirm(&pending.tool_call_id, approved).await?;
    }
    while settled.view.phase == a3s_effect::CodingPhase::Question {
        let Some(pending) = settled.view.pending_question.clone() else {
            break;
        };
        emit_interactive_host_event(
            surface,
            crate::agent::AgentEvent::UserQuestion {
                question_id: pending.question_id.clone(),
                question: pending.question.clone(),
                options: pending.options.clone(),
                allow_free_text: pending.allow_free_text,
                form: None,
            },
        )
        .await;
        let Some(text) =
            crate::ask_user::wait_for_answer(&pending.question_id, &surface.cancel).await
        else {
            break;
        };
        let output = serde_json::json!({
            "schema": crate::ask_user::USER_ANSWER_SCHEMA,
            "question_id": pending.question_id,
            "status": "answered",
            "answer": text,
            "permission_grant": false,
            "wrote_files": false,
        })
        .to_string();
        emit_interactive_host_event(
            surface,
            crate::agent::AgentEvent::ToolEnd {
                id: pending.question_id.clone(),
                name: "ask_user".into(),
                args: Some(serde_json::json!({
                    "question": pending.question,
                    "options": pending.options,
                    "allow_free_text": pending.allow_free_text,
                })),
                output: output.clone(),
                exit_code: 0,
                metadata: Some(serde_json::json!({
                    "permission_grant": false,
                    "wrote_files": false,
                })),
                error_kind: None,
            },
        )
        .await;
        settled = run.answer(&output).await?;
    }
    Ok(settled)
}

pub(super) async fn await_host_confirmation(
    surface: &crate::fact_control::SessionSurface,
    pending: &a3s_effect::PendingConfirmation,
) -> bool {
    let Some(manager) = &surface.confirmation else {
        return false;
    };
    let receiver = manager
        .request_confirmation(&pending.tool_call_id, &pending.name, &pending.args)
        .await;
    emit_interactive_host_event(
        surface,
        AgentEvent::ConfirmationRequired {
            tool_id: pending.tool_call_id.clone(),
            tool_name: pending.name.clone(),
            args: pending.args.clone(),
            timeout_ms: 0,
        },
    )
    .await;
    let reason = Some("Confirmation cancelled".to_string());
    tokio::select! {
        biased;
        _ = surface.cancel.cancelled() => {
            let _ = manager
                .confirm(&pending.tool_call_id, false, reason.clone())
                .await;
            emit_interactive_host_event(surface, AgentEvent::ConfirmationReceived {
                        tool_id: pending.tool_call_id.clone(),
                        approved: false,
                        reason,
                    }).await;
            false
        }
        response = receiver => {
            let response = response.unwrap_or(crate::hitl::ConfirmationResponse {
                approved: false,
                reason: reason.clone(),
            });
            emit_interactive_host_event(surface, AgentEvent::ConfirmationReceived {
                        tool_id: pending.tool_call_id.clone(),
                        approved: response.approved,
                        reason: response.reason.clone(),
                    }).await;
            response.approved
        }
    }
}

#[cfg(test)]
pub(super) fn seed_exact_recovery(pinned: &PinnedFact, history: &[Message]) -> anyhow::Result<()> {
    seed_exact_recovery_parts(&pinned.parts, history)
}

pub(super) fn seed_exact_recovery_parts(
    parts: &FactSession,
    history: &[Message],
) -> anyhow::Result<()> {
    let run = parts.open()?;
    let mut seed = history.to_vec();
    if seed
        .last()
        .is_some_and(|message| message.role == "assistant")
    {
        seed.pop();
    }
    let prompt = history
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(Message::text)
        .unwrap_or_default();
    run.seed_recovery_history(&seed, &prompt)?;
    Ok(())
}

pub(super) fn exact_recovery_prompt(checkpoint_identity: &str) -> String {
    format!("<resume exact checkpoint={checkpoint_identity}>")
}

pub(super) fn ensure_distinct_recovery_target(
    source_run_id: &str,
    target_run_id: &str,
) -> Result<()> {
    if source_run_id == target_run_id {
        return Err(CodeError::RunIdentityConflict {
            run_id: target_run_id.to_string(),
        });
    }
    Ok(())
}

pub(super) fn drain_detached_events(
    mut events: mpsc::Receiver<AgentEvent>,
    worker: JoinHandle<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while events.recv().await.is_some() {}
        let _ = worker.await;
    })
}

pub(crate) async fn exact_run_replay(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
) -> Result<Option<AgentRunSpawn>> {
    let Some(snapshot) = session.run_snapshot(run_id).await else {
        return Ok(None);
    };
    if snapshot.session_id != session.session_id || snapshot.prompt != prompt {
        return Err(CodeError::RunIdentityConflict {
            run_id: run_id.to_string(),
        });
    }
    Ok(Some(AgentRunSpawn::Replayed { snapshot }))
}
