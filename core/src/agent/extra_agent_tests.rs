use super::*;
use crate::agent::tests::MockLlmClient;
use crate::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::queue::SessionQueueConfig;
use crate::tools::ToolExecutor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

fn test_tool_context() -> ToolContext {
    ToolContext::new(PathBuf::from("/tmp"))
}

struct AllowDelegatedTools;

impl crate::permissions::PermissionChecker for AllowDelegatedTools {
    fn check(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
    ) -> crate::permissions::PermissionDecision {
        crate::permissions::PermissionDecision::Allow
    }
}

fn allow_delegated_tools(mut config: AgentConfig) -> AgentConfig {
    config.permission_checker = Some(Arc::new(AllowDelegatedTools));
    config
}

struct BlockingPreAnalysisClient {
    calls: AtomicUsize,
    pre_analysis_started: tokio::sync::Notify,
}

impl BlockingPreAnalysisClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            pre_analysis_started: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for BlockingPreAnalysisClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM)) {
            self.pre_analysis_started.notify_one();
            return std::future::pending().await;
        }
        Ok(MockLlmClient::text_response(
            "execution must not start after cancellation",
        ))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("execution must not start after cancellation")
    }
}

#[derive(Default)]
struct DenyPlanningBudgetGuard {
    checks: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for DenyPlanningBudgetGuard {
    async fn check_before_llm(
        &self,
        _session_id: &str,
        _estimated_prompt_tokens: usize,
    ) -> crate::budget::BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        crate::budget::BudgetDecision::Deny {
            resource: "planning_tokens".to_string(),
            reason: "planning budget denied in test".to_string(),
        }
    }
}

struct CountingPlanningClient {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for CountingPlanningClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("budget guard should have denied before the client was called")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("budget guard should have denied before the client was called")
    }
}

#[derive(Default)]
struct CountingAllowPlanningBudgetGuard {
    checks: AtomicUsize,
    records: AtomicUsize,
    recorded_tokens: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for CountingAllowPlanningBudgetGuard {
    async fn check_before_llm(
        &self,
        _session_id: &str,
        _estimated_prompt_tokens: usize,
    ) -> crate::budget::BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        crate::budget::BudgetDecision::Allow
    }

    async fn record_after_llm(&self, _session_id: &str, usage: &crate::llm::TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
        self.recorded_tokens
            .fetch_add(usage.total_tokens, Ordering::SeqCst);
    }
}

struct RepairingPreAnalysisClient {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for RepairingPreAnalysisClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            anyhow::bail!("no repair response available");
        }
        Ok(responses.remove(0))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by pre-analysis")
    }
}

struct CancellablePlanStepClient {
    calls: AtomicUsize,
    step_started: tokio::sync::Notify,
}

impl CancellablePlanStepClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            step_started: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for CancellablePlanStepClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.step_started.notify_one();
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.step_started.notify_one();
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            drop(tx);
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn pre_analysis_stops_on_parent_cancellation_before_execution() {
    let client = Arc::new(BlockingPreAnalysisClient::new());
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        AgentConfig::default(),
    );
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let run_token = cancel_token.clone();
    let run = tokio::spawn(async move {
        agent
            .execute_with_session(
                &[],
                "analyze this request",
                Some("planning-cancel"),
                None,
                Some(&run_token),
            )
            .await
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        client.pre_analysis_started.notified(),
    )
    .await
    .expect("pre-analysis should start");
    cancel_token.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("parent cancellation should stop pre-analysis")
        .expect("run task should not panic");
    assert!(
        result.is_err(),
        "cancelled routing should terminate the run"
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        1,
        "execution must not start after pre-analysis is cancelled"
    );
}

