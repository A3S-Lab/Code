//! Live kernel checks for tool calls, MCP, skills, long tasks, compaction,
//! planning, parallel fan-out, review, and workspace retrieval.
//!
//! Success is a kernel event or a side effect. Assistant wording is not a pass.
//! A model that never exercises the mechanism fails the test.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_harness_capabilities_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, CodeConfig, SessionOptions};

const MODEL_TIMEOUT: Duration = Duration::from_secs(180);
const MCP_TOKEN: &str = "mcp-live-token-7f3a";
const SKILL_TOKEN: &str = "skill-live-token-91c2";
const SEARCH_TOKEN: &str = "workspace-live-token-44e0";
const EDIT_BEFORE: &str = "edit-before-token-6c1d";
const EDIT_AFTER: &str = "edit-after-token-6c1d";
const PATCH_BEFORE: &str = "keep\npatch-before-token-8a2e\ntail\n";
const PATCH_AFTER: &str = "keep\npatch-after-token-8a2e\ntail\n";
const PATCH_DIFF: &str =
    "@@ -1,3 +1,3 @@\n keep\n-patch-before-token-8a2e\n+patch-after-token-8a2e\n tail";
const GLOB_FILE: &str = "zeta-live-glob-8a2e.txt";
const GREP_TOKEN: &str = "grep-live-token-3b9f";
const GREP_FILE: &str = "zeta-grep-archive.md";
const GIT_DIFF_TOKEN: &str = "git-diff-token-8a2e";
const ASK_HOST_ANSWER: &str = "host-answer-token-4c7b";

struct Observed {
    tools: Vec<ToolSeen>,
    subagent_starts: Vec<String>,
    task_end_before_subagent_end: bool,
    saw_subagent_end: bool,
    compacted: bool,
    planned: bool,
    goal_extracted: bool,
    steps: usize,
    errors: Vec<String>,
    end_text: String,
}

struct ToolSeen {
    name: String,
    exit_code: i32,
    output: String,
    metadata: Option<serde_json::Value>,
}

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn configured_agent() -> Agent {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let default_model = config
        .default_model
        .as_deref()
        .expect("config must declare default_model");
    assert!(
        default_model.contains("deepseek"),
        "expected the configured DeepSeek Flash default_model, got {default_model}"
    );
    eprintln!("using default_model={default_model}");
    Agent::from_config(config)
        .await
        .expect("build agent from .a3s/config.acl")
}

fn policy(allows: &[&str]) -> PermissionPolicy {
    let mut policy = PermissionPolicy {
        default_decision: PermissionDecision::Deny,
        ..PermissionPolicy::default()
    }
    .allow("read(**)");
    for rule in allows {
        policy = policy.allow(*rule);
    }
    policy
}

fn options(session_id: &str, allows: &[&str]) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(std::sync::Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy(allows))
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(8)
        .with_llm_api_timeout(90_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

async fn observe(session: &a3s_code_core::AgentSession, prompt: &str) -> Observed {
    observe_host(session, prompt, None).await
}

async fn observe_host(
    session: &a3s_code_core::AgentSession,
    prompt: &str,
    host_answer: Option<&str>,
) -> Observed {
    let host_answer = host_answer.map(str::to_string);
    let (mut events, join) = session.stream(prompt, None).await.expect("stream");
    let observed = tokio::time::timeout(MODEL_TIMEOUT, async {
        let mut observed = Observed {
            tools: Vec::new(),
            subagent_starts: Vec::new(),
            task_end_before_subagent_end: false,
            saw_subagent_end: false,
            compacted: false,
            planned: false,
            goal_extracted: false,
            steps: 0,
            errors: Vec::new(),
            end_text: String::new(),
        };
        let mut task_ended = false;
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::ToolEnd {
                    name,
                    exit_code,
                    output,
                    metadata,
                    ..
                } => {
                    if name == "task" && !observed.saw_subagent_end {
                        observed.task_end_before_subagent_end = true;
                        task_ended = true;
                    }
                    observed.tools.push(ToolSeen {
                        name,
                        exit_code,
                        output,
                        metadata,
                    });
                }
                AgentEvent::SubagentStart { agent, .. } => {
                    observed.subagent_starts.push(agent);
                }
                AgentEvent::SubagentEnd { .. } => {
                    observed.saw_subagent_end = true;
                    if !task_ended {
                        observed.task_end_before_subagent_end = false;
                    }
                }
                AgentEvent::ContextCompacted { .. } => observed.compacted = true,
                AgentEvent::PlanningEnd { .. } => observed.planned = true,
                AgentEvent::GoalExtracted { .. } => observed.goal_extracted = true,
                AgentEvent::StepEnd { .. } => observed.steps += 1,
                AgentEvent::End { text, .. } => observed.end_text = text,
                AgentEvent::Error { message } => observed.errors.push(message),
                AgentEvent::UserQuestion { question_id, .. } => {
                    if let Some(answer) = host_answer.as_deref() {
                        let _ = a3s_code_core::ask_user::answer(&question_id, answer);
                    }
                }
                _ => {}
            }
        }
        let _ = join.await;
        observed
    })
    .await
    .expect("run timed out");
    observed
}

