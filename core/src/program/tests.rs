use super::*;
use crate::tools::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;

#[test]
fn program_template_instantiates_step_args() {
    let template = ProgramTemplate::new("search", "Search")
        .with_parameter(ProgramParameter::required("query", "Search query"))
        .with_parameter(ProgramParameter::optional(
            "path",
            "Search path",
            serde_json::json!("."),
        ))
        .with_step(ProgramStepTemplate::new(
            "grep",
            serde_json::json!({
                "pattern": "{{query}}",
                "path": "{{path}}",
                "message": "query={{query}}"
            }),
        ));

    let program = template
        .instantiate(&serde_json::json!({ "query": "AgentLoop" }))
        .unwrap();

    assert_eq!(program.name, "search");
    assert_eq!(program.steps.len(), 1);
    assert_eq!(program.steps[0].args["pattern"], "AgentLoop");
    assert_eq!(program.steps[0].args["path"], ".");
    assert_eq!(program.steps[0].args["message"], "query=AgentLoop");
}

#[test]
fn program_template_requires_declared_inputs() {
    let template = ProgramTemplate::new("search", "Search")
        .with_parameter(ProgramParameter::required("query", "Search query"))
        .with_step(ProgramStepTemplate::new(
            "grep",
            serde_json::json!({ "pattern": "{{query}}" }),
        ));

    let err = template.instantiate(&serde_json::json!({})).unwrap_err();

    assert!(err
        .to_string()
        .contains("Missing required program parameter"));
}

#[test]
fn builtin_program_catalog_contains_first_ptc_programs() {
    let catalog = ProgramCatalog::with_builtin_programs();

    assert!(catalog.get("program_code_search").is_some());
    assert!(catalog.get("program_repo_map").is_some());
    assert_eq!(catalog.list().len(), 2);
}

#[test]
fn code_search_program_uses_query_path_and_glob() {
    let catalog = ProgramCatalog::with_builtin_programs();
    let program = catalog
        .instantiate(
            "program_code_search",
            &serde_json::json!({
                "query": "ContextAssembler",
                "path": "core/src",
                "glob": "*.rs"
            }),
        )
        .unwrap();

    assert_eq!(program.steps.len(), 1);
    assert_eq!(program.steps[0].tool_name, "grep");
    assert_eq!(program.steps[0].label.as_deref(), Some("search_code"));
    assert_eq!(program.steps[0].args["pattern"], "ContextAssembler");
    assert_eq!(program.steps[0].args["path"], "core/src");
    assert_eq!(program.steps[0].args["glob"], "*.rs");
}

#[test]
fn repo_map_program_uses_bounded_root_steps() {
    let catalog = ProgramCatalog::with_builtin_programs();
    let program = catalog
        .instantiate("program_repo_map", &serde_json::json!({ "path": "." }))
        .unwrap();

    assert_eq!(program.steps.len(), 7);
    assert_eq!(program.steps[0].tool_name, "ls");
    assert_eq!(program.steps[0].label.as_deref(), Some("list_root"));
    assert!(program.steps[1..]
        .iter()
        .all(|step| step.tool_name == "glob"));
    assert_eq!(program.steps[1].label.as_deref(), Some("find_Cargo.toml"));
    assert_eq!(program.steps[1].args["pattern"], "Cargo.toml");
    assert_eq!(program.steps[6].args["pattern"], "AGENTS.md");
}

#[test]
fn program_template_validation_accepts_builtin_templates() {
    for template in builtin_program_templates() {
        let validation = template.validate();
        assert!(
            validation.is_valid(),
            "unexpected validation errors: {}",
            validation.summary()
        );
    }
}

#[test]
fn program_template_validation_reports_asset_issues() {
    let template = ProgramTemplate::new("bad name", "")
        .with_parameter(ProgramParameter::required("query", "Query"))
        .with_parameter(ProgramParameter::required("query", "Duplicate query"))
        .with_step(
            ProgramStepTemplate::new(
                "",
                serde_json::json!({
                    "pattern": "{{missing}}",
                    "dangling": "{{query"
                }),
            )
            .with_label("scan"),
        )
        .with_step(
            ProgramStepTemplate::new("grep", serde_json::json!({ "pattern": "{{query}}" }))
                .with_label("scan"),
        );

    let validation = template.validate();
    let codes = validation
        .issues
        .iter()
        .map(|issue| issue.code.as_str())
        .collect::<Vec<_>>();

    assert!(!validation.is_valid());
    assert!(codes.contains(&"invalid_name"));
    assert!(codes.contains(&"empty_description"));
    assert!(codes.contains(&"duplicate_parameter"));
    assert!(codes.contains(&"empty_tool_name"));
    assert!(codes.contains(&"unknown_placeholder"));
    assert!(codes.contains(&"malformed_placeholder"));
    assert!(codes.contains(&"duplicate_step_label"));
}

#[test]
fn program_catalog_try_register_rejects_invalid_template() {
    let mut catalog = ProgramCatalog::new();
    let template = ProgramTemplate::new("empty_steps", "Missing steps");

    let err = catalog.try_register(template).unwrap_err();

    assert!(err.to_string().contains("empty_steps"));
    assert!(catalog.list().is_empty());
}

