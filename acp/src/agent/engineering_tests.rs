#![cfg(test)]

use super::{A3sCodeAgent, SessionSetup};
use a3s_code_core::{
    llm::{LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition},
    WorkPolicyV1,
};
use agent_client_protocol::{self as acp, Agent as _};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn new_sessions_default_to_engineering_and_reopen_preserves_selection() {
    for disabled in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut launch = super::test_launch(Some(root.path().join("store")));
        launch.workspace = root.path().into();
        let host = A3sCodeAgent::new(launch.clone());
        let mut request = acp::NewSessionRequest::new(root.path());
        if disabled {
            request = request.meta(
                serde_json::json!({"a3sEngineering":{"policy":"none"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
        let id = host.new_session(request).await.unwrap().session_id;
        let session = host
            .sessions
            .lock()
            .await
            .get(id.0.as_ref())
            .unwrap()
            .session
            .clone();
        let expected = if disabled {
            None
        } else {
            Some(
                crate::engineering::named_policy("engineering-v1")
                    .unwrap()
                    .digest()
                    .unwrap(),
            )
        };
        assert_eq!(session.work_policy_digest().unwrap(), expected);
        assert!(
            session.goal_definition().unwrap().is_none(),
            "an ordinary session creates no goal"
        );
        session.save().await.unwrap();
        session.close().await;
        drop(session);
        drop(host);
        let reopened = A3sCodeAgent::new(launch);
        reopened
            .load_session(acp::LoadSessionRequest::new(id.clone(), root.path()))
            .await
            .unwrap();
        let session = reopened
            .sessions
            .lock()
            .await
            .get(id.0.as_ref())
            .unwrap()
            .session
            .clone();
        assert_eq!(session.work_policy_digest().unwrap(), expected);
        assert!(session.goal_definition().unwrap().is_none());
        session.close().await;
    }
}

fn selected_meta() -> acp::Meta {
    serde_json::json!({"a3sEngineering":{"policy":"engineering-v1"},
        "a3sHost":{"role":"ENGINEERING-HOST-ROLE","guidelines":"KEEP-HOST-GUIDANCE","outputLanguage":"zh-CN"}})
        .as_object().unwrap().clone()
}

#[derive(Default)]
pub(super) struct CapturingClient {
    inputs: Mutex<Vec<String>>,
}

#[async_trait]
impl LlmClient for CapturingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.inputs
            .lock()
            .unwrap()
            .push(system.unwrap_or_default().into());
        Ok(LlmResponse {
            message: Message::assistant("ready"),
            usage: TokenUsage::default(),
            stop_reason: Some("end_turn".into()),
            token_logprobs: vec![],
            meta: None,
        })
    }
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.complete(messages, system, tools).await?;
        let (sender, receiver) = mpsc::channel(1);
        sender.send(StreamEvent::Done(response)).await.unwrap();
        Ok(receiver)
    }
}

struct PromptBudget {
    limit: usize,
    estimates: Mutex<Vec<usize>>,
}
#[async_trait]
impl a3s_code_core::budget::BudgetGuard for PromptBudget {
    async fn check_before_llm(
        &self,
        _: &str,
        estimate: usize,
    ) -> a3s_code_core::budget::BudgetDecision {
        self.estimates.lock().unwrap().push(estimate);
        if estimate > self.limit {
            a3s_code_core::budget::BudgetDecision::Deny {
                resource: "prompt_tokens".into(),
                reason: "original whole-request budget".into(),
            }
        } else {
            a3s_code_core::budget::BudgetDecision::Allow
        }
    }
}

#[tokio::test]
async fn engineering_unknown_meta_refuses_new_and_load_before_activation() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = super::test_launch(Some(root.path().join("store")));
    launch.workspace = root.path().into();
    let host = A3sCodeAgent::new(launch);
    let meta = serde_json::json!({"a3sEngineering":{"policy":"unknown"}})
        .as_object()
        .unwrap()
        .clone();
    assert!(host
        .new_session(acp::NewSessionRequest::new(root.path()).meta(meta.clone()))
        .await
        .is_err());
    assert!(host
        .load_session(acp::LoadSessionRequest::new("missing", root.path()).meta(meta))
        .await
        .is_err());
    assert!(host.core_agent.lock().await.is_none());
    assert!(host.sessions.lock().await.is_empty());
    assert!(!root.path().join(".a3s").exists());
    assert!(!root.path().join("store").exists());
}

