//! Inline governance for the existing fact model child. No writer, proxy or watcher.

use super::*;
use crate::agent::invocation_context::FactInvocationView;
use crate::harness_evidence::{
    ModelInputSnapshotV1, ModelPresentationSnapshotV1, RunCapabilitySnapshotV1,
};
use a3s_effect::ActorError;

/// Opaque framework request. Hosts may forward it, but cannot mint authority.
pub struct FactModelBindingRequest {
    submitted: Arc<dyn LlmClient>,
    view: FactInvocationView,
    admission: ModelGenerationAdmission,
}

/// Opaque binding to an immutable actual framework facade/transport.
/// It deliberately exposes no client, guard, invocation or permit accessor.
pub struct FactModelBinding {
    submitted: Arc<dyn LlmClient>,
    transport: Arc<dyn LlmClient>,
    original: FactInvocationView,
    admission: ModelGenerationAdmission,
    preadmitted: Arc<Mutex<Option<Arc<ModelGenerationPermit>>>>,
    queue_wait: Arc<AtomicU64>,
    middleware: Arc<super::super::ModelMiddlewareObs>,
    application: ModelPresentationApplicationV1,
}

pub(crate) enum FactModelFrame {
    RawStream(StreamEvent),
    Capability {
        call_sequence: u64,
        snapshot: RunCapabilitySnapshotV1,
    },
    Presentation(ModelPresentationSnapshotV1),
    Input(ModelInputSnapshotV1),
    Usage(ModelUsageSnapshotV1),
    Threshold(BudgetDecision),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactAccountingDisposition {
    NoSuccessObserved,
    CallbackReturned,
    NoGuard,
    Unproved,
}

impl FactAccountingDisposition {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoSuccessObserved => "no_success_observed",
            Self::CallbackReturned => "callback_returned",
            Self::NoGuard => "no_guard",
            Self::Unproved => "unproved",
        }
    }
    #[cfg(test)]
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "no_success_observed" => Ok(Self::NoSuccessObserved),
            "callback_returned" => Ok(Self::CallbackReturned),
            "no_guard" => Ok(Self::NoGuard),
            "unproved" => Ok(Self::Unproved),
            _ => anyhow::bail!("unknown FactAccountingDisposition: {value}"),
        }
    }
}

#[derive(Debug)]
enum AccountingState {
    NoSuccessObserved,
    Required(TokenUsage),
    Returned {
        // Retained actual usage; inspected by diagnostic tests, not release authority.
        _usage: TokenUsage,
        guard_present: bool,
    },
}

/// Passive observation of the actual child's original callback future.
/// Poison is retained as Unproved; it never suppresses an owed callback.
#[derive(Debug)]
pub(crate) struct FactModelAccountingObservation {
    state: Mutex<AccountingState>,
    pub(crate) last_projection: Mutex<FactProjectionObservation>,
    pub(crate) usage_projection: Mutex<FactProjectionObservation>,
}

#[derive(Debug, Default)]
pub(crate) enum FactStoreObservation {
    #[default]
    NotAttempted,
    NotConfigured,
    Entered,
    Returned(Option<Box<crate::run::RunSnapshot>>),
}

#[derive(Debug, Default)]
pub(crate) enum FactOutputObservation {
    #[default]
    NotAttempted,
    NotConfigured,
    Entered,
    Returned(std::result::Result<(), Box<mpsc::error::SendError<AgentEvent>>>),
}

#[derive(Debug, Default)]
pub(crate) struct FactProjectionObservation {
    // Retained enqueue observation, separate from store/output return evidence.
    pub(crate) _queued: bool,
    pub(crate) store: FactStoreObservation,
    pub(crate) output: FactOutputObservation,
}

impl Default for FactModelAccountingObservation {
    fn default() -> Self {
        Self {
            state: Mutex::new(AccountingState::NoSuccessObserved),
            last_projection: Mutex::new(FactProjectionObservation::default()),
            usage_projection: Mutex::new(FactProjectionObservation::default()),
        }
    }
}

