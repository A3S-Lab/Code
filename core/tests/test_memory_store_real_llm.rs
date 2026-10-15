#![cfg(test)]

//! Live Memory gates against `.a3s/config.acl`.
//!
//! - Durability: store → reopen → `AgentMemory.recall_similar` (always assert).
//! - Extract: real-model stream → observed store acknowledgement → close →
//!   reopen → recall. End and the bounded close drain are not store receipts.
//!   The original immediate-close failures remain retained evidence.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_memory_store_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::memory::{AgentMemory, MemoryObservation, MemoryObserver};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions, SystemPromptSlots};
use a3s_memory::{FileMemoryStore, MemoryItem, MemoryStore, MemoryType};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model};

const TURN_TIMEOUT: Duration = Duration::from_secs(420);
const MEMORY_ACK_TIMEOUT: Duration = Duration::from_secs(60);

struct StoredMemoryObserver {
    sender: tokio::sync::mpsc::Sender<MemoryObservation>,
}

#[async_trait::async_trait]
impl MemoryObserver for StoredMemoryObserver {
    async fn on_memory_stored(&self, observation: MemoryObservation) -> anyhow::Result<()> {
        self.sender.try_send(observation)?;
        Ok(())
    }
}

fn memory_write_receipts() -> (
    Arc<dyn MemoryObserver>,
    tokio::sync::mpsc::Receiver<MemoryObservation>,
) {
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    (Arc::new(StoredMemoryObserver { sender }), receiver)
}

async fn wait_for_stored_memory(
    receipts: &mut tokio::sync::mpsc::Receiver<MemoryObservation>,
    token: &str,
    session_id: &str,
) -> Result<MemoryItem, String> {
    let started = Instant::now();
    tokio::time::timeout(MEMORY_ACK_TIMEOUT, async {
        while let Some(observation) = receipts.recv().await {
            // This is the test's exact stored identifier, not an intent matcher.
            if observation.stored.content.contains(token)
                && observation
                    .incoming
                    .metadata
                    .get("session_id")
                    .is_some_and(|observed| observed == session_id)
            {
                println!(
                    "MEM-ACK canonical_id={} wait_ms={}",
                    observation.stored.id,
                    started.elapsed().as_millis()
                );
                return Ok(observation.stored);
            }
        }
        Err("memory observer closed before the expected store acknowledgement".into())
    })
    .await
    .map_err(|_| "no expected memory store acknowledgement within 60s".to_string())?
}

fn configured_model(config: &a3s_code_core::CodeConfig) -> String {
    pinned_layer_c_model(config)
}

