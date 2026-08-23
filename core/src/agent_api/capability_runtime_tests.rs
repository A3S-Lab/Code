use super::*;

use crate::capability::{
    CapabilityAdapterError, CapabilityContribution, CapabilityDescriptor, CapabilityEffect,
    CapabilityEffectError, CapabilityKind, CapabilityProjectionAdapter, CapabilityRuntimeError,
    CapabilitySet, CapabilitySource, CapabilityValue, CodeCatalogGeneration, PreparedCapability,
    RetainedUseGeneration, SessionCapabilityBatch, Sha256Digest, UseCapabilityGeneration,
    UseGenerationLeaseError, UseGenerationLeaseProvider, UsePackageGeneration,
};
use crate::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use crate::skills::{Skill, SkillKind};
use crate::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

mod agent_projection;
mod command_projection;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

fn use_generation(generation: u64, revision: char) -> UseCapabilityGeneration {
    UseCapabilityGeneration::new(generation, digest(revision), digest('f'))
}

fn use_tool_set(
    code_generation: u64,
    upstream: UseCapabilityGeneration,
    names: &[(&str, char)],
) -> (
    Arc<CapabilitySet>,
    std::collections::BTreeMap<String, crate::capability::CapabilityId>,
) {
    use_kind_set(code_generation, upstream, CapabilityKind::Tool, names)
}

fn use_skill_set(
    code_generation: u64,
    upstream: UseCapabilityGeneration,
    names: &[(&str, char)],
) -> (
    Arc<CapabilitySet>,
    std::collections::BTreeMap<String, crate::capability::CapabilityId>,
) {
    use_kind_set(code_generation, upstream, CapabilityKind::Skill, names)
}

fn use_agent_set(
    code_generation: u64,
    upstream: UseCapabilityGeneration,
    names: &[(&str, char)],
) -> (
    Arc<CapabilitySet>,
    std::collections::BTreeMap<String, crate::capability::CapabilityId>,
) {
    use_kind_set(code_generation, upstream, CapabilityKind::Agent, names)
}

fn use_kind_set(
    code_generation: u64,
    upstream: UseCapabilityGeneration,
    kind: CapabilityKind,
    names: &[(&str, char)],
) -> (
    Arc<CapabilitySet>,
    std::collections::BTreeMap<String, crate::capability::CapabilityId>,
) {
    let source = CapabilitySource::use_package(
        upstream.clone(),
        UsePackageGeneration::new(
            "acme/runtime-tools",
            "use/acme-runtime-tools",
            "runtime-tools",
            "1.0.0",
            upstream.generation(),
            digest('d'),
            digest('e'),
        )
        .unwrap(),
    )
    .unwrap();
    let mut ids = std::collections::BTreeMap::new();
    let descriptors = names
        .iter()
        .map(|(name, surface_digest)| {
            let descriptor =
                CapabilityDescriptor::new(&source, kind, *name, *name, digest(*surface_digest), [])
                    .unwrap();
            ids.insert((*name).to_string(), descriptor.id().clone());
            descriptor
        })
        .collect::<Vec<_>>();
    let contribution = CapabilityContribution::new(source, descriptors).unwrap();
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(code_generation),
        upstream,
        [contribution],
    )
    .unwrap();
    (set, ids)
}

fn skill(name: &str, version: &str) -> Arc<Skill> {
    Arc::new(Skill {
        name: name.to_string(),
        description: version.to_string(),
        allowed_tools: Some("search_skills(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: format!("Pinned instructions for {version}."),
        tags: vec!["pinned".to_string()],
        version: Some(version.to_string()),
    })
}

fn projected_agent(name: &str, version: &str) -> Arc<crate::subagent::AgentDefinition> {
    Arc::new(
        crate::subagent::AgentDefinition::new(name, version)
            .with_prompt(&format!("PROJECTED_AGENT_{version}")),
    )
}

struct VersionedTool {
    name: String,
    version: &'static str,
    executions: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl Tool for VersionedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        self.version
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "generation": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        self.executions.lock().unwrap().push(self.version);
        Ok(ToolOutput::success(self.version))
    }
}

struct TestUseLease {
    generation: UseCapabilityGeneration,
    dropped: Arc<AtomicUsize>,
}

impl RetainedUseGeneration for TestUseLease {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }
}

