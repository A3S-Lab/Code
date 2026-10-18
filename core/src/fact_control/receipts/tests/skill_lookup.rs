#![cfg(test)]

use super::*;

#[tokio::test]
async fn skill_lookup_remains_available_without_discharging_an_unknown_write() {
    let root = tempfile::tempdir().unwrap();
    let (tools, _) = executor_tools(root.path());
    tools
        .executor
        .register_dynamic_tool(Arc::new(crate::tools::skill::SearchSkillsTool::new(
            Arc::new(crate::skills::SkillRegistry::new()),
        )));
    let uncertain = preparation("earlier-write");
    let unknown =
        ToolExecutionResultV1::unknown(&uncertain, "writer disconnected after dispatch").unwrap();
    let ledger = tools.ledger.as_ref().unwrap();
    ledger
        .lock()
        .unwrap()
        .observe_tool_receipt("bash", &unknown);
    let mut lookup = request("lookup-with-write-debt", json!({"query":"absent-skill"}));
    lookup.call.name = "search_skills".into();
    let prepared = prepare_tools(Arc::clone(&tools), lookup, Default::default())
        .await
        .unwrap();
    let identity = prepared.prepared.clone();
    let result = execute_prepared(tools.clone(), prepared).await;
    result.ensure_prepared(&identity).unwrap();
    assert_eq!(result.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(
        result.effects,
        ToolEffectObservationV1::complete(Vec::new())
    );
    ledger
        .lock()
        .unwrap()
        .observe_tool_receipt("search_skills", &result);
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger.lock().unwrap(), &[], &[], false),
        crate::harness_loop::CompletionGate::Incomplete { message } if message.contains("tool_effect_unresolved")
    ));
}
