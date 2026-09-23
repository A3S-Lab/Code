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
