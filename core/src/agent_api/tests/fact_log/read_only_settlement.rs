#![cfg(test)]

use super::*;
use crate::skills::SkillRegistry;
use a3s_effect::{ToolExecutionOutcome, ToolExecutionResultV1, ToolPreparedV1};

async fn unobservable_workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let mut git =
        std::process::Command::new(crate::git::trusted_git_executable(root.path()).unwrap());
    crate::git::configure_git_environment(&mut git, root.path());
    git.args(["init", "--quiet"]);
    assert!(crate::tools::process::status_std_child(&mut git)
        .unwrap()
        .success());
    std::fs::File::create(root.path().join("large-untracked.bin"))
        .unwrap()
        .set_len(crate::porcelain::OBSERVATION_BYTE_CAP + 1)
        .unwrap();
    assert!(crate::porcelain::RunWatch::start(root.path())
        .await
        .stamps_incomplete());

    root
}

#[tokio::test]
async fn skill_lookup_finishes_when_the_workspace_exceeds_the_observation_budget() {
    let root = unobservable_workspace().await;
    let packages = tempfile::tempdir().unwrap();
    let registry = Arc::new(SkillRegistry::new());
    std::fs::create_dir(packages.path().join("lookup-contract")).unwrap();
    std::fs::write(
        packages.path().join("lookup-contract/SKILL.md"),
        "---\nname: lookup-contract\ndescription: A newly installed lookup-contract skill\n---\nUse this instruction.\n",
    ).unwrap();
    registry.load_from_dir(packages.path()).unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(ScriptedClient {
        seen: Arc::clone(&seen),
        responses: std::sync::Mutex::new(vec![
            tool_response(
                "lookup-present",
                "search_skills",
                serde_json::json!({"query":"lookup-contract"}),
            ),
            tool_response(
                "lookup-absent",
                "search_skills",
                serde_json::json!({"query":"nonexistent-skill-8bf927"}),
            ),
            tool_response(
                "missing-directory",
                "ls",
                serde_json::json!({"path":"absent-directory"}),
            ),
            text_response("The skill was found; the other skill and directory do not exist."),
        ]),
    });
    let agent = durable_source::source_agent().await;
    let session = session_for(
        &agent,
        root.path(),
        "skill-lookup-budget",
        client,
        &crate::SessionOptions::new()
            .with_skill_registry(registry)
            .with_workspace_backend(crate::workspace::WorkspaceServices::local(root.path()))
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
            .with_auto_delegation_enabled(false)
            .with_continuation(false),
    );
    let result = session
        .send("Find the skill and check the missing directory.", None)
        .await
        .unwrap();
    assert_eq!(
        result.text,
        "The skill was found; the other skill and directory do not exist."
    );
    let facts = read_workspace_facts(root.path(), "skill-lookup-budget").unwrap();
    for (id, outcome, exit_code) in [
        ("lookup-present", ToolExecutionOutcome::Succeeded, 0),
        ("lookup-absent", ToolExecutionOutcome::Succeeded, 0),
        ("missing-directory", ToolExecutionOutcome::Failed, 1),
    ] {
        let prepared: ToolPreparedV1 = serde_json::from_value(
            facts
                .iter()
                .find(|fact| {
                    fact.kind == "tool.prepared"
                        && fact.payload["prepared"]["identity"]["tool_call_id"] == id
                })
                .unwrap()
                .payload["prepared"]
                .clone(),
        )
        .unwrap();
        let receipt: ToolExecutionResultV1 = serde_json::from_value(
            facts
                .iter()
                .find(|fact| fact.kind == "tool.result" && fact.payload["toolCallId"] == id)
                .unwrap()
                .payload["receipt"]
                .clone(),
        )
        .unwrap();
        receipt.ensure_prepared(&prepared).unwrap();
        assert_eq!(receipt.outcome, outcome, "{id}");
        assert_eq!(receipt.exit_code, Some(exit_code), "{id}");
        assert_eq!(
            receipt.effects,
            a3s_effect::ToolEffectObservationV1::complete(Vec::new()),
            "{id}"
        );
        if id == "lookup-present" {
            let envelope: serde_json::Value =
                serde_json::from_str(receipt.output.as_str().unwrap()).unwrap();
            assert_eq!(envelope["metadata"]["skills"][0]["name"], "lookup-contract");
        }
    }
    assert!(!root.path().join("absent-directory").exists());
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn structured_generation_finishes_without_workspace_effect_observation() {
    let root = unobservable_workspace().await;
    let agent = durable_source::source_agent().await;
    let client = Arc::new(ScriptedClient {
        seen: Arc::new(std::sync::Mutex::new(Vec::new())),
        responses: std::sync::Mutex::new(vec![
            tool_response(
                "generate-without-files",
                "generate_object",
                serde_json::json!({
                    "schema":{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false},
                    "prompt":"Return an object with answer 42.", "max_repair_attempts":0
                }),
            ),
            text_response("{\"answer\":42}"),
            text_response("The structured result is ready."),
        ]),
    });
    let session = session_for(
        &agent,
        root.path(),
        "structured-read-only",
        client,
        &crate::SessionOptions::new()
            .with_workspace_backend(crate::workspace::WorkspaceServices::local(root.path()))
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
            .with_auto_delegation_enabled(false)
            .with_continuation(false),
    );
    let result = session
        .send("Generate the requested JSON without changing files.", None)
        .await
        .unwrap();
    assert_eq!(result.text, "The structured result is ready.");
    let facts = read_workspace_facts(root.path(), "structured-read-only").unwrap();
    let receipt: ToolExecutionResultV1 = serde_json::from_value(
        facts
            .iter()
            .find(|fact| {
                fact.kind == "tool.result" && fact.payload["toolCallId"] == "generate-without-files"
            })
            .unwrap()
            .payload["receipt"]
            .clone(),
    )
    .unwrap();
    assert_eq!(receipt.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(
        receipt.effects,
        a3s_effect::ToolEffectObservationV1::complete(Vec::new())
    );
    let envelope: serde_json::Value =
        serde_json::from_str(receipt.output.as_str().unwrap()).unwrap();
    let generated: serde_json::Value =
        serde_json::from_str(envelope["text"].as_str().unwrap()).unwrap();
    assert_eq!(generated["object"]["answer"], 42);
    session.close().await;
    agent.close().await;
}
