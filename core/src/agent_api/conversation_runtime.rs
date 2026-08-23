//! Conversation execution facade for a session.
//!
//! This module owns the public conversation contract: slash-command dispatch,
//! blocking sends, streaming sends, and attachment handling.
//! Lower-level runtime modules own run lifecycle and event forwarding.

use super::{
    command_runtime, run_admission, run_lifecycle::RunControlState, runtime::BlockingRunContext,
    runtime::ConversationInput, runtime::StreamRunContext, AgentRunSpawn, AgentSession,
};
use crate::agent::{AgentEvent, AgentResult};
use crate::error::{CodeError, Result};
use crate::llm::{Attachment, Message};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

fn bail_if_closed(session: &AgentSession) -> Result<()> {
    if session.is_closed() {
        return Err(CodeError::SessionClosed {
            session_id: session.session_id.clone(),
        });
    }
    Ok(())
}

async fn admit(
    session: &AgentSession,
    operation: &'static str,
) -> Result<run_admission::RunAdmissionLease> {
    bail_if_closed(session)?;
    let lease = session.run_admission.try_acquire(&session.session_id)?;
    let label = format!("{}:{operation}", session.session_id);
    let task_lease = session
        .task_scheduler
        .acquire(session.task_priority, label, &session.session_cancel)
        .await
        .map_err(|error| match error {
            crate::task_scheduler::TaskSchedulerError::Cancelled if session.is_closed() => {
                CodeError::SessionClosed {
                    session_id: session.session_id.clone(),
                }
            }
            crate::task_scheduler::TaskSchedulerError::Cancelled => {
                CodeError::TaskAdmissionCancelled {
                    session_id: session.session_id.clone(),
                }
            }
            crate::task_scheduler::TaskSchedulerError::Closed => CodeError::TaskSchedulerClosed,
            crate::task_scheduler::TaskSchedulerError::InvalidConfig(message) => {
                CodeError::Config(message)
            }
        })?;
    bail_if_closed(session)?;
    Ok(lease.attach_task_lease(task_lease))
}

pub(super) async fn send(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    // Admission must precede command dispatch and internal-history reads.
    let _lease = admit(session, "send").await?;

    if let Some(result) = command_runtime::dispatch_blocking(session, prompt, history).await? {
        return Ok(result);
    }

    warn_deferred_init(session);
    let input = ConversationInput::from_history(session, history);
    let blocking_run = BlockingRunContext::start(session, prompt, input.persistence).await?;
    blocking_run
        .execute_with_prompt(&input.messages, prompt, &session.session_id)
        .await
}

pub(super) async fn send_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    // Admission must precede the attachment message's internal-history clone.
    let _lease = admit(session, "send-with-attachments").await?;

    // Build one user message containing text and images, then execute from the
    // resulting message list so the loop does not append a duplicate prompt.
    let input = ConversationInput::with_attachments(session, history, prompt, attachments);
    let blocking_run = BlockingRunContext::start(session, prompt, input.persistence).await?;
    blocking_run
        .execute_from_messages(input.messages, &session.session_id)
        .await
}

pub(super) async fn stream_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    let lease = admit(session, "stream-with-attachments").await?;

    let input = ConversationInput::with_attachments(session, history, prompt, attachments);
    let stream_run = StreamRunContext::start(session, prompt, input.persistence).await?;
    let (rx, handle, worker_aborts) = stream_run.spawn_from_messages(input.messages);
    Ok((
        rx,
        run_admission::guard_stream_handle(handle, worker_aborts, lease),
    ))
}

pub(super) async fn stream(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    // Slash commands share admission because they read and may mutate the same
    // session state as model-backed operations.
    let lease = admit(session, "stream").await?;

    if let Some((rx, handle)) = command_runtime::dispatch_streaming(session, prompt).await? {
        let worker_abort = handle.abort_handle();
        return Ok((
            rx,
            run_admission::guard_stream_handle(handle, vec![worker_abort], lease),
        ));
    }

    let input = ConversationInput::from_history(session, history);
    let stream_run = StreamRunContext::start(session, prompt, input.persistence).await?;
    let (rx, handle, worker_aborts) =
        stream_run.spawn_with_prompt(input.messages, prompt.to_string());
    Ok((
        rx,
        run_admission::guard_stream_handle(handle, worker_aborts, lease),
    ))
}

/// Start one detached Code run at an exact host-selected identity.
pub(super) async fn spawn_run_with_id(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
) -> Result<AgentRunSpawn> {
    if let Some(replay) = exact_run_replay(session, run_id, prompt).await? {
        return Ok(replay);
    }
    let lease = admit(session, "spawn-run").await?;
    let input = ConversationInput::from_history(session, None);
    let run_control = RunControlState::from_session(session);
    let reservation = run_control.reserve_run_with_id(run_id, prompt).await?;
    let snapshot = reservation.snapshot().clone();
    if reservation.replayed() {
        return Ok(AgentRunSpawn::Replayed { snapshot });
    }

    let stream_run =
        match StreamRunContext::for_run(session, run_id.to_string(), input.persistence).await {
            Ok(run) => run,
            Err(error) => {
                run_control.fail_reserved_run_start(run_id, &error).await;
                return Err(error);
            }
        };
    let (events, worker, worker_aborts) =
        stream_run.spawn_with_prompt(input.messages, prompt.to_string());
    let worker = run_admission::guard_stream_handle(worker, worker_aborts, lease);
    Ok(AgentRunSpawn::Started {
        snapshot,
        worker: drain_detached_events(events, worker),
    })
}

