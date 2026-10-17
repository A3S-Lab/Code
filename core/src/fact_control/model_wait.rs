//! Waiting on a model call: host cancellation, the provider quiet gap and
//! the run's wall-clock budget.

use super::{interrupted_response, ActorError};

/// Time left in the run's wall-clock budget; an error once it is spent.
/// The error is a defect so the fold neither retries the call nor appends a
/// fact for it.
pub(super) fn execution_time_left(
    started_at: std::time::Instant,
    limit: Option<std::time::Duration>,
) -> Result<Option<std::time::Duration>, ActorError> {
    let Some(limit) = limit else {
        return Ok(None);
    };
    match limit.checked_sub(started_at.elapsed()) {
        Some(left) if !left.is_zero() => Ok(Some(left)),
        _ => Err(execution_timeout(started_at, Some(limit))),
    }
}

pub(super) fn execution_timeout(
    started_at: std::time::Instant,
    limit: Option<std::time::Duration>,
) -> ActorError {
    ActorError::Defect(format!(
        "Execution timeout reached after {} seconds (limit: {} seconds).",
        started_at.elapsed().as_secs(),
        limit.unwrap_or_default().as_secs(),
    ))
}

pub(super) fn llm_idle_timeout_error(idle: std::time::Duration) -> ActorError {
    ActorError::Handler {
        key: "infer".into(),
        message: format!("LLM call timed out after {} ms", idle.as_millis()),
    }
}

/// Sleep for one quiet gap. `None` never completes, so the caller's other
/// branches stay the only way out.
pub(super) async fn model_idle_sleep(limit: Option<std::time::Duration>) {
    match limit {
        Some(limit) => tokio::time::sleep(limit).await,
        None => std::future::pending::<()>().await,
    }
}

pub(super) struct AbortOnDrop(pub(super) tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn await_model_task(
    mut handle: tokio::task::JoinHandle<Result<crate::llm::LlmResponse, ActorError>>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::llm::LlmResponse, ActorError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            handle.abort();
            let _ = handle.await;
            Ok(interrupted_response())
        }
        joined = &mut handle => flatten_model_task(joined),
    }
}

/// Wait for the model task. `idle` is the quiet gap: each `activity` notice
/// starts it over. A gap with no notice aborts the worker and cancels the
/// provider attempt. Host cancellation is not that failure.
pub(super) async fn await_model_task_with_idle(
    mut handle: tokio::task::JoinHandle<Result<crate::llm::LlmResponse, ActorError>>,
    cancel: &tokio_util::sync::CancellationToken,
    attempt: &tokio_util::sync::CancellationToken,
    activity: &tokio::sync::Notify,
    idle: Option<std::time::Duration>,
) -> Result<crate::llm::LlmResponse, ActorError> {
    let Some(idle) = idle else {
        return await_model_task(handle, cancel).await;
    };
    let sleep = tokio::time::sleep(idle);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                handle.abort();
                let _ = handle.await;
                return Ok(interrupted_response());
            }
            joined = &mut handle => return flatten_model_task(joined),
            _ = activity.notified() => {
                sleep.as_mut().reset(tokio::time::Instant::now() + idle);
            }
            _ = &mut sleep => {
                attempt.cancel();
                handle.abort();
                let _ = handle.await;
                return Err(llm_idle_timeout_error(idle));
            }
        }
    }
}

fn flatten_model_task(
    joined: Result<Result<crate::llm::LlmResponse, ActorError>, tokio::task::JoinError>,
) -> Result<crate::llm::LlmResponse, ActorError> {
    match joined {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Err(ActorError::Handler {
            key: "infer".into(),
            message: "LLM call timed out".into(),
        }),
        Err(error) => Err(ActorError::Defect(format!("model task failed: {error}"))),
    }
}
