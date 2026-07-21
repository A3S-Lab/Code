use super::*;
use crate::context::{ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType};
use crate::llm::{ContentBlock, StreamEvent};
use crate::permissions::PermissionPolicy;
use crate::prompts::AgentStyle;
use crate::tools::ToolExecutor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

/// Create a default ToolContext for tests
fn test_tool_context() -> ToolContext {
    ToolContext::new(PathBuf::from("/tmp"))
}

#[test]
fn test_plan_step_delegation_detection() {
    use crate::planning::Task;

    assert!(AgentLoop::should_delegate_plan_step(
        &Task::new("s1", "Find relevant files").with_tool("task")
    ));
    assert!(AgentLoop::should_delegate_plan_step(
        &Task::new("s2", "Check independent areas").with_tool("parallel_task")
    ));
    assert!(!AgentLoop::should_delegate_plan_step(&Task::new(
        "s3",
        "Implement directly"
    )));
}

#[test]
fn test_delegated_agent_selection_from_step_text() {
    use crate::planning::Task;

    assert_eq!(
        AgentLoop::delegated_agent_for_step(&Task::new("s1", "查找相关实现")),
        "explore"
    );
    assert_eq!(
        AgentLoop::delegated_agent_for_step(&Task::new("s2", "Run release verification tests")),
        "verification"
    );
    assert_eq!(
        AgentLoop::delegated_agent_for_step(&Task::new("s3", "Review risky code changes")),
        "review"
    );
    assert_eq!(
        AgentLoop::delegated_agent_for_step(&Task::new("s4", "Design the architecture")),
        "plan"
    );
    assert_eq!(
        AgentLoop::delegated_agent_for_step(&Task::new("s5", "Implement the change")),
        "general"
    );
}

#[test]
fn test_delegated_task_args_include_prompt_contract() {
    use crate::planning::Task;

    let task = Task::new("s1", "验证 program 工具")
        .with_tool("task")
        .with_success_criteria("All integration checks pass.");
    let args = AgentLoop::delegated_task_args_with_goal(None, &task, 2, 5);

    assert_eq!(args["agent"], "verification");
    assert_eq!(args["description"], "验证 program 工具");
    assert!(args["prompt"].as_str().unwrap().contains("2/5"));
    assert!(args["prompt"]
        .as_str()
        .unwrap()
        .contains("All integration checks pass."));
}

#[test]
fn test_parallel_delegated_task_args_preserve_order() {
    use crate::planning::Task;

    let steps = vec![
        (Task::new("s1", "Find docs").with_tool("task"), 1),
        (Task::new("s2", "Run tests").with_tool("task"), 2),
    ];
    let args = AgentLoop::parallel_delegated_task_args_with_goal(None, &steps, 2);
    let tasks = args["tasks"].as_array().unwrap();

    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0]["agent"], "explore");
    assert_eq!(tasks[1]["agent"], "verification");
    assert_eq!(args["allow_partial_failure"], true);
}

#[test]
fn test_preserve_original_prompt_for_planning_execution() {
    let original =
        "Fix planning mode. Preserve /tmp/task.txt and do not drop negative instructions.";
    let optimized = "Fix planning mode.";

    let preserved = AgentLoop::preserve_original_prompt_for_execution(original, optimized);

    assert!(preserved.contains("Original user request"));
    assert!(preserved.contains("/tmp/task.txt"));
    assert!(preserved.contains("do not drop negative instructions"));
    assert!(preserved.contains("Planner-optimized request"));
    assert!(preserved.contains(optimized));
}

#[test]
fn test_preserve_plan_goal_context_keeps_original_request_visible() {
    use crate::planning::{Complexity, ExecutionPlan};

    let plan = ExecutionPlan::new("Fix planning mode".to_string(), Complexity::Medium);
    let execution_prompt =
        "Original user request:\nFix planning mode for /workspace/app; do not change API.";

    let preserved = AgentLoop::preserve_plan_goal_context(plan, execution_prompt);

    assert!(preserved.goal.contains("/workspace/app"));
    assert!(preserved.goal.contains("do not change API"));
    assert!(preserved.goal.contains("Planner goal"));
}

#[test]
fn test_delegated_plan_step_prompt_includes_plan_goal_context() {
    use crate::planning::Task;

    let task = Task::new("s1", "Implement the first step").with_tool("task");
    let args = AgentLoop::delegated_task_args_with_goal(
        Some("Original request: update /workspace/app and keep API stable."),
        &task,
        1,
        1,
    );
    let prompt = args["prompt"].as_str().unwrap();

    assert!(prompt.contains("Plan goal/context"));
    assert!(prompt.contains("/workspace/app"));
    assert!(prompt.contains("keep API stable"));
    assert!(prompt.contains("Implement the first step"));
}

#[test]
fn test_memory_items_become_context_result() {
    let item = a3s_memory::MemoryItem::new("Use focused regression tests for context changes.")
        .with_importance(0.8);

    let result = crate::memory::memory_items_to_context_result("memory", vec![item.clone()]);

    assert_eq!(result.provider, "memory");
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, item.id.as_str());
    assert_eq!(result.items[0].context_type, ContextType::Memory);
    let expected_source = format!("memory://{}", item.id);
    assert_eq!(
        result.items[0].source.as_deref(),
        Some(expected_source.as_str())
    );
    assert!(result.items[0].content.contains("focused regression tests"));
    assert!(result.items[0].token_count > 0);
}

#[test]
fn test_agent_config_default() {
    let config = AgentConfig::default();
    assert!(config.prompt_slots.is_empty());
    assert!(config.tools.is_empty()); // Tools are provided externally
    assert_eq!(config.max_tool_rounds, MAX_TOOL_ROUNDS);
    assert_eq!(config.max_parallel_tasks, DEFAULT_MAX_PARALLEL_TASKS);
    assert!(config.permission_checker.is_none());
    assert!(config.context_providers.is_empty());
    // A skill registry is present by default, but A3S Code no longer ships
    // embedded built-in skills.
    let registry = config
        .skill_registry
        .expect("skill_registry must be Some by default");
    assert_eq!(registry.len(), 0);
}

// ========================================================================
// Mock LLM Client for Testing
// ========================================================================

/// Mock LLM client that returns predefined responses
pub(crate) struct MockLlmClient {
    /// Responses to return (consumed in order)
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    /// User prompt texts sent to the client, in call order.
    pub(crate) request_texts: std::sync::Mutex<Vec<String>>,
    /// Tool definition names sent to the client, in call order.
    pub(crate) request_tools: std::sync::Mutex<Vec<Vec<String>>>,
    /// Number of calls made
    pub(crate) call_count: AtomicUsize,
}

struct BlockingExtractionLlmClient {
    extraction_started: Arc<tokio::sync::Notify>,
    extraction_release: Arc<tokio::sync::Notify>,
    extraction_finished: Arc<tokio::sync::Notify>,
    call_count: AtomicUsize,
}

struct HangingCompactionLlmClient {
    response: LlmResponse,
    complete_calls: AtomicUsize,
}

fn pre_analysis_user_request(prompt_text: &str) -> String {
    let request = prompt_text
        .split_once("User request:\n")
        .map(|(_, request)| request)
        .unwrap_or(prompt_text);
    request
        .split_once("You MUST respond with ONLY")
        .map(|(request, _)| request)
        .unwrap_or(request)
        .trim()
        .to_string()
}

impl BlockingExtractionLlmClient {
    fn new() -> Self {
        Self {
            extraction_started: Arc::new(tokio::sync::Notify::new()),
            extraction_release: Arc::new(tokio::sync::Notify::new()),
            extraction_finished: Arc::new(tokio::sync::Notify::new()),
            call_count: AtomicUsize::new(0),
        }
    }

    fn text_response(text: &str) -> LlmResponse {
        MockLlmClient::text_response(text)
    }
}

impl HangingCompactionLlmClient {
    fn new() -> Self {
        let mut response = MockLlmClient::text_response("Final answer complete.");
        response.usage = TokenUsage {
            prompt_tokens: 900,
            completion_tokens: 20,
            total_tokens: 920,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        Self {
            response,
            complete_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for BlockingExtractionLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let prompt_text = messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM)) {
            let prompt = pre_analysis_user_request(&prompt_text);
            let response = serde_json::json!({
                "intent": "GeneralPurpose",
                "requires_planning": false,
                "goal": { "description": prompt, "success_criteria": [] },
                "execution_plan": {
                    "complexity": "Simple",
                    "steps": [],
                    "required_tools": []
                },
                "optimized_input": prompt
            });
            return Ok(Self::text_response(&response.to_string()));
        }

        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.extraction_started.notify_one();
        self.extraction_release.notified().await;
        self.extraction_finished.notify_one();
        Ok(Self::text_response(r#"{"items":[]}"#))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let response = Self::text_response("Stored durable memory workflow.");
        let (tx, rx) = mpsc::channel(10);
        tokio::spawn(async move {
            tx.send(StreamEvent::TextDelta(
                "Stored durable memory workflow.".to_string(),
            ))
            .await
            .ok();
            tx.send(StreamEvent::Done(response)).await.ok();
        });
        Ok(rx)
    }
}

#[async_trait::async_trait]
impl LlmClient for HangingCompactionLlmClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<Result<LlmResponse>>().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let response = self.response.clone();
        let (tx, rx) = mpsc::channel(10);
        tokio::spawn(async move {
            tx.send(StreamEvent::TextDelta(response.text())).await.ok();
            tx.send(StreamEvent::Done(response)).await.ok();
        });
        Ok(rx)
    }
}

impl MockLlmClient {
    pub(crate) fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            request_texts: std::sync::Mutex::new(Vec::new()),
            request_tools: std::sync::Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
        }
    }

    /// Create a response with text only (no tool calls)
    pub(crate) fn text_response(text: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                reasoning_content: None,
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

    pub(crate) fn reasoning_only_response(reasoning: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: Vec::new(),
                reasoning_content: Some(reasoning.to_string()),
            },
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("stop".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }

    /// Create a response with a tool call
    pub(crate) fn tool_call_response(
        tool_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: tool_id.to_string(),
                    name: tool_name.to_string(),
                    input: args,
                }],
                reasoning_content: None,
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

#[async_trait::async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let prompt_text = messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM)) {
            let prompt = pre_analysis_user_request(&prompt_text);
            let response = serde_json::json!({
                "intent": "GeneralPurpose",
                "requires_planning": false,
                "goal": {
                    "description": prompt,
                    "success_criteria": []
                },
                "execution_plan": {
                    "complexity": "Simple",
                    "steps": [
                        {
                            "id": "step-1",
                            "description": prompt,
                            "dependencies": [],
                            "success_criteria": "Complete the request"
                        }
                    ],
                    "required_tools": []
                },
                "optimized_input": prompt
            });
            return Ok(MockLlmClient::text_response(&response.to_string()));
        }
        self.request_texts.lock().unwrap().push(prompt_text);
        self.request_tools.lock().unwrap().push(
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
        );
        self.call_count.fetch_add(1, Ordering::SeqCst);
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
        tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.request_texts
            .lock()
            .unwrap()
            .push("<streaming>".to_string());
        self.request_tools.lock().unwrap().push(
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
        );
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            anyhow::bail!("No more mock responses available");
        }
        let response = responses.remove(0);

        let (tx, rx) = mpsc::channel(10);
        tokio::spawn(async move {
            // Send text deltas if any
            for block in &response.message.content {
                if let ContentBlock::Text { text } = block {
                    tx.send(StreamEvent::TextDelta(text.clone())).await.ok();
                }
            }
            tx.send(StreamEvent::Done(response)).await.ok();
        });

        Ok(rx)
    }
}