#[tokio::test]
async fn pre_analysis_budget_deny_skips_the_llm_client() {
    let client = Arc::new(CountingPlanningClient {
        calls: AtomicUsize::new(0),
    });
    let budget = Arc::new(DenyPlanningBudgetGuard::default());
    let config = AgentConfig {
        budget_guard: Some(budget.clone()),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        config,
    );

    let error = agent
        .execute_with_session(
            &[],
            "analyze this request",
            Some("planning-budget-deny"),
            None,
            None,
        )
        .await
        .expect_err("budget denial should terminate routing");

    let error_chain = format!("{error:#}");
    assert!(
        error_chain.contains("planning_tokens"),
        "unexpected error: {error_chain}"
    );
    assert_eq!(budget.checks.load(Ordering::SeqCst), 1);
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pre_analysis_repair_checks_and_records_each_llm_call() {
    let valid_pre_analysis = serde_json::json!({
        "intent": "plan",
        "requires_planning": true,
        "goal": {
            "description": "Repair planning output",
            "success_criteria": []
        },
        "execution_plan": {
            "complexity": "Simple",
            "steps": [],
            "required_tools": []
        },
        "optimized_input": "Repair planning output"
    });
    let client = Arc::new(RepairingPreAnalysisClient {
        responses: std::sync::Mutex::new(vec![
            MockLlmClient::text_response("not valid JSON"),
            MockLlmClient::text_response(&valid_pre_analysis.to_string()),
        ]),
        calls: AtomicUsize::new(0),
    });
    let budget = Arc::new(CountingAllowPlanningBudgetGuard::default());
    let config = AgentConfig {
        budget_guard: Some(budget.clone()),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        config,
    );

    agent
        .execute_with_session(
            &[],
            "Repair planning output",
            Some("planning-repair-budget"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(client.calls.load(Ordering::SeqCst), 2);
    assert_eq!(budget.checks.load(Ordering::SeqCst), 2);
    assert_eq!(budget.records.load(Ordering::SeqCst), 2);
    assert_eq!(budget.recorded_tokens.load(Ordering::SeqCst), 30);
}

#[tokio::test]
async fn serial_plan_steps_stop_starting_llm_calls_after_parent_cancellation() {
    use crate::planning::{Complexity, ExecutionPlan, Task};

    let client = Arc::new(CancellablePlanStepClient::new());
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        AgentConfig::default(),
    );
    let mut plan = ExecutionPlan::new("serial cancellation", Complexity::Simple);
    plan.add_step(Task::new("s1", "First serial step"));
    plan.add_step(Task::new("s2", "Second serial step").with_dependencies(vec!["s1".into()]));
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let run_token = cancel_token.clone();
    let run = tokio::spawn(async move {
        agent
            .execute_plan(&[], &plan, Some("serial-cancel"), None, &run_token)
            .await
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        client.step_started.notified(),
    )
    .await
    .expect("first serial step should start");
    cancel_token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("serial plan should stop on cancellation")
        .expect("serial plan task should not panic")
        .expect("serial plan should return its interrupted result");

    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn parallel_plan_steps_stop_starting_llm_calls_after_parent_cancellation() {
    use crate::planning::{Complexity, ExecutionPlan, Task};

    let client = Arc::new(CancellablePlanStepClient::new());
    let config = AgentConfig {
        max_parallel_tasks: 1,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        config,
    );
    let mut plan = ExecutionPlan::new("parallel cancellation", Complexity::Simple);
    plan.add_step(Task::new("s1", "First parallel step"));
    plan.add_step(Task::new("s2", "Second parallel step"));
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let run_token = cancel_token.clone();
    let run = tokio::spawn(async move {
        agent
            .execute_plan(&[], &plan, Some("parallel-cancel"), None, &run_token)
            .await
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        client.step_started.notified(),
    )
    .await
    .expect("first parallel step should start");
    cancel_token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("parallel plan should stop on cancellation")
        .expect("parallel plan task should not panic")
        .expect("parallel plan should return its interrupted result");

    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
}

struct PlanDelegationChildClient;

impl PlanDelegationChildClient {
    fn message_text(messages: &[Message]) -> String {
        messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn pre_analysis_response(messages: &[Message]) -> LlmResponse {
        let prompt = pre_analysis_user_request(&Self::message_text(messages));
        let response = serde_json::json!({
            "intent": "GeneralPurpose",
            "requires_planning": false,
            "goal": {
                "description": prompt,
                "success_criteria": []
            },
            "execution_plan": {
                "complexity": "Simple",
                "steps": [{
                    "id": "step-1",
                    "description": prompt,
                    "dependencies": [],
                    "success_criteria": "Complete the request"
                }],
                "required_tools": []
            },
            "optimized_input": prompt
        });
        MockLlmClient::text_response(&response.to_string())
    }

    fn routed_response(messages: &[Message]) -> LlmResponse {
        let prompt = Self::message_text(messages).to_lowercase();
        let text = if prompt.contains("find the relevant docs") {
            "delegated search complete"
        } else if prompt.contains("auth") {
            "auth exploration complete"
        } else if prompt.contains("find documentation") {
            "docs exploration complete"
        } else if prompt.contains("verification")
            || prompt.contains("tests")
            || prompt.contains("checks")
        {
            "delegated tests complete"
        } else if prompt.contains("docs") || prompt.contains("documentation") {
            "delegated docs complete"
        } else {
            "delegated child complete"
        };
        MockLlmClient::text_response(text)
    }
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

#[async_trait::async_trait]
impl LlmClient for PlanDelegationChildClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM)) {
            return Ok(Self::pre_analysis_response(messages));
        }
        Ok(Self::routed_response(messages))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = Self::routed_response(messages);
        let text = response.text();
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            if !text.is_empty() {
                let _ = tx.send(StreamEvent::TextDelta(text)).await;
            }
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

// ========================================================================
// AgentConfig
// ========================================================================

#[test]
fn test_agent_config_debug() {
    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("You are helpful".to_string()),
            ..Default::default()
        },
        tools: vec![],
        max_tool_rounds: 10,
        permission_checker: None,
        confirmation_manager: None,
        context_providers: vec![],
        planning_mode: PlanningMode::Enabled,
        goal_tracking: false,
        hook_engine: None,
        skill_registry: None,
        ..AgentConfig::default()
    };
    let debug = format!("{:?}", config);
    assert!(debug.contains("AgentConfig"));
    assert!(debug.contains("planning_mode"));
}

#[test]
fn test_agent_config_default_values() {
    let config = AgentConfig::default();
    assert_eq!(config.max_tool_rounds, MAX_TOOL_ROUNDS);
    assert_eq!(config.max_parallel_tasks, DEFAULT_MAX_PARALLEL_TASKS);
    assert_eq!(config.planning_mode, PlanningMode::Auto);
    assert!(!config.goal_tracking);
    assert!(config.context_providers.is_empty());
}

#[test]
fn test_auto_pre_analysis_runs_without_keyword_gate() {
    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    assert!(agent.should_run_pre_analysis());
}

#[test]
fn test_disabled_planning_never_runs_pre_analysis() {
    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    assert!(!agent.should_run_pre_analysis());
}

#[derive(Debug)]
struct PlanningHookRecorder {
    events: Arc<std::sync::Mutex<Vec<crate::hooks::HookEvent>>>,
    result: crate::hooks::HookResult,
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for PlanningHookRecorder {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        self.events.lock().unwrap().push(event.clone());
        self.result.clone()
    }
}

fn planning_hook_recorder(
    result: crate::hooks::HookResult,
) -> (
    Arc<PlanningHookRecorder>,
    Arc<std::sync::Mutex<Vec<crate::hooks::HookEvent>>>,
) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    (
        Arc::new(PlanningHookRecorder {
            events: Arc::clone(&events),
            result,
        }),
        events,
    )
}

// ========================================================================
// Planning hooks
// ========================================================================

#[tokio::test]
async fn test_execute_with_planning_fires_pre_and_post_planning_hooks() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response(
            r#"{
                "goal": "Build planning hooks",
                "complexity": "Simple",
                "steps": [
                    {
                        "id": "step-1",
                        "description": "Wire planning hooks into the runtime",
                        "dependencies": [],
                        "success_criteria": "Pre and post hooks are emitted"
                    }
                ],
                "required_tools": []
            }"#,
        ),
        MockLlmClient::text_response("Planning hooks wired."),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let (hook, events) = planning_hook_recorder(crate::hooks::HookResult::Continue(None));
    let config = AgentConfig {
        hook_engine: Some(hook),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let result = agent
        .execute_with_planning(
            &[],
            "Build planning hooks",
            Some("planning-hooks-session"),
            None,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Planning hooks wired.");

    let planning_events = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            crate::hooks::HookEvent::PrePlanning(event) => Some((
                "pre",
                event.session_id.clone(),
                event.task_description.clone(),
                None,
            )),
            crate::hooks::HookEvent::PostPlanning(event) => Some((
                "post",
                event.session_id.clone(),
                event.task_description.clone(),
                Some((event.success, event.subtasks.clone())),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(planning_events.len(), 2);
    assert_eq!(planning_events[0].0, "pre");
    assert_eq!(planning_events[0].1, "planning-hooks-session");
    assert_eq!(planning_events[0].2, "Build planning hooks");
    assert_eq!(planning_events[1].0, "post");
    assert_eq!(planning_events[1].1, "planning-hooks-session");
    assert_eq!(planning_events[1].2, "Build planning hooks");
    assert_eq!(
        planning_events[1].3.as_ref().unwrap().1,
        vec!["Wire planning hooks into the runtime".to_string()]
    );
    assert!(planning_events[1].3.as_ref().unwrap().0);
}

#[tokio::test]
async fn goal_achievement_is_emitted_before_terminal_end() {
    use crate::planning::{AgentGoal, Complexity, ExecutionPlan, PreAnalysis, Task};
    use crate::prompts::{AgentStyle, PlanningMode};

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response(
            "Created goal_probe.txt and verified its exact 25-byte contents with cmp; exit code 0.",
        ),
        MockLlmClient::text_response(r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig {
            planning_mode: PlanningMode::Enabled,
            goal_tracking: true,
            ..AgentConfig::default()
        },
    );

    let goal = AgentGoal::new("Ship the verified goal").with_criteria(vec![
        "Implementation and verification are complete".to_string(),
    ]);
    let mut plan = ExecutionPlan::new(goal.description.clone(), Complexity::Simple);
    plan.add_step(
        Task::new(
            "implement-and-verify",
            "Implement and verify the requested goal",
        )
        .with_success_criteria("Implementation and verification are complete"),
    );
    let pre_analysis = PreAnalysis {
        intent: AgentStyle::GeneralPurpose,
        requires_planning: true,
        goal,
        execution_plan: plan,
        optimized_input: "Ship the verified goal".to_string(),
    };

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_with_planning(
            &[],
            "Ship the verified goal",
            Some("goal-event-order"),
            Some(tx),
            Some(pre_analysis),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.text,
        "Created goal_probe.txt and verified its exact 25-byte contents with cmp; exit code 0."
    );

    let mut events = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    let achieved = events
        .iter()
        .position(|event| matches!(event, AgentEvent::GoalAchieved { .. }))
        .expect("GoalAchieved should be emitted for a verified goal");
    let end = events
        .iter()
        .position(|event| matches!(event, AgentEvent::End { .. }))
        .expect("End should be emitted after goal evaluation");

    assert!(
        achieved < end,
        "GoalAchieved must precede terminal End: {events:?}"
    );
    assert!(matches!(events.last(), Some(AgentEvent::End { .. })));
}

#[tokio::test]
async fn unachieved_goal_emits_terminal_end_without_false_achievement() {
    use crate::planning::{AgentGoal, Complexity, ExecutionPlan, PreAnalysis, Task};
    use crate::prompts::{AgentStyle, PlanningMode};

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Done. Everything passes."),
        MockLlmClient::text_response(
            r#"{"achieved":false,"progress":0.6,"remaining_criteria":["verification"]}"#,
        ),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig {
            planning_mode: PlanningMode::Enabled,
            goal_tracking: true,
            ..AgentConfig::default()
        },
    );
    let goal = AgentGoal::new("Ship only after verification")
        .with_criteria(vec!["Fresh verification passes".to_string()]);
    let mut plan = ExecutionPlan::new(goal.description.clone(), Complexity::Simple);
    plan.add_step(
        Task::new("implement", "Implement the requested behavior")
            .with_success_criteria("Implementation exists"),
    );
    let pre_analysis = PreAnalysis {
        intent: AgentStyle::GeneralPurpose,
        requires_planning: true,
        goal,
        execution_plan: plan,
        optimized_input: "Ship only after verification".to_string(),
    };

    let (tx, mut rx) = mpsc::channel(100);
    agent
        .execute_with_planning(
            &[],
            "Ship only after verification",
            Some("goal-not-achieved"),
            Some(tx),
            Some(pre_analysis),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut events = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert!(events
        .iter()
        .all(|event| !matches!(event, AgentEvent::GoalAchieved { .. })));
    assert!(matches!(events.last(), Some(AgentEvent::End { .. })));
}

#[tokio::test]
async fn test_pre_planning_hook_can_block_planning_before_start_event() {
    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let (hook, events) =
        planning_hook_recorder(crate::hooks::HookResult::block("policy denied planning"));
    let config = AgentConfig {
        hook_engine: Some(hook),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let (tx, mut rx) = mpsc::channel(100);
    let error = agent
        .execute_with_planning(
            &[],
            "Plan a blocked change",
            Some("blocked-planning-session"),
            Some(tx),
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("Planning blocked by hook: policy denied planning"));

    {
        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(matches!(
            recorded.first().unwrap(),
            crate::hooks::HookEvent::PrePlanning(event)
                if event.session_id == "blocked-planning-session"
                    && event.task_description == "Plan a blocked change"
        ));
    }

    rx.close();
    while let Some(event) = rx.recv().await {
        assert!(
            !matches!(event, AgentEvent::PlanningStart { .. }),
            "PlanningStart must not be emitted after a blocking PrePlanning hook"
        );
    }
}

#[tokio::test]
async fn test_pre_planning_hook_modification_updates_planner_input() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response(
            r#"{
                "goal": "Implement the policy-refined planning task",
                "complexity": "Simple",
                "steps": [
                    {
                        "id": "step-1",
                        "description": "Follow the hook-modified task",
                        "dependencies": [],
                        "success_criteria": "The modified planning task is used"
                    }
                ],
                "required_tools": []
            }"#,
        ),
        MockLlmClient::text_response("Modified planning complete."),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let (hook, events) =
        planning_hook_recorder(crate::hooks::HookResult::continue_with(serde_json::json!({
            "modified_task": "Use the hook-modified planning task",
            "selected_strategy": "step_by_step",
            "planning_template": "Break the work into observable, testable steps.",
            "hints": ["Preserve the original user request", "Keep changes scoped"]
        })));
    let config = AgentConfig {
        hook_engine: Some(hook),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_with_planning(
            &[],
            "Original planning request",
            Some("modified-planning-session"),
            Some(tx),
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Modified planning complete.");

    let request_texts = mock_client.request_texts.lock().unwrap().clone();
    assert!(
        request_texts[0].contains("Original user request:\nOriginal planning request"),
        "planner input should preserve original request: {}",
        request_texts[0]
    );
    assert!(
        request_texts[0]
            .contains("Hook-modified planning task:\nUse the hook-modified planning task"),
        "planner input should include hook-modified task: {}",
        request_texts[0]
    );
    assert!(
        request_texts[0].contains("Break the work into observable, testable steps."),
        "planner input should include hook planning template: {}",
        request_texts[0]
    );
    assert!(
        request_texts[0].contains("- Preserve the original user request"),
        "planner input should include hook hints: {}",
        request_texts[0]
    );

    let mut planning_start_prompt = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::PlanningStart { prompt } = event {
            planning_start_prompt = Some(prompt);
            break;
        }
    }
    let planning_start_prompt = planning_start_prompt.expect("PlanningStart should be emitted");
    assert!(planning_start_prompt.contains("Hook-modified planning task"));

    let post_planning_task = events
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            crate::hooks::HookEvent::PostPlanning(event) => Some(event.task_description.clone()),
            _ => None,
        })
        .expect("PostPlanning should be emitted");
    assert!(post_planning_task.contains("Hook-modified planning task"));
}

#[tokio::test]
async fn test_pre_planning_hook_modification_discards_pre_analysis_plan() {
    use crate::planning::{AgentGoal, Complexity, ExecutionPlan, PreAnalysis, Task};
    use crate::prompts::AgentStyle;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response(
            r#"{
                "goal": "Use the hook-modified plan",
                "complexity": "Simple",
                "steps": [
                    {
                        "id": "step-1",
                        "description": "Execute the hook-modified plan",
                        "dependencies": [],
                        "success_criteria": "The pre-analysis plan is not reused"
                    }
                ],
                "required_tools": []
            }"#,
        ),
        MockLlmClient::text_response("Hook-modified plan executed."),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let (hook, events) =
        planning_hook_recorder(crate::hooks::HookResult::continue_with(serde_json::json!({
            "modified_task": "Use the hook-modified plan instead"
        })));
    let config = AgentConfig {
        hook_engine: Some(hook),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );

    let mut stale_plan = ExecutionPlan::new("Stale pre-analysis plan", Complexity::Simple);
    stale_plan.add_step(Task::new(
        "stale-step",
        "This stale pre-analysis step must not run",
    ));
    let pre_analysis = PreAnalysis {
        intent: AgentStyle::GeneralPurpose,
        requires_planning: true,
        goal: AgentGoal::new("Stale pre-analysis goal"),
        execution_plan: stale_plan,
        optimized_input: "Stale optimized input".to_string(),
    };

    let result = agent
        .execute_with_planning(
            &[],
            "Original request with stale pre-analysis",
            Some("discard-pre-analysis-session"),
            None,
            Some(pre_analysis),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.text, "Hook-modified plan executed.");

    let request_texts = mock_client.request_texts.lock().unwrap().clone();
    assert!(
        request_texts[0]
            .contains("Hook-modified planning task:\nUse the hook-modified plan instead"),
        "planner input should be regenerated from the hook-modified task: {}",
        request_texts[0]
    );
    assert!(
        !request_texts
            .iter()
            .any(|text| text.contains("This stale pre-analysis step must not run")),
        "stale pre-analysis step should not be sent to execution: {:?}",
        request_texts
    );

    let post_planning_subtasks = events
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            crate::hooks::HookEvent::PostPlanning(event) => Some(event.subtasks.clone()),
            _ => None,
        })
        .expect("PostPlanning should be emitted");
    assert_eq!(
        post_planning_subtasks,
        vec!["Execute the hook-modified plan".to_string()]
    );
}

