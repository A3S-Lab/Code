#![cfg(test)]

use super::*;
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};

fn options(id: &str) -> crate::SessionOptions {
    crate::SessionOptions::new()
        .with_session_id(id)
        .with_planning_mode(crate::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
}

async fn agent() -> crate::Agent {
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    crate::Agent::from_config(config).await.unwrap()
}

async fn prove_policy_session(store: Arc<dyn SessionStore>, reopen: Option<std::path::PathBuf>) {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(SkillCapturingClient {
        systems: systems.clone(),
    });
    let policy =
        crate::WorkPolicyV1::new("engineering", 1, "  POLICY-BYTES-保留原文。\n \t").unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("policy-session")
                .with_session_store(store.clone())
                .with_auto_save(true)
                .with_work_policy(policy.clone())
                .with_prompt_slots(
                    crate::SystemPromptSlots::default().with_role("Preserve this host role."),
                ),
        )
        .unwrap();
    let history = [
        Message::user("prior user"),
        Message::assistant("prior answer"),
    ];
    session.send("first", Some(&history)).await.unwrap();
    session.send("second", None).await.unwrap();
    assert_eq!(systems.lock().unwrap().len(), 2);
    for system in systems.lock().unwrap().iter() {
        assert_eq!(system.matches(policy.instructions()).count(), 1);
        assert_eq!(system.matches("Preserve this host role").count(), 1);
    }
    let facts = read_workspace_facts(workspace.path(), "policy-session").unwrap();
    assert_eq!(
        facts[0].kind,
        crate::standing_context::STANDING_CONTEXT_FACT_KIND
    );
    assert_eq!(facts[1].kind, "a3s.work-policy.admitted");
    assert_eq!(facts[1].payload["digest"], policy.digest().unwrap());
    let prior = facts
        .iter()
        .find(|fact| fact.cause.as_deref() == Some("infer:1:0"))
        .expect("policy metadata cannot hide original conversation history");
    assert_eq!(prior.payload["text"], "prior answer");
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == "a3s.work-policy.admitted")
            .count(),
        1
    );
    session.save().await.unwrap();
    session.close().await;
    let store: Arc<dyn SessionStore> = match reopen {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    assert!(store.load("policy-session").await.unwrap().is_some());
    let resumed = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("policy-session").with_session_store(store.clone()),
        )
        .unwrap();
    resumed.send("after actual reopen", None).await.unwrap();
    assert_eq!(
        systems
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .matches(policy.instructions())
            .count(),
        1
    );
    resumed.close().await;

    let before = read_workspace_facts(workspace.path(), "policy-session").unwrap();
    let calls = systems.lock().unwrap().len();
    let changed = crate::WorkPolicyV1::new("engineering", 2, "Different host policy.").unwrap();
    let Err(error) = agent.build_session(
        workspace.path().display().to_string(),
        client,
        &options("policy-session").with_work_policy(changed),
    ) else {
        panic!("a changed policy refuses at session admission");
    };
    assert!(
        error.to_string().contains("work_policy.conflict"),
        "{error}"
    );
    assert_eq!(systems.lock().unwrap().len(), calls);
    assert_eq!(
        read_workspace_facts(workspace.path(), "policy-session").unwrap(),
        before
    );
    agent.close().await;
}

#[tokio::test]
async fn policy_session_uses_original_memory_and_file_stores_with_actual_reopen() {
    prove_policy_session(Arc::new(MemorySessionStore::new()), None).await;
    let directory = tempfile::tempdir().unwrap();
    prove_policy_session(
        Arc::new(FileSessionStore::new(directory.path()).await.unwrap()),
        Some(directory.path().to_path_buf()),
    )
    .await;
}

async fn prove_policy_admitted_before_first_run(
    store: Arc<dyn SessionStore>,
    reopen: Option<std::path::PathBuf>,
) {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(SkillCapturingClient {
        systems: systems.clone(),
    });
    let policy = crate::WorkPolicyV1::new("engineering", 1, "Admitted before any Run.").unwrap();
    let digest = policy.digest().unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("early-policy")
                .with_session_store(store.clone())
                .with_work_policy(policy),
        )
        .unwrap();
    assert_eq!(session.work_policy_digest().unwrap(), Some(digest.clone()));
    let admitted = read_workspace_facts(workspace.path(), "early-policy").unwrap();
    let kinds: Vec<&str> = admitted.iter().map(|fact| fact.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            crate::standing_context::STANDING_CONTEXT_FACT_KIND,
            "a3s.work-policy.admitted"
        ]
    );
    assert!(systems.lock().unwrap().is_empty());
    session.save().await.unwrap();
    session.close().await;

    let store: Arc<dyn SessionStore> = match reopen {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    let resumed = agent
        .resume_session_async(
            "early-policy",
            options("early-policy")
                .with_session_store(store)
                .with_llm_client(client.clone()),
        )
        .await
        .unwrap();
    assert_eq!(resumed.work_policy_digest().unwrap(), Some(digest));
    assert_eq!(
        read_workspace_facts(workspace.path(), "early-policy").unwrap(),
        admitted
    );
    resumed.close().await;

    let plain = agent
        .build_session(
            workspace.path().display().to_string(),
            client,
            &options("no-policy"),
        )
        .unwrap();
    assert_eq!(plain.work_policy_digest().unwrap(), None);
    assert!(read_workspace_facts(workspace.path(), "no-policy")
        .unwrap()
        .is_empty());
    plain.close().await;
    agent.close().await;
}