struct CountingBudgetGuard {
    check_count: AtomicUsize,
    record_count: AtomicUsize,
    recorded_tokens: AtomicUsize,
    deny_on_check: usize,
}

impl CountingBudgetGuard {
    fn new(deny_on_check: usize) -> Self {
        Self {
            check_count: AtomicUsize::new(0),
            record_count: AtomicUsize::new(0),
            recorded_tokens: AtomicUsize::new(0),
            deny_on_check,
        }
    }
}

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for CountingBudgetGuard {
    async fn check_before_llm(
        &self,
        _session_id: &str,
        _estimated_prompt_tokens: usize,
    ) -> crate::budget::BudgetDecision {
        let count = self.check_count.fetch_add(1, Ordering::SeqCst) + 1;
        if self.deny_on_check != 0 && count == self.deny_on_check {
            crate::budget::BudgetDecision::Deny {
                resource: "llm_tokens".to_string(),
                reason: "denied by test budget".to_string(),
            }
        } else {
            crate::budget::BudgetDecision::Allow
        }
    }

    async fn record_after_llm(&self, _session_id: &str, usage: &TokenUsage) {
        self.record_count.fetch_add(1, Ordering::SeqCst);
        self.recorded_tokens
            .fetch_add(usage.total_tokens, Ordering::SeqCst);
    }
}

// ========================================================================
// Agent Loop Tests
// ========================================================================

#[tokio::test]
async fn test_agent_simple_response() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Hello, I'm an AI assistant.",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Hello", None).await.unwrap();

    assert_eq!(result.text, "Hello, I'm an AI assistant.");
    assert_eq!(result.tool_calls_count, 0);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
}

fn model_tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} test tool"),
        parameters: serde_json::json!({"type": "object"}),
    }
}

#[tokio::test]
async fn permission_checker_hides_tools_from_llm_request() {
    use crate::permissions::{PermissionChecker, PermissionDecision};

    struct ReadOnlyModelExposure;

    impl PermissionChecker for ReadOnlyModelExposure {
        fn expose_to_model(&self, tool_name: &str) -> bool {
            tool_name == "read"
        }

        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Allow
        }
    }

    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Tool exposure test complete.",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        tools: vec![
            model_tool_definition("read"),
            model_tool_definition("bash"),
            model_tool_definition("web_search"),
        ],
        permission_checker: Some(Arc::new(ReadOnlyModelExposure)),
        planning_mode: PlanningMode::Disabled,
        prompt_slots: SystemPromptSlots {
            style: Some(AgentStyle::GeneralPurpose),
            ..Default::default()
        },
        continuation_enabled: false,
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    agent
        .execute(&[], "Search the web and inspect files with bash.", None)
        .await
        .unwrap();

    assert_eq!(
        *mock_client.request_tools.lock().unwrap(),
        vec![vec!["read".to_string()]]
    );
}

#[tokio::test]
async fn permission_checker_default_exposes_selected_tools_to_llm() {
    use crate::permissions::{PermissionChecker, PermissionDecision};

    struct ExecutionOnlyChecker;

    impl PermissionChecker for ExecutionOnlyChecker {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny
        }
    }

    let checker = ExecutionOnlyChecker;
    assert!(checker.expose_to_model("bash"));

    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Default exposure test complete.",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        tools: vec![
            model_tool_definition("read"),
            model_tool_definition("bash"),
            model_tool_definition("web_search"),
        ],
        permission_checker: Some(Arc::new(checker)),
        planning_mode: PlanningMode::Disabled,
        prompt_slots: SystemPromptSlots {
            style: Some(AgentStyle::GeneralPurpose),
            ..Default::default()
        },
        continuation_enabled: false,
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    agent
        .execute(&[], "Search the web and inspect files with bash.", None)
        .await
        .unwrap();

    assert_eq!(
        *mock_client.request_tools.lock().unwrap(),
        vec![vec![
            "read".to_string(),
            "bash".to_string(),
            "web_search".to_string(),
        ]]
    );
}

#[tokio::test]
async fn test_agent_repairs_reasoning_only_response_once() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::reasoning_only_response("I have the answer but put it in reasoning."),
        MockLlmClient::text_response("The answer is 42."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        max_continuation_turns: 0,
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Answer plainly", None).await.unwrap();

    assert_eq!(result.text, "The answer is 42.");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_agent_stops_after_repeated_reasoning_only_response() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::reasoning_only_response("Thinking only, first pass."),
        MockLlmClient::reasoning_only_response("Thinking only, second pass."),
        MockLlmClient::text_response("This response should not be consumed."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Answer plainly", None).await.unwrap();

    assert_eq!(
        result.text,
        "The model completed but returned only reasoning content and did not provide a final answer."
    );
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_agent_with_tool_call() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        // First response: tool call
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo hello"}),
        ),
        // Second response: final text
        MockLlmClient::text_response("The command output was: hello"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Run echo hello", None).await.unwrap();

    assert_eq!(result.text, "The command output was: hello");
    assert_eq!(result.tool_calls_count, 1);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_agent_permission_deny() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        // First response: tool call that will be denied
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "rm -rf /tmp/test"}),
        ),
        // Second response: LLM responds to the denial
        MockLlmClient::text_response(
            "I cannot execute that command due to permission restrictions.",
        ),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create permission policy that denies rm commands
    let permission_policy = PermissionPolicy::new().deny("bash(rm:*)");

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        ..Default::default()
    };

    let (tx, mut rx) = mpsc::channel(100);
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Delete files", Some(tx)).await.unwrap();

    // Check that we received a PermissionDenied event
    let mut found_permission_denied = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::PermissionDenied { tool_name, .. } = event {
            assert_eq!(tool_name, "bash");
            found_permission_denied = true;
        }
    }
    assert!(
        found_permission_denied,
        "Should have received PermissionDenied event"
    );

    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_permission_allow() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        // First response: tool call that will be allowed
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo hello"}),
        ),
        // Second response: final text
        MockLlmClient::text_response("Done!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create permission policy that allows echo commands
    let permission_policy = PermissionPolicy::new()
        .allow("bash(echo:*)")
        .deny("bash(rm:*)");

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Echo hello", None).await.unwrap();

    assert_eq!(result.text, "Done!");
    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_streaming_events() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Hello!",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let (tx, mut rx) = mpsc::channel(100);
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let result = agent
        .execute_with_session(&[], "Hi", None, Some(tx), Some(&cancel_token))
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    assert_eq!(result.text, "Hello!");

    // Check we received Start and End events
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Start { .. })));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::End { .. })));
}

#[tokio::test]
async fn test_agent_max_tool_rounds() {
    // Create a mock that always returns tool calls (infinite loop)
    let responses: Vec<LlmResponse> = (0..100)
        .map(|i| {
            MockLlmClient::tool_call_response(
                &format!("tool-{}", i),
                "bash",
                serde_json::json!({"command": "echo loop"}),
            )
        })
        .collect();

    let mock_client = Arc::new(MockLlmClient::new(responses));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let config = AgentConfig {
        max_tool_rounds: 3,
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Loop forever", None).await;

    // Should fail due to max tool rounds exceeded
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Max tool rounds"));
}

#[tokio::test]
async fn test_agent_max_tool_rounds_reserves_tool_free_finalization() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo first"}),
        ),
        MockLlmClient::tool_call_response(
            "tool-2",
            "bash",
            serde_json::json!({"command": "echo second"}),
        ),
        MockLlmClient::text_response("Best bounded answer from collected evidence."),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        max_tool_rounds: 2,
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent
        .execute(&[], "Collect bounded evidence", None)
        .await
        .unwrap();

    assert_eq!(result.text, "Best bounded answer from collected evidence.");
    assert_eq!(result.tool_calls_count, 2);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_agent_no_permission_policy_defaults_to_ask() {
    // When no permission policy is set, tools default to Ask.
    // Without a confirmation manager, Ask = safe deny.
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "rm -rf /tmp/test"}),
        ),
        MockLlmClient::text_response("Denied!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        permission_checker: None, // No policy → defaults to Ask
        // No confirmation_manager → safe deny
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Delete", None).await.unwrap();

    // Should be denied (no policy + no CM = safe deny)
    assert_eq!(result.text, "Denied!");
    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_permission_ask_without_cm_denies() {
    // When permission is Ask and no confirmation manager configured,
    // tool execution should be denied (safe default).
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo test"}),
        ),
        MockLlmClient::text_response("Denied!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create policy where bash falls through to Ask (default)
    let permission_policy = PermissionPolicy::new(); // Default decision is Ask

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        // No confirmation_manager — safe deny
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Echo", None).await.unwrap();

    // Should deny (Ask without CM = safe deny)
    assert_eq!(result.text, "Denied!");
    // The tool result should contain the denial message
    assert!(result.tool_calls_count >= 1);
}

// ========================================================================
// HITL (Human-in-the-Loop) Tests
// ========================================================================

#[tokio::test]
async fn test_agent_hitl_approved() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo hello"}),
        ),
        MockLlmClient::text_response("Command executed!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL confirmation manager with policy enabled
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    // Create permission policy that returns Ask for bash
    let permission_policy = PermissionPolicy::new(); // Default is Ask

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        ..Default::default()
    };

    // Spawn a task to approve the confirmation
    let cm_clone = confirmation_manager.clone();
    tokio::spawn(async move {
        // Wait a bit for the confirmation request to be created
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Approve it
        cm_clone.confirm("tool-1", true, None).await.ok();
    });

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Run echo", None).await.unwrap();

    assert_eq!(result.text, "Command executed!");
    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_hitl_wait_does_not_consume_tool_timeout_budget() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo approved"}),
        ),
        MockLlmClient::text_response("Command executed after approval."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 1_000,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));
    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        tool_timeout_ms: Some(50),
        ..Default::default()
    };

    let cm_clone = confirmation_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cm_clone.confirm("tool-1", true, None).await.ok();
    });

    let started = std::time::Instant::now();
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Run echo", None).await.unwrap();

    assert!(
        started.elapsed() >= std::time::Duration::from_millis(100),
        "test must wait longer than the configured tool timeout"
    );
    assert_eq!(result.text, "Command executed after approval.");
    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_hitl_rejected() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "rm -rf /"}),
        ),
        MockLlmClient::text_response("Understood, I won't do that."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL confirmation manager
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    // Permission policy returns Ask
    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        ..Default::default()
    };

    // Spawn a task to reject the confirmation
    let cm_clone = confirmation_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cm_clone
            .confirm("tool-1", false, Some("Too dangerous".to_string()))
            .await
            .ok();
    });

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Delete everything", None).await.unwrap();

    // LLM should respond to the rejection
    assert_eq!(result.text, "Understood, I won't do that.");
}

