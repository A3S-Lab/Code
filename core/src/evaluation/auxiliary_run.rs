//! Isolated, host-supplied auxiliary evaluation runs.

use super::evidence::EvidenceSnapshotV1;
use super::identity::{digest_bytes, digest_json, validate_digest, ExecutionFrameV1};
use async_trait::async_trait;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio_util::sync::CancellationToken;

pub const AUXILIARY_RUN_SCHEMA_V1: &str = "a3s.code.auxiliary-run.v1";
pub const AUXILIARY_OUTPUT_SCHEMA_V1: &str = "a3s.code.auxiliary-output.v1";
pub const AUXILIARY_SNAPSHOT_SCHEMA_V1: &str = "a3s.code.auxiliary-run-snapshot.v1";
pub const AUXILIARY_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
pub const AUXILIARY_MAX_STEPS: u32 = 1_000_000;
const MAX_PURPOSE_BYTES: usize = 256;
const MAX_INSTRUCTION_BYTES: usize = 128 * 1024;
const MAX_MODEL_REF_BYTES: usize = 512;
const MAX_SCHEMA_BYTES: usize = 128 * 1024;
const MAX_ERROR_BYTES: usize = 16 * 1024;
const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryModeV1 {
    Detached,
    Advisory,
    Gate,
}

/// Explicit capability profile for an auxiliary run.  The service never
/// grants capabilities implicitly; a host can additionally provide a parent
/// ceiling and the requested profile must be a subset of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryCapabilityProfileV1 {
    pub read_workspace: bool,
    pub write_workspace: bool,
    pub execute_commands: bool,
    pub network: bool,
    pub spawn_children: bool,
    pub max_output_bytes: usize,
}

impl AuxiliaryCapabilityProfileV1 {
    pub const fn tool_free() -> Self {
        Self {
            read_workspace: false,
            write_workspace: false,
            execute_commands: false,
            network: false,
            spawn_children: false,
            max_output_bytes: 64 * 1024,
        }
    }

    pub const fn read_only(max_output_bytes: usize) -> Self {
        Self {
            read_workspace: true,
            write_workspace: false,
            execute_commands: false,
            network: false,
            spawn_children: false,
            max_output_bytes,
        }
    }

    pub fn validate(&self) -> Result<(), AuxiliaryRunError> {
        if self.max_output_bytes == 0 || self.max_output_bytes > AUXILIARY_MAX_OUTPUT_BYTES {
            return Err(AuxiliaryRunError::InvalidField(
                "capabilities.max_output_bytes",
            ));
        }
        Ok(())
    }

