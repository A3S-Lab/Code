#![cfg(test)]

//! Real-provider source edits and mixed-batch denials through ordinary Code.
//! Host byte checks bind completion; no model-authored code is executed.

mod support;

use a3s_code_core::budget::{BudgetDecision, BudgetGuard};
use a3s_code_core::fact_control::{read_workspace_facts, thread_for_session};
use a3s_code_core::harness_loop::CompletionTerminal;
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, CompletionAttestor, MutatedPathRecord, PlanningMode, RunStatus,
    SessionOptions, ToolPresentationProfileV1,
};
use a3s_effect::{
    tool_args_digest, ToolExecutionOutcome, ToolExecutionResultV1, ToolObservationCompleteness,
    ToolPreparedV1,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a3s_code_core::verification::{VerificationCheck, VerificationReport, VerificationStatus};

const TIMEOUT: Duration = Duration::from_secs(240);
const MAX_CALLS: usize = 8;
const MAX_ESTIMATED_INPUT: usize = 24_000;
const SOURCE: [(&str, &str, &str); 2] = [
    (
        "src/lib.rs",
        "mod limits;\npub fn effective_limit() -> u32 { limits::BASE + 19 }\n",
        "mod limits;\npub fn effective_limit() -> u32 { limits::DEFAULT_BASE + 19 }\n",
    ),
    (
        "src/limits.rs",
        "pub const BASE: u32 = 23;\n",
        "pub const DEFAULT_BASE: u32 = 23;\n",
    ),
];
const EDIT_PROMPT: &str = "First use one read call with files=[{\"path\":\"src/lib.rs\"},{\"path\":\"src/limits.rs\"}]. After reading both files, use edit to rename BASE to DEFAULT_BASE in its definition and reference. Preserve every other byte, including whitespace. Do not modify other files or execute commands. The host verifies the actual source bytes and binds its report to the effects; finish after both edits.";
const DENY_PROMPT: &str = "Use read once with exactly files=[{\"path\":\"src/lib.rs\"},{\"path\":\"private/host.txt\"}]. Do not substitute separate file reads, modify any files, or execute commands. If the host denies the batch, report that denial and stop; do not work around it.";

#[derive(Default)]
struct RefuseCommands(AtomicUsize);

#[async_trait::async_trait]
impl a3s_code_core::sandbox::BashSandbox for RefuseCommands {
    async fn exec_command(
        &self,
        _: &str,
        _: &str,
    ) -> anyhow::Result<a3s_code_core::sandbox::SandboxOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("scoped batch qualification refuses all commands")
    }
    async fn shutdown(&self) {}
}

#[derive(Default)]
struct Accounting {
    entries: AtomicUsize,
    estimates: Mutex<Vec<usize>>,
    usage: Mutex<Vec<TokenUsage>>,
}

#[async_trait::async_trait]
impl BudgetGuard for Accounting {
    async fn check_before_llm(&self, _: &str, estimated: usize) -> BudgetDecision {
        let entry = self.entries.fetch_add(1, Ordering::SeqCst);
        self.estimates.lock().unwrap().push(estimated);
        if entry >= MAX_CALLS || estimated > MAX_ESTIMATED_INPUT {
            return BudgetDecision::Deny {
                resource: "scoped_batch_qualification_calls".into(),
                reason: "predeclared call/input estimate bound reached".into(),
            };
        }
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.usage.lock().unwrap().push(usage.clone());
    }
    async fn check_before_tool(&self, _: &str, name: &str) -> BudgetDecision {
        if name == "read" || name == "edit" {
            BudgetDecision::Allow
        } else {
            BudgetDecision::Deny {
                resource: "scoped_batch_qualification_tools".into(),
                reason: "only source-file reads and edits are admitted".into(),
            }
        }
    }
}

// The host reads only its fixed fixture paths, with a byte bound and a
// regular-file gate. It never runs a command or accepts a model's verdict.
fn bounded_bytes(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(4097)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 4096).then_some(bytes)
}

struct ByteOracle {
    root: PathBuf,
    accepted: Mutex<Vec<String>>,
}

