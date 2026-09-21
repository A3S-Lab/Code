//! Optional typed System-1 decisions via A3S Apofasi.
//!
//! This module is a host substrate, not a generative model path and not a
//! Use-projected capability kind. Enable Cargo feature `apofasi` (lexical) or
//! `apofasi-infer` / `apofasi-metal` (neural). Thin `local-code` builds do not
//! link Apofasi.
//!
//! Ownership:
//! - Apofasi owns typed decide + calibrated confidence signals.
//! - Code Core exposes the facade, fail-closed admission, and digest receipts.
//! - When this feature is on, Code judges its own generations by what they
//!   return. Planning pre-analysis and goal achievement are ineligible, and
//!   their call sites refuse to skip them. Code does not invent a keyword
//!   classifier and does not lower [`GatePolicy`]. Host composition at the
//!   default gate makes zero generations on `Auto` and one on `Escalate`.
//!   The escalate prompt includes the task state. Model text never becomes
//!   an [`Answer`].
//! - Hosts still own any extra call sites, question schemas, and
//!   [`GatePolicy`] thresholds (`auto` vs `escalate`).

use std::collections::BTreeMap;
use std::path::Path;

use a3s_apofasi::{any_escalate, gate_response, Client, LexicalEngine};
use serde::{Deserialize, Serialize};

use crate::content_digest::digest_json;

#[path = "typed_decision_policy.rs"]
mod policy;
pub use policy::{
    admit_goal_achievement, admit_planning_pre_analysis, enforce_ineligible, GenerationAdmission,
};

#[path = "typed_decision_host.rs"]
mod host;
pub use host::{
    compose_host_decision, parse_host_evidence, Escalation, HostDecisionEvidence,
    HostEvidenceError, TypedDecisionComposition,
};

pub use a3s_apofasi::{
    analyse, gate_answer, Answer, CheckpointId, Criteria, DecisionKind, Detection, GateAction,
    GatePolicy, Question, RouteDecision, State, SystemOneRequest, SystemOneResponse,
    TemperatureTable, TokenUsage, VERSION as APOFASI_VERSION,
};

#[cfg(feature = "apofasi-infer")]
pub use a3s_apofasi::{CheckpointRegistry, DeviceRequest, NeuralEngine};

/// Wire schema id for [`TypedDecisionReceiptV1`].
pub const TYPED_DECISION_RECEIPT_SCHEMA_V1: &str = "a3s-code/typed-decision-receipt/v1";

const REQUEST_DIGEST_DOMAIN: &str = "a3s.code.typed_decision.request.v1";
const RESPONSE_DIGEST_DOMAIN: &str = "a3s.code.typed_decision.response.v1";

