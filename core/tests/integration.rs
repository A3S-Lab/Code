//! Integration tests for a3s-code-core
//!
//! These tests exercise the public API (Agent, AgentSession, CodeConfig, ToolExecutor)
//! against real configuration and real tool execution.
//!
//! - Groups 1-3 run without network or LLM access.
//! - Group 4 (LLM) is gated by `#[ignore]` — run with:
//!   `cargo test -p a3s-code-core --test integration -- --ignored`

use a3s_code_core::{Agent, AgentEvent, AgentSession, CodeConfig, SessionOptions};
use std::path::PathBuf;
use tempfile::TempDir;

// ============================================================================
// Helpers
// ============================================================================

/// Absolute path to `.a3s/config.hcl` in the repo root.
fn config_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // crates/code/core
    let repo_root = manifest
        .parent() // crates/code
        .and_then(|p| p.parent()) // crates
        .and_then(|p| p.parent()) // repo root
        .expect("failed to resolve repo root");
    repo_root.join(".a3s/config.hcl")
}

/// Create an Agent from the repo-root `.a3s/config.hcl`.
async fn create_agent() -> Agent {
    let path = config_path();
    Agent::new(path.display().to_string())
        .await
        .expect("failed to create agent from config.hcl")
}

/// Create an Agent + AgentSession bound to a fresh temp directory.
async fn create_session() -> (Agent, AgentSession, TempDir) {
    let agent = create_agent().await;
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .expect("failed to create session");
    (agent, session, tmp)
}

// ============================================================================
// Group 1: Config Loading (no network, no LLM)
// ============================================================================

#[test]
fn test_load_config_from_hcl_file() {
    let path = config_path();
    let config = CodeConfig::from_file(&path).expect("failed to load config.hcl");

    assert!(
        config.default_model.is_some(),
        "default_model should be set"
    );
    assert!(
        !config.providers.is_empty(),
        "should have at least one provider"
    );
}

#[test]
fn test_config_provider_lookup() {
    let config = CodeConfig::from_file(&config_path()).unwrap();

    let anthropic = config.find_provider("anthropic");
    assert!(anthropic.is_some(), "should find 'anthropic' provider");
    assert!(
        !anthropic.unwrap().models.is_empty(),
        "anthropic should have models"
    );

    let openai = config.find_provider("openai");
    assert!(openai.is_some(), "should find 'openai' provider");
    assert!(
        !openai.unwrap().models.is_empty(),
        "openai should have models"
    );

    assert!(
        config.find_provider("nonexistent").is_none(),
        "nonexistent provider should return None"
    );
}

#[test]
fn test_config_default_model_resolution() {
    let config = CodeConfig::from_file(&config_path()).unwrap();

    let result = config.default_model_config();
    assert!(result.is_some(), "default_model_config should resolve");

    let (provider, model) = result.unwrap();
    // default_model = "openai/kimi-k2.5"
    assert_eq!(provider.name, "openai");
    assert_eq!(model.id, "kimi-k2.5");
}

#[test]
fn test_config_list_all_models() {
    let config = CodeConfig::from_file(&config_path()).unwrap();
    let models = config.list_models();

    // 2 anthropic models + 1 openai model = 3
    assert_eq!(
        models.len(),
        3,
        "expected 3 models total, got {}",
        models.len()
    );

    let model_ids: Vec<&str> = models.iter().map(|(_, m)| m.id.as_str()).collect();
    assert!(model_ids.contains(&"claude-opus-4-5-20251101"));
    assert!(model_ids.contains(&"claude-sonnet-4-20250514"));
    assert!(model_ids.contains(&"kimi-k2.5"));
}

// ============================================================================
// Group 2: Agent Creation (no network needed for creation itself)
// ============================================================================

#[tokio::test]
async fn test_agent_from_hcl_file() {
    let path = config_path();
    let agent = Agent::new(path.display().to_string()).await;
    assert!(agent.is_ok(), "Agent::new from HCL file should succeed");
}

#[tokio::test]
async fn test_agent_create_from_hcl_file() {
    let path = config_path();
    let agent = Agent::create(path.display().to_string()).await;
    assert!(
        agent.is_ok(),
        "Agent::create from HCL file should succeed: {:?}",
        agent.err()
    );
}

#[tokio::test]
async fn test_agent_create_and_new_equivalence() {
    let path = config_path();
    let path_str = path.display().to_string();

    let agent_new = Agent::new(&path_str).await;
    let agent_create = Agent::create(&path_str).await;

    assert!(agent_new.is_ok(), "Agent::new should succeed");
    assert!(agent_create.is_ok(), "Agent::create should succeed");

    // Both should produce working sessions
    let tmp1 = tempfile::tempdir().unwrap();
    let tmp2 = tempfile::tempdir().unwrap();

    let session_new = agent_new
        .unwrap()
        .session(tmp1.path().display().to_string(), None);
    let session_create = agent_create
        .unwrap()
        .session(tmp2.path().display().to_string(), None);

    assert!(session_new.is_ok(), "session from new should succeed");
    assert!(session_create.is_ok(), "session from create should succeed");
}