// ========================================================================
// AgentEvent serialization
// ========================================================================

#[test]
fn test_agent_event_serialize_start() {
    let event = AgentEvent::Start {
        prompt: "Hello".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("agent_start"));
    assert!(json.contains("Hello"));
}

#[test]
fn test_agent_event_serialize_text_delta() {
    let event = AgentEvent::TextDelta {
        text: "chunk".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("text_delta"));
}

#[test]
fn test_agent_event_serialize_tool_start() {
    let event = AgentEvent::ToolStart {
        id: "t1".to_string(),
        name: "bash".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("tool_start"));
    assert!(json.contains("bash"));
}

#[test]
fn test_agent_event_serialize_tool_end() {
    let event = AgentEvent::ToolEnd {
        id: "t1".to_string(),
        name: "bash".to_string(),
        args: None,
        output: "hello".to_string(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("tool_end"));
}

#[test]
fn test_agent_event_tool_end_has_metadata_field() {
    let event = AgentEvent::ToolEnd {
        id: "t1".to_string(),
        name: "write".to_string(),
        args: None,
        output: "Wrote 5 bytes".to_string(),
        exit_code: 0,
        metadata: Some(
            serde_json::json!({ "before": "old", "after": "new", "file_path": "f.txt" }),
        ),
        error_kind: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"before\""));
}

#[test]
fn test_agent_event_serialize_error() {
    let event = AgentEvent::Error {
        message: "oops".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("error"));
    assert!(json.contains("oops"));
}

#[test]
fn test_agent_event_serialize_confirmation_required() {
    let event = AgentEvent::ConfirmationRequired {
        tool_id: "t1".to_string(),
        tool_name: "bash".to_string(),
        args: serde_json::json!({"cmd": "rm"}),
        timeout_ms: 30000,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("confirmation_required"));
}

#[test]
fn test_agent_event_serialize_confirmation_received() {
    let event = AgentEvent::ConfirmationReceived {
        tool_id: "t1".to_string(),
        approved: true,
        reason: Some("safe".to_string()),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("confirmation_received"));
}

#[test]
fn test_agent_event_serialize_confirmation_timeout() {
    let event = AgentEvent::ConfirmationTimeout {
        tool_id: "t1".to_string(),
        action_taken: "rejected".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("confirmation_timeout"));
}

#[test]
fn test_agent_event_serialize_external_task_pending() {
    let event = AgentEvent::ExternalTaskPending {
        task_id: "task-1".to_string(),
        session_id: "sess-1".to_string(),
        lane: crate::queue::SessionLane::Execute,
        command_type: "bash".to_string(),
        payload: serde_json::json!({}),
        timeout_ms: 60000,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("external_task_pending"));
}

#[test]
fn test_agent_event_serialize_external_task_completed() {
    let event = AgentEvent::ExternalTaskCompleted {
        task_id: "task-1".to_string(),
        session_id: "sess-1".to_string(),
        success: false,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("external_task_completed"));
}

#[test]
fn test_agent_event_serialize_permission_denied() {
    let event = AgentEvent::PermissionDenied {
        tool_id: "t1".to_string(),
        tool_name: "bash".to_string(),
        args: serde_json::json!({}),
        reason: "denied".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("permission_denied"));
}

#[test]
fn test_agent_event_serialize_context_compacted() {
    let event = AgentEvent::ContextCompacted {
        session_id: "sess-1".to_string(),
        before_messages: 100,
        after_messages: 20,
        percent_before: 0.85,
        summary: Some("durable continuation state".to_string()),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("context_compacted"));
    assert!(json.contains("durable continuation state"));
}

#[test]
fn test_agent_event_serialize_turn_start() {
    let event = AgentEvent::TurnStart { turn: 3 };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("turn_start"));
}

#[test]
fn test_agent_event_serialize_turn_end() {
    let event = AgentEvent::TurnEnd {
        turn: 3,
        usage: TokenUsage::default(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("turn_end"));
}

#[test]
fn test_agent_event_serialize_end() {
    let event = AgentEvent::End {
        text: "Done".to_string(),
        usage: TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        verification_summary: Box::new(crate::verification::VerificationSummary::from_reports(&[])),
        meta: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("agent_end"));
    assert!(json.contains("verification_summary"));
}

// ========================================================================
// AgentResult
// ========================================================================

#[test]
fn test_agent_result_fields() {
    let result = AgentResult {
        text: "output".to_string(),
        messages: vec![Message::user("hello")],
        usage: TokenUsage::default(),
        tool_calls_count: 3,
        verification_reports: Vec::new(),
    };
    assert_eq!(result.text, "output");
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.tool_calls_count, 3);
    assert!(result.verification_reports.is_empty());
    assert_eq!(
        result.verification_summary().status,
        crate::verification::VerificationStatus::Skipped
    );
    assert!(!result.has_pending_verification());
}

#[test]
fn test_collect_verification_report_from_tool_metadata() {
    let report = crate::verification::VerificationReport::new(
        "program:example",
        vec![crate::verification::VerificationCheck::required(
            "check:inspect",
            "inspect_artifacts",
            "Inspect artifacts",
        )],
    );
    let metadata = Some(serde_json::json!({
        "verification_report": report.to_value()
    }));
    let mut reports = Vec::new();

    AgentLoop::collect_verification_report(&mut reports, &metadata);

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].subject, "program:example");
    assert_eq!(
        reports[0].status,
        crate::verification::VerificationStatus::NeedsReview
    );
}

#[test]
fn test_agent_result_verification_summary() {
    let report = crate::verification::VerificationReport::new(
        "program:example",
        vec![crate::verification::VerificationCheck::required(
            "check:inspect",
            "inspect_artifacts",
            "Inspect artifacts",
        )],
    );
    let result = AgentResult {
        text: "output".to_string(),
        messages: Vec::new(),
        usage: TokenUsage::default(),
        tool_calls_count: 1,
        verification_reports: vec![report],
    };

    let summary = result.verification_summary();

    assert_eq!(
        summary.status,
        crate::verification::VerificationStatus::NeedsReview
    );
    assert_eq!(summary.pending_required_check_count, 1);
    assert!(result
        .verification_summary_text()
        .contains("Verification needs review"));
    assert!(result.has_pending_verification());
}

// ========================================================================
// Missing AgentEvent serialization tests
// ========================================================================

#[test]
fn test_agent_event_serialize_context_resolving() {
    let event = AgentEvent::ContextResolving {
        providers: vec!["provider1".to_string(), "provider2".to_string()],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("context_resolving"));
    assert!(json.contains("provider1"));
}