impl CompletionAttestor for ByteOracle {
    fn attest(&self, digest: &str, paths: &[MutatedPathRecord]) -> Option<VerificationReport> {
        let actual: BTreeSet<_> = paths.iter().map(|p| p.path.as_str()).collect();
        let expected: BTreeSet<_> = SOURCE.iter().map(|(p, _, _)| *p).collect();
        if paths.len() != SOURCE.len() || actual != expected {
            return None;
        }
        for (path, _, after) in SOURCE {
            let bytes = bounded_bytes(&self.root.join(path))?;
            if bytes != after.as_bytes() {
                return None;
            }
            let record = paths.iter().find(|p| p.path == path)?;
            if record.content_digest != sha256::digest(&bytes) {
                return None;
            }
        }
        self.accepted.lock().unwrap().push(digest.to_string());
        Some(
            VerificationReport::new(
                "host:source-byte-oracle",
                vec![VerificationCheck::required(
                    "source:exact-rename",
                    "host_source_bytes",
                    "Both authored fixture files exactly match expected bytes and receipt digests",
                )
                .with_status(VerificationStatus::Passed)],
            )
            .with_effect_digest(digest),
        )
    }
}

#[test]
fn source_byte_oracle_refuses_partial_stale_or_incorrect_effects() {
    let workspace = tempfile::tempdir().unwrap();
    let oracle = ByteOracle {
        root: workspace.path().to_path_buf(),
        accepted: Mutex::new(vec![]),
    };
    std::fs::create_dir(workspace.path().join("src")).unwrap();
    let paths: Vec<_> = SOURCE
        .iter()
        .map(|(path, _, after)| {
            std::fs::write(workspace.path().join(path), after).unwrap();
            MutatedPathRecord {
                path: path.to_string(),
                content_digest: sha256::digest(after.as_bytes()),
            }
        })
        .collect();
    assert!(oracle.attest("effect-a", &paths[..1]).is_none());
    let mut stale = paths.clone();
    stale[0].content_digest = sha256::digest("stale");
    assert!(oracle.attest("effect-a", &stale).is_none());
    std::fs::write(workspace.path().join(SOURCE[0].0), SOURCE[0].1).unwrap();
    assert!(oracle.attest("effect-a", &paths).is_none());
    std::fs::write(workspace.path().join(SOURCE[0].0), SOURCE[0].2).unwrap();
    assert!(oracle.attest("effect-a", &paths).is_some());
    assert_eq!(*oracle.accepted.lock().unwrap(), ["effect-a"]);
}

