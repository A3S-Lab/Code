#![cfg(test)]

//! Session send and resume go through the fact log.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::agent_api::tests::test_config;
use crate::fact_control::read_workspace_facts;
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use tokio_util::sync::CancellationToken;

mod child_policy;
mod durable_source;
mod final_delivery;
mod fork_admission;
mod host_contract_prompt;
mod host_reports;
mod loop_source;
mod navigation;
mod work_policy;

struct CountingText {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for CountingText {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LlmResponse {
            message: Message::assistant("session-hello"),
            usage: Default::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("unused")
    }
}

struct CountingStream {
    calls: Arc<AtomicUsize>,
}

fn streamed_response() -> LlmResponse {
    LlmResponse {
        message: Message::assistant("streamed-hello"),
        usage: TokenUsage::default(),
        stop_reason: None,
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl LlmClient for CountingStream {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(streamed_response())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender
                .send(StreamEvent::TextDelta("streamed-hello".into()))
                .await;
            let _ = sender.send(StreamEvent::Done(streamed_response())).await;
        });
        Ok(receiver)
    }
}

#[tokio::test]
async fn fact_log_session_send_and_resume_use_the_stored_model_turn() {
    let calls = Arc::new(AtomicUsize::new(0));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(CountingText {
                calls: Arc::clone(&calls),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-session")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let sent = session.send("hello from the session", None).await.unwrap();
    assert_eq!(sent.text, "session-hello");
    // The turn completion plus one durable-memory extraction.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let facts = read_workspace_facts(workspace.path(), "fact-session").unwrap();
    assert!(facts.iter().any(|fact| fact.kind == "model.turn"));
    let resumed = session.resume_run("ignored-checkpoint").await.unwrap();
    assert_eq!(resumed.text, "session-hello");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

struct SkillCapturingClient {
    systems: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmClient for SkillCapturingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if let Some(system) = system {
            self.systems.lock().unwrap().push(system.to_string());
        }
        Ok(streamed_response())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        if let Some(system) = system {
            self.systems.lock().unwrap().push(system.to_string());
        }
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::Done(streamed_response())).await;
        });
        Ok(receiver)
    }
}

