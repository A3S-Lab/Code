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
    assert_eq!(program.steps[0].tool_name, "search");
    assert_eq!(program.steps[0].label.as_deref(), Some("search_code"));
    assert_eq!(program.steps[0].args["mode"], "grep");
    assert_eq!(program.steps[0].args["query"], "ContextAssembler");
    assert_eq!(program.steps[0].args["path"], "core/src");
    assert_eq!(program.steps[0].args["include"], "*.rs");
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
        .all(|step| step.tool_name == "search"));
    assert!(program.steps[1..]
        .iter()
        .all(|step| step.args["mode"] == "glob"));
    assert_eq!(program.steps[1].label.as_deref(), Some("find_Cargo.toml"));
    assert_eq!(program.steps[1].args["query"], "Cargo.toml");
    assert_eq!(program.steps[6].args["query"], "AGENTS.md");
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
            tool_name: "search".to_string(),
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
    assert_eq!(hints[0].suggested_tools, vec!["read", "search"]);
}

#[test]
fn program_verification_hints_include_failures_and_artifacts() {
    let result = ProgramResult {
        program_name: "custom_program".to_string(),
        success: false,
        summary: "stopped".to_string(),
        steps: vec![ProgramStepResult {
            tool_name: "search".to_string(),
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
            tool_name: "search".to_string(),
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

#[test]
fn program_catalog_register_overwrites_existing_template() {
    let mut catalog = ProgramCatalog::new();
    catalog.register(
        ProgramTemplate::new("search", "First version")
            .with_parameter(ProgramParameter::required("query", "Query"))
            .with_step(ProgramStepTemplate::new(
                "grep",
                serde_json::json!({ "pattern": "{{query}}" }),
            )),
    );
    catalog.register(
        ProgramTemplate::new("search", "Second version")
            .with_parameter(ProgramParameter::required("query", "Query"))
            .with_step(ProgramStepTemplate::new(
                "grep",
                serde_json::json!({ "pattern": "{{query}}", "path": "." }),
            )),
    );

    assert_eq!(catalog.list().len(), 1);
    assert_eq!(catalog.get("search").unwrap().description, "Second version");
    let program = catalog
        .instantiate("search", &serde_json::json!({ "query": "needle" }))
        .unwrap();
    assert_eq!(program.steps[0].args["path"], ".");
}

#[test]
fn program_catalog_instantiate_unknown_program_errors() {
    let catalog = ProgramCatalog::new();
    let err = catalog
        .instantiate("missing_program", &serde_json::json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("Unknown program: missing_program"));
}

#[test]
fn program_template_renders_whole_value_numeric_placeholders() {
    let template = ProgramTemplate::new("numeric", "Numeric placeholder")
        .with_parameter(ProgramParameter::required("count", "Count"))
        .with_step(ProgramStepTemplate::new(
            "echo",
            serde_json::json!({ "limit": "{{count}}" }),
        ));

    let program = template
        .instantiate(&serde_json::json!({ "count": 42 }))
        .unwrap();

    assert_eq!(program.steps[0].args["limit"], 42);
}

#[tokio::test]
async fn program_executor_errors_when_tool_is_missing() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    let executor = ProgramExecutor::new(
        Arc::clone(&registry),
        ToolContext::new(PathBuf::from("/tmp")),
    );
    let program = Program::new("missing_tool", "Uses an unknown tool")
        .with_step(ProgramStep::new("does_not_exist", serde_json::json!({})));

    let result = executor.execute(&program).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.steps.len(), 1);
    assert!(result.steps[0]
        .output
        .contains("Unknown tool: does_not_exist"));
}

#[test]
fn program_trace_from_result_counts_failed_steps() {
    let result = ProgramResult {
        program_name: "failed_program".to_string(),
        success: false,
        summary: "stopped".to_string(),
        steps: vec![
            ProgramStepResult {
                tool_name: "search".to_string(),
                label: Some("scan".to_string()),
                success: true,
                output: "ok".to_string(),
                metadata: None,
            },
            ProgramStepResult {
                tool_name: "search".to_string(),
                label: Some("scan_again".to_string()),
                success: false,
                output: "failed".to_string(),
                metadata: None,
            },
        ],
    };
    let trace = ProgramTrace::from_result(
        &result,
        vec![
            ProgramTraceStep::from_result(0, &result.steps[0], false, None),
            ProgramTraceStep::from_result(1, &result.steps[1], false, None),
        ],
    );

    assert_eq!(trace.failed_steps, 1);
    assert!(!trace.success);
}

#[test]
fn program_verification_hint_to_values_serializes_hints() {
    let hints = vec![
        ProgramVerificationHint::new("inspect_matches", "Review matches")
            .required()
            .with_suggested_tools(["read"])
            .with_evidence_uris(["artifact://tool-output/abc"]),
    ];
    let values = ProgramVerificationHint::to_values(&hints);

    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["kind"], "inspect_matches");
    assert_eq!(values[0]["required"], true);
    assert_eq!(values[0]["suggested_tools"], serde_json::json!(["read"]));
}

#[test]
fn program_template_validation_reports_name_parameter_and_placeholder_issues() {
    let invalid = ProgramTemplate::new("bad name!", "")
        .with_parameter(ProgramParameter::required("query!", "Query"))
        .with_parameter(ProgramParameter::required("query!", "Dup"))
        .with_parameter({
            let mut required_with_default = ProgramParameter::required("ok", "Ok");
            required_with_default.default = Some(serde_json::json!("x"));
            required_with_default
        })
        .with_step(ProgramStepTemplate::new(
            "",
            serde_json::json!({ "pattern": "{{missing}}", "nested": ["{{ok}}", {"k": "{{!}}"}] }),
        ).with_label(""));

    let validation = invalid.validate();
    assert!(!validation.is_valid());
    let summary = validation.summary();
    assert!(summary.contains("invalid"));
    let codes: Vec<_> = validation.issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"invalid_name"));
    assert!(codes.contains(&"empty_description"));
    assert!(codes.contains(&"invalid_parameter_name") || codes.contains(&"duplicate_parameter"));
    assert!(codes
        .iter()
        .any(|c| *c == "unknown_placeholder" || *c == "invalid_placeholder"));
    assert!(codes.contains(&"empty_tool_name"));
    assert!(codes.contains(&"empty_step_label"));
    assert!(codes.contains(&"required_parameter_with_default"));
}