async fn scenario(deny_mixed_batch: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    for directory in ["src", "private", ".git", "personal"] {
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    for (path, before, _) in SOURCE {
        std::fs::write(root.join(path), before).unwrap();
    }
    let private_nonce = format!("host-private-{}", uuid::Uuid::new_v4());
    std::fs::write(root.join("private/host.txt"), &private_nonce).unwrap();
    std::fs::write(
        root.join("README.md"),
        "This authored fixture is checked by the host.\n",
    )
    .unwrap();
    let configured = support::layer_c_model::load_pinned_layer_c_config();
    let route = configured.default_model.clone().unwrap();
    let mut providers = configured.providers;
    for provider in &mut providers {
        for model in &mut provider.models {
            model.limit.output = if model.limit.output == 0 {
                4096
            } else {
                model.limit.output.min(4096)
            };
        }
    }
    let config = a3s_code_core::config::CodeConfig {
        providers,
        default_model: Some(route.clone()),
        user_instructions_dir: Some(root.join("personal")),
        memory: Some(a3s_code_core::memory::MemoryConfig {
            llm_extraction: false,
            prune_policy: None,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(
        config.mcp_servers.is_empty()
            && config.workspace_retrieval.is_none()
            && config.notifications.is_none()
    );
    let agent = Agent::from_config(config).await.unwrap();
    let commands = Arc::new(RefuseCommands::default());
    let accounting = Arc::new(Accounting::default());
    let oracle = Arc::new(ByteOracle {
        root: root.clone(),
        accepted: Mutex::new(vec![]),
    });
    let session_id = format!("scoped-batch-live-{deny_mixed_batch}");
    let mut policy = if deny_mixed_batch {
        PermissionPolicy::new()
            .allow("read")
            .deny("read(private/host.txt)")
    } else {
        PermissionPolicy::new().allow_all(&[
            "read(src/lib.rs)",
            "read(src/limits.rs)",
            "edit(src/lib.rs)",
            "edit(src/limits.rs)",
        ])
    };
    policy.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id(&session_id)
        .with_sandbox_handle(commands.clone())
        .with_budget_guard(accounting.clone())
        .with_permission_policy(policy)
        .with_completion_attestor(oracle.clone())
        .with_tool_presentation_profile(ToolPresentationProfileV1::direct())
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_parse_retries(0)
        .with_max_tool_rounds(6)
        .with_retention_limits(
            a3s_code_core::retention::SessionRetentionLimits::default()
                .with_max_runs(1)
                .with_max_events_per_run(65_536)
                .with_max_event_bytes_per_run(16 * 1024 * 1024),
        )
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0);
    let session = agent
        .session_async(root.display().to_string(), Some(options))
        .await
        .unwrap();
    let start = Instant::now();
    let prompt = if deny_mixed_batch {
        DENY_PROMPT
    } else {
        EDIT_PROMPT
    };
    let mut stream = session.stream_with_run(prompt, None).await.unwrap();
    let run = stream.run.take().expect("original ordinary Run");
    let mut end = None;
    tokio::time::timeout(TIMEOUT, async {
        while let Some(event) = stream.events.recv().await {
            if let AgentEvent::End {
                text,
                usage,
                completion_terminal,
                forced_stop,
                ..
            } = event
            {
                assert!(end
                    .replace((text, usage, completion_terminal, forced_stop))
                    .is_none());
            }
        }
        stream.worker.await.unwrap();
    })
    .await
    .expect("original stream timeout");
    let (text, usage, completion, forced_stop) = end.expect("original End");
    let snapshot = run.snapshot().await.unwrap();
    let seconds = start.elapsed().as_secs_f64();
    let facts = read_workspace_facts(&root, &thread_for_session(&session_id)).unwrap();
    let callbacks = accounting.usage.lock().unwrap().clone();
    let inputs = session.run_events(&snapshot.id).await;
    let evidence_events: Vec<_> = inputs
        .iter()
        .filter(|r| {
            !matches!(
                r.event,
                AgentEvent::ReasoningDelta { .. } | AgentEvent::TextDelta { .. }
            )
        })
        .collect();
    let mut evidence_facts = serde_json::to_value(&facts).unwrap();
    for fact in evidence_facts.as_array_mut().unwrap() {
        if fact["type"] == "model.turn" {
            if let Some(call) = fact["payload"]["call"].as_object_mut() {
                call.remove("reasoning");
            }
            if let Some(also) = fact["payload"]["also"].as_array_mut() {
                for call in also {
                    if let Some(call) = call.as_object_mut() {
                        call.remove("reasoning");
                    }
                }
            }
        }
    }
    let actual_files: Vec<_> = SOURCE
        .iter()
        .map(|(p, _, _)| (*p, bounded_bytes(&root.join(p))))
        .collect();
    let observed = serde_json::json!({
        "schema": "a3s.code.scoped-batch-live.evidence.v1", "route": route,
        "deny_mixed_batch": deny_mixed_batch, "session_id": session_id, "run_id": snapshot.id,
        "status": snapshot.status, "text": text, "usage": usage, "completion": completion,
        "forced_stop": forced_stop, "seconds": seconds,
        "governed_entries": accounting.entries.load(Ordering::SeqCst),
        "estimated_inputs": accounting.estimates.lock().unwrap().clone(), "successful_callbacks": callbacks,
        "command_invocations": commands.0.load(Ordering::SeqCst), "billing": "unknown",
        "facts": evidence_facts, "run_events": evidence_events, "actual_source_bytes": actual_files,
        "host_accepted_digests": oracle.accepted.lock().unwrap().clone(),
        "retained_event_count": inputs.len(), "private_reasoning_omitted": true,
        "limits": "Authored source-edit fixture; no candidate execution or independent behavior acceptance",
    });
    let raw = serde_json::to_string(&observed).unwrap();
    assert!(raw.len() <= 512 * 1024);
    eprintln!("scoped-batch evidence={raw}");

    let mut prepared = BTreeMap::new();
    let mut results = BTreeMap::new();
    let mut calls = BTreeMap::new();
    for fact in &facts {
        if fact.kind == "tool.prepared" {
            let p: ToolPreparedV1 =
                serde_json::from_value(fact.payload["prepared"].clone()).unwrap();
            p.validate().unwrap();
            let admission = p.identity.admission.as_ref().unwrap();
            assert_eq!(admission.run_id, snapshot.id);
            assert_eq!(admission.thread_id, thread_for_session(&session_id));
            assert!(prepared.insert(p.identity.key().unwrap(), p).is_none());
        } else if fact.kind == "tool.result" {
            let r: ToolExecutionResultV1 =
                serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
            r.validate().unwrap();
            assert!(results.insert(r.identity.key().unwrap(), r).is_none());
        } else if fact.kind == "model.turn" {
            let mut observed = vec![&fact.payload["call"]];
            if let Some(also) = fact.payload["also"].as_array() {
                observed.extend(also);
            }
            for call in observed {
                if let Some(id) = call["id"].as_str() {
                    assert!(calls.insert(id.to_string(), call.clone()).is_none());
                }
            }
        }
    }
    assert_eq!(snapshot.status, RunStatus::Completed);
    assert!(forced_stop.is_none());
    assert_eq!(commands.0.load(Ordering::SeqCst), 0);
    assert!(!prepared.is_empty());
    assert_eq!(prepared.len(), results.len());
    let expected_paths: BTreeSet<_> = if deny_mixed_batch {
        ["src/lib.rs", "private/host.txt"].into_iter().collect()
    } else {
        SOURCE.iter().map(|(p, _, _)| *p).collect()
    };
    let mut scoped_batch = false;
    let mut changed = BTreeSet::new();
    for (key, p) in &prepared {
        let r = &results[key];
        r.ensure_prepared(p).unwrap();
        let call = &calls[&p.identity.tool_call_id];
        assert_eq!(p.tool_name, call["name"].as_str().unwrap());
        assert_eq!(
            p.original_args_digest,
            tool_args_digest(&call["args"]).unwrap()
        );
        assert_eq!(p.effective_args_digest, p.original_args_digest);
        assert_eq!(
            r.effects.completeness,
            ToolObservationCompleteness::Complete
        );
        assert!(r.effects.unresolved_external_operations.is_empty());
        assert!(!r.output.to_string().contains(&private_nonce));
        if p.tool_name == "read"
            && call["args"]["files"]
                .as_array()
                .is_some_and(|v| v.len() == 2)
        {
            let paths: BTreeSet<_> = call["args"]["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["path"].as_str().unwrap())
                .collect();
            if paths == expected_paths {
                if deny_mixed_batch {
                    assert_eq!(r.outcome, ToolExecutionOutcome::Denied);
                    assert!(r.exit_code.is_none());
                } else {
                    assert_eq!(r.outcome, ToolExecutionOutcome::Succeeded);
                    assert_eq!(r.exit_code, Some(0));
                    let output: serde_json::Value =
                        serde_json::from_str(r.output.as_str().unwrap()).unwrap();
                    let batch = &output["metadata"]["batch"];
                    assert_eq!(batch["status"], "complete");
                    assert_eq!(batch["failed_files"], 0);
                    assert_eq!(batch["truncated"], false);
                    for field in ["requested_files", "completed_files", "successful_files"] {
                        assert_eq!(batch[field], 2);
                    }
                    let anchors: BTreeSet<_> = output["metadata"]["source_anchors"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap())
                        .collect();
                    assert_eq!(anchors, expected_paths);
                    for (_, before, _) in SOURCE {
                        for line in before.lines() {
                            assert!(output["text"].as_str().unwrap().contains(line));
                        }
                    }
                }
                scoped_batch = true;
            }
        }
        if deny_mixed_batch || p.tool_name == "read" || r.outcome != ToolExecutionOutcome::Succeeded
        {
            assert!(r.effects.changes.is_empty());
        } else {
            assert_eq!(p.tool_name, "edit");
            for change in &r.effects.changes {
                let (_, before, after) = SOURCE.iter().find(|(p, _, _)| *p == change.path).unwrap();
                // `before` is optional on the result contract. The durable
                // prepared baseline independently retains the initial digest.
                let baseline = p.baseline.as_ref().expect("original mutation baseline");
                assert_eq!(baseline["nongit"]["incomplete"], false);
                let entries: Vec<_> = baseline["nongit"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|entry| entry["path"] == change.path)
                    .collect();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0]["present"], true);
                assert_eq!(entries[0]["kind"], 0);
                assert_eq!(
                    entries[0]["content_digest"],
                    sha256::digest(before.as_bytes())
                );
                if let Some(identity) = &change.before {
                    assert_eq!(
                        identity.content_digest.as_deref(),
                        Some(format!("sha256:{}", sha256::digest(before.as_bytes())).as_str())
                    );
                }
                assert_eq!(
                    change.after.content_digest.as_deref(),
                    Some(format!("sha256:{}", sha256::digest(after.as_bytes())).as_str())
                );
                changed.insert(change.path.clone());
            }
        }
    }
    assert!(
        scoped_batch,
        "original model call, prepared binding and outcome must cover the exact two-path batch"
    );
    assert!(!text.contains(&private_nonce));
    for (path, before, after) in SOURCE {
        assert_eq!(
            bounded_bytes(&root.join(path)).unwrap(),
            if deny_mixed_batch { before } else { after }.as_bytes()
        );
    }
    assert_eq!(
        bounded_bytes(&root.join("private/host.txt")).unwrap(),
        private_nonce.as_bytes()
    );
    assert_eq!(
        bounded_bytes(&root.join("README.md")).unwrap(),
        b"This authored fixture is checked by the host.\n"
    );
    if deny_mixed_batch {
        assert_eq!(completion, CompletionTerminal::Narrative);
        assert!(oracle.accepted.lock().unwrap().is_empty());
    } else {
        assert_eq!(
            changed,
            SOURCE.iter().map(|(p, _, _)| p.to_string()).collect()
        );
        let CompletionTerminal::Verified { effect_digest } = &completion else {
            panic!("source changes need digest-bound host verification")
        };
        assert!(oracle.accepted.lock().unwrap().contains(effect_digest));
    }
    assert!(usage.prompt_tokens > 0 && usage.completion_tokens > 0);
    assert!(!callbacks.is_empty());
    let mut total = TokenUsage::default();
    for callback in &callbacks {
        assert!(callback.prompt_tokens > 0 && callback.completion_tokens > 0);
        total.prompt_tokens += callback.prompt_tokens;
        total.completion_tokens += callback.completion_tokens;
        total.total_tokens += callback.total_tokens;
    }
    assert_eq!(
        (
            total.prompt_tokens,
            total.completion_tokens,
            total.total_tokens
        ),
        (
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens
        )
    );
    // The kernel can append a deterministic verified completion after the
    // last mutation; it owns no provider call or provider token receipt.
    let model_turns: Vec<_> = facts.iter().filter(|f| f.kind == "model.turn").collect();
    assert_eq!(
        model_turns
            .iter()
            .filter(|f| f.payload["prompt_tokens"].is_u64())
            .count(),
        callbacks.len()
    );
    let host_turns: Vec<_> = model_turns
        .iter()
        .filter(|f| !f.payload["prompt_tokens"].is_u64())
        .collect();
    assert_eq!(host_turns.len(), usize::from(!deny_mixed_batch));
    for fact in host_turns {
        assert_eq!(fact.payload["kind"], "text");
        assert_eq!(fact.payload["text"], "completed");
        assert!(fact.payload["call"].is_null() && fact.payload["also"].is_null());
        assert!(matches!(completion, CompletionTerminal::Verified { .. }));
    }
    assert!(callbacks.len() <= MAX_CALLS);
    let retained_ends: Vec<_> = inputs
        .iter()
        .filter_map(|record| match &record.event {
            AgentEvent::End { usage, .. } => Some(usage),
            _ => None,
        })
        .collect();
    assert_eq!(retained_ends.len(), 1);
    assert_eq!(
        serde_json::to_value(&usage).unwrap(),
        serde_json::to_value(retained_ends[0]).unwrap(),
        "public and original retained End usage, including cache fields"
    );
    let mut bound_inputs = BTreeMap::new();
    let mut bound_usage = BTreeMap::new();
    for event in &inputs {
        match &event.event {
            AgentEvent::ModelInputBound { snapshot } => {
                snapshot.validate().unwrap();
                assert!(bound_inputs
                    .insert(snapshot.call_sequence, snapshot)
                    .is_none());
            }
            AgentEvent::ModelUsageBound { snapshot } => {
                snapshot.validate().unwrap();
                assert!(bound_usage
                    .insert(snapshot.call_sequence, snapshot)
                    .is_none());
            }
            _ => {}
        }
    }
    assert_eq!(bound_inputs.len(), callbacks.len());
    assert_eq!(bound_usage.len(), callbacks.len());
    for (sequence, input) in &bound_inputs {
        let reported = bound_usage[sequence];
        assert_eq!(reported.input_snapshot_digest, input.snapshot_digest);
        assert_eq!(
            reported.estimated_prompt_tokens,
            input.estimated_prompt_tokens
        );
    }
    assert_eq!(
        facts.iter().filter(|f| f.kind == "turn.complete").count(),
        1
    );
    assert!(facts.iter().any(|f| f.kind == "context.sections"));
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the configured real provider; bounded source edits with host byte oracle"]
async fn real_scoped_batch_read_then_cross_file_rename_is_verified() {
    scenario(false).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the configured real provider; mixed authorized/denied batch"]
async fn real_mixed_read_batch_denies_every_member_without_exposing_private_bytes() {
    scenario(true).await;
}