#[tokio::test]
async fn fact_log_session_send_projects_the_skill_catalog() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "extensions-marker-skill".into(),
        description: "Marker skill for the session send path".into(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Follow the marker skill.".into(),
        tags: Vec::new(),
        version: None,
    }));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(SkillCapturingClient {
                systems: Arc::clone(&systems),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-skill")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_skill_registry(registry),
        )
        .unwrap();
    session.send("use the marker skill", None).await.unwrap();
    assert!(
        session
            .skill_names()
            .iter()
            .any(|name| name == "extensions-marker-skill"),
        "the registered skill stays on the session"
    );
    let projected = systems.lock().unwrap().clone();
    assert!(
        projected
            .iter()
            .any(|system| system.contains("source=\"a3s://skills/catalog\"")),
        "session send must project the skill catalog into the model request"
    );

    let empty_systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bare = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(SkillCapturingClient {
                systems: Arc::clone(&empty_systems),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-skill-empty")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_skill_registry(Arc::new(SkillRegistry::new())),
        )
        .unwrap();
    bare.send("hello", None).await.unwrap();
    let empty_systems = empty_systems.lock().unwrap();
    assert!(
        empty_systems
            .iter()
            .all(|system| !system.contains("source=\"a3s://skills/catalog\"")),
        "an empty skill registry must not project a catalog"
    );
}

struct RecordedRequest {
    tools: Vec<String>,
    messages: Vec<Message>,
}

struct ScriptedClient {
    seen: Arc<std::sync::Mutex<Vec<RecordedRequest>>>,
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

fn text_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Message::assistant(text),
        usage: TokenUsage {
            prompt_tokens: 4,
            completion_tokens: 2,
            total_tokens: 6,
            ..TokenUsage::default()
        },
        stop_reason: None,
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.seen.lock().unwrap().push(RecordedRequest {
            tools: tools.iter().map(|tool| tool.name.clone()).collect(),
            messages: messages.to_vec(),
        });
        let mut responses = self.responses.lock().unwrap();
        if responses.len() > 1 {
            Ok(responses.remove(0))
        } else {
            Ok(responses
                .first()
                .cloned()
                .unwrap_or_else(|| text_response("done")))
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("unused")
    }
}

#[tokio::test]
async fn seam_admission_open_run_stays_frozen() {
    let workspace = tempfile::tempdir().unwrap();
    crate::fact_control::reset_session_fact_log(workspace.path());
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(CountingText {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            &crate::SessionOptions::new()
                .with_session_id("seam-admission")
                .with_permission_policy(crate::permissions::policy_for_posture("default"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    assert_ne!(session.session_id, "tui-session");

    let open = super::super::conversation_runtime::FactSession::from(&session)
        .open()
        .expect("first admission");
    let catalog = open.admitted_tool_catalog().to_vec();
    let step_limit = open.admitted_step_limit();
    let empty = serde_json::json!({});
    assert!(open.completion_gate_required());
    assert!(
        catalog.iter().any(|name| name == "write" || name == "bash"),
        "default admission should expose a mutating tool, got {catalog:?}"
    );
    assert_eq!(
        open.admitted_permission().check("bash", &empty),
        crate::permissions::PermissionDecision::Ask
    );
    assert_eq!(
        open.admitted_permission().check("write", &empty),
        crate::permissions::PermissionDecision::Ask
    );

    session
        .set_permission_posture("plan")
        .expect("posture is recorded for the next admission");

    assert_eq!(open.admitted_tool_catalog(), catalog.as_slice());
    assert_eq!(open.admitted_step_limit(), step_limit);
    assert!(open.completion_gate_required());
    assert_eq!(
        open.admitted_permission().check("write", &empty),
        crate::permissions::PermissionDecision::Ask
    );
    assert_eq!(
        open.admitted_permission().check("bash", &empty),
        crate::permissions::PermissionDecision::Ask
    );

    let next = super::super::conversation_runtime::FactSession::from(&session)
        .open()
        .expect("next admission");

    assert_eq!(open.admitted_tool_catalog(), catalog.as_slice());
    assert_eq!(open.admitted_step_limit(), step_limit);
    assert!(open.completion_gate_required());
    assert_eq!(
        open.admitted_permission().check("write", &empty),
        crate::permissions::PermissionDecision::Ask
    );
    assert!(next.completion_gate_required());
    assert_eq!(next.admitted_step_limit(), step_limit);
    assert_eq!(
        next.admitted_permission().check("write", &empty),
        crate::permissions::PermissionDecision::Deny
    );
    assert_eq!(
        next.admitted_permission()
            .check("read", &serde_json::json!({"file_path": "README.md"})),
        crate::permissions::PermissionDecision::Allow
    );
    assert!(
        next.admitted_tool_catalog()
            .iter()
            .all(|name| name != "write" && name != "bash"),
        "plan admission hides denied mutations, got {:?}",
        next.admitted_tool_catalog()
    );
    assert!(
        next.admitted_tool_catalog()
            .iter()
            .any(|name| name == "read"),
        "plan admission keeps read, got {:?}",
        next.admitted_tool_catalog()
    );
}

fn session_for(
    agent: &crate::Agent,
    workspace: &std::path::Path,
    session_id: &str,
    client: Arc<ScriptedClient>,
    options: &crate::SessionOptions,
) -> crate::AgentSession {
    agent
        .build_session(
            workspace.to_string_lossy().to_string(),
            client,
            &options
                .clone()
                .with_session_id(session_id.to_string())
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap()
}

fn request_text(messages: &[Message]) -> String {
    let mut text = String::new();
    for message in messages {
        text.push_str(&message.text());
        for block in &message.content {
            if let crate::llm::ContentBlock::ToolResult { content, .. } = block {
                text.push_str(&content.as_text());
            }
        }
    }
    text
}

fn request_has_image_bytes(messages: &[Message], png: &[u8]) -> bool {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png);
    messages.iter().any(|message| {
        message.content.iter().any(|block| match block {
            crate::llm::ContentBlock::ToolResult { content, .. } => match content {
                crate::llm::ToolResultContentField::Blocks(blocks) => blocks.iter().any(|block| {
                    matches!(
                        block,
                        crate::llm::ToolResultContent::Image { source } if source.data == encoded
                    )
                }),
                crate::llm::ToolResultContentField::Text(_) => false,
            },
            crate::llm::ContentBlock::Image { source } => source.data == encoded,
            _ => false,
        })
    })
}

fn assert_image_payload(label: &str, seen: &[RecordedRequest], png: &[u8]) {
    let carried = seen.iter().any(|request| {
        let text = request_text(&request.messages);
        text.contains("[Image: image/png") && request_has_image_bytes(&request.messages, png)
    });
    let debug = seen
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let text = request_text(&request.messages);
            format!(
                "#{index} tools={:?} placeholder={} bytes={} text={}",
                request.tools,
                text.contains("[Image: image/png"),
                request_has_image_bytes(&request.messages, png),
                text.chars().take(500).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        carried,
        "{label}: a later model request must include the read image bytes, not only the placeholder\n{debug}"
    );
}

fn turn_bounds(events: &[crate::run::RunEventRecord]) -> (usize, usize) {
    let starts = events
        .iter()
        .filter(|record| matches!(record.event, crate::AgentEvent::TurnStart { .. }))
        .count();
    let ends = events
        .iter()
        .filter(|record| matches!(record.event, crate::AgentEvent::TurnEnd { .. }))
        .count();
    (starts, ends)
}

#[tokio::test]
async fn fact_log_hides_tools_a_deny_by_default_policy_does_not_expose() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let mut policy = crate::permissions::PermissionPolicy::new().allow("read*");
    policy.default_decision = crate::permissions::PermissionDecision::Deny;
    let session = session_for(
        &agent,
        workspace.path(),
        "fact-permission",
        Arc::new(ScriptedClient {
            seen: Arc::clone(&seen),
            responses: std::sync::Mutex::new(vec![text_response("visible")]),
        }),
        &crate::SessionOptions::new().with_permission_policy(policy),
    );
    let sent = session.send("which tools can you use", None).await.unwrap();
    assert_eq!(sent.text, "visible");
    let seen = seen.lock().unwrap();
    let tools = seen
        .iter()
        .find(|request| request_text(&request.messages).contains("which tools can you use"))
        .expect("the fact-loop request")
        .tools
        .clone();
    assert!(
        tools.iter().any(|name| name == "read"),
        "an allowed tool stays on the model list: {tools:?}"
    );
    assert!(
        !tools.iter().any(|name| name == "bash"),
        "a deny-by-default policy must omit bash before the model call: {tools:?}"
    );
}

#[tokio::test]
async fn fact_log_facade_emits_turn_start_and_turn_end() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let stream = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(CountingStream {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-turn-stream")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let (mut events, handle) = stream.stream("hello from the stream", None).await.unwrap();
    let mut streamed = Vec::new();
    while let Some(event) = events.recv().await {
        streamed.push(event);
    }
    handle.await.unwrap();
    assert!(
        streamed
            .iter()
            .any(|event| matches!(event, crate::AgentEvent::TurnStart { .. })),
        "stream must emit TurnStart"
    );
    assert!(
        streamed
            .iter()
            .any(|event| matches!(event, crate::AgentEvent::TurnEnd { .. })),
        "stream must emit TurnEnd"
    );

    let sent_workspace = tempfile::tempdir().unwrap();
    let sent = agent
        .build_session(
            sent_workspace.path().to_string_lossy().to_string(),
            Arc::new(CountingText {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-turn-send")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    sent.send("hello from the session", None).await.unwrap();
    let runs = sent.runs().await;
    let run_id = runs.last().expect("send records a run").id.clone();
    let (starts, ends) = turn_bounds(&sent.run_events(&run_id).await);
    assert!(starts >= 1, "send run events must include TurnStart");
    assert!(ends >= 1, "send run events must include TurnEnd");
}

#[tokio::test]
async fn fact_log_resume_run_emits_turn_boundaries_for_the_continued_model_turn() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note.txt"), "hello").unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let mut harness = crate::HarnessComposeOptions::compose(
        vec![
            "system".into(),
            "tools".into(),
            "budget".into(),
            "compact".into(),
            "infer".into(),
        ],
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    harness.step_limit = Some(2);
    harness.model_attempts = Some(1);
    let session = session_for(
        &agent,
        workspace.path(),
        "fact-resume-turn",
        Arc::new(ScriptedClient {
            seen: Arc::new(std::sync::Mutex::new(Vec::new())),
            responses: std::sync::Mutex::new(vec![
                tool_response(
                    "read-note",
                    "read",
                    serde_json::json!({"file_path": "note.txt"}),
                ),
                text_response("after-tool"),
            ]),
        }),
        &crate::SessionOptions::new()
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
            .with_harness(harness),
    );
    let error = session
        .send("read the note", None)
        .await
        .expect_err("step limit stops the run before the follow-up infer");
    assert!(
        format!("{error:#}").contains("StepLimit"),
        "send must stop on the harness step limit, got {error:#}"
    );
    let runs = session.runs().await;
    let run_id = runs.last().expect("the stopped run is recorded").id.clone();
    let before_bounds = turn_bounds(&session.run_events(&run_id).await);
    let resumed = session
        .resume_run("ignored-checkpoint")
        .await
        .expect("resume continues the stored model turn");
    assert_eq!(resumed.text, "after-tool");
    // Ordinary resume owns a fresh executing Run; it must not append the new
    // model turn to the historical Run that stopped on its step limit.
    let runs = session.runs().await;
    assert_eq!(runs.len(), 2);
    let resumed_run = runs.iter().find(|run| run.id != run_id).unwrap();
    assert_eq!(resumed_run.status, crate::run::RunStatus::Completed);
    let (starts, ends) = turn_bounds(&session.run_events(&resumed_run.id).await);
    assert!(starts >= 1, "the executing resume Run must emit TurnStart");
    assert!(ends >= 1, "the executing resume Run must emit TurnEnd");
    assert_eq!(
        turn_bounds(&session.run_events(&run_id).await),
        before_bounds
    );
}

#[tokio::test]
async fn fact_log_read_image_bytes_reach_the_next_request_and_restored_history() {
    let png = b"PNG-FACT-LOG-BYTES-184".to_vec();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("pixel.png"), &png).unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let policy = crate::permissions::PermissionPolicy::new().allow("*");
    let session = session_for(
        &agent,
        workspace.path(),
        "fact-image",
        Arc::new(ScriptedClient {
            seen: Arc::clone(&seen),
            responses: std::sync::Mutex::new(vec![
                tool_response(
                    "read-pixel",
                    "read",
                    serde_json::json!({"file_path": "pixel.png"}),
                ),
                text_response("saw-image"),
            ]),
        }),
        &crate::SessionOptions::new().with_permission_policy(policy.clone()),
    );
    session.send("look at the image", None).await.unwrap();
    assert_image_payload("live", &seen.lock().unwrap(), &png);
    drop(session);

    let restored = Arc::new(std::sync::Mutex::new(Vec::new()));
    let restored_agent = crate::Agent::from_config(test_config()).await.unwrap();
    let again = session_for(
        &restored_agent,
        workspace.path(),
        "fact-image",
        Arc::new(ScriptedClient {
            seen: Arc::clone(&restored),
            responses: std::sync::Mutex::new(vec![text_response("still-there")]),
        }),
        &crate::SessionOptions::new().with_permission_policy(policy),
    );
    again.send("describe it again", None).await.unwrap();
    assert_image_payload("restored", &restored.lock().unwrap(), &png);

    let seeded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let history_workspace = tempfile::tempdir().unwrap();
    let attachment = crate::llm::Attachment::new(png.clone(), "image/png");
    let history = vec![Message::tool_result_with_images(
        "call-img",
        "[Image: image/png (22 bytes)]",
        &[attachment],
        false,
    )];
    let seeded_session = session_for(
        &agent,
        history_workspace.path(),
        "fact-image-seed",
        Arc::new(ScriptedClient {
            seen: Arc::clone(&seeded),
            responses: std::sync::Mutex::new(vec![text_response("from-history")]),
        }),
        &crate::SessionOptions::new(),
    );
    let error = seeded_session
        .send("what is in the image", Some(&history))
        .await
        .expect_err(
            "imported tool output without an invocation receipt is not completion authority",
        );
    assert!(error.to_string().contains("tool_effect_unresolved"));
    assert_image_payload("seeded", &seeded.lock().unwrap(), &png);
}

struct WireErrorTool;

#[async_trait]
impl crate::tools::Tool for WireErrorTool {
    fn name(&self) -> &str {
        "probe_mcp"
    }

    fn description(&self) -> &str {
        "Return one MCP tool result"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        let result: crate::mcp::CallToolResult = serde_json::from_str(
            r#"{"content":[{"type":"text","text":"refused"}],"isError":true}"#,
        )?;
        crate::mcp::project_tool_result("probe_mcp", &result, ctx).await
    }
}

#[tokio::test]
async fn fact_log_mcp_is_error_reaches_the_model_tool_result() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = session_for(
        &agent,
        workspace.path(),
        "fact-mcp-error",
        Arc::new(ScriptedClient {
            seen: Arc::clone(&seen),
            responses: std::sync::Mutex::new(vec![
                tool_response("mcp-1", "probe_mcp", serde_json::json!({})),
                text_response("noted"),
            ]),
        }),
        &crate::SessionOptions::new()
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*")),
    );
    session
        .register_dynamic_tool(Arc::new(WireErrorTool))
        .unwrap();
    session.send("call the probe", None).await.unwrap();
    let seen = seen.lock().unwrap();
    let refused = seen.iter().find(|request| {
        request.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    crate::llm::ContentBlock::ToolResult { content, .. }
                        if content.as_text().contains("refused")
                )
            })
        })
    });
    let refused = refused.expect("the MCP result must be on the next model request");
    assert!(
        refused.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    crate::llm::ContentBlock::ToolResult { is_error: Some(true), content, .. }
                        if content.as_text().contains("refused")
                )
            })
        }),
        "wire isError: true must set the model-facing tool result error flag"
    );
}