#[test]
fn test_agent_event_serialize_context_resolved() {
    let event = AgentEvent::ContextResolved {
        total_items: 5,
        total_tokens: 1000,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("context_resolved"));
    assert!(json.contains("1000"));
}

#[test]
fn test_agent_event_serialize_command_dead_lettered() {
    let event = AgentEvent::CommandDeadLettered {
        command_id: "cmd-1".to_string(),
        command_type: "bash".to_string(),
        lane: "execute".to_string(),
        error: "timeout".to_string(),
        attempts: 3,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("command_dead_lettered"));
    assert!(json.contains("cmd-1"));
}

#[test]
fn test_agent_event_serialize_command_retry() {
    let event = AgentEvent::CommandRetry {
        command_id: "cmd-2".to_string(),
        command_type: "read".to_string(),
        lane: "query".to_string(),
        attempt: 2,
        delay_ms: 1000,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("command_retry"));
    assert!(json.contains("cmd-2"));
}

#[test]
fn test_agent_event_serialize_queue_alert() {
    let event = AgentEvent::QueueAlert {
        level: "warning".to_string(),
        alert_type: "depth".to_string(),
        message: "Queue depth exceeded".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("queue_alert"));
    assert!(json.contains("warning"));
}

#[test]
fn test_agent_event_serialize_task_updated() {
    let event = AgentEvent::TaskUpdated {
        session_id: "sess-1".to_string(),
        tasks: vec![],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("task_updated"));
    assert!(json.contains("sess-1"));
}

#[test]
fn test_agent_event_serialize_memory_stored() {
    let event = AgentEvent::MemoryStored {
        memory_id: "mem-1".to_string(),
        memory_type: "conversation".to_string(),
        importance: 0.8,
        tags: vec!["important".to_string()],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("memory_stored"));
    assert!(json.contains("mem-1"));
}

#[test]
fn test_agent_event_serialize_memory_recalled() {
    let event = AgentEvent::MemoryRecalled {
        memory_id: "mem-2".to_string(),
        content: "Previous conversation".to_string(),
        relevance: 0.9,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("memory_recalled"));
    assert!(json.contains("mem-2"));
}

#[test]
fn test_agent_event_serialize_memories_searched() {
    let event = AgentEvent::MemoriesSearched {
        query: Some("search term".to_string()),
        tags: vec!["tag1".to_string()],
        result_count: 5,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("memories_searched"));
    assert!(json.contains("search term"));
}

#[test]
fn test_agent_event_serialize_memory_cleared() {
    let event = AgentEvent::MemoryCleared {
        tier: "short_term".to_string(),
        count: 10,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("memory_cleared"));
    assert!(json.contains("short_term"));
}

#[test]
fn test_agent_event_serialize_subagent_start() {
    let event = AgentEvent::SubagentStart {
        task_id: "task-1".to_string(),
        session_id: "child-sess".to_string(),
        parent_session_id: "parent-sess".to_string(),
        agent: "explore".to_string(),
        description: "Explore codebase".to_string(),
        started_ms: 123,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("subagent_start"));
    assert!(json.contains("explore"));
}

#[test]
fn test_agent_event_serialize_subagent_progress() {
    let event = AgentEvent::SubagentProgress {
        task_id: "task-1".to_string(),
        session_id: "child-sess".to_string(),
        status: "processing".to_string(),
        metadata: serde_json::json!({"progress": 50}),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("subagent_progress"));
    assert!(json.contains("processing"));
}

#[test]
fn test_agent_event_serialize_subagent_end() {
    let event = AgentEvent::SubagentEnd {
        task_id: "task-1".to_string(),
        session_id: "child-sess".to_string(),
        agent: "explore".to_string(),
        output: "Found 10 files".to_string(),
        success: true,
        finished_ms: 456,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("subagent_end"));
    assert!(json.contains("Found 10 files"));
}

#[test]
fn test_agent_event_serialize_planning_start() {
    let event = AgentEvent::PlanningStart {
        prompt: "Build a web app".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("planning_start"));
    assert!(json.contains("Build a web app"));
}

#[test]
fn test_agent_event_serialize_planning_end() {
    use crate::planning::{Complexity, ExecutionPlan};
    let plan = ExecutionPlan::new("Test goal".to_string(), Complexity::Simple);
    let event = AgentEvent::PlanningEnd {
        plan,
        estimated_steps: 3,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("planning_end"));
    assert!(json.contains("estimated_steps"));
}

#[test]
fn test_agent_event_serialize_step_start() {
    let event = AgentEvent::StepStart {
        step_id: "step-1".to_string(),
        description: "Initialize project".to_string(),
        step_number: 1,
        total_steps: 5,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("step_start"));
    assert!(json.contains("Initialize project"));
}

#[test]
fn test_agent_event_serialize_step_end() {
    let event = AgentEvent::StepEnd {
        step_id: "step-1".to_string(),
        status: TaskStatus::Completed,
        step_number: 1,
        total_steps: 5,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("step_end"));
    assert!(json.contains("step-1"));
}

#[test]
fn test_agent_event_serialize_goal_extracted() {
    use crate::planning::AgentGoal;
    let goal = AgentGoal::new("Complete the task".to_string());
    let event = AgentEvent::GoalExtracted { goal };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("goal_extracted"));
}

#[test]
fn test_agent_event_serialize_goal_progress() {
    let event = AgentEvent::GoalProgress {
        goal: "Build app".to_string(),
        progress: 0.5,
        completed_steps: 2,
        total_steps: 4,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("goal_progress"));
    assert!(json.contains("0.5"));
}

#[test]
fn test_agent_event_serialize_goal_achieved() {
    let event = AgentEvent::GoalAchieved {
        goal: "Build app".to_string(),
        total_steps: 4,
        duration_ms: 5000,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("goal_achieved"));
    assert!(json.contains("5000"));
}

