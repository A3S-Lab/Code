//! Layer C pin and live escalate twin for typed System-1 decisions.
//!
//! `panicked_pin_does_not_poison_the_next_caller` is hermetic. The other tests
//! are `#[ignore]`: TD-D reads `.a3s/config.acl` and TD-E also needs network.
//!
//! ```bash
//! export A3S_CONFIG_FILE="$(git rev-parse --show-toplevel)/.a3s/config.acl"
//! export A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash
//! cargo test -p a3s-code-core --features apofasi --test typed_decision_layer_c -- --ignored --test-threads=1
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use a3s_code_core::llm::{create_client_with_config, LlmClient, Message, ToolDefinition};
use a3s_code_core::planning::LlmPlanner;
use a3s_code_core::{
    admit_planning_pre_analysis, compose_host_decision, gate_system_one_response, Escalation,
    GatePolicy, TypedDecisionEngine, TypedDecisionService,
};

mod support;
use support::layer_c_model::{
    assert_pinned_layer_c_flash, load_pinned_layer_c_config, ALTERNATE_FLASH_MODEL,
    REQUIRED_DEFAULT_MODEL,
};

const CALL_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_OUTPUT_TOKENS: usize = 256;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_test_model<T>(model: &str, body: impl FnOnce() -> T) -> T {
    // A timed-out generation panics that test. The next proof must still run.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var("A3S_TEST_MODEL").ok();
    set_var("A3S_TEST_MODEL", model);
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => set_var("A3S_TEST_MODEL", &value),
                None => remove_var("A3S_TEST_MODEL"),
            }
        }
    }
    let _restore = Restore(previous);
    body()
}

fn set_var(key: &str, value: &str) {
    // Rust 1.87 marks set_var unsafe; keep the call explicit either way.
    unsafe { std::env::set_var(key, value) }
}

fn remove_var(key: &str) {
    unsafe { std::env::remove_var(key) }
}

