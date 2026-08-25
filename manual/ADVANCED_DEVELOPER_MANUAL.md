# A3S Code Advanced Developer Manual

For core developers, architects, and advanced users

## Table of Contents

1. Internal Architecture
2. Advanced Configuration
3. Advanced Tool Development
4. Advanced Skill Programming
5. Advanced Hook System
6. Security Hardening
7. Performance Optimization
8. Production Deployment
9. Integration
10. Troubleshooting

# Chapter 1: Internal Architecture

## 1.1 Runtime Architecture

A3S Code uses multi-threaded architecture:
- Main Thread: HTTP API / WebSocket / AgentSession calls
- Worker Thread Pool: workspace-bound AgentSession executions
- I/O Thread Pool: LLM Client / Tool Execution / File I/O

## 1.2 Internal Runtime Loop

```rust
pub enum LoopState {
    Idle,
    Planning,
    Executing,
    WaitingForLLM,
    Compacting,
    Error,
    Completed,
}
```

## 1.3 Circuit Breaker

```rust
pub struct CircuitBreaker {
    failure_threshold: u32,  // Default: 3
    reset_timeout: Duration, // Default: 30s
}
```

## 1.4 Scoped Capabilities

Installable cognitive-package lifecycle belongs to A3S Use. Code projects one
exact immutable Use capability generation into a Session-local catalog, then
pins the catalog and its governance ceiling when a Run is admitted. Temporary
Turn and Subtask capabilities are child scopes and cannot expand the parent
authority.

The migration from the current Tool, Skill, Agent, Command, Hook, MCP, and Context
registries is specified in the
[Scoped Capability Architecture](SCOPED_CAPABILITY_ARCHITECTURE.md). Read that
contract before adding a new live registration API. New APIs must preserve the
single Use package authority, atomic source-generation publication, exact
definition/execution identity, child-ceiling intersection, and bounded explicit
asynchronous close semantics.

The delivered `CAP-SET1` identity plane is available as
`a3s_code_core::capability`. Hosts can construct validated Host, Session, and
exact Use-package source batches; Built-in source construction remains sealed
inside Core. `CapabilitySet::from_contributions` canonicalizes those complete
batches, rejects mixed Use cursors and conflicts, and returns an immutable
`Arc<CapabilitySet>`. `from_use_projection` retains the exact upstream cursor
even when product filtering yields no package descriptors. It does not
activate a Tool, start MCP, or mutate a live Session. Tool, Skill, Agent,
Command, Hook, exact-client MCP, named Flow, and general Context activation now use the explicit
`SessionCapabilityBatch` host boundary below, while other runtime categories
remain on compatibility APIs until their later migration cuts.

`CAP-SCOPE1` adds the lifecycle boundary over that set. Construct a root
`CapabilityCeiling` against the exact `CapabilitySet`, create a typed Session
scope, and admit child scopes only with ceilings that are subsets of the
parent. A Use-backed Run uses `admit_use_run` and consumes a host adapter that
implements `RetainedUseGeneration` while owning the real non-clone A3S Use
snapshot lease. Do not replace that adapter with a generation number or a
fresh Registry lookup.

Capability access is a borrowed `CapabilityLease<'scope, K>`, not a cloneable
ambient context. Register asynchronous resources as `CapabilityEffect`s and
background futures through the scope supervisor. Always call the idempotent
`close().await` and inspect `ScopeCloseReport`; `Drop` cancels and aborts task
futures but cannot perform asynchronous effect teardown. Child tasks settle
first, child scopes and effects close in reverse order, and the Run's Use lease
is dropped last.

The official `AgentSession` path installs the runtime composition automatically.
Model-side orchestration and each provider/Tool iteration own Turns. A Tool may
register reversible effects through `ToolContext::register_capability_effect`;
its stream bridge is supervised by the same Turn. Foreground Skill/Task Agents
recurse through `Turn -> Subtask -> Turn`. Background Tasks and streaming memory
extraction must be synchronously admitted by an active Turn, promoted to a
Run-owned Subtask, and registered with the Run supervisor. Never use a captured
Tool context to create work after its Turn closes.

`CAP-PROJ1` supplies the atomic value plane beside those scopes. Start a
transaction with the complete next-generation `Arc<CapabilitySet>`, stage one
`CapabilityProjectionAdapter` for each descriptor, await `prepare` with a
`CancellationToken`, call `validate`, and commit only the resulting
`CapabilityTxn<Validated>`. The closed `CapabilityValue` enum does not accept
`Any`; every supported category, including bounded host-only UI documents, has
a typed contract. Unknown categories still fail closed. Value kinds, published
names, and category-specific content identities must match their exact
descriptors.

