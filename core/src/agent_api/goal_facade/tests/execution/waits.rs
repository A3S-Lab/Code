#![cfg(test)]

use super::automatic::{decided, now_ms, policy, request, slices, ACHIEVED};
use super::*;
use crate::agent_api::{AgentRunSpawn, GoalTickAdmission};
use crate::goal_control::{
    GoalPlanCommandV1, GoalTickDecisionV1, GoalWaitCommandV1, GoalWaitConditionV1,
    GoalWaitEvidenceV1,
};
use crate::planning::{Complexity, ExecutionPlan, Task, TaskStatus};

fn wait(
    session: &AgentSession,
    operation: &str,
    step: &str,
    condition: Option<GoalWaitConditionV1>,
    evidence: Option<GoalWaitEvidenceV1>,
) -> GoalWaitCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalWaitCommandV1 {
        kind: if condition.is_some() {
            GoalCommandKindV1::DeclareWait
        } else {
            GoalCommandKindV1::ObserveWait
        },
        operation_id: operation.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
        step_id: step.into(),
        condition,
        evidence,
    }
}

/// Replace the fixture plan with two independent steps through the original plan commands.
pub(super) fn independent_plan(session: &AgentSession) {
    let goal = session.goal_definition().unwrap().unwrap();
    let mut plan = ExecutionPlan::new("Independent steps", Complexity::Medium);
    plan.upsert_step(
        Task::new("ask", "Use the owner's chosen port")
            .with_success_criteria("The chosen port is recorded"),
    );
    plan.upsert_step(
        Task::new("inspect", "Inspect the original source")
            .with_success_criteria("Original source is checked"),
    );
    let mut command = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "replan".into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let proposed = session.apply_goal_plan(command.clone()).unwrap();
    command.kind = GoalCommandKindV1::AcceptPlan;
    command.operation_id = "accept-replan".into();
    command.expected_plan_revision = proposed.plan_revision;
    command.expected_goal_sequence = proposed.goal_sequence;
    command.plan = None;
    command.proposal_sequence = proposed.plan_fact_sequence;
    session.apply_goal_plan(command).unwrap();
}

