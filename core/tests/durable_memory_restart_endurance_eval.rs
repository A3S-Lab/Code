#[path = "durable_memory_restart_endurance_eval/support.rs"]
mod support;

use a3s_code_core::store::{FileSessionStore, SessionStore};
use a3s_code_core::{
    Agent, AgentSession, DurableMemoryRecallPolicy, DurableMemorySession,
    DURABLE_MEMORY_BINDING_SCHEMA_VERSION, DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, FileMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus,
    RevisionMode,
};
use futures::future::join_all;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use support::{EpochClient, Fixture, Observation, Report};
use tokio::sync::Barrier;

const FIXTURE: &str = include_str!("fixtures/durable-memory-restart-endurance-v1/evaluation.json");

fn evidence(uri: &str, content: &str, kind: EvidenceKind) -> EvidenceRef {
    EvidenceRef::try_new(
        uri,
        format!("sha256:{:x}", Sha256::digest(content.as_bytes())),
        kind,
        support::fixture_time(),
    )
    .expect("valid restart endurance evidence")
}

async fn seed_fixture(
    repository: &FileMemoryRepository,
    namespace: &MemoryNamespace,
    foreign_namespace: &MemoryNamespace,
    fixture: &Fixture,
) {
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-restart-endurance-v1",
            namespace.clone(),
            support::fixture_time(),
            vec![
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        &fixture.active_node.id,
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        &fixture.active_node.revisions[0],
                        vec![evidence(
                            "a3s://fixture/restart-endurance/active",
                            &fixture.active_node.revisions[0],
                            EvidenceKind::Verification,
                        )],
                        support::fixture_time(),
                    ),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        &fixture.candidate_node.id,
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Candidate,
                        &fixture.candidate_node.content,
                        vec![evidence(
                            "a3s://fixture/restart-endurance/candidate",
                            &fixture.candidate_node.content,
                            EvidenceKind::SessionTurn,
                        )],
                        support::fixture_time(),
                    ),
                },
            ],
        ))
        .await
        .expect("seed restart endurance namespace");
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-restart-endurance-foreign-v1",
            foreign_namespace.clone(),
            support::fixture_time(),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    &fixture.foreign_node.id,
                    foreign_namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Active,
                    &fixture.foreign_node.content,
                    vec![evidence(
                        "a3s://fixture/restart-endurance/foreign",
                        &fixture.foreign_node.content,
                        EvidenceKind::Verification,
                    )],
                    support::fixture_time(),
                ),
            }],
        ))
        .await
        .expect("seed foreign restart endurance namespace");
}

