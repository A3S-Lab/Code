//! Session send and resume go through the fact log.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::agent_api::tests::test_config;
use crate::fact_control::read_workspace_facts;
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use tokio_util::sync::CancellationToken;

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
    let (before_starts, _) = turn_bounds(&session.run_events(&run_id).await);
    let resumed = session
        .resume_run("ignored-checkpoint")
        .await
        .expect("resume continues the stored model turn");
    assert_eq!(resumed.text, "after-tool");
    let (after_starts, after_ends) = turn_bounds(&session.run_events(&run_id).await);
    assert!(
        after_starts > before_starts,
        "resume_run must emit TurnStart for the model turn it performs"
    );
    assert!(after_ends >= 1, "resume_run must emit TurnEnd");
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
    seeded_session
        .send("what is in the image", Some(&history))
        .await
        .unwrap();
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
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let mut policy = crate::permissions::PermissionPolicy::default();
    policy.default_decision = crate::permissions::PermissionDecision::Allow;
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(VerifiedThenTextClient {
                coding_prompts: Arc::clone(&coding_prompts),
                coding_calls: AtomicUsize::new(0),
            }),
            &crate::SessionOptions::new()
                .with_session_id("fact-verified-next-user")
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

    let second = session
        .send("now read hello.txt and answer", None)
        .await
        .unwrap();
    assert_eq!(second.text, "second-turn");
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

/// An explicit remember request still lands in the session store when the
/// extraction model does not return memory JSON.
#[tokio::test]
async fn fact_log_session_explicit_preference_is_stored_when_extraction_json_fails() {
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
        hits.iter().any(|item| item.content.contains(token)),
        "explicit preference must be stored, hits={hits:?}"
    );
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

#[tokio::test]
async fn fact_log_session_attachments_steer_and_history_stay_on_the_log() {
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
                .with_session_id("fact-attach")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let history = vec![Message::user("earlier"), Message::assistant("stored reply")];
    let sent = session
        .send_with_attachments(
            "see this",
            &[crate::llm::Attachment::png(vec![1, 2, 3])],
            Some(&history),
        )
        .await
        .unwrap();
    assert_eq!(sent.text, "session-hello");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let steered = session
        .steer(crate::run_control::SteerRequest::new("turn left"))
        .await
        .unwrap();
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
    // Steer is another user message, so it performs one new completion.
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