#[tokio::test]
async fn fact_log_session_stream_appends_the_model_turn_and_resume_does_not_resend_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(CountingStream {
                calls: Arc::clone(&calls),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-stream")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let (mut events, handle) = session.stream("hello from the stream", None).await.unwrap();
    while events.recv().await.is_some() {}
    handle.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let facts = read_workspace_facts(workspace.path(), "fact-stream").unwrap();
    assert!(facts.iter().any(|fact| fact.kind == "model.turn"));
    let resumed = session.resume_run("ignored-checkpoint").await.unwrap();
    assert_eq!(resumed.text, "streamed-hello");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// A verified tool result may skip the provider only while that tool result is
/// the latest folded message. The next `user.message` is a new turn.
#[tokio::test]
async fn fact_log_session_new_user_message_calls_the_model_after_a_verified_tool() {
    let coding_prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let workspace = tempfile::tempdir().unwrap();
    // This pins turn/verification protocol, not shell or native containment.
    // The write tool still produces real bytes, checked independently below.
    let sandbox = Arc::new(
        crate::tools::builtin::background::test_backend::DeterministicBackgroundSandbox::new(
            workspace.path().to_path_buf(),
            "protocol-file-exists",
            0,
            false,
        ),
    );
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let policy = crate::permissions::PermissionPolicy {
        default_decision: crate::permissions::PermissionDecision::Allow,
        ..Default::default()
    };
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(VerifiedThenTextClient {
                coding_prompts: Arc::clone(&coding_prompts),
                coding_calls: AtomicUsize::new(0),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-verified-next-user")
                .with_sandbox_handle(Arc::clone(&sandbox) as Arc<dyn crate::sandbox::BashSandbox>)
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_permission_policy(policy),
        )
        .unwrap();

    let first = session
        .send("write hello.txt then verify it exists", None)
        .await
        .unwrap();
    assert_eq!(
        first.text, "completed",
        "a verified tool result should settle that completion without another provider call"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("hello.txt")).unwrap_or_default(),
        "hello"
    );
    assert!(matches!(
        first.completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));
    assert_eq!(sandbox.calls(), 1);
    assert!(sandbox.was_released());

    let second = session
        .send("now read hello.txt and answer", None)
        .await
        .unwrap();
    assert_eq!(second.text, "second-turn");
    assert_eq!(sandbox.calls(), 1, "the new turn must not replay the tool");
    let prompts = coding_prompts.lock().unwrap();
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt.contains("now read hello.txt")),
        "the new user message must reach the model, prompts={prompts:?}"
    );
}

