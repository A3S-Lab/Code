use super::*;

pub(super) struct DynamicWorkflowExecution {
    pub(super) snapshot: WorkflowRunSnapshot,
    pub(super) history: Vec<FlowEventEnvelope>,
    pub(super) continuation_identity: ExecutionIdentityV1,
}

pub(super) struct DynamicWorkflowDriveContext<'a> {
    pub(super) source: &'a str,
    pub(super) input_for_identity: &'a Value,
    pub(super) runtime_build_id: &'a str,
    pub(super) workflow_context: &'a ToolContext,
    pub(super) parent_cancellation: CancellationToken,
    pub(super) child_cancellation: CancellationToken,
}

pub(super) async fn collect_dynamic_workflow_execution(
    engine: &FlowEngine,
    run_id: &str,
    snapshot: WorkflowRunSnapshot,
    source: &str,
    input: &Value,
    runtime_build_id: &str,
) -> Result<DynamicWorkflowExecution> {
    let history = engine.history(run_id).await?;
    let continuation_identity =
        dynamic_workflow_continuation_identity(run_id, source, input, runtime_build_id, &history)
            .map_err(|error| {
            anyhow::anyhow!("dynamic workflow continuation identity rejected after replay: {error}")
        })?;
    Ok(DynamicWorkflowExecution {
        snapshot,
        history,
        continuation_identity,
    })
}

pub(super) fn validate_dynamic_workflow_control_reason(reason: Option<&str>) -> Result<()> {
    let Some(reason) = reason else {
        return Ok(());
    };
    if reason.len() > MAX_DYNAMIC_WORKFLOW_CONTROL_REASON_BYTES {
        anyhow::bail!(
            "dynamic workflow cancellation reason exceeds {} bytes",
            MAX_DYNAMIC_WORKFLOW_CONTROL_REASON_BYTES
        );
    }
    if reason.contains('\0') {
        anyhow::bail!("dynamic workflow cancellation reason contains a NUL byte");
    }
    Ok(())
}

pub(super) async fn drive_dynamic_workflow_control_future<F>(
    engine: &FlowEngine,
    run_id: &str,
    drive_context: DynamicWorkflowDriveContext<'_>,
    lease: &DynamicWorkflowLease,
    future: F,
) -> Result<DynamicWorkflowExecution>
where
    F: Future<Output = a3s_flow::Result<WorkflowRunSnapshot>>,
{
    let DynamicWorkflowDriveContext {
        source,
        input_for_identity,
        runtime_build_id,
        workflow_context: _workflow_context,
        parent_cancellation,
        child_cancellation,
    } = drive_context;
    tokio::pin!(future);

    async fn collect_result(
        engine: &FlowEngine,
        run_id: &str,
        result: a3s_flow::Result<WorkflowRunSnapshot>,
        source: &str,
        input: &Value,
        runtime_build_id: &str,
    ) -> Result<DynamicWorkflowExecution> {
        let snapshot = result?;
        collect_dynamic_workflow_execution(
            engine,
            run_id,
            snapshot,
            source,
            input,
            runtime_build_id,
        )
        .await
    }

    let heartbeat_period = Duration::from_millis((lease.lease_ms / 3).max(1));
    let first_heartbeat = tokio::time::Instant::now() + heartbeat_period;
    let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_period);
    loop {
        tokio::select! {
            biased;
            result = &mut future => {
                let result = collect_result(
                    engine,
                    run_id,
                    result,
                    source,
                    input_for_identity,
                    runtime_build_id,
                ).await;
                return settle_dynamic_workflow_lease(lease, result).await;
            }
            _ = parent_cancellation.cancelled() => {
                DynamicWorkflowMetrics::increment(&lease.metrics.cancellations);
                child_cancellation.cancel();
                match tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut future).await {
                    Ok(result) => {
                        let result = collect_result(
                            engine,
                            run_id,
                            result,
                            source,
                            input_for_identity,
                            runtime_build_id,
                        ).await;
                        match settle_dynamic_workflow_lease(lease, result).await {
                            Ok(execution) if execution.snapshot.status.is_terminal() => {
                                return Ok(execution)
                            }
                            Ok(_) => anyhow::bail!("dynamic workflow control cancelled by its parent"),
                            Err(error) => return Err(error).context("settle dynamic workflow control cancellation"),
                        }
                    }
                    Err(_) => anyhow::bail!(
                        "dynamic workflow control cancellation did not settle within {} seconds; worker lease remains fenced until expiry",
                        MAX_DYNAMIC_WORKFLOW_SETTLE.as_secs()
                    ),
                }
            }
            _ = heartbeat.tick() => {
                match lease.renew().await {
                    Ok(true) => {}
                    Ok(false) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut future)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        anyhow::bail!("dynamic workflow control worker lease was lost before completion");
                    }
                    Err(error) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut future)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        return Err(error).context("renew dynamic workflow control worker lease");
                    }
                }
            }
        }
    }
}