#[test]
fn program_template_validation_rejects_empty_steps_and_duplicate_labels() {
    let empty = ProgramTemplate::new("empty_steps", "desc");
    let empty_validation = empty.validate();
    assert!(empty_validation
        .issues
        .iter()
        .any(|issue| issue.code == "empty_steps"));

    let dup = ProgramTemplate::new("dup_labels", "desc")
        .with_step(ProgramStepTemplate::new("search", serde_json::json!({})).with_label("same"))
        .with_step(ProgramStepTemplate::new("ls", serde_json::json!({})).with_label("same"));
    let dup_validation = dup.validate();
    assert!(dup_validation
        .issues
        .iter()
        .any(|issue| issue.code == "duplicate_step_label"));
    assert!(ProgramTemplateValidation::validate(&program_code_search()).is_valid());
    assert!(ProgramTemplateValidation::validate(&program_repo_map())
        .summary()
        .contains("is valid"));
}

#[test]
fn program_template_renders_nested_arrays_objects_and_boolean_placeholders() {
    let template = ProgramTemplate::new("nested", "Nested render")
        .with_parameter(ProgramParameter::required("flag", "Flag"))
        .with_parameter(ProgramParameter::required("name", "Name"))
        .with_step(ProgramStepTemplate::new(
            "echo",
            serde_json::json!({
                "enabled": "{{flag}}",
                "payload": {
                    "items": ["{{name}}", {"inner": "{{name}}"}]
                },
                "label": "prefix-{{name}}-suffix"
            }),
        ));

    let program = template
        .instantiate(&serde_json::json!({ "flag": true, "name": "alpha" }))
        .unwrap();
    assert_eq!(program.steps[0].args["enabled"], true);
    assert_eq!(program.steps[0].args["payload"]["items"][0], "alpha");
    assert_eq!(
        program.steps[0].args["payload"]["items"][1]["inner"],
        "alpha"
    );
    assert_eq!(program.steps[0].args["label"], "prefix-alpha-suffix");
}

#[test]
fn program_executor_summarizes_completed_runs() {
    let result = ProgramResult {
        program_name: "done".into(),
        success: true,
        summary: String::new(),
        steps: vec![ProgramStepResult {
            tool_name: "search".into(),
            label: None,
            success: true,
            output: "ok".into(),
            metadata: None,
        }],
    };
    // Exercise public summary path via ProgramTrace conversion helpers.
    let program =
        Program::new("done", "d").with_step(ProgramStep::new("search", serde_json::json!({})));
    let summary = summarize_program_result(&program, true, 1);
    assert_eq!(summary, "Program 'done' completed after 1/1 steps.");
    let _ = result;
}

#[test]
fn program_template_optional_parameter_may_be_omitted() {
    let template = ProgramTemplate::new("optional", "Optional params")
        .with_parameter(ProgramParameter::required("query", "Query"))
        .with_parameter(ProgramParameter {
            name: "limit".into(),
            description: "Limit".into(),
            required: false,
            default: None,
        })
        .with_step(ProgramStepTemplate::new(
            "search",
            serde_json::json!({ "pattern": "{{query}}" }),
        ));
    let program = template
        .instantiate(&serde_json::json!({ "query": "main" }))
        .unwrap();
    assert_eq!(program.steps[0].args["pattern"], "main");
}

#[test]
fn program_verification_hints_cover_failed_run_without_failed_steps() {
    let result = ProgramResult {
        program_name: "program_code_search".into(),
        success: false,
        summary: "failed".into(),
        steps: Vec::new(),
    };
    let hints = program_verification_hints(&result, None);
    assert!(hints.iter().any(|hint| hint.kind == "inspect_matches"));
    assert!(hints.iter().any(|hint| hint
        .message
        .contains("Investigate the failed program execution")));
}

#[test]
fn program_template_validation_rejects_empty_name_and_empty_parameter() {
    let invalid = ProgramTemplate::new("   ", "desc")
        .with_parameter(ProgramParameter::required("   ", "blank"))
        .with_step(ProgramStepTemplate::new("search", serde_json::json!({})));
    let validation = invalid.validate();
    let codes: Vec<_> = validation.issues.iter().map(|i| i.code.as_str()).collect();
    assert!(codes.contains(&"empty_name"));
    assert!(codes.contains(&"empty_parameter_name"));
}