#[tokio::test]
async fn test_extract_goal_with_json_response() {
    // LlmPlanner expects JSON with "description" and "success_criteria" fields
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        r#"{"description": "Build web app", "success_criteria": ["App runs on port 3000", "Has login page"]}"#,
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let goal = agent
        .extract_goal(
            "Build a web app",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(goal.description, "Build web app");
    assert_eq!(goal.success_criteria.len(), 2);
    assert_eq!(goal.success_criteria[0], "App runs on port 3000");
}

#[tokio::test]
async fn test_extract_goal_fallback_on_non_json() {
    // Non-JSON response triggers fallback: returns the original prompt as goal
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Some non-JSON response",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let goal = agent
        .extract_goal("Do something", &tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    // Fallback uses the original prompt as description
    assert_eq!(goal.description, "Do something");
    // Fallback adds 2 generic criteria
    assert_eq!(goal.success_criteria.len(), 2);
}

#[tokio::test]
async fn test_check_goal_achievement_json_yes() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        r#"{"achieved": true, "progress": 1.0, "remaining_criteria": []}"#,
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let goal = crate::planning::AgentGoal::new("Test goal".to_string());
    let achieved = agent
        .check_goal_achievement(
            &goal,
            "All done",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(achieved);
}

#[tokio::test]
async fn test_check_goal_achievement_fallback_not_done() {
    // Non-JSON response triggers heuristic fallback
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "invalid json",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let goal = crate::planning::AgentGoal::new("Test goal".to_string());
    // "still working" doesn't contain "complete"/"done"/"finished"
    let achieved = agent
        .check_goal_achievement(
            &goal,
            "still working",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!achieved);
}

// ========================================================================
// build_augmented_system_prompt Tests
// ========================================================================

#[test]
fn test_build_augmented_system_prompt_empty_context() {
    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        prompt_slots: SystemPromptSlots {
            extra: Some("Base prompt".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let result = agent.build_augmented_system_prompt(&[]);
    assert!(result.unwrap().contains("Base prompt"));
}

#[test]
fn test_build_augmented_system_prompt_no_custom_slots() {
    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let result = agent.build_augmented_system_prompt(&[]);
    // Default slots still produce the default agentic prompt
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(text.contains("Core Behaviour"));
    // The always-on <env> grounding block is injected at augmentation time.
    assert!(text.contains("<env>"));
    assert!(text.contains("Today's date:"));
    assert!(text.contains("## Boundaries"));
}

#[test]
fn test_project_hint_is_assembled_as_context_item() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();

    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new(temp_dir.path().display().to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        ToolContext::new(temp_dir.path().to_path_buf()),
        AgentConfig::default(),
    );

    let assembly = agent.assemble_context_results(&[]);
    assert_eq!(assembly.items.len(), 1);
    assert_eq!(
        assembly.items[0].source.as_deref(),
        Some("a3s://project-hint")
    );
    assert!(assembly.items[0].content.contains("Rust"));

    let text = agent.build_augmented_system_prompt(&[]).unwrap();
    assert!(text.contains("<context source=\"a3s://project-hint\" type=\"Resource\">"));
}

#[test]
fn test_build_augmented_system_prompt_with_context_no_base() {
    use crate::context::{ContextItem, ContextResult, ContextType};

    let mock_client = Arc::new(MockLlmClient::new(vec![]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let context = vec![ContextResult {
        provider: "test".to_string(),
        items: vec![ContextItem::new("id1", ContextType::Resource, "Content")],
        total_tokens: 10,
        truncated: false,
    }];

    let result = agent.build_augmented_system_prompt(&context);
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(text.contains("<context"));
    assert!(text.contains("Content"));
}

// ========================================================================
// AgentResult Clone and Debug
// ========================================================================

#[test]
fn test_agent_result_clone() {
    let result = AgentResult {
        text: "output".to_string(),
        messages: vec![Message::user("hello")],
        usage: TokenUsage::default(),
        tool_calls_count: 3,
        verification_reports: Vec::new(),
    };
    let cloned = result.clone();
    assert_eq!(cloned.text, result.text);
    assert_eq!(cloned.tool_calls_count, result.tool_calls_count);
}

#[test]
fn test_agent_result_debug() {
    let result = AgentResult {
        text: "output".to_string(),
        messages: vec![Message::user("hello")],
        usage: TokenUsage::default(),
        tool_calls_count: 3,
        verification_reports: Vec::new(),
    };
    let debug = format!("{:?}", result);
    assert!(debug.contains("AgentResult"));
    assert!(debug.contains("output"));
}

// ========================================================================
// handle_post_execution_metadata Tests
// ========================================================================

// ========================================================================
// ToolCommand adapter tests
// ========================================================================

#[tokio::test]
async fn test_tool_command_command_type() {
    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let cmd = ToolCommand {
        tool_executor: executor,
        tool_name: "read".to_string(),
        tool_args: serde_json::json!({"file": "test.rs"}),
        tool_timeout_ms: None,
        tool_context: test_tool_context(),
    };
    assert_eq!(cmd.command_type(), "read");
}

#[tokio::test]
async fn test_tool_command_payload() {
    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let args = serde_json::json!({"file": "test.rs", "offset": 10});
    let cmd = ToolCommand {
        tool_executor: executor,
        tool_name: "read".to_string(),
        tool_args: args.clone(),
        tool_timeout_ms: None,
        tool_context: test_tool_context(),
    };
    assert_eq!(cmd.payload(), args);
}

#[tokio::test]
async fn test_queued_tool_command_is_cancelled_before_side_effect_completion() {
    struct NeverCompletes;

    #[async_trait::async_trait]
    impl crate::tools::Tool for NeverCompletes {
        fn name(&self) -> &str {
            "never_completes"
        }

        fn description(&self) -> &str {
            "blocks until its future is dropped"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<crate::tools::ToolOutput> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    executor.register_dynamic_tool(Arc::new(NeverCompletes));
    let cancellation = tokio_util::sync::CancellationToken::new();
    let cmd = ToolCommand {
        tool_executor: executor,
        tool_name: "never_completes".to_string(),
        tool_args: serde_json::json!({}),
        tool_timeout_ms: None,
        tool_context: test_tool_context().with_cancellation(cancellation.clone()),
    };

    let task = tokio::spawn(async move { cmd.execute().await });
    tokio::task::yield_now().await;
    cancellation.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("queued execution must observe cancellation")
        .unwrap()
        .unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error_kind"]["type"], "cancelled");
    assert!(result["output"]
        .as_str()
        .is_some_and(|output| output.contains("cancelled")));
}

#[tokio::test]
async fn test_queued_tool_command_applies_execution_timeout() {
    struct NeverCompletes;

    #[async_trait::async_trait]
    impl crate::tools::Tool for NeverCompletes {
        fn name(&self) -> &str {
            "never_completes_timeout"
        }

        fn description(&self) -> &str {
            "blocks until timed out"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<crate::tools::ToolOutput> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    executor.register_dynamic_tool(Arc::new(NeverCompletes));
    let cmd = ToolCommand {
        tool_executor: executor,
        tool_name: "never_completes_timeout".to_string(),
        tool_args: serde_json::json!({}),
        tool_timeout_ms: Some(10),
        tool_context: test_tool_context(),
    };

    let result = cmd.execute().await.unwrap();
    assert_eq!(result["exit_code"], 1);
    assert!(result["output"]
        .as_str()
        .is_some_and(|output| output.contains("timed out after 10ms")));
    assert_eq!(result["error_kind"]["type"], "timeout");
    assert_eq!(result["error_kind"]["duration_ms"], 10);
}

#[tokio::test]
async fn queued_tool_timeout_cancels_and_settles_the_invocation() {
    struct CancellationAwareTool {
        cancellations: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::tools::Tool for CancellationAwareTool {
        fn name(&self) -> &str {
            "queued_cancellation_aware"
        }

        fn description(&self) -> &str {
            "waits for invocation cancellation"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> anyhow::Result<crate::tools::ToolOutput> {
            ctx.cancellation_token().cancelled().await;
            self.cancellations.fetch_add(1, Ordering::SeqCst);
            Ok(crate::tools::ToolOutput::success("settled"))
        }
    }

    let cancellations = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    executor.register_dynamic_tool(Arc::new(CancellationAwareTool {
        cancellations: Arc::clone(&cancellations),
    }));
    let cmd = ToolCommand {
        tool_executor: executor,
        tool_name: "queued_cancellation_aware".to_string(),
        tool_args: serde_json::json!({}),
        tool_timeout_ms: Some(10),
        tool_context: test_tool_context(),
    };

    let result = cmd.execute().await.unwrap();
    assert_eq!(result["exit_code"], 1);
    assert!(result["output"]
        .as_str()
        .is_some_and(|output| output.contains("timed out after 10ms")));
    assert_eq!(result["error_kind"]["type"], "timeout");
    assert_eq!(result["error_kind"]["duration_ms"], 10);
    assert_eq!(
        cancellations.load(Ordering::SeqCst),
        1,
        "the invocation must observe cancellation before timeout returns"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_queued_tool_result_preserves_metadata() {
    struct MetadataTool;

    #[async_trait::async_trait]
    impl crate::tools::Tool for MetadataTool {
        fn name(&self) -> &str {
            "metadata_tool"
        }

        fn description(&self) -> &str {
            "returns structured metadata"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::success("ok")
                .with_metadata(serde_json::json!({"source": "queue"}))
                .with_images(vec![crate::llm::Attachment::png(vec![1, 2, 3, 4])]))
        }
    }

    use tokio::sync::broadcast;

    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    executor.register_dynamic_tool(Arc::new(MetadataTool));
    let (event_tx, _) = broadcast::channel(100);
    let queue = SessionLaneQueue::new("metadata-session", SessionQueueConfig::default(), event_tx)
        .await
        .unwrap();
    queue.start().await.unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        executor,
        test_tool_context(),
        AgentConfig::default(),
    )
    .with_queue(Arc::new(queue));

    let result = agent
        .execute_tool_queued_or_direct(
            "metadata_tool",
            &serde_json::json!({}),
            &test_tool_context(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.metadata,
        Some(serde_json::json!({"source": "queue"}))
    );
    assert_eq!(result.images.len(), 1);
    assert_eq!(result.images[0].media_type, "image/png");
    assert_eq!(result.images[0].data, vec![1, 2, 3, 4]);
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_orchestrators_do_not_resubmit_nested_tools_into_the_owned_lane() {
    struct NestedEcho;

    #[async_trait::async_trait]
    impl crate::tools::Tool for NestedEcho {
        fn name(&self) -> &str {
            "nested_echo"
        }

        fn description(&self) -> &str {
            "returns from a nested queued invocation"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::success("nested-ok"))
        }
    }

    use tokio::sync::broadcast;

    let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    executor.register_dynamic_tool(Arc::new(NestedEcho));
    let config = AgentConfig {
        tool_timeout_ms: Some(500),
        ..AgentConfig::default()
    };
    let queue_config = SessionQueueConfig {
        execute_max_concurrency: 1,
        ..SessionQueueConfig::default()
    };
    let (queue_events, _) = broadcast::channel(100);
    let queue = SessionLaneQueue::new("nested-queue", queue_config, queue_events)
        .await
        .unwrap();
    queue.start().await.unwrap();
    let tool_context = test_tool_context().with_session_id("nested-queue");
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        executor,
        tool_context.clone(),
        config,
    )
    .with_queue(Arc::new(queue));
    let cancellation = tokio_util::sync::CancellationToken::new();

    let calls = [
        (
            "batch",
            serde_json::json!({
                "invocations": [{"tool": "nested_echo", "args": {}}]
            }),
        ),
        (
            "program",
            serde_json::json!({
                "type": "script",
                "language": "javascript",
                "source": "async function run(ctx) { return await ctx.tool('nested_echo', {}); }",
                "allowed_tools": ["nested_echo"]
            }),
        ),
    ];

    for (name, args) in calls {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            agent.invoke_host_tool(
                crate::tools::ToolInvocation::host_direct(format!("host-{name}"), name, args),
                "nested-queue",
                &None,
                &cancellation,
                &tool_context,
            ),
        )
        .await
        .expect("a nested tool must not deadlock its owning queue lane");

        assert_eq!(result.exit_code, 0, "{name}: {}", result.output);
        assert!(
            result.output.contains("nested-ok"),
            "{name}: {}",
            result.output
        );
    }
}

// ========================================================================
// AgentLoop with queue builder tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_loop_with_queue() {
    use tokio::sync::broadcast;

    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Hello",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let (event_tx, _) = broadcast::channel(100);
    let queue = SessionLaneQueue::new("test-session", SessionQueueConfig::default(), event_tx)
        .await
        .unwrap();

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config)
        .with_queue(Arc::new(queue));

    assert!(agent.command_queue.is_some());
}

#[tokio::test]
async fn test_agent_loop_without_queue() {
    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "Hello",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();

    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    assert!(agent.command_queue.is_none());
}

// ========================================================================
// Parallel Plan Execution Tests
// ========================================================================

#[tokio::test]
async fn test_execute_plan_parallel_independent() {
    use crate::planning::{Complexity, ExecutionPlan, Task};

    // 3 independent steps (no dependencies) — should all execute.
    // MockLlmClient needs one response per execute_loop call per step.
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Step 1 done"),
        MockLlmClient::text_response("Step 2 done"),
        MockLlmClient::text_response("Step 3 done"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );

    let mut plan = ExecutionPlan::new("Test parallel", Complexity::Simple);
    plan.add_step(Task::new("s1", "First step"));
    plan.add_step(Task::new("s2", "Second step"));
    plan.add_step(Task::new("s3", "Third step"));

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("test-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    // All 3 steps should have been executed (3 * 15 = 45 total tokens)
    assert_eq!(result.usage.total_tokens, 45);

    // Verify we received StepStart and StepEnd events for all 3 steps
    let mut step_starts = Vec::new();
    let mut step_ends = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::StepStart { step_id, .. } => step_starts.push(step_id),
            AgentEvent::StepEnd {
                step_id, status, ..
            } => {
                assert_eq!(status, TaskStatus::Completed);
                step_ends.push(step_id);
            }
            _ => {}
        }
    }
    assert_eq!(step_starts.len(), 3);
    assert_eq!(step_ends.len(), 3);
}

#[tokio::test]
async fn test_execute_plan_emits_task_list_snapshots() {
    use crate::planning::{Complexity, ExecutionPlan, Task};

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Step 1 done"),
        MockLlmClient::text_response("Step 2 done"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent = AgentLoop::new(
        mock_client,
        tool_executor,
        test_tool_context(),
        AgentConfig::default(),
    );

    let mut plan = ExecutionPlan::new("Track task list", Complexity::Simple);
    plan.add_step(Task::new("s1", "First step"));
    plan.add_step(Task::new("s2", "Second step").with_dependencies(vec!["s1".to_string()]));

    let (tx, mut rx) = mpsc::channel(100);
    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("task-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut snapshots = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::TaskUpdated { session_id, tasks } = event {
            assert_eq!(session_id, "task-session");
            snapshots.push(tasks);
        }
    }

    assert!(
        snapshots
            .first()
            .unwrap()
            .iter()
            .all(|task| task.status == TaskStatus::Pending),
        "initial snapshot should expose the pending task list"
    );
    assert!(snapshots.iter().any(|tasks| tasks
        .iter()
        .any(|task| task.id == "s1" && task.status == TaskStatus::InProgress)));
    assert!(snapshots.iter().any(|tasks| tasks
        .iter()
        .any(|task| task.id == "s1" && task.status == TaskStatus::Completed)));
    assert!(snapshots
        .last()
        .unwrap()
        .iter()
        .all(|task| task.status == TaskStatus::Completed));
}

#[tokio::test]
async fn test_execute_plan_delegates_task_tool_steps() {
    use crate::planning::{Complexity, ExecutionPlan, Task};
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let child_client = Arc::new(PlanDelegationChildClient);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    register_task(
        tool_executor.registry(),
        child_client,
        Arc::new(AgentRegistry::new()),
        "/tmp".to_string(),
    );
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        tool_executor,
        test_tool_context(),
        allow_delegated_tools(AgentConfig::default()),
    );

    let mut plan = ExecutionPlan::new("Delegate a step", Complexity::Simple);
    plan.add_step(Task::new("s1", "Find the relevant docs").with_tool("task"));

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("task-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.tool_calls_count, 1);
    assert!(
        result.text.contains("delegated search complete"),
        "{}",
        result.text
    );

    let mut saw_task_execution_start = false;
    let mut saw_completed_step = false;
    rx.close();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ToolExecutionStart { name, .. } if name == "task" => {
                saw_task_execution_start = true;
            }
            AgentEvent::StepEnd {
                status: TaskStatus::Completed,
                ..
            } => {
                saw_completed_step = true;
            }
            _ => {}
        }
    }

    assert!(saw_task_execution_start);
    assert!(saw_completed_step);
}

#[tokio::test]
async fn test_execute_plan_delegates_parallel_task_wave_once() {
    use crate::planning::{Complexity, ExecutionPlan, Task};
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let child_client = Arc::new(PlanDelegationChildClient);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    register_task(
        tool_executor.registry(),
        child_client,
        Arc::new(AgentRegistry::new()),
        "/tmp".to_string(),
    );
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        tool_executor,
        test_tool_context(),
        allow_delegated_tools(AgentConfig::default()),
    );

    let mut plan = ExecutionPlan::new("Delegate independent wave", Complexity::Medium);
    plan.add_step(Task::new("s1", "Summarize delegated documentation").with_tool("task"));
    plan.add_step(Task::new("s2", "Summarize delegated checks").with_tool("task"));

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("parallel-task-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.tool_calls_count, 1,
        "independent delegated wave should be collapsed into one parallel_task call"
    );
    assert!(
        result.text.contains("delegated docs complete"),
        "{}",
        result.text
    );
    assert!(
        result.text.contains("delegated tests complete"),
        "{}",
        result.text
    );

    let mut parallel_task_starts = 0;
    let mut completed_steps = Vec::new();
    let mut task_snapshots = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ToolExecutionStart { name, .. } if name == "parallel_task" => {
                parallel_task_starts += 1;
            }
            AgentEvent::StepEnd {
                step_id,
                status: TaskStatus::Completed,
                ..
            } => completed_steps.push(step_id),
            AgentEvent::TaskUpdated { tasks, .. } => task_snapshots.push(tasks),
            _ => {}
        }
    }

    completed_steps.sort();
    assert_eq!(parallel_task_starts, 1);
    assert_eq!(completed_steps, vec!["s1".to_string(), "s2".to_string()]);
    assert!(task_snapshots.iter().any(|tasks| tasks
        .iter()
        .all(|task| task.status == TaskStatus::InProgress)));
    assert!(task_snapshots
        .last()
        .unwrap()
        .iter()
        .all(|task| task.status == TaskStatus::Completed));
}

#[tokio::test]
async fn test_execute_plan_auto_delegates_unmarked_parallel_wave_when_enabled() {
    use crate::planning::{Complexity, ExecutionPlan, Task};
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let child_client = Arc::new(PlanDelegationChildClient);
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent_registry = Arc::new(AgentRegistry::new());
    register_task(
        tool_executor.registry(),
        child_client,
        Arc::clone(&agent_registry),
        "/tmp".to_string(),
    );
    let auto_delegation = crate::config::AutoDelegationConfig {
        enabled: true,
        auto_parallel: true,
        ..Default::default()
    };
    let config = AgentConfig {
        auto_delegation,
        agent_registry: Some(agent_registry),
        permission_checker: Some(Arc::new(AllowDelegatedTools)),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        tool_executor,
        test_tool_context(),
        config,
    );

    let mut plan = ExecutionPlan::new("Explore independent areas", Complexity::Medium);
    plan.add_step(Task::new("s1", "Find auth code"));
    plan.add_step(Task::new("s2", "Find documentation"));

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("auto-plan-parallel-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(
        result.tool_calls_count, 1,
        "auto-parallel plan wave should collapse into one parallel_task call"
    );
    assert!(result.text.contains("auth exploration complete"));
    assert!(result.text.contains("docs exploration complete"));

    let mut parallel_task_starts = 0;
    let mut completed_steps = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ToolExecutionStart { name, .. } if name == "parallel_task" => {
                parallel_task_starts += 1;
            }
            AgentEvent::StepEnd {
                step_id,
                status: TaskStatus::Completed,
                ..
            } => completed_steps.push(step_id),
            _ => {}
        }
    }

    completed_steps.sort();
    assert_eq!(parallel_task_starts, 1);
    assert_eq!(completed_steps, vec!["s1".to_string(), "s2".to_string()]);
}