/// Error surface for the optional typed-decision substrate.
#[derive(Debug, thiserror::Error)]
pub enum TypedDecisionError {
    /// Request failed Code admission before Apofasi ran.
    #[error("typed decision request was empty or invalid: {0}")]
    EmptyRequest(&'static str),
    /// Apofasi engine / schema failure.
    #[error(transparent)]
    Apofasi(#[from] a3s_apofasi::Error),
    /// Digest or receipt serialization failure.
    #[error("typed decision receipt serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Call site has no typed replacement, so the generation must still run.
    #[error("typed decision cannot replace this generation: {0}")]
    CannotReplace(&'static str),
    /// Neural path requested without the `apofasi-infer` feature.
    #[error("typed decisions require feature `apofasi-infer` for neural checkpoints")]
    NeuralFeatureDisabled,
}

/// Result alias for typed decisions.
pub type TypedDecisionResult<T> = std::result::Result<T, TypedDecisionError>;

/// Host-injectable System One engine (mirrors EmbeddingProvider ownership).
///
/// Code supplies the lexical default and optional neural loader. Hosts may
/// implement this trait for fixtures or specialized engines without changing
/// the gate / receipt surface.
pub trait TypedDecisionEngine: Send + Sync {
    /// Reported model id for the active engine.
    fn model_id(&self) -> &str;

    /// Run one System One request and return typed answers.
    fn decide(&self, request: SystemOneRequest) -> TypedDecisionResult<SystemOneResponse>;
}

/// One System One answer plus the host gate label derived from policy.
#[derive(Debug, Clone, PartialEq)]
pub struct GatedAnswer {
    /// Question id from the request map.
    pub id: String,
    /// Typed Apofasi answer.
    pub answer: Answer,
    /// Host action after applying [`GatePolicy`].
    pub gate: GateAction,
}

/// Decide + gate outcome for a full request.
#[derive(Debug, Clone, PartialEq)]
pub struct GatedDecision {
    /// Engine model id reported by Apofasi.
    pub model: String,
    /// Per-question gated answers (stable question-id order from the response).
    pub answers: Vec<GatedAnswer>,
    /// True when any answer maps to [`GateAction::Escalate`].
    pub escalate: bool,
    /// Token usage reported by the engine (may be zero on lexical).
    pub usage: TokenUsage,
}

impl GatedDecision {
    /// Lookup one gated answer by question id.
    pub fn get(&self, id: &str) -> Option<&GatedAnswer> {
        self.answers.iter().find(|row| row.id == id)
    }
}

/// Digest-bound receipt for one decide + gate call.
///
/// Hosts may persist or log this without retaining full state text. Digests use
/// domain-separated SHA-256 over canonical JSON of the request and response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedDecisionReceiptV1 {
    /// Wire schema identifier.
    pub schema: String,
    /// Apofasi crate version used for this decision.
    pub apofasi_version: String,
    /// Engine model id from the response.
    pub model: String,
    /// Domain-separated digest of the System One request.
    pub request_digest: String,
    /// Domain-separated digest of the System One response.
    pub response_digest: String,
    /// Host [`GatePolicy::min_confidence`] applied for this call.
    pub min_confidence: f32,
    /// Host [`GatePolicy::min_noul_extremity`] applied for this call.
    pub min_noul_extremity: f32,
    /// Question id → gate action wire label (`auto` / `escalate`).
    pub gates: BTreeMap<String, String>,
    /// True when any answer mapped to escalate.
    pub escalate: bool,
}

/// Fail-closed admission for System One requests owned by Code.
pub fn validate_system_one_request(request: &SystemOneRequest) -> TypedDecisionResult<()> {
    if request.questions.is_empty() {
        return Err(TypedDecisionError::EmptyRequest(
            "questions map must contain at least one question",
        ));
    }
    Ok(())
}

/// Map a System One response through host gate policy.
pub fn gate_system_one_response(
    response: &SystemOneResponse,
    policy: &GatePolicy,
) -> GatedDecision {
    let gates = gate_response(response, policy);
    let answers = response
        .answers
        .iter()
        .map(|(id, answer)| GatedAnswer {
            id: id.clone(),
            answer: answer.clone(),
            gate: gates.get(id).copied().unwrap_or(GateAction::Escalate),
        })
        .collect();
    let escalate = any_escalate(&gates);
    GatedDecision {
        model: response.model.clone(),
        answers,
        escalate,
        usage: response.usage,
    }
}

/// Build a digest-bound receipt for an admitted decide + gate outcome.
pub fn build_typed_decision_receipt(
    request: &SystemOneRequest,
    response: &SystemOneResponse,
    gated: &GatedDecision,
    policy: &GatePolicy,
) -> TypedDecisionResult<TypedDecisionReceiptV1> {
    let request_digest = digest_json(REQUEST_DIGEST_DOMAIN, request)?;
    let response_digest = digest_json(RESPONSE_DIGEST_DOMAIN, response)?;
    let mut gates = BTreeMap::new();
    for row in &gated.answers {
        gates.insert(row.id.clone(), row.gate.as_str().to_owned());
    }
    Ok(TypedDecisionReceiptV1 {
        schema: TYPED_DECISION_RECEIPT_SCHEMA_V1.to_owned(),
        apofasi_version: APOFASI_VERSION.to_owned(),
        model: gated.model.clone(),
        request_digest,
        response_digest,
        min_confidence: policy.min_confidence,
        min_noul_extremity: policy.min_noul_extremity,
        gates,
        escalate: gated.escalate,
    })
}

/// Host-facing typed decision service (System One + gate helpers).
///
/// Default construction uses the size-minimal lexical engine. Neural engines
/// require `apofasi-infer` and an on-disk checkpoint.
#[derive(Debug, Clone)]
pub struct TypedDecisionService {
    client: Client<LexicalEngine>,
}

impl Default for TypedDecisionService {
    fn default() -> Self {
        Self::lexical()
    }
}

impl TypedDecisionService {
    /// Build the lexical System One client (no ML framework).
    pub fn lexical() -> Self {
        Self {
            client: Client::lexical(),
        }
    }

    /// Override softmax temperatures on the lexical engine.
    pub fn with_temperatures(mut self, temperatures: TemperatureTable) -> Self {
        self.client = self.client.with_temperatures(temperatures);
        self
    }

    /// Route without running the engine (script/checkpoint hints).
    pub fn route(
        &self,
        request: &SystemOneRequest,
        model: Option<&str>,
        lang: Option<&str>,
    ) -> TypedDecisionResult<RouteDecision> {
        validate_system_one_request(request)?;
        Ok(self.client.route(request, model, lang)?)
    }

    /// Decide, gate, and return a digest-bound receipt.
    pub fn decide_and_gate_with_receipt(
        &self,
        request: SystemOneRequest,
        policy: &GatePolicy,
    ) -> TypedDecisionResult<(GatedDecision, TypedDecisionReceiptV1)> {
        validate_system_one_request(&request)?;
        let response = self.client.system_one(request.clone())?;
        let gated = gate_system_one_response(&response, policy);
        let receipt = build_typed_decision_receipt(&request, &response, &gated, policy)?;
        Ok((gated, receipt))
    }

    /// Decide, then map each answer through [`GatePolicy`].
    pub fn decide_and_gate(
        &self,
        request: SystemOneRequest,
        policy: &GatePolicy,
    ) -> TypedDecisionResult<GatedDecision> {
        Ok(self.decide_and_gate_with_receipt(request, policy)?.0)
    }
}

impl TypedDecisionEngine for TypedDecisionService {
    fn model_id(&self) -> &str {
        self.client.model_id()
    }

    fn decide(&self, request: SystemOneRequest) -> TypedDecisionResult<SystemOneResponse> {
        validate_system_one_request(&request)?;
        Ok(self.client.system_one(request)?)
    }
}

/// Device preference for neural typed decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NeuralDevice {
    /// Prefer Metal/CUDA when compiled in; otherwise CPU.
    #[default]
    Auto,
    /// Force CPU.
    Cpu,
}

/// Opaque neural engine handle (feature `apofasi-infer`).
#[cfg(feature = "apofasi-infer")]
pub struct NeuralEngineHandle {
    engine: NeuralEngine,
}

#[cfg(feature = "apofasi-infer")]
impl NeuralEngineHandle {
    /// Decide + gate on the neural engine.
    pub fn decide_and_gate(
        &self,
        request: SystemOneRequest,
        policy: &GatePolicy,
    ) -> TypedDecisionResult<GatedDecision> {
        let response = self.decide(request)?;
        Ok(gate_system_one_response(&response, policy))
    }

