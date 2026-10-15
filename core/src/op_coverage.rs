//! One kernel assertion for each `sdk_capabilities()` operation the deep cases
//! do not already assert. A pass is an identity, a digest, an absence, or a
//! typed terminal state. Completion text is not a pass.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::{
    Agent, AgentSession, CodeConfig, CodeError, InterruptRequest, RunControlError, SessionOptions,
    SteerRequest,
};

fn op(operation: &str, effect: &str) {
    println!("OP\t{operation}\t{effect}\tpassed");
}

fn session_id(label: &str) -> String {
    format!(
        "opcov-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    )
}

fn history_digest(session: &AgentSession) -> String {
    crate::digest_json("opcov.history", &session.history()).expect("history digest")
}

fn base_options(id: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(id)
        .with_max_tool_rounds(1)
        .with_temperature(0.0)
}

async fn open_session(root: &Path, options: SessionOptions) -> (Agent, AgentSession) {
    let config = crate::deep38_support::load_pinned_config();
    let agent = Agent::from_config(config).await.expect("build agent");
    let session = agent
        .session_async(root.to_string_lossy().as_ref(), Some(options))
        .await
        .expect("admit session");
    (agent, session)
}

fn repo_config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.a3s/config.acl")
}

fn assert_session_closed<T>(label: &str, result: Result<T, CodeError>) {
    match result {
        Err(CodeError::SessionClosed { .. }) => {}
        Err(error) => panic!("{label} rejected with {error}"),
        Ok(_) => panic!("{label} was accepted on a closed session"),
    }
}

struct BlockingPreTool {
    calls: Arc<AtomicUsize>,
}

impl crate::hooks::HookHandler for BlockingPreTool {
    fn handle(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        crate::hooks::HookResponse::block("opcov-block")
    }
}

struct DenyingBudget {
    checks: AtomicUsize,
    records: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for DenyingBudget {
    async fn check_before_llm(
        &self,
        _session_id: &str,
        _estimated_prompt_tokens: usize,
    ) -> crate::budget::BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        crate::budget::BudgetDecision::Deny {
            resource: "opcov".into(),
            reason: "opcov budget deny".into(),
        }
    }

    async fn record_after_llm(&self, _session_id: &str, _usage: &crate::TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
    }
}

struct StubCognitive;

#[async_trait::async_trait]
impl crate::CognitiveContextProvider for StubCognitive {
    fn name(&self) -> &str {
        "opcov-cognitive"
    }

    async fn query(
        &self,
        _request: &crate::CognitiveContextRequestV1,
    ) -> crate::CognitiveContextResult<crate::CognitiveContextResponseV1> {
        Err(crate::CognitiveContextError::Provider(
            "opcov stub does not serve knowledge".into(),
        ))
    }
}

fn cognitive_binding() -> crate::CognitivePackageBindingV1 {
    let generation_digest =
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
    let knowledge = crate::CognitiveKnowledgeBindingV1::new(
        "domain-knowledge",
        "0.2",
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
        7,
        generation_digest,
    )
    .unwrap();
    crate::CognitivePackageBindingV1::new(
        "contra-sense/handbook",
        "0.1.0",
        7,
        generation_digest,
        "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
        knowledge,
        crate::CognitiveContextLimits::default(),
    )
    .unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn opcov_agent_create() {
    let path = repo_config_path();
    let file_model = CodeConfig::from_file(&path)
        .expect("read config")
        .default_model;
    let missing = path.with_file_name("opcov-missing.acl");
    assert!(
        Agent::create(missing.to_string_lossy().as_ref())
            .await
            .is_err(),
        "missing config was admitted"
    );
    let agent = Agent::create(path.to_string_lossy().as_ref())
        .await
        .expect("agent.create");
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(base_options(&session_id("create"))),
        )
        .await
        .expect("session from created agent");
    assert_eq!(session.model_name(), file_model.as_deref().unwrap_or(""));
    let effect = file_model.unwrap_or_else(|| "absent".into());
    session.close().await;
    agent.close().await;
    op("agent.create", &effect);
}

#[tokio::test]
async fn opcov_live_session_facts() {
    let config = crate::deep38_support::load_pinned_config();
    let pin = crate::deep38_support::pinned_model(&config);
    let agent = Agent::from_config(config).await.expect("live agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let options = base_options(&session_id("facts")).with_model(pin.clone());
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("live session");
    let prompt = "Reply with the single word ready.";
    let absent_run = "opcov-absent-run";
    assert!(session.runs().await.is_empty());
    assert!(session.run_event_page(absent_run, None, 50).await.is_none());
    assert!(session.run_events(absent_run).await.is_empty());
    assert!(session.active_tools().await.is_empty());

    let completed = tokio::time::timeout(Duration::from_secs(180), session.send(prompt, None))
        .await
        .expect("session.send timed out")
        .expect("session.send");
    let _ = completed.text;

    let history = serde_json::to_string(&session.history()).expect("history json");
    assert!(
        history.contains(prompt),
        "history lost the submitted prompt"
    );
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].prompt, prompt);
    let run_id = runs[0].id.clone();
    let events = session.run_events(&run_id).await;
    let page = session
        .run_event_page(&run_id, None, 500)
        .await
        .expect("run event page");
    assert_eq!(page.events.len(), events.len());
    assert!(session.active_tools().await.is_empty());
    let provider = session
        .model_name()
        .split_once('/')
        .map(|(provider, _)| provider);
    assert_eq!(provider, Some("boyue"));
    assert_eq!(session.model_name(), pin);
    let middleware = session.model_middleware_health();
    assert!(middleware.provider_calls >= 1);
    assert!(middleware.completion_calls >= 1);
    let digest = history_digest(&session);

    assert!(!session.cancel().await);
    assert_eq!(history_digest(&session), digest);
    assert_eq!(session.runs().await[0].id, run_id);

    session.close().await;
    assert_session_closed("session.stream", session.stream("closed", None).await);
    assert_session_closed(
        "session.send_with_attachments",
        session
            .send_with_attachments(
                "closed",
                &[crate::Attachment::png(vec![0x89, 0x50, 0x4E, 0x47])],
                None,
            )
            .await,
    );
    assert_session_closed("session.send", session.send("closed", None).await);
    assert_eq!(history_digest(&session), digest);
    agent.close().await;

    op("session.send", "prompt-history");
    op("session.run", "prompt-history");
    op("session.stream", "SessionClosed");
    op("session.send_with_attachments", "SessionClosed");
    op("session.history", "prompt-history");
    op("session.cancel", "false");
    op("session.runs", "prompt");
    op("session.run_events", "run-id");
    op("session.run_event_page", "run-id");
    op("session.active_tools", "empty");
    op("session.model_middleware_health", "provider-calls");
}

