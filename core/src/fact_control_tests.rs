#![cfg(test)]

use super::*;

use std::sync::Mutex;

#[test]
fn prepared_context_does_not_replace_a_later_answer_or_steer() {
    let mut request = CompletionRequest {
        system: vec![],
        tools: vec![],
        summary: String::new(),
        messages: vec!["user\n请选择文档".into()],
    };
    let prepared = PreparedPrompt::capture(&request, "请选择文档\n宿主材料".into()).unwrap();
    request
        .messages
        .extend(["assistant\nA 还是 B？".into(), "user\nB".into()]);
    assert_eq!(prepared.source_index(&request), Some(0));
    request.messages.push("user\n改为 C".into());
    assert_eq!(prepared.source_index(&request), Some(0));
    request.messages = vec!["user\nB".into()];
    assert_eq!(prepared.source_index(&request), None);
    request.messages = vec!["user\n请选择文档".into()];
    request.summary = "原始任务已经被压缩".into();
    assert_eq!(prepared.source_index(&request), None);
}

struct ScriptModel {
    decisions: Mutex<Vec<Result<ModelDecision, ActorError>>>,
    calls: Arc<AtomicUsize>,
    tool_counts: Mutex<Vec<usize>>,
    messages: Mutex<Vec<Vec<String>>>,
}

impl Completion for ScriptModel {
    fn complete(
        &self,
        request: CompletionRequest,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.tool_counts.lock().unwrap().push(request.tools.len());
        self.messages.lock().unwrap().push(request.messages.clone());
        let decision = self
            .decisions
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(Ok(ModelDecision::Text {
                text: "empty".into(),
            }));
        Box::pin(async move { decision })
    }
}

struct ReportingModel {
    inner: ScriptModel,
    reports: Mutex<Vec<Option<u64>>>,
    last: AtomicU64,
}

impl Completion for ReportingModel {
    fn complete(
        &self,
        request: CompletionRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        let reported = self.reports.lock().unwrap().pop().flatten().unwrap_or(0);
        self.last.store(reported, Ordering::SeqCst);
        self.inner.complete(request, cancel)
    }

    fn reported_prompt_tokens(&self) -> Option<u64> {
        let tokens = self.last.load(Ordering::SeqCst);
        (tokens > 0).then_some(tokens)
    }
}

struct ScriptTools {
    calls: Arc<AtomicUsize>,
    fail_first: AtomicUsize,
}

struct IdTools;

impl ToolRunner for IdTools {
    fn run(
        &self,
        call: ToolCall,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<serde_json::Value, ActorError>> {
        let id = call.id;
        Box::pin(async move { Ok(serde_json::json!(format!("output-{id}"))) })
    }
}

impl ToolRunner for ScriptTools {
    fn run(
        &self,
        call: ToolCall,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<serde_json::Value, ActorError>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let fail = self.fail_first.load(Ordering::SeqCst) > 0 && n == 0;
        let name = call.name;
        Box::pin(async move {
            if fail {
                Err(ActorError::Handler {
                    key: name,
                    message: "down".into(),
                })
            } else {
                Ok(serde_json::json!(format!("ran {name}")))
            }
        })
    }
}

fn config(cap: Option<u32>, budget: u32) -> HarnessConfig {
    let config = HarnessConfig::new(
        budget,
        1_000_000,
        16,
        2,
        vec!["system".into()],
        vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
        }],
    )
    .unwrap();
    match cap {
        Some(cap) => config.with_tool_round_cap(cap),
        None => config,
    }
}

fn run_with(
    decisions: Vec<Result<ModelDecision, ActorError>>,
    cap: Option<u32>,
    budget: u32,
    fail_first: bool,
) -> (
    tempfile::TempDir,
    FactRun,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<ScriptModel>,
) {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptModel {
        decisions: Mutex::new(decisions.into_iter().rev().collect()),
        calls: Arc::clone(&calls),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let fact = FactRun::open(
        dir.path(),
        model.clone(),
        Arc::new(ScriptTools {
            calls: Arc::clone(&tool_calls),
            fail_first: AtomicUsize::new(usize::from(fail_first)),
        }),
        config(cap, budget),
        {
            static FACT_RUNS: AtomicUsize = AtomicUsize::new(0);
            format!("run-{}", FACT_RUNS.fetch_add(1, Ordering::Relaxed))
        },
    )
    .unwrap();
    (dir, fact, calls, tool_calls, model)
}

#[tokio::test]
async fn fact_log_second_resume_does_not_call_the_model() {
    let (_dir, fact, calls, tools, _) = run_with(
        vec![Ok(ModelDecision::Text { text: "hi".into() })],
        None,
        4,
        false,
    );
    let settled = fact.user_text("hello").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Done);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let again = fact.resume_limit(8).await.unwrap();
    assert_eq!(again.steps, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(tools.load(Ordering::SeqCst), 0);
    let facts = fact.read_facts().unwrap();
    assert!(facts.iter().any(|fact| fact.kind == "model.turn"));
}

#[tokio::test]
async fn fact_log_non_decision_sets_schema_error_without_a_tool_or_parse_retry() {
    let (_dir, fact, calls, tools, _) = run_with(vec![], None, 4, false);
    fact.append_model_turn(serde_json::json!({"kind": "nope"}))
        .await
        .unwrap();
    let settled = fact.resume_limit(8).await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Done);
    assert!(settled.view.schema_error.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(tools.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fact_log_denial_does_not_run_the_tool() {
    let (_dir, fact, _, tools, _) = run_with(
        vec![Ok(ModelDecision::Tool {
            call: ToolCall {
                id: "t1".into(),
                name: "read".into(),
                args: serde_json::json!({}),
                needs_confirmation: true,
                text: None,
                reasoning: None,
            },

            also: Vec::new(),
        })],
        None,
        4,
        false,
    );
    let parked = fact.user_text("read it").await.unwrap();
    assert_eq!(parked.view.phase, CodingPhase::Confirm);
    assert_eq!(tools.load(Ordering::SeqCst), 0);
    let denied = fact.confirm("t1", false).await.unwrap();
    assert_eq!(denied.view.assistant.as_deref(), Some("denied"));
    assert_eq!(tools.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fact_log_question_options_and_allow_free_text_survive_reopen() {
    let (dir, fact, calls, tools, _) = run_with(
        vec![Ok(ModelDecision::Question {
            question_id: "q1".into(),
            question: "Which?".into(),
            allow_free_text: true,
            options: vec!["left".into(), "right".into()],
        })],
        None,
        4,
        false,
    );
    let parked = fact.user_text("ask").await.unwrap();
    assert_eq!(parked.view.phase, CodingPhase::Question);
    drop(parked);
    let reopened = FactRun::open(
        dir.path(),
        Arc::new(ScriptModel {
            decisions: Mutex::new(vec![]),
            calls: Arc::clone(&calls),
            tool_counts: Mutex::new(Vec::new()),
            messages: Mutex::new(Vec::new()),
        }),
        Arc::new(ScriptTools {
            calls: Arc::new(AtomicUsize::new(0)),
            fail_first: AtomicUsize::new(0),
        }),
        config(None, 4),
        "run-fact-2",
    )
    .unwrap();
    let still = reopened.resume_limit(8).await.unwrap();
    assert_eq!(still.steps, 0);
    let question = still.view.pending_question.expect("question");
    assert!(question.allow_free_text);
    assert_eq!(
        question.options,
        vec!["left".to_string(), "right".to_string()]
    );
    assert_eq!(tools.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fact_log_question_stays_parked_until_the_answer_fact() {
    let (_dir, fact, calls, _, _) = run_with(
        vec![
            Ok(ModelDecision::Question {
                question_id: "q1".into(),
                question: "Which?".into(),
                allow_free_text: true,
                options: vec!["left".into(), "right".into()],
            }),
            Ok(ModelDecision::Text {
                text: "left".into(),
            }),
        ],
        None,
        4,
        false,
    );
    let parked = fact.user_text("ask").await.unwrap();
    assert_eq!(parked.view.phase, CodingPhase::Question);
    let calls_while_parked = calls.load(Ordering::SeqCst);
    let resumed = fact.resume_limit(4).await.unwrap();
    assert_eq!(resumed.steps, 0);
    assert_eq!(resumed.view.phase, CodingPhase::Question);
    assert_eq!(calls.load(Ordering::SeqCst), calls_while_parked);
    assert!(resumed
        .log
        .iter()
        .all(|fact| fact.kind != "question.answered"));
    let answered = fact.answer("left").await.unwrap();
    assert!(answered
        .log
        .iter()
        .any(|fact| fact.kind == "question.answered"));
    assert_eq!(answered.view.phase, CodingPhase::Done);
    assert!(calls.load(Ordering::SeqCst) > calls_while_parked);
}

#[tokio::test]
async fn fact_log_runner_failure_is_unknown_and_never_reexecuted() {
    let (_dir, fact, calls, tools, _) = run_with(
        vec![Ok(ModelDecision::Tool {
            call: ToolCall {
                id: "t1".into(),
                name: "read".into(),
                args: serde_json::json!({}),
                needs_confirmation: false,
                text: None,
                reasoning: None,
            },

            also: Vec::new(),
        })],
        None,
        4,
        true,
    );
    let settled = fact.user_text("go").await.unwrap();
    assert_eq!(tools.load(Ordering::SeqCst), 1);
    let terminal = settled
        .log
        .iter()
        .find(|fact| fact.kind == "tool.result")
        .unwrap();
    let receipt: a3s_effect::ToolExecutionResultV1 =
        serde_json::from_value(terminal.payload["receipt"].clone()).unwrap();
    assert_eq!(receipt.outcome, a3s_effect::ToolExecutionOutcome::Unknown);
    assert!(!super::receipts::fold_debt(&settled.log)
        .unresolved
        .is_empty());
    let model_calls = calls.load(Ordering::SeqCst);
    let again = fact.resume_limit(8).await.unwrap();
    assert_eq!(again.steps, 0);
    assert_eq!(tools.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), model_calls);
}

#[tokio::test]
async fn fact_log_step_limit_does_not_start_a_pending_transition() {
    let (_dir, fact, calls, _, _) = run_with(vec![], None, 4, false);
    let log = fact.open_log().unwrap();
    log.append(THREAD, &[message_fact("m1", "hello")], None)
        .unwrap();
    drop(log);
    let error = match fact.resume_limit(0).await {
        Ok(_) => panic!("a pending transition at limit 0 must not run"),
        Err(error) => error,
    };
    let rendered = format!("{error:#}");
    assert!(rendered.contains("StepLimit"), "{rendered}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn fact_log_budget_does_not_call_the_tool_body() {
    let (_dir, fact, _, tools, _) = run_with(
        vec![
            Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: false,
                    text: None,
                    reasoning: None,
                },

                also: Vec::new(),
            }),
            Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t2".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: false,
                    text: None,
                    reasoning: None,
                },

                also: Vec::new(),
            }),
        ],
        None,
        1,
        false,
    );
    let settled = fact.user_text("two tools").await.unwrap();
    assert_eq!(tools.load(Ordering::SeqCst), 1);
    assert!(settled.log.iter().any(|fact| fact.kind == "budget.denied"));
}

#[test]
fn effort_tool_rounds_are_the_fact_budget() {
    let config = session_harness_config(1_200, 1_000_000, Vec::new()).expect("config");
    assert_eq!(config.budget(), 1_200);
    assert_eq!(config.tool_round_cap(), Some(1_200));
    assert_eq!(config.model_attempts(), 2);
    assert!(config.step_limit() >= 4_800);

    let overridden =
        session_harness_config_with(5, 1_000, Vec::new(), Some(11), Some(4)).expect("limits");
    assert_eq!(overridden.budget(), 5);
    assert_eq!(overridden.tool_round_cap(), Some(5));
    assert_eq!(overridden.step_limit(), 11);
    assert_eq!(overridden.model_attempts(), 4);
    assert!(session_harness_config_with(5, 1_000, Vec::new(), None, Some(0)).is_err());
    assert!(session_harness_config_with(5, 1_000, Vec::new(), Some(0), None).is_err());
}

#[tokio::test]
async fn effort_tool_budget_runs_every_round_before_deny() {
    let dir = tempfile::tempdir().unwrap();
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptModel {
        decisions: Mutex::new(
            (1..=4)
                .rev()
                .map(|index| {
                    Ok(ModelDecision::Tool {
                        call: ToolCall {
                            id: format!("t{index}"),
                            name: "read".into(),
                            args: serde_json::json!({}),
                            needs_confirmation: false,
                            text: None,
                            reasoning: None,
                        },

                        also: Vec::new(),
                    })
                })
                .collect(),
        ),
        calls: Arc::new(AtomicUsize::new(0)),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let config = session_harness_config(
        3,
        1_000_000,
        vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
        }],
    )
    .unwrap();
    let fact = FactRun::open(
        dir.path(),
        model,
        Arc::new(ScriptTools {
            calls: Arc::clone(&tool_calls),
            fail_first: AtomicUsize::new(0),
        }),
        config,
        "effort-budget",
    )
    .unwrap();
    let settled = fact.user_text("four tools").await.unwrap();
    assert_eq!(tool_calls.load(Ordering::SeqCst), 3);
    assert!(settled.log.iter().any(|fact| fact.kind == "budget.denied"));
}