Runs read through a non-clone `CapabilityProjectionLease`, so definition and
execution borrow the same immutable value instead of consulting a latest-value
registry. A lost generation/digest CAS cannot publish. Prepared effects from
failed, cancelled, or dropped transactions move to the catalog cleanup queue,
and retired-generation effects enter that queue only after the last old lease
drops. Call `drain_cleanup_with_policy(policy).await` from the owning Session
supervisor. This Code lease pins the local projection only; a Use-backed Run
must still separately retain the real A3S Use `CapabilitySnapshotLease` through
`RetainedUseGeneration`.

`CAP-DEP1` makes adapter preparation dependency-aware without moving package
authority into Code. `CapabilityReadinessPlan::from_set` reads only the
already published `CapabilityDescriptor::dependencies()` surface edges and
binds canonical readiness waves to the set generation and digest. A catalog
transaction rejects cycles when `begin` is called and rejects a missing staged
adapter before any adapter starts. Preparation then follows dependency-first
waves; a failed prerequisite prevents dependent adapters from starting and
completed effects enter reverse rollback.

Do not derive these edges from package manifests inside Code. A3S Use must
first resolve, verify, and publish the complete package generation. Code may
then order the resulting surface descriptors while retaining that exact Use
cursor. `CapabilityReadinessPlan` is neither a package graph nor a service
locator, and surface adapters must not use it to perform SemVer resolution,
installation, Grants, lifecycle cutover, or recovery.

The first `HOST-CAP1` Core cut exposes
`AgentSession::apply_capability_batch`. Construct the complete next
`CapabilitySet`, create a `SessionCapabilityBatch`, and stage every Tool,
Skill, Agent, Command, Hook, MCP, named Flow, or general Context value before calling the
method. A Hook value is an immutable `HookBinding` that owns the exact `Hook` definition and
`Arc<dyn HookHandler>` callback as one generation-safe pair. An MCP value should
normally be staged with `McpProjectionAdapter`; it connects one exact client,
completes initialization and `tools/list`, freezes the validated `McpBinding`,
and returns the connection as a reversible effect. For a Use-backed set,
construct the batch with `from_use_projection` and a generation-specific
`UseGenerationLeaseProvider`. Its `acquire` implementation must call A3S Use
`CapabilityRegistry::acquire_snapshot_lease` with the exact cursor and return a
wrapper that owns the resulting non-clone `CapabilitySnapshotLease` while
implementing `RetainedUseGeneration`.

A Flow value is an immutable `FlowBinding`. Construct it from one validated
`WorkflowSpec` and the exact `FlowEngine` that owns store, runtime, observer,
replay, and runtime-build compatibility. `WorkflowSpec::name` must match the
capability descriptor. Use `AgentSession::projected_flow` to acquire a
non-clone host handle for the current generation, keep it alive for the whole
operation, and call `close` to release its exact A3S Use lease. The lookup is
host-only and does not make the Flow model-visible.

Construct `McpServerConfig` only after the trusted host has selected the exact
Use generation and resolved its Runtime/Gateway evidence. Do not derive a
command, URL, provider, Grant, or opaque `gateway:*` endpoint from package files
inside Code. The adapter is a transport-readiness boundary, not a Registry,
package resolver, or service locator. Projected wrappers call their exact
client directly; they never reconnect or fall back through `McpManager`.

Do not acquire one Use lease during Session publication and share it through
an `Arc`. Every Run calls the provider again so A3S Use can reject a hidden or
stale generation at its own visibility boundary. Code checks the returned
generation, capability revision, and Registry revision again, then keeps the
lease in the Run supervisor until child scopes, tasks, and effects settle.

Tool, Skill, Agent, Command, Hook, MCP, named Flow, and general Context values are the
Session kinds currently accepted by this API. The batch is validated against the compatibility
registries immediately before commit. A public-name conflict, cancellation,
Session close, preparation failure, or CAS loss leaves the current catalog
stamp unchanged. Once a name is owned by the published projection,
compatibility Tool, Skill, Agent, Command, Hook, MCP-server, and MCP-wrapper
registration cannot shadow it. Model definitions and governed execution use
the same frozen Tool `Arc`; Skill discovery and invocation use one frozen Skill registry;
automatic and Tool-driven delegation use one frozen Agent registry; blocking
and streaming slash-command dispatch use one frozen Command registry; Hook
matching and callback dispatch use one frozen definition/handler map; MCP
definitions, raw calls, and delegated children use one exact client binding;
general Context queries and completion notifications use one frozen provider list. An
optional Session-static external Hook executor runs first, but its `Skip`
cannot bypass the projected Hook layer. Projected `SessionStart`, `SessionEnd`,
`SkillLoad`, and `SkillUnload` events fail before publication because they are
outside the Run-owned production boundary. Inspect `CapabilityRuntimeError`,
close every Run, and call `drain_capability_cleanup` when a host owns cleanup
outside normal Session close.