#[tokio::test]
async fn opcov_live_delegation() {
    let config = crate::deep38_support::load_pinned_config();
    let pin = crate::deep38_support::pinned_model(&config);
    let agent = Agent::from_config(config).await.expect("delegation agent");
    let workspace = tempfile::tempdir().expect("workspace");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(base_options(&session_id("delegation")).with_model(pin)),
        )
        .await
        .expect("delegation session");
    assert!(session.subagent_tasks().await.is_empty());
    let first = session
        .register_worker_agent(crate::WorkerAgentSpec::read_only(
            "opcov-reader",
            "Read one workspace fact",
        ))
        .expect("register worker");
    assert_eq!(first.name, "opcov-reader");
    let replaced = session
        .register_worker_agent(crate::WorkerAgentSpec::read_only(
            "opcov-reader",
            "Read one workspace fact",
        ))
        .expect("same worker name replaces the first definition");
    assert_eq!(replaced.name, "opcov-reader");
    assert_eq!(first.name, "opcov-reader");
    let args = json!({});
    assert_ne!(
        first.permissions.check("bash", &args),
        crate::permissions::PermissionDecision::Allow
    );
    assert_ne!(
        first.permissions.check("write", &args),
        crate::permissions::PermissionDecision::Allow
    );
    let before = session.model_middleware_health();
    assert_eq!(before.provider_calls, 0);
    let result = tokio::time::timeout(
        Duration::from_secs(180),
        session.tool(
            "task",
            json!({
                "tasks": [{
                    "agent": "opcov-reader",
                    "description": "read the workspace",
                    "prompt": "Reply with the single word ready."
                }]
            }),
        ),
    )
    .await
    .expect("delegation timed out")
    .expect("session.tasks");
    let _ = result.output;
    let tasks = session.subagent_tasks().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].agent, "opcov-reader");
    assert_eq!(tasks[0].status, crate::SubagentStatus::Completed);
    assert_eq!(tasks[0].success, Some(true));
    assert!(session.pending_subagent_tasks().await.is_empty());
    assert_eq!(
        session.model_middleware_health().provider_calls,
        before.provider_calls
    );
    assert!(session.model_name().starts_with("boyue/"));
    session.close().await;
    agent.close().await;
    op("session.tasks", "opcov-reader");
    op("session.register_worker_agent", "opcov-reader");
    op("session.subagent_tasks", "opcov-reader");
}

#[tokio::test]
async fn opcov_run_control() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (agent, session) =
        open_session(workspace.path(), base_options(&session_id("control"))).await;
    let digest = history_digest(&session);
    assert!(session.run_control_snapshot().await.is_none());
    match session.interrupt(InterruptRequest::new()).await {
        Err(CodeError::RunControl(RunControlError::NoActiveRun)) => {}
        other => panic!("interrupt: {other:?}"),
    }
    assert_eq!(history_digest(&session), digest);
    session.close().await;
    assert_session_closed(
        "session.steer",
        session.steer(SteerRequest::new("do not run")).await,
    );
    assert_eq!(history_digest(&session), digest);
    agent.close().await;
    op("session.steer", "SessionClosed");
    op("session.interrupt", "NoActiveRun");
    op("session.run_control_snapshot", "none");
}

#[tokio::test]
async fn opcov_workspace_projection() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();
    std::fs::write(root.join("listed.txt"), "alpha-token\n").unwrap();
    let init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(root)
        .status()
        .expect("git init");
    assert!(init.success());
    let (agent, session) = open_session(root, base_options(&session_id("workspace"))).await;

    let definitions = session.tool_definitions();
    let presented = session
        .presented_tool_definitions("list the workspace")
        .expect("presented tools");
    assert!(definitions.iter().all(|tool| tool.name != "parallel_task"));
    assert!(presented.iter().all(|tool| tool.name != "parallel_task"));
    println!("parallel_task: absent");

    let tool_ls = session.tool("ls", json!({})).await.expect("session.tool");
    let direct_ls = session.ls(None).await.expect("session.ls");
    let globbed = session.glob("*.txt").await.expect("session.glob");
    let grepped = session.grep("alpha-token").await.expect("session.grep");
    std::fs::write(root.join("later-snapshot.txt"), "later\n").unwrap();
    let glob_text = globbed.join("\n");
    for (label, text) in [
        ("tool", tool_ls.output.as_str()),
        ("ls", direct_ls.output.as_str()),
        ("glob", glob_text.as_str()),
        ("grep", grepped.as_str()),
    ] {
        assert!(
            text.contains("listed.txt") || text.contains("alpha-token"),
            "{label}: {text}"
        );
        assert!(
            !text.contains("later-snapshot.txt"),
            "{label} saw a later file"
        );
    }

    let edited = session
        .edit_file("listed.txt", "alpha-token", "beta-token", false)
        .await
        .expect("session.edit_file");
    assert_eq!(edited.exit_code, 0, "{}", edited.output);
    assert_eq!(
        std::fs::read(root.join("listed.txt")).unwrap(),
        b"beta-token\n"
    );
    let stale_edit = session
        .edit_file("listed.txt", "no-such-token", "other", false)
        .await
        .expect("stale edit");
    let _ = stale_edit.exit_code;
    assert_eq!(
        std::fs::read(root.join("listed.txt")).unwrap(),
        b"beta-token\n"
    );

    let patched = session
        .patch_file("listed.txt", "@@ -1 +1 @@\n-beta-token\n+gamma-token\n")
        .await
        .expect("session.patch_file");
    assert_eq!(patched.exit_code, 0, "{}", patched.output);
    assert_eq!(
        std::fs::read(root.join("listed.txt")).unwrap(),
        b"gamma-token\n"
    );
    let _ = session
        .patch_file("listed.txt", "@@ -1 +1 @@\n-missing-token\n+other\n")
        .await
        .expect("stale patch");
    assert_eq!(
        std::fs::read(root.join("listed.txt")).unwrap(),
        b"gamma-token\n"
    );

    let bash_path = root.join("bash-artifact.txt");
    let bash_outcome = session
        .bash("printf 'opcov-bash-token\\n' > bash-artifact.txt")
        .await;
    let bash_effect = match (bash_outcome, std::fs::read(&bash_path).ok()) {
        (Ok(_), Some(bytes)) => {
            assert_eq!(bytes, b"opcov-bash-token\n");
            let _ = session.bash("exit 2").await;
            assert_eq!(std::fs::read(&bash_path).unwrap(), b"opcov-bash-token\n");
            "opcov-bash-token"
        }
        (Err(error), None) => {
            let text = error.to_string();
            assert!(
                text.contains("the default A3S native sandbox is unavailable"),
                "{text}"
            );
            "sandbox-unavailable"
        }
        (Ok(output), None) => {
            assert!(
                output.contains("the default A3S native sandbox is unavailable"),
                "{output}"
            );
            "sandbox-unavailable"
        }
        (Err(error), Some(_)) => panic!("bash error still wrote a file: {error}"),
    };

    let porcelain = || {
        let output = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(root)
            .output()
            .expect("git status");
        String::from_utf8(output.stdout).expect("porcelain")
    };
    let before_push = porcelain();
    assert!(before_push.contains("listed.txt"), "{before_push}");
    let changes = before_push.lines().filter(|line| !line.is_empty()).count();
    let git_status = session
        .tool("git", json!({"command": "status"}))
        .await
        .expect("session.git");
    assert_eq!(git_status.exit_code, 0, "{}", git_status.output);
    assert!(
        git_status
            .output
            .contains(&format!("{changes} uncommitted")),
        "{}",
        git_status.output
    );
    let rejected_git = session
        .tool("git", json!({"command": "push"}))
        .await
        .expect("git push");
    assert_ne!(rejected_git.exit_code, 0, "{}", rejected_git.output);
    assert_eq!(porcelain(), before_push);

    let download = tokio::time::timeout(
        Duration::from_secs(20),
        session.tool(
            "download",
            json!({
                "url": "http://198.18.0.1/payload.bin",
                "file_path": "payload.bin",
                "timeout": 1
            }),
        ),
    )
    .await
    .expect("download timed out")
    .expect("session.tool:download");
    assert_ne!(download.exit_code, 0);
    assert!(!root.join("payload.bin").exists());

    let batch = session
        .tool(
            "batch",
            json!({"invocations": [{"tool": "ls", "args": {}}]}),
        )
        .await
        .expect("session.tool:batch");
    assert!(batch.output.contains("listed.txt"), "{}", batch.output);
    std::fs::write(root.join("batch-later.txt"), "later\n").unwrap();
    assert!(!batch.output.contains("batch-later.txt"));
    let nested = session
        .tool(
            "batch",
            json!({
                "invocations": [{
                    "tool": "batch",
                    "args": {
                        "invocations": [{
                            "tool": "write",
                            "args": {"file_path": "nested-batch.txt", "content": "nope"}
                        }]
                    }
                }]
            }),
        )
        .await
        .expect("nested batch");
    assert_ne!(nested.exit_code, 0, "{}", nested.output);
    assert!(!root.join("nested-batch.txt").exists());

    let traces = session.trace_events();
    let encoded = serde_json::to_string(&traces).expect("traces");
    assert!(
        traces.iter().any(|event| event.name == "ls"),
        "trace events: {encoded}"
    );
    assert!(!encoded.contains("opcov-secret-not-an-argument"));

    session.close().await;
    agent.close().await;
    op("session.tool", "listed.txt");
    op("session.tool_definitions", "parallel_task-absent");
    op("session.ls", "listed.txt");
    op("session.glob", "listed.txt");
    op("session.grep", "alpha-token");
    op("session.edit_file", "beta-token");
    op("session.patch_file", "gamma-token");
    op("session.bash", bash_effect);
    op("session.git", "uncommitted-count");
    op("session.tool:download", "absent");
    op("session.tool:batch", "listed.txt");
    op("session.trace_events", "ls");
}