pub(super) async fn settle_dynamic_workflow_lease(
    lease: &DynamicWorkflowLease,
    result: Result<DynamicWorkflowExecution>,
) -> Result<DynamicWorkflowExecution> {
    match result {
        Ok(execution) if execution.snapshot.status.is_terminal() => {
            lease
                .complete()
                .await
                .context("complete dynamic workflow worker lease")?;
            Ok(execution)
        }
        Ok(execution) => {
            lease
                .release()
                .await
                .context("release suspended dynamic workflow worker lease")?;
            Ok(execution)
        }
        Err(error) => {
            if let Err(release_error) = lease.release().await {
                tracing::warn!(
                    error = %release_error,
                    "failed to release dynamic workflow worker lease after execution error"
                );
            }
            Err(error)
        }
    }
}

pub(super) async fn drive_dynamic_workflow_with_lease(
    engine: &FlowEngine,
    run_id: &str,
    spec: WorkflowSpec,
    input: Value,
    drive_context: DynamicWorkflowDriveContext<'_>,
    lease: &DynamicWorkflowLease,
) -> Result<DynamicWorkflowExecution> {
    let DynamicWorkflowDriveContext {
        source,
        input_for_identity,
        runtime_build_id,
        workflow_context,
        parent_cancellation,
        child_cancellation,
    } = drive_context;
    let execution = async {
        let started_run_id = engine.start_with_id(run_id, spec, input).await?;
        if started_run_id != run_id {
            anyhow::bail!(
                "dynamic workflow engine returned unexpected run id `{started_run_id}` for `{run_id}`"
            );
        }
        let snapshot = drive_inline_retries(engine, run_id, workflow_context).await?;
        collect_dynamic_workflow_execution(
            engine,
            run_id,
            snapshot,
            source,
            input_for_identity,
            runtime_build_id,
        )
        .await
    };
    tokio::pin!(execution);

    let heartbeat_period = Duration::from_millis((lease.lease_ms / 3).max(1));
    let first_heartbeat = tokio::time::Instant::now() + heartbeat_period;
    let mut heartbeat = tokio::time::interval_at(first_heartbeat, heartbeat_period);
    loop {
        tokio::select! {
            biased;
            result = &mut execution => {
                return settle_dynamic_workflow_lease(lease, result).await;
            }
            _ = parent_cancellation.cancelled() => {
                DynamicWorkflowMetrics::increment(&lease.metrics.cancellations);
                child_cancellation.cancel();
                match tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut execution).await {
                    Ok(result) => {
                        match settle_dynamic_workflow_lease(lease, result).await {
                            Ok(execution) if execution.snapshot.status.is_terminal() => {
                                return Ok(execution)
                            }
                            Ok(_) => anyhow::bail!("dynamic workflow cancelled by its parent"),
                            Err(error) => return Err(error).context("settle dynamic workflow cancellation"),
                        }
                    }
                    Err(_) => anyhow::bail!(
                        "dynamic workflow cancellation did not settle within {} seconds; worker lease remains fenced until expiry",
                        MAX_DYNAMIC_WORKFLOW_SETTLE.as_secs()
                    ),
                }
            }
            _ = heartbeat.tick() => {
                match lease.renew().await {
                    Ok(true) => {}
                    Ok(false) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut execution)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        anyhow::bail!("dynamic workflow worker lease was lost before execution completed");
                    }
                    Err(error) => {
                        child_cancellation.cancel();
                        if tokio::time::timeout(MAX_DYNAMIC_WORKFLOW_SETTLE, &mut execution)
                            .await
                            .is_ok()
                        {
                            let _ = lease.release().await;
                        }
                        return Err(error).context("renew dynamic workflow worker lease");
                    }
                }
            }
        }
    }
}

/// Drive short, persisted step retries inside the originating tool call.
///
/// A3S Flow deliberately suspends at a delayed retry boundary. Interactive
/// waits and hooks must remain suspended for an external host, but a bounded
/// retry delay is ordinary fault recovery: returning it as a terminal tool
/// error forces every caller to reimplement the scheduler and previously made
/// DeepResearch abandon its event-sourced run. Retry attempts and their delay
/// remain authoritative in the Flow journal; this helper only waits for the
/// due time and asks the engine to replay the same run.
pub(super) async fn drive_inline_retries(
    engine: &FlowEngine,
    run_id: &str,
    ctx: &ToolContext,
) -> Result<WorkflowRunSnapshot> {
    for _ in 0..MAX_INLINE_RETRY_RESUMES {
        let snapshot = engine.snapshot(run_id).await?;
        if snapshot.status.is_terminal() {
            return Ok(snapshot);
        }
        let Some(retry_after) = snapshot
            .steps
            .values()
            .filter(|step| step.status == StepStatus::Pending)
            .filter_map(|step| step.retry_after)
            .min()
        else {
            return Ok(snapshot);
        };
        let delay = retry_after
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or_default();
        if delay > MAX_INLINE_RETRY_DELAY {
            return Ok(snapshot);
        }
        let cancellation = ctx.cancellation_token();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                anyhow::bail!("dynamic_workflow cancelled while waiting for a scheduled retry");
            }
            _ = tokio::time::sleep(delay) => {}
        }
        engine.drive(run_id).await?;
    }
    engine.snapshot(run_id).await.map_err(Into::into)
}