impl Drop for TestUseLease {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

struct TestUseLeaseProvider {
    generation: UseCapabilityGeneration,
    acquired: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    returned_generation: Option<UseCapabilityGeneration>,
}

#[async_trait]
impl UseGenerationLeaseProvider for TestUseLeaseProvider {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }

    async fn acquire(
        &self,
        cancellation: CancellationToken,
    ) -> std::result::Result<Box<dyn RetainedUseGeneration>, UseGenerationLeaseError> {
        if cancellation.is_cancelled() {
            return Err(UseGenerationLeaseError::new("test acquisition cancelled"));
        }
        self.acquired.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(TestUseLease {
            generation: self
                .returned_generation
                .clone()
                .unwrap_or_else(|| self.generation.clone()),
            dropped: Arc::clone(&self.dropped),
        }))
    }
}

struct ReadyAdapter {
    value: CapabilityValue,
    effect: Option<Box<dyn CapabilityEffect>>,
}

#[async_trait]
impl CapabilityProjectionAdapter for ReadyAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> std::result::Result<PreparedCapability, CapabilityAdapterError> {
        let ReadyAdapter { value, effect } = *self;
        let mut prepared = PreparedCapability::new(value);
        if let Some(effect) = effect {
            prepared.push_boxed_effect(effect)?;
        }
        Ok(prepared)
    }
}

struct FailingAdapter;

#[async_trait]
impl CapabilityProjectionAdapter for FailingAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> std::result::Result<PreparedCapability, CapabilityAdapterError> {
        Err(CapabilityAdapterError::new("host batch failure"))
    }
}

struct CancellationWaitingAdapter {
    entered: Arc<Semaphore>,
}

#[async_trait]
impl CapabilityProjectionAdapter for CancellationWaitingAdapter {
    async fn prepare(
        self: Box<Self>,
        cancellation: CancellationToken,
    ) -> std::result::Result<PreparedCapability, CapabilityAdapterError> {
        self.entered.add_permits(1);
        cancellation.cancelled().await;
        Err(CapabilityAdapterError::new("preparation cancelled"))
    }
}

struct SignallingReadyAdapter {
    value: CapabilityValue,
    effect: Box<dyn CapabilityEffect>,
    prepared: Arc<Semaphore>,
}

#[async_trait]
impl CapabilityProjectionAdapter for SignallingReadyAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> std::result::Result<PreparedCapability, CapabilityAdapterError> {
        let SignallingReadyAdapter {
            value,
            effect,
            prepared,
        } = *self;
        let mut capability = PreparedCapability::new(value);
        capability.push_boxed_effect(effect)?;
        prepared.add_permits(1);
        Ok(capability)
    }
}

struct CountingEffect(Arc<AtomicUsize>);

#[async_trait]
impl CapabilityEffect for CountingEffect {
    fn name(&self) -> &str {
        "test.host-batch-effect"
    }

    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FailingEffect;

#[async_trait]
impl CapabilityEffect for FailingEffect {
    fn name(&self) -> &str {
        "test.failing-run-effect"
    }

    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        Err(CapabilityEffectError::new("deterministic close failure"))
    }
}

struct FailingUseLeaseProvider {
    generation: UseCapabilityGeneration,
}

#[async_trait]
impl UseGenerationLeaseProvider for FailingUseLeaseProvider {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }

    async fn acquire(
        &self,
        _cancellation: CancellationToken,
    ) -> std::result::Result<Box<dyn RetainedUseGeneration>, UseGenerationLeaseError> {
        Err(UseGenerationLeaseError::new(
            "deterministic Use lease failure",
        ))
    }
}

#[derive(Clone)]
struct NoopClient;

#[async_trait]
impl LlmClient for NoopClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("the no-op client must not be called")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("the no-op client must not be called")
    }
}

struct CutoverClient {
    calls: AtomicUsize,
    observed_definitions: Mutex<Vec<String>>,
    old_definition_observed: tokio::sync::Semaphore,
    release_old_call: tokio::sync::Semaphore,
}

