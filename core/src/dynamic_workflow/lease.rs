use super::*;

#[derive(Clone)]
pub(super) struct DynamicWorkflowLease {
    pub(super) ledger: Arc<dyn FlowDecisionLedger>,
    pub(super) decision_id: String,
    pub(super) request_hash: String,
    pub(super) identity: ExecutionIdentityV1,
    pub(super) owner_id: String,
    pub(super) lease_ms: u64,
    pub(super) attempt: u32,
    pub(super) metrics: Arc<DynamicWorkflowMetrics>,
}

impl DynamicWorkflowLease {
    pub(super) async fn renew(&self) -> Result<bool> {
        let result = self
            .ledger
            .renew_with_identity(
                &self.decision_id,
                &self.request_hash,
                &self.identity,
                &self.owner_id,
                dynamic_workflow_now_ms(),
                self.lease_ms,
            )
            .await;
        match &result {
            Ok(true) => DynamicWorkflowMetrics::increment(&self.metrics.renewals),
            Ok(false) => DynamicWorkflowMetrics::increment(&self.metrics.lease_lost),
            Err(_) => {
                DynamicWorkflowMetrics::increment(&self.metrics.lease_lost);
                DynamicWorkflowMetrics::increment(&self.metrics.failures);
            }
        }
        result
    }

    pub(super) async fn complete(&self) -> Result<()> {
        let result = self
            .ledger
            .complete_with_identity(
                &self.decision_id,
                &self.request_hash,
                &self.identity,
                &self.owner_id,
                dynamic_workflow_now_ms(),
            )
            .await;
        match &result {
            Ok(()) => DynamicWorkflowMetrics::increment(&self.metrics.completions),
            Err(_) => DynamicWorkflowMetrics::increment(&self.metrics.failures),
        }
        result
    }

    pub(super) async fn release(&self) -> Result<()> {
        let result = self
            .ledger
            .release_with_identity(
                &self.decision_id,
                &self.request_hash,
                &self.identity,
                &self.owner_id,
            )
            .await;
        match &result {
            Ok(()) => DynamicWorkflowMetrics::increment(&self.metrics.releases),
            Err(_) => DynamicWorkflowMetrics::increment(&self.metrics.failures),
        }
        result
    }
}

pub(super) enum DynamicWorkflowLeaseClaim {
    Owned(DynamicWorkflowLease),
    AlreadyCompleted,
}

pub(super) fn dynamic_workflow_lease_key(identity: &ExecutionIdentityV1) -> (String, String) {
    (
        format!("dynamic-workflow:{}", identity.digest),
        identity.digest.clone(),
    )
}

pub(super) async fn claim_dynamic_workflow_lease(
    ledger: Arc<dyn FlowDecisionLedger>,
    identity: ExecutionIdentityV1,
    lease_ms: u64,
    metrics: Arc<DynamicWorkflowMetrics>,
) -> Result<DynamicWorkflowLeaseClaim> {
    DynamicWorkflowMetrics::increment(&metrics.claim_attempts);
    let mut in_flight = DynamicWorkflowClaimInFlight::new(Arc::clone(&metrics));
    identity
        .validate()
        .map_err(|error| anyhow::anyhow!(error))?;
    let (decision_id, request_hash) = dynamic_workflow_lease_key(&identity);
    let owner_id = format!("dynamic-workflow-worker-{}", uuid::Uuid::new_v4());
    let outcome = ledger
        .claim_with_identity(
            &decision_id,
            &request_hash,
            &identity,
            &owner_id,
            dynamic_workflow_now_ms(),
            lease_ms,
        )
        .await
        .context("admit dynamic workflow worker lease")?;
    let result = match outcome {
        FlowDecisionClaimOutcome::Claimed { attempt } => {
            DynamicWorkflowMetrics::increment(&metrics.claims);
            if attempt > 1 {
                DynamicWorkflowMetrics::increment(&metrics.takeovers);
            }
            Ok(DynamicWorkflowLeaseClaim::Owned(DynamicWorkflowLease {
                ledger,
                decision_id,
                request_hash,
                identity,
                owner_id,
                lease_ms,
                attempt,
                metrics,
            }))
        }
        FlowDecisionClaimOutcome::Completed => {
            DynamicWorkflowMetrics::increment(&metrics.already_completed);
            Ok(DynamicWorkflowLeaseClaim::AlreadyCompleted)
        }
        FlowDecisionClaimOutcome::Busy {
            lease_expires_at_ms,
        } => {
            DynamicWorkflowMetrics::increment(&metrics.busy);
            anyhow::bail!("dynamic workflow worker lease is busy until {lease_expires_at_ms}")
        }
        FlowDecisionClaimOutcome::Conflict => {
            DynamicWorkflowMetrics::increment(&metrics.conflicts);
            anyhow::bail!("dynamic workflow worker lease identity conflicts with its claim")
        }
    };
    in_flight.finish();
    result
}

pub(super) struct DynamicWorkflowClaimInFlight {
    pub(super) metrics: Arc<DynamicWorkflowMetrics>,
    pub(super) finished: bool,
}

impl DynamicWorkflowClaimInFlight {
    pub(super) fn new(metrics: Arc<DynamicWorkflowMetrics>) -> Self {
        DynamicWorkflowMetrics::increment(&metrics.in_flight);
        Self {
            metrics,
            finished: false,
        }
    }

    pub(super) fn finish(&mut self) {
        if !self.finished {
            self.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
            self.finished = true;
        }
    }
}

impl Drop for DynamicWorkflowClaimInFlight {
    fn drop(&mut self) {
        if !self.finished {
            self.finish();
            DynamicWorkflowMetrics::increment(&self.metrics.failures);
        }
    }
}

pub(super) fn dynamic_workflow_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub(super) fn is_terminal_workflow_event(event: &FlowEvent) -> bool {
    matches!(
        event,
        FlowEvent::RunCompleted { .. }
            | FlowEvent::RunFailed { .. }
            | FlowEvent::RunCancelled { .. }
            | FlowEvent::RunTimedOut { .. }
            | FlowEvent::RunRetryExhausted { .. }
            | FlowEvent::RunHostShutdown { .. }
            | FlowEvent::RunContinuedAsNew { .. }
    )
}

pub(super) fn history_is_terminal(history: &[FlowEventEnvelope]) -> bool {
    history
        .last()
        .is_some_and(|envelope| is_terminal_workflow_event(&envelope.event))
}