fn choice_request() -> a3s_code_core::SystemOneRequest {
    serde_json::from_value(serde_json::json!({
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
    .expect("request")
}

fn complex_request() -> a3s_code_core::SystemOneRequest {
    serde_json::from_value(serde_json::json!({
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
    .expect("complex request")
}

fn billing_triage_request() -> a3s_code_core::SystemOneRequest {
    serde_json::from_value(serde_json::json!({
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
    .expect("billing triage request")
}

fn assert_billing_prompt(prompt: &str) {
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
    assert!(prompt.len() < 4_000);
}

fn complete_once(client: &std::sync::Arc<dyn LlmClient>, prompt: &str) -> Result<String, String> {
    let client = client.clone();
    let prompt = prompt.to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let response = tokio::time::timeout(CALL_TIMEOUT, async {
            client.complete(&[Message::user(&prompt)], None, &[]).await
        })
        .await
        .map_err(|_| "typed-decision escalate generation timed out".to_string())?
        .map_err(|error| format!("generation failed: {error:#}"))?;
        Ok(response.text())
    })
}

fn assert_complex_prompt(prompt: &str) {
    assert!(prompt.contains("Help! My payouts have been failing for 3 days."));
    assert!(prompt.contains("is_urgent"));
    assert!(prompt.contains("department"));
    assert!(prompt.contains("frustration"));
    assert!(prompt.contains("(noul)"));
    assert!(prompt.contains("(choice)"));
    assert!(prompt.contains("(score)"));
    assert!(prompt.len() < 4_000);
}

fn pinned_flash_client() -> Arc<dyn LlmClient> {
    let config = load_pinned_layer_c_config();
    assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
    let (provider, model_id) = ALTERNATE_FLASH_MODEL
        .split_once('/')
        .expect("provider/model");
    let mut llm_config = config.llm_config(provider, model_id).expect("pinned model");
    assert_eq!(llm_config.model, "bailian/deepseek-v4.1-flash");
    llm_config.max_tokens = Some(MAX_OUTPUT_TOKENS);
    llm_config.api_timeout_ms = Some(CALL_TIMEOUT.as_millis() as u64);
    create_client_with_config(llm_config)
}

#[test]
fn panicked_pin_does_not_poison_the_next_caller() {
    let previous = std::env::var("A3S_TEST_MODEL").ok();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_test_model("boyue/bailian/deepseek-v4-flash", || {
            panic!("provider timeout")
        });
    }));
    assert!(caught.is_err());
    assert_eq!(std::env::var("A3S_TEST_MODEL").ok(), previous);
    with_test_model("boyue/deepseek-v4-flash", || {
        assert_eq!(
            std::env::var("A3S_TEST_MODEL").ok().as_deref(),
            Some("boyue/deepseek-v4-flash")
        );
    });
    assert_eq!(std::env::var("A3S_TEST_MODEL").ok(), previous);
}

#[test]
#[ignore = "TD-D reads the monorepo ACL and remaps the bailian Flash pin"]
fn pin_remaps_bailian_flash_alias() {
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let config = load_pinned_layer_c_config();
        assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
        let provider = config.find_provider("boyue").expect("providers \"boyue\"");
        assert!(provider
            .models
            .iter()
            .any(|model| model.id == "bailian/deepseek-v4.1-flash"));
    });
}

#[test]
#[ignore = "TD-D rejects the bailina typo as a declared route"]
fn bailina_typo_is_not_a_declared_route() {
    with_test_model("boyue/bailina/deepseek-v4-flash", || {
        let config = load_pinned_layer_c_config();
        let pinned = config.default_model.as_deref();
        assert_ne!(pinned, Some("boyue/bailina/deepseek-v4-flash"));
        assert_eq!(pinned, Some(REQUIRED_DEFAULT_MODEL));
    });
}

#[test]
#[ignore = "TD-E live escalate uses the remapped bailian Flash route"]
fn live_escalate_generates_once_on_remapped_pin() {
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let config = load_pinned_layer_c_config();
        assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
        let model = config.default_model.clone().expect("pin");
        let (provider, model_id) = model.split_once('/').expect("provider/model");
        let mut llm_config = config
            .llm_config(provider, model_id)
            .expect("llm config for pinned model");
        assert_eq!(llm_config.model, "bailian/deepseek-v4.1-flash");
        llm_config.max_tokens = Some(MAX_OUTPUT_TOKENS);
        llm_config.api_timeout_ms = Some(CALL_TIMEOUT.as_millis() as u64);
        let client = create_client_with_config(llm_config);
        let service = TypedDecisionService::lexical();
        let request = choice_request();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let sealed = gate_system_one_response(
            &TypedDecisionEngine::decide(&service, request.clone()).expect("lexical decision"),
            &policy,
        );
        assert!(
            sealed.escalate,
            "default gate must escalate this lexical decision; do not lower the threshold"
        );

        let mut live_calls = 0u32;
        let live = compose_host_decision(
            &service,
            request,
            &policy,
            &model,
            &mut |seen_model, prompt| {
                live_calls += 1;
                assert_eq!(seen_model, ALTERNATE_FLASH_MODEL);
                assert!(prompt.contains("department"));
                assert!(prompt.contains("Please refund my invoice."));
                assert!(prompt.len() < 4_000);
                let client = client.clone();
                let prompt = prompt.to_owned();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                runtime.block_on(async {
                    let response = tokio::time::timeout(CALL_TIMEOUT, async {
                        client.complete(&[Message::user(&prompt)], None, &[]).await
                    })
                    .await
                    .map_err(|_| "typed-decision escalate generation timed out".to_string())?
                    .map_err(|error| format!("generation failed: {error:#}"))?;
                    Ok(response.text())
                })
            },
        )
        .expect("live composition");
        assert_eq!(live_calls, 1);
        assert_eq!(live.generations, 1);
        assert_eq!(live.decision, sealed);
        assert!(live.receipt.escalate);
        let receipt_json = serde_json::to_string(&live.receipt).expect("receipt");
        assert!(!receipt_json.contains("sk-"));
        assert!(
            !receipt_json.contains("Please refund my invoice."),
            "receipt stored the task state"
        );
        match &live.escalation {
            Escalation::Evidence(evidence) => {
                assert_eq!(evidence.model_id, ALTERNATE_FLASH_MODEL);
                assert!(evidence.payload.contains_key("department"));
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(
                    serde_json::from_value::<a3s_code_core::Answer>(payload).is_err(),
                    "model JSON was parsed as a typed Answer"
                );
            }
            Escalation::Rejected(_) => {}
            other => panic!("expected one admitted or refused generation, got {other:?}"),
        }
    });
}