#[tokio::test]
async fn opcov_session_files() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();
    let (agent, session) = open_session(root, base_options(&session_id("files"))).await;
    let token = "opcov-file-token";
    let content = format!("{token}\n");
    let path = "opcov-file.txt";
    let missing = "opcov-missing.txt";
    assert!(!root.join(path).exists());
    assert!(!root.join(missing).exists());
    let written = session
        .write_file(path, &content)
        .await
        .expect("session.write_file");
    assert_eq!(written.exit_code, 0, "{}", written.output);
    assert_eq!(
        std::fs::read(root.join(path)).expect("written bytes"),
        content.as_bytes()
    );
    let read = session.read_file(path).await.expect("session.read_file");
    assert!(read.contains(token), "{read}");
    match session.read_file(missing).await {
        Ok(text) => {
            assert!(!text.contains(token), "{text}");
        }
        Err(error) => panic!("missing read: {error}"),
    }
    assert!(!root.join(missing).exists());
    assert_eq!(
        std::fs::read(root.join(path)).expect("written bytes unchanged"),
        content.as_bytes()
    );
    session.close().await;
    agent.close().await;
    op("session.write_file", token);
    op("session.read_file", token);
}

#[tokio::test]
async fn opcov_retrieval_skills() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("listed.txt"), "alpha-token\n").unwrap();
    let (agent, session) =
        open_session(workspace.path(), base_options(&session_id("retrieval"))).await;
    let digest = history_digest(&session);
    let status = session.workspace_retrieval_status();
    assert_eq!(status.phase, crate::WorkspaceRetrievalPhase::Disabled);
    assert_eq!(status.catalog_revision, 0);
    let hybrid = session
        .hybrid_search(crate::WorkspaceHybridSearchRequest::new("alpha-token").with_limit(0))
        .await;
    match hybrid {
        Err(crate::WorkspaceRetrievalError::Unavailable) => {}
        other => panic!("hybrid search: {other:?}"),
    }
    let first_mcp = session.mcp_status().await;
    assert!(!first_mcp.contains_key("opcov-never-added"));
    let second_mcp = session.mcp_status().await;
    assert_eq!(first_mcp.len(), second_mcp.len());
    assert!(!second_mcp.contains_key("opcov-never-added"));

    let skill = crate::skills::Skill::parse(
        "---\nname: opcov-skill\ndescription: operation coverage skill\n---\n# opcov\n",
    )
    .expect("skill");
    session
        .add_skill(Arc::new(skill))
        .expect("session.add_skill");
    assert!(session
        .skill_names()
        .iter()
        .any(|name| name == "opcov-skill"));
    let duplicate = crate::skills::Skill::parse(
        "---\nname: opcov-skill\ndescription: duplicate\n---\n# again\n",
    )
    .expect("duplicate skill");
    session
        .add_skill(Arc::new(duplicate))
        .expect("same skill name replaces the first definition");
    assert_eq!(
        session
            .skill_names()
            .iter()
            .filter(|name| name.as_str() == "opcov-skill")
            .count(),
        1
    );
    assert_eq!(history_digest(&session), digest);
    session.remove_skill("opcov-skill").expect("remove skill");
    assert!(!session
        .skill_names()
        .iter()
        .any(|name| name == "opcov-skill"));
    assert_eq!(history_digest(&session), digest);

    session.close().await;
    match session
        .hybrid_search(crate::WorkspaceHybridSearchRequest::new("alpha-token").with_limit(0))
        .await
    {
        Err(crate::WorkspaceRetrievalError::Unavailable) => {}
        other => panic!("closed hybrid search: {other:?}"),
    }
    agent.close().await;
    op("session.workspace_retrieval_status", "Disabled");
    op("session.hybrid_search", "Unavailable");
    op("session.mcps", "absent");
    op("session.add_skill", "opcov-skill");
    op("session.skill_names", "opcov-skill");
}

