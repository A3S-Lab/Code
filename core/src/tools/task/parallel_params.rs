use super::*;

/// Parameters for parallel task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParallelTaskParams {
    /// List of tasks to execute concurrently
    pub tasks: Vec<TaskParams>,
    /// When true, return a successful tool result if at least one child task
    /// succeeds. Failed child results are still included in content and metadata.
    #[serde(default)]
    pub allow_partial_failure: bool,
    /// Optional total wall-clock timeout for collecting child results.
    ///
    /// When the timeout expires, completed child results are returned and any
    /// unfinished child is marked as failed in the metadata.
    #[serde(default, alias = "timeoutMs", skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Optional successful child count that is sufficient for the caller.
    ///
    /// This only enables early return when `allow_partial_failure` is true; the
    /// default remains the barrier behavior of waiting for every child.
    #[serde(
        default,
        alias = "minSuccessCount",
        skip_serializing_if = "Option::is_none"
    )]
    pub min_success_count: Option<usize>,
}

/// Get the JSON schema for ParallelTaskParams using the built-in agent catalog.
pub fn parallel_task_params_schema() -> serde_json::Value {
    parallel_task_params_schema_for_agents(&AgentRegistry::new().list_visible())
}

pub(super) fn parallel_task_params_schema_for_agents(
    agents: &[AgentDefinition],
) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tasks": {
                "type": "array",
                "description": "List of tasks to execute in parallel. Each task runs as an independent delegated child run concurrently.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "agent": task_agent_parameter_schema(agents),
                        "description": {
                            "type": "string",
                            "description": "Required. Short task label for display and tracking."
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Required. Detailed instruction for the delegated child run."
                        },
                        "background": {
                            "type": "boolean",
                            "description": "Optional. Run this delegated child task in the background. Default: false.",
                            "default": false
                        },
                        "max_steps": {
                            "type": "integer",
                            "description": "Optional. Maximum number of tool/model steps for this delegated child task."
                        },
                        "output_schema": {
                            "type": "object",
                            "description": "Optional. JSON Schema object the delegated child result must satisfy. When provided, the validated object is returned in each result's metadata."
                        }
                    },
                    "required": ["agent", "description", "prompt"]
                },
                "minItems": 1,
                "maxItems": MAX_PARALLEL_TASKS_PER_CALL
            },
            "allow_partial_failure": {
                "type": "boolean",
                "description": "Optional. Defaults to false. When true, the parallel_task tool succeeds if at least one child task succeeds, while preserving failed child results in the output and metadata.",
                "default": false
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "description": "Optional total timeout in milliseconds. On timeout, completed child results are returned and unfinished children are marked failed."
            },
            "min_success_count": {
                "type": "integer",
                "minimum": 1,
                "description": "Optional successful child count that is enough to return early. Early return is only used when allow_partial_failure is true."
            }
        },
        "required": ["tasks"],
        "examples": [
            {
                "tasks": [
                    {
                        "agent": "explore",
                        "description": "Find Rust files",
                        "prompt": "List Rust files under src/."
                    },
                    {
                        "agent": "explore",
                        "description": "Find tests",
                        "prompt": "List test files and summarize their purpose."
                    }
                ]
            }
        ]
    })
}
