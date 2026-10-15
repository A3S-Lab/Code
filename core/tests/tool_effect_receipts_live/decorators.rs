#![cfg(test)]

//! Pausing decorators over the real provider and the real file store.
//! Neither decorator fabricates a response, receipt, stored value, or counter.

use super::{announce_barrier, append_json, sync_write, Window};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::config::CodeConfig;
use a3s_code_core::llm::structured::{NativeStructuredSupport, StructuredDirective};
use a3s_code_core::llm::{
    create_client_with_config, ContentBlock, GatewayDecisionAuth, LlmClient, LlmResponse, Message,
    ModelGenerationAdmission, ModelGenerationConcurrency, ModelGenerationPermit,
    ModelGenerationPool, StreamEvent, ToolDefinition,
};
use a3s_code_core::loop_checkpoint::LoopCheckpoint;
use a3s_code_core::orchestration::WorkflowCheckpoint;
use a3s_code_core::run::RunRecord;
use a3s_code_core::store::{
    FileSessionStore, SessionData, SessionSnapshotV1, SessionStore, SessionStoreCapabilities,
    SessionStoreCommitWatch, SessionStoreWriterLeaseV1,
};
use a3s_code_core::subagent_task_tracker::SubagentTaskSnapshot;
use a3s_code_core::tools::ArtifactStore;
use a3s_code_core::trace::TraceEvent;
use a3s_code_core::verification::VerificationReport;
use anyhow::Result;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct ProviderProbe {
    root: PathBuf,
    stage: String,
    model: String,
    window: Window,
    pause_after_terminal: bool,
    calls: AtomicU64,
}

struct ObservedProvider {
    inner: Arc<dyn LlmClient>,
    probe: Arc<ProviderProbe>,
}

/// The client is built from the same ACL model route as the normal session
/// factory. Sampling/deadline/session overrides match the parent test options.
pub(super) fn observed_provider(
    config: &CodeConfig,
    model: &str,
    root: &Path,
    stage: &str,
    window: Window,
    pause_after_terminal: bool,
) -> Arc<dyn LlmClient> {
    let (provider, model_id) = model.split_once('/').expect("pinned provider/model");
    let llm = config
        .llm_config(provider, model_id)
        .expect("live ACL provider credentials and declared model")
        .with_temperature(0.0)
        .with_api_timeout(180_000)
        .with_session_id(window.session());
    sync_write(
        &root.join(format!("provider-bounds-{stage}.json")),
        &serde_json::to_vec_pretty(&json!({
            "model":model, "temperature":llm.temperature,
            "provider_output_token_cap":llm.max_tokens,
            "provider_api_timeout_ms":llm.api_timeout_ms,
            "provider_retry_policy":llm.retry_config.clone().unwrap_or_default(),
            "thinking_budget":llm.thinking_budget, "reasoning_effort":llm.reasoning_effort,
            "total_token_budget":null, "monetary_cap":null,
            "note":"unknown total/monetary budgets are not zero; parent bounds wall time and tool rounds; transport retries are retained as configuration, not invented call counts",
        }))
        .unwrap(),
    )
    .expect("retain only non-secret effective model bounds");
    Arc::new(ObservedProvider {
        inner: create_client_with_config(llm),
        probe: Arc::new(ProviderProbe {
            root: root.to_path_buf(),
            stage: stage.into(),
            model: model.into(),
            window,
            pause_after_terminal,
            calls: AtomicU64::new(0),
        }),
    })
}

impl ObservedProvider {
    fn wrap(&self, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(Self {
            inner,
            probe: Arc::clone(&self.probe),
        })
    }

    async fn before_call(&self, messages: &[Message], transport: &str) -> u64 {
        if self.probe.pause_after_terminal {
            let facts = super::facts(&self.probe.root, self.probe.window);
            let terminal = super::receipts(&facts).into_iter().find(|receipt| {
                super::preparations(&facts).iter().any(|prepared| {
                    prepared.tool_name == self.probe.window.tool()
                        && prepared.identity == receipt.identity
                })
            });
            if let Some(terminal) = terminal {
                let result_in_context = messages.iter().any(|message| {
                    message.content.iter().any(|block| {
                        matches!(block, ContentBlock::ToolResult { tool_use_id, .. }
                            if tool_use_id == &terminal.identity.tool_call_id)
                    })
                });
                assert!(
                    result_in_context,
                    "the terminal barrier must be the actual next model call with its tool result"
                );
                announce_barrier(&self.probe.root, self.probe.window, "terminal-committed");
                std::future::pending::<()>().await;
            }
        }
        let call = self.probe.calls.fetch_add(1, Ordering::SeqCst) + 1;
        append_json(
            &self
                .probe
                .root
                .join(format!("provider-{}.jsonl", self.probe.stage)),
            &json!({
                "schema":"a3s.test.real-provider-observation.v1", "phase":"started",
                "stage":self.probe.stage, "model":self.probe.model,
                "transport":transport, "call":call,
                "usage":null, "billing_cost":null,
                "note":"actual configured provider delegated; incomplete or unobserved calls remain unknown",
            }),
        );
        call
    }