/// Resume one durable loop checkpoint into an exact fresh run identity.
pub(super) async fn spawn_recovery_with_run_id(
    session: &AgentSession,
    checkpoint_run_id: &str,
    run_id: &str,
) -> Result<AgentRunSpawn> {
    let prompt = format!("<resume run={checkpoint_run_id}>");
    if let Some(replay) = exact_run_replay(session, run_id, &prompt).await? {
        return Ok(replay);
    }

    let lease = admit(session, "spawn-recovery").await?;
    let checkpoint = load_resume_checkpoint(session, checkpoint_run_id).await?;
    let run_control = RunControlState::from_session(session);
    let reservation = run_control.reserve_run_with_id(run_id, &prompt).await?;
    let snapshot = reservation.snapshot().clone();
    if reservation.replayed() {
        return Ok(AgentRunSpawn::Replayed { snapshot });
    }

    let persistence =
        Some(super::session_persistence::SessionPersistenceContext::from_session(session));
    let stream_run = match StreamRunContext::for_run(session, run_id.to_string(), persistence).await
    {
        Ok(run) => run,
        Err(error) => {
            run_control.fail_reserved_run_start(run_id, &error).await;
            return Err(error);
        }
    };
    let seed = crate::agent::ExecutionSeed {
        turn: checkpoint.turn,
        total_usage: checkpoint.total_usage.clone(),
        tool_calls_count: checkpoint.tool_calls_count,
        verification_reports: checkpoint.verification_reports.clone(),
        convergence: checkpoint.convergence.clone(),
    };
    let (events, worker, worker_aborts) =
        stream_run.spawn_from_messages_seeded(checkpoint.messages, Some(seed));
    let worker = run_admission::guard_stream_handle(worker, worker_aborts, lease);
    Ok(AgentRunSpawn::Started {
        snapshot,
        worker: drain_detached_events(events, worker),
    })
}

/// Resume a previously-checkpointed run on this session (P3 cut 2).
///
/// Loads the latest [`LoopCheckpoint`](crate::loop_checkpoint::LoopCheckpoint)
/// for `checkpoint_run_id` from the session's `SessionStore` and replays
/// the agent loop from that boundary state. A **new** run id is
/// generated for the resumed work — the relationship between the old
/// and new run is metadata the host tracks externally.
///
/// Returns an error when the session has no store configured, or when
/// no checkpoint exists for `checkpoint_run_id`.
pub(super) async fn resume_run(
    session: &AgentSession,
    checkpoint_run_id: &str,
) -> Result<crate::agent::AgentResult> {
    let _lease = admit(session, "resume-run").await?;
    let checkpoint = load_resume_checkpoint(session, checkpoint_run_id).await?;

    let persistence =
        Some(super::session_persistence::SessionPersistenceContext::from_session(session));
    let blocking_run = BlockingRunContext::start(
        session,
        &format!("<resume run={checkpoint_run_id} turn={}>", checkpoint.turn),
        persistence,
    )
    .await?;
    // Seed the resumed run's loop state with the cumulative metrics from
    // the checkpoint so token usage and tool-call counts continue from
    // where the crashed/migrated run left off rather than re-starting at
    // zero (which would under-report the resumed AgentResult).
    let seed = crate::agent::ExecutionSeed {
        turn: checkpoint.turn,
        total_usage: checkpoint.total_usage.clone(),
        tool_calls_count: checkpoint.tool_calls_count,
        verification_reports: checkpoint.verification_reports.clone(),
        convergence: checkpoint.convergence.clone(),
    };
    blocking_run
        .execute_from_messages_seeded(checkpoint.messages, &session.session_id, Some(seed))
        .await
}

async fn load_resume_checkpoint(
    session: &AgentSession,
    checkpoint_run_id: &str,
) -> Result<crate::loop_checkpoint::LoopCheckpoint> {
    let store = session.session_store.as_ref().ok_or_else(|| {
        CodeError::Session("resume_run requires a session_store on this session".to_string())
    })?;
    let checkpoint = store
        .load_loop_checkpoint(checkpoint_run_id)
        .await
        .map_err(|error| {
            CodeError::Session(format!(
                "load_loop_checkpoint('{checkpoint_run_id}') failed: {error}"
            ))
        })?
        .ok_or_else(|| {
            CodeError::Session(format!(
                "no loop checkpoint found for run '{checkpoint_run_id}'"
            ))
        })?;
    checkpoint
        .ensure_owned_by(checkpoint_run_id, &session.session_id)
        .map_err(|error| {
            CodeError::Session(format!(
                "refusing to resume checkpoint '{checkpoint_run_id}': {error:#}"
            ))
        })?;
    Ok(checkpoint)
}

fn drain_detached_events(
    mut events: mpsc::Receiver<AgentEvent>,
    worker: JoinHandle<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while events.recv().await.is_some() {}
        let _ = worker.await;
    })
}

async fn exact_run_replay(
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

fn warn_deferred_init(session: &AgentSession) {
    if let Some(warning) = &session.init_warning {
        tracing::warn!(
            session_id = %session.session_id,
            "Session init warning: {}", warning
        );
    }
}