#[tokio::test]
async fn test_agent_hitl_timeout_reject() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy, TimeoutAction};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo test"}),
        ),
        MockLlmClient::text_response("Timed out, I understand."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL with very short timeout and Reject action
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 50, // Very short timeout
        timeout_action: TimeoutAction::Reject,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager),
        ..Default::default()
    };

    // Don't approve - let it timeout
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Echo", None).await.unwrap();

    // Should get timeout rejection response from LLM
    assert_eq!(result.text, "Timed out, I understand.");
}

#[tokio::test]
async fn test_agent_hitl_timeout_auto_approve() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy, TimeoutAction};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo hello"}),
        ),
        MockLlmClient::text_response("Auto-approved and executed!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL with very short timeout and AutoApprove action
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 50, // Very short timeout
        timeout_action: TimeoutAction::AutoApprove,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager),
        ..Default::default()
    };

    // Don't approve - let it timeout and auto-approve
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Echo", None).await.unwrap();

    // Should auto-approve on timeout and execute
    assert_eq!(result.text, "Auto-approved and executed!");
    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_hitl_confirmation_events() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo test"}),
        ),
        MockLlmClient::text_response("Done!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL confirmation manager
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 5000, // Long enough timeout
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        ..Default::default()
    };

    // Spawn task to approve and collect events
    let cm_clone = confirmation_manager.clone();
    let event_handle = tokio::spawn(async move {
        let mut events = Vec::new();
        // Wait for ConfirmationRequired event
        while let Ok(event) = event_rx.recv().await {
            events.push(event.clone());
            if let AgentEvent::ConfirmationRequired { tool_id, .. } = event {
                // Approve it
                cm_clone.confirm(&tool_id, true, None).await.ok();
                // Wait for ConfirmationReceived
                if let Ok(recv_event) = event_rx.recv().await {
                    events.push(recv_event);
                }
                break;
            }
        }
        events
    });

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let _result = agent.execute(&[], "Echo", None).await.unwrap();

    // Check events
    let events = event_handle.await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ConfirmationRequired { .. })),
        "Should have ConfirmationRequired event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ConfirmationReceived { approved: true, .. })),
        "Should have ConfirmationReceived event with approved=true"
    );
}

#[tokio::test]
async fn test_agent_hitl_disabled_auto_executes() {
    // When HITL is disabled, tools should execute automatically even with Ask permission
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo auto"}),
        ),
        MockLlmClient::text_response("Auto executed!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL with enabled=false
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: false, // HITL disabled
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new(); // Default is Ask

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager),
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Echo", None).await.unwrap();

    // Should execute without waiting for confirmation
    assert_eq!(result.text, "Auto executed!");
    assert_eq!(result.tool_calls_count, 1);
}

#[tokio::test]
async fn test_agent_hitl_with_permission_deny_skips_hitl() {
    // When permission is Deny, HITL should not be triggered
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "rm -rf /"}),
        ),
        MockLlmClient::text_response("Blocked by permission."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL enabled
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    // Permission policy denies rm commands
    let permission_policy = PermissionPolicy::new().deny("bash(rm:*)");

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager),
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Delete", None).await.unwrap();

    // Should be denied without HITL
    assert_eq!(result.text, "Blocked by permission.");

    // Should NOT have any ConfirmationRequired events
    let mut found_confirmation = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
            found_confirmation = true;
        }
    }
    assert!(
        !found_confirmation,
        "HITL should not be triggered when permission is Deny"
    );
}

#[tokio::test]
async fn test_agent_hitl_with_permission_allow_skips_hitl() {
    // When permission is Allow, HITL confirmation is skipped entirely.
    // PermissionPolicy is the declarative rule engine; Allow = execute directly.
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "bash",
            serde_json::json!({"command": "echo hello"}),
        ),
        MockLlmClient::text_response("Allowed!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL enabled
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    // Permission policy allows echo commands
    let permission_policy = PermissionPolicy::new().allow("bash(echo:*)");

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Echo", None).await.unwrap();

    // Should execute directly without HITL (permission Allow skips confirmation)
    assert_eq!(result.text, "Allowed!");

    // Should NOT have ConfirmationRequired event (Allow bypasses HITL)
    let mut found_confirmation = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
            found_confirmation = true;
        }
    }
    assert!(
        !found_confirmation,
        "Permission Allow should skip HITL confirmation"
    );
}

#[tokio::test]
async fn test_agent_hitl_multiple_tool_calls() {
    // Test multiple tool calls in sequence with HITL
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        // First response: two tool calls
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo first"}),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-2".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo second"}),
                    },
                ],
                reasoning_content: None,
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
        },
        MockLlmClient::text_response("Both executed!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // Create HITL
    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 5000,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new(); // Default Ask

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        ..Default::default()
    };

    // Spawn task to approve both tools
    let cm_clone = confirmation_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cm_clone.confirm("tool-1", true, None).await.ok();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cm_clone.confirm("tool-2", true, None).await.ok();
    });

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent
        .execute_loop(
            &[],
            "run both commands now",
            AgentStyle::GeneralPurpose,
            None,
            None,
            &tokio_util::sync::CancellationToken::new(),
            true,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Both executed!");
    assert_eq!(result.tool_calls_count, 2);
}

#[tokio::test]
async fn test_agent_hitl_partial_approval() {
    // Test: first tool approved, second rejected
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        // First response: two tool calls
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo safe"}),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-2".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "rm -rf /"}),
                    },
                ],
                reasoning_content: None,
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
        },
        MockLlmClient::text_response("First worked, second rejected."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let (event_tx, _event_rx) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        default_timeout_ms: 5000,
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager.clone()),
        ..Default::default()
    };

    // Approve first, reject second
    let cm_clone = confirmation_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cm_clone.confirm("tool-1", true, None).await.ok();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cm_clone
            .confirm("tool-2", false, Some("Dangerous".to_string()))
            .await
            .ok();
    });

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Run both", None).await.unwrap();

    assert_eq!(result.text, "First worked, second rejected.");
    assert_eq!(result.tool_calls_count, 2);
}

#[tokio::test]
async fn test_agent_hitl_yolo_mode_auto_approves() {
    // YOLO mode: specific lanes auto-approve without confirmation
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use crate::queue::SessionLane;
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-1",
            "read", // Query lane tool
            serde_json::json!({"path": "/tmp/test.txt"}),
        ),
        MockLlmClient::text_response("File read!"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // YOLO mode for Query lane (read, glob, ls, grep)
    let (event_tx, mut event_rx) = broadcast::channel(100);
    let mut yolo_lanes = std::collections::HashSet::new();
    yolo_lanes.insert(SessionLane::Query);
    let hitl_policy = ConfirmationPolicy {
        enabled: true,
        yolo_lanes, // Auto-approve query operations
        ..Default::default()
    };
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new();

    let config = AgentConfig {
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager),
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Read file", None).await.unwrap();

    // Should auto-execute without confirmation (YOLO mode)
    assert_eq!(result.text, "File read!");

    // Should NOT have ConfirmationRequired for yolo lane
    let mut found_confirmation = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
            found_confirmation = true;
        }
    }
    assert!(
        !found_confirmation,
        "YOLO mode should not trigger confirmation"
    );
}

#[tokio::test]
async fn test_agent_config_with_all_options() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use tokio::sync::broadcast;

    let (event_tx, _) = broadcast::channel(100);
    let hitl_policy = ConfirmationPolicy::default();
    let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

    let permission_policy = PermissionPolicy::new().allow("bash(*)");

    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("Test system prompt".to_string()),
            ..Default::default()
        },
        tools: vec![],
        max_tool_rounds: 10,
        permission_checker: Some(Arc::new(permission_policy)),
        confirmation_manager: Some(confirmation_manager),
        context_providers: vec![],
        planning_mode: PlanningMode::default(),
        goal_tracking: false,
        hook_engine: None,
        skill_registry: None,
        ..AgentConfig::default()
    };

    assert!(config.prompt_slots.build().contains("Test system prompt"));
    assert_eq!(config.max_tool_rounds, 10);
    assert!(config.permission_checker.is_some());
    assert!(config.confirmation_manager.is_some());
    assert!(config.context_providers.is_empty());

    // Test Debug trait
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("AgentConfig"));
    assert!(debug_str.contains("permission_checker: true"));
    assert!(debug_str.contains("confirmation_manager: true"));
    assert!(debug_str.contains("context_providers: 0"));
}

// ========================================================================
// Context Provider Tests
// ========================================================================

/// Mock context provider for testing
struct MockContextProvider {
    name: String,
    items: Vec<ContextItem>,
    on_turn_calls: std::sync::Arc<tokio::sync::RwLock<Vec<(String, String, String)>>>,
}

impl MockContextProvider {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            items: Vec::new(),
            on_turn_calls: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    fn with_items(mut self, items: Vec<ContextItem>) -> Self {
        self.items = items;
        self
    }
}

#[async_trait::async_trait]
impl ContextProvider for MockContextProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
        let mut result = ContextResult::new(&self.name);
        for item in &self.items {
            result.add_item(item.clone());
        }
        Ok(result)
    }

    async fn on_turn_complete(
        &self,
        session_id: &str,
        prompt: &str,
        response: &str,
    ) -> anyhow::Result<()> {
        let mut calls = self.on_turn_calls.write().await;
        calls.push((
            session_id.to_string(),
            prompt.to_string(),
            response.to_string(),
        ));
        Ok(())
    }
}

#[tokio::test]
async fn test_agent_with_context_provider() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Response using context",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let provider = MockContextProvider::new("test-provider").with_items(vec![ContextItem::new(
        "ctx-1",
        ContextType::Resource,
        "Relevant context here",
    )
    .with_source("test://docs/example")]);

    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("You are helpful.".to_string()),
            ..Default::default()
        },
        context_providers: vec![Arc::new(provider)],
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent
        .execute(&[], "verify context provider output", None)
        .await
        .unwrap();

    assert_eq!(result.text, "Response using context");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_agent_context_provider_events() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Answer",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let provider = MockContextProvider::new("event-provider").with_items(vec![ContextItem::new(
        "item-1",
        ContextType::Memory,
        "Memory content",
    )
    .with_token_count(50)]);

    let config = AgentConfig {
        context_providers: vec![Arc::new(provider)],
        ..Default::default()
    };

    let (tx, mut rx) = mpsc::channel(100);
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let _result = agent.execute(&[], "Test prompt", Some(tx)).await.unwrap();

    // Collect events
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // Should have ContextResolving and ContextResolved events
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ContextResolving { .. })),
        "Should have ContextResolving event"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ContextResolved { .. })),
        "Should have ContextResolved event"
    );

    // Check context resolved values
    for event in &events {
        if let AgentEvent::ContextResolved {
            total_items,
            total_tokens,
        } = event
        {
            assert_eq!(*total_items, 1);
            assert_eq!(*total_tokens, 50);
        }
    }
}

