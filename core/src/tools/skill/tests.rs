use super::*;
mod child_policy;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use crate::skills::SkillKind;
use crate::tools::ToolContext;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

struct MockLlmClient {
    responses: Mutex<Vec<LlmResponse>>,
}

impl MockLlmClient {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }

    fn text_response(text: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("end_turn".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }

    fn tool_call_response(id: &str, name: &str, input: serde_json::Value) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input,
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("tool_use".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            anyhow::bail!("No more mock responses available");
        }
        Ok(responses.remove(0))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming not used in SkillTool tests")
    }
}

#[test]
fn skill_tool_does_not_retain_its_executor() {
    let executor = Arc::new(ToolExecutor::new("skill-cycle-test".to_string()));
    let lifetime = Arc::downgrade(&executor);
    let _tool = SkillTool::new_registry_bound(
        Arc::new(SkillRegistry::new()),
        Arc::new(MockLlmClient::new(Vec::new())),
        executor.clone(),
        AgentConfig::default(),
    );

    drop(executor);

    assert!(lifetime.upgrade().is_none());
}

#[derive(Default)]
struct SkillBudgetGuard {
    checks: AtomicUsize,
    records: AtomicUsize,
    sessions: Mutex<Vec<String>>,
}

#[async_trait]
impl BudgetGuard for SkillBudgetGuard {
    async fn check_before_llm(
        &self,
        session_id: &str,
        _estimated_prompt_tokens: usize,
    ) -> BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.sessions.lock().unwrap().push(session_id.to_string());
        BudgetDecision::Allow
    }

    async fn record_after_llm(&self, _session_id: &str, _usage: &TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
    }
}

struct BlockingSkillClient {
    started: Arc<Notify>,
    calls: Arc<AtomicUsize>,
}

struct SkillSideEffectTool {
    calls: Arc<AtomicUsize>,
}

struct SkillScopeProbeTool {
    log: Arc<Mutex<Vec<String>>>,
}

struct SkillScopeProbeEffect {
    scope_id: String,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl crate::capability::CapabilityEffect for SkillScopeProbeEffect {
    fn name(&self) -> &str {
        "test.skill.scope.effect"
    }

    async fn close(
        self: Box<Self>,
    ) -> std::result::Result<(), crate::capability::CapabilityEffectError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("closed:{}", self.scope_id));
        Ok(())
    }
}

#[async_trait]
impl Tool for SkillScopeProbeTool {
    fn name(&self) -> &str {
        "skill_scope_probe"
    }

    fn description(&self) -> &str {
        "Records the capability Turn owned by a nested Skill Agent"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::parallel_safe_read(1)
    }

    async fn execute(&self, _args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let scope_id = ctx
            .capability_scope_id()
            .ok_or_else(|| anyhow!("nested Skill Tool has no capability Turn"))?
            .to_owned();
        self.log
            .lock()
            .unwrap()
            .push(format!("registered:{scope_id}"));
        ctx.register_capability_effect(SkillScopeProbeEffect {
            scope_id,
            log: Arc::clone(&self.log),
        })?;
        Ok(ToolOutput::success("registered"))
    }
}

#[async_trait]
impl Tool for SkillSideEffectTool {
    fn name(&self) -> &str {
        "side_effect"
    }

    fn description(&self) -> &str {
        "Records a test-only side effect"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("unexpected-side-effect"))
    }
}

#[async_trait]
impl LlmClient for BlockingSkillClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        std::future::pending::<Result<LlmResponse>>().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming not used in SkillTool cancellation tests")
    }
}

fn declared_tool_skill_registry() -> Arc<SkillRegistry> {
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "test-skill".to_string(),
        description: "Run a focused skill".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Reply with the skill result.".to_string(),
        tags: vec!["focus".to_string()],
        version: None,
    }));
    registry
}

struct CountingPendingClient {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for CountingPendingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

#[test]
fn test_skill_permission_policy() {
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    };

