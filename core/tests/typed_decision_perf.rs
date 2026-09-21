//! Performance comparison: typed System-1 versus a generative model on the
//! same decision tasks.
//!
//! `#[ignore]` — one live generation per task against the pinned bailian Flash
//! route. Lexical timing is local and does not use the network.
//!
//! ```bash
//! cargo test -p a3s-code-core --features apofasi --test typed_decision_perf -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use a3s_code_core::llm::{create_client_with_config, Message};
use a3s_code_core::{Answer, GateAction, GatePolicy, SystemOneRequest, TypedDecisionService};

mod support;
use support::layer_c_model::{load_pinned_layer_c_config, ALTERNATE_FLASH_MODEL};

const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
const LEXICAL_ITERS: u32 = 50;

struct Task {
    name: &'static str,
    request: &'static str,
}

const TASKS: &[Task] = &[
    Task {
        name: "refund-route",
        request: r#"{
            "state": "Please refund invoice INV-2041. The charge posted twice on the same card.",
            "questions": {
                "department": {
                    "type": "choice",
                    "instructions": "Which department should handle this request?",
                    "criteria": {
                        "billing": "invoices payments refund refunds billed",
                        "technical": "bugs outages errors",
                        "sales": "pricing contracts"
                    }
                }
            }
        }"#,
    },
    Task {
        name: "checkout-outage",
        request: r#"{
            "state": "Production API has been returning 500 for 20 minutes and customers cannot check out.",
            "questions": {
                "is_urgent": {
                    "type": "noul",
                    "instructions": "Does this need immediate response?",
                    "criteria": {
                        "true": "outage down failing production now",
                        "false": "question documentation later"
                    }
                }
            }
        }"#,
    },
    Task {
        name: "secret-debug-diff",
        request: r#"{
            "state": "The patch removes the session-store lock and writes the API key to /tmp/debug.log.",
            "questions": {
                "needs_review": {
                    "type": "noul",
                    "instructions": "Does a human need to review this change before it lands?",
                    "criteria": {
                        "true": "secret credential delete lock bypass",
                        "false": "comment typo rename"
                    }
                }
            }
        }"#,
    },
    Task {
        name: "search-command",
        request: r#"{
            "state": "rg -n TODO crates/code/core/src",
            "questions": {
                "effect": {
                    "type": "choice",
                    "instructions": "What side effect does this command have?",
                    "criteria": {
                        "read_only": "rg grep find list status",
                        "mutate": "rm delete write edit mv",
                        "network": "curl wget push pull fetch"
                    }
                }
            }
        }"#,
    },
    Task {
        name: "clean-test-command",
        request: r#"{
            "state": "rm -rf target && cargo test",
            "questions": {
                "effect": {
                    "type": "choice",
                    "instructions": "What side effect does this command have?",
                    "criteria": {
                        "read_only": "rg grep find list status",
                        "mutate": "rm delete write edit mv",
                        "network": "curl wget push pull fetch"
                    }
                }
            }
        }"#,
    },
    Task {
        name: "force-push",
        request: r#"{
            "state": "git push --force origin main",
            "questions": {
                "risk": {
                    "type": "score",
                    "instructions": "How risky is this command?",
                    "criteria": ["safe local read", "reversible local write", "remote history rewrite"]
                }
            }
        }"#,
    },
];

struct ModelSample {
    latency: Duration,
    prompt_tokens: usize,
    completion_tokens: usize,
}

fn parse_task(task: &Task) -> SystemOneRequest {
    serde_json::from_str(task.request)
        .unwrap_or_else(|error| panic!("{} is not a SystemOneRequest: {error}", task.name))
}