#[tokio::test]
async fn test_agent_multiple_context_providers() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Combined response",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let provider1 = MockContextProvider::new("provider-1").with_items(vec![ContextItem::new(
        "p1-1",
        ContextType::Resource,
        "Resource from P1",
    )
    .with_token_count(100)]);

    let provider2 = MockContextProvider::new("provider-2").with_items(vec![
        ContextItem::new("p2-1", ContextType::Memory, "Memory from P2").with_token_count(50),
        ContextItem::new("p2-2", ContextType::Skill, "Skill from P2").with_token_count(75),
    ]);

    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("Base system prompt.".to_string()),
            ..Default::default()
        },
        context_providers: vec![Arc::new(provider1), Arc::new(provider2)],
        ..Default::default()
    };

    let (tx, mut rx) = mpsc::channel(100);
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent
        .execute(&[], "verify combined context", Some(tx))
        .await
        .unwrap();

    assert_eq!(result.text, "Combined response");

    // Check context resolved event has combined totals
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ContextResolved {
            total_items,
            total_tokens,
        } = event
        {
            assert_eq!(total_items, 3); // 1 + 2
            assert_eq!(total_tokens, 225); // 100 + 50 + 75
        }
    }
}

#[tokio::test]
async fn test_agent_no_context_providers() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "No context",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    // No context providers
    let config = AgentConfig::default();

    let (tx, mut rx) = mpsc::channel(100);
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent
        .execute(&[], "verify simple prompt", Some(tx))
        .await
        .unwrap();

    assert_eq!(result.text, "No context");

    // Should NOT have context events when no providers
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ContextResolving { .. })),
        "Should NOT have ContextResolving event"
    );
}

#[tokio::test]
async fn test_agent_memory_recall_routes_through_context_assembly() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Memory-aware response",
    )]));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    memory
        .remember(
            a3s_memory::MemoryItem::new(
                "verify focused regression tests caught context regressions.",
            )
            .with_importance(0.9),
        )
        .await
        .unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let (tx, mut rx) = mpsc::channel(100);
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute(&[], "verify focused regression tests", Some(tx))
        .await
        .unwrap();

    assert_eq!(result.text, "Memory-aware response");

    let mut recalled = false;
    let mut resolved_items = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::MemoryRecalled { content, .. } => {
                recalled = content.contains("focused regression tests");
            }
            AgentEvent::ContextResolved { total_items, .. } => {
                resolved_items = Some(total_items);
            }
            _ => {}
        }
    }

    assert!(recalled);
    assert_eq!(resolved_items, Some(1));
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_stores_durable_items() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Use focused memory tests after store changes."),
        MockLlmClient::text_response(
            r#"{"items":[{"memory_type":"procedural","content":"Run focused memory store tests after changing FileMemoryStore behavior.","importance":0.85,"confidence":0.94,"tags":["memory","tests"],"source":"workflow","scope":"workspace","reason":"This repeatable verification catches persistence regressions in future changes."}]}"#,
        ),
    ]));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the workflow for FileMemoryStore changes",
            Some("sess-memory-extraction"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Use focused memory tests after store changes.");
    let memory = agent.config.memory.as_ref().unwrap();
    let recalled = memory
        .recall_by_tags(&["memory".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(recalled.len(), 1);
    assert!(recalled[0]
        .content
        .contains("Run focused memory store tests"));
}

#[tokio::test]
async fn completed_tool_results_do_not_become_long_term_memory_history() {
    let memory = Arc::new(crate::memory::AgentMemory::with_config(
        Arc::new(a3s_memory::InMemoryStore::new()),
        crate::memory::MemoryConfig {
            llm_extraction: false,
            ..Default::default()
        },
    ));
    let temp_dir = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        Arc::new(ToolExecutor::new(temp_dir.path().display().to_string())),
        ToolContext::new(temp_dir.path().to_path_buf()),
        AgentConfig {
            memory: Some(Arc::clone(&memory)),
            ..Default::default()
        },
    );
    let mut state = super::execution_state::ExecutionLoopState::new(&[]);
    let no_events = None;

    for (tool_call, result) in [
        (
            crate::llm::ToolCall {
                id: "tool-success".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({"command":"cargo test"}),
            },
            crate::tools::ToolResult::success("bash", "tests passed".to_string()),
        ),
        (
            crate::llm::ToolCall {
                id: "tool-failure".to_string(),
                name: "read".to_string(),
                args: serde_json::json!({"file_path":"missing.rs"}),
            },
            crate::tools::ToolResult::error("read", "file not found".to_string()),
        ),
    ] {
        agent
            .complete_tool_call(
                &mut state,
                super::tool_completion_runtime::ToolCompletionInput {
                    tool_call: &tool_call,
                    event_tx: &no_events,
                    session_id: Some("sess-tool-history"),
                    tool_start: std::time::Instant::now(),
                    normalized: super::tool_result_runtime::NormalizedToolResult::from_tool_result(
                        result,
                    ),
                },
            )
            .await;
    }

    assert_eq!(memory.stats().await.unwrap().long_term_count, 0);
}

#[tokio::test]
async fn test_streaming_llm_memory_extraction_does_not_block_final_result() {
    let mock_client = Arc::new(BlockingExtractionLlmClient::new());

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let (event_tx, _event_rx) = mpsc::channel(32);

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        agent.execute_with_session(
            &[],
            "remember that streaming memory extraction must not block final output",
            Some("sess-streaming-memory-extraction"),
            Some(event_tx),
            None,
        ),
    )
    .await
    .expect("streaming final result should not wait for memory extraction")
    .unwrap();

    assert_eq!(result.text, "Stored durable memory workflow.");
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mock_client.extraction_started.notified(),
    )
    .await
    .expect("background extraction should still start after the final result");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);

    mock_client.extraction_release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mock_client.extraction_finished.notified(),
    )
    .await
    .expect("background extraction should finish after release");
}

#[tokio::test]
async fn test_auto_compact_timeout_does_not_block_end_event() {
    let mock_client = Arc::new(HangingCompactionLlmClient::new());
    let budget = Arc::new(CountingBudgetGuard::new(0));
    let history = (0..30)
        .map(|i| {
            if i % 2 == 0 {
                Message::user(&format!("historical user message {i}"))
            } else {
                Message::assistant(&format!("historical assistant message {i}"))
            }
        })
        .collect::<Vec<_>>();

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        prompt_slots: SystemPromptSlots {
            style: Some(AgentStyle::GeneralPurpose),
            ..Default::default()
        },
        auto_compact: true,
        auto_compact_threshold: 0.10,
        max_context_tokens: 1_000,
        llm_api_timeout_ms: Some(20),
        continuation_enabled: false,
        budget_guard: Some(budget.clone()),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let (event_tx, event_rx) = mpsc::channel(64);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.execute_with_session(
            &history,
            "finish after compact timeout",
            Some("sess-auto-compact-timeout"),
            Some(event_tx),
            None,
        ),
    )
    .await
    .expect("auto-compact timeout must not block the final result")
    .unwrap();

    assert_eq!(result.text, "Final answer complete.");
    assert_eq!(mock_client.complete_calls.load(Ordering::SeqCst), 1);
    assert_eq!(budget.check_count.load(Ordering::SeqCst), 2);
    assert_eq!(budget.record_count.load(Ordering::SeqCst), 1);

    let events = collect_events(event_rx).await;
    let turn_end_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::TurnEnd { .. }))
        .expect("turn end should be emitted before auto-compact");
    let end_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::End { .. }))
        .expect("agent end should still be emitted after compact timeout");

    assert!(turn_end_index < end_index);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ContextCompacted { .. })),
        "a timed-out summary must not be reported as a successful compaction"
    );
}

#[tokio::test]
async fn test_auto_compact_rearms_and_continues_across_cycles() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("summary cycle one"),
        MockLlmClient::text_response("first task complete"),
        MockLlmClient::text_response("summary cycle two"),
        MockLlmClient::text_response("second task complete"),
    ]));
    let history = (0..40)
        .map(|i| {
            let text = format!(
                "historical message {i} carries enough context to cross the compact threshold"
            );
            if i % 2 == 0 {
                Message::user(&text)
            } else {
                Message::assistant(&text)
            }
        })
        .collect::<Vec<_>>();

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        prompt_slots: SystemPromptSlots {
            style: Some(AgentStyle::GeneralPurpose),
            ..Default::default()
        },
        auto_compact: true,
        auto_compact_threshold: 0.50,
        max_context_tokens: 100,
        continuation_enabled: false,
        ..Default::default()
    };
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );

    let first = agent
        .execute_with_session(
            &history,
            "finish the first task",
            Some("repeatable-compaction"),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(first.text, "first task complete");

    let second = agent
        .execute_with_session(
            &first.messages,
            "finish the second task",
            Some("repeatable-compaction"),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(second.text, "second task complete");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 4);

    let requests = mock_client.request_texts.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[1].contains("summary cycle one"));
    assert!(requests[1].contains("finish the first task"));
    assert!(requests[2].contains("summary cycle one"));
    assert!(requests[3].contains("summary cycle two"));
    assert!(requests[3].contains("finish the second task"));
}

#[tokio::test]
async fn test_auto_compact_can_repeat_inside_one_tool_driven_task() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("summary before tool work"),
        MockLlmClient::tool_call_response(
            "rolling-tool",
            "bash",
            serde_json::json!({"command": "echo rolling-context"}),
        ),
        MockLlmClient::text_response("summary after tool work"),
        MockLlmClient::text_response("task continued after both compactions"),
    ]));
    let history = (0..40)
        .map(|i| {
            let text = format!("long-running task context message {i} must remain recoverable");
            if i % 2 == 0 {
                Message::user(&text)
            } else {
                Message::assistant(&text)
            }
        })
        .collect::<Vec<_>>();

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        AgentConfig {
            planning_mode: PlanningMode::Disabled,
            prompt_slots: SystemPromptSlots {
                style: Some(AgentStyle::GeneralPurpose),
                ..Default::default()
            },
            permission_checker: Some(Arc::new(PermissionPolicy::new().allow("bash(echo:*)"))),
            auto_compact: true,
            auto_compact_threshold: 0.50,
            max_context_tokens: 100,
            continuation_enabled: false,
            ..Default::default()
        },
    );

    let result = agent
        .execute_with_session(
            &history,
            "complete the tool-driven task",
            Some("same-task-compaction"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "task continued after both compactions");
    assert_eq!(result.tool_calls_count, 1);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 4);

    let requests = mock_client.request_texts.lock().unwrap();
    assert!(requests[1].contains("summary before tool work"));
    assert!(requests[2].contains("echo rolling-context"));
    assert!(requests[2].contains("rolling-context"));
    assert!(requests[3].contains("summary after tool work"));
}