    let policy = SkillTool::create_skill_permission_policy(&skill);

    // Should allow tools in allowed-tools
    assert_eq!(
        policy.check("read", &serde_json::json!({})),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check(
            "search",
            &serde_json::json!({"mode": "grep", "query": "TODO"}),
        ),
        PermissionDecision::Allow
    );

    // Should deny tools not in allowed-tools
    assert_eq!(
        policy.check("write", &serde_json::json!({})),
        PermissionDecision::Deny
    );
}

#[test]
fn test_skill_permission_policy_denies_when_unspecified() {
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    };

    let policy = SkillTool::create_skill_permission_policy(&skill);

    assert_eq!(
        policy.check("bash", &serde_json::json!({"command": "python --version"})),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check("read", &serde_json::json!({"file_path": "SKILL.md"})),
        PermissionDecision::Deny
    );
}

#[test]
fn test_skill_permission_policy_accepts_legacy_allowed_tools() {
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test".to_string(),
        allowed_tools: Some("Read Write Edit Bash".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    };

    let policy = SkillTool::create_skill_permission_policy(&skill);

    assert_eq!(
        policy.check("bash", &serde_json::json!({"command": "python --version"})),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("search", &serde_json::json!({"mode": "grep", "query": "x"}),),
        PermissionDecision::Deny
    );
}

#[test]
fn test_skill_permission_policy_accepts_wildcard_allowed_tools() {
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test".to_string(),
        allowed_tools: Some("*".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    };

    let policy = SkillTool::create_skill_permission_policy(&skill);

    assert_eq!(
        policy.check("bash", &serde_json::json!({"command": "python --version"})),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("parallel_task", &serde_json::json!({"tasks": []})),
        PermissionDecision::Allow
    );
}

#[test]
fn test_skill_args_accepts_documented_shape() {
    let args =
        SkillArgs::from_tool_args(&serde_json::json!({"skill_name": "code-review"})).unwrap();
    assert_eq!(args.skill_name, "code-review");
    assert_eq!(args.prompt, None);
}

#[test]
fn test_skill_args_accepts_common_aliases_and_wrappers() {
    let camel =
        SkillArgs::from_tool_args(&serde_json::json!({"skillName": "code-review"})).unwrap();
    assert_eq!(camel.skill_name, "code-review");

    let name = SkillArgs::from_tool_args(&serde_json::json!({
        "name": "code-review",
        "query": "review this patch"
    }))
    .unwrap();
    assert_eq!(name.skill_name, "code-review");
    assert_eq!(name.prompt.as_deref(), Some("review this patch"));

    let nested = SkillArgs::from_tool_args(&serde_json::json!({
        "input": {
            "skill_name": "code-review",
            "prompt": "review this patch"
        }
    }))
    .unwrap();
    assert_eq!(nested.skill_name, "code-review");
    assert_eq!(nested.prompt.as_deref(), Some("review this patch"));

    let direct = SkillArgs::from_tool_args(&serde_json::json!("code-review")).unwrap();
    assert_eq!(direct.skill_name, "code-review");
}

#[test]
fn test_skill_args_missing_skill_name_errors() {
    let err =
        SkillArgs::from_tool_args(&serde_json::json!({"prompt": "do something"})).unwrap_err();
    assert!(err.to_string().contains("missing field 'skill_name'"));
}

#[test]
fn test_search_skills_args_accepts_string_and_object() {
    let direct = SearchSkillsArgs::from_tool_args(&serde_json::json!("review code")).unwrap();
    assert_eq!(direct.query, "review code");
    assert_eq!(direct.limit, None);

    let object =
        SearchSkillsArgs::from_tool_args(&serde_json::json!({"query": "review", "limit": 2}))
            .unwrap();
    assert_eq!(object.query, "review");
    assert_eq!(object.limit, Some(2));
}