Use `register_hook_registration` and `unregister_hook_registration` when one
host operation owns both Hook metadata and callback. The official Node.js,
Python, and Go bridges use these atomic APIs; the older piecemeal Rust methods
remain compatibility surfaces. Observational Hook work is registered with the
Run supervisor, including `async_execution` and timed-out blocking callback
settlement, so the exact A3S Use lease remains owned while accepted work
settles. Rust cannot forcibly stop a `spawn_blocking` callback that has already
started. If it ignores the configured scope-close deadline, close reports a
timeout and may release the lease while that host callback finishes; callbacks
must therefore remain bounded and cancellation-cooperative where applicable.

The official CLI and Desktop adapters complete the `HOST-CAP1` Tool/Skill host
gate. `HOST-AGENT1` completes the Core Agent runtime cut, and
`HOST-COMMAND1` completes the Core Command runtime cut. `HOST-HOOK1` completes
the Core Hook runtime cut. `HOST-MCP1` completes the Core exact-client MCP
runtime cut. It does not claim that A3S Use already projects every MCP surface
into this adapter or that official hosts have adopted it; that wiring remains
separate integration work. `HOST-CONTEXT1` completes the general Run-frozen
Context cut; providers with cognitive package bindings must still use the
persisted Knowledge/session API. `HOST-FLOW1` completes the named host Flow cut
while A3S Flow retains store, runtime, replay, and observation ownership.
`HOST-KNOWLEDGE1` completes the exact cognitive provider/binding cut while the
Knowledge host retains OKF and query-lease ownership. `HOST-UI1` completes the
Core UI value and host-handle cut: use `UiAsset`, `UiDocument`, and `UiBinding`
for bounded, path-free reviewed bytes and `AgentSession::projected_ui` for an
exact non-clone generation handle. Rendering, CSP/origin/navigation/state,
credentials, backend routing, authoritative Use dependency projection, and
official renderer-host wiring remain host integration responsibilities.

### Tool presentation profiles

Choose model-facing Tool shape with a typed Profile, not a backend name:

```rust
use a3s_code_core::{SessionOptions, ToolPresentationProfileV1};

let options = SessionOptions::new()
    .with_tool_presentation_profile(ToolPresentationProfileV1::code());
```

Adaptive preserves historical prompt-sensitive selection. Direct presents all
permission-visible definitions. Code presents the existing `program` Tool with
a bounded compact catalog. Disabled presents no definitions. Permission
visibility runs first, and every mode preserves Tool name and parameter-schema
identity. `AgentSession::presented_tool_definitions` provides a diagnostic live
preview; it is not execution authority because Run admission freezes the exact
permission and capability generation.

The Profile persists with the Session and must match on resume. Delegated runs
inherit it exactly. It never installs a Tool, selects an A3S Use generation, or
replaces governed execution; calls still pass through the pinned Tool instance,
permission, confirmation, budget, hooks, cancellation, security, and audit.

# Chapter 2: Advanced Configuration

## 2.1 Queue System Configuration

```hcl
queue {
  control_max_concurrency = 2
  query_max_concurrency = 10
  execute_max_concurrency = 5
  generate_max_concurrency = 1
  enable_metrics = true
  enable_dlq = true
}
```

## 2.2 LLM Client Configuration

```rust
pub struct LlmClientConfig {
    pool_size: usize,             // Default: 10
    connection_timeout: Duration, // Default: 30s
    request_timeout: Duration,    // Default: 120s
}
```

## 2.3 Memory Limits

```rust
pub struct MemoryLimits {
    max_session_memory_mb: usize, // Default: 100
    max_message_history: usize,   // Default: 100
    max_context_tokens: usize,    // Default: 8000
}
```

# Chapter 3: Advanced Tool Development

## 3.1 Tool Lifecycle

Register -> Initialize -> Validate Input -> Pre-execute Hook -> Execute -> Post-execute Hook -> Cleanup

## 3.2 Advanced Tool Trait