#[tokio::test]
async fn engineering_shared_host_options_preserve_policy_and_contracts_after_file_reopen() {
    use a3s_code_core::{
        config::WorkspaceVerificationContract, verification::VerificationContractV1,
    };
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    let mut config = super::test_launch(Some(store.clone())).config;
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    config.verification_contracts = vec![WorkspaceVerificationContract {
        workspace: root.path().into(),
        contract: VerificationContractV1::new("host-proof", "python3 gate.py", ["src/**"]).unwrap(),
    }];
    let client = Arc::new(CapturingClient::default());
    let budget = Arc::new(PromptBudget {
        limit: usize::MAX,
        estimates: Mutex::new(vec![]),
    });
    let policy = crate::engineering::policy_from_meta(Some(&selected_meta()))
        .unwrap()
        .unwrap();
    for reopened in [false, true] {
        let agent = a3s_code_core::Agent::from_config(config.clone())
            .await
            .unwrap();
        let mut meta = selected_meta();
        if reopened {
            meta.remove("a3sEngineering");
        }
        let setup = SessionSetup {
            workspace: root.path().into(),
            model_id: config.default_model.as_deref().unwrap(),
            effort: super::effort_limits("high"),
            host: crate::host_context::studio_host_from_meta(Some(&meta)).unwrap(),
            components: &[],
            mcp_servers: vec![],
        };
        let options =
            A3sCodeAgent::host_session_options("engineering-file", &setup, &config, &store)
                .unwrap()
                .with_llm_client(client.clone())
                .with_budget_guard(budget.clone())
                .with_auto_save(true)
                .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_continuation(false);
        assert_eq!(options.work_policy.is_some(), !reopened);
        assert_eq!(options.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(options.verification_contracts.len(), 1);
        assert!(
            !options
                .prompt_slots
                .as_ref()
                .unwrap()
                .build()
                .contains(policy.instructions()),
            "host must not append policy twice"
        );
        let session = if reopened {
            agent
                .resume_session_async("engineering-file", options)
                .await
                .unwrap()
        } else {
            agent
                .session_async(root.path().to_string_lossy(), Some(options))
                .await
                .unwrap()
        };
        session
            .send("Reply ready without tools.", None)
            .await
            .unwrap();
        session.save().await.unwrap();
        session.close().await;
    }
    let inputs = client.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 2);
    for system in inputs.iter() {
        assert_eq!(system.matches(policy.instructions()).count(), 1);
        for text in [
            "ENGINEERING-HOST-ROLE",
            "KEEP-HOST-GUIDANCE",
            "zh-CN",
            "thinking_budget=",
            super::SESSION_OUTCOME_BOUNDARY,
            "python3 gate.py",
        ] {
            assert!(system.contains(text), "missing {text}");
        }
        assert!(budget
            .estimates
            .lock()
            .unwrap()
            .iter()
            .any(|estimate| *estimate > 0));
    }
    let facts =
        a3s_code_core::fact_control::read_workspace_facts(root.path(), "engineering-file").unwrap();
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.kind == "a3s.work-policy.admitted")
            .count(),
        1
    );
    assert!(!facts.iter().any(|f| f.kind.starts_with("a3s.goal.")));
}