    /// Decide, gate, and return a digest-bound receipt.
    pub fn decide_and_gate_with_receipt(
        &self,
        request: SystemOneRequest,
        policy: &GatePolicy,
    ) -> TypedDecisionResult<(GatedDecision, TypedDecisionReceiptV1)> {
        validate_system_one_request(&request)?;
        let response = TypedDecisionEngine::decide(self, request.clone())?;
        let gated = gate_system_one_response(&response, policy);
        let receipt = build_typed_decision_receipt(&request, &response, &gated, policy)?;
        Ok((gated, receipt))
    }
}

#[cfg(feature = "apofasi-infer")]
impl TypedDecisionEngine for NeuralEngineHandle {
    fn model_id(&self) -> &str {
        use a3s_apofasi::DecisionEngine;
        self.engine.model_id()
    }

    fn decide(&self, request: SystemOneRequest) -> TypedDecisionResult<SystemOneResponse> {
        use a3s_apofasi::DecisionEngine;
        validate_system_one_request(&request)?;
        Ok(self.engine.decide(&request)?)
    }
}

/// Load a neural System One engine from a checkpoint directory.
///
/// Requires feature `apofasi-infer`. Without that feature this always returns
/// [`TypedDecisionError::NeuralFeatureDisabled`].
pub fn load_neural_engine(checkpoint: impl AsRef<Path>) -> TypedDecisionResult<NeuralEngineHandle> {
    load_neural_engine_with(checkpoint, NeuralDevice::Auto)
}

#[cfg(feature = "apofasi-infer")]
fn load_neural_engine_with(
    checkpoint: impl AsRef<Path>,
    device: NeuralDevice,
) -> TypedDecisionResult<NeuralEngineHandle> {
    let request = match device {
        NeuralDevice::Auto => DeviceRequest::Auto,
        NeuralDevice::Cpu => DeviceRequest::Cpu,
    };
    let engine = NeuralEngine::load_with(checkpoint.as_ref(), request)?;
    Ok(NeuralEngineHandle { engine })
}

#[cfg(not(feature = "apofasi-infer"))]
fn load_neural_engine_with(
    _checkpoint: impl AsRef<Path>,
    _device: NeuralDevice,
) -> TypedDecisionResult<NeuralEngineHandle> {
    Err(TypedDecisionError::NeuralFeatureDisabled)
}

/// Stub type when neural feature is off (keeps call sites compilable behind cfg).
#[cfg(not(feature = "apofasi-infer"))]
#[derive(Debug)]
pub struct NeuralEngineHandle {
    _private: (),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_choice_request() -> SystemOneRequest {
        serde_json::from_value(json!({
            "state": "Please refund my invoice.",
            "questions": {
                "department": {
                    "type": "choice",
                    "instructions": "Which department should handle this request?",
                    "criteria": {
                        "billing": "refunds invoices",
                        "sales": "pricing product interest",
                        "other": "everything else"
                    }
                }
            }
        }))
        .expect("valid SystemOneRequest JSON")
    }