#[test]
fn program_trace_serializes_with_stable_schema() {
    let result = ProgramResult {
        program_name: "program_code_search".to_string(),
        success: true,
        summary: "done".to_string(),
        steps: vec![ProgramStepResult {
            tool_name: "grep".to_string(),
            label: Some("search_code".to_string()),
            success: true,
            output: "match".to_string(),
            metadata: Some(serde_json::json!({ "exit_code": 0 })),
        }],
    };

    let step_trace = ProgramTraceStep::from_result(
        0,
        &result.steps[0],
        true,
        Some(ProgramTraceArtifact {
            artifact_id: "artifact-1".to_string(),
            artifact_uri: "artifact://tool-output/artifact-1".to_string(),
            original_bytes: 100,
            shown_bytes: 10,
        }),
    );
    let trace = ProgramTrace::from_result(&result, vec![step_trace]);
    let value = trace.to_value();

    assert_eq!(value["schema"], PROGRAM_TRACE_SCHEMA);
    assert_eq!(value["type"], "program_execution");
    assert_eq!(value["program_name"], "program_code_search");
    assert_eq!(value["step_count"], 1);
    assert_eq!(value["failed_steps"], 0);
    assert_eq!(value["steps"][0]["label"], "search_code");
    assert_eq!(value["steps"][0]["output_bytes"], 5);
    assert_eq!(value["steps"][0]["metadata"]["exit_code"], 0);
    assert_eq!(
        value["steps"][0]["artifact"]["artifact_uri"],
        "artifact://tool-output/artifact-1"
    );
}

#[test]
fn program_verification_hints_include_program_contract() {
    let result = ProgramResult {
        program_name: "program_repo_map".to_string(),
        success: true,
        summary: "done".to_string(),
        steps: vec![],
    };

    let hints = program_verification_hints(&result, None);

    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].kind, "inspect_project_files");
    assert!(hints[0].required);
    assert_eq!(hints[0].suggested_tools, vec!["read", "glob"]);
}

#[test]
fn program_verification_hints_include_failures_and_artifacts() {
    let result = ProgramResult {
        program_name: "custom_program".to_string(),
        success: false,
        summary: "stopped".to_string(),
        steps: vec![ProgramStepResult {
            tool_name: "grep".to_string(),
            label: Some("scan".to_string()),
            success: false,
            output: "failed".to_string(),
            metadata: None,
        }],
    };
    let trace = ProgramTrace::from_result(
        &result,
        vec![ProgramTraceStep {
            index: 0,
            label: "scan".to_string(),
            tool_name: "grep".to_string(),
            success: false,
            output_bytes: 6,
            compacted: true,
            artifact: Some(ProgramTraceArtifact {
                artifact_id: "artifact-1".to_string(),
                artifact_uri: "artifact://tool-output/artifact-1".to_string(),
                original_bytes: 100,
                shown_bytes: 6,
            }),
            metadata: None,
        }],
    );

    let hints = program_verification_hints(&result, Some(&trace));

    assert_eq!(hints.len(), 2);
    assert_eq!(hints[0].kind, "investigate_failed_steps");
    assert!(hints[0].message.contains("scan"));
    assert_eq!(hints[1].kind, "inspect_artifacts");
    assert_eq!(
        hints[1].evidence_uris,
        vec!["artifact://tool-output/artifact-1"]
    );
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes the message argument"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success(
            args["message"].as_str().unwrap_or_default(),
        ))
    }
}

struct FailTool;

#[async_trait]
impl Tool for FailTool {
    fn name(&self) -> &str {
        "fail"
    }

    fn description(&self) -> &str {
        "Always fails"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::error("failed"))
    }
}

#[tokio::test]
async fn program_executor_runs_steps_in_order() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    let executor = ProgramExecutor::new(
        Arc::clone(&registry),
        ToolContext::new(PathBuf::from("/tmp")),
    );
    let program = Program::new("two_echoes", "Run two echo steps")
        .with_step(ProgramStep::new(
            "echo",
            serde_json::json!({ "message": "one" }),
        ))
        .with_step(ProgramStep::new(
            "echo",
            serde_json::json!({ "message": "two" }),
        ));

    let result = executor.execute(&program).await.unwrap();

    assert!(result.success);
    assert_eq!(result.steps.len(), 2);
    assert_eq!(result.steps[0].output, "one");
    assert_eq!(result.steps[1].output, "two");
    assert_eq!(result.steps[0].label, None);
    assert_eq!(
        result.summary,
        "Program 'two_echoes' completed after 2/2 steps."
    );
}

#[tokio::test]
async fn program_executor_stops_after_failed_step() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    registry.register(Arc::new(FailTool));
    let executor = ProgramExecutor::new(
        Arc::clone(&registry),
        ToolContext::new(PathBuf::from("/tmp")),
    );
    let program = Program::new("fail_fast", "Stop after a failed step")
        .with_step(ProgramStep::new(
            "echo",
            serde_json::json!({ "message": "before" }),
        ))
        .with_step(ProgramStep::new("fail", serde_json::json!({})))
        .with_step(ProgramStep::new(
            "echo",
            serde_json::json!({ "message": "after" }),
        ));

    let result = executor.execute(&program).await.unwrap();

    assert!(!result.success);
    assert_eq!(result.steps.len(), 2);
    assert_eq!(result.steps[0].output, "before");
    assert_eq!(result.steps[1].output, "failed");
    assert_eq!(
        result.summary,
        "Program 'fail_fast' stopped after 2/3 steps."
    );
}