fn tool_outputs<'a>(observed: &'a Observed, name: &str) -> Vec<&'a ToolSeen> {
    observed
        .tools
        .iter()
        .filter(|tool| tool.name.eq_ignore_ascii_case(name))
        .collect()
}

fn workspace_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".a3s" || name == ".a3s-code" || name == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
    files.sort();
    files
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_read_tool_returns_the_fixture() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("note.txt"), "hello\n").expect("fixture");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-tool-read", &[]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "Read note.txt with the read tool and stop. Do not invent the contents.",
    )
    .await;
    let reads = tool_outputs(&observed, "read");
    assert!(
        reads
            .iter()
            .any(|tool| tool.exit_code == 0 && tool.output.contains("hello")),
        "read tool did not return the fixture: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_search_finds_the_workspace_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let nested = workspace.path().join("notes");
    std::fs::create_dir_all(&nested).expect("notes");
    std::fs::write(
        nested.join("zeta-archive.md"),
        format!("unrelated\n{SEARCH_TOKEN}\n"),
    )
    .expect("fixture");
    std::fs::write(workspace.path().join("other.txt"), "nothing here\n").expect("other");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-search", &["search(**)", "grep(**)"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        &format!(
            "Find the workspace file that contains {SEARCH_TOKEN}. Use the search tool. Do not guess the path."
        ),
    )
    .await;
    let found = observed.tools.iter().any(|tool| {
        matches!(tool.name.as_str(), "search" | "grep")
            && tool.exit_code == 0
            && tool.output.contains("zeta-archive.md")
    });
    assert!(
        found,
        "workspace search did not return zeta-archive.md: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.exit_code))
            .collect::<Vec<_>>()
    );
    assert!(
        !observed
            .errors
            .iter()
            .any(|error| error.contains("completion gate:")),
        "retrieval index was treated as a source mutation: {:?}",
        observed.errors
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_glob_finds_the_nested_fixture() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let nested = workspace.path().join("notes");
    std::fs::create_dir_all(&nested).expect("notes");
    std::fs::write(nested.join(GLOB_FILE), "listed\n").expect("fixture");
    std::fs::write(workspace.path().join("other.txt"), "decoy\n").expect("decoy");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-glob", &["search(**)"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        &format!(
            "Find files matching **/{GLOB_FILE} with search mode glob, then stop. Do not guess the directory."
        ),
    )
    .await;
    let found = tool_outputs(&observed, "search").iter().any(|tool| {
        tool.exit_code == 0
            && search_mode(tool) == Some("glob")
            && tool.output.contains(&format!("notes/{GLOB_FILE}"))
    });
    assert!(
        found,
        "search mode glob did not return notes/{GLOB_FILE}: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (
                tool.name.as_str(),
                tool.exit_code,
                tool.output.chars().take(180).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(nested.join(GLOB_FILE)).expect("fixture"),
        "listed\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_grep_finds_the_nested_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let nested = workspace.path().join("notes");
    std::fs::create_dir_all(&nested).expect("notes");
    std::fs::write(nested.join(GREP_FILE), format!("unrelated\n{GREP_TOKEN}\n")).expect("fixture");
    std::fs::write(workspace.path().join("other.txt"), "nothing here\n").expect("other");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-grep", &["search(**)"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        &format!(
            "Find the workspace file that contains {GREP_TOKEN} with search mode grep, then stop. Do not guess the path."
        ),
    )
    .await;
    let found = tool_outputs(&observed, "search").iter().any(|tool| {
        tool.exit_code == 0
            && search_mode(tool) == Some("grep")
            && grep_hit_contains(tool, &format!("notes/{GREP_FILE}"))
    });
    assert!(
        found,
        "search mode grep did not return notes/{GREP_FILE}: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (
                tool.name.as_str(),
                tool.exit_code,
                tool.output.chars().take(180).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_git_diff_returns_the_changed_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    init_git_repo(workspace.path());
    std::fs::write(
        workspace.path().join("tracked.txt"),
        format!("tracked\n{GIT_DIFF_TOKEN}\n"),
    )
    .expect("edit tracked");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-git", &["git(**)"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "Show the working-tree diff with the git tool command diff, then stop. Do not guess the changed token.",
    )
    .await;
    let found = tool_outputs(&observed, "git").iter().any(|tool| {
        tool.exit_code == 0
            && tool.output.contains("tracked.txt")
            && tool.output.contains(GIT_DIFF_TOKEN)
    });
    assert!(
        found,
        "git diff did not return the changed token: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (
                tool.name.as_str(),
                tool.exit_code,
                tool.output.chars().take(240).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_ask_user_returns_the_host_answer() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-ask", &["ask_user"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe_host(
        &session,
        "Call ask_user once to ask which token to use, with allow_free_text true, then stop. Do not invent the answer and do not write a file.",
        Some(ASK_HOST_ANSWER),
    )
    .await;
    let answered = tool_outputs(&observed, "ask_user").iter().any(|tool| {
        tool.exit_code == 0
            && tool.output.contains(ASK_HOST_ANSWER)
            && tool.output.contains("\"status\":\"answered\"")
            && tool.metadata.as_ref().is_some_and(|metadata| {
                metadata
                    .get("permission_grant")
                    .and_then(|value| value.as_bool())
                    == Some(false)
                    && metadata
                        .get("wrote_files")
                        .and_then(|value| value.as_bool())
                        == Some(false)
            })
    });
    assert!(
        answered,
        "ask_user did not return the host answer as a non-grant: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (
                tool.name.as_str(),
                tool.exit_code,
                tool.output.chars().take(240).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        workspace_files(workspace.path()).is_empty(),
        "answering ask_user wrote a workspace file"
    );
}