impl CutoverClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            observed_definitions: Mutex::new(Vec::new()),
            old_definition_observed: tokio::sync::Semaphore::new(0),
            release_old_call: tokio::sync::Semaphore::new(0),
        }
    }

    fn observe_projected_definition(&self, tools: &[ToolDefinition]) -> anyhow::Result<()> {
        let definition = tools
            .iter()
            .find(|definition| definition.name == "projected")
            .ok_or_else(|| anyhow::anyhow!("projected Tool definition is missing"))?;
        self.observed_definitions
            .lock()
            .unwrap()
            .push(definition.description.clone());
        Ok(())
    }

    fn tool_call(generation: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: format!("projected-{generation}"),
                    name: "projected".to_string(),
                    input: serde_json::json!({"generation": generation}),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("tool_use".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }

    fn final_text(text: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("end_turn".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }
}

#[async_trait]
impl LlmClient for CutoverClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        // Session completion can perform auxiliary model work without the Run
        // tool surface (for example memory extraction). Keep those calls out
        // of the deterministic Run script.
        if !tools
            .iter()
            .any(|definition| definition.name == "projected")
        {
            return Ok(Self::final_text("auxiliary completion"));
        }
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.observe_projected_definition(tools)?;
                self.old_definition_observed.add_permits(1);
                self.release_old_call.acquire().await?.forget();
                Ok(Self::tool_call("one"))
            }
            1 => Ok(Self::final_text("generation one complete")),
            2 => {
                self.observe_projected_definition(tools)?;
                Ok(Self::tool_call("two"))
            }
            3 => Ok(Self::final_text("generation two complete")),
            call => anyhow::bail!("unexpected CutoverClient call {call}"),
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the cutover test")
    }
}

struct SkillCutoverClient {
    calls: AtomicUsize,
    observed_search_versions: Mutex<Vec<String>>,
    old_surface_observed: Semaphore,
    release_old_call: Semaphore,
}

impl SkillCutoverClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            observed_search_versions: Mutex::new(Vec::new()),
            old_surface_observed: Semaphore::new(0),
            release_old_call: Semaphore::new(0),
        }
    }

    fn search_call(id: &str, query: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "search_skills".to_string(),
                    input: serde_json::json!({"query": query}),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("tool_use".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }

    fn latest_tool_result(messages: &[Message]) -> anyhow::Result<String> {
        messages
            .iter()
            .rev()
            .flat_map(|message| message.content.iter().rev())
            .find_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content.as_text()),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("search_skills result is missing"))
    }
}

#[async_trait]
impl LlmClient for SkillCutoverClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if !tools
            .iter()
            .any(|definition| definition.name == "search_skills")
        {
            return Ok(CutoverClient::final_text("auxiliary completion"));
        }

        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.old_surface_observed.add_permits(1);
                self.release_old_call.acquire().await?.forget();
                Ok(Self::search_call("search-generation-one", "pinned-skill"))
            }
            1 => {
                let result = Self::latest_tool_result(messages)?;
                anyhow::ensure!(
                    result.contains("generation-one"),
                    "first Run search_skills resolved another generation: {result}"
                );
                self.observed_search_versions
                    .lock()
                    .unwrap()
                    .push("generation-one".to_string());
                Ok(CutoverClient::final_text("skill generation one complete"))
            }
            2 => Ok(Self::search_call("search-generation-two", "pinned")),
            3 => {
                let result = Self::latest_tool_result(messages)?;
                anyhow::ensure!(
                    result.contains("generation-two"),
                    "second Run search_skills resolved another generation: {result}"
                );
                self.observed_search_versions
                    .lock()
                    .unwrap()
                    .push("generation-two".to_string());
                Ok(CutoverClient::final_text("skill generation two complete"))
            }
            call => anyhow::bail!("unexpected SkillCutoverClient call {call}"),
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the Skill cutover test")
    }
}

async fn test_session(name: &str) -> AgentSession {
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    agent
        .build_session(
            format!("/tmp/{name}"),
            Arc::new(NoopClient),
            &SessionOptions::new().with_session_id(name),
        )
        .unwrap()
}