async fn revise_active_node(
    repository: &FileMemoryRepository,
    namespace: &MemoryNamespace,
    fixture: &Fixture,
) {
    repository
        .apply(MemoryChangeSet::new(
            "revise-durable-memory-restart-endurance-v1",
            namespace.clone(),
            support::fixture_time(),
            vec![MemoryOperation::Revise {
                node_id: fixture.active_node.id.clone(),
                expected_revision: 1,
                content: fixture.active_node.revisions[1].clone(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence(
                    "a3s://fixture/restart-endurance/revision-2",
                    &fixture.active_node.revisions[1],
                    EvidenceKind::Verification,
                )],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .expect("revise active memory between restart epochs");
}

async fn run_epoch(
    epoch: usize,
    fixture: &Fixture,
    current_content: &str,
    workspace: &Path,
    session_root: &Path,
    durable: &DurableMemorySession,
    observations: Arc<Mutex<Vec<Observation>>>,
) -> Vec<String> {
    let session_store = Arc::new(
        FileSessionStore::new(session_root)
            .await
            .expect("open restart endurance session store"),
    );
    let barrier = Arc::new(Barrier::new(fixture.agents.len()));
    let mut owners: Vec<(Agent, AgentSession)> = Vec::with_capacity(fixture.agents.len());
    for agent_fixture in &fixture.agents {
        let client = Arc::new(EpochClient::new(
            &agent_fixture.id,
            epoch,
            fixture,
            current_content,
            barrier.clone(),
            observations.clone(),
        ));
        let agent = Agent::from_config(support::offline_config())
            .await
            .expect("construct independent restart endurance agent");
        let options = support::session_options(
            agent_fixture,
            fixture,
            session_store.clone(),
            durable.clone(),
            client,
        );
        let session = if epoch == 0 {
            agent
                .session_async(
                    workspace.display().to_string(),
                    Some(options.with_session_id(&agent_fixture.session_id)),
                )
                .await
        } else {
            agent
                .resume_session_async(&agent_fixture.session_id, options)
                .await
        }
        .expect("construct or resume restart endurance session");
        owners.push((agent, session));
    }

    for _ in 0..fixture.turns_per_agent_per_epoch {
        let results = join_all(
            owners
                .iter()
                .map(|(_, session)| session.send(&fixture.query, None)),
        )
        .await;
        for (result, agent_fixture) in results.into_iter().zip(&fixture.agents) {
            assert_eq!(
                result.expect("complete restart endurance turn").text,
                format!("PASS:{}:epoch{}", agent_fixture.id, epoch)
            );
        }
    }

    let mut retained_run_ids = Vec::with_capacity(owners.len());
    for (_, session) in &owners {
        let runs = session.runs().await;
        assert_eq!(runs.len(), fixture.max_runs_retained);
        retained_run_ids.push(runs[0].id.clone());
        session
            .save()
            .await
            .expect("save restart endurance session");
    }
    for (agent, session) in owners {
        session.close().await;
        drop(session);
        agent.close().await;
    }
    drop(session_store);
    retained_run_ids
}

#[tokio::test]
async fn repeated_restart_reuse_preserves_every_context_and_current_revision() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid restart endurance fixture");
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(
        fixture.binding_schema_version,
        DURABLE_MEMORY_BINDING_SCHEMA_VERSION
    );
    assert_eq!(
        fixture.context_identity_profile,
        DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2
    );
    assert_eq!(fixture.active_node.revisions.len(), 2);
    assert_eq!(fixture.agents.len(), 4);

    let repository_root = tempfile::tempdir().expect("create endurance repository root");
    let session_root = tempfile::tempdir().expect("create endurance session root");
    let workspace = tempfile::tempdir().expect("create endurance workspace");
    let namespace =
        MemoryNamespace::try_new("beacon-tenant", "beacon-team", "beacon-workspace").unwrap();
    let foreign_namespace =
        MemoryNamespace::try_new("beacon-tenant", "foreign-team", "beacon-workspace").unwrap();
    let policy = DurableMemoryRecallPolicy::try_new(3, 0.20).unwrap();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let mut run_ids_by_session: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut repository_opens = 0usize;

    for epoch in 0..fixture.epochs {
        let repository = Arc::new(
            FileMemoryRepository::open(repository_root.path())
                .await
                .expect("open repository for restart epoch"),
        );
        repository_opens += 1;
        if epoch == 0 {
            seed_fixture(&repository, &namespace, &foreign_namespace, &fixture).await;
        }
        let durable =
            DurableMemorySession::active_recall(repository.clone(), namespace.clone(), policy);
        let current_revision = usize::from(epoch + 1 == fixture.epochs);
        let retained = run_epoch(
            epoch,
            &fixture,
            &fixture.active_node.revisions[current_revision],
            workspace.path(),
            session_root.path(),
            &durable,
            observations.clone(),
        )
        .await;
        for (agent, run_id) in fixture.agents.iter().zip(retained) {
            run_ids_by_session
                .entry(agent.session_id.clone())
                .or_default()
                .push(run_id);
        }
        let expected =
            u64::try_from((epoch + 1) * fixture.agents.len() * fixture.turns_per_agent_per_epoch)
                .unwrap();
        assert_eq!(
            repository
                .usage_summary(&namespace, &fixture.active_node.id)
                .await
                .unwrap()
                .admissions,
            expected,
            "every distinct context must remain visible across restart and run retention"
        );
        if epoch + 2 == fixture.epochs {
            revise_active_node(&repository, &namespace, &fixture).await;
        }
        drop(durable);
        drop(repository);
    }

    let repository = FileMemoryRepository::open(repository_root.path())
        .await
        .expect("reopen repository for final endurance verification");
    repository_opens += 1;
    let node = repository
        .get(&namespace, &fixture.active_node.id)
        .await
        .unwrap()
        .unwrap();
    let admissions = repository
        .usage_summary(&namespace, &fixture.active_node.id)
        .await
        .unwrap()
        .admissions;
    let candidate_admissions = repository
        .usage_summary(&namespace, &fixture.candidate_node.id)
        .await
        .unwrap()
        .admissions;
    assert_eq!(admissions, fixture.thresholds.expected_admissions);
    assert_eq!(
        candidate_admissions,
        fixture.thresholds.expected_candidate_admissions
    );
    assert_eq!(node.revision, 2);
    assert_eq!(node.history.len(), 1);
    assert_eq!(node.content, fixture.active_node.revisions[1]);
    assert_eq!(
        repository
            .usage_summary(&foreign_namespace, &fixture.foreign_node.id)
            .await
            .unwrap()
            .admissions,
        0
    );

    let observations = observations.lock().unwrap().clone();
    let forbidden_context_hits = observations
        .iter()
        .filter(|observation| observation.forbidden_visible)
        .count();
    assert_eq!(observations.len(), fixture.thresholds.expected_model_calls);
    assert!(observations.iter().all(|item| item.current_visible));
    assert!(forbidden_context_hits <= fixture.thresholds.maximum_forbidden_context_hits);
    assert_eq!(
        repository_opens,
        fixture.thresholds.expected_repository_opens
    );
    let session_resumes = fixture.agents.len() * fixture.epochs.saturating_sub(1);
    assert_eq!(session_resumes, fixture.thresholds.expected_session_resumes);
    let reused_retained_run_ids = run_ids_by_session
        .values()
        .all(|ids| ids.len() == fixture.epochs && ids.windows(2).all(|pair| pair[0] == pair[1]));
    assert!(reused_retained_run_ids);

    let session_store = FileSessionStore::new(session_root.path())
        .await
        .expect("reopen final endurance session store");
    for agent in &fixture.agents {
        let snapshot = session_store
            .load_snapshot(&agent.session_id)
            .await
            .unwrap()
            .expect("persisted endurance session");
        let binding = snapshot
            .session
            .durable_memory_binding
            .expect("persisted durable-memory binding");
        assert_eq!(binding.schema_version(), fixture.binding_schema_version);
        assert_eq!(
            binding.context_id_profile(),
            fixture.context_identity_profile
        );
        assert_eq!(snapshot.run_records.len(), fixture.max_runs_retained);
    }

    println!(
        "A3S_DURABLE_MEMORY_RESTART_ENDURANCE_EVAL={}",
        serde_json::to_string(&Report {
            schema_version: fixture.schema_version,
            binding_schema_version: DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
            context_identity_profile: DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2,
            epochs: fixture.epochs,
            independent_agents_per_epoch: fixture.agents.len(),
            model_calls: observations.len(),
            admissions,
            candidate_admissions,
            forbidden_context_hits,
            repository_opens,
            session_resumes,
            reused_retained_run_ids,
            final_node_revision: node.revision,
        })
        .expect("serialize restart endurance report")
    );
}