impl FactModelAccountingObservation {
    fn required(&self, usage: &TokenUsage) {
        if let Ok(mut state) = self.state.lock() {
            *state = AccountingState::Required(usage.clone());
        }
    }
    fn returned(&self, guard_present: bool) {
        if let Ok(mut state) = self.state.lock() {
            if let AccountingState::Required(usage) = &*state {
                *state = AccountingState::Returned {
                    _usage: usage.clone(),
                    guard_present,
                };
            }
        }
    }
    pub(crate) fn disposition(&self) -> FactAccountingDisposition {
        match self.state.lock().as_deref() {
            Ok(AccountingState::NoSuccessObserved) => FactAccountingDisposition::NoSuccessObserved,
            Ok(AccountingState::Returned {
                guard_present: true,
                ..
            }) => FactAccountingDisposition::CallbackReturned,
            Ok(AccountingState::Returned {
                guard_present: false,
                ..
            }) => FactAccountingDisposition::NoGuard,
            _ => FactAccountingDisposition::Unproved,
        }
    }
    pub(crate) fn projection_has_missing_return(&self) -> bool {
        [&self.last_projection, &self.usage_projection]
            .into_iter()
            .any(|observation| {
                observation.lock().map_or(true, |state| {
                    matches!(state.store, FactStoreObservation::Entered)
                        || matches!(state.output, FactOutputObservation::Entered)
                })
            })
    }
    #[cfg(test)]
    pub(crate) fn observed_usage(&self) -> Option<TokenUsage> {
        match self.state.lock().as_deref() {
            Ok(
                AccountingState::Required(usage) | AccountingState::Returned { _usage: usage, .. },
            ) => Some(usage.clone()),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct FactModelCallOutcome {
    pub(crate) accounting: FactAccountingDisposition,
}

/// Actual owned future inputs. The framework-owned strong graph is limited to
/// original provider transport/permit/counters, passive view and request data.
pub(crate) struct PreparedFactModelCall {
    transport: Arc<dyn LlmClient>,
    view: FactInvocationView,
    admission: ModelGenerationAdmission,
    preadmitted: Arc<Mutex<Option<Arc<ModelGenerationPermit>>>>,
    queue_wait: Arc<AtomicU64>,
    middleware: Arc<super::super::ModelMiddlewareObs>,
    application: ModelPresentationApplicationV1,
    messages: Vec<Message>,
    system: Option<String>,
    tools: Vec<ToolDefinition>,
}

pub(super) fn bind_invoker(
    invoker: &LlmInvoker,
    request: &FactModelBindingRequest,
) -> Option<FactModelBinding> {
    // Unknown forwarding wrappers cannot attest an immutable framework
    // transport: the submitted object must be this actual facade object.
    if !std::ptr::addr_eq(
        Arc::as_ptr(&request.submitted),
        invoker as *const LlmInvoker,
    ) || !invoker.invocation.matches_fact_view(&request.view)
        || !invoker
            .model_generation_admission
            .same_original_gate(&request.admission)
        || invoker.inner.model_generation_is_managed()
    {
        return None;
    }
    let permit = invoker.preadmitted_permit.lock().ok()?;
    if permit
        .as_ref()
        .is_some_and(|permit| !request.admission.owns(permit))
    {
        return None;
    }
    drop(permit);
    Some(FactModelBinding {
        submitted: Arc::clone(&request.submitted),
        transport: Arc::clone(&invoker.inner),
        original: request.view.clone(),
        admission: invoker.model_generation_admission.clone(),
        preadmitted: Arc::clone(&invoker.preadmitted_permit),
        queue_wait: Arc::clone(&invoker.queue_wait_micros),
        middleware: Arc::clone(&invoker.middleware_obs),
        application: invoker.presentation_application,
    })
}

impl FactModelBinding {
    fn validate_current(&self, current: &FactModelBindingRequest) -> anyhow::Result<()> {
        if !Arc::ptr_eq(&self.submitted, &current.submitted)
            || !self.original.same_original(&current.view)
            || !self.admission.same_original_gate(&current.admission)
            || self.transport.model_generation_is_managed()
        {
            anyhow::bail!("foreign or stale fact model binding");
        }
        let permit = self
            .preadmitted
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned original fact permit"))?;
        if permit
            .as_ref()
            .is_some_and(|permit| !current.admission.owns(permit))
        {
            anyhow::bail!("foreign fact generation permit");
        }
        // Private construction plus actual submitted immutable facade identity
        // proves this stored transport/counter/decorator lineage. A host cannot
        // rewrite these fields from a new request or self-described label.
        Ok(())
    }
}

fn original_request(
    agent: &AgentLoop,
    expected_session_id: &str,
    submitted: Arc<dyn LlmClient>,
) -> anyhow::Result<FactModelBindingRequest> {
    let original = agent.bound_invocation.as_ref().ok_or_else(|| {
        anyhow::anyhow!("fact model requires the actual original bound invocation")
    })?;
    let view = original.freeze_for_fact_attempt()?;
    if view.run_id().is_empty()
        || view.session_id().is_empty()
        || view.session_id() != expected_session_id
    {
        anyhow::bail!("fact model original invocation is unbound");
    }
    Ok(FactModelBindingRequest {
        admission: agent.model_generation_admission_for_client(Some(&submitted)),
        submitted,
        view,
    })
}

impl crate::agent::TurnEngine {
    pub(crate) fn prepare_fact_model_call(
        &self,
        expected_session_id: &str,
        submitted: Arc<dyn LlmClient>,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Vec<ToolDefinition>,
    ) -> anyhow::Result<PreparedFactModelCall> {
        let request = original_request(self, expected_session_id, submitted)?;
        let binding = match request.submitted.bind_fact_model_governance(&request) {
            Some(binding) => binding,
            None if request.submitted.model_generation_is_managed() => {
                anyhow::bail!("managed fact model has no compatible typed original binding")
            }
            None => FactModelBinding {
                submitted: Arc::clone(&request.submitted),
                transport: Arc::clone(&request.submitted),
                original: request.view.clone(),
                admission: request.admission.clone(),
                preadmitted: Arc::new(Mutex::new(None)),
                queue_wait: Arc::new(AtomicU64::new(0)),
                middleware: Arc::clone(&self.middleware_obs),
                application: ModelPresentationApplicationV1::Auxiliary,
            },
        };
        binding.validate_current(&request)?;
        Ok(PreparedFactModelCall {
            transport: binding.transport,
            // Current parent freeze is data, not rewritten binding authority.
            // All binding provenance above was validated before selecting it.
            view: request.view,
            admission: binding.admission,
            preadmitted: binding.preadmitted,
            queue_wait: binding.queue_wait,
            middleware: binding.middleware,
            application: binding.application,
            messages,
            system,
            tools,
        })
    }
}

impl PreparedFactModelCall {
    async fn send(
        &self,
        sender: &mpsc::Sender<FactModelFrame>,
        frame: FactModelFrame,
        attempt: &CancellationToken,
    ) -> anyhow::Result<()> {
        tokio::select! {
            biased;
            _ = self.view.cancellation().cancelled() => anyhow::bail!("Operation cancelled by user"),
            _ = attempt.cancelled() => anyhow::bail!("Operation cancelled by user"),
            sent = sender.send(frame) => sent.map_err(|_| anyhow::anyhow!("fact observation receiver closed")),
        }
    }

    async fn admit_leg(
        &self,
        sender: &mpsc::Sender<FactModelFrame>,
        attempt: &CancellationToken,
        streaming: bool,
    ) -> anyhow::Result<(ModelUsageBinding, Arc<ModelGenerationPermit>)> {
        admit_model_trust(&self.messages, &self.middleware, streaming)?;
        let estimate = estimate_prompt_tokens(&self.messages, self.system.as_deref(), &self.tools);
        let decision = tokio::select! {
            biased;
            _ = attempt.cancelled() => anyhow::bail!("Operation cancelled by user"),
            decision = budget_decision_before_llm(self.view.budget_guard(), self.view.session_id(), estimate, self.view.cancellation()) => decision?,
        };
        if !matches!(decision, BudgetDecision::Allow) {
            self.send(sender, FactModelFrame::Threshold(decision.clone()), attempt)
                .await?;
        }
        budget_decision_result(&decision)?;
        let evidence = self
            .view
            .capture(ModelCallObservation::with_presentation_application(
                ModelInputKindV1::Completion,
                &self.messages,
                self.system.as_deref(),
                &self.tools,
                None,
                estimate,
                self.application,
            ))?;
        let binding = ModelUsageBinding::from_input(&evidence.input, evidence.tool_result_context);
        self.send(
            sender,
            FactModelFrame::Capability {
                call_sequence: evidence.input.call_sequence,
                snapshot: evidence.capability,
            },
            attempt,
        )
        .await?;
        self.send(
            sender,
            FactModelFrame::Presentation(evidence.presentation),
            attempt,
        )
        .await?;
        self.send(sender, FactModelFrame::Input(evidence.input), attempt)
            .await?;
        let original_permit = {
            let mut original = self
                .preadmitted
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned original fact permit"))?;
            if original
                .as_ref()
                .is_some_and(|permit| !self.admission.owns(permit))
            {
                anyhow::bail!("foreign fact generation permit");
            }
            original.take()
        };
        let permit = tokio::select! {
            biased;
            _ = attempt.cancelled() => anyhow::bail!("Operation cancelled by user"),
            permit = acquire_generation(&self.admission, original_permit, &self.queue_wait, self.view.cancellation()) => permit?,
        };
        self.middleware.record_provider_call();
        Ok((binding, permit))
    }

    async fn finish_success(
        &self,
        response: LlmResponse,
        binding: ModelUsageBinding,
        sender: &mpsc::Sender<FactModelFrame>,
        attempt: &CancellationToken,
        accounting: &FactModelAccountingObservation,
    ) -> anyhow::Result<FactModelCallOutcome> {
        // The caller retained the winning actual Ok/Done and synchronously
        // marked Required before entering this callback future.
        record_after_llm(
            self.view.budget_guard(),
            self.view.session_id(),
            &response.usage,
        )
        .await;
        accounting.returned(self.view.budget_guard().is_some());
        self.middleware.record_usage();
        let outcome = FactModelCallOutcome {
            accounting: accounting.disposition(),
        };
        let usage = ModelUsageSnapshotV1::from_binding(&binding, &response.usage)?;
        // Optional late telemetry cannot undo an actual returned callback.
        // Parent cancellation closes observation, never the owed callback.
        if self
            .send(sender, FactModelFrame::Usage(usage), attempt)
            .await
            .is_ok()
        {
            let _ = self
                .send(
                    sender,
                    FactModelFrame::RawStream(StreamEvent::Done(response)),
                    attempt,
                )
                .await;
        }
        Ok(outcome)
    }

    pub(crate) async fn run(
        self,
        sender: mpsc::Sender<FactModelFrame>,
        attempt: CancellationToken,
        accounting: Arc<FactModelAccountingObservation>,
    ) -> Result<FactModelCallOutcome, ActorError> {
        self.run_inner(&sender, &attempt, &accounting)
            .await
            .map_err(crate::fact_control::model_error)
    }

    async fn run_inner(
        &self,
        sender: &mpsc::Sender<FactModelFrame>,
        attempt: &CancellationToken,
        accounting: &FactModelAccountingObservation,
    ) -> anyhow::Result<FactModelCallOutcome> {
        let (binding, permit) = self.admit_leg(sender, attempt, true).await?;
        let stream = tokio::select! {
            biased;
            _ = self.view.cancellation().cancelled() => anyhow::bail!("Operation cancelled by user"),
            _ = attempt.cancelled() => anyhow::bail!("Operation cancelled by user"),
            stream = self.transport.complete_streaming(&self.messages, self.system.as_deref(), &self.tools, attempt.clone()) => stream,
        };
        match stream {
            Ok(mut stream) => loop {
                let event = tokio::select! {
                    biased;
                    _ = self.view.cancellation().cancelled() => anyhow::bail!("Operation cancelled by user"),
                    _ = attempt.cancelled() => anyhow::bail!("Operation cancelled by user"),
                    event = stream.recv() => event,
                };
                match event {
                    Some(StreamEvent::Done(response)) => {
                        // No EOF wait and no cancellation check between the
                        // winning provider poll and marking this actual success.
                        accounting.required(&response.usage);
                        let result = self
                            .finish_success(response, binding, sender, attempt, accounting)
                            .await;
                        drop(permit);
                        return result;
                    }
                    Some(event) => {
                        self.send(sender, FactModelFrame::RawStream(event), attempt)
                            .await?
                    }
                    None => anyhow::bail!("stream ended before a response"),
                }
            },
            Err(error) => {
                drop(permit);
                if self.view.cancellation().is_cancelled() || attempt.is_cancelled() {
                    anyhow::bail!("Operation cancelled by user");
                }
                if crate::llm::non_retryable_llm_error_message(&error).is_some() {
                    return Err(error);
                }
                // Failed-leg billing is unknown. The actual fallback re-enters
                // the SAME policy with a distinct original input sequence.
                let (binding, permit) = self.admit_leg(sender, attempt, false).await?;
                let response = tokio::select! {
                    biased;
                    _ = self.view.cancellation().cancelled() => anyhow::bail!("Operation cancelled by user"),
                    _ = attempt.cancelled() => anyhow::bail!("Operation cancelled by user"),
                    response = self.transport.complete(&self.messages, self.system.as_deref(), &self.tools) => response?,
                };
                accounting.required(&response.usage);
                let result = self
                    .finish_success(response, binding, sender, attempt, accounting)
                    .await;
                drop(permit);
                result
            }
        }
    }
}

#[cfg(test)]
mod tests;
