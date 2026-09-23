use super::execution_state::ExecutionLoopState;
use super::tool_result_runtime::{push_tool_result_message, NormalizedToolResult};
use super::{AgentEvent, AgentLoop};
use crate::llm::ToolCall;
use std::time::Instant;
use tokio::sync::mpsc;

pub(super) struct ToolCompletionInput<'a> {
    pub(super) tool_call: &'a ToolCall,
    pub(super) event_tx: &'a Option<mpsc::Sender<AgentEvent>>,
    pub(super) session_id: Option<&'a str>,
    pub(super) tool_start: Instant,
    pub(super) normalized: NormalizedToolResult,
}

impl AgentLoop {
    pub(super) async fn complete_tool_call(
        &self,
        state: &mut ExecutionLoopState,
        input: ToolCompletionInput<'_>,
    ) {
        let ToolCompletionInput {
            tool_call,
            event_tx,
            session_id,
            tool_start,
            mut normalized,
        } = input;
        let tool_duration = tool_start.elapsed();
        crate::telemetry::record_tool_result(normalized.exit_code, tool_duration);
        state.mutations.observe_tool(
            &tool_call.name,
            normalized.exit_code,
            normalized.metadata.as_ref(),
        );
        if let Some(metadata) = normalized.metadata.as_ref() {
            if let Some(path) = metadata.get("file_path").and_then(|path| path.as_str()) {
                state.targeted_paths.push(path.to_string());
            }
            if let Some(paths) = metadata
                .get("changed_paths")
                .and_then(|paths| paths.as_array())
            {
                for path in paths.iter().filter_map(|path| path.as_str()) {
                    state.targeted_paths.push(path.to_string());
                }
            }
        }
        if let Some(path) = tool_call
            .args
            .get("file_path")
            .and_then(|path| path.as_str())
        {
            state.targeted_paths.push(path.to_string());
        }
        self.attach_mutation_observation(tool_call, &mut normalized)
            .await;
        Self::collect_verification_report(&mut state.verification_reports, &normalized.metadata);
        self.bind_or_synthesize_mutation_path_verification(state, tool_call, &normalized);

        let output = normalized.output.clone();

        state.remember_tool_signature(&tool_call.name, &tool_call.args, normalized.is_error);

        self.config.rl_trajectory_recorder.record_tool_result(
            session_id.unwrap_or(""),
            state.current_turn(),
            &tool_call.id,
            &tool_call.name,
            &output,
            normalized.exit_code,
            tool_duration.as_millis() as u64,
            &normalized.metadata,
            normalized
                .error_kind
                .as_ref()
                .map(|kind| format!("{kind:?}")),
        );

        if let Some(tx) = event_tx {
            tx.send(AgentEvent::ToolEnd {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                args: Some(tool_call.args.clone()),
                output: output.clone(),
                exit_code: normalized.exit_code,
                metadata: normalized.metadata.clone(),
                error_kind: normalized.error_kind.clone(),
            })
            .await
            .ok();
        }

        push_tool_result_message(
            state,
            &tool_call.id,
            &output,
            normalized.is_error,
            normalized.images,
            normalized.trust,
            normalized.redaction_reviewed,
        );
    }