#[tokio::test]
async fn effort_ceiling_asks_for_a_final_answer_before_denying() {
    let dir = tempfile::tempdir().unwrap();
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ScriptModel {
        decisions: Mutex::new(vec![
            Ok(ModelDecision::Text {
                text: "done".into(),
            }),
            Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t2".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: false,
                    text: None,
                    reasoning: None,
                },

                also: Vec::new(),
            }),
            Ok(ModelDecision::Tool {
                call: ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    args: serde_json::json!({}),
                    needs_confirmation: false,
                    text: None,
                    reasoning: None,
                },

                also: Vec::new(),
            }),
        ]),
        calls: Arc::new(AtomicUsize::new(0)),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let config = session_harness_config(
        2,
        1_000_000,
        vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
        }],
    )
    .unwrap();
    let fact = FactRun::open(
        dir.path(),
        model.clone(),
        Arc::new(ScriptTools {
            calls: Arc::clone(&tool_calls),
            fail_first: AtomicUsize::new(0),
        }),
        config,
        "effort-wrap-up",
    )
    .unwrap();
    let settled = fact.user_text("finish").await.unwrap();
    assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
    assert_eq!(settled.view.assistant.as_deref(), Some("done"));
    assert!(settled.log.iter().all(|fact| fact.kind != "budget.denied"));
    assert_eq!(model.tool_counts.lock().unwrap().last().copied(), Some(0));
}

#[tokio::test]
async fn auto_compact_keeps_the_latest_tool_pair_and_continues() {
    let user = "user\nhi";
    let call = format!("assistant\ntool read\n{}", serde_json::json!({}));
    let first = "tool\noutput-t1";
    let after_first = user.len() + call.len() + first.len();
    let dir = tempfile::tempdir().unwrap();
    let model = Arc::new(ScriptModel {
        decisions: Mutex::new(
            vec![
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t1".into(),
                        name: "read".into(),
                        args: serde_json::json!({}),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },

                    also: Vec::new(),
                }),
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t2".into(),
                        name: "read".into(),
                        args: serde_json::json!({}),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },

                    also: Vec::new(),
                }),
                Ok(ModelDecision::Text {
                    text: "done".into(),
                }),
            ]
            .into_iter()
            .rev()
            .collect(),
        ),
        calls: Arc::new(AtomicUsize::new(0)),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let config = session_harness_config(
        8,
        after_first + 1,
        vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
        }],
    )
    .unwrap();
    let fact = FactRun::open(
        dir.path(),
        model.clone(),
        Arc::new(IdTools),
        config,
        "auto-compact",
    )
    .unwrap();
    let settled = fact.user_text("hi").await.unwrap();
    assert_eq!(settled.view.assistant.as_deref(), Some("done"));
    assert!(settled.view.phase != CodingPhase::Compact);
    let summary = settled
        .log
        .iter()
        .rev()
        .find(|fact| fact.kind == "compaction.done")
        .and_then(|fact| fact.payload.get("summary"))
        .and_then(|value| value.as_str())
        .expect("summary");
    assert!(summary.contains("output-t1"));
    assert!(!summary.contains("output-t2"));
    let last = model
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("final infer");
    assert!(last.iter().any(|line| line.contains("output-t2")));
    assert!(last.iter().all(|line| !line.contains("output-t1")));
    assert!(last.iter().any(|line| line.starts_with("assistant\ntool ")));
}

#[tokio::test]
async fn auto_compact_triggers_on_provider_tokens_and_keeps_a_recent_tail() {
    token_compact_keeps_a_recent_tail(None).await;
}

#[tokio::test]
async fn explicit_stock_compose_keeps_token_compaction() {
    let compose = crate::meta_harness::HarnessComposeOptions::compose(
        ["system", "tools", "budget", "compact", "infer"]
            .map(String::from)
            .to_vec(),
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    token_compact_keeps_a_recent_tail(Some(&compose)).await;
}

async fn token_compact_keeps_a_recent_tail(
    compose: Option<&crate::meta_harness::HarnessComposeOptions>,
) {
    let dir = tempfile::tempdir().unwrap();
    let model = Arc::new(ReportingModel {
        inner: ScriptModel {
            decisions: Mutex::new(
                vec![
                    Ok(ModelDecision::Tool {
                        call: ToolCall {
                            id: "t1".into(),
                            name: "read".into(),
                            args: serde_json::json!({}),
                            needs_confirmation: false,
                            text: None,
                            reasoning: None,
                        },

                        also: Vec::new(),
                    }),
                    Ok(ModelDecision::Tool {
                        call: ToolCall {
                            id: "t2".into(),
                            name: "read".into(),
                            args: serde_json::json!({ "n": 2 }),
                            needs_confirmation: false,
                            text: None,
                            reasoning: None,
                        },

                        also: Vec::new(),
                    }),
                    Ok(ModelDecision::Tool {
                        call: ToolCall {
                            id: "t3".into(),
                            name: "read".into(),
                            args: serde_json::json!({ "n": 3 }),
                            needs_confirmation: false,
                            text: None,
                            reasoning: None,
                        },

                        also: Vec::new(),
                    }),
                    Ok(ModelDecision::Text {
                        text: "done".into(),
                    }),
                ]
                .into_iter()
                .rev()
                .collect(),
            ),
            calls: Arc::new(AtomicUsize::new(0)),
            tool_counts: Mutex::new(Vec::new()),
            messages: Mutex::new(Vec::new()),
        },
        reports: Mutex::new(
            vec![Some(10), Some(10), Some(100), Some(100)]
                .into_iter()
                .rev()
                .collect(),
        ),
        last: AtomicU64::new(0),
    });
    let call = format!("assistant\ntool read\n{}", serde_json::json!({"n": 2}));
    let second = "tool\noutput-t2";
    let third_call = format!("assistant\ntool read\n{}", serde_json::json!({"n": 3}));
    let third = "tool\noutput-t3";
    let keep = call.len().div_ceil(4)
        + second.len().div_ceil(4)
        + third_call.len().div_ceil(4)
        + third.len().div_ceil(4)
        + 8;
    let config = session_harness_config(
        8,
        1_000_000,
        vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
        }],
    )
    .unwrap()
    .with_token_compact(100, keep);
    let fact = FactRun::open_composed(
        dir.path(),
        model.clone(),
        Arc::new(IdTools),
        config,
        "token-compact",
        compose,
    )
    .unwrap();
    let settled = fact.user_text("hi").await.unwrap();
    assert_eq!(settled.view.assistant.as_deref(), Some("done"));
    let summary = settled
        .log
        .iter()
        .rev()
        .find(|fact| fact.kind == "compaction.done")
        .and_then(|fact| fact.payload.get("summary"))
        .and_then(|value| value.as_str())
        .expect("summary");
    assert!(summary.contains("output-t1"));
    assert!(!summary.contains("output-t3"));
    let last = model
        .inner
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("final infer");
    assert!(last.iter().any(|line| line.contains("output-t2")));
    assert!(last.iter().any(|line| line.contains("output-t3")));
    assert!(last.iter().all(|line| !line.contains("output-t1")));
}