    fn sample_multi_kind_request() -> SystemOneRequest {
        serde_json::from_value(json!({
            "state": "Help! My payouts have been failing for 3 days.",
            "questions": {
                "is_urgent": {
                    "type": "noul",
                    "instructions": "Does this convey urgency?",
                    "criteria": {
                        "true": "Explicitly time-sensitive",
                        "false": "No urgency expressed"
                    }
                },
                "department": {
                    "type": "choice",
                    "instructions": "Which team should handle this?",
                    "criteria": {
                        "billing": "Payments, invoicing, refunds",
                        "technical": "Bugs, outages, integrations",
                        "sales": "Pricing, upgrades, new accounts"
                    }
                },
                "frustration": {
                    "type": "score",
                    "instructions": "How frustrated is the customer?",
                    "criteria": ["Calm", "Frustrated", "Very angry"]
                }
            }
        }))
        .expect("valid multi-kind SystemOneRequest")
    }

    fn billing_triage_request() -> SystemOneRequest {
        serde_json::from_value(json!({
            "state": {
                "from": "user@acme.com",
                "subject": "Duplicate charge on invoice #4411",
                "body": "Hi, we were billed twice for March. Please refund the duplicate today or we will cancel our plan."
            },
            "questions": {
                "department": {
                    "type": "choice",
                    "instructions": "Which department should handle this request?",
                    "criteria": {
                        "billing": "invoices, payments, refunds",
                        "technical": "bugs, outages, system errors",
                        "sales": "pricing, new contracts",
                        "other": "everything else"
                    }
                },
                "urgency": {
                    "type": "score",
                    "instructions": "How urgent is this request?",
                    "criteria": ["not urgent", "soon", "critical deadline or blocking issue"]
                },
                "churn_risk": {
                    "type": "noul",
                    "instructions": "Does the user threaten to cancel or leave?"
                },
                "refund_requested": {
                    "type": "noul",
                    "instructions": "Does the user explicitly request a refund?"
                }
            }
        }))
        .expect("valid billing triage request")
    }