#[tokio::test]
async fn opcov_scheduler_and_queue() {
    let workspace = tempfile::tempdir().expect("workspace");
    let options = base_options(&session_id("queue"))
        .with_queue_config(crate::queue::SessionQueueConfig::default());
    let (agent, session) = open_session(workspace.path(), options).await;
    let stats = session.task_scheduler_stats().await.expect("stats");
    let health = session.task_scheduler_health().await.expect("health");
    assert_eq!(stats.active, health.active);
    let pool = session.model_generation_pool_health().await.expect("pool");
    let pool_again = session
        .model_generation_pool_health()
        .await
        .expect("pool again");
    assert_eq!(pool, pool_again);
    let queued = session.queue_stats().await;
    assert_eq!(queued.total_active, 0);
    session
        .set_lane_handler(
            crate::queue::SessionLane::Execute,
            crate::queue::LaneHandlerConfig {
                mode: crate::queue::TaskHandlerMode::External,
                timeout_ms: 60_000,
            },
        )
        .await
        .expect("set lane");
    let after = session.queue_stats().await;
    assert_eq!(after.total_active, 0);
    let lane = after.lanes.get("Execute").expect("Execute lane");
    assert_eq!(lane.handler_mode, crate::queue::TaskHandlerMode::External);
    let held_active = after.total_active;
    let held_pending = after.total_pending;
    session.close().await;
    assert_session_closed(
        "session.set_lane_handler",
        session
            .set_lane_handler(
                crate::queue::SessionLane::Execute,
                crate::queue::LaneHandlerConfig {
                    mode: crate::queue::TaskHandlerMode::External,
                    timeout_ms: 60_000,
                },
            )
            .await,
    );
    let closed_stats = session.queue_stats().await;
    assert_eq!(closed_stats.total_active, held_active);
    assert_eq!(closed_stats.total_pending, held_pending);
    agent.close().await;
    op("session.task_scheduler_stats", "active");
    op("session.task_scheduler_health", "active");
    op("session.model_generation_pool_health", "stable");
    op("session.queue_stats", "Execute");
    op("session.set_lane_handler", "External");
}

#[tokio::test]
async fn opcov_artifact() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut body = String::new();
    for _ in 0..2000 {
        body.push_str(&"a".repeat(90));
        body.push('\n');
    }
    std::fs::write(workspace.path().join("wide.txt"), &body).unwrap();
    let (agent, session) =
        open_session(workspace.path(), base_options(&session_id("artifact"))).await;
    let read = session
        .tool("read", json!({"file_path": "wide.txt"}))
        .await
        .expect("read");
    let uri = read
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/artifact/artifact_uri"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("read metadata {:?}", read.metadata));
    let artifact = session.get_artifact(&uri).expect("session.get_artifact");
    assert_eq!(artifact.artifact_uri, uri);
    assert!(uri.contains(&sha256_hex(artifact.content.as_bytes())));
    assert!(session
        .get_artifact("a3s://tool-output/read/missing")
        .is_none());
    session.close().await;
    agent.close().await;
    op("session.get_artifact", "sha256");
}

#[tokio::test]
async fn opcov_pending_confirmations() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::default(),
        event_tx,
    ));
    let options = base_options(&session_id("confirm"))
        .with_confirmation_manager(
            Arc::clone(&manager) as Arc<dyn crate::hitl::ConfirmationProvider>
        );
    let (agent, session) = open_session(workspace.path(), options).await;
    assert!(session
        .pending_confirmations()
        .await
        .iter()
        .all(|pending| pending.tool_id != "opcov-tool"));
    let pending_response = manager
        .request_confirmation("opcov-tool", "write", &json!({"file_path": "x"}))
        .await;
    assert!(session
        .pending_confirmations()
        .await
        .iter()
        .any(|pending| pending.tool_id == "opcov-tool"));
    assert!(manager
        .confirm("opcov-tool", false, None)
        .await
        .expect("confirm"));
    drop(pending_response);
    assert!(session
        .pending_confirmations()
        .await
        .iter()
        .all(|pending| pending.tool_id != "opcov-tool"));
    session.close().await;
    agent.close().await;
    op("session.pending_confirmations", "opcov-tool");
}

#[tokio::test]
async fn opcov_register_hook() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (agent, session) = open_session(workspace.path(), base_options(&session_id("hook"))).await;
    let digest = history_digest(&session);
    let before = session.hook_count();
    session
        .register_hook(crate::hooks::Hook::new(
            "opcov-block",
            crate::hooks::HookEventType::PreToolUse,
        ))
        .expect("register hook");
    assert_eq!(session.hook_count(), before + 1);
    session
        .register_hook(crate::hooks::Hook::new(
            "opcov-block",
            crate::hooks::HookEventType::PreToolUse,
        ))
        .expect("same hook id replaces the first definition");
    assert_eq!(session.hook_count(), before + 1);
    let calls = Arc::new(AtomicUsize::new(0));
    session
        .register_hook_handler(
            "opcov-block",
            Arc::new(BlockingPreTool {
                calls: Arc::clone(&calls),
            }),
        )
        .expect("hook handler");
    let write = session
        .tool(
            "write",
            json!({"file_path": "hook-target.txt", "content": "nope"}),
        )
        .await
        .expect("hooked write");
    assert_ne!(write.exit_code, 0, "{}", write.output);
    assert!(!workspace.path().join("hook-target.txt").exists());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(history_digest(&session), digest);
    session.close().await;
    agent.close().await;
    op("session.register_hook", "blocked");
}

#[tokio::test]
async fn opcov_verify_commands() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (agent, session) =
        open_session(workspace.path(), base_options(&session_id("verify"))).await;
    let subject = "opcov-verify";
    let report = session
        .verify_commands(
            subject,
            &[crate::verification::VerificationCommand::required(
                "opcov-false",
                "command",
                "a command that exits 1",
                "false",
            )],
        )
        .await
        .expect("session.verify_commands");
    assert_eq!(report.subject, subject);
    assert_ne!(
        report.status,
        crate::verification::VerificationStatus::Passed
    );
    assert_eq!(
        report.status,
        crate::verification::VerificationStatus::Failed
    );
    assert!(report.effect_digest.is_none());
    session.close().await;
    agent.close().await;
    op("session.verify_commands", "Failed");
}