#[tokio::test]
async fn repeated_tool_call_is_a_nudge_and_the_turn_continues() {
    let dir = tempfile::tempdir().unwrap();
    let args = serde_json::json!({});
    let model = Arc::new(ScriptModel {
        decisions: Mutex::new(
            vec![
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t1".into(),
                        name: "read".into(),
                        args: args.clone(),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },

                    also: Vec::new(),
                }),
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t2".into(),
                        name: "read".into(),
                        args: args.clone(),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },

                    also: Vec::new(),
                }),
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t3".into(),
                        name: "read".into(),
                        args,
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },

                    also: Vec::new(),
                }),
                Ok(ModelDecision::Text {
                    text: "changed".into(),
                }),
            ]
            .into_iter()
            .rev()
            .collect(),
        ),
        calls: Arc::new(AtomicUsize::new(0)),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let config = session_harness_config(
        8,
        1_000_000,
        vec![ToolSpec {
            name: "read".into(),
            description: "Read".into(),
        }],
    )
    .unwrap()
    .with_duplicate_threshold(2);
    let fact = FactRun::open(
        dir.path(),
        model.clone(),
        Arc::new(ScriptTools {
            calls: Arc::clone(&calls),
            fail_first: AtomicUsize::new(0),
        }),
        config,
        "duplicate-nudge",
    )
    .unwrap();
    let settled = fact.user_text("hi").await.unwrap();
    assert_eq!(settled.view.assistant.as_deref(), Some("changed"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let last = model
        .messages
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("final");
    assert!(last
        .iter()
        .any(|line| { line.starts_with("tool-error\n") && line.contains("Change your approach") }));
    let nudge = settled
        .log
        .iter()
        .find(|fact| {
            fact.kind == "tool.result"
                && fact
                    .payload
                    .get("output")
                    .and_then(|value| value.as_str())
                    .is_some_and(|output| output.contains("Change your approach"))
        })
        .expect("nudge result");
    assert_eq!(
        nudge.payload.get("ok").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        nudge
            .payload
            .get("countRun")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
}

#[tokio::test]
async fn configured_duplicate_guard_refuses_the_second_identical_call() {
    let dir = tempfile::tempdir().unwrap();
    let same = serde_json::json!({"command": "command -v a3s"});
    let other = serde_json::json!({"command": "pwd"});
    let model = Arc::new(ScriptModel {
        decisions: Mutex::new(
            vec![
                Ok(ModelDecision::Tool {
                    call: ToolCall {
                        id: "t1".into(),
                        name: "bash".into(),
                        args: same.clone(),
                        needs_confirmation: false,
                        text: None,
                        reasoning: None,
                    },
                    also: vec![
                        ToolCall {
                            id: "t2".into(),
                            name: "bash".into(),
                            args: same,
                            needs_confirmation: false,
                            text: None,
                            reasoning: None,
                        },
                        ToolCall {
                            id: "t3".into(),
                            name: "bash".into(),
                            args: other,
                            needs_confirmation: false,
                            text: None,
                            reasoning: None,
                        },
                    ],
                }),
                Ok(ModelDecision::Text {
                    text: "done".into(),
                }),
            ]
            .into_iter()
            .rev()
            .collect(),
        ),
        calls: Arc::new(AtomicUsize::new(0)),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let config = session_harness_config(
        8,
        1_000_000,
        vec![ToolSpec {
            name: "bash".into(),
            description: "Bash".into(),
        }],
    )
    .unwrap()
    .with_duplicate_threshold(2);
    let fact = FactRun::open(
        dir.path(),
        model,
        Arc::new(ScriptTools {
            calls: Arc::clone(&calls),
            fail_first: AtomicUsize::new(0),
        }),
        config,
        "same-response",
    )
    .unwrap();
    let settled = fact.user_text("hi").await.unwrap();
    assert_eq!(settled.view.assistant.as_deref(), Some("done"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let nudged = settled
        .log
        .iter()
        .filter(|fact| {
            fact.kind == "tool.result"
                && fact
                    .payload
                    .get("output")
                    .and_then(|value| value.as_str())
                    .is_some_and(|output| output.contains("identical arguments"))
        })
        .count();
    assert_eq!(nudged, 1);
}

#[test]
fn read_only_evidence_stops_at_the_configured_limit() {
    let ledger = crate::harness_loop::MutationLedger::default();
    let mut messages = vec!["user\ninstall the skill".to_string()];
    for index in 0..crate::agent::READ_ONLY_EVIDENCE_LIMIT - 1 {
        messages.push(format!("tool\nresult-{index}"));
    }
    assert!(!read_only_evidence_exhausted(&ledger, &messages));
    messages.push("tool-error\nresult-7".into());
    assert!(read_only_evidence_exhausted(&ledger, &messages));
    messages.push("assistant\ntool bash\n{}".into());
    assert!(read_only_evidence_exhausted(&ledger, &messages));
    let mut mutated = crate::harness_loop::MutationLedger::default();
    mutated.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path": "notes.md", "content_digest": "abc"})),
    );
    assert!(!read_only_evidence_exhausted(&mutated, &messages));
    messages.push("user\nrun the command".into());
    assert!(!read_only_evidence_exhausted(&ledger, &messages));
}

fn tool_response(calls: Vec<(&str, &str, serde_json::Value)>) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: calls
                .into_iter()
                .map(|(id, name, args)| crate::llm::ContentBlock::ToolUse {
                    id: id.into(),
                    name: name.into(),
                    input: args,
                })
                .collect(),
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        },
        usage: crate::llm::TokenUsage::default(),
        stop_reason: Some("tool_calls".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[tokio::test]
async fn one_response_keeps_every_non_question_tool_call() {
    let policy = PermissionPolicy::default();
    let both = decision_from_response(
        &tool_response(vec![
            ("a", "read_file", serde_json::json!({"path": "a.rs"})),
            ("b", "grep", serde_json::json!({"pattern": "fn"})),
        ]),
        &policy,
        None,
    )
    .await;
    match both {
        ModelDecision::Tool { call, also } => {
            assert_eq!(call.id, "a");
            assert_eq!(also.len(), 1);
            assert_eq!(also[0].id, "b");
        }
        other => panic!("expected both calls, got {other:?}"),
    }
    let mixed = decision_from_response(
        &tool_response(vec![
            ("q", "ask_user", serde_json::json!({"question": "which?"})),
            ("g", "grep", serde_json::json!({"pattern": "fn"})),
        ]),
        &policy,
        None,
    )
    .await;
    match mixed {
        ModelDecision::Tool { call, also } => {
            assert_eq!(call.name, "grep");
            assert!(also.is_empty());
        }
        other => panic!("expected the non-question call, got {other:?}"),
    }
    let only_question = decision_from_response(
        &tool_response(vec![(
            "q",
            "ask_user",
            serde_json::json!({"question": "which?"}),
        )]),
        &policy,
        None,
    )
    .await;
    assert!(matches!(only_question, ModelDecision::Question { .. }));
}

#[test]
fn parallel_batches_are_reads_or_distinct_writes() {
    let read = |id: &str, path: &str| ToolCall {
        id: id.into(),
        name: "read".into(),
        args: serde_json::json!({ "file_path": path }),
        needs_confirmation: false,
        text: None,
        reasoning: None,
    };
    let write = |id: &str, path: &str| ToolCall {
        id: id.into(),
        name: "write".into(),
        args: serde_json::json!({ "file_path": path }),
        needs_confirmation: false,
        text: None,
        reasoning: None,
    };
    assert!(!batch_can_run_in_parallel(&[read("a", "a.rs")]));
    assert!(batch_can_run_in_parallel(&[
        read("a", "a.rs"),
        read("b", "b.rs")
    ]));
    assert!(batch_can_run_in_parallel(&[
        write("a", "a.rs"),
        write("b", "b.rs")
    ]));
    assert!(!batch_can_run_in_parallel(&[
        write("a", "a.rs"),
        write("b", "a.rs")
    ]));
    assert!(!batch_can_run_in_parallel(&[
        read("a", "a.rs"),
        write("b", "b.rs")
    ]));
    let bash = ToolCall {
        id: "sh".into(),
        name: "bash".into(),
        args: serde_json::json!({}),
        needs_confirmation: false,
        text: None,
        reasoning: None,
    };
    assert!(!batch_can_run_in_parallel(&[bash.clone(), bash]));
}

/// The parallel-safe sets must track the REGISTERED tool names. The
/// original set drifted to registry-absent verbs (`read_file`, `list_dir`,
/// `write_file`, `edit_file`), which silently forced every real batch
/// sequential. This test pins the registered spellings.
#[test]
fn parallel_safe_names_match_the_registered_tool_registry() {
    let registered_read_tools = ["read", "ls", "grep", "glob", "search", "hybrid"];
    for name in registered_read_tools {
        assert!(
            is_parallel_safe_read(name),
            "{name} is registered but missing from the parallel-safe read set"
        );
    }
    for name in ["write", "edit"] {
        assert!(
            is_parallel_safe_write(name),
            "{name} is registered but missing from the parallel-safe write set"
        );
    }
    for drifted in ["read_file", "list_dir", "write_file", "edit_file"] {
        assert!(
            !is_parallel_safe_read(drifted) && !is_parallel_safe_write(drifted),
            "{drifted} is not a registered tool name; the sets must not drift back to it"
        );
    }
    // A write call without a file target stays sequential.
    let unkeyed = ToolCall {
        id: "w".into(),
        name: "write".into(),
        args: serde_json::json!({ "content": "x" }),
        needs_confirmation: false,
        text: None,
        reasoning: None,
    };
    let keyed = ToolCall {
        id: "w2".into(),
        name: "write".into(),
        args: serde_json::json!({ "file_path": "b.rs", "content": "x" }),
        needs_confirmation: false,
        text: None,
        reasoning: None,
    };
    assert!(!batch_can_run_in_parallel(&[unkeyed, keyed]));
}

struct FailingSummaryClient;

#[async_trait::async_trait]
impl LlmClient for FailingSummaryClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        anyhow::bail!("summary down")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("unused")
    }
}

#[tokio::test]
async fn a_failed_model_summary_still_shrinks_the_prefix() {
    let client = Arc::new(FailingSummaryClient);
    let huge = "y".repeat(40_000);
    let summary = compact_handoff(
        client.as_ref(),
        &[format!("user\n{huge}"), "user\nkeep".into()],
        128_000,
        None,
    )
    .await;
    assert!(summary.contains("[earlier transcript compacted]"));
    assert!(!summary.contains(&huge));
}

#[tokio::test]
async fn a_failed_model_summary_keeps_a_normal_task_instruction() {
    let client = Arc::new(FailingSummaryClient);
    let instruction = format!(
        "Ship the release without forbidden output. {}",
        "R".repeat(2_500)
    );
    assert!(instruction.len() > 2_000);
    assert!(instruction.len() < 16_384);
    let summary = compact_handoff(
        client.as_ref(),
        &[format!("user\n{instruction}")],
        128_000,
        None,
    )
    .await;
    assert!(
        summary.contains(&format!("## Goal\n{instruction}")),
        "a task instruction above 2000 characters must stay when the summary model fails"
    );
}

#[tokio::test]
async fn fact_log_tool_round_cap_uses_an_empty_tool_list() {
    let (_dir, fact, calls, _, model) = run_with(
        vec![Ok(ModelDecision::Text { text: "ok".into() })],
        Some(0),
        4,
        false,
    );
    fact.user_text("hello").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.tool_counts.lock().unwrap().as_slice(), &[0]);
    assert!(model
        .messages
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .all(|line| !line.contains("TOOL_BUDGET_FINALIZATION")));
}

#[tokio::test]
async fn fact_log_steer_is_a_user_message_fact() {
    let (_dir, fact, calls, _, _) = run_with(
        vec![
            Ok(ModelDecision::Text {
                text: "first".into(),
            }),
            Ok(ModelDecision::Text {
                text: "steered".into(),
            }),
        ],
        None,
        4,
        false,
    );
    fact.user_text("hello").await.unwrap();
    let steered = fact.steer("turn left").await.unwrap();
    assert_eq!(steered.view.assistant.as_deref(), Some("steered"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let facts = fact.read_facts().unwrap();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == "user.message")
            .count(),
        2
    );
}

struct HangClient;

#[async_trait::async_trait]
impl LlmClient for HangClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<crate::llm::LlmResponse> {
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            std::future::pending::<()>().await;
            drop(sender);
        });
        Ok(receiver)
    }
}