#[tokio::test]
async fn engineering_original_budget_and_legacy_rebind_refuse_before_provider() {
    for legacy in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        let mut config = super::test_launch(Some(store.clone())).config;
        config.memory = Some(a3s_code_core::memory::MemoryConfig {
            llm_extraction: false,
            prune_policy: None,
            ..Default::default()
        });
        let client = Arc::new(CapturingClient::default());
        let setup = SessionSetup {
            workspace: root.path().into(),
            model_id: config.default_model.as_deref().unwrap(),
            effort: None,
            host: crate::host_context::studio_host_from_meta(Some(&selected_meta())).unwrap(),
            components: &[],
            mcp_servers: vec![],
        };
        let options =
            A3sCodeAgent::host_session_options("refuse-engineering", &setup, &config, &store)
                .unwrap()
                .with_llm_client(client.clone())
                .with_auto_save(true)
                .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_continuation(false);
        if legacy {
            let agent = a3s_code_core::Agent::from_config(config.clone())
                .await
                .unwrap();
            let mut plain = options.clone();
            plain.work_policy = None;
            let session = agent
                .session_async(root.path().to_string_lossy(), Some(plain))
                .await
                .unwrap();
            session.send("Reply ready.", None).await.unwrap();
            session.save().await.unwrap();
            session.close().await;
            drop(agent);
            client.inputs.lock().unwrap().clear();
        }
        let budget = Arc::new(PromptBudget {
            limit: 0,
            estimates: Mutex::new(vec![]),
        });
        let options = if legacy {
            options
        } else {
            options.with_budget_guard(budget.clone())
        };
        let agent = a3s_code_core::Agent::from_config(config).await.unwrap();
        if legacy {
            let Err(error) = agent
                .resume_session_async("refuse-engineering", options)
                .await
            else {
                panic!("a legacy session refuses a new policy at reopen");
            };
            let error = error.to_string();
            assert!(error.contains("work_policy.legacy_rebind"), "{error}");
            assert!(client.inputs.lock().unwrap().is_empty());
            continue;
        }
        let session = agent
            .session_async(root.path().to_string_lossy(), Some(options))
            .await
            .unwrap();
        let error = session
            .send("Reply ready.", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("original whole-request budget"), "{error}");
        assert!(client.inputs.lock().unwrap().is_empty());
        assert!(budget
            .estimates
            .lock()
            .unwrap()
            .iter()
            .any(|estimate| *estimate > 0));
        session.close().await;
    }
}

#[tokio::test]
async fn engineering_explicit_conflict_refuses_acp_load_without_changing_source() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    let mut launch = super::test_launch(Some(store.clone()));
    launch.workspace = root.path().into();
    launch.config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let id = "engineering-conflict";
    a3s_code_core::session_folder::prepare_session_home(root.path(), id).unwrap();
    let client = Arc::new(CapturingClient::default());
    let agent = a3s_code_core::Agent::from_config(launch.config.clone())
        .await
        .unwrap();
    let session = agent
        .session_async(
            root.path().to_string_lossy(),
            Some(
                a3s_code_core::SessionOptions::new()
                    .with_session_id(id)
                    .with_file_session_store(&store)
                    .with_auto_save(true)
                    .with_llm_client(client)
                    .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_continuation(false)
                    .with_work_policy(
                        WorkPolicyV1::new(
                            "previous-host-policy",
                            1,
                            "Preserve the previous host policy.",
                        )
                        .unwrap(),
                    ),
            ),
        )
        .await
        .unwrap();
    session.send("Reply ready.", None).await.unwrap();
    session.save().await.unwrap();
    session.close().await;
    drop(agent);
    let before = a3s_code_core::fact_control::read_workspace_facts(root.path(), id).unwrap();
    let host = A3sCodeAgent::new(launch);
    let error = host
        .load_session(acp::LoadSessionRequest::new(id, root.path()).meta(selected_meta()))
        .await
        .unwrap_err();
    assert!(
        format!("{error:?}").contains("work_policy.conflict"),
        "{error:?}"
    );
    assert!(host.sessions.lock().await.is_empty());
    assert_eq!(
        before,
        a3s_code_core::fact_control::read_workspace_facts(root.path(), id).unwrap()
    );
}