async fn test_session_with_client(name: &str, client: Arc<dyn LlmClient>) -> AgentSession {
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    agent
        .build_session(
            format!("/tmp/{name}"),
            client,
            &SessionOptions::new()
                .with_session_id(name)
                .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap()
}

fn provider(
    generation: UseCapabilityGeneration,
    acquired: &Arc<AtomicUsize>,
    dropped: &Arc<AtomicUsize>,
) -> Arc<dyn UseGenerationLeaseProvider> {
    Arc::new(TestUseLeaseProvider {
        generation,
        acquired: Arc::clone(acquired),
        dropped: Arc::clone(dropped),
        returned_generation: None,
    })
}

#[test]
fn session_batch_accepts_migrated_kinds_but_keeps_hook_fail_closed() {
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let agent_generation = use_generation(1, 'a');
    let (agent_set, _) = use_agent_set(1, agent_generation.clone(), &[("projected-agent", 'b')]);
    assert!(SessionCapabilityBatch::from_use_projection(
        agent_set,
        provider(agent_generation, &acquired, &dropped),
    )
    .is_ok());

    let command_generation = use_generation(2, 'c');
    let (command_set, _) = use_kind_set(
        2,
        command_generation.clone(),
        CapabilityKind::Command,
        &[("projected-command", 'd')],
    );
    assert!(SessionCapabilityBatch::from_use_projection(
        command_set,
        provider(command_generation, &acquired, &dropped),
    )
    .is_ok());

    let hook_generation = use_generation(3, 'e');
    let (hook_set, _) = use_kind_set(
        3,
        hook_generation.clone(),
        CapabilityKind::Hook,
        &[("projected-hook", 'f')],
    );
    assert!(matches!(
        SessionCapabilityBatch::from_use_projection(
            hook_set,
            provider(hook_generation, &acquired, &dropped),
        ),
        Err(CapabilityRuntimeError::UnsupportedSessionKind {
            kind: CapabilityKind::Hook,
        })
    ));
}

#[tokio::test]
async fn failed_session_batch_never_advances_the_visible_generation() {
    let session = test_session("capability-batch-atomicity").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(upstream, &acquired, &dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "generation-one",
                executions: Arc::clone(&executions),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let before = session.capability_catalog_stamp();
    let rollback_count = Arc::new(AtomicUsize::new(0));
    let next_upstream = use_generation(2, 'c');
    let (second_set, second_ids) =
        use_tool_set(2, next_upstream.clone(), &[("alpha", 'd'), ("omega", 'e')]);
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(next_upstream, &acquired, &dropped),
    )
    .unwrap();
    second
        .stage(
            second_ids["alpha"].clone(),
            ReadyAdapter {
                value: CapabilityValue::Tool(Arc::new(VersionedTool {
                    name: "alpha".to_string(),
                    version: "generation-two-alpha",
                    executions,
                })),
                effect: Some(Box::new(CountingEffect(Arc::clone(&rollback_count)))),
            },
        )
        .unwrap();
    second
        .stage(second_ids["omega"].clone(), FailingAdapter)
        .unwrap();

    assert!(matches!(
        session
            .apply_capability_batch(second, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::Projection(_))
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    let cleanup = session.drain_capability_cleanup().await;
    assert_eq!(cleanup.rollback_batches, 1);
    assert_eq!(rollback_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn run_admission_consumes_and_releases_the_exact_use_lease() {
    let session = test_session("capability-run-use-lease").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(7, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "lease-test",
                executions,
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = session.admit_capability_run().await.unwrap();
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    run.close().await.unwrap();
    drop(run);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn run_admission_rejects_a_cursor_mismatched_use_lease() {
    let session = test_session("capability-run-use-mismatch").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(9, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let mismatched_provider: Arc<dyn UseGenerationLeaseProvider> = Arc::new(TestUseLeaseProvider {
        generation: upstream,
        acquired: Arc::clone(&acquired),
        dropped: Arc::clone(&dropped),
        returned_generation: Some(use_generation(10, 'c')),
    });
    let mut batch = SessionCapabilityBatch::from_use_projection(set, mismatched_provider).unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "mismatch-test",
                executions,
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(
        session.admit_capability_run().await,
        Err(CapabilityRuntimeError::Scope(
            crate::capability::CapabilityScopeError::UseGenerationLeaseMismatch { .. }
        ))
    ));
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_run_keeps_n_definition_executor_and_use_lease_across_n_plus_one_cutover() {
    let client = Arc::new(CutoverClient::new());
    let session = Arc::new(
        test_session_with_client(
            "capability-run-cutover",
            Arc::clone(&client) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let executions = Arc::new(Mutex::new(Vec::new()));
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_tool_set(1, first_upstream.clone(), &[("projected", 'b')]);
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "generation-one",
                executions: Arc::clone(&executions),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Call the projected tool once.", None).await }
    });
    client
        .old_definition_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) = use_tool_set(2, second_upstream.clone(), &[("projected", 'd')]);
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "generation-two",
                executions: Arc::clone(&executions),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();

    // Publication N+1 cannot rewrite the already admitted N executor or
    // release its exact upstream lease while the model call is still active.
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    client.release_old_call.add_permits(1);
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "generation one complete");
    assert_eq!(&*executions.lock().unwrap(), &["generation-one"]);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new_result = session
        .send("Call the projected tool once.", None)
        .await
        .unwrap();
    assert_eq!(new_result.text, "generation two complete");
    assert_eq!(
        new_result.tool_calls_count, 1,
        "unexpected second-generation messages: {:#?}",
        new_result.messages
    );
    assert_eq!(
        &*executions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*client.observed_definitions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_host_batch_preparation_rolls_back_without_publication() {
    let session = Arc::new(test_session("capability-batch-cancellation").await);
    let before = session.capability_catalog_stamp();
    let closed_effects = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("alpha", 'b'), ("omega", 'c')]);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(
            ids["alpha"].clone(),
            ReadyAdapter {
                value: CapabilityValue::Tool(Arc::new(VersionedTool {
                    name: "alpha".to_string(),
                    version: "cancel-alpha",
                    executions,
                })),
                effect: Some(Box::new(CountingEffect(Arc::clone(&closed_effects)))),
            },
        )
        .unwrap();
    batch
        .stage(
            ids["omega"].clone(),
            CancellationWaitingAdapter {
                entered: Arc::clone(&entered),
            },
        )
        .unwrap();

    let cancellation = CancellationToken::new();
    let apply = tokio::spawn({
        let session = Arc::clone(&session);
        let cancellation = cancellation.clone();
        async move { session.apply_capability_batch(batch, cancellation).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    cancellation.cancel();

    assert!(matches!(
        apply.await.unwrap(),
        Err(CapabilityRuntimeError::Cancelled)
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    let cleanup = session.drain_capability_cleanup().await;
    assert_eq!(cleanup.rollback_batches, 1);
    assert_eq!(cleanup.effects_closed, 1);
    assert_eq!(closed_effects.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_close_cancels_preparation_and_releases_prepared_effects() {
    let session = Arc::new(test_session("capability-batch-close-cancellation").await);
    let before = session.capability_catalog_stamp();
    let closed_effects = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("alpha", 'b'), ("omega", 'c')]);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(
            ids["alpha"].clone(),
            ReadyAdapter {
                value: CapabilityValue::Tool(Arc::new(VersionedTool {
                    name: "alpha".to_string(),
                    version: "close-alpha",
                    executions,
                })),
                effect: Some(Box::new(CountingEffect(Arc::clone(&closed_effects)))),
            },
        )
        .unwrap();
    batch
        .stage(
            ids["omega"].clone(),
            CancellationWaitingAdapter {
                entered: Arc::clone(&entered),
            },
        )
        .unwrap();

    let apply = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .apply_capability_batch(batch, CancellationToken::new())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    session.close().await;

    assert!(matches!(
        apply.await.unwrap(),
        Err(CapabilityRuntimeError::SessionClosed)
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(closed_effects.load(Ordering::SeqCst), 1);
    assert_eq!(session.capability_catalog.pending_cleanup_batches(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn session_close_and_ready_batch_commit_have_a_single_linearization() {
    let session = Arc::new(test_session("capability-batch-close-commit-race").await);
    let before = session.capability_catalog_stamp();
    let closed_effects = Arc::new(AtomicUsize::new(0));
    let prepared = Arc::new(Semaphore::new(0));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(
            ids["projected"].clone(),
            SignallingReadyAdapter {
                value: CapabilityValue::Tool(Arc::new(VersionedTool {
                    name: "projected".to_string(),
                    version: "commit-race",
                    executions,
                })),
                effect: Box::new(CountingEffect(Arc::clone(&closed_effects))),
                prepared: Arc::clone(&prepared),
            },
        )
        .unwrap();

    let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let lock_owner = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            let guard = session
                .close_handle
                .immediate_extension_mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        })
    };
    locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let apply = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .apply_capability_batch(batch, CancellationToken::new())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), prepared.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let close = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.close().await }
    });
    tokio::task::yield_now().await;
    release_tx.send(()).unwrap();
    lock_owner.join().unwrap();

    let result = apply.await.unwrap();
    close.await.unwrap();
    match result {
        Ok(receipt) => {
            assert_eq!(receipt.previous(), &before);
            assert_eq!(receipt.committed().generation().get(), 1);
            assert_eq!(session.capability_catalog_stamp().generation().get(), 1);
        }
        Err(CapabilityRuntimeError::SessionClosed) => {
            assert_eq!(session.capability_catalog_stamp(), before);
        }
        Err(error) => panic!("unexpected close/commit race result: {error}"),
    }
    assert_eq!(closed_effects.load(Ordering::SeqCst), 1);
    assert_eq!(session.capability_catalog.pending_cleanup_batches(), 0);
}

#[tokio::test]
async fn compatibility_name_conflicts_fail_before_generation_publication() {
    let tool_session = test_session("capability-tool-name-conflict").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    tool_session
        .register_dynamic_tool(Arc::new(VersionedTool {
            name: "projected".to_string(),
            version: "compatibility",
            executions: Arc::clone(&executions),
        }))
        .unwrap();
    let tool_before = tool_session.capability_catalog_stamp();
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "use-projection",
                executions,
            })),
        )
        .unwrap();
    assert!(matches!(
        tool_session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Tool,
            ..
        })
    ));
    assert_eq!(tool_session.capability_catalog_stamp(), tool_before);

    let skill_session = test_session("capability-skill-name-conflict").await;
    skill_session
        .add_skill(skill("projected-skill", "compatibility"))
        .unwrap();
    let skill_before = skill_session.capability_catalog_stamp();
    let upstream = use_generation(1, 'c');
    let (set, ids) = use_skill_set(1, upstream.clone(), &[("projected-skill", 'd')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-skill"].clone(),
            CapabilityValue::Skill(skill("projected-skill", "use-projection")),
        )
        .unwrap();
    assert!(matches!(
        skill_session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Skill,
            ..
        })
    ));
    assert_eq!(skill_session.capability_catalog_stamp(), skill_before);

    // Agent lookup accepts compatibility aliases. A projected alias must not
    // bypass the canonical name already owned by the built-in registry.
    let agent_session = test_session("capability-agent-name-conflict").await;
    let agent_before = agent_session.capability_catalog_stamp();
    let upstream = use_generation(1, 'e');
    let (set, ids) = use_agent_set(1, upstream.clone(), &[("reviewer", 'f')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["reviewer"].clone(),
            CapabilityValue::Agent(projected_agent("reviewer", "use-projection")),
        )
        .unwrap();
    assert!(matches!(
        agent_session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Agent,
            ..
        })
    ));
    assert_eq!(agent_session.capability_catalog_stamp(), agent_before);
}

#[tokio::test]
async fn compatibility_mutation_cannot_shadow_a_published_projection() {
    let tool_session = test_session("capability-tool-post-publication-conflict").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "published",
                executions: Arc::clone(&executions),
            })),
        )
        .unwrap();
    tool_session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let tool_stamp = tool_session.capability_catalog_stamp();
    assert!(matches!(
        tool_session.register_dynamic_tool(Arc::new(VersionedTool {
            name: "projected".to_string(),
            version: "compatibility",
            executions,
        })),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Tool,
                ..
            }
        ))
    ));
    assert_eq!(tool_session.capability_catalog_stamp(), tool_stamp);

    let skill_session = test_session("capability-skill-post-publication-conflict").await;
    let upstream = use_generation(1, 'c');
    let (set, ids) = use_skill_set(1, upstream.clone(), &[("projected-skill", 'd')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-skill"].clone(),
            CapabilityValue::Skill(skill("projected-skill", "published")),
        )
        .unwrap();
    skill_session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let skill_stamp = skill_session.capability_catalog_stamp();
    assert!(matches!(
        skill_session.add_skill(skill("projected-skill", "compatibility")),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Skill,
                ..
            }
        ))
    ));
    assert_eq!(skill_session.capability_catalog_stamp(), skill_stamp);

    let agent_session = test_session("capability-agent-post-publication-conflict").await;
    let upstream = use_generation(1, 'e');
    let (set, ids) = use_agent_set(1, upstream.clone(), &[("published-agent", 'f')]);
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["published-agent"].clone(),
            CapabilityValue::Agent(projected_agent("published-agent", "published")),
        )
        .unwrap();
    agent_session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let agent_stamp = agent_session.capability_catalog_stamp();
    assert!(matches!(
        agent_session.register_worker_agent(crate::subagent::WorkerAgentSpec::custom(
            "published-agent",
            "Compatibility worker",
        )),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Agent,
                ..
            }
        ))
    ));
    assert!(!agent_session.agent_registry.exists("published-agent"));
    assert!(matches!(
        agent_session.register_worker_agents([
            crate::subagent::WorkerAgentSpec::custom("batch-safe-agent", "Must roll back"),
            crate::subagent::WorkerAgentSpec::custom(
                "published-agent",
                "Conflicting compatibility worker",
            ),
        ]),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Agent,
                ..
            }
        ))
    ));
    assert!(!agent_session.agent_registry.exists("batch-safe-agent"));

    let agent_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        agent_dir.path().join("directory-safe-agent.yaml"),
        "name: directory-safe-agent\ndescription: Must not partially register\n",
    )
    .unwrap();
    std::fs::write(
        agent_dir.path().join("published-agent.yaml"),
        "name: published-agent\ndescription: Compatibility directory agent\n",
    )
    .unwrap();
    assert!(matches!(
        agent_session.register_agent_dir(agent_dir.path()),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Agent,
                ..
            }
        ))
    ));
    assert!(!agent_session.agent_registry.exists("published-agent"));
    assert!(!agent_session.agent_registry.exists("directory-safe-agent"));
    assert_eq!(agent_session.capability_catalog_stamp(), agent_stamp);
}