#[tokio::test]
async fn test_execute_plan_delegated_parallel_wave_maps_child_failure() {
    use crate::planning::{Complexity, ExecutionPlan, Task};
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let child_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "delegated docs complete",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let registry = Arc::new(AgentRegistry::new());
    registry.unregister("verification");
    register_task(
        tool_executor.registry(),
        child_client,
        registry,
        "/tmp".to_string(),
    );
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        tool_executor,
        test_tool_context(),
        allow_delegated_tools(AgentConfig::default()),
    );

    let mut plan = ExecutionPlan::new("Delegate partially failing wave", Complexity::Medium);
    plan.add_step(Task::new("s1", "Find relevant docs").with_tool("task"));
    plan.add_step(Task::new("s2", "Run verification tests").with_tool("task"));

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("parallel-task-failure-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    let mut completed_steps = Vec::new();
    let mut failed_steps = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::StepEnd {
            step_id, status, ..
        } = event
        {
            match status {
                TaskStatus::Completed => completed_steps.push(step_id),
                TaskStatus::Failed => failed_steps.push(step_id),
                _ => {}
            }
        }
    }

    completed_steps.sort();
    failed_steps.sort();
    assert_eq!(completed_steps, vec!["s1".to_string()]);
    assert_eq!(failed_steps, vec!["s2".to_string()]);

    let envelope = result
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .find_map(|message| {
            let text = message
                .content
                .iter()
                .filter_map(|block| {
                    if let crate::llm::ContentBlock::Text { text } = block {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            serde_json::from_str::<serde_json::Value>(&text).ok()
        })
        .expect("parallel result envelope");
    assert_eq!(envelope["type"], "parallel_results");
    let steps = envelope["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["step_id"], "s1");
    assert_eq!(steps[0]["status"], "completed");
    assert!(steps[0]["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("delegated docs complete")));
    assert!(!steps[0]["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("Unknown agent")));
    assert_eq!(steps[1]["step_id"], "s2");
    assert_eq!(steps[1]["status"], "failed");
    assert!(steps[1]["error"]
        .as_str()
        .is_some_and(|error| error.contains("Unknown agent")));
}

#[tokio::test]
async fn test_auto_delegation_runs_parallel_specialists_when_enabled() {
    use crate::prompts::PlanningMode;
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("review child complete"),
        MockLlmClient::text_response("verification child complete"),
        MockLlmClient::text_response("final answer with automatic context"),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent_registry = Arc::new(AgentRegistry::new());
    register_task(
        tool_executor.registry(),
        mock_client.clone(),
        agent_registry.clone(),
        "/tmp".to_string(),
    );
    let auto_delegation = crate::config::AutoDelegationConfig {
        enabled: true,
        max_tasks: 2,
        ..Default::default()
    };
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        auto_delegation,
        agent_registry: Some(agent_registry),
        permission_checker: Some(Arc::new(AllowDelegatedTools)),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_with_session(
            &[],
            "Review the current diff and run regression tests",
            Some("auto-parallel-session"),
            Some(tx),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.tool_calls_count, 1);
    assert!(result.text.contains("final answer"));

    let mut parallel_task_starts = 0;
    rx.close();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolExecutionStart { name, .. } = event {
            if name == "parallel_task" {
                parallel_task_starts += 1;
            }
        }
    }
    assert_eq!(parallel_task_starts, 1);
}

#[tokio::test]
async fn test_auto_delegation_global_parallel_switch_uses_single_task() {
    use crate::prompts::PlanningMode;
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("single child complete"),
        MockLlmClient::text_response("final answer"),
    ]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent_registry = Arc::new(AgentRegistry::new());
    register_task(
        tool_executor.registry(),
        mock_client.clone(),
        agent_registry.clone(),
        "/tmp".to_string(),
    );
    let auto_delegation = crate::config::AutoDelegationConfig {
        enabled: true,
        auto_parallel: false,
        max_tasks: 2,
        ..Default::default()
    };
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        auto_delegation,
        agent_registry: Some(agent_registry),
        permission_checker: Some(Arc::new(AllowDelegatedTools)),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_with_session(
            &[],
            "Review the current diff and run regression tests",
            Some("auto-single-session"),
            Some(tx),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.tool_calls_count, 1);

    let mut task_starts = 0;
    let mut parallel_task_starts = 0;
    rx.close();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolExecutionStart { name, .. } = event {
            if name == "task" {
                task_starts += 1;
            } else if name == "parallel_task" {
                parallel_task_starts += 1;
            }
        }
    }
    assert_eq!(task_starts, 1);
    assert_eq!(parallel_task_starts, 0);
}

#[tokio::test]
async fn test_auto_delegation_disabled_does_not_start_subagents() {
    use crate::prompts::PlanningMode;
    use crate::subagent::AgentRegistry;
    use crate::tools::register_task;

    let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
        "final answer without delegation",
    )]));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let agent_registry = Arc::new(AgentRegistry::new());
    register_task(
        tool_executor.registry(),
        mock_client.clone(),
        agent_registry.clone(),
        "/tmp".to_string(),
    );
    let config = AgentConfig {
        planning_mode: PlanningMode::Disabled,
        auto_delegation: crate::config::AutoDelegationConfig {
            enabled: false,
            ..Default::default()
        },
        agent_registry: Some(agent_registry),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_with_session(
            &[],
            "Review the current diff and run regression tests",
            Some("auto-disabled-session"),
            Some(tx),
            None,
        )
        .await
        .unwrap();

    assert_eq!(result.tool_calls_count, 0);

    let mut task_tool_starts = 0;
    rx.close();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolExecutionStart { name, .. } = event {
            if name == "task" || name == "parallel_task" {
                task_tool_starts += 1;
            }
        }
    }
    assert_eq!(task_tool_starts, 0);
}