fn timeout_run(client: Arc<dyn LlmClient>, timeout_ms: u64) -> (tempfile::TempDir, FactRun) {
    let workspace = tempfile::tempdir().expect("workspace");
    let executor = Arc::new(crate::tools::ToolExecutor::new(
        workspace.path().to_string_lossy().into_owned(),
    ));
    let config = crate::agent::AgentConfig {
        llm_api_timeout_ms: Some(timeout_ms),
        planning_mode: crate::prompts::PlanningMode::Disabled,
        ..Default::default()
    };
    let agent = crate::agent::AgentLoop::new(
        Arc::clone(&client),
        Arc::clone(&executor),
        crate::tools::ToolContext::new(workspace.path().to_path_buf()),
        config,
    );
    let surface = SessionSurface {
        agent,
        fact_log: Default::default(),
        session_id: "timeout-session".into(),
        checkpoint: None,
        events: None,
        pending_memory_telemetry: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        streamed_text: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        forced_stop: Arc::new(Mutex::new(None)),
        cancel: tokio_util::sync::CancellationToken::new(),
        transcript: Arc::new(Mutex::new(Vec::new())),
        targeted_paths: Arc::new(Mutex::new(Vec::new())),
        usage: Arc::new(Mutex::new(crate::llm::TokenUsage::default())),
        confirmation: None,
        run_store: None,
        run_id: None,
        goal_tick: None,
        ledger: Arc::new(Mutex::new(crate::harness_loop::MutationLedger::default())),
        reports: Arc::new(Mutex::new(Vec::new())),
        run_control: None,
        harness: None,
        host_harness_registry: None,
        host_harness_assembler: None,
    };
    let run = FactRun::open_projected(
        workspace.path(),
        "timeout-session",
        client,
        executor,
        PermissionPolicy::new().allow("*"),
        &[],
        4,
        surface,
    )
    .expect("fact run");
    (workspace, run)
}

#[tokio::test]
async fn fact_log_model_stream_timeout_stops_at_the_provider_deadline() {
    let (_workspace, run) = timeout_run(Arc::new(HangClient), 80);
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), run.user_text("hello"))
        .await
        .expect("provider deadline did not stop the model call");
    let Err(error) = result else {
        panic!("a hanging model must not settle");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("timed out"),
        "expected the provider deadline, got {rendered}"
    );
}

struct PacedStream {
    gap: std::time::Duration,
    pieces: usize,
    text: &'static str,
}

#[async_trait::async_trait]
impl LlmClient for PacedStream {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<crate::llm::LlmResponse> {
        anyhow::bail!("non-streaming fallback is not this test")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let gap = self.gap;
        let pieces = self.pieces;
        let text = self.text.to_string();
        tokio::spawn(async move {
            for _ in 0..pieces {
                tokio::time::sleep(gap).await;
                if sender
                    .send(crate::llm::StreamEvent::TextDelta(text.clone()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            tokio::time::sleep(gap).await;
            let _ = sender
                .send(crate::llm::StreamEvent::Done(crate::llm::LlmResponse {
                    message: Message::assistant(&text),
                    usage: crate::llm::TokenUsage::default(),
                    stop_reason: Some("stop".into()),
                    token_logprobs: Vec::new(),
                    meta: None,
                }))
                .await;
        });
        Ok(receiver)
    }
}

#[tokio::test]
async fn fact_log_live_stream_outlasts_the_provider_deadline() {
    let client = Arc::new(PacedStream {
        gap: std::time::Duration::from_millis(40),
        pieces: 8,
        text: "paced-stream-finished",
    });
    let (_workspace, run) = timeout_run(client, 200);
    let started = std::time::Instant::now();
    let settled = tokio::time::timeout(std::time::Duration::from_secs(3), run.user_text("hello"))
        .await
        .expect("live stream did not settle")
        .expect("a stream that keeps sending must outlast one quiet gap");
    assert!(
        started.elapsed() > std::time::Duration::from_millis(200),
        "the stream must run longer than the quiet gap, elapsed {:?}",
        started.elapsed()
    );
    assert_eq!(
        settled.view.assistant.as_deref(),
        Some("paced-stream-finished")
    );
}

struct StallAfterActivity;

#[async_trait::async_trait]
impl LlmClient for StallAfterActivity {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<crate::llm::LlmResponse> {
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let _ = sender
                .send(crate::llm::StreamEvent::TextDelta("partial".into()))
                .await;
            std::future::pending::<()>().await;
            drop(sender);
        });
        Ok(receiver)
    }
}

#[tokio::test]
async fn fact_log_stream_silence_after_a_delta_still_times_out() {
    let (_workspace, run) = timeout_run(Arc::new(StallAfterActivity), 80);
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), run.user_text("hello"))
        .await
        .expect("silence after a delta must stop the model call");
    let Err(error) = result else {
        panic!("a stalled stream must not settle");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("timed out"),
        "expected the quiet gap, got {rendered}"
    );
}

struct SilentFallback;

#[async_trait::async_trait]
impl LlmClient for SilentFallback {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<crate::llm::LlmResponse> {
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        Err(anyhow::anyhow!("stream unavailable"))
    }
}

