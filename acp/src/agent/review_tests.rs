#![cfg(test)]

use super::A3sCodeAgent;
use agent_client_protocol::{self as acp, Agent as _};
use serde_json::{json, Value};

async fn ext(host: &A3sCodeAgent, method: &str, params: Value) -> acp::Result<Value> {
    let raw = serde_json::value::to_raw_value(&params).unwrap();
    let response = host
        .ext_method(acp::ExtRequest::new(method, raw.into()))
        .await?;
    Ok(serde_json::from_str(response.0.get()).unwrap())
}

fn engineering_meta() -> acp::Meta {
    json!({"a3sEngineering":{"policy":"engineering-v1"}})
        .as_object()
        .unwrap()
        .clone()
}

/// One scripted turn through the shared host options admits the policy fact,
/// as the review Run does in product use; the host then loads the session.
async fn policy_session(launch: &crate::config::LaunchConfig, id: &str) -> acp::SessionId {
    let mut config = launch.config.clone();
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let store = A3sCodeAgent::interactive_session_store_dir(config.sessions_dir.as_deref());
    let meta = engineering_meta();
    let setup = super::SessionSetup {
        workspace: launch.workspace.clone(),
        model_id: config.default_model.as_deref().unwrap(),
        effort: super::effort_limits("high"),
        host: crate::host_context::studio_host_from_meta(Some(&meta)).unwrap(),
        components: &[],
        mcp_servers: vec![],
    };
    let options = A3sCodeAgent::host_session_options(id, &setup, &config, &store)
        .unwrap()
        .with_llm_client(std::sync::Arc::new(
            super::engineering_tests::CapturingClient::default(),
        ))
        .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_continuation(false);
    let agent = a3s_code_core::Agent::from_config(config).await.unwrap();
    let session = agent
        .session_async(launch.workspace.to_string_lossy(), Some(options))
        .await
        .unwrap();
    session
        .send("Reply ready without tools.", None)
        .await
        .unwrap();
    session.save().await.unwrap();
    session.close().await;
    acp::SessionId::new(id)
}

fn finding(id: &str, claim: &str, target: &str) -> Value {
    json!({"findingId":id,"severity":"warning","category":"correctness","claim":claim,
        "evidence":"src/budget.rs:2","subject":{"type":"opaque","kind":"engineering.target","uri":target}})
}

