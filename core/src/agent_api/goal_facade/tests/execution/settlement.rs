#![cfg(test)]

use super::acceptance::{script, VerifySource};
use super::automatic::ACHIEVED;
use super::*;
use crate::goal_control::GoalTickDecisionV1;
use crate::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};

/// The fixture's read-only check, counting actual executions.
struct Counted(VerifySource, Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Tool for Counted {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn parameters(&self) -> serde_json::Value {
        self.0.parameters()
    }
    fn capabilities(&self, args: &serde_json::Value) -> ToolCapabilities {
        self.0.capabilities(args)
    }
    async fn execute(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.execute(args, ctx).await
    }
}

pub(super) const SOURCE: &str = "independent expected source";

pub(super) fn opts() -> crate::SessionOptions {
    options("goal-run").with_permission_policy(
        crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
    )
}

pub(super) fn facts(root: &std::path::Path) -> Vec<a3s_effect::Fact> {
    crate::fact_control::read_workspace_facts(root, "goal-run").unwrap()
}

pub(super) fn seq_of(facts: &[a3s_effect::Fact], kind: &str) -> u64 {
    facts.iter().find(|fact| fact.kind == kind).unwrap().seq
}

/// One complete original slice: a counted check, its text answer and the judgment.
pub(super) async fn complete_slice(root: &std::path::Path) -> (crate::agent::AgentResult, usize) {
    std::fs::write(root.join("source.txt"), SOURCE).unwrap();
    let (agent, session, client) = fixture(root, true, opts()).await;
    let executed = Arc::new(AtomicUsize::new(0));
    session
        .register_dynamic_tool(Arc::new(Counted(
            VerifySource(SOURCE.into()),
            executed.clone(),
        )))
        .unwrap();
    script(&client, ACHIEVED);
    let result = session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    session.close().await;
    agent.close().await;
    (result, executed.load(Ordering::SeqCst))
}

/// A host that did not run the slice: no Run cache, a fresh model and tool count.
async fn observer(
    root: &std::path::Path,
) -> (Agent, AgentSession, Arc<GoalClient>, Arc<AtomicUsize>) {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(GoalClient::default());
    let session = agent
        .build_session(root.display().to_string(), client.clone(), &opts())
        .unwrap();
    let executed = Arc::new(AtomicUsize::new(0));
    session
        .register_dynamic_tool(Arc::new(Counted(
            VerifySource(SOURCE.into()),
            executed.clone(),
        )))
        .unwrap();
    (agent, session, client, executed)
}

pub(super) fn tick(session: &AgentSession) -> crate::goal_control::GoalControlCommandV1 {
    super::automatic::request(session, "reconcile-wake")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_original_owner_closes_a_completed_slice_and_a_forged_closeout_refuses() {
    let root = tempfile::tempdir().unwrap();
    let (result, executed) = complete_slice(root.path()).await;
    assert_eq!(executed, 1);
    let all = facts(root.path());
    let settled = all.last().unwrap();
    assert_eq!(settled.kind, "a3s.goal.slice.settled");
    let body = &settled.payload["body"];
    assert_eq!(body["outcome"], "completed");
    assert_eq!(
        body["run_sequence"],
        seq_of(&all, "a3s.goal.slice.admitted")
    );
    assert_eq!(body["completion_sequence"], seq_of(&all, "turn.complete"));
    assert_eq!(
        body["step_sequence"],
        seq_of(&all, "a3s.goal.step.accepted")
    );
    assert_eq!(body["usage"]["total_tokens"], result.usage.total_tokens);
    assert!(!crate::goal_control::slice_unsettled(&all));

    let (agent, session, client, executed) = observer(root.path()).await;
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        goal.goal_sequence,
        seq_of(&all, "a3s.goal.step.accepted"),
        "a closeout changes no goal state"
    );
    assert_eq!(session.reconcile_goal_slices().unwrap(), 0);
    assert_eq!(
        session.preview_goal_tick(&tick(&session)).await.unwrap(),
        GoalTickDecisionV1::BudgetExhausted,
        "the closed slice no longer asks for reconciliation"
    );
    assert_eq!(facts(root.path()), all);
    let workspace = goal.binding.workspace.clone();
    let fold = |facts: &[a3s_effect::Fact]| {
        crate::goal_control::definition_from_facts(facts, "goal-run", &workspace, None)
    };
    assert!(fold(&all).is_ok());
    for (field, value) in [
        ("outcome", serde_json::json!("interrupted")),
        ("usage", serde_json::Value::Null),
        ("step_sequence", serde_json::Value::Null),
        ("completion_sequence", serde_json::Value::Null),
        ("run_sequence", serde_json::json!(1)),
        ("run_id", serde_json::json!("foreign")),
    ] {
        let mut forged = all.clone();
        forged.last_mut().unwrap().payload["body"][field] = value;
        assert!(fold(&forged).is_err(), "{field}");
    }
    let mut repeated = all.clone();
    let mut copy = settled.clone();
    copy.seq += 1;
    repeated.push(copy);
    assert!(fold(&repeated).is_err(), "one closeout per Run");
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert_eq!(executed.load(Ordering::SeqCst), 0);
    session.close().await;
    agent.close().await;
}

/// Stop the original owner after each fact of a complete slice, then reopen in a
/// host without its Run. Prepared-but-not-executed and stopped-after-acceptance
/// slices close once with unknown usage; an unknown tool outcome refuses and
/// stays unknown. No model call, tool execution, acceptance or usage repeats.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_owner_at_each_barrier_reconciles_without_replay_or_new_usage() {
    let mut unknown = Vec::new();
    let mut closed = Vec::new();
    let probe = tempfile::tempdir().unwrap();
    complete_slice(probe.path()).await;
    let reference = facts(probe.path());
    let first = seq_of(&reference, "a3s.goal.slice.admitted");
    let accepted = seq_of(&reference, "a3s.goal.step.accepted");
    let completion = seq_of(&reference, "turn.complete");
    assert_eq!(reference.last().unwrap().seq, accepted + 1);
    for stop in first..=accepted {
        let root = tempfile::tempdir().unwrap();
        let (_, executed) = complete_slice(root.path()).await;
        assert_eq!(executed, 1);
        let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
            root.path(),
            "goal-run",
        ))
        .unwrap();
        log.truncate_after(&crate::fact_control::thread_for_session("goal-run"), stop)
            .unwrap();
        drop(log);

        let (agent, session, client, executed) = observer(root.path()).await;
        let before = facts(root.path());
        assert_eq!(before.last().unwrap().seq, stop);
        assert_eq!(
            session.preview_goal_tick(&tick(&session)).await.unwrap(),
            GoalTickDecisionV1::Reconcile,
            "stop after {}",
            before.last().unwrap().kind
        );
        if crate::fact_control::has_unsettled_effects(&before) {
            let error = session.reconcile_goal_slices().unwrap_err().to_string();
            assert!(error.contains("goal.receipt_unsettled"), "{error}");
            assert_eq!(
                facts(root.path()),
                before,
                "an unknown outcome stays unknown"
            );
            assert_eq!(
                session.preview_goal_tick(&tick(&session)).await.unwrap(),
                GoalTickDecisionV1::Reconcile
            );
            unknown.push(before.last().unwrap().kind.clone());
        } else {
            assert_eq!(session.reconcile_goal_slices().unwrap(), 1);
            assert_eq!(session.reconcile_goal_slices().unwrap(), 0, "closes once");
            let after = facts(root.path());
            assert_eq!(&after[..before.len()], before.as_slice());
            assert_eq!(after.len(), before.len() + 1);
            let body = &after.last().unwrap().payload["body"];
            assert_eq!(after.last().unwrap().kind, "a3s.goal.slice.settled");
            assert_eq!(body["outcome"], "interrupted");
            assert!(body["usage"].is_null(), "usage stays unknown");
            assert_eq!(body["run_sequence"], first);
            assert_eq!(
                body["completion_sequence"],
                if stop >= completion {
                    serde_json::json!(completion)
                } else {
                    serde_json::Value::Null
                }
            );
            assert_eq!(
                body["step_sequence"],
                if stop >= accepted {
                    serde_json::json!(accepted)
                } else {
                    serde_json::Value::Null
                }
            );
            assert_eq!(
                after
                    .iter()
                    .filter(|fact| fact.kind == "a3s.goal.step.accepted")
                    .count(),
                usize::from(stop >= accepted),
                "acceptance is neither repeated nor invented"
            );
            let goal = session.goal_definition().unwrap().unwrap();
            let ready = goal.ready_steps().first().unwrap().id.clone();
            assert_eq!(
                ready,
                if stop >= accepted {
                    "verify"
                } else {
                    "inspect"
                }
            );
            assert_eq!(
                session.preview_goal_tick(&tick(&session)).await.unwrap(),
                GoalTickDecisionV1::BudgetExhausted,
                "a closed slice leaves only the host bound"
            );
            closed.push(before.last().unwrap().kind.clone());
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), 0, "no model call");
        assert_eq!(executed.load(Ordering::SeqCst), 0, "no tool replay");
        assert_eq!(
            session.recorded_usage_tokens(),
            (0, 0, 0),
            "no usage callback"
        );
        session.close().await;
        agent.close().await;
    }
    assert!(
        unknown.contains(&"tool.prepared".to_string()),
        "{unknown:?}"
    );
    assert_eq!(closed.first().unwrap(), "a3s.goal.slice.admitted");
    assert_eq!(closed.last().unwrap(), "a3s.goal.step.accepted");
    eprintln!("BARRIERS closed={closed:?} unknown={unknown:?}");
}
