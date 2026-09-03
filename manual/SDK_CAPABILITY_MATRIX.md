# A3S Code SDK capability matrix

This is the release contract for the Rust, Node.js, Python, and Go SDKs. The
Core function `sdk_capabilities()` is the single source of truth; the bindings
project the same ordered records and do not maintain a second, hand-written
feature list.

## Discovery

```rust
let capabilities = a3s_code_core::sdk_capabilities();
```

```js
const { sdkCapabilities, sdkCapabilitiesSchema } = require('@a3s-lab/code')
```

```python
from a3s_code import sdk_capabilities, sdk_capabilities_schema
```

```go
capabilities := agent.ProductCapabilities()
```

Every record has a stable `id`, a category, canonical Core operation names, a
description, and `host_owned`. `host_owned` identifies the policy, credential,
or external lifecycle owner; it is not an SDK availability flag. An operation
can still return a typed unavailable/policy error when the host has not
enabled its required resource.

## Product capabilities

| ID | SDK entrypoint shape | Host-owned boundary |
| --- | --- | --- |
| `agent_runtime` | `Agent.create` / `Agent.createFromConfig`, session factories, close | No |
| `governed_tools` | `Session.tool`, `governedTool` / `governed_tool`, Go `Tool` / `GovernedTool` | Policy and confirmation are host decisions |
| `code_intelligence` | `Session.tool("code_symbols" …)` and the built-in tool definitions | Workspace language service |
| `workspace_retrieval` | `workspaceRetrievalStatus`, `semanticSearch`, `hybridSearch` | Embedding callback and workspace source |
| `context_memory` | Memory record/recall/health methods | Memory store and retention policy |
| `cognitive_packages` | Exact binding inspection and run evidence | A3S Use/Knowledge host supplies the package |
| `use_runtime_tasks` | Governed runtime task tool | A3S Use owns grants and package lifecycle |
| `model_adapters` | ACL or typed `CodeConfig` construction | Provider credentials and network policy |
| `structured_output` | Request/task/program object options | Model/provider schema support |
| `mcp_and_skills` | MCP and Skill discovery/mutation methods | Child process, URL, and credential policy |
| `planning_delegation` | `task`, `tasks`, workers, parallel orchestration | Worker model and execution budget |
| `priority_scheduling` | Scheduler options, stats, queue APIs | Host chooses capacity and priority |
| `programmable_workflows` | `program`, `parallel`, resumable workflow methods | Script and Flow policy |
| `persistence` | Save, resume, artifacts, traces, snapshots | Store location and retention |
| `state_graph` | Node/Python `StateGraphRuntime`; Go `StateGraphRuntime` over the versioned bridge | Graph persistence owner |
| `agent_release_contract` | Release admission/verification through host runtime | Artifact publication and provenance |
| `agent_protocol` | Versioned harness/bridge event and recovery operations | Service transport owner |
| `web_search` | `webSearch` / `web_search` and generic `Tool` | Network, proxy, and engine policy |
| `moli_runtime` | Default Moli provisioning via web search; packaged/cache diagnostics | Shared cache and executable ownership |
| `s3_workspace` | Typed S3 workspace provider options | Object-store credentials and endpoint |
| `filesystem_agent_server` | `serveAgentDir` / `serve_agent_dir` / `ServeAgentDir` | Daemon lifecycle and schedule policy |
| `opentelemetry` | Core telemetry configuration and trace inspection | Collector endpoint and export policy |
| `conversation` | `send`, `run`, `stream`, attachments, history, cancel | User identity and interaction policy |
| `run_control` | `steer`, `interrupt`, `runControlSnapshot` / `run_control_snapshot` | Host approval, run identity, and lifecycle policy |
| `workspace_tools` | File, shell, glob, grep, and Git helpers | Workspace boundary and authorization |
| `run_observability` | Run snapshots, pages, active tools, child tasks, traces | Evidence retention policy |
| `governance` | Confirmations, permissions, hooks, budgets, verification | Host authorization and trust decisions |

The exact set and order are tested by Core, Node, Python, and Go bridge
qualification. Use the discovery endpoint for optional-feature checks; do not
infer support from a package's file layout.

## Extension boundary

Rust can additionally accept arbitrary in-process trait objects (for example a
custom `LlmClient`, `ContextProvider`, `MemoryStore`, `BashSandbox`, or a
`Tool`). Those are implementation extension points, not separate product
capabilities. Node.js, Python, and Go expose the corresponding value-shaped
configuration and callback transports wherever a safe adapter exists. A raw
Rust trait object is never silently dropped at an FFI boundary; unsupported
custom implementations fail with a typed configuration error and the built-in
capability remains available.

The parity checker (`scripts/sdk_api_alignment_check.mjs`) and the capability
verification ledger run in release CI. They fail when a product capability,
event type, or required Agent/Session operation disappears from one official
SDK.