    fn returned(&self, call: u64, response: &LlmResponse) {
        let observed_usage = (response.usage.prompt_tokens > 0
            || response.usage.completion_tokens > 0
            || response.usage.total_tokens > 0)
            .then_some(&response.usage);
        append_json(
            &self
                .probe
                .root
                .join(format!("provider-{}.jsonl", self.probe.stage)),
            &json!({
                "schema":"a3s.test.real-provider-observation.v1", "phase":"returned",
                "stage":self.probe.stage, "model":self.probe.model, "call":call,
                "tool_calls":response.tool_calls(), "usage":observed_usage,
                "raw_provider_usage_fields":response.usage,
                "billing_cost":null,
                "note":"all-default zero usage is unqualified and remains unknown; raw provider fields are retained separately; billing is unknown, never free",
            }),
        );
    }

    fn observe_stream(
        &self,
        call: u64,
        mut receiver: mpsc::Receiver<StreamEvent>,
    ) -> mpsc::Receiver<StreamEvent> {
        let probe = Arc::clone(&self.probe);
        let inner = Arc::clone(&self.inner);
        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            let observer = Self { inner, probe };
            while let Some(event) = receiver.recv().await {
                if let StreamEvent::Done(response) = &event {
                    observer.returned(call, response);
                }
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        rx
    }
}

#[async_trait::async_trait]
impl LlmClient for ObservedProvider {
    fn gateway_decision_auth(&self) -> Option<GatewayDecisionAuth> {
        self.inner.gateway_decision_auth()
    }

    fn model_generation_concurrency(&self) -> ModelGenerationConcurrency {
        self.inner.model_generation_concurrency()
    }

    fn model_generation_pool(&self) -> Option<ModelGenerationPool> {
        self.inner.model_generation_pool()
    }

    fn bind_model_generation_admission(
        &self,
        admission: ModelGenerationAdmission,
        preadmitted: Option<Arc<ModelGenerationPermit>>,
    ) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .bind_model_generation_admission(admission, preadmitted)
            .map(|inner| self.wrap(inner))
    }

    fn model_generation_is_managed(&self) -> bool {
        self.inner.model_generation_is_managed()
    }

    fn take_model_generation_queue_wait(&self) -> Duration {
        self.inner.take_model_generation_queue_wait()
    }

    fn fork_for_session(&self, session_id: &str) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .fork_for_session(session_id)
            .map(|inner| self.wrap(inner))
    }

    fn with_active_generation_timeout(&self, timeout: Duration) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .with_active_generation_timeout(timeout)
            .map(|inner| self.wrap(inner))
    }

    fn with_output_cap(&self, max_tokens: u32) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .with_output_cap(max_tokens)
            .map(|inner| self.wrap(inner))
    }

    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let call = self.before_call(messages, "complete").await;
        let response = self.inner.complete(messages, system, tools).await?;
        self.returned(call, &response);
        Ok(response)
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let call = self.before_call(messages, "streaming").await;
        let receiver = self
            .inner
            .complete_streaming(messages, system, tools, cancel_token)
            .await?;
        Ok(self.observe_stream(call, receiver))
    }

    fn native_structured_support(&self) -> NativeStructuredSupport {
        self.inner.native_structured_support()
    }

    fn has_distinct_non_streaming_transport(&self) -> bool {
        self.inner.has_distinct_non_streaming_transport()
    }

    async fn complete_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &StructuredDirective,
    ) -> Result<LlmResponse> {
        let call = self.before_call(messages, "structured").await;
        let response = self
            .inner
            .complete_structured(messages, system, tools, directive)
            .await?;
        self.returned(call, &response);
        Ok(response)
    }

    async fn complete_streaming_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &StructuredDirective,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let call = self.before_call(messages, "streaming-structured").await;
        let receiver = self
            .inner
            .complete_streaming_structured(messages, system, tools, directive, cancel_token)
            .await?;
        Ok(self.observe_stream(call, receiver))
    }
}

/// Preserve every FileSessionStore method/capability. Only the addressed
/// checkpoint write is paused, before the real store is asked to commit it.
pub(super) struct CheckpointBarrierStore {
    pub(super) inner: Arc<FileSessionStore>,
    pub(super) root: PathBuf,
    pub(super) window: Window,
}