const PRE_ANALYSIS_TIMEOUT: Duration = Duration::from_secs(180);
const PRE_ANALYSIS_SENTINEL: &str = "src/billing/invoice_store.rs";

struct CountingClient {
    inner: Arc<dyn LlmClient>,
    calls: AtomicUsize,
    system: Mutex<String>,
    text: Mutex<String>,
}

#[async_trait::async_trait]
impl LlmClient for CountingClient {
    fn native_structured_support(&self) -> a3s_code_core::llm::structured::NativeStructuredSupport {
        self.inner.native_structured_support()
    }

    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<a3s_code_core::llm::LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(system) = system {
            *self.system.lock().expect("system") = system.to_string();
        }
        let response = self.inner.complete(messages, system, tools).await?;
        *self.text.lock().expect("text") = response.text();
        Ok(response)
    }

    async fn complete_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &a3s_code_core::llm::structured::StructuredDirective,
    ) -> anyhow::Result<a3s_code_core::llm::LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(system) = system {
            *self.system.lock().expect("system") = system.to_string();
        }
        let response = self
            .inner
            .complete_structured(messages, system, tools, directive)
            .await?;
        *self.text.lock().expect("text") = response.text();
        Ok(response)
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<a3s_code_core::llm::StreamEvent>> {
        self.inner
            .complete_streaming(messages, system, tools, cancel_token)
            .await
    }
}

#[test]
#[ignore = "TD-E live pre-analysis against the pinned Flash model"]
fn live_pre_analysis_is_not_replaced_by_a_typed_decision() {
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let admission = admit_planning_pre_analysis();
        assert!(!admission.eligible);

        let config = load_pinned_layer_c_config();
        assert_pinned_layer_c_flash(&config, "typed-decision pre-analysis");
        assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
        let (provider, model_id) = ALTERNATE_FLASH_MODEL
            .split_once('/')
            .expect("provider/model");
        let llm_config = config.llm_config(provider, model_id).expect("pinned model");
        let inner = create_client_with_config(llm_config);
        let counted = Arc::new(CountingClient {
            inner,
            calls: AtomicUsize::new(0),
            system: Mutex::new(String::new()),
            text: Mutex::new(String::new()),
        });
        let client: Arc<dyn LlmClient> = counted.clone();

        let prompt = format!(
            "Rename the session helper in {PRE_ANALYSIS_SENTINEL} without changing its public signature."
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let analysis = runtime
            .block_on(async {
                tokio::time::timeout(
                    PRE_ANALYSIS_TIMEOUT,
                    LlmPlanner::pre_analyze(&client, &prompt, None),
                )
                .await
            })
            .expect("pre-analysis timed out")
            .expect("pre-analysis generation");

        let calls = counted.calls.load(Ordering::SeqCst);
        assert!(calls >= 1, "pre-analysis made no model call");
        let system = counted.system.lock().expect("system").clone();
        assert!(
            system.contains("optimized_input"),
            "pre-analysis prompt was not used: {system}"
        );
        assert!(
            system.contains("requires_planning"),
            "pre-analysis prompt was not used: {system}"
        );
        assert!(
            !system.contains("needs_plan"),
            "typed-decision question leaked into the pre-analysis prompt"
        );
        assert!(!analysis.optimized_input.trim().is_empty());
        assert!(!analysis.goal.description.trim().is_empty());
        let preserved = analysis.optimized_input.contains(PRE_ANALYSIS_SENTINEL)
            || analysis.goal.description.contains(PRE_ANALYSIS_SENTINEL)
            || analysis
                .execution_plan
                .steps
                .iter()
                .any(|step| step.content.contains(PRE_ANALYSIS_SENTINEL));
        assert!(
            preserved,
            "real pre-analysis dropped the concrete path: optimized_input={:?} goal={:?}",
            analysis.optimized_input, analysis.goal.description
        );
        assert!(
            serde_json::from_str::<a3s_code_core::Answer>(analysis.optimized_input.trim()).is_err(),
            "model prose was parsed as a typed Answer"
        );
        eprintln!(
            "[pre-analysis] calls={calls} requires_planning={} intent={:?}",
            analysis.requires_planning, analysis.intent
        );
    });
}

