//! Live-provider qualification for model-visible extensibility surfaces.
//!
//! These tests deliberately verify the complete path from a configured model's
//! tool choice through the governed runtime. They are ignored by default
//! because they consume provider quota. Select any model declared in the same
//! ACL file with `A3S_TEST_MODEL=provider/model`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::hooks::{HookEvent, HookExecutor, HookResult};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::skills::{Skill, SkillKind, SkillRegistry};
use a3s_code_core::{
    dynamic_workflow_store_path, Agent, AgentEvent, AgentSession, CodeConfig, PlanningMode,
    RunStatus, SessionOptions, SystemPromptSlots,
};
use serde_json::{json, Value};

#[derive(Debug, Default)]
struct RewritingReadHook {
    events: std::sync::Mutex<Vec<HookEvent>>,
}

#[async_trait::async_trait]
impl HookExecutor for RewritingReadHook {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        self.events.lock().unwrap().push(event.clone());
        if matches!(event, HookEvent::PreToolUse(pre) if pre.tool == "read") {
            return HookResult::continue_with(json!({
                "updatedInput": { "file_path": "actual-evidence.txt" }
            }));
        }
        HookResult::Continue(None)
    }
}

const REAL_TIMEOUT: Duration = Duration::from_secs(300);
const CONFORMANCE_GUIDELINES: &str = "This is a deterministic integration test. Follow the numbered protocol exactly, use the named tools with their canonical schemas, inspect every result, do not replace a required tool call with prose, and stop after reporting the requested marker.";

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn real_agent() -> (Agent, String) {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let model = std::env::var("A3S_TEST_MODEL")
        .ok()
        .filter(|model| !model.trim().is_empty())
        .or_else(|| config.default_model.clone())
        .expect("real config must declare default_model");
    let (provider, model_id) = model
        .split_once('/')
        .expect("selected model must use provider/model syntax");
    assert!(
        config.llm_config(provider, model_id).is_some(),
        "selected model {model} is not declared in {}",
        path.display()
    );
    eprintln!("[extensibility-real] model={model}");
    (
        Agent::from_config(config)
            .await
            .expect("build real-provider agent"),
        model,
    )
}

fn governed_options(model: &str, session_id: &str, rules: &[&str]) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(rules);
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session_id)
        .with_model(model)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(8)
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0)
        .with_continuation(false)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(CONFORMANCE_GUIDELINES))
}

async fn run_and_events(
    session: &AgentSession,
    prompt: &str,
) -> (
    a3s_code_core::AgentResult,
    Vec<a3s_code_core::run::RunEventRecord>,
) {
    let result = tokio::time::timeout(REAL_TIMEOUT, session.send(prompt, None))
        .await
        .expect("real-provider scenario timed out")
        .expect("real-provider scenario failed");
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "scenario must create exactly one run");
    assert_eq!(runs[0].status, RunStatus::Completed);
    let events = session.run_events(&runs[0].id).await;
    (result, events)
}

