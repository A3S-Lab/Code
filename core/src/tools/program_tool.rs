//! Tool wrapper for programmatic tool calling.

use crate::program::{
    program_verification_hints, ProgramCatalog, ProgramExecutor, ProgramResult, ProgramStepResult,
    ProgramTrace, ProgramTraceArtifact, ProgramTraceStep, ProgramVerificationHint,
};
use crate::text::truncate_utf8;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{tool_output_artifact, ToolArtifact, ToolRegistry};
use crate::verification::VerificationReport;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

const MAX_PROGRAM_STEP_OUTPUT_BYTES: usize = 4 * 1024;

pub struct ProgramTool {
    registry: Arc<ToolRegistry>,
    catalog: ProgramCatalog,
}

impl ProgramTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self::with_catalog(registry, ProgramCatalog::with_builtin_programs())
    }

    pub fn with_catalog(registry: Arc<ToolRegistry>, catalog: ProgramCatalog) -> Self {
        Self { registry, catalog }
    }
}

#[async_trait]
impl Tool for ProgramTool {
    fn name(&self) -> &str {
        "program"
    }

    fn description(&self) -> &str {
        "Run a named harness program such as program_code_search or program_repo_map. Programs execute bounded tool chains and return summarized step results."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Required. Program name to run.",
                    "enum": self.catalog.list().iter().map(|program| program.name.clone()).collect::<Vec<_>>()
                },
                "inputs": {
                    "type": "object",
                    "description": "Optional. Program-specific inputs such as query, path, and glob."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(name) = args.get("name").and_then(|value| value.as_str()) else {
            return Ok(ToolOutput::error("name parameter is required"));
        };
        let inputs = args
            .get("inputs")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let program = match self.catalog.instantiate(name, &inputs) {
            Ok(program) => program,
            Err(err) => return Ok(ToolOutput::error(err.to_string())),
        };

        let executor = ProgramExecutor::new(Arc::clone(&self.registry), ctx.clone());
        let result = executor.execute(&program).await?;
        let rendered = render_program_result(&result, &self.registry);
        let verification_hints = program_verification_hints(&result, Some(&rendered.trace));
        let verification_report =
            VerificationReport::from_program_hints(&result.program_name, &verification_hints);
        Ok(
            ToolOutput::success(rendered.output).with_metadata(serde_json::json!({
                "program": {
                    "name": result.program_name,
                    "success": result.success,
                    "summary": result.summary,
                    "steps": result.steps.iter().map(|step| {
                        serde_json::json!({
                            "tool_name": step.tool_name,
                            "label": step.label,
                            "success": step.success,
                            "metadata": step.metadata,
                        })
                    }).collect::<Vec<_>>(),
                },
                "trace": rendered.trace.to_value(),
                "verification_hints": ProgramVerificationHint::to_values(&verification_hints),
                "verification_report": verification_report.to_value(),
            })),
        )
    }
}

#[derive(Debug)]
struct RenderedProgram {
    output: String,
    trace: ProgramTrace,
}

#[derive(Debug)]
struct RenderedStep {
    output: String,
    trace: ProgramTraceStep,
}

fn render_program_result(result: &ProgramResult, registry: &ToolRegistry) -> RenderedProgram {
    let mut output = String::new();
    output.push_str(&result.summary);
    if let Some(summary) = program_specific_summary(result) {
        output.push('\n');
        output.push_str(&summary);
    }

    let mut trace_steps = Vec::with_capacity(result.steps.len());
    for (index, step) in result.steps.iter().enumerate() {
        let rendered_step = render_step(&result.program_name, index, step, registry);
        let label = step.label.as_deref().unwrap_or(&step.tool_name);
        output.push_str(&format!(
            "\n\n## Step {}: {} [{}] ({})\n{}",
            index + 1,
            label,
            step.tool_name,
            if step.success { "ok" } else { "failed" },
            rendered_step.output
        ));
        trace_steps.push(rendered_step.trace);
    }

    RenderedProgram {
        output,
        trace: ProgramTrace::from_result(result, trace_steps),
    }
}

fn program_specific_summary(result: &ProgramResult) -> Option<String> {
    match result.program_name.as_str() {
        "program_code_search" => summarize_code_search(result),
        "program_repo_map" => summarize_repo_map(result),
        _ => None,
    }
}

