//! Host composition over a typed decision: gate, receipt, then optional generation.
//!
//! Apofasi still produces the [`super::Answer`]. A generative model is invoked
//! only when the host gate escalates, and its text is host evidence. It is
//! never parsed into an [`super::Answer`].

use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    build_typed_decision_receipt, gate_system_one_response, validate_system_one_request, Criteria,
    DecisionKind, GatePolicy, GatedDecision, SystemOneRequest, TypedDecisionEngine,
    TypedDecisionReceiptV1, TypedDecisionResult,
};

/// JSON object accepted as host evidence after an escalate generation.
#[derive(Debug, Clone, PartialEq)]
pub struct HostDecisionEvidence {
    /// Model id the host passed into the generator.
    pub model_id: String,
    /// Parsed object. Keys match the request question ids exactly.
    pub payload: serde_json::Map<String, Value>,
}

/// Why host evidence was refused. The typed decision and receipt stay intact.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostEvidenceError {
    /// Generator returned empty or whitespace.
    #[error("host escalate output was empty")]
    Empty,
    /// Generator output was not a JSON object.
    #[error("host escalate output was not a JSON object")]
    NotJson,
    /// JSON object keys were not exactly the question ids.
    #[error("host escalate JSON keys did not match the question ids")]
    KeyMismatch,
}

/// What the host did after the typed decision was already sealed.
#[derive(Debug, Clone, PartialEq)]
pub enum Escalation {
    /// Gate was not escalate. No generator call.
    NotRequired,
    /// One generation admitted as host evidence.
    Evidence(HostDecisionEvidence),
    /// One generation ran, but the text was refused.
    Rejected(HostEvidenceError),
    /// The generator itself failed. Call count is still one.
    GenerationFailed(String),
}

/// Sealed typed decision plus the host's optional generation.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedDecisionComposition {
    /// Apofasi answers and gate labels. Not derived from model text.
    pub decision: GatedDecision,
    /// Digest receipt for the System One request and response.
    pub receipt: TypedDecisionReceiptV1,
    /// Generator invocations performed for this call. Zero or one.
    pub generations: u32,
    /// Host follow-up. Independent of [`Self::decision`].
    pub escalation: Escalation,
}

/// Decide, gate, and receipt first. Call `generate` only when the gate escalates.
///
/// `generate` receives the host model id and the bounded prompt. This function
/// calls it at most once, and only after the typed decision and receipt exist.
pub fn compose_host_decision(
    engine: &dyn TypedDecisionEngine,
    request: SystemOneRequest,
    policy: &GatePolicy,
    model_id: &str,
    generate: &mut dyn FnMut(&str, &str) -> Result<String, String>,
) -> TypedDecisionResult<TypedDecisionComposition> {
    validate_system_one_request(&request)?;
    let response = engine.decide(request.clone())?;
    let decision = gate_system_one_response(&response, policy);
    let receipt = build_typed_decision_receipt(&request, &response, &decision, policy)?;
    if !decision.escalate {
        return Ok(TypedDecisionComposition {
            decision,
            receipt,
            generations: 0,
            escalation: Escalation::NotRequired,
        });
    }
    let question_ids = request.questions.keys().cloned().collect::<BTreeSet<_>>();
    let prompt = escalation_prompt(&request);
    let generated = generate(model_id, &prompt);
    let escalation = match generated {
        Err(error) => Escalation::GenerationFailed(error),
        Ok(text) => match parse_host_evidence(model_id, &question_ids, &text) {
            Ok(evidence) => Escalation::Evidence(evidence),
            Err(error) => Escalation::Rejected(error),
        },
    };
    Ok(TypedDecisionComposition {
        decision,
        receipt,
        generations: 1,
        escalation,
    })
}

/// Admit generator text as host evidence. Refusal does not modify typed answers.
pub fn parse_host_evidence(
    model_id: &str,
    question_ids: &BTreeSet<String>,
    text: &str,
) -> Result<HostDecisionEvidence, HostEvidenceError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(HostEvidenceError::Empty);
    }
    let stripped = strip_fences(trimmed);
    let value = parse_object(stripped).or_else(|_| {
        let start = stripped.find('{');
        let end = stripped.rfind('}');
        match (start, end) {
            (Some(start), Some(end)) if end > start => parse_object(&stripped[start..=end]),
            _ => Err(HostEvidenceError::NotJson),
        }
    })?;
    let Some(payload) = value.as_object().cloned() else {
        return Err(HostEvidenceError::NotJson);
    };
    let keys = payload.keys().cloned().collect::<BTreeSet<_>>();
    if keys != *question_ids {
        return Err(HostEvidenceError::KeyMismatch);
    }
    Ok(HostDecisionEvidence {
        model_id: model_id.to_owned(),
        payload,
    })
}

fn escalation_prompt(request: &SystemOneRequest) -> String {
    let state = request
        .state
        .model_text()
        .unwrap_or_else(|_| request.state.flat_text());
    let keys = request
        .questions
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let mut questions = String::new();
    for (id, question) in &request.questions {
        let instructions = a3s_apofasi::instructions_text(&question.instructions);
        let value = match question.type_ {
            DecisionKind::Choice => "one option key",
            DecisionKind::Score => "a number",
            DecisionKind::Noul => "a boolean",
        };
        questions.push_str(&format!(
            "- {id} ({}): {instructions} Value must be {value}",
            question.type_.as_str()
        ));
        if let Some(criteria) = question.criteria.as_ref() {
            let options = criteria_options(criteria);
            if !options.is_empty() {
                questions.push_str(&format!(". Options: {options}"));
            }
        }
        questions.push('\n');
    }
    format!(
        "State:\n{state}\n\nQuestions:\n{questions}\nReturn only one JSON object. Keys must be exactly: {keys}. No markdown and no extra keys."
    )
}

fn criteria_options(criteria: &Criteria) -> String {
    match criteria {
        Criteria::Choice(options) => options.keys().cloned().collect::<Vec<_>>().join(", "),
        Criteria::Score(levels) => format!("{} levels", levels.len()),
        Criteria::Noul { .. } => "true, false".to_string(),
    }
}

fn strip_fences(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest
        .strip_prefix("json")
        .or_else(|| rest.strip_prefix("JSON"))
        .unwrap_or(rest)
        .trim_start();
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn parse_object(text: &str) -> Result<Value, HostEvidenceError> {
    serde_json::from_str::<Value>(text).map_err(|_| HostEvidenceError::NotJson)
}