#[tokio::test]
async fn test_execute_plan_respects_dependencies() {
    use crate::planning::{Complexity, ExecutionPlan, Task};

    // s1 and s2 are independent (wave 1), s3 depends on both (wave 2).
    // This requires 3 responses total.
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::text_response("Step 1 done"),
        MockLlmClient::text_response("Step 2 done"),
        MockLlmClient::text_response("Step 3 done"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );

    let mut plan = ExecutionPlan::new("Test deps", Complexity::Medium);
    plan.add_step(Task::new("s1", "Independent A"));
    plan.add_step(Task::new("s2", "Independent B"));
    plan.add_step(
        Task::new("s3", "Depends on A+B")
            .with_dependencies(vec!["s1".to_string(), "s2".to_string()]),
    );

    let (tx, mut rx) = mpsc::channel(100);
    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("test-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    // All 3 steps should have been executed (3 * 15 = 45 total tokens)
    assert_eq!(result.usage.total_tokens, 45);

    // Verify ordering: s3's StepStart must come after s1 and s2's StepEnd
    let mut events = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        match &event {
            AgentEvent::StepStart { step_id, .. } => {
                events.push(format!("start:{}", step_id));
            }
            AgentEvent::StepEnd { step_id, .. } => {
                events.push(format!("end:{}", step_id));
            }
            _ => {}
        }
    }

    // s3 start must occur after both s1 end and s2 end
    let s1_end = events.iter().position(|e| e == "end:s1").unwrap();
    let s2_end = events.iter().position(|e| e == "end:s2").unwrap();
    let s3_start = events.iter().position(|e| e == "start:s3").unwrap();
    assert!(
        s3_start > s1_end,
        "s3 started before s1 ended: {:?}",
        events
    );
    assert!(
        s3_start > s2_end,
        "s3 started before s2 ended: {:?}",
        events
    );

    // Final result should reflect step 3 (last sequential step)
    assert!(result.text.contains("Step 3 done") || !result.text.is_empty());
}

#[tokio::test]
async fn test_execute_plan_handles_step_failure() {
    use crate::planning::{Complexity, ExecutionPlan, Task};

    // s1 succeeds, s2 depends on s1 (succeeds), s3 depends on nothing (succeeds),
    // s4 depends on a step that will fail (s_fail).
    // We simulate failure by providing no responses for s_fail's execute_loop.
    //
    // Simpler approach: s1 succeeds, s2 depends on s1 (will fail because no
    // mock response left), s3 is independent.
    // Layout: s1 (independent), s3 (independent) → wave 1 parallel
    //         s2 depends on s1 → wave 2
    //         s4 depends on s2 → wave 3 (should deadlock since s2 fails)
    let mock_client = Arc::new(MockLlmClient::new(vec![
        // Wave 1: s1 and s3 execute in parallel
        MockLlmClient::text_response("s1 done"),
        MockLlmClient::text_response("s3 done"),
        // Wave 2: s2 executes — but we give it no response, causing failure
        // Actually the MockLlmClient will fail with "No more mock responses"
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig::default();
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );

    let mut plan = ExecutionPlan::new("Test failure", Complexity::Medium);
    plan.add_step(Task::new("s1", "Independent step"));
    plan.add_step(Task::new("s2", "Depends on s1").with_dependencies(vec!["s1".to_string()]));
    plan.add_step(Task::new("s3", "Another independent"));
    plan.add_step(Task::new("s4", "Depends on s2").with_dependencies(vec!["s2".to_string()]));

    let (tx, mut rx) = mpsc::channel(100);
    let _result = agent
        .execute_plan(
            &[],
            &plan,
            Some("test-session"),
            Some(tx),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    // s1 and s3 should succeed (wave 1), s2 should fail (wave 2),
    // s4 should never execute (deadlock — dep s2 failed, not completed)
    let mut completed_steps = Vec::new();
    let mut failed_steps = Vec::new();
    rx.close();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::StepEnd {
            step_id, status, ..
        } = event
        {
            match status {
                TaskStatus::Completed => completed_steps.push(step_id),
                TaskStatus::Failed => failed_steps.push(step_id),
                _ => {}
            }
        }
    }

    assert!(
        completed_steps.contains(&"s1".to_string()),
        "s1 should complete"
    );
    assert!(
        completed_steps.contains(&"s3".to_string()),
        "s3 should complete"
    );
    assert!(failed_steps.contains(&"s2".to_string()), "s2 should fail");
    // s4 should NOT appear in either list — it was never started
    assert!(
        !completed_steps.contains(&"s4".to_string()),
        "s4 should not complete"
    );
    assert!(
        !failed_steps.contains(&"s4".to_string()),
        "s4 should not fail (never started)"
    );
}

// ========================================================================
// Phase 4: Error Recovery & Resilience Tests
// ========================================================================

#[test]
fn test_agent_config_resilience_defaults() {
    let config = AgentConfig::default();
    assert_eq!(config.max_parse_retries, 2);
    assert_eq!(config.tool_timeout_ms, None);
    assert_eq!(config.llm_api_timeout_ms, None);
    assert_eq!(config.circuit_breaker_threshold, 3);
}

/// 4.1 — Parse error recovery: bails after max_parse_retries exceeded
#[tokio::test]
async fn test_parse_error_recovery_bails_after_threshold() {
    // 3 parse errors with max_parse_retries=2: count reaches 3 > 2 → bail
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "c1",
            "bash",
            serde_json::json!({"__parse_error": "unexpected token at position 5"}),
        ),
        MockLlmClient::tool_call_response(
            "c2",
            "bash",
            serde_json::json!({"__parse_error": "missing closing brace"}),
        ),
        MockLlmClient::tool_call_response(
            "c3",
            "bash",
            serde_json::json!({"__parse_error": "still broken"}),
        ),
        MockLlmClient::text_response("Done"), // never reached
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        max_parse_retries: 2,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Do something", None).await;
    assert!(result.is_err(), "should bail after parse error threshold");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("malformed tool arguments"),
        "error should mention malformed tool arguments, got: {}",
        err
    );
}