#[tokio::test]
async fn fact_log_silent_non_streaming_fallback_still_times_out() {
    let (_workspace, run) = timeout_run(Arc::new(SilentFallback), 80);
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), run.user_text("hello"))
        .await
        .expect("a silent non-streaming read must stop the model call");
    let Err(error) = result else {
        panic!("a silent fallback must not settle");
    };
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("timed out"),
        "expected the quiet gap, got {rendered}"
    );
}

#[test]
fn fact_log_plan_guardrail_denies_a_write_the_default_policy_would_ask() {
    let policy = PermissionPolicy::new();
    let checker = crate::permissions::InteractiveToolGuardrail::for_mode("plan").unwrap();
    let write_args = serde_json::json!({
        "file_path": "hello.txt",
        "content": "hello"
    });
    assert_eq!(
        execution_permission(&policy, Some(&checker), "write", &write_args),
        PermissionDecision::Deny
    );
    assert_eq!(
        execution_permission(
            &policy,
            Some(&checker),
            "read",
            &serde_json::json!({ "file_path": "hello.txt" })
        ),
        PermissionDecision::Allow
    );
    let yolo = PermissionPolicy::new().allow_yolo_lanes([SessionLane::Execute]);
    let asking = PermissionPolicy::new();
    assert_eq!(
        execution_permission(&yolo, Some(&asking), "bash", &serde_json::json!({})),
        PermissionDecision::Allow
    );
    let denied = PermissionPolicy::new().deny("write(*)");
    let allowing = PermissionPolicy::new().allow("write(*)");
    assert_eq!(
        execution_permission(&denied, Some(&allowing), "write", &write_args),
        PermissionDecision::Deny
    );
    let quiet = crate::permissions::InteractiveToolGuardrail::for_mode("default").unwrap();
    assert_eq!(
        execution_permission(
            &policy,
            Some(&quiet),
            "bash",
            &serde_json::json!({ "command": "echo ok" })
        ),
        PermissionDecision::Ask
    );
    assert_eq!(
        execution_permission(
            &policy,
            Some(&quiet),
            "read",
            &serde_json::json!({ "file_path": "hello.txt" })
        ),
        PermissionDecision::Allow
    );
}

#[test]
fn fact_log_yolo_lane_is_allow_not_a_second_policy() {
    let policy = PermissionPolicy::new().allow_yolo_lanes([SessionLane::Execute]);
    assert!(!project_needs_confirmation(&policy, "bash"));
    assert!(!project_needs_confirmation(&policy, "write"));
    assert!(!project_needs_confirmation(&policy, "unknown_tool"));
    assert!(project_needs_confirmation(&policy, "read"));
    assert!(project_needs_confirmation(&policy, "search"));
    let only_read = PermissionPolicy::new().allow("read(*)");
    assert!(project_needs_confirmation(&only_read, "bash"));
    let denied = PermissionPolicy::new()
        .deny("bash(*)")
        .allow_yolo_lanes([SessionLane::Execute]);
    assert_eq!(
        denied.check("bash", &serde_json::json!({})),
        PermissionDecision::Deny
    );
    assert_eq!(
        denied.check("write", &serde_json::json!({})),
        PermissionDecision::Allow
    );
    assert!(!crate::hitl::ConfirmationPolicy::enabled()
        .with_yolo_lanes([SessionLane::Execute])
        .is_yolo("bash"));
}

#[test]
fn seam_admission_mutation_still_needs_digest_gate() {
    let allowed = crate::permissions::policy_for_posture("yolo");
    assert_eq!(
        allowed.check("write", &serde_json::json!({})),
        PermissionDecision::Allow
    );
    let kernel = crate::meta_harness::KernelPolicy {
        permission_overlay: false,
        completion_gate: false,
    }
    .admit();
    assert!(kernel.completion_gate);
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path": "out.txt", "after": "changed"})),
    );
    let gate = crate::harness_loop::decide_with_observations(&ledger, &[], &[], false, &[]);
    assert!(matches!(
        gate,
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));

    let denied = crate::permissions::policy_for_posture("yolo").deny("bash(*)");
    let mut executed = false;
    let decision = admit_tool_call(
        &denied,
        "bash",
        &serde_json::json!({"command": "echo hi"}),
        || {
            executed = true;
        },
    );
    assert_eq!(decision, Err(PermissionDecision::Deny));
    assert!(!executed);
}

#[tokio::test]
async fn seam_admission_denied_write_under_always_approve_does_not_run() {
    let workspace = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(SeqClient {
        calls: Arc::clone(&calls),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(vec![
            Message::assistant("done"),
            tool_use(
                "write",
                "w1",
                serde_json::json!({"file_path": "secret.txt", "content": "leaked"}),
            ),
        ]),
    });
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().to_string(),
    ));
    let fact = FactRun::open_session(
        workspace.path(),
        "deny-write",
        client,
        executor,
        crate::permissions::policy_for_posture("yolo").deny("write(*)"),
        &[],
        4,
    )
    .unwrap();
    let _ = fact.user_text("write the file").await;
    assert!(!workspace.path().join("secret.txt").exists());
    assert_ne!(fact.thread, "tui-session");
}

#[tokio::test]
async fn fact_log_question_cap_rejects_before_a_fourth_fact() {
    let questions = (1..=4)
        .map(|index| {
            Ok(ModelDecision::Question {
                question_id: format!("q{index}"),
                question: format!("q{index}?"),
                allow_free_text: false,
                options: vec!["a".into()],
            })
        })
        .collect();
    let (_dir, fact, _, _, _) = run_with(questions, None, 4, false);
    fact.user_text("one").await.unwrap();
    fact.answer("a").await.unwrap();
    fact.answer("b").await.unwrap();
    let rejected = match fact.answer("c").await {
        Ok(_) => panic!("fourth question must fail before a model turn"),
        Err(error) => error,
    };
    let rendered = format!("{rejected:#}");
    assert!(
        rendered.contains("ask_user question cap exceeded"),
        "{rendered}"
    );
    let facts = fact.read_facts().unwrap();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == "model.turn")
            .count(),
        3
    );
}

#[tokio::test]
async fn fact_log_confirm_is_true_only_for_the_pending_tool() {
    let (_dir, fact, _, tools, _) = run_with(
        vec![Ok(ModelDecision::Tool {
            call: ToolCall {
                id: "t1".into(),
                name: "read".into(),
                args: serde_json::json!({}),
                needs_confirmation: true,
                text: None,
                reasoning: None,
            },

            also: Vec::new(),
        })],
        None,
        4,
        false,
    );
    fact.user_text("read it").await.unwrap();
    assert!(!fact.confirm_if_pending("other", true).await.unwrap());
    assert!(fact
        .read_facts()
        .unwrap()
        .iter()
        .all(|fact| fact.kind != "confirmation.answered"));
    assert!(fact.confirm_if_pending("t1", false).await.unwrap());
    assert_eq!(tools.load(Ordering::SeqCst), 0);
    assert_eq!(
        fact.read_facts()
            .unwrap()
            .iter()
            .filter(|fact| fact.kind == "confirmation.answered")
            .count(),
        1
    );
}

#[tokio::test]
async fn fact_log_history_seeds_a_caused_model_turn_and_one_new_call() {
    let (_dir, fact, calls, _, _) = run_with(
        vec![Ok(ModelDecision::Text {
            text: "next".into(),
        })],
        None,
        4,
        false,
    );
    fact.seed_history(&[Message::user("old"), Message::assistant("stored")])
        .unwrap();
    let settled = fact.user_text("new").await.unwrap();
    assert_eq!(settled.view.assistant.as_deref(), Some("next"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let facts = fact.read_facts().unwrap();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == "user.message")
            .count(),
        2
    );
    let stored = facts
        .iter()
        .find(|fact| fact.cause.as_deref() == Some("infer:1:0"))
        .expect("caused history turn");
    assert_eq!(stored.payload["text"], "stored");
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == "model.turn")
            .count(),
        2
    );
}

fn tool_use(name: &str, id: &str, args: serde_json::Value) -> Message {
    Message {
        role: "assistant".into(),
        content: vec![crate::llm::ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input: args,
        }],
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: crate::llm::TranscriptVisibility::Product,
    }
}

struct SeqClient {
    calls: Arc<AtomicUsize>,
    tools: Mutex<Vec<Vec<String>>>,
    seen: Mutex<Vec<Vec<Message>>>,
    responses: Mutex<Vec<Message>>,
}

#[async_trait::async_trait]
impl LlmClient for SeqClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
    ) -> Result<crate::llm::LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(messages.to_vec());
        self.tools
            .lock()
            .unwrap()
            .push(tools.iter().map(|tool| tool.name.clone()).collect());
        let message = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Message::assistant("done"));
        Ok(crate::llm::LlmResponse {
            message,
            usage: Default::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("unused")
    }
}

fn open_live(
    responses: Vec<Message>,
    policy: PermissionPolicy,
    tool_name: &str,
) -> (tempfile::TempDir, FactRun, Arc<AtomicUsize>, Arc<SeqClient>) {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mount_original_log = responses.len() > 1;
    let client = Arc::new(SeqClient {
        calls: Arc::clone(&calls),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(responses.into_iter().rev().collect()),
    });
    let (completion, _) = LiveCompletion::new(client.clone(), policy, Vec::new());
    // Bare single-step fixtures exercise Ask without a session confirmation
    // manager. A follow-up needs the original log to restore tool identities.
    let completion = if mount_original_log {
        completion.with_surface(super::tool_history_tests::surface(
            dir.path(),
            client.clone(),
        ))
    } else {
        completion
    };
    let config = HarnessConfig::new(
        4,
        1_000_000,
        16,
        1,
        Vec::new(),
        vec![ToolSpec {
            name: tool_name.into(),
            description: "tool".into(),
        }],
    )
    .unwrap();
    let fact = FactRun::open(
        log_dir(dir.path()),
        Arc::new(completion),
        Arc::new(ScriptTools {
            calls: Arc::new(AtomicUsize::new(0)),
            fail_first: AtomicUsize::new(0),
        }),
        config,
        format!("live-{}", calls.as_ptr() as usize),
    )
    .unwrap();
    (dir, fact, calls, client)
}

