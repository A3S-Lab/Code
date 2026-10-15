//! WC-INJECT1 hermetic pin: path-scoped instruction fragments inject on
//! the FACT spine (the default session surface), not only the legacy loop.
//! A `read` under `src/**` targets a matching path; the next model input
//! carries the budget-cut fragment. The live A/B twin is
//! `tests/test_injection_budget_real_llm.rs`.

use super::*;

const CODENAME: &str = "XYLOPHONE-4471";

fn src_rule() -> crate::path_instructions::PathRule {
    let mut text = String::from("Workspace path instructions for src work:\n");
    let lines = [
        "Keep module boundaries boring; prefer plain functions over traits.",
        "Every error path returns a typed error; no stringly failure modes.",
        "Naming follows the closed set: parse and as_str stay paired.",
        "A budget is a fact, not a suggestion; every queue has a ceiling.",
    ];
    let mut index = 0usize;
    while text.len() < 5 * 1024 {
        text.push_str(&format!(
            "{}. Item {}.\n",
            lines[index % lines.len()],
            index
        ));
        index += 1;
    }
    text.push_str(&format!(
        "The workspace codename for src work is {CODENAME}.\n"
    ));
    crate::path_instructions::PathRule {
        glob: "src/**".into(),
        text,
    }
}

struct ReadThenTextClient {
    calls: std::sync::atomic::AtomicUsize,
    rendered: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for ReadThenTextClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let rendered = messages
            .iter()
            .map(|message| message.text())
            .collect::<Vec<_>>()
            .join("\n");
        self.rendered.lock().expect("rendered").push(rendered);
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let message = if call == 0 {
            Message {
                role: "assistant".into(),
                content: vec![crate::llm::ContentBlock::ToolUse {
                    id: "read-1".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "file_path": "src/lib.rs" }),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            }
        } else {
            Message::assistant("done")
        };
        Ok(LlmResponse {
            message,
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
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this pin")
    }
}

async fn run_fact_session(budget: Option<usize>) -> Vec<String> {
    let workspace = tempfile::tempdir().unwrap();
    let src = workspace.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "pub fn depot() {}\n").unwrap();

    let client = Arc::new(ReadThenTextClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        rendered: std::sync::Mutex::new(Vec::new()),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let mut options = SessionOptions::new()
        .with_session_id("path-inject-fact")
        .with_planning_mode(crate::prompts::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_path_rules(vec![src_rule()]);
    if let Some(budget) = budget {
        options = options.with_injection_budget(budget);
    }
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &options,
        )
        .unwrap();
    let result = session
        .send("Read src/lib.rs and reply with its contents.", None)
        .await
        .expect("fact turn");
    assert!(!result.text.is_empty());

    let rendered = client.rendered.lock().expect("rendered").clone();
    assert!(
        rendered.len() >= 2,
        "the pin expects a tool round then a text round"
    );
    // Call 0 emits the read; call 1 is the model turn that must see the
    // fragment. Any later captured calls are post-turn machinery (memory
    // extraction) that never passes through the completion injection.
    rendered
}

#[tokio::test]
async fn fact_spine_injects_the_full_fragment_under_a_large_budget() {
    let rendered = run_fact_session(Some(16 * 1024)).await;
    let after_tool = &rendered[1];
    assert!(
        after_tool.contains(CODENAME),
        "the 16 KiB budget must keep the tail codename in the fact-path model input"
    );
}

#[tokio::test]
async fn fact_spine_cuts_the_tail_under_the_default_budget() {
    let rendered = run_fact_session(None).await;
    let after_tool = &rendered[1];
    assert!(
        after_tool.contains("[path instructions digest="),
        "the matching rule must inject a fragment on the fact path"
    );
    assert!(
        !after_tool.contains(CODENAME),
        "the 4 KiB default cut must drop the tail codename"
    );
}

#[tokio::test]
async fn fact_spine_injects_nothing_when_no_rule_matches_the_target() {
    // A rule scoped to `docs/**` while the turn targets `src/lib.rs`:
    // no match is an empty injection, not an error.
    let workspace = tempfile::tempdir().unwrap();
    let src = workspace.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "pub fn depot() {}\n").unwrap();

    let client = Arc::new(ReadThenTextClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        rendered: std::sync::Mutex::new(Vec::new()),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let mut non_matching = src_rule();
    non_matching.glob = "docs/**".into();
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("path-inject-fact-nomatch")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_injection_budget(16 * 1024)
                .with_path_rules(vec![non_matching]),
        )
        .unwrap();
    let result = session
        .send("Read src/lib.rs and reply with its contents.", None)
        .await
        .expect("fact turn");
    assert!(!result.text.is_empty());
    let rendered = client.rendered.lock().expect("rendered").clone();
    let after_tool = &rendered[1];
    assert!(
        !after_tool.contains("[path instructions digest="),
        "a rule that matches no targeted path must inject nothing"
    );
}