/// 4.1 — Parse error recovery: counter resets after a valid tool execution
#[tokio::test]
async fn test_parse_error_counter_resets_on_success() {
    // 2 parse errors (= max_parse_retries, not yet exceeded)
    // Then a valid tool call (resets counter)
    // Then final text — should NOT bail
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "c1",
            "bash",
            serde_json::json!({"__parse_error": "bad args"}),
        ),
        MockLlmClient::tool_call_response(
            "c2",
            "bash",
            serde_json::json!({"__parse_error": "bad args again"}),
        ),
        // Valid call — resets parse_error_count to 0
        MockLlmClient::tool_call_response("c3", "bash", serde_json::json!({"command": "echo ok"})),
        MockLlmClient::text_response("All done"),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        max_parse_retries: 2,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Do something", None).await;
    assert!(
        result.is_ok(),
        "should not bail — counter reset after successful tool, got: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().text, "All done");
}

/// 4.2 — Tool timeout: slow tool produces a timeout error result; session continues
#[tokio::test]
async fn test_tool_timeout_produces_error_result() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response("t1", "bash", serde_json::json!({"command": "sleep 10"})),
        MockLlmClient::text_response("The command timed out."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        // 50ms — sleep 10 will never finish
        tool_timeout_ms: Some(50),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Run sleep", None).await;
    assert!(
        result.is_ok(),
        "session should continue after tool timeout: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().text, "The command timed out.");
    // LLM called twice: initial request + response after timeout error
    assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn direct_tool_timeout_cancels_and_settles_the_invocation() {
    struct CancellationAwareTool {
        cancellations: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::tools::Tool for CancellationAwareTool {
        fn name(&self) -> &str {
            "direct_cancellation_aware"
        }

        fn description(&self) -> &str {
            "waits for invocation cancellation"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> anyhow::Result<crate::tools::ToolOutput> {
            ctx.cancellation_token().cancelled().await;
            self.cancellations.fetch_add(1, Ordering::SeqCst);
            Ok(crate::tools::ToolOutput::success("settled"))
        }
    }

    let cancellations = Arc::new(AtomicUsize::new(0));
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    tool_executor.register_dynamic_tool(Arc::new(CancellationAwareTool {
        cancellations: Arc::clone(&cancellations),
    }));
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response("t1", "direct_cancellation_aware", serde_json::json!({})),
        MockLlmClient::text_response("The tool timed out."),
    ]));
    let config = allow_delegated_tools(AgentConfig {
        tool_timeout_ms: Some(10),
        ..AgentConfig::default()
    });
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

    let result = agent.execute(&[], "Run the tool", None).await.unwrap();

    assert_eq!(result.text, "The tool timed out.");
    assert_eq!(
        cancellations.load(Ordering::SeqCst),
        1,
        "the invocation must observe cancellation before the model continues"
    );
}

/// 4.2 — Tool timeout: tool that finishes before the deadline succeeds normally
#[tokio::test]
async fn test_tool_within_timeout_succeeds() {
    let mock_client = Arc::new(MockLlmClient::new(vec![
        MockLlmClient::tool_call_response(
            "t1",
            "bash",
            serde_json::json!({"command": "echo fast"}),
        ),
        MockLlmClient::text_response("Command succeeded."),
    ]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        tool_timeout_ms: Some(5_000), // 5 s — echo completes in <100ms
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Run something fast", None).await;
    assert!(
        result.is_ok(),
        "fast tool should succeed: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().text, "Command succeeded.");
}

/// 4.3 — Circuit breaker: retries non-streaming LLM failures up to threshold
#[tokio::test]
async fn test_circuit_breaker_retries_non_streaming() {
    // Empty response list → every call bails with "No more mock responses"
    // threshold=2 → tries twice, then bails with circuit-breaker message
    let mock_client = Arc::new(MockLlmClient::new(vec![]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        circuit_breaker_threshold: 2,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Hello", None).await;
    assert!(result.is_err(), "should fail when LLM always errors");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("circuit breaker"),
        "error should mention circuit breaker, got: {}",
        err
    );
    assert_eq!(
        mock_client.call_count.load(Ordering::SeqCst),
        2,
        "should make exactly threshold=2 LLM calls"
    );
}

#[tokio::test]
async fn test_circuit_breaker_backoff_stops_immediately_on_cancellation() {
    struct AlwaysFails {
        calls: AtomicUsize,
        first_failure: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl LlmClient for AlwaysFails {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.first_failure.notify_one();
            anyhow::bail!("transient provider failure")
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            anyhow::bail!("streaming is not used by this test")
        }
    }

    let client = Arc::new(AlwaysFails {
        calls: AtomicUsize::new(0),
        first_failure: tokio::sync::Notify::new(),
    });
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new("/tmp".to_string())),
        test_tool_context(),
        AgentConfig {
            circuit_breaker_threshold: 10,
            planning_mode: crate::prompts::PlanningMode::Disabled,
            ..AgentConfig::default()
        },
    );
    let cancellation = tokio_util::sync::CancellationToken::new();
    let run_cancellation = cancellation.clone();
    let run = tokio::spawn(async move {
        agent
            .execute_with_session(
                &[],
                "Hello",
                Some("retry-cancel"),
                None,
                Some(&run_cancellation),
            )
            .await
    });

    client.first_failure.notified().await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    cancellation.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_millis(50), run)
        .await
        .expect("cancellation must interrupt retry backoff")
        .unwrap()
        .unwrap();
    assert!(result
        .messages
        .last()
        .is_some_and(|message| message.text().contains("interrupted")));
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
}

/// 4.3 — Circuit breaker: threshold=1 bails on the very first failure
#[tokio::test]
async fn test_circuit_breaker_threshold_one_no_retry() {
    let mock_client = Arc::new(MockLlmClient::new(vec![]));

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let config = AgentConfig {
        circuit_breaker_threshold: 1,
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(
        mock_client.clone(),
        tool_executor,
        test_tool_context(),
        config,
    );
    let result = agent.execute(&[], "Hello", None).await;
    assert!(result.is_err());
    assert_eq!(
        mock_client.call_count.load(Ordering::SeqCst),
        1,
        "with threshold=1 exactly one attempt should be made"
    );
}

/// 4.3 — Circuit breaker: succeeds when LLM recovers before hitting threshold
#[tokio::test]
async fn test_circuit_breaker_succeeds_if_llm_recovers() {
    // First call fails, second call succeeds; threshold=3 — recovery within threshold
    struct FailOnceThenSucceed {
        inner: MockLlmClient,
        failed_once: std::sync::atomic::AtomicBool,
        call_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmClient for FailOnceThenSucceed {
        async fn complete(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let already_failed = self
                .failed_once
                .swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already_failed {
                anyhow::bail!("transient network error");
            }
            self.inner.complete(messages, system, tools).await
        }

        async fn complete_streaming(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[ToolDefinition],
            cancel_token: tokio_util::sync::CancellationToken,
        ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            self.inner
                .complete_streaming(messages, system, tools, cancel_token)
                .await
        }
    }

    let mock = Arc::new(FailOnceThenSucceed {
        inner: MockLlmClient::new(vec![MockLlmClient::text_response("Recovered!")]),
        failed_once: std::sync::atomic::AtomicBool::new(false),
        call_count: AtomicUsize::new(0),
    });

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let budget = Arc::new(CountingAllowPlanningBudgetGuard::default());
    let config = AgentConfig {
        circuit_breaker_threshold: 3,
        planning_mode: crate::prompts::PlanningMode::Disabled,
        budget_guard: Some(budget.clone()),
        ..AgentConfig::default()
    };
    let agent = AgentLoop::new(mock.clone(), tool_executor, test_tool_context(), config);
    let result = agent.execute(&[], "Hello", None).await;
    assert!(
        result.is_ok(),
        "should succeed when LLM recovers within threshold: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().text, "Recovered!");
    assert_eq!(
        mock.call_count.load(Ordering::SeqCst),
        2,
        "should have made exactly 2 calls (1 fail + 1 success)"
    );
    assert_eq!(
        budget.checks.load(Ordering::SeqCst),
        2,
        "each provider retry must pass through the budget gateway"
    );
    assert_eq!(budget.records.load(Ordering::SeqCst), 1);
}

// ── Continuation detection tests ─────────────────────────────────────────

#[test]
fn test_looks_incomplete_empty() {
    assert!(AgentLoop::looks_incomplete(""));
    assert!(AgentLoop::looks_incomplete("   "));
}

#[test]
fn test_looks_incomplete_trailing_colon() {
    assert!(AgentLoop::looks_incomplete("Let me check the file:"));
    assert!(AgentLoop::looks_incomplete("Next steps:"));
}

#[test]
fn test_looks_incomplete_ellipsis() {
    assert!(AgentLoop::looks_incomplete("Working on it..."));
    assert!(AgentLoop::looks_incomplete("Processing…"));
}

#[test]
fn test_looks_incomplete_intent_phrases() {
    assert!(AgentLoop::looks_incomplete(
        "I'll start by reading the file."
    ));
    assert!(AgentLoop::looks_incomplete(
        "Let me check the configuration."
    ));
    assert!(AgentLoop::looks_incomplete("I will now run the tests."));
    assert!(AgentLoop::looks_incomplete(
        "I need to update the Cargo.toml."
    ));
}

#[test]
fn test_looks_complete_final_answer() {
    // Clear final answers should NOT trigger continuation
    assert!(!AgentLoop::looks_incomplete(
        "The tests pass. All changes have been applied successfully."
    ));
    assert!(!AgentLoop::looks_incomplete(
        "Done. I've updated the three files and verified the build succeeds."
    ));
    assert!(!AgentLoop::looks_incomplete("42"));
    assert!(!AgentLoop::looks_incomplete("Yes."));
}

#[test]
fn test_looks_incomplete_multiline_complete() {
    let text =
        "Here is the summary:\n\n- Fixed the bug in agent.rs\n- All tests pass\n- Build succeeds";
    assert!(!AgentLoop::looks_incomplete(text));
}