#[async_trait::async_trait]
impl SessionStore for CheckpointBarrierStore {
    async fn save(&self, session: &SessionData) -> Result<()> {
        self.inner.save(session).await
    }
    async fn load(&self, id: &str) -> Result<Option<SessionData>> {
        self.inner.load(id).await
    }
    async fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id).await
    }
    async fn list(&self) -> Result<Vec<String>> {
        self.inner.list().await
    }
    async fn exists(&self, id: &str) -> Result<bool> {
        self.inner.exists(id).await
    }
    async fn save_snapshot(&self, snapshot: &SessionSnapshotV1) -> Result<()> {
        self.inner.save_snapshot(snapshot).await
    }
    async fn save_snapshot_cas(
        &self,
        snapshot: &SessionSnapshotV1,
        expected_current_digest: Option<&str>,
    ) -> Result<bool> {
        self.inner
            .save_snapshot_cas(snapshot, expected_current_digest)
            .await
    }
    async fn acquire_writer_lease(&self, holder_id: &str) -> Result<SessionStoreWriterLeaseV1> {
        self.inner.acquire_writer_lease(holder_id).await
    }
    async fn writer_lease(&self) -> Result<Option<SessionStoreWriterLeaseV1>> {
        self.inner.writer_lease().await
    }
    async fn watch_commits(&self) -> Result<SessionStoreCommitWatch> {
        self.inner.watch_commits().await
    }
    async fn load_snapshot(&self, id: &str) -> Result<Option<SessionSnapshotV1>> {
        self.inner.load_snapshot(id).await
    }
    fn capabilities(&self) -> SessionStoreCapabilities {
        self.inner.capabilities()
    }
    async fn save_artifacts(&self, id: &str, artifacts: &ArtifactStore) -> Result<()> {
        self.inner.save_artifacts(id, artifacts).await
    }
    async fn load_artifacts(&self, id: &str) -> Result<Option<ArtifactStore>> {
        self.inner.load_artifacts(id).await
    }
    async fn save_trace_events(&self, id: &str, events: &[TraceEvent]) -> Result<()> {
        self.inner.save_trace_events(id, events).await
    }
    async fn load_trace_events(&self, id: &str) -> Result<Option<Vec<TraceEvent>>> {
        self.inner.load_trace_events(id).await
    }
    async fn save_run_records(&self, id: &str, records: &[RunRecord]) -> Result<()> {
        self.inner.save_run_records(id, records).await
    }
    async fn load_run_records(&self, id: &str) -> Result<Option<Vec<RunRecord>>> {
        self.inner.load_run_records(id).await
    }
    async fn save_verification_reports(
        &self,
        id: &str,
        reports: &[VerificationReport],
    ) -> Result<()> {
        self.inner.save_verification_reports(id, reports).await
    }
    async fn load_verification_reports(&self, id: &str) -> Result<Option<Vec<VerificationReport>>> {
        self.inner.load_verification_reports(id).await
    }
    async fn save_subagent_tasks(&self, id: &str, tasks: &[SubagentTaskSnapshot]) -> Result<()> {
        self.inner.save_subagent_tasks(id, tasks).await
    }
    async fn load_subagent_tasks(&self, id: &str) -> Result<Option<Vec<SubagentTaskSnapshot>>> {
        self.inner.load_subagent_tasks(id).await
    }
    async fn save_loop_checkpoint(&self, run_id: &str, checkpoint: &LoopCheckpoint) -> Result<()> {
        checkpoint.ensure_addressed_by(run_id)?;
        anyhow::ensure!(
            checkpoint.session_id == self.window.session(),
            "checkpoint belongs to this child Session"
        );
        sync_write(
            &self.root.join("checkpoint-request.json"),
            &serde_json::to_vec_pretty(checkpoint)?,
        )?;
        announce_barrier(&self.root, self.window, "checkpoint-before-commit");
        std::future::pending::<()>().await;
        self.inner.save_loop_checkpoint(run_id, checkpoint).await
    }
    async fn load_loop_checkpoint(&self, run_id: &str) -> Result<Option<LoopCheckpoint>> {
        self.inner.load_loop_checkpoint(run_id).await
    }
    async fn delete_loop_checkpoint(&self, run_id: &str) -> Result<()> {
        self.inner.delete_loop_checkpoint(run_id).await
    }
    async fn save_workflow_checkpoint(
        &self,
        workflow_id: &str,
        checkpoint: &WorkflowCheckpoint,
    ) -> Result<()> {
        self.inner
            .save_workflow_checkpoint(workflow_id, checkpoint)
            .await
    }
    async fn load_workflow_checkpoint(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowCheckpoint>> {
        self.inner.load_workflow_checkpoint(workflow_id).await
    }
    async fn delete_workflow_checkpoint(&self, workflow_id: &str) -> Result<()> {
        self.inner.delete_workflow_checkpoint(workflow_id).await
    }
    async fn health_check(&self) -> Result<()> {
        self.inner.health_check().await
    }
    fn backend_name(&self) -> &str {
        self.inner.backend_name()
    }
}