#[tokio::test]
async fn test_search_skills_tool_returns_matching_skills() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "code-review".to_string(),
        description: "Review code changes".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Review instructions".to_string(),
        tags: vec!["review".to_string()],
        version: None,
    }));

    let tool = SearchSkillsTool::new(registry);
    let result = tool
        .execute(
            &serde_json::json!({"query": "review"}),
            &ToolContext::new(hermetic_root.clone()),
        )
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.content.contains("code-review"));
    assert!(result.content.contains("Found 1 matching skill(s)."));
    assert_eq!(result.metadata.unwrap()["skills"][0]["name"], "code-review");
}

#[tokio::test]
async fn test_search_skills_lead_follows_the_user_language() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "code-review".to_string(),
        description: "Review code changes".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Review instructions".to_string(),
        tags: vec!["review".to_string()],
        version: None,
    }));
    let tool = SearchSkillsTool::new(registry);
    let ctx = ToolContext::new(hermetic_root);
    ctx.set_reply_language(Some("zh-CN"));
    let result = tool
        .execute(&serde_json::json!({"query": "review"}), &ctx)
        .await
        .unwrap();
    assert!(result.content.starts_with("找到 1 个匹配的技能"));
    let empty = SearchSkillsTool::new(Arc::new(SkillRegistry::new()));
    let missed = empty
        .execute(&serde_json::json!({"query": "missing"}), &ctx)
        .await
        .unwrap();
    assert_eq!(missed.content, "没有匹配的技能。请继续使用现有工具。");
}

#[tokio::test]
async fn test_search_skills_tool_clamps_limit_and_excludes_personas() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let registry = Arc::new(SkillRegistry::new());
    for index in 0..25 {
        registry.register_unchecked(Arc::new(Skill {
            name: format!("review-{index:02}"),
            description: "Review code changes".to_string(),
            allowed_tools: Some("read(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Review instructions".to_string(),
            tags: vec!["review".to_string()],
            version: None,
        }));
    }
    registry.register_unchecked(Arc::new(Skill {
        name: "review-persona".to_string(),
        description: "Review persona".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Persona,
        content: "Persona instructions".to_string(),
        tags: vec!["review".to_string()],
        version: None,
    }));

    let tool = SearchSkillsTool::new(registry);
    let result = tool
        .execute(
            &serde_json::json!({"query": "review", "limit": 100}),
            &ToolContext::new(hermetic_root.clone()),
        )
        .await
        .unwrap();

    let metadata = result.metadata.unwrap();
    let skills = metadata["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 20);
    assert!(skills.iter().all(|skill| skill["kind"] == "instruction"));
}

#[test]
fn test_skill_tool_schema_enforces_canonical_shape() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let registry = Arc::new(SkillRegistry::new());
    let llm = Arc::new(MockLlmClient::new(vec![]));
    let executor = Arc::new(ToolExecutor::new(hermetic_root.display().to_string()));
    let tool = SkillTool::new(registry, llm, executor, AgentConfig::default());

    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert_eq!(params["additionalProperties"], serde_json::json!(false));
    assert_eq!(params["required"], serde_json::json!(["skill_name"]));

    let examples = params["examples"].as_array().unwrap();
    assert_eq!(examples[0]["skill_name"], "code-review");
    assert!(examples[0].get("name").is_none());
    assert!(examples[0].get("skillName").is_none());
}