    #[test]
    fn lexical_decide_returns_choice_answer() {
        let service = TypedDecisionService::lexical();
        let response = service.decide(sample_choice_request()).expect("decide");
        assert!(response.answers.contains_key("department"));
        match response.answers.get("department").expect("department") {
            Answer::Choice { choice, .. } => {
                assert!(!choice.is_empty());
            }
            other => panic!("expected choice, got {other:?}"),
        }
    }

    #[test]
    fn lexical_decide_returns_score_and_noul_answers() {
        let service = TypedDecisionService::lexical();
        let response = service
            .decide(sample_multi_kind_request())
            .expect("decide multi");
        assert!(matches!(
            response.answers.get("frustration"),
            Some(Answer::Score { .. })
        ));
        assert!(matches!(
            response.answers.get("is_urgent"),
            Some(Answer::Noul { .. })
        ));
        assert!(matches!(
            response.answers.get("department"),
            Some(Answer::Choice { .. })
        ));
    }

    #[test]
    fn empty_questions_fail_closed_before_engine() {
        let service = TypedDecisionService::lexical();
        let request: SystemOneRequest = serde_json::from_value(json!({
            "state": "unused",
            "questions": {}
        }))
        .expect("empty questions deserialize");
        let err = service.decide(request).expect_err("must reject empty");
        assert!(matches!(err, TypedDecisionError::EmptyRequest(_)));
    }

    #[test]
    fn decide_and_gate_exposes_host_action() {
        let service = TypedDecisionService::lexical();
        let gated = service
            .decide_and_gate(sample_choice_request(), &GatePolicy::default())
            .expect("decide_and_gate");
        assert_eq!(gated.answers.len(), 1);
        let row = gated.get("department").expect("department row");
        assert!(matches!(row.gate, GateAction::Auto | GateAction::Escalate));
        let _ = gated.escalate;
    }

    #[test]
    fn strict_gate_policy_can_force_escalate() {
        let service = TypedDecisionService::lexical();
        let policy = GatePolicy {
            min_confidence: 1.0,
            min_noul_extremity: 1.0,
        };
        let gated = service
            .decide_and_gate(sample_multi_kind_request(), &policy)
            .expect("strict gate");
        assert!(gated.escalate, "impossible thresholds must escalate");
        assert!(gated
            .answers
            .iter()
            .all(|row| row.gate == GateAction::Escalate));
    }

    #[test]
    fn decide_and_gate_with_receipt_is_digest_bound() {
        let service = TypedDecisionService::lexical();
        let request = sample_choice_request();
        let policy = GatePolicy::default();
        let (gated, receipt) = service
            .decide_and_gate_with_receipt(request.clone(), &policy)
            .expect("receipt");
        assert_eq!(receipt.schema, TYPED_DECISION_RECEIPT_SCHEMA_V1);
        assert_eq!(receipt.apofasi_version, APOFASI_VERSION);
        assert_eq!(receipt.model, gated.model);
        assert_eq!(receipt.escalate, gated.escalate);
        assert_eq!(receipt.min_confidence, policy.min_confidence);
        crate::content_digest::validate_digest(&receipt.request_digest)
            .expect("request digest shape");
        crate::content_digest::validate_digest(&receipt.response_digest)
            .expect("response digest shape");
        assert_eq!(
            receipt.request_digest,
            digest_json(REQUEST_DIGEST_DOMAIN, &request).expect("digest request")
        );
        let row = gated.get("department").expect("department");
        assert_eq!(
            receipt.gates.get("department").map(String::as_str),
            Some(row.gate.as_str())
        );
    }