#[tokio::test]
async fn test_streaming_llm_memory_extraction_queues_every_completed_turn() {
    let mock_client = Arc::new(BlockingExtractionLlmClient::new());

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );

    let (first_tx, _first_rx) = mpsc::channel(32);
    let first = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        agent.execute_with_session(
            &[],
            "remember the first durable workflow for memory extraction",
            Some("sess-streaming-memory-single-flight-1"),
            Some(first_tx),
            None,
        ),
    )
    .await
    .expect("first streaming result should not wait for memory extraction")
    .unwrap();
    assert_eq!(first.text, "Stored durable memory workflow.");

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mock_client.extraction_started.notified(),
    )
    .await
    .expect("first background extraction should start");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);

    let (second_tx, _second_rx) = mpsc::channel(32);
    let second = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        agent.execute_with_session(
            &[],
            "remember the second durable workflow for memory extraction",
            Some("sess-streaming-memory-single-flight-2"),
            Some(second_tx),
            None,
        ),
    )
    .await
    .expect("second streaming result should not wait for the first extraction")
    .unwrap();
    assert_eq!(second.text, "Stored durable memory workflow.");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        mock_client.call_count.load(Ordering::SeqCst),
        3,
        "the second extraction must wait behind the first instead of running concurrently"
    );

    mock_client.extraction_release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mock_client.extraction_finished.notified(),
    )
    .await
    .expect("first background extraction should finish after release");

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mock_client.extraction_started.notified(),
    )
    .await
    .expect("the queued second extraction should start after the first finishes");
    assert_eq!(
        mock_client.call_count.load(Ordering::SeqCst),
        4,
        "both completed turns must receive an extraction call"
    );

    mock_client.extraction_release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mock_client.extraction_finished.notified(),
    )
    .await
    .expect("second queued extraction should finish after release");
}

#[tokio::test]
async fn test_streaming_llm_memory_judge_evaluates_trivial_turns_asynchronously() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("hello"),
        MockLlmClient::text_response(r#"{"items":[]}"#),
    ]));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let (event_tx, _event_rx) = mpsc::channel(32);

    let result = agent
        .execute_with_session(
            &[],
            "hi",
            Some("sess-trivial-stream-memory"),
            Some(event_tx),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "hello");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        mock_client.call_count.load(Ordering::SeqCst),
        2,
        "the LLM value judge should evaluate even short completed turns"
    );
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_uses_budget_guard() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Use focused memory tests after store changes."),
        MockLlmClient::text_response(
            r#"{"items":[{"memory_type":"procedural","content":"Run focused memory store tests after changing FileMemoryStore behavior.","importance":0.85,"confidence":0.94,"tags":["memory","tests"],"source":"workflow","scope":"workspace","reason":"This repeatable verification catches persistence regressions in future changes."}]}"#,
        ),
    ]));
    let budget = Arc::new(CountingBudgetGuard::new(0));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        budget_guard: Some(budget.clone() as Arc<dyn crate::budget::BudgetGuard>),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the workflow for FileMemoryStore changes",
            Some("sess-memory-budget"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Use focused memory tests after store changes.");
    // Auto pre-analysis, the normal turn, and memory extraction are three
    // distinct provider calls and each must cross the same budget boundary.
    assert_eq!(budget.check_count.load(Ordering::SeqCst), 3);
    assert_eq!(budget.record_count.load(Ordering::SeqCst), 3);
    assert_eq!(budget.recorded_tokens.load(Ordering::SeqCst), 45);
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_includes_related_memory_context() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Use focused memory tests after store changes."),
        MockLlmClient::text_response(r#"{"items":[]}"#),
    ]));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    memory
        .remember(
            a3s_memory::MemoryItem::new(
                "Run focused memory store tests after changing FileMemoryStore behavior.",
            )
            .with_type(a3s_memory::MemoryType::Procedural)
            .with_tag("memory"),
        )
        .await
        .unwrap();
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the workflow for FileMemoryStore changes",
            Some("sess-memory-related-context"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Use focused memory tests after store changes.");
    let prompts = mock_client.request_texts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    let extraction_prompt = prompts.last().unwrap();
    assert!(extraction_prompt.contains("Related existing memories"));
    assert!(extraction_prompt.contains("FileMemoryStore behavior"));
    assert!(extraction_prompt.contains("avoid duplicates"));
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_supersedes_related_memory() {
    let memory = Arc::new(crate::memory::AgentMemory::new(Arc::new(
        a3s_memory::InMemoryStore::new(),
    )));
    let old_item = a3s_memory::MemoryItem::new(
        "Run focused memory store tests after changing FileMemoryStore behavior.",
    )
    .with_type(a3s_memory::MemoryType::Procedural)
    .with_tag("memory");
    let old_id = old_item.id.clone();
    memory.remember(old_item).await.unwrap();

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Use focused memory and file store tests after changes."),
        MockLlmClient::text_response(&format!(
            r#"{{"items":[{{"memory_type":"procedural","content":"Run focused memory store and file-backed persistence tests after changing FileMemoryStore behavior.","importance":0.9,"confidence":0.96,"tags":["memory","tests"],"source":"workflow","scope":"workspace","reason":"This improved workflow verifies both in-memory and file-backed persistence regressions.","supersedes":["{old_id}"]}}]}}"#
        )),
    ]));

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(memory.clone()),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the improved workflow for FileMemoryStore changes",
            Some("sess-memory-supersede"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        result.text,
        "Use focused memory and file store tests after changes."
    );
    assert!(memory.store().retrieve(&old_id).await.unwrap().is_none());
    let recalled = memory
        .recall_by_tags(&["consolidated".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].metadata.get("supersedes").unwrap(), &old_id);
    assert!(recalled[0].content.contains("file-backed persistence"));
    assert_eq!(memory.stats().await.unwrap().long_term_count, 1);
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_marks_conflicting_related_memory() {
    let memory = Arc::new(crate::memory::AgentMemory::new(Arc::new(
        a3s_memory::InMemoryStore::new(),
    )));
    let old_item = a3s_memory::MemoryItem::new("TUI memory defaults to ~/.a3s/memory.")
        .with_type(a3s_memory::MemoryType::Semantic)
        .with_tag("memory");
    let old_id = old_item.id.clone();
    memory.remember(old_item).await.unwrap();

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("SDK sessions use workspace-local memory by default."),
        MockLlmClient::text_response(&format!(
            r#"{{"items":[{{"memory_type":"semantic","content":"SDK sessions default to workspace-local .a3s/memory stores, while the TUI default is global ~/.a3s/memory.","importance":0.8,"confidence":0.93,"tags":["memory","defaults"],"source":"project_fact","scope":"workspace","reason":"This durable default changes where future sessions look for persisted memory.","conflicts_with":["{old_id}"]}}]}}"#
        )),
    ]));

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(memory.clone()),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the corrected memory default behavior",
            Some("sess-memory-conflict"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        result.text,
        "SDK sessions use workspace-local memory by default."
    );
    assert!(memory.store().retrieve(&old_id).await.unwrap().is_some());
    let conflicts = memory
        .recall_by_tags(&["conflict".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].metadata.get("conflicts_with").unwrap(),
        &old_id
    );
    assert_eq!(memory.stats().await.unwrap().long_term_count, 2);
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_skips_when_budget_denies() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Use focused memory tests after store changes.",
    )]));
    // Deny the memory extraction call after pre-analysis + the main turn.
    let budget = Arc::new(CountingBudgetGuard::new(3));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        budget_guard: Some(budget.clone() as Arc<dyn crate::budget::BudgetGuard>),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the workflow for FileMemoryStore changes",
            Some("sess-memory-budget-denied"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Use focused memory tests after store changes.");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
    assert_eq!(budget.check_count.load(Ordering::SeqCst), 3);
    assert_eq!(budget.record_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent
            .config
            .memory
            .as_ref()
            .unwrap()
            .stats()
            .await
            .unwrap()
            .long_term_count,
        0
    );
}

#[tokio::test]
async fn test_agent_llm_memory_extraction_merges_duplicate_durable_items() {
    let existing_content =
        "Run focused memory store tests after changing FileMemoryStore behavior.";
    let extracted_content = existing_content;
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Already noted."),
        MockLlmClient::text_response(&format!(
            r#"{{"items":[{{"memory_type":"procedural","content":"{extracted_content}","importance":0.85,"confidence":0.94,"tags":["memory","tests"],"source":"workflow","scope":"workspace","reason":"This exact workflow is reusable after future FileMemoryStore changes."}}]}}"#
        )),
    ]));

    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let existing = a3s_memory::MemoryItem::new(existing_content)
        .with_type(a3s_memory::MemoryType::Procedural)
        .with_importance(0.4)
        .with_tag("memory");
    let existing_id = existing.id.clone();
    memory.remember(existing).await.unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "remember the workflow for FileMemoryStore changes",
            Some("sess-memory-dedupe"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Already noted.");
    let stats = agent.config.memory.as_ref().unwrap().stats().await.unwrap();
    assert_eq!(stats.long_term_count, 1);
    let merged = agent
        .config
        .memory
        .as_ref()
        .unwrap()
        .store()
        .retrieve(&existing_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(merged.content, existing_content);
    assert_eq!(merged.importance, 0.85);
    assert!(merged.tags.contains(&"tests".to_string()));
    assert_eq!(
        merged.metadata.get("duplicate_count").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        merged.metadata.get("source").map(String::as_str),
        Some("workflow")
    );
}

#[tokio::test]
async fn test_agent_llm_memory_judge_returns_empty_for_trivial_turns() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Hello."),
        MockLlmClient::text_response(r#"{"items":[]}"#),
    ]));
    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let temp_dir = tempfile::tempdir().unwrap();
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(&[], "hi", Some("sess-memory-trivial"), None, None)
        .await
        .unwrap();

    assert_eq!(result.text, "Hello.");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent
            .config
            .memory
            .as_ref()
            .unwrap()
            .stats()
            .await
            .unwrap()
            .long_term_count,
        0
    );
}

#[tokio::test]
async fn test_agent_llm_memory_judge_returns_empty_for_read_only_tool_turns() {
    use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
    use crate::queue::SessionLane;
    use tokio::sync::broadcast;

    let temp_dir = tempfile::tempdir().unwrap();
    let readme = temp_dir.path().join("README.md");
    tokio::fs::write(&readme, "A3S memory notes.\n")
        .await
        .unwrap();

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "tool-read",
            "read",
            serde_json::json!({"file_path": "README.md"}),
        ),
        MockLlmClient::text_response("README says A3S memory notes."),
        MockLlmClient::text_response(r#"{"items":[]}"#),
    ]));
    let memory = crate::memory::AgentMemory::new(Arc::new(a3s_memory::InMemoryStore::new()));
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let (event_tx, _event_rx) = broadcast::channel(100);
    let mut yolo_lanes = std::collections::HashSet::new();
    yolo_lanes.insert(SessionLane::Query);
    let confirmation_manager = Arc::new(ConfirmationManager::new(
        ConfirmationPolicy {
            enabled: true,
            yolo_lanes,
            ..Default::default()
        },
        event_tx,
    ));
    let config = AgentConfig {
        memory: Some(Arc::new(memory)),
        confirmation_manager: Some(confirmation_manager),
        permission_checker: Some(Arc::new(PermissionPolicy::new())),
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        config,
    );
    let result = agent
        .execute_with_session(
            &[],
            "read README",
            Some("sess-short-read-only-memory"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "README says A3S memory notes.");
    assert_eq!(result.tool_calls_count, 1);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 3);
    assert_eq!(
        agent
            .config
            .memory
            .as_ref()
            .unwrap()
            .stats()
            .await
            .unwrap()
            .long_term_count,
        0
    );
}

