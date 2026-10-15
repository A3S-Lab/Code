//! WC-WTREE1 hermetic twin: the Run that creates a worktree removes it at
//! settlement — session-level, no LLM. The live twin is
//! `tests/test_worktree_run_real_llm.rs`; this pin keeps the whole chain
//! (capability registration on the fact spine → scope-close settlement →
//! effect removal) inside the hermetic suite.

use super::*;

struct WorktreeThenTextClient {
    calls: std::sync::atomic::AtomicUsize,
    branch: String,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for WorktreeThenTextClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let message = if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Message {
                role: "assistant".into(),
                content: vec![crate::llm::ContentBlock::ToolUse {
                    id: "git-1".into(),
                    name: "git".into(),
                    input: serde_json::json!({
                        "command": "worktree",
                        "subcommand": "create",
                        "name": self.branch,
                    }),
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

fn git(path: &std::path::Path, args: &[&str]) {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(path).args(args);
    let output = command.output().expect("git spawns");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn the_run_removes_its_worktree_at_settlement_without_an_llm() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "wt@a3s.local"]);
    git(&root, &["config", "user.name", "WT Settlement"]);
    std::fs::write(root.join("README.md"), "settlement twin\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-q", "-m", "base"]);

    let branch = format!("wt-hermetic-{}", uuid::Uuid::new_v4().simple());
    // The provider's default: a sibling of the workspace root named
    // "{repo_name}-{branch}".
    let expected = root.parent().unwrap().join(format!(
        "{}-{branch}",
        root.file_name().unwrap().to_string_lossy()
    ));

    let client = Arc::new(WorktreeThenTextClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        branch: branch.clone(),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("worktree-settlement-twin")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false),
        )
        .unwrap();

    let result = session
        .send("Create the worktree, then reply done.", None)
        .await
        .expect("fact turn");
    assert!(!result.text.is_empty());

    // The creation happened: the fact log records the tool result naming
    // the worktree path.
    let facts =
        crate::fact_control::read_workspace_facts(&root, "worktree-settlement-twin").unwrap();
    let created = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .any(|fact| {
            fact.payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains("Created worktree at:"))
        });
    assert!(
        created,
        "the worktree creation must land as a tool.result fact"
    );

    // The settlement oracle: after the turn settles, the Run-owned effect
    // has removed the worktree. Idempotent-close means the default
    // (sibling) path is the one the provider reported and the effect
    // removed.
    assert!(
        !expected.exists(),
        "the Run must remove its worktree at settlement; {expected:?} remains"
    );
}