#[tokio::test]
async fn test_agent_create_session_tool_execution() {
    let path = config_path();
    let agent = Agent::create(path.display().to_string()).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let session = agent
        .session(tmp.path().display().to_string(), None)
        .unwrap();

    // Verify tool execution works through create()-built agent
    let output = session.bash("echo create-works").await.unwrap();
    assert!(
        output.contains("create-works"),
        "tool via create() agent should work, got: {}",
        output
    );
}

#[tokio::test]
async fn test_agent_from_config_struct() {
    let config = CodeConfig::from_file(&config_path()).unwrap();
    let agent = Agent::from_config(config).await;
    assert!(
        agent.is_ok(),
        "Agent::from_config should succeed: {:?}",
        agent.err()
    );
}

#[tokio::test]
async fn test_agent_session_creation() {
    let agent = create_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let session = agent.session(tmp.path().display().to_string(), None);
    assert!(
        session.is_ok(),
        "session creation should succeed: {:?}",
        session.err()
    );
}

#[tokio::test]
async fn test_agent_session_with_model_override() {
    let agent = create_agent().await;
    let tmp = tempfile::tempdir().unwrap();
    let opts = SessionOptions::new().with_model("anthropic/claude-sonnet-4-20250514");
    let session = agent.session(tmp.path().display().to_string(), Some(opts));
    assert!(
        session.is_ok(),
        "session with model override should succeed: {:?}",
        session.err()
    );
}

#[tokio::test]
async fn test_agent_session_invalid_model() {
    let agent = create_agent().await;
    let tmp = tempfile::tempdir().unwrap();

    // Missing provider/model separator
    let opts = SessionOptions::new().with_model("invalid-no-slash");
    let session = agent.session(tmp.path().display().to_string(), Some(opts));
    assert!(session.is_err(), "invalid model format should fail");

    // Nonexistent model
    let opts2 = SessionOptions::new().with_model("anthropic/nonexistent-model");
    let session2 = agent.session(tmp.path().display().to_string(), Some(opts2));
    assert!(session2.is_err(), "nonexistent model should fail");
}

// ============================================================================
// Group 3: Tool Execution via Session (no LLM, local operations)
// ============================================================================

#[tokio::test]
async fn test_tool_bash() {
    let (_agent, session, _tmp) = create_session().await;
    let output = session
        .bash("echo hello")
        .await
        .expect("bash should succeed");
    assert!(
        output.contains("hello"),
        "bash output should contain 'hello', got: {}",
        output
    );
}

#[tokio::test]
async fn test_tool_read_file() {
    let (_agent, session, tmp) = create_session().await;

    // Create a file in the temp workspace
    let file_path = tmp.path().join("test-read.txt");
    std::fs::write(&file_path, "integration test content").unwrap();

    let content = session
        .read_file(file_path.to_str().unwrap())
        .await
        .expect("read_file should succeed");

    assert!(
        content.contains("integration test content"),
        "read content should contain written text, got: {}",
        content
    );
}

#[tokio::test]
async fn test_tool_write_and_read() {
    let (_agent, session, tmp) = create_session().await;

    let file_path = tmp.path().join("test-write.txt");
    let write_content = "roundtrip test content\nsecond line";

    // Write via tool API
    session
        .tool(
            "write",
            serde_json::json!({
                "file_path": file_path.to_str().unwrap(),
                "content": write_content,
            }),
        )
        .await
        .expect("write tool should succeed");

    // Read back via convenience method
    let read_back = session
        .read_file(file_path.to_str().unwrap())
        .await
        .expect("read_file should succeed");

    assert!(
        read_back.contains("roundtrip test content"),
        "read back should contain written content, got: {}",
        read_back
    );
    assert!(
        read_back.contains("second line"),
        "read back should contain second line"
    );
}

#[tokio::test]
async fn test_tool_edit() {
    let (_agent, session, tmp) = create_session().await;

    let file_path = tmp.path().join("test-edit.txt");
    std::fs::write(&file_path, "hello world\nfoo bar\nbaz").unwrap();

    // Edit: replace "foo bar" with "foo replaced"
    let result = session
        .tool(
            "edit",
            serde_json::json!({
                "file_path": file_path.to_str().unwrap(),
                "old_string": "foo bar",
                "new_string": "foo replaced",
            }),
        )
        .await
        .expect("edit tool should succeed");

    assert_eq!(result.exit_code, 0, "edit should exit 0: {}", result.output);

    // Verify the edit was applied
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(
        content.contains("foo replaced"),
        "file should contain edited text, got: {}",
        content
    );
    assert!(
        !content.contains("foo bar"),
        "file should not contain old text"
    );
}