#[tokio::test]
async fn test_skill_tool_execute_runs_skill_and_returns_metadata() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    use crate::prompts::PlanningMode;

    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "test-skill".to_string(),
        description: "Run a focused skill".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Reply with the skill result.".to_string(),
        tags: vec!["focus".to_string()],
        version: None,
    }));

    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(ToolExecutor::new(hermetic_root.display().to_string()));
    // An active goal forces planning on the parent. The instruction skill
    // must still return without opening that second model call.
    let config = AgentConfig {
        planning_mode: PlanningMode::Enabled,
        continuation_enabled: false,
        ..Default::default()
    };
    let tool = SkillTool::new(
        registry,
        Arc::new(CountingPendingClient {
            calls: Arc::clone(&calls),
        }),
        executor,
        config,
    );

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        tool.execute(
            &serde_json::json!({
                "skill_name": "test-skill",
                "prompt": "verify the skill result"
            }),
            &ToolContext::new(hermetic_root.clone()),
        ),
    )
    .await
    .expect("an instruction skill must return without a nested model call")
    .unwrap();

    assert!(result.success);
    assert!(result.content.contains("Reply with the skill result."));
    assert!(result.content.contains("verify the skill result"));
    assert!(result.content.contains("Do not invoke `test-skill` again"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["skill_name"], "test-skill");
    assert_eq!(metadata["tool_calls"], 0);
    assert_eq!(metadata["delivery"], "instructions");
    assert!(metadata.get("changed_paths").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool("Skill", 0, Some(&metadata));
    assert!(ledger.is_empty(), "instruction handoff is not a mutation");
    assert!(!ledger.observation_incomplete());
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[], &[], false),
        crate::harness_loop::CompletionGate::Allow(
            crate::harness_loop::CompletionTerminal::Narrative
        )
    ));
}

#[tokio::test]
async fn review_skill_invocation_returns_instructions_instead_of_not_found() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    use crate::prompts::PlanningMode;

    let registry = Arc::new(SkillRegistry::new());
    registry.register_host(Arc::new(Skill {
        name: "review".to_string(),
        description: "Single-document review".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Emit fences for the injected batch only.".to_string(),
        tags: vec![],
        version: None,
    }));

    let executor = Arc::new(ToolExecutor::new(hermetic_root.display().to_string()));
    let config = AgentConfig {
        planning_mode: PlanningMode::Enabled,
        continuation_enabled: false,
        ..Default::default()
    };
    let tool = SkillTool::new(
        registry,
        Arc::new(CountingPendingClient {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        executor,
        config,
    );
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        tool.execute(
            &serde_json::json!({
                "skill_name": "review",
                "prompt": "审查本批注入正文"
            }),
            &ToolContext::new(hermetic_root.clone()),
        ),
    )
    .await
    .expect("review instructions must return to the parent turn")
    .unwrap();

    assert!(result.success);
    assert!(result
        .content
        .contains("Emit fences for the injected batch only."));
    assert!(result.content.contains("审查本批注入正文"));
    assert!(!result.content.contains("not found"));
    assert_eq!(
        result.metadata.unwrap()["delivery"],
        serde_json::json!("instructions")
    );
}

#[tokio::test]
async fn skill_workspace_write_is_a_parent_mutation() {
    use crate::prompts::PlanningMode;

    let workspace = tempfile::tempdir().unwrap();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "writer-skill".to_string(),
        description: "Write a file then narrate".to_string(),
        allowed_tools: Some("skill_workspace_writer".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Write the file, then finish.".to_string(),
        tags: Vec::new(),
        version: None,
    }));
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().into_owned(),
    ));
    executor.register_dynamic_tool(Arc::new(SkillWorkspaceWriter));
    let tool = SkillTool::new(
        registry,
        Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "write-call",
                "skill_workspace_writer",
                serde_json::json!({}),
            ),
            MockLlmClient::text_response("done"),
        ])),
        executor,
        AgentConfig {
            planning_mode: PlanningMode::Disabled,
            continuation_enabled: false,
            ..Default::default()
        },
    );
    let result = tool
        .execute(
            &serde_json::json!({"skill_name": "writer-skill"}),
            &ToolContext::new(workspace.path().to_path_buf()).with_session_id("skill-write"),
        )
        .await
        .unwrap();

    assert!(
        workspace.path().join("guest.txt").is_file(),
        "skill child did not write"
    );
    let metadata = result.metadata.expect("skill metadata");
    let paths = metadata["changed_paths"].as_array().expect("changed_paths");
    assert!(
        paths.iter().any(|path| path == "guest.txt"),
        "skill hid a workspace write: {metadata}"
    );
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool("skill", 0, Some(&metadata));
    assert!(
        !ledger.is_empty(),
        "parent completion gate did not see the skill write"
    );
}