    /// Record one fact-log tool result on the completion ledger.
    ///
    /// The tool body already ran. This attaches the mutation observation the
    /// model and `ToolEnd` see, then binds a host shell check to the ledger
    /// digest. It does not choose the next model call.
    pub(crate) async fn record_fact_tool_effect(
        &self,
        name: &str,
        args: &serde_json::Value,
        exit_code: i32,
        output: &mut String,
        metadata: &mut Option<serde_json::Value>,
        ledger: &mut crate::harness_loop::MutationLedger,
        reports: &mut Vec<crate::verification::VerificationReport>,
    ) {
        ledger.observe_tool(name, exit_code, metadata.as_ref());
        let mut normalized = NormalizedToolResult {
            output: output.clone(),
            exit_code,
            is_error: exit_code != 0,
            metadata: metadata.clone(),
            images: Vec::new(),
            error_kind: None,
            trust: crate::llm::ToolResultTrustV1::Trusted,
            redaction_reviewed: true,
        };
        let tool_call = crate::llm::ToolCall {
            id: String::new(),
            name: name.to_string(),
            args: args.clone(),
        };
        self.attach_mutation_observation(&tool_call, &mut normalized)
            .await;
        Self::collect_verification_report(reports, &normalized.metadata);
        let mut state = ExecutionLoopState::new_seeded(&[], None);
        state.mutations = ledger.clone();
        state.verification_reports = std::mem::take(reports);
        self.bind_or_synthesize_mutation_path_verification(&mut state, &tool_call, &normalized);
        *reports = state.verification_reports;
        *ledger = state.mutations;
        *output = normalized.output;
        *metadata = normalized.metadata;
    }

    fn bind_or_synthesize_mutation_path_verification(
        &self,
        state: &mut ExecutionLoopState,
        tool_call: &ToolCall,
        normalized: &NormalizedToolResult,
    ) {
        if state.mutations.is_empty() {
            return;
        }
        let mutation_paths: Vec<String> = state.mutations.paths().map(str::to_string).collect();
        let digest = state.mutations.digest().to_string();
        let shell_command = normalized
            .metadata
            .as_ref()
            .and_then(|metadata| {
                metadata
                    .get("verification_shell_command")
                    .or_else(|| metadata.get("command"))
                    .and_then(|value| value.as_str())
            })
            .or_else(|| {
                tool_call
                    .args
                    .get("command")
                    .and_then(|value| value.as_str())
            });

        let Some(command) = shell_command else {
            return;
        };
        let checks = crate::verification::existence_checks(command);
        if checks.is_empty() {
            return;
        }
        let mut existence_only = None;
        for check in &checks {
            if !mutation_paths
                .iter()
                .any(|mutated| crate::verification::mutation_path_matches(mutated, &check.path))
            {
                continue;
            }
            if !state.mutations.has_content_digest(&check.path) {
                if existence_only.is_none() {
                    existence_only = Some(check);
                }
                continue;
            }
            // A write records the `after` bytes. A later changed_paths row stores
            // a metadata hash and must not erase that match. Bytes that differ
            // from every recorded digest do not verify.
            let Some(on_disk) = on_disk_content_digest(&self.tool_context.workspace, &check.path)
            else {
                continue;
            };
            if !state
                .mutations
                .content_digest_matches(&check.path, &on_disk)
            {
                continue;
            }
            let pair = (on_disk.as_str(), on_disk.as_str());
            crate::verification::bind_host_shell_reports_to_mutations_with_content(
                &mut state.verification_reports,
                Some(check.segment.as_str()),
                &mutation_paths,
                &digest,
                Some(pair),
            );
            if !tool_call.name.eq_ignore_ascii_case("bash") {
                return;
            }
            if state
                .verification_reports
                .iter()
                .any(|report| report.effect_digest.as_deref() == Some(digest.as_str()))
            {
                return;
            }
            if let Some(report) =
                crate::verification::host_report_for_verified_mutation_path_with_content(
                    check.segment.as_str(),
                    normalized.exit_code,
                    &mutation_paths,
                    &digest,
                    Some(pair),
                )
            {
                state.verification_reports.push(report);
            }
            return;
        }
        // No recorded file bytes: an existing Passed report may still bind to
        // an existence check. Do not synthesize a content verification.
        if let Some(check) = existence_only {
            crate::verification::bind_host_shell_reports_to_mutations_with_content(
                &mut state.verification_reports,
                Some(check.segment.as_str()),
                &mutation_paths,
                &digest,
                None,
            );
        }
    }

