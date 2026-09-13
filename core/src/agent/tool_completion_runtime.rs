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