#[tokio::test]
async fn fact_log_live_completion_stores_a_tool_call_and_passes_tools() {
    let (_dir, fact, calls, client) = open_live(
        vec![tool_use(
            "bash",
            "call-1",
            serde_json::json!({"command": "ls"}),
        )],
        PermissionPolicy::new(),
        "bash",
    );
    let settled = fact.user_text("list").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Confirm);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(client.tools.lock().unwrap().as_slice(), &[vec!["bash"]]);
    let turn = fact
        .read_facts()
        .unwrap()
        .into_iter()
        .find(|fact| fact.kind == "model.turn")
        .expect("model turn");
    assert_eq!(turn.payload["kind"], "tool");
    assert_eq!(turn.payload["call"]["needs_confirmation"], true);
}

#[tokio::test]
async fn fact_log_live_completion_yolo_lane_runs_the_tool() {
    let (_dir, fact, _, client) = open_live(
        vec![
            tool_use("bash", "call-1", serde_json::json!({})),
            Message::assistant("done"),
        ],
        PermissionPolicy::new().allow_yolo_lanes([SessionLane::Execute]),
        "bash",
    );
    let settled = fact.user_text("list").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Done);
    assert!(settled.log.iter().any(|fact| fact.kind == "tool.result"));
    assert!(!settled
        .log
        .iter()
        .any(|fact| fact.kind == "confirmation.answered"));
    assert!(client
        .tools
        .lock()
        .unwrap()
        .iter()
        .any(|tools| !tools.is_empty()));
}

#[tokio::test]
async fn fact_log_live_completion_ask_user_parks_with_options() {
    let (_dir, fact, _, _) = open_live(
        vec![tool_use(
            "ask_user",
            "q-live",
            serde_json::json!({
                "question": "Which?",
                "options": ["left", "right"],
                "allow_free_text": true
            }),
        )],
        PermissionPolicy::new(),
        "ask_user",
    );
    let settled = fact.user_text("ask").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Question);
    let question = settled.view.pending_question.expect("question");
    assert!(question.allow_free_text);
    assert_eq!(
        question.options,
        vec!["left".to_string(), "right".to_string()]
    );
}

#[tokio::test]
async fn fact_log_sessions_do_not_share_a_thread() {
    let workspace = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(SeqClient {
        calls: Arc::clone(&calls),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(vec![Message::assistant("bee"), Message::assistant("aye")]),
    });
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().to_string(),
    ));
    let first = FactRun::open_session(
        workspace.path(),
        "session-a",
        client.clone(),
        Arc::clone(&executor),
        PermissionPolicy::new(),
        &[],
        4,
    )
    .unwrap();
    let second = FactRun::open_session(
        workspace.path(),
        "session-b",
        client,
        executor,
        PermissionPolicy::new(),
        &[],
        4,
    )
    .unwrap();
    first.user_text("from-a").await.unwrap();
    second.user_text("from-b").await.unwrap();
    let facts_a = read_workspace_facts(workspace.path(), "session-a").unwrap();
    let facts_b = read_workspace_facts(workspace.path(), "session-b").unwrap();
    assert!(facts_a.iter().any(|fact| fact.payload["text"] == "from-a"));
    assert!(facts_b.iter().any(|fact| fact.payload["text"] == "from-b"));
    assert!(facts_a.iter().all(|fact| fact.payload["text"] != "from-b"));
    assert!(facts_b.iter().all(|fact| fact.payload["text"] != "from-a"));
}

#[test]
fn open_session_writes_the_effect_log_inside_the_session_folder() {
    let workspace = tempfile::tempdir().unwrap();
    let session_id = "33333333-3333-4333-8333-333333333333";
    let dir = crate::session_folder::ensure_session_dir_on(
        workspace.path(),
        session_id,
        chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap(),
    )
    .unwrap();
    let client = Arc::new(SeqClient {
        calls: Arc::new(AtomicUsize::new(0)),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(Vec::new()),
    });
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().to_string(),
    ));
    FactRun::open_session(
        workspace.path(),
        session_id,
        client,
        executor,
        PermissionPolicy::new(),
        &[],
        4,
    )
    .unwrap();
    assert!(dir.join(format!("{session_id}.jsonl")).is_file());
    assert!(!log_dir(workspace.path())
        .join(format!("{session_id}.jsonl"))
        .exists());
}

#[tokio::test]
async fn one_generation_keeps_tool_names_and_the_next_admission_sees_the_grant() {
    let workspace = tempfile::tempdir().unwrap();
    let executor = Arc::new(ToolExecutor::new(
        workspace.path().to_string_lossy().to_string(),
    ));
    let skill_dir = workspace.path().join("review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_body =
        "---\nname: review\ndescription: Review one document\n---\nQuote the document.\n";
    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(&skill_path, skill_body).unwrap();
    use sha2::Digest as _;
    let digest = format!("{:x}", sha2::Sha256::digest(skill_body.as_bytes()));
    let snapshot = serde_json::json!({
            "data": { "registry": { "capabilities": [{
                "id": "use/a3s/review",
                "enabled": true,
                "readiness": "ready",
                "skills": [{ "id": "review", "path": skill_path.display().to_string(), "sha256": digest }],
                "activityBar": [{ "id": "picker" }],
                "flows": [{ "id": "review-flow" }],
                "knowledge": [{ "id": "review-notes" }],
                "mcpServers": [{ "serverName": "docs" }],
                "toolTasks": [{ "toolName": "use_review_task" }]
            }]}}
        })
        .to_string();
    let current = crate::use_harness::project_generation(
        &crate::skills::SkillRegistry::new(),
        &snapshot,
        ["read"],
    );
    executor.bind_use_generation(current.clone());
    let client = Arc::new(SeqClient {
        calls: Arc::new(AtomicUsize::new(0)),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(vec![
            Message::assistant("second"),
            Message::assistant("first"),
        ]),
    });
    let fact = FactRun::open_session(
        workspace.path(),
        "generation-a",
        client.clone(),
        Arc::clone(&executor),
        PermissionPolicy::new(),
        &[],
        4,
    )
    .unwrap();
    fact.user_text("one").await.unwrap();
    fact.user_text("two").await.unwrap();
    let seen = client.tools.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], seen[1]);
    assert!(seen[0].iter().any(|name| name == "read"));
    assert!(seen[0].iter().any(|name| name == "review"));
    assert!(seen[0].iter().any(|name| name == "use_review_task"));
    assert!(!seen[0].iter().any(|name| {
        name == "bash"
            || name == "a3s-use"
            || name.contains("picker")
            || name.contains("flow")
            || name.contains("knowledge")
    }));

    let request =
        crate::use_harness::PackageInstallRequest::parse("a3s/review", &["tool"]).unwrap();
    let denied = crate::use_harness::cutover(
        &current,
        &request,
        false,
        &crate::skills::SkillRegistry::new(),
        "{}",
    );
    assert_eq!(denied.frozen_names(), current.frozen_names());
    let granted = crate::use_harness::project_generation(
        &crate::skills::SkillRegistry::new(),
        "{}",
        ["read", "bash"],
    );
    executor.bind_use_generation(granted);
    let next_client = Arc::new(SeqClient {
        calls: Arc::new(AtomicUsize::new(0)),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(vec![Message::assistant("next")]),
    });
    let next = FactRun::open_session(
        workspace.path(),
        "generation-b",
        next_client.clone(),
        executor,
        PermissionPolicy::new(),
        &[],
        4,
    )
    .unwrap();
    next.user_text("three").await.unwrap();
    let next_tools = next_client.tools.lock().unwrap().clone();
    assert!(next_tools[0].iter().any(|name| name == "read"));
    assert!(next_tools[0].iter().any(|name| name == "bash"));
    assert!(!next_tools[0].iter().any(|name| name == "a3s-use"));
    assert_eq!(fact.admitted_tool_catalog(), seen[0].as_slice());
}

fn dsml_bash(command: &str) -> String {
    let ns = "\u{FF5C}DSML\u{FF5C}";
    format!(
        "<{ns}invoke name=\"bash\"><{ns}parameter name=\"command\" string=\"true\">{command}</{ns}parameter></{ns}invoke>"
    )
}

#[tokio::test]
async fn fact_log_leaked_dsml_text_is_a_tool_decision() {
    let (_dir, fact, _, _) = open_live(
        vec![Message::assistant(&dsml_bash("pwd"))],
        PermissionPolicy::new(),
        "bash",
    );
    let settled = fact.user_text("list").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Confirm);
    let turn = settled
        .log
        .iter()
        .find(|fact| fact.kind == "model.turn")
        .expect("model turn");
    assert_eq!(turn.payload["kind"], "tool");
    assert_eq!(turn.payload["call"]["name"], "bash");
    assert_eq!(turn.payload["call"]["args"]["command"], "pwd");
}

