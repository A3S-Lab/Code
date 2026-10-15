#![cfg(test)]

use super::*;
use serde_json::Value;

pub(super) fn facts(workspace: &Path, home: &Path) -> Vec<Value> {
    session_tool_trace(workspace, home)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("type").is_some() && value.get("seq").is_some())
        .collect()
}

pub(super) fn settled_turns(facts: &[Value]) -> usize {
    facts
        .iter()
        .filter(|fact| fact["type"] == "turn.complete")
        .count()
}

pub(super) fn receipt_for<'a>(facts: &'a [Value], tool: &str) -> Vec<&'a Value> {
    facts
        .iter()
        .filter_map(|fact| {
            if fact["type"] != "tool.prepared" || fact["payload"]["prepared"]["tool_name"] != tool {
                return None;
            }
            let prepared = &fact["payload"]["prepared"];
            facts.iter().find_map(|candidate| {
                let receipt = &candidate["payload"]["receipt"];
                (candidate["type"] == "tool.result" && receipt["identity"] == prepared["identity"])
                    .then_some(receipt)
            })
        })
        .collect()
}