    async fn attach_mutation_observation(
        &self,
        tool_call: &ToolCall,
        normalized: &mut NormalizedToolResult,
    ) {
        if normalized.exit_code != 0 {
            return;
        }
        let paths = mutation_observation_paths(&tool_call.name, normalized.metadata.as_ref());
        if paths.is_empty() {
            return;
        }
        for path in paths.into_iter().take(4) {
            let observation = self.mutation_observation_for(&path).await;
            crate::harness_loop::attach_observation(&mut normalized.metadata, &observation);
            normalized.output =
                crate::harness_loop::model_visible_observation(&normalized.output, &observation);
        }
    }

    async fn mutation_observation_for(
        &self,
        path: &str,
    ) -> crate::harness_loop::MutationObservationV1 {
        use crate::harness_loop::MutationObservationV1;
        let Some(provider) = self.tool_context.workspace_services.code_intelligence() else {
            return MutationObservationV1::unavailable(path);
        };
        let workspace_path = crate::workspace::WorkspacePath::from_normalized(path);
        let query = provider.diagnostics(
            Some(&workspace_path),
            self.tool_context.cancellation_token(),
        );
        let result = match tokio::time::timeout(std::time::Duration::from_millis(1500), query).await
        {
            Ok(Ok(result)) => result,
            _ => return MutationObservationV1::unavailable(path),
        };
        if result
            .document
            .as_ref()
            .is_some_and(|document| document.stale)
        {
            return crate::harness_loop::observation_from_diagnostics(
                path,
                Some(result.workspace_revision),
                true,
                Vec::new(),
            );
        }
        let items = result
            .items
            .iter()
            .take(8)
            .map(|item| {
                format!(
                    "{}:{}: {}",
                    item.location.path.as_str(),
                    item.location.range.start.line,
                    item.message
                )
            })
            .collect();
        crate::harness_loop::observation_from_diagnostics(
            path,
            Some(result.workspace_revision),
            false,
            items,
        )
    }
}

/// Paths a mutation result actually identified. Command text is ignored.
fn mutation_observation_paths(
    tool_name: &str,
    metadata: Option<&serde_json::Value>,
) -> Vec<String> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let name = tool_name.to_ascii_lowercase();
    if matches!(name.as_str(), "write" | "edit" | "patch") {
        return metadata
            .get("file_path")
            .and_then(|path| path.as_str())
            .map(|path| vec![path.to_string()])
            .unwrap_or_default();
    }
    let mut paths: Vec<String> = metadata
        .get("changed_paths")
        .and_then(|paths| paths.as_array())
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_string))
                .filter(|path| !path.is_empty())
                .take(8)
                .collect()
        })
        .unwrap_or_default();
    for (name, nested) in crate::harness_loop::nested_tool_calls(metadata) {
        paths.extend(mutation_observation_paths(name, nested));
    }
    paths.truncate(8);
    paths
}