#[test]
#[ignore = "TD-E live goal achievement against the pinned Flash model"]
fn live_goal_achievement_is_not_replaced_by_a_typed_decision() {
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let admission = a3s_code_core::admit_goal_achievement();
        assert!(!admission.eligible);
        a3s_code_core::enforce_ineligible(admission).expect("achievement stays a generation");

        let config = load_pinned_layer_c_config();
        assert_pinned_layer_c_flash(&config, "typed-decision achievement");
        assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
        let (provider, model_id) = ALTERNATE_FLASH_MODEL
            .split_once('/')
            .expect("provider/model");
        let llm_config = config.llm_config(provider, model_id).expect("pinned model");
        let counted = Arc::new(CountingClient {
            inner: create_client_with_config(llm_config),
            calls: AtomicUsize::new(0),
            system: Mutex::new(String::new()),
            text: Mutex::new(String::new()),
        });
        let client: Arc<dyn LlmClient> = counted.clone();
        let goal = a3s_code_core::planning::AgentGoal::new(
            "Rename the session helper in src/billing/invoice_store.rs",
        )
        .with_criteria(vec![
            "public signature of the helper in src/billing/invoice_store.rs is unchanged"
                .to_string(),
        ]);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime
            .block_on(async {
                tokio::time::timeout(
                    PRE_ANALYSIS_TIMEOUT,
                    LlmPlanner::check_achievement(
                        &client,
                        &goal,
                        "The helper in src/billing/invoice_store.rs was renamed and cargo test was not run.",
                    ),
                )
                .await
            })
            .expect("achievement timed out")
            .expect("achievement generation");
        let calls = counted.calls.load(Ordering::SeqCst);
        assert!(calls >= 1, "goal achievement made no model call");
        let system = counted.system.lock().expect("system").clone();
        assert!(system.contains("remaining_criteria"));
        assert!(system.contains("achieved"));
        let _ = result.progress;
        assert!(
            serde_json::from_str::<a3s_code_core::Answer>(
                counted.text.lock().expect("text").trim()
            )
            .is_err(),
            "model text was parsed as a typed Answer"
        );
        eprintln!(
            "[achievement] calls={calls} achieved={} remaining={}",
            result.achieved,
            result.remaining_criteria.len()
        );
    });
}

/// Published checkpoint, default gate. A confident neural answer must not call
/// the generative model. This test does not lower `GatePolicy` and does not
/// treat the chosen label as correctness.
#[cfg(feature = "apofasi-infer")]
#[test]
#[ignore = "TD-E neural Auto against the published checkpoint at the default gate"]
fn live_neural_auto_skips_the_generative_model() {
    let checkpoint = std::env::var("APOFASI_CHECKPOINT")
        .expect("APOFASI_CHECKPOINT must point at the published english checkpoint");
    let engine = a3s_code_core::load_neural_engine(&checkpoint).expect("load neural engine");
    let policy = GatePolicy::default();
    assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
    let mut calls = 0u32;
    let composition = compose_host_decision(
        &engine,
        choice_request(),
        &policy,
        ALTERNATE_FLASH_MODEL,
        &mut |_, _| {
            calls += 1;
            Err("default-gate Auto must not generate".into())
        },
    )
    .expect("neural composition");
    assert_eq!(calls, 0, "neural decision escalated at the default gate");
    assert_eq!(composition.generations, 0);
    assert!(matches!(composition.escalation, Escalation::NotRequired));
    assert!(!composition.decision.escalate);
    eprintln!(
        "[neural-auto] model={} answer={:?}",
        composition.decision.model,
        composition.decision.answers.first().map(|row| &row.answer)
    );
}