fn lexical_median(
    service: &TypedDecisionService,
    request: &SystemOneRequest,
) -> (Duration, GateAction, String) {
    let policy = GatePolicy::default();
    for _ in 0..5 {
        service
            .decide_and_gate(request.clone(), &policy)
            .expect("warmup");
    }
    let mut samples = Vec::with_capacity(LEXICAL_ITERS as usize);
    let mut last = None;
    for _ in 0..LEXICAL_ITERS {
        let started = Instant::now();
        let gated = service
            .decide_and_gate(request.clone(), &policy)
            .expect("decide");
        samples.push(started.elapsed());
        last = Some(gated);
    }
    samples.sort();
    let gated = last.expect("sample");
    let row = gated.answers.first().expect("answer");
    (samples[samples.len() / 2], row.gate, signal(&row.answer))
}

fn signal(answer: &Answer) -> String {
    match answer {
        Answer::Choice {
            choice, confidence, ..
        } => format!("{choice} conf={confidence:.2}"),
        Answer::Score {
            score, confidence, ..
        } => format!("score={score:.2} conf={confidence:.2}"),
        Answer::Noul { noul } => format!("noul={noul:.2}"),
    }
}

fn model_prompt(task: &Task) -> String {
    format!(
        "Decide this System One request. Return only a JSON object whose keys are the question ids. Do not explain.\n{}",
        task.request
    )
}

async fn model_once(
    client: &dyn a3s_code_core::llm::LlmClient,
    prompt: &str,
) -> Result<ModelSample, String> {
    let started = Instant::now();
    let response = tokio::time::timeout(MODEL_TIMEOUT, async {
        client.complete(&[Message::user(prompt)], None, &[]).await
    })
    .await
    .map_err(|_| "model call timed out".to_string())?
    .map_err(|error| format!("{error:#}"))?;
    Ok(ModelSample {
        latency: started.elapsed(),
        prompt_tokens: response.usage.prompt_tokens,
        completion_tokens: response.usage.completion_tokens,
    })
}

#[tokio::test]
#[ignore = "live performance comparison against pinned bailian Flash"]
async fn typed_decision_versus_model_on_real_tasks() {
    unsafe { std::env::set_var("A3S_TEST_MODEL", "boyue/bailian/deepseek-v4-flash") };
    let config = load_pinned_layer_c_config();
    assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
    let model = config.default_model.clone().expect("pin");
    let (provider, model_id) = model.split_once('/').expect("provider/model");
    let mut llm_config = config
        .llm_config(provider, model_id)
        .expect("pinned llm config");
    llm_config.max_tokens = Some(128);
    llm_config.api_timeout_ms = Some(MODEL_TIMEOUT.as_millis() as u64);
    let client = create_client_with_config(llm_config);
    let service = TypedDecisionService::lexical();

    eprintln!("model={model}");
    eprintln!(
        "{:<20} {:<10} {:>12} {:>12} {:>8} {:>8}  signal",
        "task", "gate", "lexical", "model", "prompt", "output"
    );

    let mut model_ms = 0.0;
    let mut auto_model_ms = 0.0;
    let mut prompt_tokens = 0usize;
    let mut auto_prompt_tokens = 0usize;
    let mut completion_tokens = 0usize;
    let mut auto_count = 0u32;

    for task in TASKS {
        let request = parse_task(task);
        let (lexical, gate, signal) = lexical_median(&service, &request);
        let sample = model_once(client.as_ref(), &model_prompt(task))
            .await
            .unwrap_or_else(|error| panic!("{} model call failed: {error}", task.name));
        let lexical_us = lexical.as_secs_f64() * 1_000_000.0;
        let sample_ms = sample.latency.as_secs_f64() * 1_000.0;
        model_ms += sample_ms;
        prompt_tokens += sample.prompt_tokens;
        completion_tokens += sample.completion_tokens;
        if gate == GateAction::Auto {
            auto_count += 1;
            auto_model_ms += sample_ms;
            auto_prompt_tokens += sample.prompt_tokens;
        }
        eprintln!(
            "{:<20} {:<10} {lexical_us:>10.1}us {sample_ms:>10.1}ms {:>8} {:>8}  {signal}",
            task.name,
            gate.as_str(),
            sample.prompt_tokens,
            sample.completion_tokens
        );
    }

    eprintln!(
        "tasks={} auto={} model_total={model_ms:.0}ms model_tokens={} (prompt {prompt_tokens} + completion {completion_tokens}) auto_skips={auto_model_ms:.0}ms and {auto_prompt_tokens} prompt tokens",
        TASKS.len(),
        auto_count,
        prompt_tokens + completion_tokens
    );
}

