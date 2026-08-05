//! End-to-end smoke test for automatic subagent parallel delegation.
//!
//! Run with:
//!   A3S_CONFIG_FILE=/Users/roylin/code/a3s/.a3s/config.acl \
//!     cargo test -p a3s-code-core --test test_auto_delegation_real_parallel -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Duration;

use a3s_code_core::{Agent, AgentEvent, AutoDelegationConfig, CodeConfig, SessionOptions};

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn auto_delegation_triggers_unified_task_fanout_with_real_provider() {
    let config_path = repo_config_path();
    let config = CodeConfig::from_file(&config_path)
        .unwrap_or_else(|err| panic!("failed to load {}: {err}", config_path.display()));
    let agent = Agent::from_config(config)
        .await
        .expect("agent from real config");

    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::write(
        workspace.path().join("README.md"),
        "# Auto Delegation Smoke Test\n\nThis workspace is intentionally tiny.\n",
    )
    .expect("write smoke file");

    let auto = AutoDelegationConfig {
        enabled: true,
        auto_parallel: true,
        max_tasks: 4,
        ..Default::default()
    };

    let opts = SessionOptions::new()
        .with_planning(false)
        .with_max_tool_rounds(8)
        .with_max_parallel_tasks(4)
        .with_auto_delegation(auto);

    let session = agent
        .session_async(workspace.path().display().to_string(), Some(opts))
        .await
        .expect("session");

    let prompt = "Search and inspect this tiny smoke-test workspace, plan the approach, \
        review code quality, and verify regression-test confidence. \
        This is a smoke test; keep every specialist result compact and do not modify files.";
    let (mut rx, handle) = session.stream(prompt, None).await.expect("stream starts");

    let fanout_task_starts = tokio::time::timeout(Duration::from_secs(240), async {
        let mut fanout_task_starts = 0_usize;

        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::ToolExecutionStart { name, args, .. } if name == "task" => {
                    if args
                        .get("tasks")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|tasks| tasks.len() > 1)
                    {
                        fanout_task_starts += 1;
                        return fanout_task_starts;
                    }
                }
                AgentEvent::Error { message } => {
                    panic!("stream failed before task fan-out started: {message}");
                }
                _ => {}
            }
        }

        panic!("stream ended before task fan-out started")
    })
    .await
    .expect("timed out waiting for automatic task fan-out start");

    let _ = session.cancel().await;
    handle.abort();
    let _ = handle.await;

    println!("task fan-out starts: {fanout_task_starts}");

    assert_eq!(
        fanout_task_starts, 1,
        "auto delegation should collapse multiple specialist matches into one task call"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn builtin_subagents_execute_with_real_provider() {
    let config_path = repo_config_path();
    let config = CodeConfig::from_file(&config_path)
        .unwrap_or_else(|err| panic!("failed to load {}: {err}", config_path.display()));
    let agent = Agent::from_config(config)
        .await
        .expect("agent from real config");

    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::write(
        workspace.path().join("README.md"),
        "# Builtin Subagent Smoke Test\n\nNo tool use is required for this test.\n",
    )
    .expect("write smoke file");

    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .expect("session");

    let tasks = serde_json::json!({
        "tasks": [
            {
                "agent": "explore",
                "description": "Smoke explore",
                "prompt": "Do not call tools. Reply with one line: BUILTIN_OK explore.",
                "max_steps": 1
            },
            {
                "agent": "plan",
                "description": "Smoke plan",
                "prompt": "Do not call tools. Reply with one line: BUILTIN_OK plan.",
                "max_steps": 1
            },
            {
                "agent": "review",
                "description": "Smoke review",
                "prompt": "Do not call tools. Reply with one line: BUILTIN_OK review.",
                "max_steps": 1
            },
            {
                "agent": "verification",
                "description": "Smoke verification",
                "prompt": "Do not call tools. Reply with one line: BUILTIN_OK verification.",
                "max_steps": 1
            },
            {
                "agent": "general-purpose",
                "description": "Smoke general-purpose alias",
                "prompt": "Do not call tools. Reply with one line: BUILTIN_OK general-purpose.",
                "max_steps": 1
            }
        ]
    });

    let result = tokio::time::timeout(
        Duration::from_secs(240),
        session.tool("parallel_task", tasks),
    )
    .await
    .expect("timed out waiting for built-in subagents")
    .expect("parallel_task tool should execute");

    println!("parallel_task exit_code: {}", result.exit_code);
    println!("parallel_task output: {}", result.output);

    assert_eq!(result.exit_code, 0, "{}", result.output);
    let metadata = result.metadata.expect("parallel_task metadata");
    assert_eq!(metadata["task_count"], 5);
    let results = metadata["results"].as_array().expect("results");
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|result| result["success"] == true));
}