fn successful_tool_end<'a>(
    events: &'a [a3s_code_core::run::RunEventRecord],
    expected_name: &str,
) -> (&'a Value, &'a Value) {
    events
        .iter()
        .find_map(|record| match &record.event {
            AgentEvent::ToolEnd {
                name,
                args: Some(args),
                exit_code: 0,
                metadata: Some(metadata),
                ..
            } if name == expected_name => Some((args, metadata)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {expected_name} ToolEnd: {events:?}"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_discovers_and_invokes_a_scoped_skill() {
    let workspace = tempfile::tempdir().expect("skill workspace");
    std::fs::write(
        workspace.path().join("skill-evidence.txt"),
        "SKILL_MARKER=SKILL_REAL_OK\n",
    )
    .expect("write skill evidence");

    let skills = Arc::new(SkillRegistry::new());
    skills.register_unchecked(Arc::new(Skill {
        name: "evidence-reader".into(),
        description: "Read the local conformance evidence marker".into(),
        allowed_tools: Some("read(*)".into()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Call read with file_path='skill-evidence.txt'. Return exactly the value after SKILL_MARKER=. Never invent the value or use another tool.".into(),
        tags: vec!["evidence".into(), "conformance".into()],
        version: Some("1.0.0".into()),
    }));

    let (agent, model) = real_agent().await;
    let options = governed_options(
        &model,
        "real-skill-extensibility",
        &["search_skills(*)", "Skill(*)", "read(*)"],
    )
    .with_skill_registry(skills);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create skill session");

    let prompt = r#"Run this exact skill protocol:
1. Call search_skills with query "local conformance evidence marker".
2. From its result, call Skill with skill_name "evidence-reader" and ask it to read and return the marker. Do not call read directly from the parent.
3. After the Skill result, return exactly the marker value and no other text."#;
    let (result, events) = run_and_events(&session, prompt).await;

    assert_eq!(result.text.trim(), "SKILL_REAL_OK");
    let _ = successful_tool_end(&events, "search_skills");
    let (skill_args, skill_metadata) = successful_tool_end(&events, "Skill");
    assert_eq!(skill_args["skill_name"], "evidence-reader");
    assert_eq!(skill_metadata["skill_name"], "evidence-reader");
    assert!(
        !events.iter().any(|record| matches!(
            &record.event,
            AgentEvent::ToolEnd { name, .. } if name == "read"
        )),
        "the parent bypassed the Skill boundary"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_uses_ptc_program_for_bounded_parallel_tool_calls() {
    let workspace = tempfile::tempdir().expect("PTC workspace");
    std::fs::write(workspace.path().join("left.txt"), "PTC_LEFT\n").unwrap();
    std::fs::write(workspace.path().join("right.txt"), "PTC_RIGHT\n").unwrap();

    let (agent, model) = real_agent().await;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(governed_options(
                &model,
                "real-ptc-extensibility",
                &["program(*)", "read(*)"],
            )),
        )
        .await
        .expect("create PTC session");

    let prompt = r#"Use the program tool exactly once to compute a result from files you have not read yet.
The program arguments must use type="script", allowed_tools=["read"], inputs={"left":"left.txt","right":"right.txt"}, and an inline JavaScript async function run(ctx, inputs).
Inside the function, call ctx.readFile for both input paths concurrently with Promise.all, trim both strings, and return {"marker":"<left>|<right>"}.
Do not call read outside program. After observing the program result, return exactly its marker and no other text."#;
    let (result, events) = run_and_events(&session, prompt).await;

    assert_eq!(result.text.trim(), "PTC_LEFT|PTC_RIGHT");
    let (args, metadata) = successful_tool_end(&events, "program");
    assert_eq!(args["type"], "script");
    assert_eq!(args["allowed_tools"], json!(["read"]));
    assert_eq!(metadata["program"]["runtime"], "embedded-quickjs");
    assert_eq!(metadata["script_result"]["marker"], "PTC_LEFT|PTC_RIGHT");
    let calls = metadata["program"]["tool_calls"]
        .as_array()
        .expect("program tool-call evidence");
    assert_eq!(calls.len(), 2, "PTC must perform exactly two nested reads");
    assert!(calls.iter().all(|call| call["tool_name"] == "read"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_drives_replayable_dynamic_workflow() {
    let workspace = tempfile::tempdir().expect("dynamic workflow workspace");
    std::fs::write(
        workspace.path().join("workflow-evidence.txt"),
        "DYNAMIC_WORKFLOW_OK\n",
    )
    .unwrap();

    let (agent, model) = real_agent().await;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(governed_options(
                &model,
                "real-dynamic-workflow-extensibility",
                &["dynamic_workflow(*)", "read(*)"],
            )),
        )
        .await
        .expect("create dynamic workflow session");
    session
        .register_dynamic_workflow_runtime()
        .expect("register dynamic workflow runtime");

    let source = r#"async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const completed = inputs.step_outputs.read_evidence;
    if (completed) {
      return { type: "complete", output: { marker: completed.trim() } };
    }
    return {
      type: "schedule_step",
      step_id: "read_evidence",
      step_name: "read_evidence",
      input: { path: inputs.input.path },
      retry: { max_attempts: 1, delay_ms: 0 }
    };
  }
  if (inputs.kind === "step" && inputs.step_name === "read_evidence") {
    return await ctx.readFile(inputs.input.path);
  }
  return { type: "fail", error: "unexpected invocation" };
}"#;
    let prompt = format!(
        "Run the dynamic_workflow tool exactly once. Pass run_id=\"real-dynamic-workflow\", input={{\"path\":\"workflow-evidence.txt\"}}, allowed_tools=[\"read\"], and copy the following source exactly as the source argument:\n\n```javascript\n{source}\n```\n\nAfter the workflow completes, return exactly the marker from its output and no other text."
    );
    let (result, events) = run_and_events(&session, &prompt).await;

    assert_eq!(result.text.trim(), "DYNAMIC_WORKFLOW_OK");
    let (args, metadata) = successful_tool_end(&events, "dynamic_workflow");
    assert_eq!(args["run_id"], "real-dynamic-workflow");
    assert_eq!(metadata["dynamic_workflow"]["status"], "Completed");
    assert_eq!(
        metadata["dynamic_workflow"]["snapshot"]["steps"]["read_evidence"]["status"],
        "completed"
    );
    let log_path =
        dynamic_workflow_store_path(workspace.path()).join("real-dynamic-workflow.jsonl");
    assert!(
        log_path.is_file(),
        "dynamic workflow journal was not persisted"
    );

    // Re-submit the exact model-generated request through the public direct
    // tool facade. The same run id/source/input must replay to the same result
    // rather than duplicating the workflow's step side effect.
    let replay = session
        .tool("dynamic_workflow", args.clone())
        .await
        .expect("replay dynamic workflow");
    assert_eq!(replay.exit_code, 0, "{}", replay.output);
    assert!(replay.output.contains("DYNAMIC_WORKFLOW_OK"));
    assert_eq!(
        replay.metadata.as_ref().expect("replay metadata")["dynamic_workflow"]["status"],
        "Completed"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_tool_call_is_rewritten_and_observed_by_hooks() {
    let workspace = tempfile::tempdir().expect("hook workspace");
    std::fs::write(workspace.path().join("decoy.txt"), "WRONG_MARKER\n").unwrap();
    std::fs::write(
        workspace.path().join("actual-evidence.txt"),
        "HOOK_REAL_OK\n",
    )
    .unwrap();
    let hook = Arc::new(RewritingReadHook::default());
    let (agent, model) = real_agent().await;
    let options = governed_options(&model, "real-hook-extensibility", &["read(*)"])
        .with_hook_executor(hook.clone());
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create hook session");

    let (result, events) = run_and_events(
        &session,
        "Call read exactly once with file_path='decoy.txt'. Return exactly the file content without line numbers or other text.",
    )
    .await;

    assert_eq!(result.text.trim(), "HOOK_REAL_OK");
    let read_start = events
        .iter()
        .find_map(|record| match &record.event {
            AgentEvent::ToolExecutionStart { name, args, .. } if name == "read" => Some(args),
            _ => None,
        })
        .expect("rewritten read execution");
    assert_eq!(read_start["file_path"], "actual-evidence.txt");
    let hook_events = hook.events.lock().unwrap();
    assert!(hook_events.iter().any(|event| matches!(
        event,
        HookEvent::PreToolUse(pre)
            if pre.tool == "read" && pre.args["file_path"] == "decoy.txt"
    )));
    assert!(hook_events.iter().any(|event| matches!(
        event,
        HookEvent::PostToolUse(post)
            if post.tool == "read"
                && post.result.success
                && post.result.output.contains("HOOK_REAL_OK")
    )));
}