#[tokio::test]
async fn a_policy_session_admits_its_digest_before_the_first_run() {
    prove_policy_admitted_before_first_run(Arc::new(MemorySessionStore::new()), None).await;
    let directory = tempfile::tempdir().unwrap();
    prove_policy_admitted_before_first_run(
        Arc::new(FileSessionStore::new(directory.path()).await.unwrap()),
        Some(directory.path().to_path_buf()),
    )
    .await;
}

#[tokio::test]
async fn existing_execution_history_refuses_policy_rebinding_without_a_model_or_fact() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(SkillCapturingClient {
        systems: systems.clone(),
    });
    let original = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("legacy-policy"),
        )
        .unwrap();
    original.send("legacy", None).await.unwrap();
    original.close().await;
    let before = read_workspace_facts(workspace.path(), "legacy-policy").unwrap();
    let calls = systems.lock().unwrap().len();
    let Err(error) = agent.build_session(
        workspace.path().display().to_string(),
        client,
        &options("legacy-policy")
            .with_work_policy(crate::WorkPolicyV1::new("p", 1, "Policy body.").unwrap()),
    ) else {
        panic!("execution history refuses a new policy at session admission");
    };
    assert!(
        error.to_string().contains("work_policy.legacy_rebind"),
        "{error}"
    );
    assert_eq!(systems.lock().unwrap().len(), calls);
    assert_eq!(
        read_workspace_facts(workspace.path(), "legacy-policy").unwrap(),
        before
    );
    agent.close().await;
}

#[tokio::test]
async fn corrupt_policy_source_refuses_before_provider_entry() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent().await;
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(SkillCapturingClient {
        systems: systems.clone(),
    });
    let options = options("corrupt-policy")
        .with_work_policy(crate::WorkPolicyV1::new("p", 1, "Policy body.").unwrap());
    let original = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options,
        )
        .unwrap();
    original.send("admit", None).await.unwrap();
    original.close().await;
    let mut facts = read_workspace_facts(workspace.path(), "corrupt-policy").unwrap();
    facts
        .iter_mut()
        .find(|fact| fact.kind == "a3s.work-policy.admitted")
        .unwrap()
        .payload["schema"] = serde_json::json!("future-policy-schema");
    let path = crate::session_folder::effect_log_dir(workspace.path(), "corrupt-policy")
        .join("corrupt-policy.jsonl");
    let bytes = facts
        .iter()
        .map(|fact| serde_json::to_string(fact).unwrap() + "\n")
        .collect::<String>();
    std::fs::write(&path, &bytes).unwrap();
    let calls = systems.lock().unwrap().len();
    let Err(error) = agent.build_session(workspace.path().display().to_string(), client, &options)
    else {
        panic!("a corrupt policy source refuses at session admission");
    };
    assert!(error.to_string().contains("work_policy.corrupt"), "{error}");
    assert_eq!(systems.lock().unwrap().len(), calls);
    assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
    agent.close().await;
}

#[tokio::test]
async fn two_actual_policy_sessions_share_original_owner_exclusion_and_legacy_refusal() {
    let workspace = tempfile::tempdir().unwrap();
    let first_agent = agent().await;
    let second_agent = agent().await;
    let first_client = Arc::new(AttachmentSteerClient {
        text: CountingText {
            calls: Arc::new(AtomicUsize::new(0)),
        },
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        seen: Default::default(),
    });
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let second_client = Arc::new(SkillCapturingClient {
        systems: systems.clone(),
    });
    let policy = crate::WorkPolicyV1::new("p", 1, "Keep original host policy.").unwrap();
    let options = options("shared-policy-owner").with_work_policy(policy.clone());
    let first = Arc::new(
        first_agent
            .build_session(
                workspace.path().display().to_string(),
                first_client.clone(),
                &options,
            )
            .unwrap(),
    );
    let second = second_agent
        .build_session(
            workspace.path().display().to_string(),
            second_client,
            &options,
        )
        .unwrap();
    let worker = {
        let first = first.clone();
        tokio::spawn(async move { first.send("hold original provider call", None).await })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        first_client.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    let before = read_workspace_facts(workspace.path(), "shared-policy-owner").unwrap();
    assert!(matches!(
        second.send("must refuse overlap", None).await,
        Err(crate::error::CodeError::SessionBusy { .. })
    ));
    let unqualified = super::super::super::conversation_runtime::FactSession::from(&second)
        .open()
        .err()
        .expect("legacy opener cannot execute a bound policy");
    assert!(unqualified.to_string().contains("work_policy.invalid"));
    assert!(systems.lock().unwrap().is_empty());
    assert_eq!(
        read_workspace_facts(workspace.path(), "shared-policy-owner").unwrap(),
        before
    );
    first_client.release.add_permits(1);
    worker.await.unwrap().unwrap();
    second
        .send("after actual original retirement", None)
        .await
        .unwrap();
    assert_eq!(systems.lock().unwrap().len(), 1);
    assert_eq!(
        systems.lock().unwrap()[0]
            .matches(policy.instructions())
            .count(),
        1
    );
    assert_eq!(
        read_workspace_facts(workspace.path(), "shared-policy-owner")
            .unwrap()
            .iter()
            .filter(|fact| fact.kind == "a3s.work-policy.admitted")
            .count(),
        1
    );
    first.close().await;
    second.close().await;
    first_agent.close().await;
    second_agent.close().await;
}