struct SkillWorkspaceWriter;

#[async_trait]
impl Tool for SkillWorkspaceWriter {
    fn name(&self) -> &str {
        "skill_workspace_writer"
    }

    fn description(&self) -> &str {
        "Writes a workspace file without publishing a mutation path"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::parallel_safe_read(1)
    }

    async fn execute(&self, _args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        std::fs::write(ctx.workspace.join("guest.txt"), "hello\n")?;
        Ok(ToolOutput::success("wrote"))
    }
}

#[tokio::test]
async fn skill_child_agent_owns_recursive_subtask_and_turn_scopes() {
    use crate::prompts::PlanningMode;

    let workspace = tempfile::tempdir().unwrap();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "scoped-skill".to_string(),
        description: "Exercise nested scope ownership".to_string(),
        allowed_tools: Some("skill_scope_probe".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Call skill_scope_probe once, then finish.".to_string(),
        tags: Vec::new(),
        version: None,
    }));
    let log = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().into_owned(),
    ));
    executor.register_dynamic_tool(Arc::new(SkillScopeProbeTool {
        log: Arc::clone(&log),
    }));
    let tool = SkillTool::new(
        registry,
        Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "skill-probe",
                "skill_scope_probe",
                serde_json::json!({}),
            ),
            MockLlmClient::text_response("skill complete"),
        ])),
        executor,
        AgentConfig {
            planning_mode: PlanningMode::Disabled,
            continuation_enabled: false,
            ..Default::default()
        },
    );
    let set = crate::capability::CapabilitySet::empty().unwrap();
    let ceiling = crate::capability::CapabilityCeiling::all(
        &set,
        crate::capability::WorkspaceCapabilityCeiling::all(),
        crate::capability::GovernanceCapabilityCeiling::none_required(),
        crate::capability::CapabilityExecutionCeiling::new(
            4,
            2,
            Some(1_000),
            Some(1_000),
            Some(5_000),
        )
        .unwrap(),
    )
    .unwrap();
    let session = crate::capability::CapabilityScope::<crate::capability::Session>::new_session(
        "session-1",
        set,
        ceiling.clone(),
    )
    .unwrap();
    let run = session.admit_run("run-1", ceiling).unwrap();
    let runtime = crate::capability::AgentCapabilityRuntime::from_run(&run);
    let parent_turn = runtime.begin_turn(1).unwrap();
    let capability_context = parent_turn.tool_context();
    let parent_scope_id = capability_context.foreground_scope_id().to_owned();
    let context = ToolContext::new(workspace.path().to_path_buf())
        .with_cancellation(parent_turn.cancellation())
        .with_capability_context(capability_context);

    let result = tool
        .execute(&serde_json::json!({"skill_name": "scoped-skill"}), &context)
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    let entries = log.lock().unwrap().clone();
    assert_eq!(entries.len(), 2, "{entries:?}");
    let registered = entries[0].strip_prefix("registered:").unwrap();
    let closed = entries[1].strip_prefix("closed:").unwrap();
    assert_eq!(registered, closed);
    assert!(
        registered.starts_with(&format!("{parent_scope_id}/subtask/skill-")),
        "{registered}"
    );
    assert!(registered.contains("/turn/turn-1-1"), "{registered}");

    parent_turn.close().await.unwrap();
    run.close().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn skill_permissions_cannot_replace_the_parent_host_boundary() {
    use crate::prompts::PlanningMode;

    let workspace = tempfile::tempdir().unwrap();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "bash-skill".to_string(),
        description: "Attempt a shell call".to_string(),
        allowed_tools: Some("bash(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Use Bash once.".to_string(),
        tags: Vec::new(),
        version: None,
    }));
    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "bash-call",
            "bash",
            serde_json::json!({"command": "printf leaked > skill-boundary-leak"}),
        ),
        MockLlmClient::text_response("The host boundary rejected the call."),
    ]));
    let parent_policy = PermissionPolicy::new().deny("bash(*)");
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        continuation_enabled: false,
        permission_checker: Some(Arc::new(parent_policy.clone())),
        permission_policy: Some(parent_policy),
        ..Default::default()
    };
    let tool = SkillTool::new(
        registry,
        llm,
        Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        )),
        config,
    );

    let result = tool
        .execute(
            &serde_json::json!({"skill_name": "bash-skill"}),
            &ToolContext::new(workspace.path().to_path_buf()),
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(
        !workspace.path().join("skill-boundary-leak").exists(),
        "a skill-local allow-list must not bypass the parent host boundary"
    );
}