#[tokio::test]
#[ignore = "requires A3S_CONFIG_FILE and the live boyue/deepseek-v4-flash model"]
async fn review_real_model_findings_are_admitted_for_current_bytes_and_stale_after_an_edit() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("src/budget.rs"),
        "pub const MAX_LINES: usize = 3;\n\npub fn first_lines(lines: &[&str]) -> Vec<String> {\n    let mut out = Vec::new();\n    for index in 0..=MAX_LINES {\n        out.push(lines[index].to_string());\n    }\n    out\n}\n",
    )
    .unwrap();
    let config = std::env::var_os("A3S_CONFIG_FILE").expect("explicit live ACL path");
    let mut launch =
        crate::config::load_launch_config(root.path().to_owned(), Some(config.into())).unwrap();
    launch.model_id = "boyue/deepseek-v4-flash".into();
    launch.config.default_model = Some(launch.model_id.clone());
    launch.config.sessions_dir = Some(root.path().join("sessions"));
    launch.workspace = root.path().into();
    let host = A3sCodeAgent::new(launch.clone());
    let id = host
        .new_session(acp::NewSessionRequest::new(root.path()).meta(engineering_meta()))
        .await
        .unwrap()
        .session_id;
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    ext(
        &host,
        "x.ai/permission_posture",
        json!({"sessionId":id.0.as_ref(),"posture":"yolo","deny":["write(*)","edit(*)","bash(*)"]}),
    )
    .await
    .unwrap();
    let paths = json!(["src/budget.rs"]);
    let target = ext(
        &host,
        "a3s/review/target",
        json!({"sessionId":id.0.as_ref(),"paths":paths}),
    )
    .await
    .unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let started = std::time::Instant::now();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        host.prompt(acp::PromptRequest::new(
            id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(
                "Review src/budget.rs for correctness. Do not edit files. End your answer with exactly one line \
                 `FINDINGS <json>` where <json> is a JSON array; each item has findingId, severity \
                 (info|warning|error|blocker), category, claim and evidence (path:line).",
            ))],
        )),
    )
    .await
    .expect("bounded live review")
    .unwrap();
    assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
    let answer = session
        .durable_transcript()
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .unwrap()
        .text();
    let line = answer
        .lines()
        .rev()
        .find_map(|line| line.trim().trim_matches('`').strip_prefix("FINDINGS "))
        .unwrap_or_else(|| panic!("no FINDINGS line: {answer}"))
        .to_owned();
    let mut findings: Vec<Value> = serde_json::from_str(line.trim()).unwrap();
    assert!(!findings.is_empty(), "{answer}");
    assert!(
        findings
            .iter()
            .any(|item| item.to_string().contains("MAX_LINES") || item.to_string().contains("0..=")),
        "{answer}"
    );
    for item in &mut findings {
        item["subject"] = json!({"type":"opaque","kind":"engineering.target","uri":target});
    }
    let usage = session.recorded_usage_tokens();
    assert!(usage.2 > 0);
    let admission = json!({"sessionId":id.0.as_ref(),"scenarioId":"engineering.change",
        "sourceReviewId":"live-review-1","workPolicyDigest":session.work_policy_digest().unwrap().unwrap(),
        "target":{"paths":paths,"digest":target},"scope":{"type":"sessionTarget"},"findings":findings});
    let admitted = ext(&host, "a3s/review/admit", admission.clone())
        .await
        .unwrap();
    assert_eq!(admitted["admitted"], findings.len());
    let stored = session.session_review_store();
    assert_eq!(stored.findings.len(), findings.len());
    std::fs::write(
        root.path().join("src/budget.rs"),
        "pub const MAX_LINES: usize = 3;\n\npub fn first_lines(lines: &[&str]) -> Vec<String> {\n    lines.iter().take(MAX_LINES).map(|line| line.to_string()).collect()\n}\n",
    )
    .unwrap();
    let mut later = admission;
    later["sourceReviewId"] = json!("live-review-2");
    assert!(ext(&host, "a3s/review/admit", later).await.is_err());
    assert_eq!(session.session_review_store(), stored);
    assert_eq!(
        session.recorded_usage_tokens(),
        usage,
        "admission calls no model"
    );
    eprintln!(
        "REVIEW-ADMIT-LIVE elapsed_ms={} usage={usage:?} findings={} answer_tail={:?}",
        started.elapsed().as_millis(),
        findings.len(),
        answer
            .chars()
            .rev()
            .take(400)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    );
    session.close().await;
    drop(host);
    let next = A3sCodeAgent::new(launch);
    next.load_session(
        acp::LoadSessionRequest::new(id.clone(), root.path()).meta(engineering_meta()),
    )
    .await
    .unwrap();
    let reopened = next
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert_eq!(reopened.session_review_store(), stored);
    assert_eq!(
        reopened.pending_session_review_findings().len(),
        findings.len()
    );
    assert_eq!(
        reopened.recorded_usage_tokens(),
        usage,
        "reopen runs no review"
    );
    reopened.close().await;
}

