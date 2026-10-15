//! Live durable a3s-vec index lifecycle against `.a3s/config.acl`.
//!
//! Two product-contract gates over the persistent workspace FTS projection:
//!
//! - **Incremental update**: one live session keeps serving `bm25` from the
//!   durable a3s-vec index after a mid-session source edit — manifest watcher
//!   → catalog reconciliation → new published generation, with the real model
//!   driving the search tool against the *new* content.
//! - **Cold-start reuse**: a restarted session object graph over the same
//!   workspace root reopens the published generation instead of rebuilding
//!   it (`CURRENT` and the generation directory set stay byte-identical), and
//!   the real model still retrieves through `persistent_a3s_vec_fts`.
//!
//! The restart boundary here is the object graph plus the on-disk index — the
//! generation directories and storage locks are the cross-process artifacts;
//! a Cargo test cannot cheaply fork the host process, and the design target
//! (`WorkspacePersistentIndex` reopen + `load_current`) is exactly that
//! boundary.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_workspace_index_cycle_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::workspace::WorkspaceServices;
use a3s_code_core::{
    Agent, AgentEvent, PlanningMode, SessionOptions, SystemPromptSlots,
    WorkspacePersistentIndexPhase, WorkspacePersistentIndexStatus,
};
use serde_json::Value;
use support::layer_c_model::load_pinned_layer_c_config;

const INDEX_READY_TIMEOUT: Duration = Duration::from_secs(120);
const CATALOG_UPDATE_TIMEOUT: Duration = Duration::from_secs(120);
const TURN_TIMEOUT: Duration = Duration::from_secs(420);
const STABLE_IDENTIFIER: &str = "admission_fence_probe";
const BEACON_IDENTIFIER: &str = "beacon_reload_after_edit";
const BEACON_MARKER: &str = "INCREMENTAL_BEACON_RELOAD";
const REOPEN_IDENTIFIER: &str = "reopen_persistence_probe";
const POST_RESTART_BEACON_IDENTIFIER: &str = "post_restart_edit_beacon";
const POST_RESTART_BEACON_MARKER: &str = "POST_RESTART_EDIT_BEACON";
const ONE_TOOL_PROTOCOL: &str = "Use exactly one call to the `search` tool and no other tool. \
     After the tool result, reply with only the Rust identifier the evidence supports \
     (no prose, no punctuation), or NOT_FOUND when no relevant identifier is present.";

fn write_source(root: &Path, path: &str, body: &str) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create fixture directory");
    }
    std::fs::write(full, body).expect("write fixture source");
}

fn write_cycle_fixture(root: &Path) {
    write_source(
        root,
        "src/admission_fence.rs",
        r#"//! Admission fencing for the durable workspace index cycle fixture.
pub fn admission_fence_probe(key: &str) -> bool {
    !key.is_empty()
}
"#,
    );
    write_source(
        root,
        "src/feature_area.rs",
        r#"//! Editable area refreshed mid-session by the incremental gate.
pub fn feature_area_baseline() -> u32 {
    7
}
"#,
    );
    for index in 0..8 {
        write_source(
            root,
            &format!("src/filler_{index:02}.rs"),
            &format!(
                "//! Filler module {index} for the index cycle fixture.\n\
                 pub fn filler_{index:02}_routine() -> u32 {{ {index} }}\n"
            ),
        );
    }
}

fn write_reopen_fixture(root: &Path) {
    write_source(
        root,
        "src/reopen_probe.rs",
        r#"//! Cold-start reuse probe for the durable workspace index.
pub fn reopen_persistence_probe() -> bool {
    true
}
"#,
    );
    for index in 0..11 {
        write_source(
            root,
            &format!("src/module_{index:02}.rs"),
            &format!(
                "//! Reopen fixture module {index}.\n\
                 pub fn module_{index:02}_helper() -> u32 {{ {index} }}\n"
            ),
        );
    }
}