    #[test]
    fn route_returns_decision_for_admitted_request() {
        let service = TypedDecisionService::lexical();
        let decision = service
            .route(&sample_choice_request(), None, None)
            .expect("route");
        let _ = decision;
    }

    #[test]
    fn neural_loader_requires_infer_feature_or_checkpoint() {
        let err = load_neural_engine("/tmp/missing-apofasi-checkpoint")
            .err()
            .expect("must fail without valid checkpoint or feature");
        #[cfg(not(feature = "apofasi-infer"))]
        {
            assert!(matches!(err, TypedDecisionError::NeuralFeatureDisabled));
        }
        #[cfg(feature = "apofasi-infer")]
        {
            assert!(matches!(err, TypedDecisionError::Apofasi(_)));
        }
    }

    #[test]
    fn typed_decision_engine_trait_is_implemented_by_service() {
        let engine: &dyn TypedDecisionEngine = &TypedDecisionService::lexical();
        assert!(!engine.model_id().is_empty());
        let response = engine
            .decide(sample_choice_request())
            .expect("trait decide");
        assert!(response.answers.contains_key("department"));
    }

    const REMAPPED_PIN: &str = "boyue/bailian/deepseek-v4.1-flash";

    struct FixedChoice {
        confidence: f32,
    }

    impl TypedDecisionEngine for FixedChoice {
        fn model_id(&self) -> &str {
            "test-fixed"
        }

        fn decide(&self, request: SystemOneRequest) -> TypedDecisionResult<SystemOneResponse> {
            let mut answers = serde_json::Map::new();
            for id in request.questions.keys() {
                answers.insert(
                    id.clone(),
                    json!({
                        "type": "choice",
                        "choice": "billing",
                        "confidence": self.confidence,
                        "probabilities": { "billing": self.confidence }
                    }),
                );
            }
            Ok(serde_json::from_value(json!({
                "model": "test-fixed",
                "answers": answers,
                "usage": { "input_tokens": 0, "output_tokens": 0 }
            }))
            .expect("fixed response"))
        }
    }

    struct RecordingEngine {
        inner: TypedDecisionService,
        order: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    impl TypedDecisionEngine for RecordingEngine {
        fn model_id(&self) -> &str {
            self.inner.model_id()
        }

        fn decide(&self, request: SystemOneRequest) -> TypedDecisionResult<SystemOneResponse> {
            self.order.lock().expect("order").push("decide");
            self.inner.decide(request)
        }
    }

    #[test]
    fn default_gate_auto_makes_zero_generations() {
        let engine = FixedChoice { confidence: 0.91 };
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let mut calls = 0u32;
        let composition = compose_host_decision(
            &engine,
            sample_choice_request(),
            &policy,
            REMAPPED_PIN,
            &mut |_model, _prompt| {
                calls += 1;
                Ok("{}".to_string())
            },
        )
        .expect("compose");
        assert_eq!(calls, 0);
        assert_eq!(composition.generations, 0);
        assert!(!composition.receipt.escalate);
        assert!(matches!(composition.escalation, Escalation::NotRequired));
        assert!(composition.decision.get("department").is_some());
    }

