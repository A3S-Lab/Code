//! Live Memory gates against `.a3s/config.acl`.
//!
//! - Durability: store → reopen → `AgentMemory.recall_similar` (always assert).
//! - Extract: real-model turn → LLM extraction → reopen → recall (hard fail;
//!   soft-skip would green Layer C without proving durable extract effect).
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_memory_store_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::memory::AgentMemory;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions, SystemPromptSlots};
use a3s_memory::{FileMemoryStore, MemoryItem, MemoryStore, MemoryType};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model};

const TURN_TIMEOUT: Duration = Duration::from_secs(180);

fn configured_model(config: &a3s_code_core::CodeConfig) -> String {
    pinned_layer_c_model(config)
}

async fn run_text_turn(
    session: &a3s_code_core::AgentSession,
    prompt: &str,
) -> Result<(String, usize), String> {
    let (mut events, worker) = session
        .stream(prompt, None)
        .await
        .map_err(|error| error.to_string())?;
    let outcome = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(AgentEvent::End { text, usage, .. }) => {
                    break Ok((text, usage.total_tokens));
                }
                Some(AgentEvent::Error { message }) => {
                    return Err(message);
                }
                Some(AgentEvent::ConfirmationRequired { tool_name, .. }) => {
                    return Err(format!("unexpected confirmation for {tool_name}"));
                }
                Some(_) => {}
                None => return Err("model event stream ended before completion".into()),
            }
        }
    })
    .await
    .map_err(|_| "model turn timed out".to_string())??;
    worker
        .await
        .map_err(|error| format!("model stream worker join failed: {error}"))?;
    Ok(outcome)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_config_memory_store_survives_reopen_and_agent_recall() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let selected_model = configured_model(&config);
    config.default_model = Some(selected_model.clone());
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let memory_root = tempfile::tempdir().expect("memory dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let store: Arc<dyn MemoryStore> = Arc::new(
        FileMemoryStore::new(memory_root.path())
            .await
            .expect("open file memory store"),
    );
    let agent_memory = Arc::new(AgentMemory::new(Arc::clone(&store)));
    let token = "LIVE-MEM-9941";
    let item = MemoryItem::new(format!(
        "The project live-memory verification codename is {token}."
    ))
    .with_type(MemoryType::Semantic)
    .with_importance(0.95)
    .with_tag("live-memory-gate");

    agent_memory
        .remember(item)
        .await
        .expect("AgentMemory.remember must persist through the shared store");

    drop(agent_memory);
    drop(store);

    let reopened: Arc<dyn MemoryStore> = Arc::new(
        FileMemoryStore::new(memory_root.path())
            .await
            .expect("reopen file memory store"),
    );
    let recalled = AgentMemory::new(Arc::clone(&reopened))
        .recall_similar(token, 5)
        .await
        .expect("recall after reopen");
    assert_eq!(recalled.len(), 1, "{recalled:?}");
    assert!(
        recalled[0].content.contains(token),
        "reopened store must return the durable item: {:?}",
        recalled[0].content
    );

    let options = SessionOptions::new()
        .with_session_id("memory-store-real-llm")
        .with_model(selected_model.clone())
        .with_memory(reopened)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session with reopened memory");
    let session_memory = session.memory().expect("session exposes memory");
    let via_session = session_memory
        .recall_similar(token, 5)
        .await
        .expect("session recall");
    assert_eq!(via_session.len(), 1);
    assert!(via_session[0].content.contains(token));
    session.close().await;

    println!(
        "memory-store-real-llm model={} token={} turn_ms={} result=pass",
        selected_model,
        token,
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_memory_extract_survives_reopen() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let selected_model = configured_model(&config);
    config.default_model = Some(selected_model.clone());
    // Layer C requires extract effect, not a soft narrative acknowledgement.
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: true,
        ..Default::default()
    });
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let memory_root = tempfile::tempdir().expect("memory dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let store: Arc<dyn MemoryStore> = Arc::new(
        FileMemoryStore::new(memory_root.path())
            .await
            .expect("open file memory store"),
    );

    let mut permissions = PermissionPolicy::new();
    permissions.default_decision = PermissionDecision::Deny;
    let token = "EXTRACT-LIVE-7733";
    let options = SessionOptions::new()
        .with_session_id("memory-extract-real-llm")
        .with_model(selected_model.clone())
        .with_memory(Arc::clone(&store))
        .with_permission_policy(permissions)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "Do not call tools. Acknowledge the durable workspace preference clearly \
             and treat the verification codename as durable memory for future sessions.",
        ));
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create extract session");

    let prompt = format!(
        "Please remember this durable workspace preference for future sessions: \
         always use the verification codename {token} when referring to the \
         memory-extract live gate. Confirm you understood the preference."
    );
    let (final_text, tokens) = run_text_turn(&session, &prompt)
        .await
        .unwrap_or_else(|message| panic!("live extract turn failed: {message}"));
    // Close drains pending LLM extraction before we reopen the file store.
    session.close().await;
    drop(store);

    let reopened = FileMemoryStore::new(memory_root.path())
        .await
        .expect("reopen after extract");
    let hits = MemoryStore::search(&reopened, token, 5)
        .await
        .expect("search after reopen");
    assert!(
        !hits.is_empty(),
        "Layer C extract must persist {token} (model={selected_model} tokens={tokens} \
         reply_chars={} turn_ms={}); empty store is a failed effect, not a soft-skip",
        final_text.chars().count(),
        started.elapsed().as_millis()
    );
    assert!(
        hits.iter().any(|item| item.content.contains(token)),
        "extracted durable memory must mention {token}: {hits:?}"
    );
    println!(
        "memory-extract-real-llm model={} token={} tokens={} turn_ms={} result=pass",
        selected_model,
        token,
        tokens,
        started.elapsed().as_millis()
    );
}