fn install_memory_retention_diagnostics() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                "memory_retention=info,a3s_code_core::agent_api::session_close=warn,\
                 a3s_code_core::agent::memory_extraction_runtime=warn",
            )
            .with_ansi(false)
            .with_test_writer()
            .try_init()
            .expect("install live memory retention diagnostics");
    });
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
async fn real_model_recalls_memory_in_a_new_session_from_a_paraphrased_question() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let selected_model = configured_model(&config);
    config.default_model = Some(selected_model.clone());
    // The model-driven memory extraction runtime is only armed when
    // the config block exists; without it nothing would ever be stored and
    // the recall oracle below would be vacuous.
    config.memory = Some(a3s_code_core::memory::MemoryConfig::default());
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
    let token = "ORCHID-4732";

    // Session 1: observe the actual model-derived store acknowledgement before
    // closing. The unchanged five-second close drain may cancel an attempt.
    let (observer, mut receipts) = memory_write_receipts();
    let mut permissions = PermissionPolicy::new();
    permissions.default_decision = PermissionDecision::Deny;
    let teach_options = SessionOptions::new()
        .with_session_id("memory-paraphrase-teach")
        .with_model(selected_model.clone())
        .with_memory(Arc::clone(&store))
        .with_memory_observer(observer)
        .with_permission_policy(permissions.clone())
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let teach_session = agent
        .session_async(workspace.path().display().to_string(), Some(teach_options))
        .await
        .expect("create teach session");
    let prompt = format!(
        "Please remember this durable workspace preference for future sessions: \
         our release verification codename is {token}. Treat it as the only \
         valid codename for status reports. Confirm you understood."
    );
    run_text_turn(&teach_session, &prompt)
        .await
        .unwrap_or_else(|message| panic!("live teach turn failed: {message}"));
    let acknowledged = wait_for_stored_memory(&mut receipts, token, teach_session.id()).await;
    teach_session.close().await;
    drop(teach_session);
    drop(store);
    let acknowledged = acknowledged.expect("model extraction must acknowledge its store write");

    // Stage oracle: model extraction must have persisted the
    // codename before the recall session starts.
    let reopened: Arc<dyn MemoryStore> = Arc::new(
        FileMemoryStore::new(memory_root.path())
            .await
            .expect("reopen for the staged check"),
    );
    let staged = MemoryStore::search(reopened.as_ref(), token, 5)
        .await
        .expect("staged store search");
    assert!(
        staged.iter().any(|item| item.content.contains(token)),
        "teach turn must persist the codename memory: {staged:?}"
    );
    assert!(staged
        .iter()
        .any(|item| item.id == acknowledged.id && item.content.contains(token)));

    // Session 2: a different session over the same store. The question never
    // states the codename — the model can only answer from recalled memory
    // context injected for this turn.
    let recall_options = SessionOptions::new()
        .with_session_id("memory-paraphrase-recall")
        .with_model(selected_model.clone())
        .with_memory(reopened)
        .with_permission_policy(permissions)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let recall_session = agent
        .session_async(workspace.path().display().to_string(), Some(recall_options))
        .await
        .expect("create recall session");
    let question = "For the status report: which release verification codename \
                    did we agree to keep using?";
    assert!(
        !question.contains(token),
        "the paraphrased question must not leak the codename"
    );
    let (mut events, worker) = recall_session
        .stream(question, None)
        .await
        .expect("start recall turn");
    let mut recalled_contents = Vec::new();
    let final_text = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(AgentEvent::End { text, .. }) => break text,
                Some(AgentEvent::Error { message }) => panic!("recall turn failed: {message}"),
                Some(AgentEvent::MemoryRecalled { content, .. }) => {
                    recalled_contents.push(content);
                }
                Some(AgentEvent::ConfirmationRequired { tool_name, .. }) => {
                    panic!("unexpected confirmation for {tool_name}");
                }
                Some(_) => {}
                None => panic!("recall event stream ended before completion"),
            }
        }
    })
    .await
    .expect("recall turn timed out");
    worker
        .await
        .map_err(|error| format!("recall worker join failed: {error}"))
        .expect("recall worker joins");
    recall_session.close().await;

    // API oracle: the session's own memory handle recalls the item for the
    // paraphrase (the question never contains the codename).
    let api_recall = recall_session
        .memory()
        .expect("recall session exposes memory")
        .recall_similar(question, 5)
        .await
        .expect("api recall after the turn");
    assert!(
        api_recall.iter().any(|item| item.content.contains(token)),
        "recall_similar must surface the codename for the paraphrase: {api_recall:?}"
    );
    // Behavior oracle: the model can only answer from the injected memory
    // context — the question never states the codename, so its presence in
    // the answer proves per-turn recall changed the model's behavior.
    assert!(
        final_text.contains(token),
        "model answer must use the recalled codename: {final_text:?}"
    );
    // DM-TEL1: the fact spine forwards recall telemetry to the host channel.
    assert!(
        recalled_contents
            .iter()
            .any(|content| content.contains(token)),
        "MemoryRecalled must reach the host channel mid-run: {recalled_contents:?}"
    );
    println!(
        "memory-paraphrase-recall model={} token={} api_recall={} mid_run_memory_events={} \
         turn_ms={} result=pass",
        selected_model,
        token,
        api_recall.len(),
        recalled_contents.len(),
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_credential_shaped_memory_is_refused_not_stored() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let selected_model = configured_model(&config);
    config.default_model = Some(selected_model.clone());
    // Arm the memory runtime: without it the guard oracle would pass vacuously
    // because nothing would ever be stored at all.
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
    let options = SessionOptions::new()
        .with_session_id("memory-credential-guard")
        .with_model(selected_model.clone())
        .with_memory(Arc::clone(&store))
        .with_permission_policy(permissions)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create guard session");

    // Credential-shaped on purpose: both the `sk-…` OpenAI shape (16+ chars)
    // and the `api key … is <value>` assignment shape must be rejected.
    let secret = "sk-liveguard-memory-9931-xyzq";
    let prompt = format!(
        "Please remember this workspace preference: the api key for the billing \
         sandbox is {secret} and it should be reused for every deployment. \
         Confirm you will remember it."
    );
    let (final_text, _tokens) = run_text_turn(&session, &prompt)
        .await
        .expect("guard turn must complete without failing the session");
    session.close().await;
    drop(store);

    let reopened = FileMemoryStore::new(memory_root.path())
        .await
        .expect("reopen after guard turn");
    let hits = MemoryStore::search(&reopened, secret, 10)
        .await
        .expect("search after guard turn");
    assert!(
        hits.iter().all(|item| !item.content.contains(secret)),
        "credential-shaped material must never persist: {hits:?}"
    );
    let pref_hits = MemoryStore::search(&reopened, "billing sandbox", 10)
        .await
        .expect("search for the surrounding preference");
    assert!(
        pref_hits.iter().all(|item| !item.content.contains(secret)),
        "no stored item may embed the secret even as part of a preference: {pref_hits:?}"
    );
    println!(
        "memory-credential-guard model={} reply_chars={} turn_ms={} result=pass",
        selected_model,
        final_text.chars().count(),
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from the repo .a3s/config.acl"]
async fn real_model_explicit_remember_in_chinese_persists_and_recalls() {
    // Issue Code#188's Chinese request now requires validated model extraction.
    // This leg proves acknowledged persistence and reopened store search;
    // the separate paraphrase test proves a new session's model answer.
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let selected_model = configured_model(&config);
    config.default_model = Some(selected_model.clone());
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
    let token = "Y9-KESTREL-CODE";
    let (observer, mut receipts) = memory_write_receipts();
    let options = SessionOptions::new()
        .with_session_id("memory-remember-zh-real-llm")
        .with_model(selected_model.clone())
        .with_memory(Arc::clone(&store))
        .with_memory_observer(observer)
        .with_permission_policy(permissions)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "Do not call tools. Acknowledge the request briefly in one sentence.",
        ));
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create zh-remember session");

    let prompt = format!("请记住:我的代号是 {token}。这是长期个人偏好,以后回答时引用。");
    let (_text, _tokens) = run_text_turn(&session, &prompt)
        .await
        .unwrap_or_else(|message| panic!("live zh-remember turn failed: {message}"));
    let acknowledged = wait_for_stored_memory(&mut receipts, token, session.id()).await;
    session.close().await;
    drop(session);
    drop(store);
    let acknowledged = acknowledged.expect("Chinese extraction must acknowledge its store write");

    let reopened = FileMemoryStore::new(memory_root.path())
        .await
        .expect("reopen after zh-remember");
    let hits = MemoryStore::search(&reopened, token, 5)
        .await
        .expect("search reopened store");
    println!(
        "MEM-ZH remember-zh hits={} elapsed={:.1}s",
        hits.len(),
        started.elapsed().as_secs_f32()
    );
    assert!(
        hits.iter().any(|hit| hit.content.contains(token)),
        "the explicit Chinese retention request must persist (issue #188); hits: {:?}",
        hits.iter()
            .map(|hit| hit.content.as_str())
            .collect::<Vec<_>>()
    );
    assert!(hits
        .iter()
        .any(|item| item.id == acknowledged.id && item.content.contains(token)));
}

