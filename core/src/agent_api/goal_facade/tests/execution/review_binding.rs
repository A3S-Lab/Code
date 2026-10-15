#![cfg(test)]

use super::*;
use crate::session_review::{
    ReviewFindingDraft, ReviewScenario, ReviewSubjectV1, ReviewTriggerPolicy,
    SessionReviewSeverityV1, SessionReviewStatusV1,
};

struct Explicit;

impl ReviewScenario for Explicit {
    fn scenario_id(&self) -> &str {
        "engineering.change"
    }
}

fn pending_ids(session: &AgentSession) -> Vec<String> {
    session
        .pending_session_review_findings()
        .into_iter()
        .map(|finding| finding.finding_id)
        .collect()
}

#[tokio::test]
async fn engineering_findings_lifecycle_survives_both_stores_and_replan_stales_the_step() {
    for file_store in [false, true] {
        prove_findings_reopen(file_store).await;
    }
}

async fn prove_findings_reopen(file_store: bool) {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = if file_store {
        Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run").with_session_store(store.clone()),
    )
    .await;
    assert_eq!(Explicit.triggers(), ReviewTriggerPolicy::ExplicitRequest);
    session
        .register_review_scenario(Arc::new(Explicit))
        .unwrap();
    session
        .send("Continue the admitted current step.", None)
        .await
        .unwrap();
    let goal = session.goal_definition().unwrap().unwrap();
    let run = slice_runs(root.path()).pop().unwrap();
    let plan = goal.plan_revision.unwrap();
    let bind = |session: &AgentSession, plan| {
        session.check_goal_step_run(
            &goal.binding.goal_id,
            goal.binding.goal_revision,
            plan,
            "inspect",
            &run,
        )
    };
    bind(&session, plan).unwrap();
    let drafts = ["f-1", "f-2", "f-3", "f-4"].map(|id| draft(id, &format!("claim {id}")));
    assert_eq!(
        session
            .admit_session_review_drafts("engineering.change", "review-1", 1, drafts.to_vec())
            .unwrap(),
        4
    );
    session
        .mark_session_review_addressed("f-1", &run, 2)
        .unwrap();
    session
        .accept_session_review_finding("f-1", "review-2", 3)
        .unwrap();
    session
        .mark_session_review_addressed("f-2", &run, 2)
        .unwrap();
    session
        .reopen_session_review_finding("f-2", "evidence incomplete", 3)
        .unwrap();
    session.waive_session_review_finding("f-3", 3).unwrap();
    assert_eq!(pending_ids(&session), ["f-2", "f-4"]);
    let saved = session.session_review_store();
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let calls = client.calls.load(Ordering::SeqCst);
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let store: Arc<dyn SessionStore> = if file_store {
        Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
    } else {
        store
    };
    let restored = agent
        .resume_session_async("goal-run", options("goal-run").with_session_store(store))
        .await
        .unwrap();
    assert_eq!(restored.session_review_store(), saved);
    assert!(
        pending_ids(&restored).is_empty(),
        "an unregistered scenario injects nothing"
    );
    restored
        .register_review_scenario(Arc::new(Explicit))
        .unwrap();
    assert_eq!(pending_ids(&restored), ["f-2", "f-4"]);
    assert_eq!(
        restored
            .admit_session_review_drafts("engineering.change", "review-1", 9, drafts.to_vec())
            .unwrap(),
        0,
        "redelivery after reopen keeps every lifecycle state"
    );
    assert_eq!(restored.session_review_store(), saved);
    assert!(restored
        .admit_session_review_drafts(
            "engineering.change",
            "review-1",
            9,
            vec![draft("f-1", "revised artifact claim")]
        )
        .is_err());
    restored
        .mark_session_review_addressed("f-2", &run, 10)
        .unwrap();
    restored
        .accept_session_review_finding("f-2", "review-3", 11)
        .unwrap();
    let statuses: Vec<_> = restored
        .session_review_store()
        .findings
        .iter()
        .map(|finding| finding.status)
        .collect();
    assert_eq!(
        statuses,
        [
            SessionReviewStatusV1::Accepted,
            SessionReviewStatusV1::Accepted,
            SessionReviewStatusV1::Waived,
            SessionReviewStatusV1::Pending
        ]
    );
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        facts,
        "Findings lifecycle writes no Goal fact"
    );
    bind(&restored, plan).unwrap();

    let current = restored.goal_definition().unwrap().unwrap();
    let mut replan = crate::planning::ExecutionPlan::new(
        "Repair existing callers after review",
        crate::planning::Complexity::Medium,
    );
    replan.upsert_step(
        crate::planning::Task::new("inspect", "Inspect the reviewed source again")
            .with_success_criteria("Reviewed source is checked"),
    );
    let mut proposal = crate::goal_control::GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "replan".into(),
        goal_id: current.binding.goal_id.clone(),
        session_id: current.binding.session_id.clone(),
        workspace: current.binding.workspace.clone(),
        work_policy_digest: current.binding.work_policy_digest.clone(),
        expected_goal_revision: current.binding.goal_revision,
        expected_plan_revision: current.plan_revision,
        expected_goal_sequence: current.goal_sequence,
        plan: Some(replan),
        proposal_sequence: None,
    };
    let proposed = restored.apply_goal_plan(proposal.clone()).unwrap();
    proposal.kind = GoalCommandKindV1::AcceptPlan;
    proposal.operation_id = "accept-replan".into();
    proposal.expected_plan_revision = proposed.plan_revision;
    proposal.expected_goal_sequence = proposed.goal_sequence;
    proposal.plan = None;
    proposal.proposal_sequence = proposed.plan_fact_sequence;
    let replanned = restored.apply_goal_plan(proposal).unwrap();
    assert_ne!(replanned.plan_revision, Some(plan));
    assert!(
        bind(&restored, plan).is_err(),
        "replan stales the old report"
    );
    assert!(
        bind(&restored, replanned.plan_revision.unwrap()).is_err(),
        "the old Run is not the new plan's slice"
    );
    assert!(
        !crate::fact_control::read_workspace_facts(root.path(), "goal-run")
            .unwrap()
            .iter()
            .any(|fact| fact.kind == "a3s.goal.step.accepted")
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), calls, "no model call");
    restored.close().await;
    agent.close().await;
}