    pub const fn is_within(self, ceiling: Self) -> bool {
        (!self.read_workspace || ceiling.read_workspace)
            && (!self.write_workspace || ceiling.write_workspace)
            && (!self.execute_commands || ceiling.execute_commands)
            && (!self.network || ceiling.network)
            && (!self.spawn_children || ceiling.spawn_children)
            && self.max_output_bytes <= ceiling.max_output_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryRunSpecV1 {
    pub schema: String,
    pub id: String,
    pub parent: ExecutionFrameV1,
    pub purpose: String,
    pub mode: AuxiliaryModeV1,
    pub instruction: String,
    pub evidence_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_ref: Option<String>,
    pub capabilities: AuxiliaryCapabilityProfileV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ceiling: Option<AuxiliaryCapabilityProfileV1>,
    pub max_steps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
}

impl AuxiliaryRunSpecV1 {
    pub fn new(
        parent: ExecutionFrameV1,
        purpose: impl Into<String>,
        instruction: impl Into<String>,
        evidence_digest: impl Into<String>,
    ) -> Self {
        Self {
            schema: AUXILIARY_RUN_SCHEMA_V1.to_string(),
            id: format!("aux-{}", uuid::Uuid::new_v4()),
            parent,
            purpose: purpose.into(),
            mode: AuxiliaryModeV1::Detached,
            instruction: instruction.into(),
            evidence_digest: evidence_digest.into(),
            model_ref: None,
            capabilities: AuxiliaryCapabilityProfileV1::tool_free(),
            parent_ceiling: None,
            max_steps: 1,
            timeout_ms: None,
            output_schema: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_mode(mut self, mode: AuxiliaryModeV1) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_model_ref(mut self, model_ref: impl Into<String>) -> Self {
        self.model_ref = Some(model_ref.into());
        self
    }

    pub fn with_capabilities(mut self, capabilities: AuxiliaryCapabilityProfileV1) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_parent_ceiling(mut self, ceiling: AuxiliaryCapabilityProfileV1) -> Self {
        self.parent_ceiling = Some(ceiling);
        self
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    pub fn validate(&self, evidence_digest: &str) -> Result<(), AuxiliaryRunError> {
        if self.schema != AUXILIARY_RUN_SCHEMA_V1 {
            return Err(AuxiliaryRunError::UnsupportedSchema);
        }
        self.parent
            .validate()
            .map_err(|_| AuxiliaryRunError::InvalidField("parent"))?;
        validate_text("id", &self.id, MAX_PURPOSE_BYTES)?;
        validate_text("purpose", &self.purpose, MAX_PURPOSE_BYTES)?;
        if self.instruction.is_empty()
            || self.instruction.len() > MAX_INSTRUCTION_BYTES
            || self.instruction.contains('\0')
        {
            return Err(AuxiliaryRunError::InvalidField("instruction"));
        }
        validate_digest(&self.evidence_digest)
            .map_err(|_| AuxiliaryRunError::InvalidField("evidence_digest"))?;
        if self.evidence_digest != evidence_digest {
            return Err(AuxiliaryRunError::EvidenceMismatch);
        }
        if let Some(model_ref) = &self.model_ref {
            validate_text("model_ref", model_ref, MAX_MODEL_REF_BYTES)?;
        }
        self.capabilities.validate()?;
        if let Some(ceiling) = self.parent_ceiling {
            ceiling.validate()?;
            if !self.capabilities.is_within(ceiling) {
                return Err(AuxiliaryRunError::CapabilityEscalation);
            }
        }
        if self.max_steps == 0 || self.max_steps > AUXILIARY_MAX_STEPS {
            return Err(AuxiliaryRunError::InvalidField("max_steps"));
        }
        if self
            .timeout_ms
            .is_some_and(|timeout| timeout == 0 || timeout > MAX_TIMEOUT_MS)
        {
            return Err(AuxiliaryRunError::InvalidField("timeout_ms"));
        }
        if let Some(schema) = &self.output_schema {
            let bytes = serde_json::to_vec(schema)
                .map_err(|error| AuxiliaryRunError::Serialization(error.to_string()))?;
            if bytes.len() > MAX_SCHEMA_BYTES {
                return Err(AuxiliaryRunError::InvalidField("output_schema"));
            }
            jsonschema::draft202012::options()
                .build(schema)
                .map_err(|_| AuxiliaryRunError::InvalidField("output_schema"))?;
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, AuxiliaryRunError> {
        self.validate(&self.evidence_digest)?;
        digest_json("a3s.code.auxiliary-run.identity.v1", self)
            .map_err(|error| AuxiliaryRunError::Serialization(error.to_string()))
    }
}

#[derive(Clone)]
pub struct AuxiliaryRunContextV1 {
    pub spec: AuxiliaryRunSpecV1,
    pub evidence: EvidenceSnapshotV1,
    pub cancellation: CancellationToken,
}

impl std::fmt::Debug for AuxiliaryRunContextV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuxiliaryRunContextV1")
            .field("spec", &self.spec)
            .field("evidence_digest", &self.evidence.snapshot_digest)
            .field("cancelled", &self.cancellation.is_cancelled())
            .finish()
    }
}

#[async_trait]
pub trait AuxiliaryExecutor: Send + Sync {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError>;
}

/// Adapter for the provider-neutral structured-output engine.  The request
/// factory is host-owned so Core does not prescribe a rubric, prompt, or
/// result vocabulary.
pub struct StructuredAuxiliaryExecutor {
    client: Arc<dyn crate::llm::LlmClient>,
    request_factory: Arc<
        dyn Fn(&AuxiliaryRunContextV1) -> crate::llm::structured::StructuredRequest + Send + Sync,
    >,
}

impl std::fmt::Debug for StructuredAuxiliaryExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StructuredAuxiliaryExecutor")
            .finish_non_exhaustive()
    }
}

impl StructuredAuxiliaryExecutor {
    pub fn new<F>(client: Arc<dyn crate::llm::LlmClient>, request_factory: F) -> Self
    where
        F: Fn(&AuxiliaryRunContextV1) -> crate::llm::structured::StructuredRequest
            + Send
            + Sync
            + 'static,
    {
        Self {
            client,
            request_factory: Arc::new(request_factory),
        }
    }
}

#[async_trait]
impl AuxiliaryExecutor for StructuredAuxiliaryExecutor {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError> {
        if context.cancellation.is_cancelled() {
            return Err(AuxiliaryRunError::Cancelled);
        }
        let request = (self.request_factory)(&context);
        let result = crate::llm::structured::generate_blocking(self.client.as_ref(), &request)
            .await
            .map_err(|error| AuxiliaryRunError::Executor(error.to_string()))?;
        if context.cancellation.is_cancelled() {
            return Err(AuxiliaryRunError::Cancelled);
        }
        Ok(result.object)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryRunStateV1 {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl AuxiliaryRunStateV1 {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryRunOutputV1 {
    pub schema: String,
    pub value: serde_json::Value,
    pub output_bytes: u64,
    pub output_digest: String,
}

impl AuxiliaryRunOutputV1 {
    fn from_value(value: serde_json::Value) -> Result<Self, AuxiliaryRunError> {
        let encoded = serde_json::to_vec(&value)
            .map_err(|error| AuxiliaryRunError::Serialization(error.to_string()))?;
        Ok(Self {
            schema: AUXILIARY_OUTPUT_SCHEMA_V1.to_string(),
            output_bytes: u64::try_from(encoded.len())
                .map_err(|_| AuxiliaryRunError::NumericOverflow)?,
            output_digest: digest_bytes("a3s.code.auxiliary-output.value.v1", &encoded),
            value,
        })
    }

    /// Validate a serialized output at a host/process boundary.
    pub fn validate(
        &self,
        max_bytes: usize,
        schema: Option<&serde_json::Value>,
    ) -> Result<(), AuxiliaryRunError> {
        if self.schema != AUXILIARY_OUTPUT_SCHEMA_V1 {
            return Err(AuxiliaryRunError::UnsupportedSchema);
        }
        let encoded = serde_json::to_vec(&self.value)
            .map_err(|error| AuxiliaryRunError::Serialization(error.to_string()))?;
        let encoded_bytes =
            u64::try_from(encoded.len()).map_err(|_| AuxiliaryRunError::NumericOverflow)?;
        if encoded.len() > max_bytes || self.output_bytes != encoded_bytes {
            return Err(AuxiliaryRunError::OutputLimit);
        }
        validate_digest(&self.output_digest)
            .map_err(|_| AuxiliaryRunError::InvalidField("output_digest"))?;
        if self.output_digest != digest_bytes("a3s.code.auxiliary-output.value.v1", &encoded) {
            return Err(AuxiliaryRunError::DigestMismatch("output_digest"));
        }
        if let Some(schema) = schema {
            let validator = jsonschema::draft202012::options()
                .build(schema)
                .map_err(|_| AuxiliaryRunError::InvalidField("output_schema"))?;
            if validator.iter_errors(&self.value).next().is_some() {
                return Err(AuxiliaryRunError::OutputSchemaMismatch);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryRunSnapshotV1 {
    pub schema: String,
    pub id: String,
    pub parent: ExecutionFrameV1,
    pub mode: AuxiliaryModeV1,
    pub state: AuxiliaryRunStateV1,
    pub spec_digest: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AuxiliaryRunSnapshotV1 {
    pub fn validate(&self) -> Result<(), AuxiliaryRunError> {
        if self.schema != AUXILIARY_SNAPSHOT_SCHEMA_V1 {
            return Err(AuxiliaryRunError::UnsupportedSchema);
        }
        validate_text("id", &self.id, MAX_PURPOSE_BYTES)?;
        self.parent
            .validate()
            .map_err(|_| AuxiliaryRunError::InvalidField("parent"))?;
        validate_digest(&self.spec_digest)
            .map_err(|_| AuxiliaryRunError::InvalidField("spec_digest"))?;
        if self.output_digest.is_some() {
            validate_digest(
                self.output_digest
                    .as_deref()
                    .ok_or(AuxiliaryRunError::InvalidField("output_digest"))?,
            )
            .map_err(|_| AuxiliaryRunError::InvalidField("output_digest"))?;
        }
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.len() > MAX_ERROR_BYTES)
        {
            return Err(AuxiliaryRunError::InvalidField("error"));
        }
        if self.updated_at_ms < self.created_at_ms {
            return Err(AuxiliaryRunError::InvalidField("updated_at_ms"));
        }
        match self.state {
            AuxiliaryRunStateV1::Completed
                if self.output_digest.is_none() || self.error.is_some() =>
            {
                return Err(AuxiliaryRunError::InvalidField("state"));
            }
            AuxiliaryRunStateV1::Queued | AuxiliaryRunStateV1::Running
                if self.output_digest.is_some() || self.error.is_some() =>
            {
                return Err(AuxiliaryRunError::InvalidField("state"));
            }
            AuxiliaryRunStateV1::Failed
            | AuxiliaryRunStateV1::Cancelled
            | AuxiliaryRunStateV1::TimedOut
                if self.output_digest.is_some() || self.error.is_none() =>
            {
                return Err(AuxiliaryRunError::InvalidField("state"));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuxiliaryRunError {
    #[error("auxiliary run schema is unsupported")]
    UnsupportedSchema,
    #[error("auxiliary run field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("auxiliary run evidence does not match the spec")]
    EvidenceMismatch,
    #[error("auxiliary run parent target does not match its evidence target")]
    TargetMismatch,
    #[error("auxiliary run would exceed its parent capability ceiling")]
    CapabilityEscalation,
    #[error("auxiliary run output exceeds its bounded contract")]
    OutputLimit,
    #[error("auxiliary run output does not match its schema")]
    OutputSchemaMismatch,
    #[error("auxiliary run digest for `{0}` does not match")]
    DigestMismatch(&'static str),
    #[error("auxiliary run is already registered with a different spec")]
    Conflict,
    #[error("auxiliary run was not found")]
    NotFound,
    #[error("auxiliary run was cancelled")]
    Cancelled,
    #[error("auxiliary run timed out")]
    TimedOut,
    #[error("auxiliary run executor failed: {0}")]
    Executor(String),
    #[error("auxiliary run numeric value does not fit the wire type")]
    NumericOverflow,
    #[error("auxiliary run serialization failed: {0}")]
    Serialization(String),
}

#[derive(Debug)]
struct AuxiliaryEntry {
    spec: AuxiliaryRunSpecV1,
    evidence: EvidenceSnapshotV1,
    snapshot: Mutex<AuxiliaryRunSnapshotV1>,
    output: Mutex<Option<Result<AuxiliaryRunOutputV1, AuxiliaryRunError>>>,
    notify: Notify,
    cancellation: CancellationToken,
}

/// A cloneable capability to observe or cancel exactly one auxiliary run.
#[derive(Clone)]
pub struct AuxiliaryRunHandle {
    entry: Arc<AuxiliaryEntry>,
}

impl std::fmt::Debug for AuxiliaryRunHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuxiliaryRunHandle")
            .field("id", &self.entry.spec.id)
            .finish()
    }
}

impl AuxiliaryRunHandle {
    pub fn id(&self) -> &str {
        &self.entry.spec.id
    }

    pub async fn snapshot(&self) -> AuxiliaryRunSnapshotV1 {
        self.entry.snapshot.lock().await.clone()
    }

    pub async fn cancel(&self) -> bool {
        let snapshot = self.entry.snapshot.lock().await;
        if snapshot.state.is_terminal() {
            return false;
        }
        drop(snapshot);
        self.entry.cancellation.cancel();
        true
    }

    pub async fn wait(&self) -> Result<AuxiliaryRunOutputV1, AuxiliaryRunError> {
        loop {
            // Enable the notification before checking the output. Merely
            // constructing `Notified` does not register a waiter; `enable`
            // does. Without it, `notify_waiters` can run between the check
            // and the await and leave a waiter asleep forever.
            let notified = self.entry.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = self.entry.output.lock().await.clone() {
                // The worker publishes the terminal snapshot before the
                // output. Keep `wait` linearizable for callers that observe
                // both handles independently.
                if self.entry.snapshot.lock().await.state.is_terminal() {
                    return result;
                }
            }
            notified.await;
        }
    }
}

#[async_trait]
pub trait AuxiliaryRunService: Send + Sync {
    async fn spawn(
        &self,
        spec: AuxiliaryRunSpecV1,
        evidence: EvidenceSnapshotV1,
        parent_cancellation: Option<CancellationToken>,
    ) -> Result<AuxiliaryRunHandle, AuxiliaryRunError>;
    async fn get(&self, id: &str) -> Option<AuxiliaryRunSnapshotV1>;

    async fn list(&self) -> Vec<AuxiliaryRunSnapshotV1> {
        Vec::new()
    }

    async fn cancel(&self, _id: &str) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct InMemoryAuxiliaryRunService {
    entries: Arc<RwLock<HashMap<String, Arc<AuxiliaryEntry>>>>,
    order: Arc<RwLock<VecDeque<String>>>,
    executor: Arc<dyn AuxiliaryExecutor>,
    max_terminal_runs: Option<usize>,
}

impl std::fmt::Debug for InMemoryAuxiliaryRunService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryAuxiliaryRunService")
            .field(
                "entry_count",
                &self.entries.try_read().map(|entries| entries.len()).ok(),
            )
            .field("max_terminal_runs", &self.max_terminal_runs)
            .finish()
    }
}

impl InMemoryAuxiliaryRunService {
    pub fn new(executor: Arc<dyn AuxiliaryExecutor>) -> Self {
        Self::with_max_terminal_runs(executor, None)
    }

    pub fn with_max_terminal_runs(
        executor: Arc<dyn AuxiliaryExecutor>,
        max_terminal_runs: Option<usize>,
    ) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            order: Arc::new(RwLock::new(VecDeque::new())),
            executor,
            max_terminal_runs,
        }
    }

    async fn prune_terminal(&self) {
        let Some(limit) = self.max_terminal_runs else {
            return;
        };
        loop {
            let id = {
                let order = self.order.read().await;
                if order.len() <= limit {
                    return;
                }
                order.front().cloned()
            };
            let Some(id) = id else {
                return;
            };
            let terminal = self
                .entries
                .read()
                .await
                .get(&id)
                .cloned()
                .map(|entry| async move { entry.snapshot.lock().await.state.is_terminal() });
            let Some(terminal) = terminal else {
                let mut order = self.order.write().await;
                order.pop_front();
                continue;
            };
            if !terminal.await {
                return;
            }
            {
                let mut order = self.order.write().await;
                if order.front().is_some_and(|front| front == &id) {
                    order.pop_front();
                }
            }
            self.entries.write().await.remove(&id);
        }
    }

    async fn run_entry(self, entry: Arc<AuxiliaryEntry>) {
        {
            let mut snapshot = entry.snapshot.lock().await;
            if snapshot.state == AuxiliaryRunStateV1::Queued {
                snapshot.state = AuxiliaryRunStateV1::Running;
                snapshot.updated_at_ms = now_ms().max(snapshot.updated_at_ms);
            }
        }
        let context = AuxiliaryRunContextV1 {
            spec: entry.spec.clone(),
            evidence: entry.evidence.clone(),
            cancellation: entry.cancellation.clone(),
        };
        let execution = self.execute_with_deadline(context, &entry);
        let result = execution.await;
        let (state, stored_result, error_text) = match result {
            Ok(output) => (AuxiliaryRunStateV1::Completed, Ok(output), None),
            Err(error @ AuxiliaryRunError::Cancelled) => (
                AuxiliaryRunStateV1::Cancelled,
                Err(error.clone()),
                Some(bound_error(&error.to_string())),
            ),
            Err(error @ AuxiliaryRunError::TimedOut) => (
                AuxiliaryRunStateV1::TimedOut,
                Err(error.clone()),
                Some(bound_error(&error.to_string())),
            ),
            Err(error) => (
                AuxiliaryRunStateV1::Failed,
                Err(error.clone()),
                Some(bound_error(&error.to_string())),
            ),
        };
        let output_digest = match &stored_result {
            Ok(output) => Some(output.output_digest.clone()),
            Err(_) => None,
        };
        {
            let mut snapshot = entry.snapshot.lock().await;
            snapshot.state = state;
            snapshot.updated_at_ms = now_ms().max(snapshot.updated_at_ms);
            snapshot.error = error_text;
            snapshot.output_digest = output_digest;
        }
        {
            let mut output = entry.output.lock().await;
            *output = Some(stored_result);
        }
        entry.notify.notify_waiters();
        self.order.write().await.push_back(entry.spec.id.clone());
        self.prune_terminal().await;
    }

    async fn execute_with_deadline(
        &self,
        context: AuxiliaryRunContextV1,
        entry: &AuxiliaryEntry,
    ) -> Result<AuxiliaryRunOutputV1, AuxiliaryRunError> {
        let cancellation = entry.cancellation.clone();
        let executor = Arc::clone(&self.executor);
        let run = async move {
            tokio::select! {
                result = executor.execute(context) => result,
                _ = cancellation.cancelled() => Err(AuxiliaryRunError::Cancelled),
            }
        };
        let run = std::panic::AssertUnwindSafe(run).catch_unwind();
        let value = if let Some(timeout_ms) = entry.spec.timeout_ms {
            tokio::select! {
                result = tokio::time::timeout(Duration::from_millis(timeout_ms), run) => {
                    result
                        .map_err(|_| AuxiliaryRunError::TimedOut)?
                        .map_err(|_| AuxiliaryRunError::Executor("executor panicked".to_string()))?
                }
                _ = entry.cancellation.cancelled() => return Err(AuxiliaryRunError::Cancelled),
            }
        } else {
            run.await
                .map_err(|_| AuxiliaryRunError::Executor("executor panicked".to_string()))?
        }?;
        let output = AuxiliaryRunOutputV1::from_value(value)?;
        output.validate(
            entry.spec.capabilities.max_output_bytes,
            entry.spec.output_schema.as_ref(),
        )?;
        Ok(output)
    }
}

#[async_trait]
impl AuxiliaryRunService for InMemoryAuxiliaryRunService {
    async fn spawn(
        &self,
        spec: AuxiliaryRunSpecV1,
        evidence: EvidenceSnapshotV1,
        parent_cancellation: Option<CancellationToken>,
    ) -> Result<AuxiliaryRunHandle, AuxiliaryRunError> {
        evidence
            .validate()
            .map_err(|_| AuxiliaryRunError::EvidenceMismatch)?;
        if spec.parent.target != evidence.target {
            return Err(AuxiliaryRunError::TargetMismatch);
        }
        spec.validate(&evidence.snapshot_digest)?;
        let spec_digest = spec.digest()?;
        let id = spec.id.clone();
        if let Some(existing) = self.entries.read().await.get(&id) {
            if existing.spec == spec
                && existing.evidence.snapshot_digest == evidence.snapshot_digest
            {
                return Ok(AuxiliaryRunHandle {
                    entry: Arc::clone(existing),
                });
            }
            return Err(AuxiliaryRunError::Conflict);
        }
        let now = now_ms();
        let cancellation = parent_cancellation
            .map(|parent| parent.child_token())
            .unwrap_or_default();
        let entry = Arc::new(AuxiliaryEntry {
            spec: spec.clone(),
            evidence,
            snapshot: Mutex::new(AuxiliaryRunSnapshotV1 {
                schema: AUXILIARY_SNAPSHOT_SCHEMA_V1.to_string(),
                id: id.clone(),
                parent: spec.parent.clone(),
                mode: spec.mode,
                state: AuxiliaryRunStateV1::Queued,
                spec_digest,
                created_at_ms: now,
                updated_at_ms: now,
                output_digest: None,
                error: None,
            }),
            output: Mutex::new(None),
            notify: Notify::new(),
            cancellation,
        });
        let mut entries = self.entries.write().await;
        if let Some(existing) = entries.get(&id) {
            if existing.spec == spec {
                return Ok(AuxiliaryRunHandle {
                    entry: Arc::clone(existing),
                });
            }
            return Err(AuxiliaryRunError::Conflict);
        }
        entries.insert(id, Arc::clone(&entry));
        drop(entries);
        let service = self.clone();
        let task_entry = Arc::clone(&entry);
        tokio::spawn(async move { service.run_entry(task_entry).await });
        Ok(AuxiliaryRunHandle { entry })
    }

    async fn get(&self, id: &str) -> Option<AuxiliaryRunSnapshotV1> {
        let entry = self.entries.read().await.get(id).cloned()?;
        let snapshot = entry.snapshot.lock().await.clone();
        if snapshot.validate().is_err() {
            return None;
        }
        Some(snapshot)
    }

    async fn list(&self) -> Vec<AuxiliaryRunSnapshotV1> {
        let entries = self
            .entries
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(entries.len());
        for entry in entries {
            snapshots.push(entry.snapshot.lock().await.clone());
        }
        snapshots.sort_by_key(|snapshot| snapshot.created_at_ms);
        snapshots
    }

    async fn cancel(&self, id: &str) -> bool {
        let Some(entry) = self.entries.read().await.get(id).cloned() else {
            return false;
        };
        let handle = AuxiliaryRunHandle { entry };
        handle.cancel().await
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), AuxiliaryRunError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.contains('\0')
        || value.lines().count() != 1
    {
        return Err(AuxiliaryRunError::InvalidField(field));
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn bound_error(error: &str) -> String {
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_string();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    error[..end].to_string()
}

#[cfg(test)]
mod tests;