#[tokio::test]
async fn host_direct_context_does_not_leak_into_skill_model_orchestrators() {
    use crate::prompts::PlanningMode;

    struct RecordingBoundary {
        checked: Arc<Mutex<Vec<String>>>,
    }

    impl crate::permissions::PermissionChecker for RecordingBoundary {
        fn check(&self, tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            self.checked.lock().unwrap().push(tool_name.to_string());
            if tool_name == "side_effect" {
                PermissionDecision::Deny
            } else {
                PermissionDecision::Allow
            }
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let checked = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "orchestrator-skill".to_string(),
        description: "Exercise governed orchestrators".to_string(),
        allowed_tools: Some("batch(*), program(*), side_effect(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Run the requested orchestration.".to_string(),
        tags: Vec::new(),
        version: None,
    }));
    let llm = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "model-batch",
            "batch",
            serde_json::json!({
                "invocations": [{
                    "tool": "program",
                    "args": {
                        "type": "script",
                        "language": "javascript",
                        "source": "async function run(ctx) { return await ctx.tool('side_effect', {}); }",
                        "allowed_tools": ["side_effect"]
                    }
                }]
            }),
        ),
        MockLlmClient::text_response("The boundary held."),
    ]));
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().into_owned(),
    ));
    executor.register_dynamic_tool(Arc::new(SkillSideEffectTool {
        calls: Arc::clone(&calls),
    }));
    let tool = SkillTool::new(
        registry,
        llm,
        executor,
        AgentConfig {
            planning_mode: PlanningMode::Disabled,
            continuation_enabled: false,
            permission_checker: Some(Arc::new(RecordingBoundary {
                checked: Arc::clone(&checked),
            })),
            ..Default::default()
        },
    );
    let context = ToolContext::new(workspace.path().to_path_buf())
        .with_host_direct_policy(crate::tools::HostDirectPolicy::TrustedControlPlane);

    let result = tool
        .execute(
            &serde_json::json!({"skill_name": "orchestrator-skill"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert_eq!(result.content, "The boundary held.");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let checked = checked.lock().unwrap();
    for expected in ["batch", "program", "side_effect"] {
        assert!(
            checked.iter().any(|tool| tool == expected),
            "{expected} must cross the parent permission boundary: {checked:?}"
        );
    }
}

#[tokio::test]
async fn skill_child_llm_call_uses_parent_session_budget_scope() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    use crate::prompts::PlanningMode;

    let guard = Arc::new(SkillBudgetGuard::default());
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        continuation_enabled: false,
        budget_guard: Some(Arc::clone(&guard) as Arc<dyn BudgetGuard>),
        ..Default::default()
    };
    let tool = SkillTool::new(
        declared_tool_skill_registry(),
        Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "skill completed",
        )])),
        Arc::new(ToolExecutor::new(hermetic_root.display().to_string())),
        config,
    );
    let ctx = ToolContext::new(hermetic_root.clone()).with_session_id("parent-session");

    let result = tool
        .execute(&serde_json::json!({"skill_name": "test-skill"}), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(guard.checks.load(Ordering::SeqCst), 1);
    assert_eq!(guard.records.load(Ordering::SeqCst), 1);
    assert_eq!(
        guard.sessions.lock().unwrap().as_slice(),
        &["parent-session".to_string()]
    );
}