#[tokio::test]
#[ignore = "requires A3S_CONFIG_FILE and the live boyue/deepseek-v4-flash model"]
async fn engineering_real_model_acp_new_model_replacement_and_new_host_reopen() {
    let root = tempfile::tempdir().unwrap();
    let config = std::env::var_os("A3S_CONFIG_FILE").expect("explicit live ACL path");
    let mut launch =
        crate::config::load_launch_config(root.path().into(), Some(config.into())).unwrap();
    launch.model_id = "boyue/deepseek-v4-flash".into();
    launch.config.default_model = Some(launch.model_id.clone());
    launch.config.sessions_dir = Some(root.path().join("store"));
    let model_id = launch.model_id.clone();
    let first = A3sCodeAgent::new(launch.clone());
    let id = first
        .new_session(acp::NewSessionRequest::new(root.path()).meta(selected_meta()))
        .await
        .unwrap()
        .session_id;
    let started = std::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_secs(180), first.prompt(acp::PromptRequest::new(id.clone(), vec![acp::ContentBlock::Text(acp::TextContent::new(
        "Quote verbatim the single sentence in your engineering discipline that begins with Reuse. Do not use tools."))])))
        .await.unwrap().unwrap();
    let session = first
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    let answer = session
        .history()
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .unwrap()
        .text();
    assert!(answer.contains("Reuse the existing project path first, then the standard library, then an installed dependency."), "{answer}");
    let usage = session.recorded_usage_tokens();
    assert!(usage.2 > 0);
    eprintln!(
        "engineering real new elapsed_ms={} cumulative_tokens={usage:?}",
        started.elapsed().as_millis()
    );
    first
        .set_session_model(acp::SetSessionModelRequest::new(id.clone(), model_id))
        .await
        .unwrap();
    let replaced = first
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert_eq!(replaced.recorded_usage_tokens(), usage);
    replaced.save().await.unwrap();
    replaced.close().await;
    drop(first);
    let next = A3sCodeAgent::new(launch);
    let mut meta = selected_meta();
    meta.remove("a3sEngineering");
    next.load_session(acp::LoadSessionRequest::new(id.clone(), root.path()).meta(meta))
        .await
        .unwrap();
    let session = next
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert_eq!(session.recorded_usage_tokens(), usage);
    assert!(
        !session
            .history()
            .iter()
            .any(|m| m.text().contains("Guidance does not grant authority")),
        "the second answer must not already occur in conversation history"
    );
    let started = std::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_secs(180), next.prompt(acp::PromptRequest::new(id.clone(), vec![acp::ContentBlock::Text(acp::TextContent::new(
        "Quote verbatim the single sentence in your engineering discipline that begins with Guidance. Do not use tools."))])))
        .await.unwrap().unwrap();
    let answer = session
        .history()
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .unwrap()
        .text();
    assert!(
        answer.contains("Guidance does not grant authority or waive a check."),
        "{answer}"
    );
    assert!(session.recorded_usage_tokens().2 > usage.2);
    let policy = crate::engineering::policy_from_meta(Some(&selected_meta()))
        .unwrap()
        .unwrap();
    let facts =
        a3s_code_core::fact_control::read_workspace_facts(root.path(), id.0.as_ref()).unwrap();
    let admitted: Vec<_> = facts
        .iter()
        .filter(|f| f.kind == "a3s.work-policy.admitted")
        .collect();
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].payload["digest"], policy.digest().unwrap());
    assert!(session.goal_definition().unwrap().is_none());
    eprintln!(
        "engineering real reopened elapsed_ms={} cumulative_tokens={:?} policy_digest={}",
        started.elapsed().as_millis(),
        session.recorded_usage_tokens(),
        policy.digest().unwrap()
    );
    session.save().await.unwrap();
    session.close().await;
}