#[tokio::test]
async fn admitted_findings_are_saved_and_a_new_host_reregisters_explicit_scenarios() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("lib.rs"), "pub fn f() {}\n").unwrap();
    let mut launch = super::test_launch(Some(root.path().join("store")));
    launch.workspace = root.path().into();
    let meta = engineering_meta();
    let id = policy_session(&launch, "review-reopen").await;
    let host = A3sCodeAgent::new(launch.clone());
    host.load_session(acp::LoadSessionRequest::new(id.clone(), root.path()).meta(meta.clone()))
        .await
        .unwrap();
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    let scenarios = ["engineering.change", "engineering.workspace"];
    assert!(scenarios
        .iter()
        .all(|id| session.review_scenario_ids().contains(&id.to_string())));
    let registry = session.review_scenario_registry();
    assert!(scenarios
        .iter()
        .all(|id| registry.get(id).unwrap().triggers()
            == a3s_code_core::session_review::ReviewTriggerPolicy::ExplicitRequest));
    let paths = json!(["lib.rs"]);
    let target = ext(
        &host,
        "a3s/review/target",
        json!({"sessionId":id.0.as_ref(),"paths":paths}),
    )
    .await
    .unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let admission = json!({"sessionId":id.0.as_ref(),"scenarioId":"engineering.workspace",
        "sourceReviewId":"audit-1","workPolicyDigest":session.work_policy_digest().unwrap().unwrap(),
        "target":{"paths":paths,"digest":target},"scope":{"type":"sessionTarget"},
        "findings":[finding("a-1","Public function lacks a test",&target),finding("a-2","Unused export",&target)]});
    assert_eq!(
        ext(&host, "a3s/review/admit", admission.clone())
            .await
            .unwrap(),
        json!({"admitted":2})
    );
    let pending = |session: &a3s_code_core::AgentSession| {
        session
            .pending_session_review_findings()
            .into_iter()
            .map(|finding| finding.finding_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(pending(&session), ["a-1", "a-2"]);
    // No explicit save: the admission itself persisted through the original save path.
    let saved = session.session_review_store();
    let usage = session.recorded_usage_tokens();
    session.close().await;
    drop(host);

    for (with_policy, expected) in [
        (false, vec![]),
        (true, vec!["a-1".to_string(), "a-2".to_string()]),
    ] {
        let next = A3sCodeAgent::new(launch.clone());
        let request = acp::LoadSessionRequest::new(id.clone(), root.path());
        next.load_session(if with_policy {
            request.meta(meta.clone())
        } else {
            request
        })
        .await
        .unwrap();
        let reopened = next
            .sessions
            .lock()
            .await
            .get(id.0.as_ref())
            .unwrap()
            .session
            .clone();
        assert_eq!(reopened.session_review_store(), saved);
        assert_eq!(pending(&reopened), expected, "policy {with_policy}");
        assert_eq!(reopened.recorded_usage_tokens(), usage, "no review ran");
        if with_policy {
            assert_eq!(
                ext(&next, "a3s/review/admit", admission.clone())
                    .await
                    .unwrap(),
                json!({"admitted":0}),
                "redelivery after reopen"
            );
            assert_eq!(reopened.session_review_store(), saved);
        }
        reopened.close().await;
    }
}

#[tokio::test]
async fn review_admission_binds_session_policy_and_current_target_bytes_without_a_model() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("src/budget.rs"),
        "pub fn lines() {\n    for _ in 0..=MAX_LINES {}\n}\n",
    )
    .unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "mod budget;\n").unwrap();
    let mut launch = super::test_launch(Some(root.path().join("store")));
    launch.workspace = root.path().into();
    let id = policy_session(&launch, "review-hermetic").await;
    let host = A3sCodeAgent::new(launch);
    host.load_session(
        acp::LoadSessionRequest::new(id.clone(), root.path()).meta(engineering_meta()),
    )
    .await
    .unwrap();
    let plain = host
        .new_session(acp::NewSessionRequest::new(root.path()))
        .await
        .unwrap()
        .session_id;
    let fresh = host
        .new_session(acp::NewSessionRequest::new(root.path()).meta(engineering_meta()))
        .await
        .unwrap()
        .session_id;
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    let paths = json!(["src/lib.rs", "src/budget.rs"]);
    let target = ext(
        &host,
        "a3s/review/target",
        json!({"sessionId":id.0.as_ref(),"paths":paths}),
    )
    .await
    .unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        ext(
            &host,
            "a3s/review/target",
            json!({"sessionId":id.0.as_ref(),"paths":["src/budget.rs","src/lib.rs"]})
        )
        .await
        .unwrap()["digest"],
        target.as_str(),
        "path order does not change the target"
    );
    let policy = session.work_policy_digest().unwrap().unwrap();
    let fresh_session = host
        .sessions
        .lock()
        .await
        .get(fresh.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert_eq!(
        fresh_session.work_policy_digest().unwrap().as_deref(),
        Some(policy.as_str()),
        "a policy session admits its digest before its first Run"
    );
    let admission = json!({"sessionId":id.0.as_ref(),"scenarioId":"engineering.change",
        "sourceReviewId":"review-1","workPolicyDigest":policy,
        "target":{"paths":paths,"digest":target},"scope":{"type":"sessionTarget"},
        "findings":[finding("f-1","Off by one: 0..=MAX_LINES runs MAX_LINES+1 times",&target)]});
    let usage = session.recorded_usage_tokens();
    let admit = |params: Value| ext(&host, "a3s/review/admit", params);

    let mut refusals = Vec::new();
    let mut variant = |edit: &dyn Fn(&mut Value)| {
        let mut params = admission.clone();
        edit(&mut params);
        refusals.push(params);
    };
    variant(&|p| p["sessionId"] = json!(plain.0.as_ref()));
    variant(&|p| p["workPolicyDigest"] = json!(format!("sha256:{}", "0".repeat(64))));
    variant(&|p| p["scenarioId"] = json!("reply.transcript"));
    variant(&|p| p["findings"] = json!([]));
    variant(&|p| p["findings"][0]["subject"]["uri"] = json!("sha256:other"));
    variant(&|p| p["findings"][0]["unknown"] = json!(true));
    variant(&|p| p["unknown"] = json!(true));
    variant(&|p| p["target"]["digest"] = json!(format!("sha256:{}", "1".repeat(64))));
    variant(&|p| p["target"]["paths"] = json!(["src/lib.rs"]));
    variant(&|p| p["target"]["paths"] = json!(["../outside.rs"]));
    variant(&|p| p["target"]["paths"] = json!(["/etc/hosts"]));
    variant(&|p| {
        p["scope"] =
            json!({"type":"goalStep","goalId":"g","goalRevision":1,"planRevision":1,"stepId":"s"})
    });
    variant(
        &|p| p["scope"] = json!({"type":"goalStep","goalId":"g","goalRevision":1,"planRevision":1,"stepId":"s","runId":"r"}),
    );
    variant(&|p| p["scope"] = json!({"type":"unknown"}));
    variant(&|p| {
        p.as_object_mut().unwrap().remove("scope");
    });
    for (index, params) in refusals.into_iter().enumerate() {
        assert!(admit(params).await.is_err(), "variant {index}");
        assert!(
            session.session_review_store().findings.is_empty(),
            "variant {index}"
        );
    }
    assert!(ext(
        &host,
        "a3s/review/admit",
        json!({"sessionId":"unknown-session"})
    )
    .await
    .is_err());

    assert_eq!(
        admit(admission.clone()).await.unwrap(),
        json!({"admitted":1})
    );
    assert_eq!(
        admit(admission.clone()).await.unwrap(),
        json!({"admitted":0}),
        "identical redelivery"
    );
    let stored = session.session_review_store();
    assert_eq!(stored.findings[0].scenario_id, "engineering.change");
    assert_eq!(stored.findings[0].source_review_id, "review-1");
    let mut conflict = admission.clone();
    conflict["findings"][0]["claim"] = json!("different content");
    assert!(admit(conflict).await.is_err());
    assert_eq!(session.session_review_store(), stored);

    std::fs::write(
        root.path().join("src/budget.rs"),
        "pub fn lines() {\n    for _ in 0..MAX_LINES {}\n}\n",
    )
    .unwrap();
    let mut stale = admission.clone();
    stale["findings"][0] = finding("f-2", "Later report on the old bytes", &target);
    assert!(
        admit(stale).await.is_err(),
        "edited target makes the review stale"
    );
    assert_eq!(session.session_review_store(), stored);
    assert_eq!(session.recorded_usage_tokens(), usage, "no model call");

    let settle = |finding: &str, digest: &str, scope: &Value, action: Value| {
        ext(
            &host,
            "a3s/review/settle",
            json!({"sessionId":id.0.as_ref(),"findingId":finding,"targetDigest":digest,
                "scope":scope,"action":action}),
        )
    };
    let session_target = json!({"type":"sessionTarget"});
    let goal_step = json!({"type":"goalStep","goalId":"g","goalRevision":1,"planRevision":1,
        "stepId":"s","runId":"r"});
    let address = json!({"type":"address","runId":"run-fix"});
    let other_target = format!("sha256:{}", "2".repeat(64));
    for (index, (finding, digest, scope, action)) in [
        ("f-1", target.as_str(), &goal_step, address.clone()),
        (
            "f-1",
            other_target.as_str(),
            &session_target,
            address.clone(),
        ),
        (
            "f-unknown",
            target.as_str(),
            &session_target,
            address.clone(),
        ),
        (
            "f-1",
            target.as_str(),
            &session_target,
            json!({"type":"accept","reviewId":"r-2"}),
        ),
        (
            "f-1",
            target.as_str(),
            &session_target,
            json!({"type":"address"}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            settle(finding, digest, scope, action).await.is_err(),
            "settle {index}"
        );
        assert_eq!(session.session_review_store(), stored, "settle {index}");
    }
    assert_eq!(
        settle("f-1", &target, &session_target, address)
            .await
            .unwrap(),
        json!({"status":"addressed"})
    );
    assert_eq!(
        settle(
            "f-1",
            &target,
            &session_target,
            json!({"type":"accept","reviewId":"r-2"})
        )
        .await
        .unwrap(),
        json!({"status":"accepted"})
    );
    assert!(session.session_review_store().findings[0]
        .status
        .is_terminal());
    for id in [&id, &plain, &fresh] {
        host.sessions
            .lock()
            .await
            .get(id.0.as_ref())
            .unwrap()
            .session
            .close()
            .await;
    }
    assert!(admit(admission).await.is_err(), "closed session");
}
