//! Cross-language SDK capability contract.
//!
//! The Rust core has a deliberately larger implementation surface than any
//! one FFI binding (for example, Rust hosts can provide native trait objects).
//! This module is the single source of truth for the *product* capability
//! surface.  Every official SDK exposes this inventory verbatim so an
//! embedding application can discover features instead of guessing from
//! package versions or parsing tool definitions.

use serde::{Deserialize, Serialize};

/// Schema identifier for [`SdkCapability`] values.
pub const SDK_CAPABILITIES_SCHEMA_V1: &str = "a3s-code/sdk-capabilities/v1";

/// A product capability exposed by the Core and its official SDKs.
///
/// `host_owned` means that the embedding application supplies policy,
/// credentials, or a lifecycle owner.  It does not mean that the capability
/// is unavailable to an SDK; the SDK exposes the same typed operation and the
/// host remains responsible for the external resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SdkCapability {
    /// Stable snake-case identifier.
    pub id: String,
    /// Broad area used for UI grouping and telemetry.
    pub category: String,
    /// Human-readable contract summary.
    pub description: String,
    /// Canonical operation names. Language bindings map these to their naming
    /// conventions (for example `web_search` becomes `webSearch` in Node).
    pub operations: Vec<String>,
    /// Whether the host owns policy, credentials, or an external lifecycle.
    pub host_owned: bool,
}

struct CapabilitySpec {
    id: &'static str,
    category: &'static str,
    description: &'static str,
    operations: &'static [&'static str],
    host_owned: bool,
}