#[tokio::test]
async fn test_agent_context_on_turn_complete() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Final response",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let provider = Arc::new(MockContextProvider::new("memory-provider"));
    let on_turn_calls = provider.on_turn_calls.clone();

    let config = AgentConfig {
        context_providers: vec![provider],
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    // Execute with session ID
    let result = agent
        .execute_with_session(&[], "verify user prompt", Some("sess-123"), None, None)
        .await
        .unwrap();

    assert_eq!(result.text, "Final response");

    // Check on_turn_complete was called
    let calls = on_turn_calls.read().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "sess-123");
    assert_eq!(calls[0].1, "verify user prompt");
    assert_eq!(calls[0].2, "Final response");
}

#[tokio::test]
async fn test_agent_context_on_turn_complete_no_session() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Response",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let provider = Arc::new(MockContextProvider::new("memory-provider"));
    let on_turn_calls = provider.on_turn_calls.clone();

    let config = AgentConfig {
        context_providers: vec![provider],
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    // Execute without session ID (uses execute() which passes None)
    let _result = agent.execute(&[], "Prompt", None).await.unwrap();

    // on_turn_complete should NOT be called when session_id is None
    let calls = on_turn_calls.read().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn test_agent_build_augmented_system_prompt() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response("OK")]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

    let provider = MockContextProvider::new("test").with_items(vec![ContextItem::new(
        "doc-1",
        ContextType::Resource,
        "Auth uses JWT tokens.",
    )
    .with_source("viking://docs/auth")]);

    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("You are helpful.".to_string()),
            ..Default::default()
        },
        context_providers: vec![Arc::new(provider)],
        ..Default::default()
    };

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    // Test building augmented prompt
    let context_results = agent.resolve_context("test", None).await;
    let augmented = agent.build_augmented_system_prompt(&context_results);

    let augmented_str = augmented.unwrap();
    assert!(augmented_str.contains("You are helpful."));
    assert!(augmented_str.contains("<context source=\"viking://docs/auth\" type=\"Resource\">"));
    assert!(augmented_str.contains("Auth uses JWT tokens."));
}

// ========================================================================
// Agentic Loop Integration Tests
// ========================================================================

/// Helper: collect all events from a channel
async fn collect_events(mut rx: mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    // Drain remaining
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn test_agent_multi_turn_tool_chain() {
    // LLM calls tool A → sees result → calls tool B → sees result → final answer
    let mock_client = Arc::new(MockLlmClient::new(vec![
        // Turn 1: call ls
        MockLlmClient::tool_call_response(
            "t1",
            "bash",
            serde_json::json!({"command": "echo step1"}),
        ),
        // Turn 2: call another tool based on first result
        MockLlmClient::tool_call_response(
            "t2",
            "bash",
            serde_json::json!({"command": "echo step2"}),
        ),
        // Turn 3: final answer
        MockLlmClient::text_response("Completed both steps: step1 then step2"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Run two steps", None).await.unwrap();

    assert_eq!(result.text, "Completed both steps: step1 then step2");
    assert_eq!(result.tool_calls_count, 2);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 3);

    // Verify message history: user → assistant(tool_use) → user(tool_result) → assistant(tool_use) → user(tool_result) → assistant(text)
    assert_eq!(result.messages[0].role, "user");
    assert_eq!(result.messages[1].role, "assistant"); // tool call 1
    assert_eq!(result.messages[2].role, "user"); // tool result 1 (Anthropic convention)
    assert_eq!(result.messages[3].role, "assistant"); // tool call 2
    assert_eq!(result.messages[4].role, "user"); // tool result 2
    assert_eq!(result.messages[5].role, "assistant"); // final text
    assert_eq!(result.messages.len(), 6);
}

#[tokio::test]
async fn test_agent_conversation_history_preserved() {
    // Pass existing history, verify it's preserved in output
    let existing_history = vec![
        Message::user("What is Rust?"),
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Rust is a systems programming language.".to_string(),
            }],
            reasoning_content: None,
        },
    ];

    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Rust was created by Graydon Hoare at Mozilla.",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        AgentConfig {
            prompt_slots: SystemPromptSlots {
                style: Some(AgentStyle::GeneralPurpose),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let result = agent
        .execute(&existing_history, "Who created it?", None)
        .await
        .unwrap();

    // History should contain: old user + old assistant + new user + new assistant
    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.messages[0].text(), "What is Rust?");
    assert_eq!(
        result.messages[1].text(),
        "Rust is a systems programming language."
    );
    assert_eq!(result.messages[2].text(), "Who created it?");
    assert_eq!(
        result.messages[3].text(),
        "Rust was created by Graydon Hoare at Mozilla."
    );
}

#[tokio::test]
async fn test_agent_event_stream_completeness() {
    // Verify full event sequence for a single tool call loop
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response("t1", "bash", serde_json::json!({"command": "echo hi"})),
        MockLlmClient::text_response("Done"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig {
            permission_checker: Some(Arc::new(PermissionPolicy::new().allow("bash(echo:*)"))),
            ..Default::default()
        },
    );

    let (tx, rx) = mpsc::channel(100);
    let result = agent.execute(&[], "Say hi", Some(tx)).await.unwrap();
    assert_eq!(result.text, "Done");

    let events = collect_events(rx).await;

    // Verify event sequence
    let event_types: Vec<&str> = events
        .iter()
        .map(|e| match e {
            AgentEvent::Start { .. } => "Start",
            AgentEvent::TurnStart { .. } => "TurnStart",
            AgentEvent::TurnEnd { .. } => "TurnEnd",
            AgentEvent::ToolEnd { .. } => "ToolEnd",
            AgentEvent::End { .. } => "End",
            _ => "Other",
        })
        .collect();

    // Mode/context events may precede Start; the execution lifecycle still
    // needs a Start before turns and an End as the final event.
    let start_index = event_types
        .iter()
        .position(|t| *t == "Start")
        .expect("Start event should be present");
    let first_turn_index = event_types
        .iter()
        .position(|t| *t == "TurnStart")
        .expect("TurnStart event should be present");
    assert!(start_index < first_turn_index);
    assert_eq!(event_types.last(), Some(&"End"));

    // Must have 2 TurnStarts (tool call turn + final answer turn)
    let turn_starts = event_types.iter().filter(|&&t| t == "TurnStart").count();
    assert_eq!(turn_starts, 2);

    // Must have 1 ToolEnd
    let tool_ends = event_types.iter().filter(|&&t| t == "ToolEnd").count();
    assert_eq!(tool_ends, 1);
}

#[tokio::test]
async fn test_duplicate_tool_guard_reports_tool_end_without_stream_error() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(temp_dir.path().join("note.txt"), "small searchable fixture").unwrap();
    let workspace = temp_dir.path().to_string_lossy().to_string();
    let args = serde_json::json!({"pattern": "a3s-duplicate-guard-never-matches"});
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response("grep-1", "grep", args.clone()),
        MockLlmClient::tool_call_response("grep-2", "grep", args.clone()),
        MockLlmClient::tool_call_response("grep-3", "grep", args),
        MockLlmClient::text_response("Recovered after duplicate grep guard."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new(workspace.clone()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(PathBuf::from(workspace)),
        AgentConfig {
            permission_checker: Some(Arc::new(PermissionPolicy::new().allow("grep(*)"))),
            duplicate_tool_call_threshold: 2,
            ..Default::default()
        },
    );

    let (tx, rx) = mpsc::channel(100);
    let result = agent.execute(&[], "Repeat grep", Some(tx)).await.unwrap();
    let events = collect_events(rx).await;

    assert_eq!(result.text, "Recovered after duplicate grep guard.");
    assert!(
        !events.iter().any(|event| matches!(event, AgentEvent::Error { message } if message.contains("identical arguments"))),
        "duplicate tool guard should not emit a fatal stream error"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolEnd {
            id,
            name,
            output,
            exit_code: 1,
            metadata,
            ..
        } if id == "grep-3"
            && name == "grep"
            && output.contains("identical arguments")
            && metadata
                .as_ref()
                .and_then(|value| value.get("guard"))
                .and_then(|value| value.as_str())
                == Some("duplicate_tool_call")
    )));
}

#[tokio::test]
async fn test_agent_aborts_after_ignoring_duplicate_guard_feedback_twice() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().to_string_lossy().to_string();
    let args = serde_json::json!({"pattern": "never-matches"});
    let responses = (1..=5)
        .map(|index| {
            MockLlmClient::tool_call_response(&format!("grep-{index}"), "grep", args.clone())
        })
        .collect();
    let mock_client = Arc::new(MockLlmClient::new(responses));
    let agent = AgentLoop::new(
        mock_client.clone(),
        Arc::new(ToolExecutor::new(workspace.clone())),
        ToolContext::new(PathBuf::from(workspace)),
        AgentConfig {
            permission_checker: Some(Arc::new(PermissionPolicy::new().allow("grep(*)"))),
            duplicate_tool_call_threshold: 2,
            max_tool_rounds: 100,
            ..Default::default()
        },
    );

    let error = agent
        .execute(&[], "Repeat forever", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("failed to converge")
            || error.to_string().contains("stopping after")
    );
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn test_repeated_incomplete_text_converges_after_one_continuation() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Let me inspect the code..."),
        MockLlmClient::text_response("  LET   ME inspect the code...  "),
        MockLlmClient::text_response("This response must not be consumed."),
    ]));
    let agent = AgentLoop::new(
        mock_client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        AgentConfig {
            max_continuation_turns: 20,
            max_tool_rounds: 100,
            ..Default::default()
        },
    );

    let result = agent.execute(&[], "Inspect", None).await.unwrap();
    assert!(result.text.contains("no progress was being made"));
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_standalone_greeting_does_not_trigger_continuation() {
    let greeting = "I'll be happy to help. What would you like to work on?";
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response(greeting),
        MockLlmClient::text_response("This response must not be consumed."),
    ]));
    let agent = AgentLoop::new(
        mock_client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        AgentConfig {
            max_continuation_turns: 20,
            max_tool_rounds: 100,
            ..Default::default()
        },
    );

    let result = agent.execute(&[], "你好", None).await.unwrap();
    assert_eq!(result.text, greeting);
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_agent_multiple_tools_single_turn() {
    // LLM returns 2 tool calls in one response
    let mock_client = Arc::new(MockLlmClient::new(vec![
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo first"}),
                    },
                    ContentBlock::ToolUse {
                        id: "t2".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "echo second"}),
                    },
                ],
                reasoning_content: None,
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
        },
        MockLlmClient::text_response("Both commands ran"),
        MockLlmClient::text_response("Both commands ran"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        AgentConfig {
            prompt_slots: SystemPromptSlots {
                style: Some(AgentStyle::GeneralPurpose),
                ..Default::default()
            },
            ..Default::default()
        },
    );

    let result = agent
        .execute_loop(
            &[],
            "run both commands now",
            AgentStyle::GeneralPurpose,
            None,
            None,
            &tokio_util::sync::CancellationToken::new(),
            true,
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Both commands ran");
    assert_eq!(result.tool_calls_count, 2);
    assert!(
        mock_client.call_count.load(Ordering::SeqCst) >= 2,
        "expected at least the tool-call turn and final response turn"
    );

    // Messages: user → assistant(2 tools) → user(tool_result) → user(tool_result) → assistant(text)
    assert_eq!(result.messages[0].role, "user");
    assert_eq!(result.messages[1].role, "assistant");
    assert_eq!(result.messages[2].role, "user"); // tool result 1
    assert_eq!(result.messages[3].role, "user"); // tool result 2
    assert_eq!(result.messages[4].role, "assistant");
}