#[tokio::test]
async fn test_tool_glob() {
    let (_agent, session, tmp) = create_session().await;

    // Create several files
    std::fs::write(tmp.path().join("alpha.txt"), "a").unwrap();
    std::fs::write(tmp.path().join("beta.txt"), "b").unwrap();
    std::fs::write(tmp.path().join("gamma.rs"), "c").unwrap();

    let matches = session.glob("*.txt").await.expect("glob should succeed");

    assert!(
        matches.len() >= 2,
        "glob *.txt should find at least 2 files, got: {:?}",
        matches
    );

    // Verify .rs file is NOT matched
    let has_rs = matches.iter().any(|m| m.contains("gamma.rs"));
    assert!(!has_rs, "glob *.txt should not match .rs files");
}

#[tokio::test]
async fn test_tool_grep() {
    let (_agent, session, tmp) = create_session().await;

    std::fs::write(
        tmp.path().join("searchable.txt"),
        "line one\nfind_this_pattern here\nline three",
    )
    .unwrap();

    let output = session
        .grep("find_this_pattern")
        .await
        .expect("grep should succeed");

    assert!(
        output.contains("find_this_pattern"),
        "grep output should contain the pattern, got: {}",
        output
    );
}

#[tokio::test]
async fn test_tool_ls() {
    let (_agent, session, tmp) = create_session().await;

    std::fs::write(tmp.path().join("file1.txt"), "").unwrap();
    std::fs::write(tmp.path().join("file2.txt"), "").unwrap();
    std::fs::create_dir(tmp.path().join("subdir")).unwrap();

    let result = session
        .tool(
            "ls",
            serde_json::json!({ "path": tmp.path().to_str().unwrap() }),
        )
        .await
        .expect("ls tool should succeed");

    assert_eq!(result.exit_code, 0, "ls should exit 0: {}", result.output);
    assert!(
        result.output.contains("file1.txt"),
        "ls should list file1.txt, got: {}",
        result.output
    );
    assert!(
        result.output.contains("subdir"),
        "ls should list subdir, got: {}",
        result.output
    );
}

#[tokio::test]
async fn test_tool_unknown() {
    let (_agent, session, _tmp) = create_session().await;

    let result = session
        .tool("nonexistent_tool", serde_json::json!({}))
        .await;

    // The ToolExecutor returns a ToolResult with exit_code=1 for unknown tools
    // rather than an Err, so we check for that
    match result {
        Ok(r) => {
            assert_eq!(r.exit_code, 1, "unknown tool should exit 1");
            assert!(
                r.output.contains("Unknown tool"),
                "output should mention unknown tool, got: {}",
                r.output
            );
        }
        Err(e) => {
            // Also acceptable if it returns an error
            let msg = format!("{}", e);
            assert!(
                msg.to_lowercase().contains("unknown") || msg.to_lowercase().contains("not found"),
                "error should mention unknown/not found: {}",
                msg
            );
        }
    }
}

// ============================================================================
// Group 3b: Builtin Tools Metadata (no network, no LLM)
// ============================================================================

#[test]
fn test_builtin_tools_md_no_binary_references() {
    let content = include_str!("../skills/builtin-tools.md");

    // Must not reference the deprecated a3s-tools binary
    assert!(
        !content.contains("a3s-tools"),
        "builtin-tools.md should not reference 'a3s-tools' binary"
    );

    // Must not reference TOOL_ARGS env var (old binary protocol)
    assert!(
        !content.contains("TOOL_ARGS"),
        "builtin-tools.md should not reference 'TOOL_ARGS'"
    );

    // All backends should be 'builtin'
    assert!(
        !content.contains("type: binary"),
        "builtin-tools.md should not contain 'type: binary'"
    );

    // Should contain 'type: builtin' for each tool
    let builtin_count = content.matches("type: builtin").count();
    assert_eq!(
        builtin_count, 11,
        "expected 11 'type: builtin' entries, found {}",
        builtin_count
    );
}

// ============================================================================
// Group 4: LLM Integration (requires network + API keys)
// ============================================================================

#[tokio::test]
#[ignore = "requires network + valid API keys in .a3s/config.hcl"]
async fn test_llm_send_simple_prompt() {
    let (_agent, session, _tmp) = create_session().await;

    let result = session
        .send("What is 2+2? Reply with just the number.")
        .await
        .expect("LLM send should succeed");

    assert!(
        !result.text.is_empty(),
        "LLM response text should not be empty"
    );
    assert!(
        result.text.contains('4'),
        "LLM response should contain '4', got: {}",
        result.text
    );
}

#[tokio::test]
#[ignore = "requires network + valid API keys in .a3s/config.hcl"]
async fn test_llm_stream_events() {
    let (_agent, session, _tmp) = create_session().await;

    let (mut rx, handle) = session
        .stream("Say hello")
        .await
        .expect("stream should succeed");

    let mut got_text_delta = false;
    let mut got_end = false;
    let mut collected_text = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta { text } => {
                got_text_delta = true;
                collected_text.push_str(&text);
            }
            AgentEvent::End { .. } => {
                got_end = true;
                break;
            }
            _ => {}
        }
    }

    // Clean up the spawned task
    handle.abort();

    assert!(got_text_delta, "should have received TextDelta events");
    assert!(got_end, "should have received End event");
    assert!(
        !collected_text.is_empty(),
        "collected stream text should not be empty"
    );
}