#[tokio::test]
async fn skill_child_llm_call_stops_on_parent_cancellation() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    use crate::prompts::PlanningMode;

    let started = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let tool = SkillTool::new(
        declared_tool_skill_registry(),
        Arc::new(BlockingSkillClient {
            started: Arc::clone(&started),
            calls: Arc::clone(&calls),
        }),
        Arc::new(ToolExecutor::new(hermetic_root.display().to_string())),
        AgentConfig {
            planning_mode: PlanningMode::Disabled,
            continuation_enabled: false,
            ..Default::default()
        },
    );
    let cancellation = CancellationToken::new();
    let ctx = ToolContext::new(hermetic_root.clone())
        .with_session_id("parent-session")
        .with_cancellation(cancellation.clone());
    let started_wait = started.notified();
    let run = tokio::spawn(async move {
        tool.execute(&serde_json::json!({"skill_name": "test-skill"}), &ctx)
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), started_wait)
        .await
        .expect("skill provider call should start");
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("parent cancellation must stop the skill child")
        .expect("skill join should succeed")
        .expect_err("cancelled skill must not return success");

    assert!(error.to_string().contains("cancelled"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_skill_tool_execute_errors_for_unknown_skill() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let llm = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "unused",
    )]));
    let executor = Arc::new(ToolExecutor::new(hermetic_root.display().to_string()));
    let tool = SkillTool::new(
        Arc::new(SkillRegistry::new()),
        llm,
        executor,
        AgentConfig::default(),
    );

    let err = tool
        .execute(
            &serde_json::json!({"skill_name": "missing-skill"}),
            &ToolContext::new(hermetic_root.clone()),
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("Skill 'missing-skill' not found"));
}

#[tokio::test]
async fn skill_tool_sees_a_package_added_after_the_registry_was_built() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let root = tempfile::TempDir::new().expect("temp dir");
    let skills = root.path().join("skills");
    std::fs::create_dir_all(&skills).expect("skills dir");
    let registry = Arc::new(SkillRegistry::new());
    registry.watch_dir(&skills);

    let package = skills.join("later");
    std::fs::create_dir_all(&package).expect("package dir");
    std::fs::write(
        package.join("SKILL.md"),
        "---\nname: later-skill\ndescription: Arrived after the session\nkind: instruction\n---\nLATER_TOKEN\n",
    )
    .expect("skill file");

    let tool = SkillTool::new(
        registry,
        Arc::new(MockLlmClient::new(Vec::new())),
        Arc::new(ToolExecutor::new(hermetic_root.display().to_string())),
        AgentConfig::default(),
    );
    let result = tool
        .execute(
            &serde_json::json!({
                "skill_name": "later-skill",
                "prompt": "use it"
            }),
            &ToolContext::new(hermetic_root),
        )
        .await
        .expect("watched package is visible to Skill");

    assert!(result.success);
    assert!(result.content.contains("later-skill"));
    assert!(result.content.contains("LATER_TOKEN"));
}

#[tokio::test]
async fn skill_tool_rejects_disable_model_invocation_skills() {
    let hermetic_root = crate::test_support::hermetic_workspace();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "host-only".to_string(),
        description: "Not for the model".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: true,
        kind: SkillKind::Instruction,
        content: "Secret host skill.".to_string(),
        tags: Vec::new(),
        version: None,
    }));

    let llm = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "unused",
    )]));
    let executor = Arc::new(ToolExecutor::new(hermetic_root.display().to_string()));
    let tool = SkillTool::new(registry, llm, executor, AgentConfig::default());

    let err = tool
        .execute(
            &serde_json::json!({"skill_name": "host-only"}),
            &ToolContext::new(hermetic_root.clone()),
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("disables model invocation"));
}