#[tokio::test]
async fn exact_run_admission_failure_settles_the_reserved_run() {
    let session = test_session("capability-exact-run-admission-failure").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let provider: Arc<dyn UseGenerationLeaseProvider> = Arc::new(FailingUseLeaseProvider {
        generation: upstream,
    });
    let mut batch = SessionCapabilityBatch::from_use_projection(set, provider).unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "never-executed",
                executions,
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let error = match session
        .spawn_run_with_id("exact-capability-failure", "must not start")
        .await
    {
        Ok(_) => panic!("exact Run unexpectedly started"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        crate::error::CodeError::Capability(CapabilityRuntimeError::UseLeaseAcquisition { .. })
    ));
    let snapshot = session
        .run_snapshot("exact-capability-failure")
        .await
        .unwrap();
    assert_eq!(snapshot.status, crate::run::RunStatus::Failed);
    assert!(snapshot.error.unwrap().contains("Use generation lease"));
    assert!(session.current_run().await.is_none());
}

#[tokio::test]
async fn non_clean_capability_run_close_is_a_typed_failure() {
    let session = test_session("capability-run-close-failure").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(Mutex::new(Vec::new()));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_string(),
                version: "close-failure",
                executions,
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = session.admit_capability_run().await.unwrap();
    run.run_scope().register_effect(FailingEffect).unwrap();
    assert!(matches!(
        run.close().await,
        Err(CapabilityRuntimeError::RunCloseIncomplete {
            child_scopes_failed: 1,
            ..
        })
    ));
    drop(run);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_run_keeps_n_skill_search_registry_and_use_lease_across_cutover() {
    let client = Arc::new(SkillCutoverClient::new());
    let session = Arc::new(
        test_session_with_client(
            "capability-skill-cutover",
            Arc::clone(&client) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_skill_set(1, first_upstream.clone(), &[("pinned-skill", 'b')]);
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["pinned-skill"].clone(),
            CapabilityValue::Skill(skill("pinned-skill", "generation-one")),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .send("Use pinned-skill and search for it.", None)
                .await
        }
    });
    client
        .old_surface_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) =
        use_skill_set(2, second_upstream.clone(), &[("pinned-skill", 'd')]);
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["pinned-skill"].clone(),
            CapabilityValue::Skill(skill("pinned-skill", "generation-two")),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    client.release_old_call.add_permits(1);
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "skill generation one complete");
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new_result = session
        .send("Use pinned-skill and search the pinned catalog.", None)
        .await
        .unwrap();
    assert_eq!(new_result.text, "skill generation two complete");
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*client.observed_search_versions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
}

#[test]
fn capability_run_guards_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<crate::capability::CapabilityCatalog>();
    assert_send_sync::<crate::capability::CapabilityProjectionLease>();
    assert_send_sync::<crate::capability::SessionCapabilityRun>();
    assert_send_sync::<Arc<dyn UseGenerationLeaseProvider>>();
}