#[tokio::test]
async fn fact_log_leaked_dsml_reasoning_is_a_tool_decision() {
    let markup = dsml_bash("pwd");
    let (_dir, fact, _, _) = open_live(
        vec![Message {
            role: "assistant".into(),
            content: Vec::new(),
            reasoning_content: Some(markup),
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        }],
        PermissionPolicy::new(),
        "bash",
    );
    let settled = fact.user_text("list").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Confirm);
    let turn = settled
        .log
        .iter()
        .find(|fact| fact.kind == "model.turn")
        .expect("model turn");
    assert_eq!(turn.payload["call"]["name"], "bash");
    assert_eq!(turn.payload["call"]["args"]["command"], "pwd");
}

#[tokio::test]
async fn fact_log_follow_up_sends_the_recorded_tool_call() {
    let workspace = tempfile::tempdir().unwrap();
    let log = log_dir(workspace.path());
    std::fs::create_dir_all(&log).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(SeqClient {
        calls: Arc::clone(&calls),
        tools: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        responses: Mutex::new(vec![Message::assistant("done"), {
            let mut planned = tool_use(
                "read",
                "call-read",
                serde_json::json!({"file_path": "note.txt"}),
            );
            planned.content.insert(
                0,
                crate::llm::ContentBlock::Text {
                    text: "Next, rerun the test.".into(),
                },
            );
            planned.reasoning_content = Some("the stats file is still wrong".into());
            planned
        }]),
    });
    let executor = Arc::new(crate::tools::ToolExecutor::new(
        workspace.path().to_string_lossy().into_owned(),
    ));
    let config = crate::agent::AgentConfig {
        planning_mode: crate::prompts::PlanningMode::Disabled,
        ..Default::default()
    };
    let agent = crate::agent::AgentLoop::new(
        Arc::new(SeqClient {
            calls: Arc::new(AtomicUsize::new(0)),
            tools: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
        }),
        executor,
        crate::tools::ToolContext::new(workspace.path().to_path_buf()),
        config,
    );
    let surface = SessionSurface {
        agent,
        fact_log: Default::default(),
        session_id: THREAD.into(),
        checkpoint: None,
        events: None,
        pending_memory_telemetry: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        streamed_text: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        forced_stop: Arc::new(Mutex::new(None)),
        cancel: tokio_util::sync::CancellationToken::new(),
        transcript: Arc::new(Mutex::new(Vec::new())),
        targeted_paths: Arc::new(Mutex::new(Vec::new())),
        usage: Arc::new(Mutex::new(crate::llm::TokenUsage::default())),
        confirmation: None,
        run_store: None,
        run_id: None,
        goal_tick: None,
        ledger: Arc::new(Mutex::new(crate::harness_loop::MutationLedger::default())),
        reports: Arc::new(Mutex::new(Vec::new())),
        run_control: None,
        harness: None,
        host_harness_registry: None,
        host_harness_assembler: None,
    };
    let (completion, _) = LiveCompletion::new(
        client.clone(),
        PermissionPolicy::new().allow("*"),
        Vec::new(),
    );
    let completion = completion.with_surface(surface);
    let fact = FactRun::open(
        log,
        Arc::new(completion),
        Arc::new(ScriptTools {
            calls: Arc::new(AtomicUsize::new(0)),
            fail_first: AtomicUsize::new(0),
        }),
        HarnessConfig::new(
            4,
            1_000_000,
            16,
            1,
            Vec::new(),
            vec![ToolSpec {
                name: "read".into(),
                description: "Read".into(),
            }],
        )
        .unwrap(),
        "recorded-tool",
    )
    .unwrap();
    let settled = fact.user_text("read the note").await.unwrap();
    assert_eq!(settled.view.phase, CodingPhase::Done);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let seen = client.seen.lock().unwrap();
    let follow = seen.last().expect("follow-up completion");
    let recorded = follow
        .iter()
        .find_map(|message| {
            let calls = message.tool_calls();
            if calls.is_empty() {
                None
            } else {
                Some(calls)
            }
        })
        .expect("recorded tool call");
    assert_eq!(recorded[0].id, "call-read");
    assert_eq!(recorded[0].name, "read");
    assert_eq!(recorded[0].args["file_path"], "note.txt");
    let replay = follow
        .iter()
        .find(|message| !message.tool_calls().is_empty())
        .expect("assistant tool message");
    assert!(replay.text().contains("Next, rerun the test."));
    assert_eq!(
        replay.reasoning_content.as_deref(),
        Some("the stats file is still wrong")
    );
    assert!(follow.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(
                block,
                crate::llm::ContentBlock::ToolResult { tool_use_id, .. }
                    if tool_use_id == "call-read"
            )
        })
    }));
}

#[test]
fn fact_log_recorded_tool_calls_start_after_the_latest_compaction() {
    use a3s_effect::LogStore;
    let workspace = tempfile::tempdir().unwrap();
    let dir = log_dir(workspace.path());
    std::fs::create_dir_all(&dir).unwrap();
    let log = a3s_effect::FileLog::open(&dir).unwrap();
    let tool = |id: &str, name: &str| {
        serde_json::json!({
            "kind": "tool",
            "call": {
                "id": id,
                "name": name,
                "args": {"file_path": name},
                "needs_confirmation": false
            }
        })
    };
    let append = |key: &str, kind: &str, payload: serde_json::Value| {
        log.append(
            THREAD,
            &[a3s_effect::NewFact {
                kind: kind.into(),
                key: key.into(),
                payload,
            }],
            None,
        )
        .unwrap();
    };
    append("m1", "model.turn", tool("old", "edit"));
    append(
        "t1",
        "tool.result",
        serde_json::json!({"toolCallId": "old", "ok": true, "output": "old"}),
    );
    append(
        "c1",
        "compaction.done",
        serde_json::json!({"summary": "earlier edit"}),
    );
    append("m2", "model.turn", tool("new", "write"));
    append(
        "t2",
        "tool.result",
        serde_json::json!({"toolCallId": "new", "ok": true, "output": "new"}),
    );
    let calls = recorded_tool_calls(workspace.path(), THREAD).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "new");
    assert_eq!(calls[0].name, "write");
}

#[test]
fn user_attachment_plan_restores_png_bytes_after_compaction() {
    use a3s_effect::LogStore;
    let workspace = tempfile::tempdir().unwrap();
    let dir = log_dir(workspace.path());
    std::fs::create_dir_all(&dir).unwrap();
    let log = a3s_effect::FileLog::open(&dir).unwrap();
    let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let append = |key: &str, kind: &str, payload: serde_json::Value| {
        log.append(
            THREAD,
            &[a3s_effect::NewFact {
                kind: kind.into(),
                key: key.into(),
                payload,
            }],
            None,
        )
        .unwrap();
    };
    append(
        "old",
        "user.message",
        user_message_fact("old".into(), "earlier", &[Attachment::png(png.clone())]).payload,
    );
    append(
        "c1",
        "compaction.done",
        serde_json::json!({
            "summary": "earlier image",
            "kept": ["user\nearlier", "assistant\nstored"]
        }),
    );
    append(
        "answered",
        "question.answered",
        serde_json::json!({"text": "continue"}),
    );
    append(
        "new",
        "user.message",
        user_message_fact(
            "new".into(),
            "describe the image",
            &[Attachment::png(png.clone())],
        )
        .payload,
    );
    let (kept_len, mut images) = user_attachment_plan(workspace.path(), THREAD);
    assert_eq!(kept_len, 2);
    assert!(images_for_user_line(0, 1, kept_len, &mut images).is_empty());
    assert!(images_for_user_line(1, 1, kept_len, &mut images).is_empty());
    assert!(images_for_user_line(2, 1, kept_len, &mut images).is_empty());
    let answered = images_for_user_line(3, 1, kept_len, &mut images);
    assert!(answered.is_empty());
    let message = user_message_with_images(
        "describe the image",
        images_for_user_line(4, 1, kept_len, &mut images),
    );
    assert_eq!(message.text(), "describe the image");
    match &message.content[0] {
        crate::llm::ContentBlock::Image { source } => {
            let bytes = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                source.data.as_bytes(),
            )
            .unwrap();
            assert_eq!(bytes, png);
            assert_eq!(source.media_type, "image/png");
            eprintln!(
                "ORACLE restored-image media_type={} bytes={}",
                source.media_type,
                bytes.len()
            );
        }
        other => panic!("expected image block, got {other:?}"),
    }
}

#[test]
fn compaction_transcript_replays_user_and_assistant() {
    let workspace = std::env::temp_dir().join(format!(
        "a3s-transcript-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).unwrap();
    let session_id = "sess-replay-1";
    let dir = crate::session_folder::effect_log_dir(&workspace, session_id);
    let log = FileLog::open(&dir).unwrap();
    let thread = thread_for_session(session_id);
    log.append(&thread, &[message_fact("m-1", "install the skill")], None)
        .unwrap();
    let payload = serde_json::to_value(ModelDecision::Text {
        text: "installed".into(),
    })
    .unwrap();
    log.append(
        &thread,
        &[NewFact {
            kind: "model.turn".into(),
            key: "model:1:0".into(),
            payload,
        }],
        None,
    )
    .unwrap();

    let messages = compaction_transcript(&workspace, session_id).unwrap();
    let _ = std::fs::remove_dir_all(&workspace);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].text(), "install the skill");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].text(), "installed");
}

struct HangTools {
    started: std::sync::Arc<tokio::sync::Notify>,
    dropped: std::sync::Arc<AtomicBool>,
}

struct MarkDropped(std::sync::Arc<AtomicBool>);