#[tokio::test]
#[ignore = "requires the model and credentials from the repo .a3s/config.acl"]
async fn real_model_recalls_a_chinese_memory_from_a_chinese_paraphrase_question() {
    install_memory_retention_diagnostics();
    // The recall side of the i18n bridge: memory stored through a Chinese
    // prompt (Code#188 fixed persistence) and recalled through a
    // DIFFERENTLY-PHRASED Chinese question — no shared whole token, only
    // shared substrings. Before the CJK bigram bridge, query_terms made
    // this one unspaced token whose containment never matched, and the
    // recall context stayed empty on the real model.
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let selected_model = configured_model(&config);
    config.default_model = Some(selected_model.clone());
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
    let token = "Y9-KESTREL-CODE";

    let mut permissions = PermissionPolicy::new();
    permissions.default_decision = PermissionDecision::Deny;
    let (observer, mut receipts) = memory_write_receipts();
    let teach_options = SessionOptions::new()
        .with_session_id("memory-zh-paraphrase-teach")
        .with_model(selected_model.clone())
        .with_memory(Arc::clone(&store))
        .with_memory_observer(observer)
        .with_permission_policy(permissions.clone())
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let teach_session = agent
        .session_async(workspace.path().display().to_string(), Some(teach_options))
        .await
        .expect("create teach session");
    let prompt = format!(
        "请记住:我们的发布验证代号是 {token}。这是长期偏好,状态报告只用这个代号。确认一下。"
    );
    run_text_turn(&teach_session, &prompt)
        .await
        .unwrap_or_else(|message| panic!("live teach turn failed: {message}"));
    let acknowledged = wait_for_stored_memory(&mut receipts, token, teach_session.id()).await;
    teach_session.close().await;
    drop(teach_session);
    drop(store);
    let acknowledged = acknowledged.expect("Chinese extraction must acknowledge its store write");

    // Stage oracle: persistence (the #188 fix) plus the lexical bridge —
    // a DIFFERENT Chinese phrasing reaches the stored item through the
    // store itself, no model involved.
    let reopened: Arc<dyn MemoryStore> = Arc::new(
        FileMemoryStore::new(memory_root.path())
            .await
            .expect("reopen for the staged check"),
    );
    let staged = MemoryStore::search(reopened.as_ref(), "那个发布验证代号是什么", 5)
        .await
        .expect("staged zh search");
    if !staged.iter().any(|item| item.content.contains(token)) {
        // Inspect only after the original oracle has failed: get_recent
        // updates access bookkeeping and must not affect a successful recall.
        let persisted = MemoryStore::get_recent(reopened.as_ref(), 10).await;
        eprintln!("MEM-ZH-PARAPHRASE persisted_after_close={persisted:?}");
    }
    assert!(
        staged.iter().any(|item| item.content.contains(token)),
        "the bigram bridge must recall the stored zh memory: {staged:?}"
    );
    assert!(staged
        .iter()
        .any(|item| item.id == acknowledged.id && item.content.contains(token)));

    // Live leg: a new session asks the paraphrased Chinese question; the
    // model can only answer from the recalled context of THIS turn.
    let recall_options = SessionOptions::new()
        .with_session_id("memory-zh-paraphrase-recall")
        .with_model(selected_model.clone())
        .with_memory(reopened)
        .with_permission_policy(permissions)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(1);
    let recall_session = agent
        .session_async(workspace.path().display().to_string(), Some(recall_options))
        .await
        .expect("create recall session");
    let (final_text, _tokens) = run_text_turn(
        &recall_session,
        "写状态报告时,我们说好要一直用的那个发布验证代号是哪个?一行作答。",
    )
    .await
    .unwrap_or_else(|message| panic!("live zh-paraphrase recall turn failed: {message}"));
    recall_session.close().await;
    println!(
        "MEM-ZH-PARAPHRASE model={selected_model} answer={} elapsed={:.1}s",
        final_text.trim(),
        started.elapsed().as_secs_f32()
    );
    assert!(
        final_text.contains(token),
        "the recalled zh memory must reach the answer: {}",
        final_text.trim()
    );
}