/// Same structured request as `typed_decision_layer_c::billing_triage_request`
/// and the Apofasi binary triage. Do not retune glosses to force Auto.
const BILLING_TRIAGE: &str = r#"{
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
}"#;

struct HostSample {
    latency: Duration,
    prompt_tokens: usize,
    completion_tokens: usize,
    admitted: bool,
    class: &'static str,
}

fn escalation_class(escalation: &a3s_code_core::Escalation) -> &'static str {
    match escalation {
        a3s_code_core::Escalation::Evidence(_) => "evidence",
        a3s_code_core::Escalation::Rejected(a3s_code_core::HostEvidenceError::Empty) => {
            "rejected_empty"
        }
        a3s_code_core::Escalation::Rejected(a3s_code_core::HostEvidenceError::NotJson) => {
            "rejected_not_json"
        }
        a3s_code_core::Escalation::Rejected(a3s_code_core::HostEvidenceError::KeyMismatch) => {
            "rejected_key_mismatch"
        }
        a3s_code_core::Escalation::GenerationFailed(_) => "generation_failed",
        a3s_code_core::Escalation::NotRequired => "not_required",
    }
}

fn percentile(samples: &mut [Duration], index_ratio: usize) -> Duration {
    samples.sort();
    samples[samples.len() / index_ratio]
}

fn pinned_flash_client() -> std::sync::Arc<dyn a3s_code_core::llm::LlmClient> {
    unsafe { std::env::set_var("A3S_TEST_MODEL", "boyue/bailian/deepseek-v4-flash") };
    let config = load_pinned_layer_c_config();
    assert_eq!(config.default_model.as_deref(), Some(ALTERNATE_FLASH_MODEL));
    let (provider, model_id) = ALTERNATE_FLASH_MODEL
        .split_once('/')
        .expect("provider/model");
    let mut llm_config = config
        .llm_config(provider, model_id)
        .expect("pinned llm config");
    llm_config.max_tokens = Some(1024);
    llm_config.api_timeout_ms = Some(MODEL_TIMEOUT.as_millis() as u64);
    create_client_with_config(llm_config)
}