#[tokio::test]
async fn opcov_budget() {
    let workspace = tempfile::tempdir().expect("workspace");
    let options = base_options(&session_id("budget")).with_worker_agent(
        crate::WorkerAgentSpec::read_only("opcov-budget", "Budget boundary"),
    );
    let (agent, session) = open_session(workspace.path(), options).await;
    let guard = Arc::new(DenyingBudget {
        checks: AtomicUsize::new(0),
        records: AtomicUsize::new(0),
    });
    session
        .set_budget_guard(Some(
            Arc::clone(&guard) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .expect("set budget");
    let before = session.model_middleware_health().provider_calls;
    let result = tokio::time::timeout(
        Duration::from_secs(45),
        session.tool(
            "task",
            json!({
                "agent": "opcov-budget",
                "description": "budget check",
                "prompt": "Return a short answer."
            }),
        ),
    )
    .await
    .expect("budget tool timed out")
    .expect("budget task");
    assert_ne!(result.exit_code, 0, "{}", result.output);
    assert_eq!(guard.checks.load(Ordering::SeqCst), 1);
    assert_eq!(guard.records.load(Ordering::SeqCst), 0);
    assert_eq!(session.model_middleware_health().provider_calls, before);
    assert!(!workspace.path().join("budget-sentinel.txt").exists());
    session.close().await;
    agent.close().await;
    op("session.set_budget_guard", "Deny");
}

#[tokio::test]
async fn opcov_memory() {
    let workspace = tempfile::tempdir().expect("workspace");
    let options =
        base_options(&session_id("memory")).with_memory(Arc::new(a3s_memory::InMemoryStore::new()));
    let (agent, session) = open_session(workspace.path(), options).await;
    let memory = Arc::clone(session.memory().expect("session.memory"));
    memory
        .remember(a3s_memory::MemoryItem::new("opcov-active-canary-zeta"))
        .await
        .expect("remember active");
    memory
        .remember(
            a3s_memory::MemoryItem::new("opcov-superseded-canary-quota")
                .with_metadata("a3s.memory.status", "superseded"),
        )
        .await
        .expect("remember superseded");
    let recalled = memory
        .recall_similar("opcov-active-canary-zeta", 10)
        .await
        .expect("recall");
    let recent = memory.get_recent(10).await.expect("recent");
    assert!(recalled
        .iter()
        .any(|item| item.content == "opcov-active-canary-zeta"));
    assert!(recalled
        .iter()
        .all(|item| item.content != "opcov-superseded-canary-quota"));
    assert!(recent
        .iter()
        .all(|item| item.content != "opcov-superseded-canary-quota"));
    let stats = memory.stats().await.expect("stats");
    assert_eq!(stats.long_term_count, 2);
    session.close().await;
    agent.close().await;
    op("session.memory", "present");
    op("session.remember", "opcov-active-canary-zeta");
    op("session.memory_stats", "retained-2");
}

#[tokio::test]
async fn opcov_code_intelligence() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("admitted.rs"),
        "fn admitted_symbol() {}\n",
    )
    .unwrap();
    let (agent, session) = open_session(workspace.path(), base_options(&session_id("intel"))).await;
    let symbols = session
        .tool(
            "code_symbols",
            json!({"operation": "outline", "path": "admitted.rs"}),
        )
        .await
        .expect("code_symbols");
    let navigation = session
        .tool(
            "code_navigation",
            json!({"operation": "definition", "path": "admitted.rs", "line": 0, "character": 0}),
        )
        .await
        .expect("code_navigation");
    let diagnostics = session
        .tool("code_diagnostics", json!({"path": "admitted.rs"}))
        .await
        .expect("code_diagnostics");
    std::fs::write(
        workspace.path().join("sentinel-later.rs"),
        "fn sentinel_later() {}\n",
    )
    .unwrap();
    let mut effects = Vec::new();
    for (name, result) in [
        ("session.tool:code_symbols", symbols),
        ("session.tool:code_navigation", navigation),
        ("session.tool:code_diagnostics", diagnostics),
    ] {
        assert!(
            !result.output.contains("sentinel-later.rs"),
            "{name}: {}",
            result.output
        );
        if result.exit_code == 0 {
            assert!(
                result.output.contains("admitted"),
                "{name}: {}",
                result.output
            );
            effects.push("admitted.rs");
        } else {
            effects.push("unavailable");
        }
    }
    session.close().await;
    agent.close().await;
    op("session.tool:code_symbols", effects[0]);
    op("session.tool:code_navigation", effects[1]);
    op("session.tool:code_diagnostics", effects[2]);
}

#[tokio::test]
async fn opcov_cognitive_current() {
    let workspace = tempfile::tempdir().expect("workspace");
    let (agent, bare) = open_session(
        workspace.path(),
        base_options(&session_id("cognitive-none")),
    )
    .await;
    assert!(bare.current_cognitive_package_binding().is_none());
    bare.close().await;
    let binding = cognitive_binding();
    let digest = binding.generation_digest.clone();
    let context = crate::CognitiveContextSession::new(binding, Arc::new(StubCognitive))
        .expect("cognitive session");
    let bound = agent
        .session_async(
            workspace.path().to_string_lossy().as_ref(),
            Some(base_options(&session_id("cognitive-bound")).with_cognitive_context(context)),
        )
        .await
        .expect("bound session");
    let current = bound
        .current_cognitive_package_binding()
        .expect("current binding");
    assert_eq!(current.generation_digest, digest);
    bound.close().await;
    agent.close().await;
    op("session.current_cognitive_package_binding", &digest);
}

#[tokio::test]
async fn opcov_protocol_recover() {
    let workspace = tempfile::tempdir().expect("workspace");
    let options = base_options(&session_id("recover"))
        .with_session_store(Arc::new(crate::store::MemorySessionStore::new()));
    let (agent, session) = open_session(workspace.path(), options).await;
    let session = Arc::new(session);
    let release_identity = format!("sha256:{}", "d".repeat(64));
    let host = crate::AgentProtocolHost::new(release_identity.clone(), Arc::clone(&session))
        .expect("protocol host");
    let checkpoint_run_id = "opcov-missing-checkpoint";
    let run_id = "opcov-recovery-run";
    let identity = crate::AgentProtocolRunIdentityV1 {
        schema: crate::AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: crate::AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release_identity,
        session_id: session.session_id().into(),
        run_id: run_id.into(),
    };
    let recover = crate::AgentProtocolCommandV1::Recover {
        request: crate::AgentProtocolRunRecoverV1 {
            schema: crate::AgentProtocolRunRecoverV1::SCHEMA.into(),
            request_id: "opcov-protocol-recover".into(),
            identity,
            checkpoint_run_id: checkpoint_run_id.into(),
        },
    };
    assert_ne!(checkpoint_run_id, run_id);
    match host.execute(&recover).await {
        Err(crate::AgentProtocolHostError::Code(CodeError::Session(message))) => {
            assert!(message.contains("no loop checkpoint found"), "{message}");
        }
        other => panic!("protocol recover: {other:?}"),
    }
    assert!(session.runs().await.iter().all(|run| run.id != run_id));
    session.close().await;
    agent.close().await;
    op("agent_protocol.recover", "Code");
}