fn slice_runs(root: &std::path::Path) -> Vec<String> {
    crate::fact_control::read_workspace_facts(root, "goal-run")
        .unwrap()
        .iter()
        .filter(|fact| fact.kind == "a3s.goal.slice.admitted")
        .map(|fact| fact.payload["body"]["run_id"].as_str().unwrap().to_owned())
        .collect()
}

fn draft(id: &str, claim: &str) -> ReviewFindingDraft {
    ReviewFindingDraft {
        finding_id: id.into(),
        severity: SessionReviewSeverityV1::Warning,
        category: "correctness".into(),
        claim: claim.into(),
        evidence: "src/lib.rs:3".into(),
        subject: ReviewSubjectV1::opaque("engineering.target", "sha256:target").unwrap(),
        quote: None,
        suggestion: None,
    }
}

#[tokio::test]
async fn review_binding_names_the_current_step_run_and_findings_never_accept_it() {
    let root = tempfile::tempdir().unwrap();
    let policy = crate::WorkPolicyV1::new("engineering-v1", 1, "Review before acting.").unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run").with_work_policy(policy.clone()),
    )
    .await;
    assert_eq!(
        session.work_policy_digest().unwrap(),
        Some(policy.digest().unwrap())
    );
    let goal = session.goal_definition().unwrap().unwrap();
    let check = |goal_id: &str, goal_revision, plan_revision, step: &str, run: &str| {
        session.check_goal_step_run(goal_id, goal_revision, plan_revision, step, run)
    };
    let (id, revision, plan) = (
        goal.binding.goal_id.clone(),
        goal.binding.goal_revision,
        goal.plan_revision.unwrap(),
    );
    assert!(
        check(&id, revision, plan, "inspect", "no-run").is_err(),
        "no slice yet"
    );

    session
        .send("Continue the admitted current step.", None)
        .await
        .unwrap();
    let first = slice_runs(root.path()).pop().unwrap();
    check(&id, revision, plan, "inspect", &first).unwrap();
    for (goal_id, goal_revision, plan_revision, step, run) in [
        (id.as_str(), revision, plan, "verify", first.as_str()),
        (id.as_str(), revision, plan, "inspect", "another-run"),
        ("another-goal", revision, plan, "inspect", first.as_str()),
        (id.as_str(), revision + 1, plan, "inspect", first.as_str()),
        (id.as_str(), revision, plan + 1, "inspect", first.as_str()),
    ] {
        assert!(
            check(goal_id, goal_revision, plan_revision, step, run).is_err(),
            "{goal_id} {goal_revision} {plan_revision} {step} {run}"
        );
    }

    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let calls = client.calls.load(Ordering::SeqCst);
    assert_eq!(
        session
            .admit_session_review_drafts(
                "engineering.change",
                "review-1",
                1,
                vec![draft("f-1", "off by one")]
            )
            .unwrap(),
        1
    );
    assert_eq!(
        session
            .admit_session_review_drafts(
                "engineering.change",
                "review-1",
                2,
                vec![draft("f-1", "off by one")]
            )
            .unwrap(),
        0,
        "identical redelivery is a no-op"
    );
    session
        .mark_session_review_addressed("f-1", &first, 3)
        .unwrap();
    session
        .accept_session_review_finding("f-1", "review-2", 4)
        .unwrap();
    let accepted = session.session_review_store();
    for batch in [
        vec![draft("f-1", "off by one")],
        vec![draft("f-1", "other content")],
        vec![draft("f-2", "new"), draft("f-1", "other content")],
        vec![draft("f-3", "new"), draft("f-3", "new")],
    ] {
        let redelivered = batch.len() == 1 && batch[0] == draft("f-1", "off by one");
        let result =
            session.admit_session_review_drafts("engineering.change", "review-1", 5, batch);
        assert_eq!(result.is_ok(), redelivered, "{result:?}");
        assert_eq!(
            session.session_review_store(),
            accepted,
            "refusal and redelivery leave the store and lifecycle unchanged"
        );
    }
    assert!(session
        .admit_session_review_drafts(
            "engineering.change",
            "review-other",
            5,
            vec![draft("f-1", "off by one")]
        )
        .is_err());
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        facts,
        "accepting a finding writes no Goal fact and never step.accepted"
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), calls, "no model call");
    let after = session.goal_definition().unwrap().unwrap();
    assert!(after
        .plan
        .unwrap()
        .steps
        .iter()
        .all(|step| step.status == crate::planning::TaskStatus::Pending));

    session
        .send("Continue the admitted current step.", None)
        .await
        .unwrap();
    let second = slice_runs(root.path()).pop().unwrap();
    assert_ne!(second, first);
    assert!(
        check(&id, revision, plan, "inspect", &first).is_err(),
        "a later slice makes the earlier Run stale"
    );
    check(&id, revision, plan, "inspect", &second).unwrap();
    session.close().await;
    agent.close().await;
}