struct VerifiedThenTextClient {
    coding_prompts: Arc<std::sync::Mutex<Vec<String>>>,
    coding_calls: AtomicUsize,
}

fn tool_response(id: &str, name: &str, input: serde_json::Value) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![crate::llm::ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Product,
        },
        usage: TokenUsage::default(),
        stop_reason: None,
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl LlmClient for VerifiedThenTextClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if tools.is_empty() {
            return Ok(LlmResponse {
                message: Message::assistant("[]"),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: Vec::new(),
                meta: None,
            });
        }
        let prompt = messages
            .iter()
            .map(Message::text)
            .collect::<Vec<_>>()
            .join("\n");
        self.coding_prompts.lock().unwrap().push(prompt);
        let index = self.coding_calls.fetch_add(1, Ordering::SeqCst);
        let response = match index {
            0 => tool_response(
                "write-1",
                "write",
                serde_json::json!({ "file_path": "hello.txt", "content": "hello" }),
            ),
            1 => tool_response(
                "bash-1",
                "bash",
                serde_json::json!({ "command": "test -f hello.txt" }),
            ),
            _ => LlmResponse {
                message: Message::assistant("second-turn"),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: Vec::new(),
                meta: None,
            },
        };
        Ok(response)
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("use complete")
    }
}

/// A natural-language remember request grants no write authority when the
/// extraction model does not return valid memory JSON. This explicitly retires
/// the previous raw-input fallback promise while retaining its exact fixture.
#[tokio::test]
async fn fact_log_session_explicit_preference_fails_closed_when_extraction_json_fails() {
    let token = "REMEMBER-TOKEN-4411";
    let store: Arc<dyn a3s_memory::MemoryStore> = Arc::new(a3s_memory::InMemoryStore::new());
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(ProseExtractionClient),
            &crate::SessionOptions::new()
                .with_session_id("fact-remember-preference")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_memory(store.clone()),
        )
        .unwrap();
    let prompt = format!(
        "Please remember this durable workspace preference for future sessions: \
         always use the verification codename {token}."
    );
    session.send(&prompt, None).await.unwrap();
    let hits = a3s_memory::MemoryStore::search(store.as_ref(), token, 5)
        .await
        .unwrap();
    assert!(
        hits.is_empty(),
        "invalid extraction must not write, hits={hits:?}"
    );
    assert_eq!(store.count().await.unwrap(), 0);
}