// Keep this list in a stable product-facing order.  It intentionally follows the
// capability map in README.md and includes the release/runtime surfaces that
// are not model-visible tools (events, persistence, protocol, and Moli).
const CAPABILITY_SPECS: &[CapabilitySpec] = &[
    CapabilitySpec {
        id: "agent_runtime",
        category: "runtime",
        description: "Create agents, bind workspaces, resume, replace, and close sessions.",
        operations: &["agent.create", "agent.session", "agent.resume_session", "agent.close"],
        host_owned: false,
    },
    CapabilitySpec {
        id: "governed_tools",
        category: "execution",
        description: "Invoke built-in and MCP tools through validation, policy, confirmation, hooks, budgets, and tracing.",
        operations: &["session.tool", "session.governed_tool", "session.tool_definitions"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "code_intelligence",
        category: "workspace",
        description: "Query saved-file symbols, navigation, and diagnostics when a language service is available.",
        operations: &["session.tool:code_symbols", "session.tool:code_navigation", "session.tool:code_diagnostics"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "workspace_retrieval",
        category: "workspace",
        description: "Run exact, lexical, symbol, semantic, and hybrid retrieval with bounded session-owned state.",
        operations: &["session.workspace_retrieval_status", "session.semantic_search", "session.hybrid_search"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "context_memory",
        category: "context",
        description: "Use working, short-term, durable, and semantic memory with typed health and recall APIs.",
        operations: &["session.memory", "session.remember", "session.recall", "session.memory_stats"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "cognitive_packages",
        category: "context",
        description: "Bind exact cited cognitive knowledge generations supplied by an authoritative host.",
        operations: &["session.cognitive_package_binding", "session.current_cognitive_package_binding"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "use_runtime_tasks",
        category: "integration",
        description: "Project host-owned A3S Use runtime tasks as governed model and direct-tool capabilities.",
        operations: &["session.tool:use_runtime_task"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "model_adapters",
        category: "model",
        description: "Resolve configured Anthropic, OpenAI-compatible, and custom host model adapters.",
        operations: &["agent.create", "session.send", "session.stream"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "structured_output",
        category: "model",
        description: "Request schema-constrained model output with validation and bounded repair.",
        operations: &["session.send", "session.run", "session.task"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "mcp_and_skills",
        category: "extension",
        description: "Discover and mutate isolated MCP servers and filesystem or inline Skills.",
        operations: &["session.add_mcp", "session.remove_mcp", "session.mcps", "session.add_skill", "session.skill_names"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "planning_delegation",
        category: "orchestration",
        description: "Run plans, worker agents, delegated tasks, bounded fan-out, and cancellation.",
        operations: &["session.task", "session.tasks", "session.parallel_task", "session.register_worker_agent"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "priority_scheduling",
        category: "orchestration",
        description: "Share bounded priority/FIFO admission and observe scheduler occupancy, fairness, and lifecycle counters.",
        operations: &["session.task_scheduler_stats", "session.task_scheduler_health", "session.queue_stats", "session.set_lane_handler"],
        host_owned: false,
    },
    CapabilitySpec {
        id: "programmable_workflows",
        category: "orchestration",
        description: "Execute bounded QuickJS programs and resumable parallel or Flow-backed workflows.",
        operations: &["session.program", "session.parallel", "session.parallel_resumable", "session.workflow_step"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "persistence",
        category: "state",
        description: "Atomically save and restore session state, runs, artifacts, traces, and verification evidence.",
        operations: &["session.save", "agent.resume_session", "session.get_artifact"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "state_graph",
        category: "state",
        description: "Maintain hash-linked state graph events, patches, forks, and deterministic diffs.",
        operations: &["state_graph.create", "state_graph.restore", "state_graph.propose_patch", "state_graph.diff"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "agent_release_contract",
        category: "deployment",
        description: "Validate versioned asset manifests, provenance, and compatibility before activation.",
        operations: &["release.admit", "release.bind_publication", "release.verify"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "agent_protocol",
        category: "transport",
        description: "Serve versioned session/run start, cancellation, recovery, and event-page protocols.",
        operations: &["agent_protocol.start", "agent_protocol.cancel", "agent_protocol.recover", "agent_protocol.events"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "evaluation_substrate",
        category: "evaluation",
        description: "Project bounded evidence, isolated auxiliary lifecycle, restart-safe dispatch claims, and immutable evaluation records through versioned boundaries.",
        operations: &["evaluation.evidence", "evaluation.auxiliary", "evaluation.dispatch_ledger", "evaluation.result", "evaluation.result_store", "evaluation.wire_v1"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "web_search",
        category: "web",
        description: "Search HTTP, native, RSS, and JavaScript-rendered engines through a3s-search v3.1.0.",
        operations: &["session.web_search", "session.tool:web_search"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "moli_runtime",
        category: "web",
        description: "Use a verified packaged or shared-cache Moli runtime with cross-process installation locking.",
        operations: &["moli.default", "moli.ensure", "moli.packaged"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "s3_workspace",
        category: "workspace",
        description: "Use an S3-compatible workspace backend with bounded reads and search.",
        operations: &["session.workspace_backend:s3", "session.read_file", "session.write_file"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "filesystem_agent_server",
        category: "deployment",
        description: "Serve agent directories with validated schedules, tools, readiness, and joined shutdown.",
        operations: &["agent.serve_agent_dir", "serve.status", "serve.stop"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "opentelemetry",
        category: "observability",
        description: "Export redacted runtime traces and metrics through the optional OTLP integration.",
        operations: &["telemetry.init", "session.trace_events"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "conversation",
        category: "runtime",
        description: "Send, run, stream, attach content, inspect history, and cancel transcript-affecting turns.",
        operations: &["session.send", "session.run", "session.stream", "session.send_with_attachments", "session.history", "session.cancel"],
        host_owned: false,
    },
    CapabilitySpec {
        id: "run_control",
        category: "runtime",
        description: "Steer or cooperatively interrupt an active run with idempotent receipts and optimistic turn guards.",
        operations: &["session.steer", "session.interrupt", "session.run_control_snapshot"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "workspace_tools",
        category: "workspace",
        description: "Read, write, list, edit, patch, shell, glob, and grep through the governed workspace boundary.",
        operations: &["session.read_file", "session.write_file", "session.ls", "session.edit_file", "session.patch_file", "session.bash", "session.glob", "session.grep", "session.git"],
        host_owned: true,
    },
    CapabilitySpec {
        id: "run_observability",
        category: "observability",
        description: "Inspect durable run snapshots, event pages, active tools, traces, and child-task state.",
        operations: &["session.runs", "session.run_snapshot", "session.run_events", "session.run_event_page", "session.active_tools", "session.subagent_tasks"],
        host_owned: false,
    },
    CapabilitySpec {
        id: "governance",
        category: "security",
        description: "Configure permissions, confirmations, hooks, budgets, verification, sanitization, and sandbox policy.",
        operations: &["session.pending_confirmations", "session.confirm_tool_use", "session.register_hook", "session.set_budget_guard", "session.verify_commands"],
        host_owned: true,
    },
];

/// Return the complete, ordered product capability inventory.
pub fn sdk_capabilities() -> Vec<SdkCapability> {
    CAPABILITY_SPECS
        .iter()
        .map(|spec| SdkCapability {
            id: spec.id.to_owned(),
            category: spec.category.to_owned(),
            description: spec.description.to_owned(),
            operations: spec
                .operations
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            host_owned: spec.host_owned,
        })
        .collect()
}

/// Return the schema identifier used by the inventory endpoint.
pub const fn sdk_capabilities_schema() -> &'static str {
    SDK_CAPABILITIES_SCHEMA_V1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn inventory_is_stable_and_complete() {
        let capabilities = sdk_capabilities();
        assert!(capabilities.len() >= 20);
        let ids = capabilities
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len());
        for capability in &capabilities {
            assert!(!capability.category.is_empty());
            assert!(!capability.description.is_empty());
            assert!(!capability.operations.is_empty());
        }
        let required = [
            "agent_runtime",
            "conversation",
            "governed_tools",
            "web_search",
            "moli_runtime",
            "evaluation_substrate",
            "persistence",
            "governance",
            "run_control",
        ];
        for id in required {
            assert!(ids.contains(&id), "missing capability {id}");
        }
    }

    #[test]
    fn inventory_serializes_with_schema() {
        let value = serde_json::json!({
            "schema": sdk_capabilities_schema(),
            "capabilities": sdk_capabilities(),
        });
        assert_eq!(value["schema"], SDK_CAPABILITIES_SCHEMA_V1);
        assert!(value["capabilities"]
            .as_array()
            .is_some_and(|items| !items.is_empty()));
    }
}