async fn admit(session: &AgentSession, operation: &str) {
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
        .tick_goal(&request(session, operation))
        .await
        .unwrap()
    else {
        panic!("{operation} must admit one Run");
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
}

fn status(session: &AgentSession, step: &str) -> TaskStatus {
    let goal = session.goal_definition().unwrap().unwrap();
    goal.plan
        .unwrap()
        .steps
        .into_iter()
        .find(|s| s.id == step)
        .unwrap()
        .status
}

fn facts(root: &std::path::Path) -> Vec<a3s_effect::Fact> {
    crate::fact_control::read_workspace_facts(root, "goal-run").unwrap()
}

#[tokio::test]
async fn goal_wait_gates_only_its_step_and_an_answer_reaches_the_next_run() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "current source").unwrap();
    let mut permission = crate::permissions::PermissionPolicy::new().allow("verify_source(*)");
    permission.default_decision = crate::permissions::PermissionDecision::Deny;
    let (agent, session, client) = fixture_with(
        root.path(),
        true,
        options("goal-run").with_permission_policy(permission),
        Some(policy(true, 8, 8, now_ms() + 600_000)),
    )
    .await;
    session
        .register_dynamic_tool(Arc::new(acceptance::VerifySource("current source".into())))
        .unwrap();
    independent_plan(&session);
    let question = GoalWaitConditionV1::QuestionAnswer {
        question: "Which port should the service use?".into(),
    };
    assert!(
        session
            .apply_goal_wait(wait(&session, "w-ask", "ask", Some(question), None))
            .unwrap()
            .recorded
    );

    // The waiting step is skipped; the independent step runs and is accepted.
    acceptance::script(&client, ACHIEVED);
    admit(&session, "wake-1").await;
    assert_eq!(slices(root.path())[0].payload["body"]["step_id"], "inspect");
    assert_eq!(status(&session, "inspect"), TaskStatus::Completed);
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    let judged = client.inputs.lock().unwrap()[2].clone();
    assert!(judged.contains("Original source is checked"), "{judged}");
    assert!(!judged.contains("The chosen port is recorded"), "{judged}");

    // Only the open question remains: no model call, no fact, no Run.
    let before = facts(root.path());
    assert_eq!(
        decided(&session, &request(&session, "wake-2")).await,
        GoalTickDecisionV1::NeedsInput
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    assert_eq!(facts(root.path()), before);

    // A stale answer refuses; the current answer is recorded but approves nothing.
    let answer = || {
        Some(GoalWaitEvidenceV1::QuestionAnswer {
            answer: "Use port 8443".into(),
        })
    };
    let mut stale = wait(&session, "o-stale", "ask", None, answer());
    stale.expected_goal_sequence -= 1;
    assert!(session.apply_goal_wait(stale).is_err());
    assert!(
        session
            .apply_goal_wait(wait(&session, "o-ask", "ask", None, answer()))
            .unwrap()
            .recorded
    );
    assert_eq!(status(&session, "ask"), TaskStatus::Pending);
    assert!(session
        .apply_goal_wait(wait(&session, "o-late", "ask", None, answer()))
        .is_err());

    admit(&session, "wake-3").await;
    let admitted = slices(root.path());
    assert_eq!(admitted[1].payload["body"]["step_id"], "ask");
    let inputs = client.inputs.lock().unwrap().clone();
    assert!(
        inputs[3].contains("Use port 8443"),
        "the next Run sees the answer"
    );
    assert!(!inputs[..3]
        .iter()
        .any(|input| input.contains("Use port 8443")));
    assert!(
        inputs[0].contains("current admitted step `inspect`"),
        "{}",
        inputs[0]
    );
    assert!(
        inputs[3].contains("current admitted step `ask`"),
        "{}",
        inputs[3]
    );
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_due_wait_wakes_only_after_a_host_clock_observation() {
    for due in [now_ms() + 600_000, 1] {
        let root = tempfile::tempdir().unwrap();
        let (agent, session, client) = fixture_with(
            root.path(),
            true,
            options("goal-run"),
            Some(policy(true, 8, 8, now_ms() + 600_000)),
        )
        .await;
        let condition = GoalWaitConditionV1::UtcDue {
            due_unix_ms: std::num::NonZeroU64::new(due).unwrap(),
        };
        session
            .apply_goal_wait(wait(&session, "w-due", "inspect", Some(condition), None))
            .unwrap();
        // Elapsed time alone is not a fact: the step waits until the host observes its clock.
        let before = facts(root.path());
        assert_eq!(
            decided(&session, &request(&session, "wake-1")).await,
            GoalTickDecisionV1::Wait
        );
        let observed = session
            .apply_goal_wait(wait(
                &session,
                "o-due",
                "inspect",
                None,
                Some(GoalWaitEvidenceV1::UtcDue {}),
            ))
            .unwrap();
        if due > 1 {
            assert!(!observed.recorded, "not due is quiet");
            assert_eq!(facts(root.path()), before);
            assert_eq!(
                decided(&session, &request(&session, "wake-2")).await,
                GoalTickDecisionV1::Wait
            );
            assert_eq!(client.calls.load(Ordering::SeqCst), 0);
        } else {
            assert!(observed.recorded);
            let clock = observed.definition.waits[0]
                .observed
                .as_ref()
                .unwrap()
                .observed_unix_ms;
            assert!(clock.unwrap() > 1);
            assert_eq!(
                status(&session, "inspect"),
                TaskStatus::Pending,
                "due is not approval"
            );
            assert_eq!(client.calls.load(Ordering::SeqCst), 0);
            admit(&session, "wake-2").await;
            assert_eq!(slices(root.path())[0].payload["body"]["step_id"], "inspect");
            assert!(client.calls.load(Ordering::SeqCst) > 0);
        }
        session.close().await;
        agent.close().await;
    }
}