struct ProseExtractionClient;

#[async_trait]
impl LlmClient for ProseExtractionClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            message: Message::assistant("I will keep that in mind."),
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let response = LlmResponse {
                message: Message::assistant("Understood. I will use that codename."),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: Vec::new(),
                meta: None,
            };
            let _ = sender
                .send(StreamEvent::TextDelta(
                    "Understood. I will use that codename.".into(),
                ))
                .await;
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

struct AttachmentSteerClient {
    text: CountingText,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    seen: std::sync::Mutex<Vec<Vec<Message>>>,
}

#[async_trait]
impl LlmClient for AttachmentSteerClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if !tools.is_empty() {
            self.seen.lock().unwrap().push(messages.to_vec());
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        self.text.complete(messages, system, tools).await
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("blocking attachment fixture must use complete")
    }
}

#[tokio::test]
async fn fact_log_session_attachments_steer_and_history_stay_on_the_log() {
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(AttachmentSteerClient {
        text: CountingText {
            calls: calls.clone(),
        },
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        seen: Default::default(),
    });
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            client.clone(),
            &crate::SessionOptions::new()
                .with_session_id("fact-attach")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let history = vec![Message::user("earlier"), Message::assistant("stored reply")];
    let attachments = [crate::llm::Attachment::png(vec![1, 2, 3])];
    let send = session.send_with_attachments("see this", &attachments, Some(&history));
    tokio::pin!(send);
    tokio::select! {
        permit = client.entered.acquire() => permit.unwrap().forget(),
        result = &mut send => panic!("send ended before the active input boundary: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("model was not entered"),
    }
    let run = session.current_run().await.unwrap();
    let mut request = crate::run_control::SteerRequest::new("turn left").with_run_id(run.id());
    request.request_id = Some("attachment-steer".into());
    let retry = request.clone().into_protocol(session.id(), run.id());
    let inbox = super::super::RunControlState::from_session(&session)
        .current_run_control()
        .await
        .unwrap();
    let steered = session.steer(request).await.unwrap();
    assert_eq!(
        steered.state,
        crate::run_control::RunControlReceiptState::Accepted
    );
    client.release.add_permits(1);
    let sent = tokio::time::timeout(std::time::Duration::from_secs(5), &mut send)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent.text, "session-hello");
    {
        let requests = client.seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(request_has_image_bytes(&requests[0], &[1, 2, 3]));
        let text = request_text(&requests[0]);
        assert!(text.contains("earlier"));
        assert!(text.contains("stored reply"));
        assert!(text.contains("see this"));
    }
    let steered = inbox.submit(retry, 1).await.unwrap();
    assert_eq!(
        steered.state,
        crate::run_control::RunControlReceiptState::Applied
    );
    assert!(!session
        .confirm_tool_use("missing", true, None)
        .await
        .unwrap());
    let facts = read_workspace_facts(workspace.path(), "fact-attach").unwrap();
    let users: Vec<_> = facts
        .iter()
        .filter(|fact| fact.kind == "user.message")
        .collect();
    assert_eq!(users.len(), 3);
    assert_eq!(users[0].payload["text"], "earlier");
    assert!(users[1].payload["text"]
        .as_str()
        .unwrap()
        .contains("see this"));
    assert_eq!(users[2].payload["text"], "turn left");
    let stored = facts
        .iter()
        .find(|fact| fact.cause.as_deref() == Some("infer:1:0"))
        .expect("history model turn");
    assert_eq!(stored.payload["text"], "stored reply");
    assert!(facts
        .iter()
        .all(|fact| fact.kind != "confirmation.answered"));
    // Terminal Steer is retained as the next input, not inferred by a new
    // unowned call after the original Run has completed.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

struct StandingClient {
    calls: Arc<AtomicUsize>,
    systems: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmClient for StandingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.systems
            .lock()
            .unwrap()
            .push(system.unwrap_or("").to_string());
        Ok(LlmResponse {
            message: Message::assistant("ok"),
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("unused")
    }
}

#[tokio::test]
async fn standing_context_stays_the_admitted_projection() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("instructions.md"),
        "OPEN-TIME-INSTRUCTIONS\n",
    )
    .unwrap();
    crate::fact_control::reset_session_fact_log(workspace.path());
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(StandingClient {
                calls: Arc::clone(&calls),
                systems: Arc::clone(&systems),
            }),
            &crate::SessionOptions::new()
                .with_session_id("standing-seal")
                .with_permission_policy(crate::permissions::policy_for_posture("default"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let open = super::super::conversation_runtime::FactSession::from(&session)
        .open()
        .unwrap();
    let digest = open.standing_context_digest().unwrap().to_string();
    let payload = open
        .read_facts()
        .unwrap()
        .into_iter()
        .find(|fact| fact.kind == crate::standing_context::STANDING_CONTEXT_FACT_KIND)
        .unwrap()
        .payload
        .to_string();
    assert!(payload.contains(&digest));
    assert!(!payload.contains("OPEN-TIME-INSTRUCTIONS"));

    std::fs::write(
        workspace.path().join("instructions.md"),
        "CHANGED-INSTRUCTIONS\n",
    )
    .unwrap();
    open.user_text("ping").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let seen = systems.lock().unwrap();
    assert!(seen[0].contains("OPEN-TIME-INSTRUCTIONS"));
    assert!(!seen[0].contains("CHANGED-INSTRUCTIONS"));
    drop(seen);
    drop(open);

    let mismatch = super::super::conversation_runtime::FactSession::from(&session)
        .open()
        .err()
        .expect("changed instructions fail closed");
    assert!(mismatch
        .to_string()
        .contains("standing context digest mismatch"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    std::fs::write(
        workspace.path().join("instructions.md"),
        "OPEN-TIME-INSTRUCTIONS\n",
    )
    .unwrap();
    let resumed = super::super::conversation_runtime::FactSession::from(&session)
        .open()
        .unwrap();
    assert_eq!(resumed.standing_context_digest(), Some(digest.as_str()));
}

#[tokio::test]
async fn a_new_run_without_a_standing_digest_does_not_call_the_model() {
    use a3s_effect::LogStore;
    let workspace = tempfile::tempdir().unwrap();
    crate::fact_control::reset_session_fact_log(workspace.path());
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    a3s_effect::FileLog::open(&log_dir)
        .unwrap()
        .append(
            "no-digest",
            &[a3s_effect::NewFact {
                kind: crate::standing_context::STANDING_CONTEXT_FACT_KIND.to_string(),
                key: "standing-context".into(),
                payload: serde_json::json!({}),
            }],
            None,
        )
        .unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(StandingClient {
                calls: Arc::clone(&calls),
                systems: Arc::new(std::sync::Mutex::new(Vec::new())),
            }),
            &crate::SessionOptions::new()
                .with_session_id("no-digest")
                .with_permission_policy(crate::permissions::policy_for_posture("default"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let opened = super::super::conversation_runtime::FactSession::from(&session).open();
    let error = opened.err().expect("missing digest fails closed");
    assert!(error.to_string().contains("missing a digest"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Records the user-role texts of every model request, so a fork's effect on
/// the replayed context is observable at the provider boundary.
struct ForkRecordingClient {
    requests: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl LlmClient for ForkRecordingClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let users = messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.text())
            .collect();
        self.requests.lock().unwrap().push(users);
        Ok(text_response("fork-ok"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("unused")
    }
}

#[tokio::test]
async fn fork_session_at_discards_later_facts_and_the_next_turn_replays_the_prefix() {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(ForkRecordingClient {
                requests: Arc::clone(&requests),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-fork")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    session.send("alpha prompt", None).await.unwrap();
    session.send("beta prompt", None).await.unwrap();
    let facts = read_workspace_facts(workspace.path(), "fact-fork").unwrap();
    let beta = facts
        .iter()
        .find(|fact| {
            fact.kind == "user.message" && fact.payload["text"].as_str() == Some("beta prompt")
        })
        .expect("the beta turn is on the log");
    let cut = beta.seq - 1;
    let retained = facts.iter().filter(|fact| fact.seq <= cut).count();
    let removed = session.fork_session_at(cut).await.unwrap();
    assert_eq!(removed, (facts.len() - retained) as u64);
    assert_eq!(
        session.fork_session_at(cut).await.unwrap(),
        0,
        "a second cut at the same seq removes nothing"
    );
    let after = read_workspace_facts(workspace.path(), "fact-fork").unwrap();
    assert_eq!(after.len(), retained);
    assert!(after.iter().all(|fact| fact.seq <= cut));
    assert!(
        session
            .history()
            .iter()
            .all(|message| !message.text().contains("beta prompt")),
        "in-memory history must drop the discarded turn"
    );

    requests.lock().unwrap().clear();
    let sent = session.send("gamma prompt", None).await.unwrap();
    assert_eq!(sent.text, "fork-ok");
    let seen = requests.lock().unwrap();
    assert!(
        seen.iter()
            .flatten()
            .all(|text| !text.contains("beta prompt")),
        "no post-fork model request may see the discarded turn"
    );
    assert!(
        seen.iter()
            .any(|users| users.iter().any(|text| text.contains("alpha prompt"))),
        "the retained prefix must stay visible, requests={seen:?}"
    );
    assert!(
        seen.iter()
            .any(|users| users.iter().any(|text| text.contains("gamma prompt"))),
        "the new prompt must reach the model, requests={seen:?}"
    );
}

#[tokio::test]
async fn fork_session_at_refuses_while_a_run_is_streaming() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(super::CancellableStreamingClient::new("partial answer")),
            &crate::SessionOptions::new()
                .with_session_id("fact-fork-busy")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, crate::AgentEvent::TextDelta { .. }) {
            break;
        }
    }
    let busy = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        session.fork_session_at(1),
    )
    .await
    .expect("busy admission is fail-fast")
    .unwrap_err();
    assert!(matches!(
        busy,
        crate::error::CodeError::SessionBusy { ref session_id } if session_id == "fact-fork-busy"
    ));
    assert!(session.cancel().await);
    while rx.recv().await.is_some() {}
    handle.await.unwrap();
    assert_eq!(
        session.fork_session_at(u64::MAX).await.unwrap(),
        0,
        "once the run settles, the same fork proceeds"
    );
}

/// A context provider with interior-mutable content, so a test can flip a
/// source between turns and watch the recorded section digest track it.
struct StaticProbeProvider {
    name: String,
    content: Arc<std::sync::Mutex<String>>,
}

#[async_trait]
impl crate::context::ContextProvider for StaticProbeProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn query(
        &self,
        _query: &crate::context::ContextQuery,
    ) -> anyhow::Result<crate::context::ContextResult> {
        let content = self
            .content
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut result = crate::context::ContextResult::new(&self.name);
        result.add_item(crate::context::ContextItem::new(
            format!("{}-item", self.name),
            crate::context::ContextType::Resource,
            content,
        ));
        Ok(result)
    }
}

/// Digested `(provider, digest)` pairs from the last `context.sections` fact.
fn last_context_sections(facts: &[a3s_effect::Fact]) -> Vec<(String, String)> {
    let Some(fact) = facts.iter().rfind(|fact| fact.kind == "context.sections") else {
        panic!("no context.sections fact recorded");
    };
    fact.payload["sections"]
        .as_array()
        .expect("sections array")
        .iter()
        .map(|entry| {
            (
                entry["provider"].as_str().expect("provider").to_string(),
                entry["digest"].as_str().expect("digest").to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn context_sections_are_recorded_stable_and_source_tracked() {
    let calls = Arc::new(AtomicUsize::new(0));
    let content = Arc::new(std::sync::Mutex::new("probe content v1".to_string()));
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(CountingText {
                calls: Arc::clone(&calls),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-ctx-sections")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_context_provider(Arc::new(StaticProbeProvider {
                    name: "probe".to_string(),
                    content: Arc::clone(&content),
                })),
        )
        .unwrap();

    session.send("first turn", None).await.unwrap();
    let facts = read_workspace_facts(workspace.path(), "fact-ctx-sections").unwrap();
    let context_fact = facts
        .iter()
        .find(|fact| fact.kind == "context.sections")
        .expect("the first model call records its context sections");
    let first_turn = facts
        .iter()
        .find(|fact| fact.kind == "model.turn")
        .expect("a model turn");
    assert!(
        context_fact.seq < first_turn.seq,
        "the context record precedes the model call it describes"
    );
    let first = last_context_sections(&facts);
    assert_eq!(first.len(), 1, "one provider, one section: {first:?}");
    assert_eq!(first[0].0, "probe");

    // Unchanged sources: the next turn assembles the same section bytes,
    // and the recorded digest proves the stability.
    session.send("second turn", None).await.unwrap();
    let facts = read_workspace_facts(workspace.path(), "fact-ctx-sections").unwrap();
    let second = last_context_sections(&facts);
    assert_eq!(second, first, "unchanged sources must digest identically");

    // A mutated source changes the recorded digest — the replayable record
    // tracks content, not just provider identity.
    *content
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = "probe content v2".to_string();
    session.send("third turn", None).await.unwrap();
    let facts = read_workspace_facts(workspace.path(), "fact-ctx-sections").unwrap();
    let third = last_context_sections(&facts);
    assert_eq!(third[0].0, "probe");
    assert_ne!(
        third[0].1, first[0].1,
        "a changed source must digest differently"
    );
}