fn on_disk_content_digest(workspace: &std::path::Path, relative: &str) -> Option<String> {
    let relative = relative.trim_start_matches("./");
    if relative.is_empty() {
        return None;
    }
    let path = workspace.join(relative);
    let bytes = std::fs::read(&path).ok()?;
    Some(sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_intelligence::{
        CodeDiagnostic, CodeIntelligenceError, CodeIntelligenceResult, CodeIntelligenceStatus,
        CodeLocation, CodeQueryResult, CodeRange, DocumentSnapshot, DocumentSymbol, NavigationKind,
        SymbolInformation, WorkspaceCodeIntelligence,
    };
    use crate::llm::ToolResultTrustV1;
    use crate::tools::{ToolContext, ToolExecutor};
    use crate::workspace::WorkspacePath;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct KnownDiagnostic {
        message: String,
        stale: bool,
    }

    #[async_trait]
    impl WorkspaceCodeIntelligence for KnownDiagnostic {
        fn subscribe_status(&self) -> tokio::sync::watch::Receiver<CodeIntelligenceStatus> {
            let (sender, receiver) = tokio::sync::watch::channel(CodeIntelligenceStatus::default());
            drop(sender);
            receiver
        }

        async fn document_symbols(
            &self,
            path: &WorkspacePath,
            _: tokio_util::sync::CancellationToken,
        ) -> CodeIntelligenceResult<CodeQueryResult<DocumentSymbol>> {
            Err(CodeIntelligenceError::Unsupported {
                operation: "document_symbols".into(),
                message: path.as_str().to_string(),
            })
        }

        async fn search_symbols(
            &self,
            _: &str,
            _: usize,
            _: tokio_util::sync::CancellationToken,
        ) -> CodeIntelligenceResult<CodeQueryResult<SymbolInformation>> {
            Err(CodeIntelligenceError::Unsupported {
                operation: "search_symbols".into(),
                message: "unused".into(),
            })
        }

        async fn navigate(
            &self,
            _: NavigationKind,
            path: &WorkspacePath,
            _: crate::code_intelligence::CodePosition,
            _: tokio_util::sync::CancellationToken,
        ) -> CodeIntelligenceResult<CodeQueryResult<CodeLocation>> {
            Err(CodeIntelligenceError::Unsupported {
                operation: "navigate".into(),
                message: path.as_str().to_string(),
            })
        }

        async fn diagnostics(
            &self,
            path: Option<&WorkspacePath>,
            _: tokio_util::sync::CancellationToken,
        ) -> CodeIntelligenceResult<CodeQueryResult<CodeDiagnostic>> {
            let path = path
                .cloned()
                .unwrap_or_else(|| WorkspacePath::from_normalized("src/leaked.rs"));
            Ok(CodeQueryResult {
                items: vec![CodeDiagnostic {
                    location: CodeLocation {
                        path: path.clone(),
                        range: CodeRange::default(),
                    },
                    severity: None,
                    code: None,
                    source: Some("rust-analyzer".into()),
                    message: self.message.clone(),
                }],
                truncated: false,
                workspace_revision: 4,
                document: Some(DocumentSnapshot {
                    revision: crate::code_intelligence::DocumentRevision::default(),
                    content_hash: "after-edit".into(),
                    stale: self.stale,
                }),
            })
        }
    }

    fn loop_with(provider: KnownDiagnostic) -> AgentLoop {
        let root = tempfile::tempdir().expect("workspace");
        let services = crate::workspace::WorkspaceServices::local(root.path())
            .with_code_intelligence(Arc::new(provider));
        let context = ToolContext::new(root.path().to_path_buf()).with_workspace_services(services);
        AgentLoop::new(
            Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
            Arc::new(ToolExecutor::new(
                root.path().to_string_lossy().into_owned(),
            )),
            context,
            crate::agent::AgentConfig {
                planning_mode: crate::agent::PlanningMode::Disabled,
                ..crate::agent::AgentConfig::default()
            },
        )
    }

    fn edit_result() -> NormalizedToolResult {
        NormalizedToolResult {
            output: "edited src/leaked.rs".into(),
            exit_code: 0,
            is_error: false,
            metadata: Some(serde_json::json!({"file_path": "src/leaked.rs"})),
            images: Vec::new(),
            error_kind: None,
            trust: ToolResultTrustV1::WorkspaceData,
            redaction_reviewed: true,
        }
    }

    async fn next_model_text(agent: &AgentLoop) -> String {
        let mut state = ExecutionLoopState::new(&[]);
        agent
            .complete_tool_call(
                &mut state,
                ToolCompletionInput {
                    tool_call: &ToolCall {
                        id: "edit-1".into(),
                        name: "edit".into(),
                        args: serde_json::json!({"file_path": "src/leaked.rs"}),
                    },
                    event_tx: &None,
                    session_id: None,
                    tool_start: Instant::now(),
                    normalized: edit_result(),
                },
            )
            .await;
        let text = state
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                crate::llm::ContentBlock::ToolResult {
                    content: crate::llm::ToolResultContentField::Text(text),
                    ..
                } => Some(text.clone()),
                crate::llm::ContentBlock::ToolUse { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains("code_diagnostics"),
            "the diagnostic must arrive without a model code_diagnostics call"
        );
        text
    }

    #[tokio::test]
    async fn edit_diagnostic_is_on_the_next_model_input_without_a_diagnostics_call() {
        let agent = loop_with(KnownDiagnostic {
            message: "unused binding leaked".into(),
            stale: false,
        });
        let text = next_model_text(&agent).await;
        assert!(text.contains("unused binding leaked"));
        assert!(text.contains("src/leaked.rs"));
    }

    #[tokio::test]
    async fn fact_log_tool_effect_attaches_observation_and_binds_a_host_existence_check() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::write(workspace.path().join("hello.txt"), "hello").expect("fixture");
        let executor = Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let agent = AgentLoop::new(
            Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
            executor,
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig::default(),
        );
        let mut ledger = crate::harness_loop::MutationLedger::default();
        let mut reports = Vec::new();
        let mut output = "wrote".to_string();
        let mut metadata = Some(serde_json::json!({
            "file_path": "hello.txt",
            "after": "hello"
        }));
        agent
            .record_fact_tool_effect(
                "write",
                &serde_json::json!({ "file_path": "hello.txt", "content": "hello" }),
                0,
                &mut output,
                &mut metadata,
                &mut ledger,
                &mut reports,
            )
            .await;
        assert!(
            output.contains("[mutation observation]"),
            "tool result missing mutation observation: {output}"
        );
        let schema = metadata
            .as_ref()
            .and_then(|value| value.get("mutation_observation"))
            .and_then(|observation| observation.get("schema"))
            .and_then(|schema| schema.as_str());
        assert!(schema.is_some(), "metadata missing mutation observation");

        let mut bash_output = String::new();
        let mut bash_metadata = Some(serde_json::json!({ "command": "test -f hello.txt" }));
        agent
            .record_fact_tool_effect(
                "bash",
                &serde_json::json!({ "command": "test -f hello.txt" }),
                0,
                &mut bash_output,
                &mut bash_metadata,
                &mut ledger,
                &mut reports,
            )
            .await;
        let gate = agent.fact_completion_gate(&ledger, &reports);
        assert!(
            matches!(
                gate,
                crate::harness_loop::CompletionGate::Allow(
                    crate::harness_loop::CompletionTerminal::Verified { .. }
                )
            ),
            "host existence check should Allow(Verified), got {gate:?} reports={reports:?}"
        );
    }

    #[tokio::test]
    async fn fact_log_compound_host_check_binds_when_write_content_matches() {
        let workspace = tempfile::tempdir().expect("workspace");
        let executor = Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let agent = AgentLoop::new(
            Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
            Arc::clone(&executor),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig::default(),
        );
        let mut ledger = crate::harness_loop::MutationLedger::default();
        let mut reports = Vec::new();
        let write_ctx =
            ToolContext::new(workspace.path().to_path_buf()).with_session_id("fact-log-bind");
        let write = executor
            .execute_with_context(
                "write",
                &serde_json::json!({ "file_path": "hello.txt", "content": "hello" }),
                &write_ctx,
            )
            .await
            .expect("write");
        assert_eq!(write.exit_code, 0, "{}", write.output);
        let mut output = write.output;
        let mut metadata = write.metadata;
        agent
            .record_fact_tool_effect(
                "write",
                &serde_json::json!({ "file_path": "hello.txt", "content": "hello" }),
                write.exit_code,
                &mut output,
                &mut metadata,
                &mut ledger,
                &mut reports,
            )
            .await;
        let bash = executor
            .execute_with_context(
                "bash",
                &serde_json::json!({ "command": "cd . && test -f hello.txt" }),
                &write_ctx,
            )
            .await
            .expect("bash");
        assert_eq!(bash.exit_code, 0, "{}", bash.output);
        let mut bash_metadata = bash.metadata.unwrap_or_else(|| serde_json::json!({}));
        if let Some(object) = bash_metadata.as_object_mut() {
            object.insert(
                "changed_paths".to_string(),
                serde_json::json!(["hello.txt"]),
            );
        }
        let mut bash_output = bash.output;
        let mut bash_metadata = Some(bash_metadata);
        agent
            .record_fact_tool_effect(
                "bash",
                &serde_json::json!({ "command": "cd . && test -f hello.txt" }),
                bash.exit_code,
                &mut bash_output,
                &mut bash_metadata,
                &mut ledger,
                &mut reports,
            )
            .await;
        let gate = agent.fact_completion_gate(&ledger, &reports);
        assert!(
            matches!(
                gate,
                crate::harness_loop::CompletionGate::Allow(
                    crate::harness_loop::CompletionTerminal::Verified { .. }
                )
            ),
            "compound host check should Allow(Verified), got {gate:?} reports={reports:?}"
        );
    }

    #[tokio::test]
    async fn fact_log_bash_write_and_existence_check_does_not_self_verify() {
        let workspace = tempfile::tempdir().expect("workspace");
        let executor = Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let agent = AgentLoop::new(
            Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
            Arc::clone(&executor),
            ToolContext::new(workspace.path().to_path_buf()),
            crate::agent::AgentConfig::default(),
        );
        let command = "printf 'hello' > hello.txt && test -f hello.txt";
        let bash = executor
            .execute("bash", &serde_json::json!({ "command": command }))
            .await
            .expect("bash");
        assert_eq!(bash.exit_code, 0, "{}", bash.output);
        assert!(
            workspace.path().join("hello.txt").is_file(),
            "bash did not create hello.txt"
        );
        let mut ledger = crate::harness_loop::MutationLedger::default();
        let mut reports = Vec::new();
        let mut output = bash.output;
        let mut metadata = bash.metadata;
        agent
            .record_fact_tool_effect(
                "bash",
                &serde_json::json!({ "command": command }),
                bash.exit_code,
                &mut output,
                &mut metadata,
                &mut ledger,
                &mut reports,
            )
            .await;
        assert!(
            !ledger.is_empty(),
            "bash write must land on the mutation ledger, metadata={metadata:?}"
        );
        let gate = agent.fact_completion_gate(&ledger, &reports);
        assert!(
            matches!(gate, crate::harness_loop::CompletionGate::Incomplete { .. }),
            "a bash write must not verify itself with test -f, got {gate:?} reports={reports:?}"
        );
    }

    #[tokio::test]
    async fn fact_log_gate_keeps_an_open_external_observation() {
        let observation = crate::external_observation::ExternalObservationV1::new(
            "ci",
            "pipeline",
            "obs-live-ci",
            "the required check is still red",
            crate::external_observation::RequiredAction::WorkspaceChange,
        )
        .expect("observation");
        let workspace = tempfile::tempdir().expect("workspace");
        let executor = Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().into_owned(),
        ));
        let mut config = crate::agent::AgentConfig::default();
        config.external_observations = vec![observation];
        let agent = AgentLoop::new(
            Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
            executor,
            ToolContext::new(workspace.path().to_path_buf()),
            config,
        );
        let gate = agent.fact_completion_gate(&crate::harness_loop::MutationLedger::default(), &[]);
        match gate {
            crate::harness_loop::CompletionGate::Incomplete { message } => {
                assert!(
                    message.contains("completion gate: external observation obs-live-ci"),
                    "{message}"
                );
            }
            other => panic!("open observation must stay incomplete, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pre_edit_revision_diagnostic_is_absent_from_the_next_model_input() {
        let agent = loop_with(KnownDiagnostic {
            message: "previous revision".into(),
            stale: true,
        });
        let text = next_model_text(&agent).await;
        assert!(!text.contains("previous revision"));
        assert!(text.contains("stale"));
    }
}
