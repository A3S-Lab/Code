//! AHP decision-to-hook-result mapping.

use crate::hooks::HookResult;
use a3s_ahp::protocol::{
    ConfirmationDecision, ContextPerceptionDecision, IntentDetectionDecision, MemoryRecallDecision,
    PlanningDecision, RateLimitDecision, ReasoningDecision,
};
use a3s_ahp::{Decision, EventType};
use tracing::debug;

/// Map an AHP decision to the local hook result contract.
pub(super) fn map_decision(
    event_type: EventType,
    decision_payload: serde_json::Value,
) -> HookResult {
    match event_type {
        EventType::ContextPerception => map_context_perception_decision(&decision_payload),
        EventType::MemoryRecall => map_memory_recall_decision(&decision_payload),
        EventType::Planning => map_planning_decision(&decision_payload),
        EventType::Reasoning => map_reasoning_decision(&decision_payload),
        EventType::RateLimit => map_rate_limit_decision(&decision_payload),
        EventType::Confirmation => map_confirmation_decision(&decision_payload),
        EventType::IntentDetection => map_intent_detection_decision(&decision_payload),
        _ => map_generic_decision(decision_payload),
    }
}

fn map_generic_decision(payload: serde_json::Value) -> HookResult {
    match serde_json::from_value::<Decision>(payload) {
        Ok(Decision::Allow {
            modified_payload, ..
        }) => {
            if let Some(modified) = modified_payload {
                HookResult::Continue(Some(modified))
            } else {
                HookResult::Continue(None)
            }
        }
        Ok(Decision::Block { reason, .. }) => HookResult::Block(reason),
        Ok(Decision::Defer {
            retry_after_ms,
            reason,
        }) => {
            if let Some(r) = reason {
                debug!("AHP defer: {}", r);
            }
            HookResult::Retry(retry_after_ms)
        }
        Ok(Decision::Modify {
            modified_payload, ..
        }) => HookResult::Continue(Some(modified_payload)),
        Ok(Decision::Escalate {
            reason,
            escalation_target,
        }) => HookResult::Escalate {
            reason,
            target: escalation_target,
        },
        Err(_) => HookResult::Block("Invalid decision payload".into()),
    }
}

fn map_context_perception_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<ContextPerceptionDecision>(payload.clone()) {
        Ok(ContextPerceptionDecision::Allow {
            injected_context, ..
        }) => {
            let value = serde_json::to_value(injected_context).ok();
            HookResult::Continue(value)
        }
        Ok(ContextPerceptionDecision::Block { reason, .. }) => HookResult::Block(reason),
        Ok(ContextPerceptionDecision::Refine {
            refined_intent,
            refined_target,
            scope_hints,
        }) => HookResult::Continue(Some(serde_json::json!({
            "refined_intent": refined_intent,
            "refined_target": refined_target,
            "scope_hints": scope_hints
        }))),
        Err(_) => map_generic_decision(payload.clone()),
    }
}

fn map_memory_recall_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<MemoryRecallDecision>(payload.clone()) {
        Ok(MemoryRecallDecision::Allow { injected_facts, .. }) => {
            let value = serde_json::to_value(injected_facts).ok();
            HookResult::Continue(value)
        }
        Ok(MemoryRecallDecision::Block { reason, .. }) => HookResult::Block(reason),
        Err(_) => map_generic_decision(payload.clone()),
    }
}

fn map_planning_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<PlanningDecision>(payload.clone()) {
        Ok(PlanningDecision::Allow {
            selected_strategy,
            planning_template,
            ..
        }) => HookResult::Continue(Some(serde_json::json!({
            "selected_strategy": selected_strategy,
            "planning_template": planning_template
        }))),
        Ok(PlanningDecision::Block { reason, .. }) => HookResult::Block(reason),
        Ok(PlanningDecision::Modify {
            modified_task,
            hints,
        }) => HookResult::Continue(Some(serde_json::json!({
            "modified_task": modified_task,
            "hints": hints
        }))),
        Err(_) => map_generic_decision(payload.clone()),
    }
}

fn map_reasoning_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<ReasoningDecision>(payload.clone()) {
        Ok(ReasoningDecision::Allow { hints, .. }) => {
            let value = serde_json::to_value(hints).ok();
            HookResult::Continue(value)
        }
        Ok(ReasoningDecision::Block { reason, .. }) => HookResult::Block(reason),
        Err(_) => map_generic_decision(payload.clone()),
    }
}

fn map_rate_limit_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<RateLimitDecision>(payload.clone()) {
        Ok(RateLimitDecision::Retry { retry_after_ms, .. }) => HookResult::Retry(retry_after_ms),
        Ok(RateLimitDecision::Queue) => HookResult::Skip,
        Ok(RateLimitDecision::Skip { .. }) => HookResult::Skip,
        Err(_) => map_generic_decision(payload.clone()),
    }
}

fn map_confirmation_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<ConfirmationDecision>(payload.clone()) {
        Ok(ConfirmationDecision::Escalate) => HookResult::Escalate {
            reason: "Human confirmation required".into(),
            target: None,
        },
        Ok(ConfirmationDecision::Approve) => HookResult::continue_(),
        Ok(ConfirmationDecision::Reject { reason }) => HookResult::Block(reason),
        Err(_) => map_generic_decision(payload.clone()),
    }
}

fn map_intent_detection_decision(payload: &serde_json::Value) -> HookResult {
    match serde_json::from_value::<IntentDetectionDecision>(payload.clone()) {
        Ok(IntentDetectionDecision::Allow {
            detected_intent,
            confidence,
            target_hints,
        }) => HookResult::Continue(Some(serde_json::json!({
            "detected_intent": detected_intent,
            "confidence": confidence,
            "target_hints": target_hints
        }))),
        Ok(IntentDetectionDecision::Block { reason, .. }) => HookResult::Block(reason),
        Err(_) => map_generic_decision(payload.clone()),
    }
}