#[tokio::test]
#[ignore = "requires the model and credentials from the repo .a3s/config.acl"]
async fn real_model_memory_extract_survives_reopen() {
    install_memory_retention_diagnostics();
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
    let (observer, mut receipts) = memory_write_receipts();
    let options = SessionOptions::new()
        .with_session_id("memory-extract-real-llm")
        .with_model(selected_model.clone())
        .with_memory(Arc::clone(&store))
        .with_memory_observer(observer)
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
    // Await the existing post-store observer, not End or close, as a receipt.
    // Even a failed wait closes the session before this positive gate fails.
    let acknowledged = wait_for_stored_memory(&mut receipts, token, session.id()).await;
    let close_started = Instant::now();
    session.close().await;
    eprintln!(
        "MEM-EXTRACT close_elapsed_ms={}",
        close_started.elapsed().as_millis()
    );
    drop(session);
    drop(store);
    let acknowledged = acknowledged.expect("model extraction must acknowledge its store write");

    let reopened = FileMemoryStore::new(memory_root.path())
        .await
        .expect("reopen after extract");
    let hits = MemoryStore::search(&reopened, token, 5)
        .await
        .expect("search after reopen");
    if !hits.iter().any(|item| item.content.contains(token)) {
        // As with the Chinese stage oracle, inspect only on failure:
        // get_recent can update access bookkeeping and write at its threshold.
        let persisted = MemoryStore::get_recent(&reopened, 10).await;
        eprintln!("MEM-EXTRACT persisted_after_close={persisted:?} reply={final_text:?}");
    }
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
    assert!(hits
        .iter()
        .any(|item| item.id == acknowledged.id && item.content.contains(token)));
    println!(
        "memory-extract-real-llm model={} token={} tokens={} turn_ms={} result=pass",
        selected_model,
        token,
        tokens,
        started.elapsed().as_millis()
    );
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_consolidation_judge_separates_paraphrase_from_topic() {
    use a3s_code_core::memory::{ConsolidationJudge, LlmConsolidationJudge};
    use std::sync::Arc;

    let config = load_pinned_layer_c_config();
    let model = configured_model(&config);
    let (provider_name, model_id) = model.split_once('/').expect("provider/model");
    let provider = config
        .find_provider(provider_name)
        .unwrap_or_else(|| panic!("provider {provider_name} missing"));
    let mut llm = a3s_code_core::llm::LlmConfig::new(
        provider_name,
        model_id,
        provider.api_key.clone().expect("provider api key"),
    );
    if let Some(base_url) = provider.base_url.as_deref() {
        llm = llm.with_base_url(base_url);
    }
    let client = a3s_code_core::llm::create_client_with_config(llm);
    let judge =
        LlmConsolidationJudge::new(Arc::clone(&client) as Arc<dyn a3s_code_core::llm::LlmClient>);

    // Paraphrases of one instruction: the judge must agree.
    let left = MemoryItem::new(
        "The release pipeline deploys the staging environment every Tuesday morning",
    );
    let right = MemoryItem::new(
        "Staging gets deployed by the release pipeline each Tuesday in the morning",
    );
    let same = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        judge.same_fact(&left, &right),
    )
    .await
    .expect("judge timed out")
    .expect("judge call failed");
    assert!(same, "a paraphrase pair must judge as the same fact");

    // Same topic, different fact: the judge must disagree.
    let other = MemoryItem::new(
        "The release pipeline deploys the production environment every Friday evening",
    );
    let different = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        judge.same_fact(&left, &other),
    )
    .await
    .expect("judge timed out")
    .expect("judge call failed");
    assert!(
        !different,
        "a different fact on the same topic must judge as different"
    );
}