    #[test]
    fn default_gate_lexical_escalates_once_and_keeps_the_typed_answer() {
        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine = RecordingEngine {
            inner: TypedDecisionService::lexical(),
            order: std::sync::Arc::clone(&order),
        };
        let policy = GatePolicy::default();
        let mut seen_model = String::new();
        let mut seen_prompt = String::new();
        let composition = compose_host_decision(
            &engine,
            sample_choice_request(),
            &policy,
            REMAPPED_PIN,
            &mut |model, prompt| {
                order.lock().expect("order").push("generate");
                seen_model = model.to_owned();
                seen_prompt = prompt.to_owned();
                Ok(r#"{"department":"host-evidence-sentinel"}"#.to_string())
            },
        )
        .expect("compose");
        assert!(
            composition.decision.escalate,
            "lexical refund request must escalate at the default gate"
        );
        assert_eq!(
            order.lock().expect("order").as_slice(),
            ["decide", "generate"]
        );
        assert_eq!(composition.generations, 1);
        assert_eq!(seen_model, REMAPPED_PIN);
        assert!(seen_prompt.contains("Please refund my invoice."));
        assert!(seen_prompt.contains("department"));
        assert!(seen_prompt.contains("choice"));
        assert!(composition.receipt.escalate);
        match &composition.decision.get("department").expect("row").answer {
            Answer::Choice { .. } => {}
            other => panic!("typed answer must stay Choice, got {other:?}"),
        }
        match &composition.escalation {
            Escalation::Evidence(evidence) => {
                assert_eq!(evidence.model_id, REMAPPED_PIN);
                assert_eq!(
                    evidence.payload.get("department").and_then(|v| v.as_str()),
                    Some("host-evidence-sentinel")
                );
            }
            other => panic!("expected evidence, got {other:?}"),
        }
        let receipt_json = serde_json::to_string(&composition.receipt).expect("receipt json");
        assert!(!receipt_json.contains("host-evidence-sentinel"));
        assert!(!receipt_json.contains("Please refund my invoice."));
        assert!(!receipt_json.contains("sk-"));
    }

    #[test]
    fn default_gate_multi_kind_escalates_once_and_keeps_each_answer() {
        let service = TypedDecisionService::lexical();
        let request = sample_multi_kind_request();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let mut calls = 0u32;
        let composition = compose_host_decision(
            &service,
            request,
            &policy,
            REMAPPED_PIN,
            &mut |_model, prompt| {
                calls += 1;
                assert!(prompt.contains("Help! My payouts have been failing for 3 days."));
                assert!(prompt.contains("is_urgent"));
                assert!(prompt.contains("department"));
                assert!(prompt.contains("frustration"));
                assert!(prompt.contains("(noul)"));
                assert!(prompt.contains("(choice)"));
                assert!(prompt.contains("(score)"));
                Ok(
                    r#"{"department":"host-evidence-sentinel","frustration":1,"is_urgent":false}"#
                        .to_string(),
                )
            },
        )
        .expect("compose");
        assert!(
            composition.decision.escalate,
            "lexical multi-kind request must escalate at the default gate; do not lower it"
        );
        assert_eq!(calls, 1);
        assert_eq!(composition.generations, 1);
        assert!(matches!(
            composition.decision.get("is_urgent").expect("noul").answer,
            Answer::Noul { .. }
        ));
        assert!(matches!(
            composition
                .decision
                .get("department")
                .expect("choice")
                .answer,
            Answer::Choice { .. }
        ));
        assert!(matches!(
            composition
                .decision
                .get("frustration")
                .expect("score")
                .answer,
            Answer::Score { .. }
        ));
        match &composition.escalation {
            Escalation::Evidence(evidence) => {
                assert_eq!(
                    evidence
                        .payload
                        .get("department")
                        .and_then(|value| value.as_str()),
                    Some("host-evidence-sentinel")
                );
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(serde_json::from_value::<Answer>(payload).is_err());
            }
            other => panic!("expected evidence, got {other:?}"),
        }
        let receipt_json = serde_json::to_string(&composition.receipt).expect("receipt");
        assert!(!receipt_json.contains("payouts have been failing"));
        assert!(!receipt_json.contains("host-evidence-sentinel"));
    }

    #[test]
    fn default_gate_billing_triage_escalates_once_and_keeps_each_answer() {
        let service = TypedDecisionService::lexical();
        let request = billing_triage_request();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let mut calls = 0u32;
        let composition = compose_host_decision(
            &service,
            request,
            &policy,
            REMAPPED_PIN,
            &mut |_model, prompt| {
                calls += 1;
                assert!(prompt.contains("user@acme.com"));
                assert!(prompt.contains("Duplicate charge on invoice #4411"));
                assert!(prompt.contains("cancel our plan"));
                assert!(prompt.contains("department"));
                assert!(prompt.contains("urgency"));
                assert!(prompt.contains("churn_risk"));
                assert!(prompt.contains("refund_requested"));
                assert!(prompt.contains("(choice)"));
                assert!(prompt.contains("(score)"));
                assert!(prompt.contains("(noul)"));
                Ok(
                    r#"{"churn_risk":false,"department":"host-evidence-sentinel","refund_requested":true,"urgency":1}"#
                        .to_string(),
                )
            },
        )
        .expect("compose");
        assert!(
            composition.decision.escalate,
            "lexical billing triage must escalate at the default gate; do not lower it"
        );
        assert_eq!(calls, 1);
        assert_eq!(composition.generations, 1);
        assert!(matches!(
            composition
                .decision
                .get("department")
                .expect("choice")
                .answer,
            Answer::Choice { .. }
        ));
        assert!(matches!(
            composition.decision.get("urgency").expect("score").answer,
            Answer::Score { .. }
        ));
        assert!(matches!(
            composition.decision.get("churn_risk").expect("noul").answer,
            Answer::Noul { .. }
        ));
        assert!(matches!(
            composition
                .decision
                .get("refund_requested")
                .expect("noul")
                .answer,
            Answer::Noul { .. }
        ));
        match &composition.escalation {
            Escalation::Evidence(evidence) => {
                assert_eq!(evidence.payload.len(), 4);
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(serde_json::from_value::<Answer>(payload).is_err());
            }
            other => panic!("expected evidence, got {other:?}"),
        }
        let receipt_json = serde_json::to_string(&composition.receipt).expect("receipt");
        assert!(!receipt_json.contains("user@acme.com"));
        assert!(!receipt_json.contains("invoice #4411"));
        assert!(!receipt_json.contains("host-evidence-sentinel"));
    }

    #[test]
    fn bad_escalate_payloads_do_not_replace_answer_or_receipt() {
        let service = TypedDecisionService::lexical();
        let request = sample_choice_request();
        let policy = GatePolicy::default();
        let (baseline, baseline_receipt) = service
            .decide_and_gate_with_receipt(request.clone(), &policy)
            .expect("baseline");
        assert!(baseline.escalate);
        for payload in ["", "not-json", r#"{"other":"only-extra"}"#] {
            let composition = compose_host_decision(
                &service,
                request.clone(),
                &policy,
                REMAPPED_PIN,
                &mut |_model, _prompt| Ok(payload.to_string()),
            )
            .expect("compose");
            assert_eq!(composition.generations, 1);
            assert!(matches!(composition.escalation, Escalation::Rejected(_)));
            assert_eq!(composition.decision, baseline);
            assert_eq!(composition.receipt, baseline_receipt);
            assert!(matches!(
                composition.decision.get("department").expect("row").answer,
                Answer::Choice { .. }
            ));
        }
    }

    #[test]
    fn state_change_changes_request_digest_only() {
        let engine = FixedChoice { confidence: 0.91 };
        let mut first = sample_choice_request();
        let mut second = first.clone();
        first.state = State::Text("alpha".into());
        second.state = State::Text("beta".into());
        let left = compose_host_decision(
            &engine,
            first,
            &GatePolicy::default(),
            REMAPPED_PIN,
            &mut |_, _| Ok("{}".into()),
        )
        .expect("left");
        let right = compose_host_decision(
            &engine,
            second,
            &GatePolicy::default(),
            REMAPPED_PIN,
            &mut |_, _| Ok("{}".into()),
        )
        .expect("right");
        assert_eq!(left.generations, 0);
        assert_eq!(right.generations, 0);
        assert_ne!(left.receipt.request_digest, right.receipt.request_digest);
        assert_eq!(left.receipt.schema, right.receipt.schema);
        assert_eq!(left.receipt.schema, TYPED_DECISION_RECEIPT_SCHEMA_V1);
    }
}