#[test]
#[ignore = "TD-E complex three-kind request escalates once on the remapped Flash pin"]
fn live_complex_multi_kind_escalates_once_on_remapped_pin() {
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let client = pinned_flash_client();
        let service = TypedDecisionService::lexical();
        let request = complex_request();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let sealed = gate_system_one_response(
            &TypedDecisionEngine::decide(&service, request.clone()).expect("lexical decision"),
            &policy,
        );
        assert!(
            sealed.escalate,
            "default gate must escalate this lexical multi-kind decision; do not lower the threshold"
        );
        let mut calls = 0u32;
        let live = compose_host_decision(
            &service,
            request,
            &policy,
            ALTERNATE_FLASH_MODEL,
            &mut |seen_model, prompt| {
                calls += 1;
                assert_eq!(seen_model, ALTERNATE_FLASH_MODEL);
                assert_complex_prompt(prompt);
                complete_once(&client, prompt)
            },
        )
        .expect("live composition");
        assert_eq!(calls, 1);
        assert_eq!(live.generations, 1);
        assert_eq!(live.decision, sealed);
        let receipt_json = serde_json::to_string(&live.receipt).expect("receipt");
        assert!(!receipt_json.contains("sk-"));
        assert!(!receipt_json.contains("payouts have been failing"));
        match &live.escalation {
            Escalation::Evidence(evidence) => {
                assert_eq!(evidence.model_id, ALTERNATE_FLASH_MODEL);
                assert_eq!(evidence.payload.len(), 3);
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(
                    serde_json::from_value::<a3s_code_core::Answer>(payload).is_err(),
                    "model JSON was parsed as a typed Answer"
                );
            }
            Escalation::Rejected(_) => {}
            other => panic!("expected one admitted or refused generation, got {other:?}"),
        }
        eprintln!("[complex-lexical] generations={}", live.generations);
    });
}

/// Same three-kind request on the published checkpoint. Generation count follows
/// the default gate: zero on Auto, one Flash completion on Escalate.
#[cfg(feature = "apofasi-infer")]
#[test]
#[ignore = "TD-E neural complex request at the default gate, then at most one Flash call"]
fn live_neural_complex_generation_matches_the_gate() {
    let checkpoint = std::env::var("APOFASI_CHECKPOINT")
        .expect("APOFASI_CHECKPOINT must point at the published english checkpoint");
    let engine = a3s_code_core::load_neural_engine(&checkpoint).expect("load neural engine");
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let client = pinned_flash_client();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let mut calls = 0u32;
        let composition = compose_host_decision(
            &engine,
            complex_request(),
            &policy,
            ALTERNATE_FLASH_MODEL,
            &mut |seen_model, prompt| {
                calls += 1;
                assert_eq!(calls, 1, "complex escalate generated more than once");
                assert_eq!(seen_model, ALTERNATE_FLASH_MODEL);
                assert_complex_prompt(prompt);
                complete_once(&client, prompt)
            },
        )
        .expect("neural complex composition");
        let expected = u32::from(composition.decision.escalate);
        assert_eq!(calls, expected);
        assert_eq!(composition.generations, expected);
        let receipt_json = serde_json::to_string(&composition.receipt).expect("receipt");
        assert!(!receipt_json.contains("sk-"));
        assert!(!receipt_json.contains("payouts have been failing"));
        match &composition.escalation {
            Escalation::NotRequired => assert_eq!(calls, 0),
            Escalation::Evidence(evidence) => {
                assert_eq!(calls, 1);
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(serde_json::from_value::<a3s_code_core::Answer>(payload).is_err());
            }
            Escalation::Rejected(_) => assert_eq!(calls, 1),
            Escalation::GenerationFailed(error) => {
                panic!("complex generation failed: {error}")
            }
        }
        eprintln!(
            "[complex-neural] escalate={} generations={}",
            composition.decision.escalate, composition.generations
        );
    });
}