```rust
#[async_trait]
pub trait AdvancedTool: Tool {
    async fn initialize(&mut self, config: &ToolConfig) -> Result<()>;
    fn validate_input(&self, input: &Value) -> Result<()>;
    fn pre_execute(&self, ctx: &Context) -> Result<PreExecuteAction>;
    async fn execute_async(&self, input: ToolInput) -> Result<ToolOutput>;
    fn post_execute(&self, output: &ToolOutput) -> Result<()>;
    async fn cleanup(&mut self) -> Result<()>;
}
```

## 3.3 Async Tool Example

```rust
use async_trait::async_trait;

pub struct AsyncWebTool {
    client: reqwest::Client,
    rate_limiter: RateLimiter,
}

#[async_trait]
impl Tool for AsyncWebTool {
    fn name(&self) -> &str { "async_web_fetch" }
    
    async fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        self.rate_limiter.acquire().await?;
        let response = self.client
            .get(input.get("url")?)
            .timeout(Duration::from_secs(30))
            .send().await?;
        Ok(ToolOutput::new(response.text().await?))
    }
}
```

# Chapter 4: Advanced Skill Programming

## 4.1 Skill Parsing Flow

1. Frontmatter parsing (YAML)
2. Content extraction (Markdown)
3. Template compilation
4. Permission validation
5. Injection into system prompt

## 4.2 Programmatic Skill Generation

```rust
pub struct SkillBuilder {
    name: String,
    description: String,
    allowed_tools: Vec<String>,
    content: String,
}

impl SkillBuilder {
    pub fn new(name: &str) -> Self;
    pub fn with_description(mut self, desc: &str) -> Self;
    pub fn with_tool(mut self, tool: &str) -> Self;
    pub fn build(self) -> Skill;
}
```

# Chapter 5: Advanced Hook System

## 5.1 Hook Chains

```rust
pub struct HookChain {
    hooks: Vec<(u32, Box<dyn HookHandler>)>,
}

impl HookChain {
    pub fn register(&mut self, priority: u32, hook: Box<dyn HookHandler>);
    pub async fn execute(&self, event: Event) -> HookResult;
}
```

## 5.2 Conditional Hooks

```rust
pub struct ConditionalHook {
    condition: Box<dyn Fn(&Event) -> bool>,
    hook: Box<dyn HookHandler>,
}
```

# Chapter 6: Security Hardening

## 6.1 Sandboxing

```rust
// 2.0 exposes sandboxing through a concrete BashSandbox handle.
// Host applications provide the implementation and attach it with
// SessionOptions::with_sandbox_handle(...).
pub trait BashSandbox {
    async fn run(&self, command: &str, cwd: &Path) -> Result<SandboxOutput>;
}
```

# Chapter 7: Performance Optimization

## 7.1 Token Usage Optimization

- Use compact context strategies
- Enable message summarization
- Set appropriate token limits

## 7.2 Caching Strategies

```rust
pub struct CacheConfig {
    enabled: bool,
    backend: CacheBackend,
    ttl: Duration,
    max_size: usize,
}

pub enum CacheBackend {
    InMemory,
    Redis(String),
    Disk(PathBuf),
}
```

# Chapter 8: Production Deployment

## 8.1 Docker Deployment

```dockerfile
FROM rust:1.75-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3
COPY --from=builder /app/target/release/a3s-code /usr/local/bin/
ENTRYPOINT ["a3s-code"]
```

## 8.2 Kubernetes Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: a3s-code
spec:
  replicas: 3
  selector:
    matchLabels:
      app: a3s-code
  template:
    metadata:
      labels:
        app: a3s-code
    spec:
      containers:
      - name: a3s-code
        image: a3s-lab/code:latest
        resources:
          limits:
            memory: "2Gi"
            cpu: "1000m"
```

# Chapter 9: Integration

## 9.1 MCP Protocol

```rust
pub struct MCPConfig {
    enabled: bool,
    server_url: String,
    capabilities: Vec<MCPCapability>,
}
```

## 9.2 Custom Storage Backend

```rust
pub trait CustomStorage: Send + Sync {
    async fn save(&self, key: &str, value: &[u8]) -> Result<()>;
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn delete(&self, key: &str) -> Result<()>;
}
```

# Chapter 10: Troubleshooting

## 10.1 Debug Mode

```bash
export RUST_LOG=debug
export A3S_DEBUG=1
```

## 10.2 Common Issues

| Issue | Solution |
|-------|----------|
| High memory usage | Reduce max_context_tokens |
| Slow LLM responses | Check connection pool size |
| Tool timeout | Increase timeout in config |

---

End of Advanced Developer Manual