/// Product latency for the complex triage: one host composition per sample.
/// Lexical System-1 at the default 0.7 gate escalates, so each measured sample
/// is exactly one Flash generation. A label in the JSON is not a pass.
#[test]
#[ignore = "complex billing triage Flash percentiles on the remapped pin"]
fn complex_billing_triage_flash_percentiles() {
    let policy = GatePolicy::default();
    assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
    let request: SystemOneRequest =
        serde_json::from_str(BILLING_TRIAGE).expect("billing triage request");
    let service = TypedDecisionService::lexical();
    let sealed = service
        .decide_and_gate(request.clone(), &policy)
        .expect("lexical gate");
    assert!(
        sealed.escalate,
        "lexical triage must escalate at 0.7; do not lower the gate to manufacture Auto"
    );

    let client = pinned_flash_client();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    const WARMUP: usize = 1;
    const MEASURED: usize = 7;
    let mut samples = Vec::with_capacity(MEASURED);
    for index in 0..(WARMUP + MEASURED) {
        let started = Instant::now();
        let mut usage = (0usize, 0usize);
        let composition = a3s_code_core::compose_host_decision(
            &service,
            request.clone(),
            &policy,
            ALTERNATE_FLASH_MODEL,
            &mut |_seen_model, prompt| {
                let response = runtime
                    .block_on(async {
                        tokio::time::timeout(MODEL_TIMEOUT, async {
                            client.complete(&[Message::user(prompt)], None, &[]).await
                        })
                        .await
                    })
                    .map_err(|_| "model call timed out".to_string())?
                    .map_err(|error| format!("{error:#}"))?;
                usage = (
                    response.usage.prompt_tokens,
                    response.usage.completion_tokens,
                );
                let text = response.text();
                if text.trim().is_empty() {
                    Err("model returned no text".to_string())
                } else {
                    Ok(text)
                }
            },
        );
        let sample = match composition {
            Ok(composition) => {
                assert_eq!(
                    composition.generations, 1,
                    "an escalation is one generation"
                );
                HostSample {
                    latency: started.elapsed(),
                    prompt_tokens: usage.0,
                    completion_tokens: usage.1,
                    admitted: matches!(
                        composition.escalation,
                        a3s_code_core::Escalation::Evidence(_)
                    ),
                    class: escalation_class(&composition.escalation),
                }
            }
            Err(error) => panic!("host composition failed: {error}"),
        };
        if index >= WARMUP {
            eprintln!(
                "[billing-flash] sample={} {:.1}ms {} prompt={} completion={}",
                samples.len() + 1,
                sample.latency.as_secs_f64() * 1_000.0,
                sample.class,
                sample.prompt_tokens,
                sample.completion_tokens
            );
            samples.push(sample);
        }
    }
    let mut latencies: Vec<Duration> = samples.iter().map(|sample| sample.latency).collect();
    let p50 = percentile(&mut latencies, 2);
    let prompt: usize = samples.iter().map(|sample| sample.prompt_tokens).sum();
    let completion: usize = samples.iter().map(|sample| sample.completion_tokens).sum();
    let admitted = samples.iter().filter(|sample| sample.admitted).count();
    eprintln!(
        "[billing-flash] samples={} warmup={WARMUP} p50={:.1}ms min={:.1}ms max={:.1}ms generations=1 admitted={admitted}/{} prompt_tokens={} completion_tokens={}",
        samples.len(),
        p50.as_secs_f64() * 1_000.0,
        latencies.first().expect("min").as_secs_f64() * 1_000.0,
        latencies.last().expect("max").as_secs_f64() * 1_000.0,
        samples.len(),
        prompt,
        completion
    );
}

/// Local neural latency for the same triage. Does not call Flash. Gate outcome
/// is recorded; it is not rewritten to force Auto.
#[test]
#[ignore = "complex billing triage neural percentiles; requires APOFASI_CHECKPOINT"]
#[cfg(feature = "apofasi-infer")]
fn complex_billing_triage_neural_percentiles() {
    let policy = GatePolicy::default();
    assert!((policy.min_confidence - 0.7).abs() < f32::EPSILON);
    let request: SystemOneRequest =
        serde_json::from_str(BILLING_TRIAGE).expect("billing triage request");
    let checkpoint = std::env::var("APOFASI_CHECKPOINT")
        .expect("APOFASI_CHECKPOINT must point at the published english checkpoint");
    let engine = a3s_code_core::load_neural_engine(&checkpoint).expect("neural checkpoint");
    const WARMUP: usize = 3;
    const MEASURED: usize = 20;
    let mut samples = Vec::with_capacity(MEASURED);
    let mut escalated = 0usize;
    for index in 0..(WARMUP + MEASURED) {
        let started = Instant::now();
        let response = a3s_code_core::TypedDecisionEngine::decide(&engine, request.clone())
            .expect("neural decide");
        let gated = a3s_code_core::gate_system_one_response(&response, &policy);
        let elapsed = started.elapsed();
        if index >= WARMUP {
            if gated.escalate {
                escalated += 1;
            }
            samples.push(elapsed);
        }
    }
    let p50 = percentile(&mut samples, 2);
    eprintln!(
        "[billing-neural] samples={} warmup={WARMUP} p50={:.1}ms min={:.1}ms max={:.1}ms escalate={escalated}/{}",
        samples.len(),
        p50.as_secs_f64() * 1_000.0,
        samples.first().expect("min").as_secs_f64() * 1_000.0,
        samples.last().expect("max").as_secs_f64() * 1_000.0,
        samples.len()
    );
}