#[tokio::test]
async fn test_agent_token_usage_accumulation() {
    // Verify usage sums across multiple turns
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response("t1", "bash", serde_json::json!({"command": "echo x"})),
        MockLlmClient::text_response("Done"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let result = agent.execute(&[], "test", None).await.unwrap();

    // Each mock response has prompt=10, completion=5, total=15
    // 2 LLM calls → 20 prompt, 10 completion, 30 total
    assert_eq!(result.usage.prompt_tokens, 20);
    assert_eq!(result.usage.completion_tokens, 10);
    assert_eq!(result.usage.total_tokens, 30);
}

#[tokio::test]
async fn test_agent_system_prompt_passed() {
    // Verify system prompt is used (MockLlmClient captures calls)
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "I am a coding assistant.",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("You are a coding assistant.".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "What are you?", None).await.unwrap();

    assert_eq!(result.text, "I am a coding assistant.");
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_agent_max_rounds_with_persistent_tool_calls() {
    // LLM keeps calling tools forever — should hit max_tool_rounds
    let mut responses = Vec::new();
    for i in 0..15 {
        responses.push(MockLlmClient::tool_call_response(
            &format!("t{}", i),
            "bash",
            serde_json::json!({"command": format!("echo round{}", i)}),
        ));
    }

    let mock_client = Arc::new(MockLlmClient::new(responses));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        max_tool_rounds: 5,
        ..Default::default()
    };

    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Loop forever", None).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Max tool rounds (5) exceeded"));
}

#[tokio::test]
async fn test_agent_end_event_contains_final_text() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Final answer here",
    )]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let (tx, rx) = mpsc::channel(100);
    agent.execute(&[], "test", Some(tx)).await.unwrap();

    let events = collect_events(rx).await;
    let end_event = events.iter().find(|e| matches!(e, AgentEvent::End { .. }));
    assert!(end_event.is_some());

    if let AgentEvent::End { text, usage, .. } = end_event.unwrap() {
        assert_eq!(text, "Final answer here");
        assert_eq!(usage.total_tokens, 15);
    }
}

/// Regression: the parallel write fast path bypasses `ToolSafetyGate`, so it
/// may run only when the gate would unconditionally EXECUTE every call. Before
/// the fix it ignored the permission checker and skill restrictions entirely,
/// letting denied / ask / skill-restricted writes land ungated.
#[test]
fn parallel_write_batch_fast_path_is_only_a_data_race_check() {
    use crate::llm::ToolCall;
    use crate::permissions::{PermissionChecker, PermissionDecision};
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    struct Static(PermissionDecision);
    impl PermissionChecker for Static {
        fn check(&self, _tool: &str, _args: &serde_json::Value) -> PermissionDecision {
            self.0
        }
    }

    fn write_call(id: &str, path: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "write_file".to_string(),
            args: serde_json::json!({ "path": path, "content": "x" }),
        }
    }

    fn loop_with(
        checker: Option<Arc<dyn PermissionChecker>>,
        skills: Option<Arc<SkillRegistry>>,
        enforce_active_skill_tool_restrictions: bool,
    ) -> AgentLoop {
        // `skill_registry` overrides the default builtins where needed.
        let config = AgentConfig {
            permission_checker: checker,
            skill_registry: skills,
            enforce_active_skill_tool_restrictions,
            ..Default::default()
        };
        AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(ToolExecutor::new("/tmp".to_string())),
            test_tool_context(),
            config,
        )
    }

    let calls = vec![write_call("a", "a.txt"), write_call("b", "b.txt")];
    let allow = || Some(Arc::new(Static(PermissionDecision::Allow)) as Arc<dyn PermissionChecker>);

    assert!(loop_with(allow(), None, false).can_run_parallel_write_batch(&calls));
    assert!(loop_with(None, None, false).can_run_parallel_write_batch(&calls));
    assert!(loop_with(
        Some(Arc::new(Static(PermissionDecision::Deny))),
        None,
        false
    )
    .can_run_parallel_write_batch(&calls));
    assert!(
        loop_with(Some(Arc::new(Static(PermissionDecision::Ask))), None, false)
            .can_run_parallel_write_batch(&calls)
    );

    // By default, active skill restrictions do not block ordinary session
    // tools before the permission/HITL chain.
    let restricted = SkillRegistry::new();
    restricted.register_unchecked(Arc::new(Skill {
        name: "read-only".to_string(),
        description: String::new(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    }));
    let restricted = Arc::new(restricted);
    assert!(loop_with(allow(), Some(Arc::clone(&restricted)), false)
        .can_run_parallel_write_batch(&calls));
    assert!(loop_with(allow(), Some(restricted), true).can_run_parallel_write_batch(&calls));

    // Empty compatibility builtins do not restrict → still fast-paths with Allow.
    assert!(
        loop_with(
            allow(),
            Some(Arc::new(SkillRegistry::with_builtins())),
            false
        )
        .can_run_parallel_write_batch(&calls),
        "governance configuration does not determine data-race safety"
    );
}

mod nested_tool_governance_tests {
    use super::*;
    use crate::budget::{BudgetDecision, BudgetGuard};
    use crate::hooks::{HookEvent, HookExecutor, HookResult};
    use crate::permissions::{PermissionChecker, PermissionDecision};
    use crate::tools::{Tool, ToolContext, ToolOutput};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tokio::sync::Notify;

    struct SideEffectTool {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for SideEffectTool {
        fn name(&self) -> &str {
            "side_effect"
        }