fn summarize_code_search(result: &ProgramResult) -> Option<String> {
    let step = result.steps.first()?;
    if step.output.contains("No matches found") {
        return Some("Search summary: no matches found.".to_string());
    }

    step.output
        .lines()
        .rev()
        .find(|line| line.contains("match(es) in") && line.contains("file(s)"))
        .map(|line| format!("Search summary: {}.", line.trim()))
}

fn summarize_repo_map(result: &ProgramResult) -> Option<String> {
    let mut files = Vec::new();
    for step in &result.steps {
        if step.tool_name != "glob" || !step.success || step.output.contains("No files found") {
            continue;
        }
        for line in step.output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.ends_with("file(s) found") {
                continue;
            }
            files.push(trimmed.to_string());
        }
    }

    files.sort();
    files.dedup();
    if files.is_empty() {
        Some("Repo map summary: no known project files found.".to_string())
    } else {
        Some(format!(
            "Repo map summary: found project files: {}.",
            files.join(", ")
        ))
    }
}

fn render_step(
    program_name: &str,
    step_index: usize,
    step: &ProgramStepResult,
    registry: &ToolRegistry,
) -> RenderedStep {
    let base_trace = |compacted: bool, artifact: Option<ProgramTraceArtifact>| {
        ProgramTraceStep::from_result(step_index, step, compacted, artifact)
    };

    if step.output.len() <= MAX_PROGRAM_STEP_OUTPUT_BYTES {
        return RenderedStep {
            output: step.output.clone(),
            trace: base_trace(false, None),
        };
    }

    let shown = truncate_utf8(&step.output, MAX_PROGRAM_STEP_OUTPUT_BYTES);
    let artifact = tool_output_artifact(
        &format!(
            "program-step-{program_name}-{}-{step_index}",
            step.tool_name
        ),
        &step.output,
        shown.len(),
    );
    registry.artifact_store().put(ToolArtifact {
        artifact_id: artifact.artifact_id.clone(),
        artifact_uri: artifact.artifact_uri.clone(),
        tool_name: format!("program:{program_name}:{}", step.tool_name),
        content: step.output.clone(),
        original_bytes: artifact.original_bytes,
        shown_bytes: artifact.shown_bytes,
    });

    let artifact_id = artifact.artifact_id.clone();
    let artifact_uri = artifact.artifact_uri.clone();
    let artifact_trace = ProgramTraceArtifact {
        artifact_id,
        artifact_uri,
        original_bytes: artifact.original_bytes,
        shown_bytes: artifact.shown_bytes,
    };

    RenderedStep {
        output: format!(
            "{}\n\n[program step output compacted: showing the first {} of {} bytes. Full step artifact: {}.]",
            shown, artifact.shown_bytes, artifact.original_bytes, artifact.artifact_uri
        ),
        trace: base_trace(true, Some(artifact_trace)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::PROGRAM_TRACE_SCHEMA;
    use crate::tools::{Tool, ToolOutput};
    use anyhow::Result;
    use async_trait::async_trait;
    use std::path::PathBuf;

    struct EchoGrepTool;

    #[async_trait]
    impl Tool for EchoGrepTool {
        fn name(&self) -> &str {
            "grep"
        }

        fn description(&self) -> &str {
            "Echo grep args"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            if args["pattern"].as_str() == Some("large") {
                return Ok(ToolOutput::success(
                    "x".repeat(MAX_PROGRAM_STEP_OUTPUT_BYTES + 1),
                ));
            }
            if args["pattern"].as_str() == Some("missing") {
                return Ok(ToolOutput::success("No matches found for pattern: missing"));
            }

            Ok(ToolOutput::success(format!(
                ">src/lib.rs:1: {} in {}\n\n1 match(es) in 1 file(s)",
                args["pattern"].as_str().unwrap_or_default(),
                args["path"].as_str().unwrap_or_default()
            )))
        }
    }

    struct RepoMapTool;

    #[async_trait]
    impl Tool for RepoMapTool {
        fn name(&self) -> &str {
            "glob"
        }

        fn description(&self) -> &str {
            "Return selected repo files"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            match args["pattern"].as_str().unwrap_or_default() {
                "Cargo.toml" => Ok(ToolOutput::success("Cargo.toml\n\n1 file(s) found")),
                "README.md" => Ok(ToolOutput::success("README.md\n\n1 file(s) found")),
                pattern => Ok(ToolOutput::success(format!(
                    "No files found matching pattern: {pattern}"
                ))),
            }
        }
    }

    struct LsTool;

    #[async_trait]
    impl Tool for LsTool {
        fn name(&self) -> &str {
            "ls"
        }

        fn description(&self) -> &str {
            "List files"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success("Directory: /tmp\n\nfile Cargo.toml"))
        }
    }

    #[tokio::test]
    async fn program_tool_runs_catalog_program() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoGrepTool));
        let tool = ProgramTool::new(Arc::clone(&registry));
        let output = tool
            .execute(
                &serde_json::json!({
                    "name": "program_code_search",
                    "inputs": {
                        "query": "AgentLoop",
                        "path": "core/src"
                    }
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert!(output.success);
        assert!(output.content.contains("program_code_search"));
        assert!(output.content.contains("Step 1: search_code [grep]"));
        assert!(output
            .content
            .contains("Search summary: 1 match(es) in 1 file(s)."));
        assert!(output.content.contains("AgentLoop in core/src"));
        let metadata = output.metadata.as_ref().expect("metadata");
        assert_eq!(metadata["program"]["name"], "program_code_search");
        assert_eq!(metadata["trace"]["schema"], PROGRAM_TRACE_SCHEMA);
        assert_eq!(metadata["trace"]["type"], "program_execution");
        assert_eq!(metadata["trace"]["step_count"], 1);
        assert_eq!(metadata["trace"]["steps"][0]["label"], "search_code");
        assert_eq!(metadata["verification_hints"][0]["kind"], "inspect_matches");
        assert_eq!(metadata["verification_hints"][0]["required"], true);
        assert_eq!(metadata["verification_report"]["status"], "needs_review");
        assert_eq!(
            metadata["verification_report"]["checks"][0]["kind"],
            "inspect_matches"
        );
    }

    #[tokio::test]
    async fn program_tool_summarizes_code_search_misses() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoGrepTool));
        let tool = ProgramTool::new(Arc::clone(&registry));
        let output = tool
            .execute(
                &serde_json::json!({
                    "name": "program_code_search",
                    "inputs": {
                        "query": "missing"
                    }
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert!(output.success);
        assert!(output.content.contains("Search summary: no matches found."));
    }

    #[tokio::test]
    async fn program_tool_summarizes_repo_map_files() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(LsTool));
        registry.register(Arc::new(RepoMapTool));
        let tool = ProgramTool::new(Arc::clone(&registry));
        let output = tool
            .execute(
                &serde_json::json!({
                    "name": "program_repo_map",
                    "inputs": {
                        "path": "."
                    }
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert!(output.success);
        assert!(output
            .content
            .contains("Repo map summary: found project files: Cargo.toml, README.md."));
    }

    #[tokio::test]
    async fn program_tool_rejects_unknown_program() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        let tool = ProgramTool::new(registry);
        let output = tool
            .execute(
                &serde_json::json!({ "name": "missing" }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert!(!output.success);
        assert!(output.content.contains("Unknown program"));
    }

    #[tokio::test]
    async fn program_tool_compacts_large_step_output_into_artifact() {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoGrepTool));
        let tool = ProgramTool::new(Arc::clone(&registry));
        let output = tool
            .execute(
                &serde_json::json!({
                    "name": "program_code_search",
                    "inputs": {
                        "query": "large"
                    }
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert!(output.success);
        assert!(output.content.contains("[program step output compacted:"));
        let metadata = output.metadata.as_ref().expect("metadata");
        assert_eq!(metadata["trace"]["steps"][0]["compacted"], true);
        assert_eq!(
            metadata["verification_hints"][1]["kind"],
            "inspect_artifacts"
        );
        let trace_artifact_uri = metadata["trace"]["steps"][0]["artifact"]["artifact_uri"]
            .as_str()
            .expect("trace artifact uri");
        assert_eq!(
            metadata["verification_hints"][1]["evidence_uris"][0],
            trace_artifact_uri
        );
        assert_eq!(
            metadata["verification_report"]["checks"][1]["evidence_uris"][0],
            trace_artifact_uri
        );
        let artifact_uri = output
            .content
            .split("Full step artifact: ")
            .nth(1)
            .and_then(|tail| tail.split('.').next())
            .expect("artifact uri");
        assert_eq!(trace_artifact_uri, artifact_uri);
        let artifact = registry
            .get_artifact(artifact_uri)
            .expect("stored step artifact");
        assert_eq!(artifact.content.len(), MAX_PROGRAM_STEP_OUTPUT_BYTES + 1);
    }
}