fn search_mode(tool: &ToolSeen) -> Option<&str> {
    tool.metadata
        .as_ref()
        .and_then(|metadata| metadata.get("mode"))
        .and_then(serde_json::Value::as_str)
}

fn grep_hit_contains(tool: &ToolSeen, path: &str) -> bool {
    if tool.output.contains(path) {
        return true;
    }
    tool.metadata
        .as_ref()
        .and_then(|metadata| metadata.get("source_anchors"))
        .and_then(|anchors| anchors.as_array())
        .is_some_and(|anchors| anchors.iter().any(|anchor| anchor.as_str() == Some(path)))
}

fn init_git_repo(root: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init"]);
    run(&["config", "user.email", "live@example.com"]);
    run(&["config", "user.name", "live"]);
    std::fs::write(root.join("tracked.txt"), "tracked\n").expect("tracked");
    run(&["add", "tracked.txt"]);
    run(&["commit", "-m", "init"]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_mcp_tool_returns_the_server_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let script = workspace.path().join("mcp_fixture.py");
    std::fs::write(&script, mcp_fixture_script()).expect("mcp script");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-mcp", &["mcp__fixture__*"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let registered = session
        .add_mcp_server(a3s_code_core::mcp::McpServerConfig {
            name: "fixture".to_string(),
            transport: a3s_code_core::mcp::McpTransportConfig::Stdio {
                command: "python3".to_string(),
                args: vec![script.to_string_lossy().to_string()],
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 30,
        })
        .await
        .expect("connect fixture MCP server");
    assert!(registered >= 1, "MCP server registered no tools");
    let observed = observe(
        &session,
        "Call mcp__fixture__lookup and return only the token that tool outputs. The token is not in this message.",
    )
    .await;
    let called = observed.tools.iter().any(|tool| {
        tool.name == "mcp__fixture__lookup"
            && tool.exit_code == 0
            && tool.output.contains(MCP_TOKEN)
    });
    assert!(
        called,
        "MCP tool did not return the server token: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.exit_code, tool.output.as_str()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_skill_tool_returns_the_skill_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let skills = workspace.path().join("skills");
    std::fs::create_dir_all(&skills).expect("skills");
    std::fs::write(
        skills.join("fixture-token.md"),
        format!(
            "---\nname: fixture-token\ndescription: Report the skill body token.\nkind: instruction\n---\n# Fixture\nThe token is {SKILL_TOKEN}. Reply with that token only. Do not call tools.\n"
        ),
    )
    .expect("skill");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("live-skill", &["Skill(**)"])
                    .with_skills_from_dir(&skills)
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");
    assert!(
        session
            .skill_names()
            .iter()
            .any(|name| name == "fixture-token"),
        "skill was not registered: {:?}",
        session.skill_names()
    );
    let observed = observe(
        &session,
        "Invoke the Skill tool with skill_name fixture-token. The token is only in the skill body. Return the tool's token.",
    )
    .await;
    let invoked = tool_outputs(&observed, "Skill")
        .into_iter()
        .any(|tool| tool.exit_code == 0 && tool.output.contains(SKILL_TOKEN));
    assert!(
        invoked,
        "Skill tool did not return the skill token: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.exit_code))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_background_task_returns_before_the_child_finishes() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("token.txt"), "bg-token\n").expect("fixture");
    let before = workspace_files(workspace.path());
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("live-background", &["task(**)"])
                    .with_manual_delegation_enabled(true)
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "Call the task tool once with exactly one item: agent explore, background true, description read-token, prompt \"Read token.txt and return its contents.\". Then stop. Do not read the file yourself.",
    )
    .await;
    let launched = observed.tools.iter().any(|tool| {
        tool.name == "task"
            && tool.exit_code == 0
            && tool.output.contains("Task started in background")
    });
    assert!(
        launched,
        "background task did not launch: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.exit_code, tool.output.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(
        observed
            .subagent_starts
            .iter()
            .any(|agent| agent == "explore"),
        "background launch did not emit SubagentStart: {:?}",
        observed.subagent_starts
    );
    assert_eq!(workspace_files(workspace.path()), before);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_compaction_keeps_the_earlier_token() {
    let token = "compact-live-token-18ab";
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("live-compact", &[])
                    .with_read_only_session(true)
                    .with_auto_compact(true)
                    .with_auto_compact_threshold(0.05)
                    .with_max_context_tokens(4_000),
            ),
        )
        .await
        .expect("session");
    let filler = "note ".repeat(2_000);
    let first = session
        .send(
            &format!(
                "Remember this fixture token: {token}. Context filler: {filler} Reply with ok."
            ),
            None,
        )
        .await;
    assert!(
        first.is_ok(),
        "first turn failed before compaction: {first:?}"
    );
    let observed = observe(
        &session,
        "What fixture token did the previous message ask you to remember? Reply with the token only.",
    )
    .await;
    assert!(
        observed.compacted,
        "context was not compacted: errors={:?}",
        observed.errors
    );
    assert!(
        observed.end_text.contains(token),
        "compaction dropped the fixture token: text={} errors={:?}",
        observed.end_text,
        observed.errors
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_plan_tracks_a_read_only_goal() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("note.txt"), "plan-token\n").expect("fixture");
    let before = workspace_files(workspace.path());
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("live-plan", &["search(**)", "grep(**)"])
                    .with_planning(true)
                    .with_planning_mode(a3s_code_core::PlanningMode::Enabled)
                    .with_goal_tracking(true)
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "Read note.txt and report its contents. Do not write or edit files.",
    )
    .await;
    assert!(
        observed.planned,
        "planning did not emit PlanningEnd: {:?}",
        observed.errors
    );
    assert!(
        observed.goal_extracted,
        "goal tracking did not emit GoalExtracted: {:?}",
        observed.errors
    );
    assert!(
        observed.steps > 0,
        "plan had no tracked steps: {:?}",
        observed.errors
    );
    assert_eq!(workspace_files(workspace.path()), before);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_task_fanout_reads_two_files() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("alpha.txt"), "alpha-token\n").expect("alpha");
    std::fs::write(workspace.path().join("beta.txt"), "beta-token\n").expect("beta");
    let before = workspace_files(workspace.path());
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("live-parallel", &["task(**)"])
                    .with_manual_delegation_enabled(true)
                    .with_max_parallel_tasks(2)
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "Call the task tool once with two independent items, both agent explore. One reads alpha.txt and must return alpha-token. The other reads beta.txt and must return beta-token. Do not read the files yourself.",
    )
    .await;
    let fanout = observed.tools.iter().find(|tool| {
        tool.name == "task"
            && tool.exit_code == 0
            && tool
                .metadata
                .as_ref()
                .and_then(|value| value.get("task_count"))
                .and_then(serde_json::Value::as_u64)
                >= Some(2)
            && tool.output.contains("alpha-token")
            && tool.output.contains("beta-token")
    });
    assert!(
        fanout.is_some(),
        "task fan-out did not return both tokens: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.exit_code, tool.metadata.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        observed
            .subagent_starts
            .iter()
            .filter(|agent| *agent == "explore")
            .count()
            >= 2,
        "fan-out did not start two explore children: {:?}",
        observed.subagent_starts
    );
    assert_eq!(workspace_files(workspace.path()), before);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_review_agent_does_not_edit() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let guest = "fn add(left: i32, right: i32) -> i32 { left + right + 1 }\n";
    std::fs::write(workspace.path().join("guest.rs"), guest).expect("fixture");
    let before = workspace_files(workspace.path());
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                options("live-review", &["task(**)", "search(**)", "grep(**)"])
                    .with_manual_delegation_enabled(true)
                    .with_read_only_session(true),
            ),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "Review guest.rs by calling the task tool with agent review. Report the bug. Do not edit files and do not run tests.",
    )
    .await;
    assert!(
        observed
            .subagent_starts
            .iter()
            .any(|agent| agent == "review"),
        "review agent was not started: {:?}",
        observed.subagent_starts
    );
    assert!(
        observed
            .tools
            .iter()
            .all(|tool| !matches!(tool.name.as_str(), "write" | "edit" | "patch" | "download")),
        "review run executed a write tool"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("guest.rs")).expect("guest"),
        guest
    );
    assert_eq!(workspace_files(workspace.path()), before);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_ls_lists_the_fixture() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("catalog.txt"), "listed\n").expect("fixture");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-ls", &["ls(**)"]).with_read_only_session(true)),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        "List this workspace with the ls tool and stop. Do not invent filenames.",
    )
    .await;
    let listings = tool_outputs(&observed, "ls");
    assert!(
        listings
            .iter()
            .any(|tool| tool.exit_code == 0 && tool.output.contains("catalog.txt")),
        "ls did not list catalog.txt: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool.exit_code))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_edit_replaces_the_fixture_token() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("guest.txt");
    std::fs::write(&path, format!("keep\n{EDIT_BEFORE}\n")).expect("fixture");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-edit", &["edit(**)"])),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        &format!(
            "Replace {EDIT_BEFORE} with {EDIT_AFTER} in guest.txt using the edit tool, then stop."
        ),
    )
    .await;
    let edits = tool_outputs(&observed, "edit");
    assert!(
        edits.iter().any(|tool| tool.exit_code == 0),
        "edit tool did not succeed: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (
                tool.name.as_str(),
                tool.exit_code,
                tool.output.chars().take(160).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
    let after = std::fs::read_to_string(&path).expect("guest");
    assert!(
        after.contains(EDIT_AFTER) && !after.contains(EDIT_BEFORE),
        "edit did not replace the fixture token: {after:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the DeepSeek Flash model configured in .a3s/config.acl"]
async fn deepseek_flash_patch_applies_the_given_hunk() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("guest.txt");
    std::fs::write(&path, PATCH_BEFORE).expect("fixture");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(options("live-patch", &["patch(**)"])),
        )
        .await
        .expect("session");
    let observed = observe(
        &session,
        &format!(
            "Apply this unified diff to guest.txt with the patch tool, then stop. Do not invent a different diff.\n{PATCH_DIFF}"
        ),
    )
    .await;
    let patches = tool_outputs(&observed, "patch");
    assert!(
        patches.iter().any(|tool| {
            tool.exit_code == 0
                && tool
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("file_path"))
                    .and_then(|path| path.as_str())
                    == Some("guest.txt")
        }),
        "patch tool did not apply guest.txt: {:?}",
        observed
            .tools
            .iter()
            .map(|tool| (
                tool.name.as_str(),
                tool.exit_code,
                tool.output.chars().take(180).collect::<String>()
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("guest"),
        PATCH_AFTER,
        "patch did not apply the hunk"
    );
    assert!(
        observed
            .errors
            .iter()
            .any(|message| message.starts_with("completion gate:")),
        "a committed patch was treated as narrative success: {:?}",
        observed.errors
    );
}

fn mcp_fixture_script() -> String {
    format!(
        r#"#!/usr/bin/env python3
import json, sys
TOKEN = {token:?}

def reply(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg.get("method")
    mid = msg["id"]
    if method == "initialize":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"protocolVersion": "2024-11-05", "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "fixture", "version": "0"}}}}}})
    elif method == "tools/list":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"tools": [{{"name": "lookup", "description": "Return the fixture token.", "inputSchema": {{"type": "object", "properties": {{}}}}, "annotations": {{"readOnlyHint": True, "destructiveHint": False, "openWorldHint": False}}}}]}}}})
    elif method == "tools/call":
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{"content": [{{"type": "text", "text": TOKEN}}], "isError": False}}}})
    else:
        reply({{"jsonrpc": "2.0", "id": mid, "result": {{}}}})
"#,
        token = MCP_TOKEN
    )
}