        fn description(&self) -> &str {
            "Records a test-only side effect"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "additionalProperties": false})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("side-effect-ok"))
        }
    }

    struct DelegatedSideEffectTool {
        calls: Arc<AtomicUsize>,
    }

    struct PublicRuntimeOrchestrator;

    struct ParallelSideEffectTool {
        calls: Arc<AtomicUsize>,
    }

    struct BlockingSideEffectTool {
        calls: Arc<AtomicUsize>,
        started: Arc<Notify>,
    }

    #[async_trait]
    impl Tool for DelegatedSideEffectTool {
        fn name(&self) -> &str {
            "task"
        }

        fn description(&self) -> &str {
            "Records a delegated test-only side effect"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("delegated-side-effect-ok"))
        }
    }

    #[async_trait]
    impl Tool for PublicRuntimeOrchestrator {
        fn name(&self) -> &str {
            "public_runtime_orchestrator"
        }

        fn description(&self) -> &str {
            "Invokes a child through the public governed runtime"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "additionalProperties": false})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            let result = ctx
                .invocation_runtime()
                .invoke_tool("side_effect", serde_json::json!({}))
                .await?;
            Ok(ToolOutput::success(result.output))
        }
    }

    #[async_trait]
    impl Tool for ParallelSideEffectTool {
        fn name(&self) -> &str {
            "parallel_task"
        }

        fn description(&self) -> &str {
            "Records a test-only parallel side effect"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("parallel-side-effect-ok"))
        }
    }

    #[async_trait]
    impl Tool for BlockingSideEffectTool {
        fn name(&self) -> &str {
            "blocking_side_effect"
        }

        fn description(&self) -> &str {
            "Waits before performing a test-only side effect"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            self.started.notify_one();
            std::future::pending::<()>().await;
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("unexpected-side-effect"))
        }
    }

    struct TargetPermission {
        target: &'static str,
        decision: PermissionDecision,
    }

    impl PermissionChecker for TargetPermission {
        fn check(&self, tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            if tool_name == self.target {
                self.decision
            } else {
                PermissionDecision::Allow
            }
        }
    }

    struct TargetToolBudget {
        target: &'static str,
    }

    #[async_trait]
    impl BudgetGuard for TargetToolBudget {
        async fn check_before_tool(&self, _session_id: &str, tool_name: &str) -> BudgetDecision {
            if tool_name == self.target {
                BudgetDecision::Deny {
                    resource: "tool_calls".to_string(),
                    reason: "denied nested tool in test".to_string(),
                }
            } else {
                BudgetDecision::Allow
            }
        }
    }

    #[derive(Debug, Default)]
    struct RecordingToolHooks {
        events: Mutex<Vec<HookEvent>>,
    }

    #[async_trait]
    impl HookExecutor for RecordingToolHooks {
        async fn fire(&self, event: &HookEvent) -> HookResult {
            self.events.lock().unwrap().push(event.clone());
            HookResult::Continue(None)
        }
    }

    fn governed_agent(
        responses: Vec<crate::llm::LlmResponse>,
        calls: Arc<AtomicUsize>,
        config: AgentConfig,
    ) -> (AgentLoop, Arc<MockLlmClient>) {
        let client = Arc::new(MockLlmClient::new(responses));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        executor.register_dynamic_tool(Arc::new(SideEffectTool { calls }));
        (
            AgentLoop::new(client.clone(), executor, test_tool_context(), config),
            client,
        )
    }

    #[tokio::test]
    async fn cancelling_one_invocation_settles_only_its_confirmation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        executor.register_dynamic_tool(Arc::new(SideEffectTool {
            calls: Arc::clone(&calls),
        }));
        let (confirmation_events, _) = tokio::sync::broadcast::channel(8);
        let confirmations = Arc::new(crate::hitl::ConfirmationManager::new(
            crate::hitl::ConfirmationPolicy::enabled()
                .with_timeout(5_000, crate::hitl::TimeoutAction::Reject),
            confirmation_events,
        ));
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            executor,
            test_tool_context(),
            AgentConfig {
                permission_checker: Some(Arc::new(TargetPermission {
                    target: "side_effect",
                    decision: PermissionDecision::Ask,
                })),
                confirmation_manager: Some(confirmations.clone()),
                ..Default::default()
            },
        );
        let invoker = agent.scoped_tool_invoker(Some("confirmation-isolation"), &None);
        let cancellation_a = tokio_util::sync::CancellationToken::new();
        let cancellation_b = tokio_util::sync::CancellationToken::new();
        let invocation_a = crate::tools::ToolInvocation::agent(
            "confirmation-a",
            "side_effect",
            serde_json::json!({}),
            Vec::new(),
        );
        let invocation_b = crate::tools::ToolInvocation::agent(
            "confirmation-b",
            "side_effect",
            serde_json::json!({}),
            Vec::new(),
        );
        let invoker_a = Arc::clone(&invoker);
        let context_a = test_tool_context().with_cancellation(cancellation_a.clone());
        let run_a = tokio::spawn(async move { invoker_a.invoke(invocation_a, &context_a).await });
        let invoker_b = Arc::clone(&invoker);
        let context_b = test_tool_context().with_cancellation(cancellation_b);
        let run_b = tokio::spawn(async move { invoker_b.invoke(invocation_b, &context_b).await });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while confirmations.pending_count().await != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both confirmation requests must become pending");

        cancellation_a.cancel();
        let result_a = run_a.await.unwrap();
        assert_ne!(result_a.exit_code, 0);
        assert!(result_a.output.contains("cancelled by caller"));
        let pending = confirmations.pending_confirmations().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "confirmation-b");

        confirmations
            .confirm("confirmation-b", true, None)
            .await
            .unwrap();
        let result_b = run_b.await.unwrap();
        assert_eq!(result_b.exit_code, 0, "{}", result_b.output);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn trusted_host_batch_and_program_propagate_only_builtin_nested_authority() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        executor.register_dynamic_tool(Arc::new(SideEffectTool {
            calls: Arc::clone(&calls),
        }));
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            executor,
            test_tool_context(),
            AgentConfig {
                permission_checker: Some(Arc::new(TargetPermission {
                    target: "side_effect",
                    decision: PermissionDecision::Deny,
                })),
                ..Default::default()
            },
        );
        let cancellation = tokio_util::sync::CancellationToken::new();
        let context = test_tool_context();
        let calls_to_run = [
            (
                "batch",
                serde_json::json!({
                    "invocations": [{"tool": "side_effect", "args": {}}]
                }),
            ),
            (
                "program",
                serde_json::json!({
                    "type": "script",
                    "language": "javascript",
                    "source": "async function run(ctx) { return await ctx.tool('side_effect', {}); }",
                    "allowed_tools": ["side_effect"]
                }),
            ),
        ];

        for (index, (name, args)) in calls_to_run.into_iter().enumerate() {
            let result = agent
                .invoke_host_tool(
                    crate::tools::ToolInvocation::host_direct(
                        format!("host-orchestrator-{index}"),
                        name,
                        args,
                    ),
                    "host-orchestrator",
                    &None,
                    &cancellation,
                    &context,
                )
                .await;
            assert_eq!(result.exit_code, 0, "{name}: {}", result.output);
            assert!(result.output.contains("side-effect-ok"), "{name}");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn public_runtime_cannot_amplify_a_host_direct_custom_tool_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        executor.register_dynamic_tool(Arc::new(SideEffectTool {
            calls: Arc::clone(&calls),
        }));
        executor.register_dynamic_tool(Arc::new(PublicRuntimeOrchestrator));
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            executor,
            test_tool_context(),
            AgentConfig {
                permission_checker: Some(Arc::new(TargetPermission {
                    target: "side_effect",
                    decision: PermissionDecision::Deny,
                })),
                ..Default::default()
            },
        );
        let result = agent
            .invoke_host_tool(
                crate::tools::ToolInvocation::host_direct(
                    "host-custom",
                    "public_runtime_orchestrator",
                    serde_json::json!({}),
                ),
                "host-custom",
                &None,
                &tokio_util::sync::CancellationToken::new(),
                &test_tool_context(),
            )
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(
            result.output.contains("Permission denied"),
            "{}",
            result.output
        );
    }

    #[tokio::test]
    async fn batch_nested_tool_obeys_permission_deny_without_side_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let responses = vec![
            MockLlmClient::tool_call_response(
                "batch-1",
                "batch",
                serde_json::json!({
                    "invocations": [{"tool": "side_effect", "args": {}}]
                }),
            ),
            MockLlmClient::text_response("done"),
        ];
        let config = AgentConfig {
            permission_checker: Some(Arc::new(TargetPermission {
                target: "side_effect",
                decision: PermissionDecision::Deny,
            })),
            ..Default::default()
        };
        let (agent, _) = governed_agent(responses, Arc::clone(&calls), config);

        agent
            .execute_with_session(&[], "run batch", Some("nested-permission"), None, None)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn program_nested_tool_obeys_budget_deny_without_side_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let responses = vec![
            MockLlmClient::tool_call_response(
                "program-1",
                "program",
                serde_json::json!({
                    "type": "script",
                    "source": "async function run(ctx) { return await ctx.tool('side_effect', {}); }",
                    "allowed_tools": ["side_effect"]
                }),
            ),
            MockLlmClient::text_response("done"),
        ];
        let config = AgentConfig {
            permission_checker: Some(Arc::new(TargetPermission {
                target: "side_effect",
                decision: PermissionDecision::Allow,
            })),
            budget_guard: Some(Arc::new(TargetToolBudget {
                target: "side_effect",
            })),
            ..Default::default()
        };
        let (agent, _) = governed_agent(responses, Arc::clone(&calls), config);

        agent
            .execute_with_session(&[], "run program", Some("nested-budget"), None, None)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn delegated_tool_obeys_permission_deny_without_side_effects() {
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        executor.register_dynamic_tool(Arc::new(DelegatedSideEffectTool {
            calls: Arc::clone(&calls),
        }));
        let config = AgentConfig {
            permission_checker: Some(Arc::new(TargetPermission {
                target: "task",
                decision: PermissionDecision::Deny,
            })),
            ..Default::default()
        };
        let agent = AgentLoop::new(
            Arc::new(MockLlmClient::new(vec![])),
            executor,
            test_tool_context(),
            config,
        );

        let (output, exit_code, is_error, _) = agent
            .execute_delegated_plan_tool(
                "task",
                &serde_json::json!({"prompt": "probe"}),
                Some("delegated-permission"),
                &None,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(exit_code, 1);
        assert!(is_error);
        assert!(output.contains("Permission denied"));
    }

    #[tokio::test]
    async fn batch_nested_tool_fires_pre_and_post_hooks_and_preserves_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(RecordingToolHooks::default());
        let responses = vec![
            MockLlmClient::tool_call_response(
                "batch-allowed",
                "batch",
                serde_json::json!({
                    "invocations": [{"tool": "side_effect", "args": {}}]
                }),
            ),
            MockLlmClient::text_response("done"),
        ];
        let config = AgentConfig {
            permission_checker: Some(Arc::new(TargetPermission {
                target: "side_effect",
                decision: PermissionDecision::Allow,
            })),
            hook_engine: Some(hooks.clone()),
            ..Default::default()
        };
        let (agent, _) = governed_agent(responses, Arc::clone(&calls), config);

        agent
            .execute_with_session(&[], "run batch", Some("nested-hooks"), None, None)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = hooks.events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            HookEvent::PreToolUse(pre) if pre.tool == "side_effect"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HookEvent::PostToolUse(post)
                if post.tool == "side_effect" && post.result.output == "side-effect-ok"
        )));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dynamic_workflow_parallel_step_obeys_permission_without_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let responses = vec![
            MockLlmClient::tool_call_response(
                "dynamic-permission",
                "dynamic_workflow",
                serde_json::json!({
                    "source": r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const completed = inputs.step_outputs.fanout;
    if (completed) return { type: "complete", output: completed };
    return {
      type: "schedule_step",
      step_id: "fanout",
      step_name: "parallel_task",
      input: { tasks: [{ prompt: "must not run" }] },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }
  return { type: "fail", error: "unexpected script step" };
}
"#,
                    "run_id": format!("permission-{}", uuid::Uuid::new_v4()),
                    "allowed_tools": []
                }),
            ),
            MockLlmClient::text_response("done"),
        ];
        let client = Arc::new(MockLlmClient::new(responses));
        let executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        executor.register_dynamic_tool(Arc::new(ParallelSideEffectTool {
            calls: Arc::clone(&calls),
        }));
        crate::dynamic_workflow::register_dynamic_workflow(executor.registry());
        let agent = AgentLoop::new(
            client,
            executor,
            ToolContext::new(dir.path().to_path_buf()),
            AgentConfig {
                permission_checker: Some(Arc::new(TargetPermission {
                    target: "parallel_task",
                    decision: PermissionDecision::Deny,
                })),
                ..Default::default()
            },
        );

        agent
            .execute_with_session(&[], "run workflow", Some("dynamic-permission"), None, None)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dynamic_workflow_script_step_obeys_budget_without_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let responses = vec![
            MockLlmClient::tool_call_response(
                "dynamic-budget",
                "dynamic_workflow",
                serde_json::json!({
                    "source": r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const completed = inputs.step_outputs.governed_step;
    if (completed) return { type: "complete", output: completed };
    return {
      type: "schedule_step",
      step_id: "governed_step",
      step_name: "governed_step",
      input: {},
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }
  const result = await ctx.tool("side_effect", {});
  return { result };
}
"#,
                    "run_id": format!("budget-{}", uuid::Uuid::new_v4()),
                    "allowed_tools": ["side_effect"]
                }),
            ),
            MockLlmClient::text_response("done"),
        ];
        let client = Arc::new(MockLlmClient::new(responses));
        let executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        executor.register_dynamic_tool(Arc::new(SideEffectTool {
            calls: Arc::clone(&calls),
        }));
        crate::dynamic_workflow::register_dynamic_workflow(executor.registry());
        let agent = AgentLoop::new(
            client,
            executor,
            ToolContext::new(dir.path().to_path_buf()),
            AgentConfig {
                permission_checker: Some(Arc::new(TargetPermission {
                    target: "side_effect",
                    decision: PermissionDecision::Allow,
                })),
                budget_guard: Some(Arc::new(TargetToolBudget {
                    target: "side_effect",
                })),
                ..Default::default()
            },
        );

        agent
            .execute_with_session(&[], "run workflow", Some("dynamic-budget"), None, None)
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dynamic_workflow_nested_cancellation_prevents_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let responses = vec![MockLlmClient::tool_call_response(
            "dynamic-cancel",
            "dynamic_workflow",
            serde_json::json!({
                "source": r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const result = await ctx.tool("blocking_side_effect", {});
    return { type: "complete", output: result };
  }
  return { type: "fail", error: "unexpected script step" };
}
"#,
                "run_id": format!("cancel-{}", uuid::Uuid::new_v4()),
                "allowed_tools": ["blocking_side_effect"]
            }),
        )];
        let client = Arc::new(MockLlmClient::new(responses));
        let executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        executor.register_dynamic_tool(Arc::new(BlockingSideEffectTool {
            calls: Arc::clone(&calls),
            started: Arc::clone(&started),
        }));
        crate::dynamic_workflow::register_dynamic_workflow(executor.registry());
        let agent = AgentLoop::new(
            client,
            executor,
            ToolContext::new(dir.path().to_path_buf()),
            AgentConfig {
                permission_checker: Some(Arc::new(TargetPermission {
                    target: "blocking_side_effect",
                    decision: PermissionDecision::Allow,
                })),
                ..Default::default()
            },
        );
        let cancellation = tokio_util::sync::CancellationToken::new();
        let run_cancellation = cancellation.clone();
        let run = tokio::spawn(async move {
            agent
                .execute_with_session(
                    &[],
                    "run workflow",
                    Some("dynamic-cancel"),
                    None,
                    Some(&run_cancellation),
                )
                .await
        });

        // This deadline protects test setup only. QuickJS initialization can
        // take several seconds on a loaded Windows runner; cancellation latency
        // remains independently bounded below.
        tokio::time::timeout(std::time::Duration::from_secs(10), started.notified())
            .await
            .expect("nested tool must start before cancellation");
        cancellation.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancellation must stop the dynamic workflow")
            .unwrap()
            .expect("the agent loop should preserve an interrupted result");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(result.text.is_empty());
        assert!(result
            .messages
            .last()
            .is_some_and(|message| message.text().contains("interrupted")));
    }
}