struct PendingProtocolClient;

#[async_trait::async_trait]
impl crate::LlmClient for PendingProtocolClient {
    async fn complete(
        &self,
        _messages: &[crate::llm::Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::LlmResponse> {
        anyhow::bail!("protocol cancel fixture does not complete")
    }

    async fn complete_streaming(
        &self,
        _messages: &[crate::llm::Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender
                .send(crate::llm::StreamEvent::TextDelta("working".into()))
                .await;
            cancel_token.cancelled().await;
        });
        Ok(receiver)
    }
}

fn offline_protocol_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/static".into()),
        providers: vec![crate::config::ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![crate::config::ModelConfig {
                id: "static".into(),
                name: "Static".into(),
                family: "fixture".into(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: crate::config::ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn opcov_protocol_cancel() {
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = Agent::from_config(offline_protocol_config())
        .await
        .expect("offline agent");
    let session_id = session_id("protocol-cancel");
    let run_id = "opcov-protocol-cancel";
    let session = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy().as_ref(),
                Some(
                    SessionOptions::new()
                        .with_session_id(&session_id)
                        .with_llm_client(Arc::new(PendingProtocolClient)),
                ),
            )
            .await
            .expect("protocol session"),
    );
    let release_identity = format!("sha256:{}", "c".repeat(64));
    let host = crate::AgentProtocolHost::new(release_identity.clone(), Arc::clone(&session))
        .expect("protocol host");
    let identity = crate::AgentProtocolRunIdentityV1 {
        schema: crate::AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: crate::AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release_identity,
        session_id: session.session_id().into(),
        run_id: run_id.into(),
    };
    let start = crate::AgentProtocolCommandV1::Start {
        request: crate::AgentProtocolRunStartV1 {
            schema: crate::AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: "opcov-protocol-cancel-start".into(),
            identity: identity.clone(),
            prompt: "wait for protocol cancel".into(),
        },
    };
    let start_receipt = host.execute(&start).await.expect("protocol start");
    let start_digest = start.digest().expect("start digest");
    assert_eq!(start_receipt.identity, identity);
    assert_eq!(start_receipt.command_digest, start_digest);
    assert_eq!(
        start_receipt.action,
        crate::AgentProtocolCommandActionV1::Start
    );
    assert!(!start_receipt.replayed);
    let start_effect = match start_receipt.state {
        crate::AgentProtocolRunStateV1::Created => "Created",
        crate::AgentProtocolRunStateV1::Planning => "Planning",
        crate::AgentProtocolRunStateV1::Executing => "Executing",
        crate::AgentProtocolRunStateV1::Verifying => "Verifying",
        state => panic!("protocol start settled early: {}", state.as_str()),
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if session
                .run_snapshot(run_id)
                .await
                .is_some_and(|snapshot| !snapshot.status.is_terminal() && snapshot.event_count > 0)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("active run before protocol cancel");

    let cancel = crate::AgentProtocolCommandV1::Cancel {
        request: crate::AgentProtocolRunCancelV1 {
            schema: crate::AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "opcov-protocol-cancel-cancel".into(),
            identity: identity.clone(),
            reason: "opcov".into(),
        },
    };
    let receipt = host.execute(&cancel).await.expect("protocol cancel");
    let digest = cancel.digest().expect("cancel digest");
    assert_eq!(receipt.identity, identity);
    assert_eq!(receipt.command_digest, digest);
    assert_eq!(receipt.state, crate::AgentProtocolRunStateV1::Cancelled);
    assert!(!receipt.replayed);
    receipt
        .validate_for(&cancel)
        .expect("receipt binds the cancel");
    let settled = session.run_snapshot(run_id).await.expect("settled run");
    assert_eq!(settled.id, run_id);
    assert_eq!(settled.status, crate::RunStatus::Cancelled);

    let mut missing_identity = identity.clone();
    missing_identity.run_id = "opcov-protocol-cancel-absent".into();
    let missing = crate::AgentProtocolCommandV1::Cancel {
        request: crate::AgentProtocolRunCancelV1 {
            schema: crate::AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "opcov-protocol-cancel-missing".into(),
            identity: missing_identity,
            reason: "opcov".into(),
        },
    };
    match host.execute(&missing).await {
        Err(crate::AgentProtocolHostError::RunNotFound) => {}
        other => panic!("missing protocol cancel: {other:?}"),
    }
    assert!(session
        .runs()
        .await
        .iter()
        .all(|run| run.id != "opcov-protocol-cancel-absent"));
    let unchanged = session.run_snapshot(run_id).await.expect("accepted run");
    assert_eq!(unchanged.id, run_id);
    assert_eq!(unchanged.status, crate::RunStatus::Cancelled);
    session.close().await;
    agent.close().await;
    op("agent_protocol.start", start_effect);
    op("agent_protocol.cancel", "Cancelled");
}

struct ParallelIdentityExecutor {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::AgentExecutor for ParallelIdentityExecutor {
    async fn execute_step(
        &self,
        spec: crate::AgentStepSpec,
        _event_tx: Option<tokio::sync::broadcast::Sender<crate::AgentEvent>>,
    ) -> crate::StepOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if spec.task_id == "opcov-parallel-reject" {
            return crate::StepOutcome::failed(spec.task_id, spec.agent, "rejected");
        }
        crate::StepOutcome {
            task_id: spec.task_id.clone(),
            session_id: format!("task-run-{}", spec.task_id),
            agent: spec.agent,
            output: String::new(),
            success: true,
            structured: None,
            source_anchors: Vec::new(),
        }
    }
}

#[tokio::test]
async fn opcov_parallel() {
    let executor = Arc::new(ParallelIdentityExecutor {
        calls: AtomicUsize::new(0),
    });
    let workflow =
        crate::Workflow::builder(executor.clone() as Arc<dyn crate::AgentExecutor>).build();
    let outcomes = workflow
        .parallel(vec![
            crate::AgentStepSpec::new("opcov-parallel", "opcov-agent", "admit", "admit-one"),
            crate::AgentStepSpec::new(
                "opcov-parallel-reject",
                "opcov-agent",
                "reject",
                "reject-one",
            ),
        ])
        .await;
    assert_eq!(executor.calls.load(Ordering::SeqCst), 2);
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].task_id, "opcov-parallel");
    assert_eq!(outcomes[0].agent, "opcov-agent");
    assert!(outcomes[0].success);
    assert_eq!(outcomes[1].task_id, "opcov-parallel-reject");
    assert_eq!(outcomes[1].agent, "opcov-agent");
    assert!(!outcomes[1].success);
    op("session.parallel", "opcov-parallel");
}

#[test]
fn opcov_workflow_step() {
    let spec = crate::AgentStepSpec::new(
        "opcov-step",
        "opcov-reader",
        "one step",
        "Reply with the single word ready.",
    );
    let outcome = crate::StepOutcome {
        task_id: spec.task_id.clone(),
        session_id: "opcov-step-session".into(),
        agent: spec.agent.clone(),
        output: "cached-kernel".into(),
        success: true,
        structured: None,
        source_anchors: Vec::new(),
    };
    let identity =
        crate::workflow_step_execution_identity("opcov-workflow", &spec).expect("identity");
    let receipt = crate::workflow_step_result_receipt("opcov-workflow", &spec, &outcome, None)
        .expect("receipt");
    assert_eq!(receipt.identity, identity);
    let again = crate::workflow_step_result_receipt("opcov-workflow", &spec, &outcome, None)
        .expect("receipt again");
    assert_eq!(again.identity, identity);
    let encoded = serde_json::to_string(&receipt).expect("receipt json");
    assert!(!encoded.contains("cached-kernel"));
    let mut mismatched = spec.clone();
    mismatched.task_id = "other-step".into();
    match crate::workflow_step_result_receipt("opcov-workflow", &mismatched, &outcome, None) {
        Err(crate::execution_identity::ExecutionIdentityError::InvalidReceiptField("task_id")) => {}
        other => panic!("mismatched receipt: {other:?}"),
    }
    assert_eq!(
        crate::workflow_step_execution_identity("opcov-workflow", &spec).expect("identity stays"),
        identity
    );
    op("session.workflow_step", "identity");
}

#[cfg(feature = "state-graph")]
#[test]
fn opcov_state_graph() {
    let mut runtime = crate::GraphRuntime::new();
    let before = runtime.graph().state_hash().expect("empty hash");
    let applied = runtime
        .propose_patch(
            crate::GraphPatch::new(
                0,
                vec![crate::PatchOperation::AddObject {
                    id: "a".into(),
                    object_type: "task".into(),
                    data: json!({}),
                }],
            ),
            None,
        )
        .expect("propose setup");
    assert!(applied);
    let head = runtime.graph().state_hash().expect("head hash");
    assert_ne!(head, before);
    let restored = crate::GraphRuntime::restore(runtime.events().to_vec()).expect("restore");
    assert_eq!(restored.graph().state_hash().expect("restored hash"), head);
    assert!(runtime.diff(&restored).is_empty());
    op("state_graph.create", "state-hash");
    op("state_graph.restore", "state-hash");
    op("state_graph.diff", "empty");
}

#[test]
fn opcov_release_bind() {
    let raw = include_str!("../../fixtures/agent-release-contract/.a3s/asset.acl");
    let manifest =
        crate::release::AgentReleaseManifest::parse(&raw.replace("\r\n", "\n")).expect("fixture");
    let original = manifest.identity().to_string();
    let submitted = "sha256:3333333333333333333333333333333333333333333333333333333333333333";
    let bound = manifest
        .bind_publication(
            submitted,
            [
                crate::release::AgentReleaseProvenance::new(
                    "source",
                    "https://github.com/A3S-Lab/Code",
                    "sha256:2222222222222222222222222222222222222222222222222222222222222222",
                )
                .expect("source"),
                crate::release::AgentReleaseProvenance::new(
                    "builder",
                    "urn:a3s:builder:github-actions",
                    "sha256:4444444444444444444444444444444444444444444444444444444444444444",
                )
                .expect("builder"),
            ],
        )
        .expect("bind");
    assert_eq!(bound.artifact().digest(), submitted);
    assert_eq!(manifest.identity(), original);
    match manifest.bind_publication(
        submitted,
        [crate::release::AgentReleaseProvenance::new(
            "publisher",
            "https://github.com/A3S-Lab/Code",
            "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        )
        .expect("publisher")],
    ) {
        Err(crate::release::AgentReleaseError::InvalidField(
            crate::release::AgentReleaseField::ProvenanceKind,
        )) => {}
        other => panic!("rejected provenance: {other:?}"),
    }
    assert_eq!(manifest.identity(), original);
    op("release.bind_publication", "artifact-digest");
}

#[cfg(feature = "evaluation")]
struct TurnPolicy;

#[cfg(feature = "evaluation")]
impl crate::EvaluationPolicy for TurnPolicy {
    fn plan(&self, _fact: &crate::ExecutionFactV1) -> Option<crate::EvaluationPlanV1> {
        Some(crate::EvaluationPlanV1::new(
            crate::EvaluationBoundaryV1::TurnEnd,
            "turn-check",
            "inspect the bounded evidence",
        ))
    }
}

#[cfg(feature = "evaluation")]
struct RecordingExecutor;

#[cfg(feature = "evaluation")]
#[async_trait::async_trait]
impl crate::AuxiliaryExecutor for RecordingExecutor {
    async fn execute(
        &self,
        context: crate::AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, crate::AuxiliaryRunError> {
        Ok(json!({
            "sequence": context.evidence.events.first().map(|event| event.sequence)
        }))
    }
}

#[cfg(feature = "evaluation")]
#[tokio::test]
async fn opcov_evaluation() {
    let runs = Arc::new(crate::InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("opcov-run".into(), "opcov-session", "prompt")
        .await;
    let target = crate::ExecutionTargetV1::new("opcov-session", &run.id);
    runs.record_event(
        &run.id,
        crate::AgentEvent::TurnEnd {
            turn: 1,
            usage: crate::TokenUsage::default(),
        },
    )
    .await;
    let request = crate::EvidenceReadRequestV1::new(target.clone());
    let snapshot = crate::RunEvidenceReader::new(Arc::clone(&runs))
        .read(request.clone())
        .await
        .expect("evidence");
    assert_eq!(snapshot.target, request.target);
    let ledger = Arc::new(crate::InMemoryEvaluationDispatchLedger::new());
    let supervisor = crate::EvaluationSupervisor::with_dispatch_ledger(
        Arc::new(crate::InMemoryExecutionFactJournal::new()),
        Arc::new(crate::RunEvidenceReader::new(Arc::clone(&runs))),
        Arc::new(crate::InMemoryAuxiliaryRunService::new(Arc::new(
            RecordingExecutor,
        ))),
        Arc::new(TurnPolicy),
        ledger,
    );
    let start = crate::RunEventRecord {
        sequence: 0,
        timestamp_ms: 1,
        event: crate::AgentEvent::TurnStart { turn: 1 },
    };
    let ignored = supervisor
        .observe_event(crate::ExecutionFrameV1::root(target.clone()), &start)
        .await
        .expect("turn start");
    assert_eq!(ignored.outcome, crate::EvaluationDispatchOutcome::Ignored);
    let end = crate::RunEventRecord {
        sequence: 1,
        timestamp_ms: 2,
        event: crate::AgentEvent::TurnEnd {
            turn: 1,
            usage: crate::TokenUsage::default(),
        },
    };
    let frame = crate::ExecutionFrameV1::root(target.clone());
    let dispatched = supervisor
        .observe_event(frame.clone(), &end)
        .await
        .expect("turn end");
    assert_eq!(
        dispatched.outcome,
        crate::EvaluationDispatchOutcome::Dispatched
    );
    assert_eq!(dispatched.fact.frame.target, target);
    dispatched
        .handle
        .expect("dispatch handle")
        .wait()
        .await
        .expect("dispatch wait");
    let replay = supervisor.observe_event(frame, &end).await.expect("replay");
    assert_eq!(replay.outcome, crate::EvaluationDispatchOutcome::Ignored);
    let envelope = crate::EvaluationWireEnvelopeV1::new(
        crate::EvaluationWireKindV1::EvidenceReadRequest,
        serde_json::to_value(&request).expect("request json"),
    )
    .expect("envelope");
    let bytes = envelope.to_vec().expect("envelope bytes");
    assert_eq!(
        crate::EvaluationWireEnvelopeV1::from_slice(&bytes).expect("roundtrip"),
        envelope
    );
    let mut mutated = serde_json::to_value(&envelope).expect("envelope json");
    mutated["kind"] = json!("not-a-kind");
    match crate::EvaluationWireEnvelopeV1::from_value(mutated) {
        Err(crate::EvaluationProtocolError::UnknownKind) => {}
        other => panic!("unknown kind: {other:?}"),
    }
    assert_eq!(
        crate::EvaluationWireEnvelopeV1::from_slice(&bytes).expect("bytes stay"),
        envelope
    );
    op("evaluation.evidence", "target");
    op("evaluation.auxiliary", "Dispatched");
    op("evaluation.dispatch_ledger", "Ignored");
    op("evaluation.wire_v1", "roundtrip");
}

#[cfg(feature = "apofasi")]
#[test]
fn opcov_typed_decision() {
    let request: crate::SystemOneRequest = serde_json::from_value(json!({
        "state": "Help! My payouts have been failing for 3 days.",
        "questions": {
            "is_urgent": {
                "type": "noul",
                "instructions": "Does this convey urgency?",
                "criteria": {
                    "true": "Explicitly time-sensitive",
                    "false": "No urgency expressed"
                }
            },
            "department": {
                "type": "choice",
                "instructions": "Which team should handle this?",
                "criteria": {
                    "billing": "Payments, invoicing, refunds",
                    "technical": "Bugs, outages, integrations",
                    "sales": "Pricing, upgrades, new accounts"
                }
            },
            "frustration": {
                "type": "score",
                "instructions": "How frustrated is the customer?",
                "criteria": ["Calm", "Frustrated", "Very angry"]
            }
        }
    }))
    .expect("request");
    use crate::TypedDecisionEngine;
    let service = crate::TypedDecisionService::lexical();
    let policy = crate::GatePolicy::default();
    let decided = service.decide(request.clone()).expect("decide");
    let (gated, receipt) = service
        .decide_and_gate_with_receipt(request.clone(), &policy)
        .expect("gate");
    let (_, again) = service
        .decide_and_gate_with_receipt(request.clone(), &policy)
        .expect("gate again");
    assert!(gated.answers.len() > 1);
    assert_eq!(decided.model, gated.model);
    assert_eq!(gated.model, receipt.model);
    assert_eq!(receipt.request_digest, again.request_digest);
    let expected =
        crate::digest_json("a3s.code.typed_decision.request.v1", &request).expect("request digest");
    assert_eq!(receipt.request_digest, expected);
    let mut rejected = request.clone();
    rejected.questions.clear();
    assert!(service.decide(rejected).is_err());
    assert_eq!(receipt.request_digest, expected);
    let route = service.route(&request, None, None).expect("route");
    let route_again = service.route(&request, None, None).expect("route again");
    assert_eq!(route.model, route_again.model);
    op("typed_decision.decide", "model");
    op("typed_decision.decide_and_gate", "model");
    op("typed_decision.receipt_v1", "request-digest");
    op("typed_decision.route", "checkpoint");
}

#[test]
fn opcov_moli() {
    let version = crate::moli_runtime::default_moli_version();
    assert_eq!(version, crate::moli_runtime::DEFAULT_MOLI_VERSION);
    assert!(!version.is_empty());
    let packaged = crate::moli_runtime::packaged_moli();
    let effect = match &packaged {
        Some(path) => {
            assert!(path.is_absolute());
            let mode = std::fs::metadata(path)
                .expect("packaged metadata")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0);
            "executable"
        }
        None => "absent",
    };
    op("moli.default", version);
    op("moli.packaged", effect);
}

#[tokio::test]
async fn opcov_moli_ensure() {
    let root = tempfile::tempdir().expect("workspace");
    let executable = root.path().join("opcov-moli");
    std::fs::write(&executable, b"#!/bin/sh\n").expect("executable bytes");
    let mut permissions = std::fs::metadata(&executable)
        .expect("executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).expect("executable mode");
    let mut config = crate::config::HeadlessConfig {
        browser_path: Some(executable.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let ensured = crate::moli_runtime::ensure_moli(&config, Duration::from_secs(1))
        .await
        .expect("ensure explicit executable");
    assert_eq!(ensured, executable);

    let missing = root.path().join("opcov-moli-missing");
    assert!(!missing.exists());
    config.browser_path = Some(missing.to_string_lossy().into_owned());
    match crate::moli_runtime::ensure_moli(&config, Duration::from_secs(1)).await {
        Ok(path) => panic!("missing moli executable admitted: {}", path.display()),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("configured Moli executable is missing or not executable"),
                "{message}"
            );
            assert!(
                message.contains(&missing.display().to_string()),
                "{message}"
            );
        }
    }
    assert!(!missing.exists());
    op("moli.ensure", "opcov-moli");
}

#[cfg(feature = "telemetry")]
#[tokio::test]
async fn opcov_telemetry_init() {
    let guard = crate::telemetry_otel::TelemetryConfig::new("http://127.0.0.1:9")
        .with_service_name("opcov")
        .init()
        .expect("telemetry.init");
    guard.shutdown();
    op("telemetry.init", "shutdown");
}
