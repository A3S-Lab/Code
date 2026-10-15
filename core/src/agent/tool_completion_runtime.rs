use super::execution_state::ExecutionLoopState;
use super::tool_result_runtime::{push_tool_result_message, NormalizedToolResult};
use super::{AgentEvent, AgentLoop};
use crate::llm::ToolCall;
use std::time::Instant;
use tokio::sync::mpsc;

fn admit_surface_form(
    session_id: Option<&str>,
    tool_id: &str,
    prompt: &str,
) -> Option<crate::ask_user::UserQuestionV1> {
    let run_id = session_id.unwrap_or("run");
    let question_id = format!("surface-{run_id}-{tool_id}");
    crate::ask_user::begin(run_id, &question_id, prompt, &[], true).ok()
}

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
        normalize_mutation_metadata_path(
            &self.tool_context.workspace,
            normalized.metadata.as_mut(),
        );
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
        self.record_current_call_verification(
            tool_call,
            &mut normalized,
            &mut state.mutations,
            &mut state.verification_reports,
        );

        let output = normalized.output.clone();
        let mut model_output = output.clone();

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

        if normalized.exit_code == 0 {
            if let Some(decision) = crate::surface_decision::decide_tool_surface(
                &output,
                self.llm_client.gateway_decision_auth(),
            )
            .await
            {
                if let Some(tx) = event_tx {
                    let id = format!("apofasi-{}", tool_call.id);
                    tx.send(AgentEvent::ToolExecutionStart {
                        id: id.clone(),
                        name: "apofasi".to_owned(),
                        args: decision.args.clone(),
                    })
                    .await
                    .ok();
                    tx.send(AgentEvent::ToolEnd {
                        id,
                        name: "apofasi".to_owned(),
                        args: Some(decision.args),
                        output: decision.output,
                        exit_code: decision.exit_code,
                        metadata: None,
                        error_kind: None,
                    })
                    .await
                    .ok();
                }
                if let Some(form) = decision.form {
                    if let Some(tx) = event_tx {
                        let mut parked = false;
                        if form.interactive {
                            if let Some(question) =
                                admit_surface_form(session_id, &tool_call.id, &form.prompt)
                            {
                                parked = tx
                                    .send(AgentEvent::UserQuestion {
                                        question_id: question.question_id,
                                        question: question.question,
                                        options: question.options,
                                        allow_free_text: question.allow_free_text,
                                        form: Some(form.openui.clone()),
                                    })
                                    .await
                                    .is_ok();
                            }
                        }
                        if !parked {
                            parked = tx
                                .send(AgentEvent::TextDelta {
                                    text: format!("\n```openui\n{}\n```\n", form.openui),
                                })
                                .await
                                .is_ok();
                        }
                        if parked {
                            model_output = form.model_note;
                            if form.interactive {
                                state.surface_form_parked = true;
                            }
                        }
                    }
                } else if let Some(note) = decision.model_note {
                    model_output = note;
                }
            }
        }

        push_tool_result_message(
            state,
            &tool_call.id,
            &model_output,
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
    #[allow(clippy::too_many_arguments)]
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
        normalize_mutation_metadata_path(&self.tool_context.workspace, metadata.as_mut());
        ledger.observe_tool(name, exit_code, metadata.as_ref());
        let tool_call = crate::llm::ToolCall {
            id: String::new(),
            name: name.to_string(),
            args: args.clone(),
        };
        self.record_prepared_tool_verification(
            &tool_call, exit_code, output, metadata, ledger, reports,
        )
        .await;
    }

    /// Bind only this executed call's verification after its typed receipt
    /// has projected the current file identities. Do not add a legacy
    /// mutation row: that would replace the digest the receipt established.
    pub(crate) async fn record_prepared_tool_verification(
        &self,
        tool_call: &crate::llm::ToolCall,
        exit_code: i32,
        output: &mut String,
        metadata: &mut Option<serde_json::Value>,
        ledger: &mut crate::harness_loop::MutationLedger,
        reports: &mut Vec<crate::verification::VerificationReport>,
    ) {
        normalize_mutation_metadata_path(&self.tool_context.workspace, metadata.as_mut());
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
        self.attach_mutation_observation(tool_call, &mut normalized)
            .await;
        self.record_current_call_verification(tool_call, &mut normalized, ledger, reports);
        *output = normalized.output;
        *metadata = normalized.metadata;
    }

    /// The existing metadata contract has one report slot per invocation.
    /// Bind only that invocation's accepted report, or synthesize one when the
    /// slot is absent. The same final report must reach live and replay readers.
    fn record_current_call_verification(
        &self,
        tool_call: &ToolCall,
        normalized: &mut NormalizedToolResult,
        ledger: &mut crate::harness_loop::MutationLedger,
        reports: &mut Vec<crate::verification::VerificationReport>,
    ) {
        if normalized.metadata.as_ref().is_some_and(|metadata| {
            !metadata.is_object()
                || metadata
                    .get("verification_author")
                    .and_then(serde_json::Value::as_str)
                    == Some("editor")
        }) {
            // A malformed envelope or an editor's report is not a host proof.
            return;
        }
        let occupied = normalized
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.get("verification_report").is_some());
        let mut state = ExecutionLoopState::new_seeded(&[], None);
        state.mutations = ledger.clone();
        Self::collect_verification_report(&mut state.verification_reports, &normalized.metadata);
        self.bind_or_synthesize_mutation_path_verification(
            &mut state, tool_call, normalized, !occupied,
        );
        let report = match state.verification_reports.as_slice() {
            [] => return,
            [report] => report,
            _ => {
                // Future producers must not silently create a live-only second
                // report outside the frozen single-report metadata contract.
                tracing::error!("Refusing multiple verification reports for one tool result");
                return;
            }
        };
        let metadata = normalized
            .metadata
            .get_or_insert_with(|| serde_json::json!({}));
        let Some(object) = metadata.as_object_mut() else {
            return;
        };
        object.insert("verification_report".to_string(), report.to_value());
        *ledger = state.mutations;
        reports.extend(state.verification_reports);
    }

    fn bind_or_synthesize_mutation_path_verification(
        &self,
        state: &mut ExecutionLoopState,
        tool_call: &ToolCall,
        normalized: &NormalizedToolResult,
        allow_synthesis: bool,
    ) {
        if normalized.exit_code != 0 || state.mutations.is_empty() {
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
            // No path-referencing segment: the only remaining binder is a
            // host-declared suite contract (`node test.mjs` runs the suite).
            self.bind_verification_contract(
                state,
                tool_call,
                normalized.exit_code,
                command,
                &digest,
                allow_synthesis,
            );
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
            if !tool_call.name.eq_ignore_ascii_case("bash") || !allow_synthesis {
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
        } else {
            // The existence segments did not bind: a declared suite contract
            // is the last remaining binder for this digest.
            self.bind_verification_contract(
                state,
                tool_call,
                normalized.exit_code,
                command,
                &digest,
                allow_synthesis,
            );
        }
    }

    /// Bind a Host Passed report when the executed bash command runs a
    /// host-declared verification contract (the project's suite) with exit 0
    /// and every mutated path is covered by the contract's manifest.
    ///
    /// The contract list is host-authored at session build and never
    /// model-grantable; the exit code is the real host-bash execution result;
    /// the report binds the live ledger digest, so any later mutation
    /// invalidates it before the gate re-checks. A digest-bound report from
    /// this invocation is left alone; reports from prior calls never enter
    /// this invocation's binder.
    fn bind_verification_contract(
        &self,
        state: &mut ExecutionLoopState,
        tool_call: &ToolCall,
        exit_code: i32,
        command: &str,
        digest: &str,
        allow_synthesis: bool,
    ) {
        if !allow_synthesis || self.config.verification_contracts.is_empty() {
            return;
        }
        // Only the host bash tool's own execution result counts. A wrapper's
        // command text is not the suite running.
        if !tool_call.name.eq_ignore_ascii_case("bash") {
            return;
        }
        if state
            .verification_reports
            .iter()
            .any(|report| report.effect_digest.as_deref() == Some(digest))
        {
            return;
        }
        let mutation_paths: Vec<String> = state.mutations.paths().map(str::to_string).collect();
        for contract in &self.config.verification_contracts {
            let Some(report) = crate::verification::host_report_for_verification_contract(
                contract,
                command,
                exit_code,
                &mutation_paths,
                digest,
            ) else {
                continue;
            };
            state.verification_reports.push(report);
            return;
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
/// Normalize a mutating tool's `file_path` metadata to workspace-relative
/// form so the mutation ledger, contract covers, and existence matching
/// agree regardless of how the model spelled the path. See
/// [`crate::harness_loop::workspace_relative_path`].
fn normalize_mutation_metadata_path(
    workspace: &std::path::Path,
    metadata: Option<&mut serde_json::Value>,
) {
    let Some(metadata) = metadata else {
        return;
    };
    let Some(path) = metadata.get("file_path").and_then(|value| value.as_str()) else {
        return;
    };
    let normalized = crate::harness_loop::workspace_relative_path(workspace, path);
    if normalized != path {
        if let Some(object) = metadata.as_object_mut() {
            object.insert(
                "file_path".to_string(),
                serde_json::Value::String(normalized),
            );
        }
    }
}

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
mod verification_metadata_tests;

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
        let config = crate::agent::AgentConfig {
            external_observations: vec![observation],
            ..Default::default()
        };
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

    fn leave_card_result() -> NormalizedToolResult {
        NormalizedToolResult {
            output: "已生成\n```json\n{\"root\":\"leave_card\",\"state\":{\"form\":{\"cm_content\":\"去医院\",\"cm_type\":\"病假\",\"days\":2}}}\n```\n".into(),
            exit_code: 0,
            is_error: false,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
            trust: ToolResultTrustV1::WorkspaceData,
            redaction_reviewed: true,
        }
    }

    async fn finish_leave(session_id: &str) -> (ExecutionLoopState, Vec<AgentEvent>) {
        let agent = loop_with(KnownDiagnostic {
            message: "unused".into(),
            stale: false,
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = ExecutionLoopState::new(&[]);
        agent
            .complete_tool_call(
                &mut state,
                ToolCompletionInput {
                    tool_call: &ToolCall {
                        id: "leave-1".into(),
                        name: "mcp_a3s-maas__leave_apply".into(),
                        args: serde_json::json!({"text": "明天请假"}),
                    },
                    event_tx: &Some(tx),
                    session_id: Some(session_id),
                    tool_start: Instant::now(),
                    normalized: leave_card_result(),
                },
            )
            .await;
        drop(agent);
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        (state, events)
    }

    fn model_tool_text(state: &ExecutionLoopState) -> String {
        state
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                crate::llm::ContentBlock::ToolResult {
                    content: crate::llm::ToolResultContentField::Text(text),
                    ..
                } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn a_declared_leave_card_parks_a_form_and_hides_the_json_from_the_model() {
        let (state, events) = finish_leave("leave-surface-park").await;
        assert!(state.surface_form_parked);
        let shown = model_tool_text(&state);
        assert!(shown.contains("Do not repeat"));
        assert!(!shown.contains("leave_card"));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::UserQuestion { form: Some(form), .. } if form.contains("Input(")
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text.contains("```openui")
        )));
    }

    #[tokio::test]
    async fn a_declared_surface_that_cannot_be_projected_is_hidden_from_the_model() {
        let agent = loop_with(KnownDiagnostic {
            message: "unused".into(),
            stale: false,
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = ExecutionLoopState::new(&[]);
        agent
            .complete_tool_call(
                &mut state,
                ToolCompletionInput {
                    tool_call: &ToolCall {
                        id: "chart-form".into(),
                        name: "week_chart".into(),
                        args: serde_json::json!({}),
                    },
                    event_tx: &Some(tx),
                    session_id: Some("chart-form-session"),
                    tool_start: Instant::now(),
                    normalized: NormalizedToolResult {
                        output: "{\"root\":\"week\",\"state\":{\"form\":{\"labels\":[\"一\",\"二\"],\"values\":[1,2]}}}".into(),
                        exit_code: 0,
                        is_error: false,
                        metadata: None,
                        images: Vec::new(),
                        error_kind: None,
                        trust: ToolResultTrustV1::WorkspaceData,
                        redaction_reviewed: true,
                    },
                },
            )
            .await;
        drop(agent);
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let shown = model_tool_text(&state);
        assert!(shown.contains(crate::surface_decision::DECLARED_SURFACE_REFUSAL));
        assert!(!shown.contains("labels"));
        assert!(!state.surface_form_parked);
        assert!(!events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text.contains("```openui")
        )));
    }

    #[tokio::test]
    async fn a_declared_view_stays_in_the_turn() {
        let agent = loop_with(KnownDiagnostic {
            message: "unused".into(),
            stale: false,
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut state = ExecutionLoopState::new(&[]);
        agent
            .complete_tool_call(
                &mut state,
                ToolCompletionInput {
                    tool_call: &ToolCall {
                        id: "week-view".into(),
                        name: "week_chart".into(),
                        args: serde_json::json!({}),
                    },
                    event_tx: &Some(tx),
                    session_id: Some("week-view-session"),
                    tool_start: Instant::now(),
                    normalized: NormalizedToolResult {
                        output: "{\"root\":\"week\",\"title\":\"本周\",\"state\":{\"view\":{\"labels\":[\"一\",\"二\"],\"values\":[1,2],\"note\":\"已汇总完成\"}}}".into(),
                        exit_code: 0,
                        is_error: false,
                        metadata: None,
                        images: Vec::new(),
                        error_kind: None,
                        trust: ToolResultTrustV1::WorkspaceData,
                        redaction_reviewed: true,
                    },
                },
            )
            .await;
        drop(agent);
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(!state.surface_form_parked);
        let shown = model_tool_text(&state);
        assert!(shown.contains("Do not repeat"));
        assert!(!shown.contains("已汇总完成"));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text.contains("```openui") && text.contains("BarChart")
        )));
    }

    #[tokio::test]
    async fn a_declared_leave_card_paints_openui_when_the_question_cap_is_full() {
        let session = "leave-surface-capped";
        for index in 0..3 {
            crate::ask_user::begin(session, &format!("prior-{index}"), "已有问题", &[], true)
                .expect("prior question");
        }
        let (state, events) = finish_leave(session).await;
        assert!(state.surface_form_parked);
        assert!(!model_tool_text(&state).contains("leave_card"));
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text } if text.contains("```openui") && text.contains("去医院")
        )));
    }
}