async fn wait_for_native_index(
    root: &Path,
    services: &WorkspaceServices,
) -> WorkspacePersistentIndexStatus {
    let catalog = services
        .chunk_catalog()
        .expect("local retrieval sessions expose a chunk catalog");
    let index = services
        .persistent_index()
        .expect("a3s-vec-fts builds must expose a durable workspace index");
    let current = root.join(".a3s-code/index/CURRENT");
    tokio::time::timeout(INDEX_READY_TIMEOUT, async {
        loop {
            if let Ok(snapshot) = catalog.snapshot() {
                if snapshot.source_revision() > 0 {
                    let _ = index.sync_snapshot(&snapshot);
                }
            }
            if index.is_ready() && tokio::fs::try_exists(&current).await.unwrap_or(false) {
                return index.status();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "a3s-vec index did not become ready: status={:?} path={}",
            index.status(),
            current.display()
        )
    })
}

async fn wait_for_catalog_term(
    services: &WorkspaceServices,
    term: &str,
) -> a3s_code_core::workspace::ChunkCatalogSnapshot {
    let catalog = services
        .chunk_catalog()
        .expect("chunk catalog must stay attached");
    tokio::time::timeout(CATALOG_UPDATE_TIMEOUT, async {
        loop {
            let snapshot = catalog
                .snapshot()
                .expect("catalog snapshot must stay available");
            let found = snapshot
                .chunks()
                .iter()
                .any(|chunk| chunk.text.contains(term));
            if found && snapshot.source_revision() > 0 {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("catalog never admitted a chunk containing {term}"))
}

async fn wait_for_generation_change(
    services: &WorkspaceServices,
    previous: &WorkspacePersistentIndexStatus,
) -> WorkspacePersistentIndexStatus {
    let catalog = services
        .chunk_catalog()
        .expect("chunk catalog must stay attached");
    let index = services
        .persistent_index()
        .expect("durable index must stay attached");
    let mut last_sync_error = String::new();
    tokio::time::timeout(INDEX_READY_TIMEOUT, async {
        loop {
            if let Ok(snapshot) = catalog.snapshot() {
                match index.sync_snapshot(&snapshot) {
                    Ok(()) => last_sync_error.clear(),
                    Err(error) => last_sync_error = error.to_string(),
                }
            }
            let status = index.status();
            // Generation names are epoch-unique, so a changed name alone
            // proves a new publication — cross-epoch revision counters are
            // not comparable and must not gate this check.
            let published_newer =
                status.generation.is_some() && status.generation != previous.generation;
            if published_newer {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "durable index never published past {previous:?}: status={:?} last_sync_error={last_sync_error:?}",
            index.status()
        )
    })
}

fn generation_dirs(index_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(index_root)
        .expect("index root must be listable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("generation-"))
        })
        .collect();
    dirs.sort();
    dirs
}

#[derive(Debug)]
struct SearchTurn {
    final_text: String,
    mode: String,
    execution_mode: String,
    freshness: String,
    result_paths: Vec<String>,
    total_tokens: usize,
    elapsed_ms: u128,
}

async fn run_search_turn(session: &a3s_code_core::AgentSession, prompt: &str) -> SearchTurn {
    let started = Instant::now();
    let (mut events, worker) = session
        .stream(prompt, None)
        .await
        .expect("start model turn");
    let mut starts = HashMap::<String, (String, Value)>::new();
    let mut tool_names = Vec::new();
    let (search_metadata, final_text, usage) = tokio::time::timeout(TURN_TIMEOUT, async {
        let mut search_metadata = Value::Null;
        loop {
            match events
                .recv()
                .await
                .expect("model event stream ended before completion")
            {
                AgentEvent::ToolExecutionStart { id, name, args } => {
                    starts.insert(id, (name, args));
                }
                AgentEvent::ToolEnd {
                    id, name, metadata, ..
                } => {
                    assert!(
                        starts.remove(&id).is_some(),
                        "tool end without a recorded start: {name}"
                    );
                    tool_names.push(name.clone());
                    if name == "search" {
                        search_metadata = metadata.unwrap_or(Value::Null);
                    }
                }
                AgentEvent::End { text, usage, .. } => {
                    break (search_metadata, text, usage);
                }
                AgentEvent::Error { message } => panic!("model turn failed: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("model search turn timed out");
    worker.await.expect("model stream worker joins");
    assert!(starts.is_empty(), "tool starts without terminal events");
    assert_eq!(tool_names, ["search"], "exactly one search call expected");
    let mode = search_metadata["mode"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let execution_mode = search_metadata["execution_mode"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let freshness = search_metadata["freshness"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let result_paths = search_metadata["results"]
        .as_array()
        .map(|results| {
            results
                .iter()
                .filter_map(|result| result["path"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    SearchTurn {
        final_text,
        mode,
        execution_mode,
        freshness,
        result_paths,
        total_tokens: usage.total_tokens,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

fn search_session(
    model: &str,
    session_id: &str,
    services: Arc<WorkspaceServices>,
) -> SessionOptions {
    let mut permissions = PermissionPolicy::new().allow_all(&["search(*)"]);
    permissions.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session_id.to_owned())
        .with_model(model.to_owned())
        .with_permission_policy(permissions)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(2)
        .with_workspace_backend(services)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(ONE_TOOL_PROTOCOL))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_retrieves_incremental_updates_within_one_session() {
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned Layer C config must declare default_model");
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let workspace = tempfile::tempdir().expect("create fixture workspace");
    write_cycle_fixture(workspace.path());
    let services = WorkspaceServices::local_with_indexed_retrieval(workspace.path())
        .expect("configure durable a3s-vec workspace index");
    let initial = wait_for_native_index(workspace.path(), services.as_ref()).await;

    let options = search_session(
        &model,
        "workspace-index-cycle-incremental",
        Arc::clone(&services),
    );
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create incremental gate session");

    // Turn 1: the model retrieves content that has been indexed since the
    // start, proving the session is serving from the durable projection.
    let warmup = run_search_turn(
        &session,
        "Investigate this workspace. Use exactly one `search` tool call with mode 'bm25', \
         query 'admission_fence_probe', path '.', include '*.rs', limit 5, context 2. \
         Reply with only the identifier the evidence supports.",
    )
    .await;
    assert_eq!(warmup.mode, "bm25");
    assert_eq!(warmup.execution_mode, "persistent_a3s_vec_fts");
    assert!(
        warmup.final_text.contains(STABLE_IDENTIFIER),
        "warmup answer must be grounded in durable-index evidence: {:?}",
        warmup.final_text
    );

    // Mid-session source edit: append the beacon to an already-indexed file.
    let edited_path = workspace.path().join("src/feature_area.rs");
    let previous_body = std::fs::read_to_string(&edited_path).expect("read editable fixture");
    let edited_body = format!(
        "{previous_body}\n/// {BEACON_MARKER}: appended after the session started indexing.\npub fn {BEACON_IDENTIFIER}() -> bool {{ true }}\n"
    );
    std::fs::write(&edited_path, edited_body).expect("apply mid-session edit");

    // The manifest watcher → catalog reconciliation must admit the new chunk,
    // and the durable projection must publish a new generation.
    wait_for_catalog_term(services.as_ref(), BEACON_MARKER).await;
    let updated = wait_for_generation_change(services.as_ref(), &initial).await;
    assert_ne!(
        updated.generation, initial.generation,
        "a content change must publish a new durable generation"
    );

    // Turn 2 on the SAME session: the model must retrieve the new content
    // through the durable projection, not a stale pre-edit generation.
    let incremental = run_search_turn(
        &session,
        "The workspace just changed. Use exactly one `search` tool call with mode 'bm25', \
         query 'INCREMENTAL_BEACON_RELOAD beacon', path '.', include '*.rs', limit 5, \
         context 2. Reply with only the identifier the evidence supports.",
    )
    .await;
    assert_eq!(incremental.mode, "bm25");
    assert_eq!(incremental.execution_mode, "persistent_a3s_vec_fts");
    assert_eq!(incremental.freshness, "ready");
    assert!(
        incremental
            .result_paths
            .iter()
            .any(|path| path.ends_with("src/feature_area.rs")),
        "incremental results must include the edited file: {:?}",
        incremental.result_paths
    );
    assert!(
        incremental.final_text.contains(BEACON_IDENTIFIER),
        "model answer must reflect the mid-session edit: {:?}",
        incremental.final_text
    );
    session.close().await;

    println!(
        "workspace-index-cycle-incremental model={} warmup_ms={} incremental_ms={} \
         generation={:?}-> {:?} tokens={}+{} result=pass",
        model,
        warmup.elapsed_ms,
        incremental.elapsed_ms,
        initial.generation,
        updated.generation,
        warmup.total_tokens,
        incremental.total_tokens
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_reuses_persistent_generation_across_restart() {
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned Layer C config must declare default_model");
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let workspace = tempfile::tempdir().expect("create fixture workspace");
    write_reopen_fixture(workspace.path());

    // Phase 1: a fresh workspace builds and publishes its first generation.
    let first_build_started = Instant::now();
    let services = WorkspaceServices::local_with_indexed_retrieval(workspace.path())
        .expect("configure durable a3s-vec workspace index (first pass)");
    let first = wait_for_native_index(workspace.path(), services.as_ref()).await;
    let first_build_ms = first_build_started.elapsed().as_millis();
    let index_root = workspace.path().join(".a3s-code/index");
    let first_current = std::fs::read_to_string(index_root.join("CURRENT"))
        .expect("first pass must publish CURRENT");
    let first_generations = generation_dirs(&index_root);
    let first_generation = first
        .generation
        .clone()
        .expect("first pass must name its generation");
    assert_eq!(
        first_generations.len(),
        1,
        "one generation expected: {first_generations:?}"
    );
    // The session-level services must be released before the restart phase.
    drop(services);
    drop(agent);
    let config = load_pinned_layer_c_config();
    let agent = Agent::from_config(config)
        .await
        .expect("recreate agent for the restart phase");

    // Phase 2: a restarted object graph over the same root must reopen the
    // published generation rather than rebuild the postings from scratch.
    let reopen_started = Instant::now();
    let services = WorkspaceServices::local_with_indexed_retrieval(workspace.path())
        .expect("configure durable a3s-vec workspace index (restart)");
    let index = services
        .persistent_index()
        .expect("restart must expose the durable workspace index");
    let reopened = index.status();
    let reopen_ms = reopen_started.elapsed().as_millis();
    assert_eq!(
        reopened.phase,
        WorkspacePersistentIndexPhase::Ready,
        "reopened index must be ready immediately: {reopened:?}"
    );
    assert_eq!(
        reopened.generation.as_deref(),
        Some(first_generation.as_str()),
        "restart must reuse the published generation"
    );
    assert_eq!(
        reopened.indexed_chunks, first.indexed_chunks,
        "reopened generation must serve the same chunk count"
    );
    let second_current = std::fs::read_to_string(index_root.join("CURRENT"))
        .expect("restart must keep CURRENT readable");
    assert_eq!(
        first_current, second_current,
        "CURRENT must stay byte-identical across restart"
    );
    assert_eq!(
        generation_dirs(&index_root),
        first_generations,
        "no new generation directories may appear on restart"
    );

    // The real model must still retrieve through the reopened generation.
    let options = search_session(
        &model,
        "workspace-index-cycle-reopen",
        Arc::clone(&services),
    );
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create restart-phase session");
    let turn = run_search_turn(
        &session,
        "Investigate this workspace. Use exactly one `search` tool call with mode 'bm25', \
         query 'reopen_persistence_probe', path '.', include '*.rs', limit 5, context 2. \
         Reply with only the identifier the evidence supports.",
    )
    .await;
    assert_eq!(turn.mode, "bm25");
    assert_eq!(turn.execution_mode, "persistent_a3s_vec_fts");
    assert!(
        turn.final_text.contains(REOPEN_IDENTIFIER),
        "model answer must be grounded in the reopened generation: {:?}",
        turn.final_text
    );
    session.close().await;

    println!(
        "workspace-index-cycle-reopen model={} first_build_ms={first_build_ms} \
         reopen_ms={reopen_ms} generation={first_generation} chunks={} tokens={} \
         result=pass",
        model, reopened.indexed_chunks, turn.total_tokens
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_retrieves_post_restart_edits_through_the_durable_index() {
    // Regression shape: a restarted object graph reopens the published
    // generation with the dead epoch's revision counters. The first edit in
    // the new epoch arrives with smaller counters; the durable projection
    // must publish it anyway (content decides across epochs) and the real
    // model must retrieve the edited content through the persistent path.
    let config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned Layer C config must declare default_model");
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let workspace = tempfile::tempdir().expect("create fixture workspace");
    write_cycle_fixture(workspace.path());
    let services = WorkspaceServices::local_with_indexed_retrieval(workspace.path())
        .expect("configure durable a3s-vec workspace index (epoch one)");
    let initial = wait_for_native_index(workspace.path(), services.as_ref()).await;
    drop(services);
    drop(agent);
    let config = load_pinned_layer_c_config();
    let agent = Agent::from_config(config)
        .await
        .expect("recreate agent for the restart epoch");

    // Restart epoch: fresh counters, loaded generation.
    let services = WorkspaceServices::local_with_indexed_retrieval(workspace.path())
        .expect("configure durable a3s-vec workspace index (restart epoch)");
    let index = services
        .persistent_index()
        .expect("restart epoch exposes the durable workspace index");
    assert_eq!(
        index.status().generation.as_deref(),
        initial.generation.as_deref(),
        "the restart epoch must reopen the published generation"
    );

    // Edit after restart: the new epoch's catalog counters restart small,
    // so this snapshot is numerically "older" than the loaded generation.
    let edited_path = workspace.path().join("src/admission_fence.rs");
    let previous_body = std::fs::read_to_string(&edited_path).expect("read fixture source");
    std::fs::write(
        &edited_path,
        format!(
            "{previous_body}/// {POST_RESTART_BEACON_MARKER}: edited after the restart epoch began.\npub fn {POST_RESTART_BEACON_IDENTIFIER}() -> bool {{ true }}\n"
        ),
    )
    .expect("apply post-restart edit");
    wait_for_catalog_term(services.as_ref(), POST_RESTART_BEACON_MARKER).await;
    let updated = wait_for_generation_change(services.as_ref(), &initial).await;
    assert_ne!(
        updated.generation, initial.generation,
        "a post-restart edit must publish a new generation despite smaller counters"
    );

    let options = search_session(
        &model,
        "workspace-index-cycle-post-restart-edit",
        Arc::clone(&services),
    );
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create post-restart-edit session");
    let turn = run_search_turn(
        &session,
        "Investigate this workspace. Use exactly one `search` tool call with mode 'bm25',          query 'POST_RESTART_EDIT_BEACON', path '.', include '*.rs', limit 5, context 2.          Reply with only the identifier the evidence supports.",
    )
    .await;
    assert_eq!(turn.mode, "bm25");
    assert_eq!(turn.execution_mode, "persistent_a3s_vec_fts");
    assert!(
        turn.final_text.contains(POST_RESTART_BEACON_IDENTIFIER),
        "model answer must reflect the post-restart edit: {:?}",
        turn.final_text
    );
    session.close().await;

    println!(
        "workspace-index-cycle-post-restart-edit model={} generation={:?}-> {:?} turn_ms={} result=pass",
        model,
        initial.generation,
        updated.generation,
        turn.elapsed_ms
    );
}