#[test]
#[ignore = "TD-E structured billing triage escalates once on the remapped Flash pin"]
fn live_billing_triage_escalates_once_on_remapped_pin() {
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let client = pinned_flash_client();
        let service = TypedDecisionService::lexical();
        let request = billing_triage_request();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let sealed = gate_system_one_response(
            &TypedDecisionEngine::decide(&service, request.clone()).expect("lexical decision"),
            &policy,
        );
        assert!(
            sealed.escalate,
            "default gate must escalate this lexical billing triage; do not lower the threshold"
        );
        let mut calls = 0u32;
        let live = compose_host_decision(
            &service,
            request,
            &policy,
            ALTERNATE_FLASH_MODEL,
            &mut |seen_model, prompt| {
                calls += 1;
                assert_eq!(seen_model, ALTERNATE_FLASH_MODEL);
                assert_billing_prompt(prompt);
                complete_once(&client, prompt)
            },
        )
        .expect("live composition");
        assert_eq!(calls, 1);
        assert_eq!(live.generations, 1);
        assert_eq!(live.decision, sealed);
        let receipt_json = serde_json::to_string(&live.receipt).expect("receipt");
        assert!(!receipt_json.contains("sk-"));
        assert!(!receipt_json.contains("user@acme.com"));
        assert!(!receipt_json.contains("invoice #4411"));
        match &live.escalation {
            Escalation::Evidence(evidence) => {
                assert_eq!(evidence.model_id, ALTERNATE_FLASH_MODEL);
                assert_eq!(evidence.payload.len(), 4);
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(
                    serde_json::from_value::<a3s_code_core::Answer>(payload).is_err(),
                    "model JSON was parsed as a typed Answer"
                );
            }
            Escalation::Rejected(error) => {
                eprintln!("[billing-lexical] host refused evidence: {error}");
            }
            other => panic!("expected one admitted or refused generation, got {other:?}"),
        }
        eprintln!(
            "[billing-lexical] generations={} admitted={}",
            live.generations,
            matches!(live.escalation, Escalation::Evidence(_))
        );
    });
}

/// Structured four-question triage on the published checkpoint. Generation
/// count follows the default gate. Labels are not the oracle.
#[cfg(feature = "apofasi-infer")]
#[test]
#[ignore = "TD-E neural billing triage at the default gate, then at most one Flash call"]
fn live_neural_billing_triage_matches_the_gate() {
    let checkpoint = std::env::var("APOFASI_CHECKPOINT")
        .expect("APOFASI_CHECKPOINT must point at the published english checkpoint");
    let engine = a3s_code_core::load_neural_engine(&checkpoint).expect("load neural engine");
    with_test_model("boyue/bailian/deepseek-v4-flash", || {
        let client = pinned_flash_client();
        let policy = GatePolicy::default();
        assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
        let mut calls = 0u32;
        let composition = compose_host_decision(
            &engine,
            billing_triage_request(),
            &policy,
            ALTERNATE_FLASH_MODEL,
            &mut |seen_model, prompt| {
                calls += 1;
                assert_eq!(calls, 1, "billing triage generated more than once");
                assert_eq!(seen_model, ALTERNATE_FLASH_MODEL);
                assert_billing_prompt(prompt);
                complete_once(&client, prompt)
            },
        )
        .expect("neural billing composition");
        let expected = u32::from(composition.decision.escalate);
        assert_eq!(calls, expected);
        assert_eq!(composition.generations, expected);
        let receipt_json = serde_json::to_string(&composition.receipt).expect("receipt");
        assert!(!receipt_json.contains("sk-"));
        assert!(!receipt_json.contains("user@acme.com"));
        assert!(!receipt_json.contains("invoice #4411"));
        match &composition.escalation {
            Escalation::NotRequired => assert_eq!(calls, 0),
            Escalation::Evidence(evidence) => {
                assert_eq!(calls, 1);
                assert_eq!(evidence.payload.len(), 4);
                let payload = serde_json::Value::Object(evidence.payload.clone());
                assert!(serde_json::from_value::<a3s_code_core::Answer>(payload).is_err());
            }
            Escalation::Rejected(error) => {
                assert_eq!(calls, 1);
                eprintln!("[billing-neural] host refused evidence: {error}");
            }
            Escalation::GenerationFailed(error) => panic!("billing generation failed: {error}"),
        }
        eprintln!(
            "[billing-neural] escalate={} generations={} admitted={}",
            composition.decision.escalate,
            composition.generations,
            matches!(composition.escalation, Escalation::Evidence(_))
        );
    });
}