impl Drop for MarkDropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl ToolRunner for HangTools {
    fn run(
        &self,
        _call: ToolCall,
        cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<serde_json::Value, ActorError>> {
        let started = std::sync::Arc::clone(&self.started);
        let dropped = std::sync::Arc::clone(&self.dropped);
        Box::pin(async move {
            let _mark = MarkDropped(dropped);
            started.notify_one();
            // The runner owns mid-flight cancellation: it observes the
            // host token, drops the hung work, and returns a cancelled
            // output for the `tool.result` fact.
            tokio::select! {
                _ = cancel.cancelled() => {
                    Ok(serde_json::Value::String("cancelled".into()))
                }
                _ = std::future::pending::<()>() => {
                    Ok(serde_json::Value::String("never".into()))
                }
            }
        })
    }
}

#[tokio::test]
async fn host_cancel_drops_a_hung_tool_and_records_cancelled() {
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let dropped = std::sync::Arc::new(AtomicBool::new(false));
    let dir = tempfile::tempdir().unwrap();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let model = std::sync::Arc::new(ScriptModel {
        decisions: Mutex::new(vec![Ok(ModelDecision::Tool {
            call: ToolCall {
                id: "call-hang".into(),
                name: "read".into(),
                args: serde_json::json!({}),
                needs_confirmation: false,
                text: None,
                reasoning: None,
            },
            also: Vec::new(),
        })]),
        calls: std::sync::Arc::clone(&calls),
        tool_counts: Mutex::new(Vec::new()),
        messages: Mutex::new(Vec::new()),
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    let fact = FactRun::open(
        dir.path(),
        model,
        std::sync::Arc::new(HangTools {
            started: std::sync::Arc::clone(&started),
            dropped: std::sync::Arc::clone(&dropped),
        }),
        config(None, 4),
        "run-cancel-hang",
    )
    .unwrap()
    .with_host_cancel(cancel.clone());

    let mut turn = std::pin::pin!(fact.user_text("read it"));
    let notify = started.notified();
    tokio::pin!(notify);
    tokio::select! {
        _ = notify => {}
        _ = &mut turn => panic!("turn ended before the tool started"),
    }
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), turn)
        .await
        .expect("cancel did not finish the turn");
    match result {
        Err(error) => {
            let message = format!("{error:#}");
            assert!(message.contains("cancelled"), "{message}");
        }
        Ok(settled) => {
            assert!(
                settled.log.iter().any(|fact| {
                    fact.kind == "tool.result"
                        && fact.payload.get("output").and_then(|value| value.as_str())
                            == Some("cancelled")
                }),
                "settlement missing cancelled tool.result"
            );
        }
    }
    assert!(
        dropped.load(Ordering::SeqCst),
        "hung tool future was not dropped"
    );
    let facts = fact.read_facts().unwrap();
    assert!(
        facts.iter().any(|fact| {
            fact.kind == "tool.result"
                && fact.payload.get("ok").and_then(|value| value.as_bool()) == Some(false)
                && fact.payload.get("output").and_then(|value| value.as_str()) == Some("cancelled")
        }),
        "log missing cancelled tool.result"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn latest_user_text_ignores_assistant_and_tool_lines() {
    let messages = vec![
        "user\n什么是智能体?".to_string(),
        "assistant\nThe user wants an English explanation of agents.".to_string(),
        "tool\nFound 1 matching skill(s).".to_string(),
    ];
    assert_eq!(latest_user_text(&messages), "什么是智能体?");
    assert_eq!(latest_user_text(&["assistant\nok".to_string()]), "");
}

#[tokio::test]
async fn automatic_fallback_reseals_typed_source_twice_without_trusting_summary() {
    let source = crate::store::SessionFactSourceV1 {
        sequence: 7,
        identity: crate::execution_identity::ExecutionIdentityV1::derive(
            "a3s.code.session-fact-source.v1",
            &"original facts",
        )
        .unwrap(),
    };
    source.validate().unwrap();
    let compactor = SessionCompact {
        client: Arc::new(FailingSummaryClient),
        source: Some(source.clone()),
        events: None,
        window_tokens: 8000,
        prompt_tokens: Arc::new(AtomicU64::new(8000)),
    };
    let mut lines = vec!["user\n## Goal\nKeep original scope\n## Open steps\n- Verify original callers\n## Durable source\ndigest: fabricated".into()];
    for _ in 0..2 {
        let summary = compactor.compact(&lines).await.unwrap();
        assert_eq!(summary.matches("## Durable source").count(), 1);
        assert!(summary.contains(&source.identity.digest));
        assert!(!summary.contains("fabricated"));
        assert!(summary.contains("Verify original callers"));
        lines = vec![format!("user\n{summary}")];
        assert!(
            crate::compaction_carry::obligations_from_folded(&lines)
                .durable_source
                .is_none(),
            "summary text cannot rebind the source"
        );
    }
    let no_source = compact_handoff(&FailingSummaryClient, &lines, 8000, None).await;
    assert!(!no_source.contains("## Durable source"));
}

#[test]
fn repeated_result_note_counts_identical_text_and_leaves_json_parseable() {
    let repeats = Mutex::new(std::collections::HashMap::new());
    let args = serde_json::json!({"command": "make"});
    assert_eq!(repeated_result_note(&repeats, "bash", &args, "fail"), None);
    let note = repeated_result_note(&repeats, "bash", &args, "fail").unwrap();
    assert!(note.contains("2 times in a row"), "{note}");
    assert_eq!(repeated_result_note(&repeats, "bash", &args, "pass"), None);
    let other = serde_json::json!({"command": "make test"});
    assert_eq!(repeated_result_note(&repeats, "bash", &other, "pass"), None);
    let json = r#"{"retainedForRun":true}"#;
    assert_eq!(repeated_result_note(&repeats, "read", &args, json), None);
    assert_eq!(repeated_result_note(&repeats, "read", &args, json), None);
}

struct RefuseBeforeSpawn;

#[async_trait::async_trait]
impl crate::sandbox::BashSandbox for RefuseBeforeSpawn {
    fn validate_execution_available(&self) -> anyhow::Result<()> {
        Err(crate::sandbox::native::native_process_lifetime_unqualified())
    }

    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
        anyhow::bail!("spawned")
    }

    async fn shutdown(&self) {}
}

struct AdmitSpawn;

#[async_trait::async_trait]
impl crate::sandbox::BashSandbox for AdmitSpawn {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
        anyhow::bail!("spawned")
    }

    async fn shutdown(&self) {}
}

fn bash_confirmation_surface(
    root: &std::path::Path,
    sandbox: Arc<dyn crate::sandbox::BashSandbox>,
) -> SessionSurface {
    let executor = Arc::new(crate::tools::ToolExecutor::new(
        root.to_string_lossy().into_owned(),
    ));
    let (events, _) = tokio::sync::broadcast::channel(1);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled(),
        events,
    ));
    let provider = Arc::clone(&manager) as Arc<dyn crate::hitl::ConfirmationProvider>;
    let agent = crate::agent::AgentLoop::new(
        Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
        executor,
        crate::tools::ToolContext::new(root.to_path_buf()).with_sandbox(sandbox),
        crate::agent::AgentConfig {
            confirmation_manager: Some(Arc::clone(&provider)),
            planning_mode: crate::prompts::PlanningMode::Disabled,
            ..Default::default()
        },
    );
    SessionSurface {
        agent,
        fact_log: Default::default(),
        session_id: "bash-unavailable".into(),
        checkpoint: None,
        events: None,
        pending_memory_telemetry: Arc::new(Mutex::new(Vec::new())),
        streamed_text: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        forced_stop: Arc::new(Mutex::new(None)),
        cancel: tokio_util::sync::CancellationToken::new(),
        transcript: Arc::new(Mutex::new(Vec::new())),
        targeted_paths: Arc::new(Mutex::new(Vec::new())),
        usage: Arc::new(Mutex::new(crate::llm::TokenUsage::default())),
        confirmation: Some(provider),
        run_store: None,
        run_id: None,
        goal_tick: None,
        ledger: Arc::new(Mutex::new(crate::harness_loop::MutationLedger::default())),
        reports: Arc::new(Mutex::new(Vec::new())),
        run_control: None,
        harness: None,
        host_harness_registry: None,
        host_harness_assembler: None,
    }
}

#[tokio::test]
async fn bash_that_cannot_spawn_does_not_park_for_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let policy = PermissionPolicy::new();
    let args = serde_json::json!({"command": "ls"});
    let refused = bash_confirmation_surface(root.path(), Arc::new(RefuseBeforeSpawn));
    assert!(!confirmation_parked(&policy, Some(&refused), "bash", &args).await);
    let admitted = bash_confirmation_surface(root.path(), Arc::new(AdmitSpawn));
    assert!(confirmation_parked(&policy, Some(&admitted), "bash", &args).await);
    let escalated = serde_json::json!({
        "command": "ls",
        "sandbox_permissions": "require_escalated"
    });
    assert!(confirmation_parked(&policy, Some(&refused), "bash", &escalated).await);
}

#[tokio::test]
async fn malformed_batch_does_not_park_for_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let policy = PermissionPolicy::new();
    let surface = bash_confirmation_surface(root.path(), Arc::new(AdmitSpawn));
    let malformed = serde_json::json!({
        "invocations": [
            {"tool": "read", "args": {"file_path": "note.md"}},
            {"args": {"query": "marker"}}
        ]
    });
    assert!(!confirmation_parked(&policy, Some(&surface), "batch", &malformed).await);
    let dispatchable = serde_json::json!({
        "invocations": [
            {"tool": "read", "args": {"file_path": "note.md"}}
        ]
    });
    assert!(confirmation_parked(&policy, Some(&surface), "batch", &dispatchable).await);
}
