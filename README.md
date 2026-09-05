<p align="center">
  <img src="./assets/readme/hero.svg" width="100%" alt="A3S Code governed agent runtime with an explicit model, policy, tool, event, and snapshot flow">
</p>

<p align="center">
  <a href="https://github.com/A3S-Lab/Code/releases"><img alt="GitHub release" src="https://img.shields.io/github/v/release/A3S-Lab/Code?style=flat-square&color=6ca3ff"></a>
  <a href="https://github.com/A3S-Lab/Code/actions/workflows/ci.yml"><img alt="CI status" src="https://img.shields.io/github/actions/workflow/status/A3S-Lab/Code/ci.yml?branch=main&style=flat-square&label=CI"></a>
  <a href="https://crates.io/crates/a3s-code-core"><img alt="Crates.io" src="https://img.shields.io/crates/v/a3s-code-core?style=flat-square&color=f0b44d"></a>
  <a href="https://www.npmjs.com/package/@a3s-lab/code"><img alt="npm" src="https://img.shields.io/npm/v/%40a3s-lab%2Fcode?style=flat-square&color=cb3837"></a>
  <a href="https://pypi.org/project/a3s-code/"><img alt="PyPI" src="https://img.shields.io/pypi/v/a3s-code?style=flat-square&color=3775a9"></a>
  <a href="./LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-3ccf91?style=flat-square"></a>
</p>

**A3S Code** is an async Rust runtime for building governed coding agents. It
keeps the agent loop, workspace tools, model adapters, policy decisions,
versioned events, workspace retrieval, and durable evidence behind
explicit contracts. Use it from Rust, Node.js, Python, Go, or through the
`a3s code` terminal application.

<p align="center">
  <a href="#start-in-60-seconds">Start</a> ·
  <a href="#whats-new-in-80">v8</a> ·
  <a href="#why-a3s-code">Why Code</a> ·
  <a href="#capability-map">Capabilities</a> ·
  <a href="#configure-the-runtime">Configure</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#documentation">Documentation</a>
</p>

## What's new in 8.0

- **Run-owned spacetime composition.** Session, Run, Turn, and Subtask scopes
  now form one downward-only authority and cancellation tree with bounded,
  reverse-order effect settlement.
- **Generation-exact capability projection.** Tools, Skills, Agents, Commands,
  Hooks, MCP, Context, Flow, Knowledge, and UI values publish atomically and
  remain pinned for the lifetime of every admitted Run.
- **Exact temporal recovery.** Run and logical-checkpoint evidence binds the
  Code catalog, complete authority ceiling, and optional A3S Use cursor. An N
  checkpoint cannot silently resume through N+1.
- **Portable checkpoint artifacts.** Canonical semantic and logical state is
  content-addressed as one host-storable payload, with fail-closed drift checks
  and a fresh-Session exact historical bootstrap path.
- **Bounded model evidence.** Tool requests, deterministic result transforms,
  immutable original-content references, model inputs, and capability surfaces
  are digest-bound without retaining credentials or prompt plaintext.

Go consumers must update the module path to
`github.com/A3S-Lab/Code/sdk/go/v8`. See [CHANGELOG.md](CHANGELOG.md) for the
complete compatibility and release record.

## Start in 60 seconds

### Run the terminal product

```bash
brew install A3S-Lab/tap/a3s

# Or install from crates.io
cargo install a3s

cd /path/to/your/project
a3s code
```

The terminal product streams reasoning, tool activity, approvals, task
progress, and diffs. Resume persisted work with `a3s code resume` or
`a3s code resume <session-id>`.

### Embed the runtime

```bash
cargo add a3s-code-core
cargo add tokio --features macros,rt-multi-thread
```

```rust,no_run
use a3s_code_core::{Agent, AgentEvent};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let agent = Agent::new("agent.acl").await?;
    let session = agent.session_builder(".").build().await?;

    let (mut events, lifecycle) = session
        .stream("Find the authentication entry points.", None)
        .await?;

    while let Some(event) = events.recv().await {
        match event {
            AgentEvent::TextDelta { text } => print!("{text}"),
            AgentEvent::End { .. } => break,
            _ => {}
        }
    }

    let _ = lifecycle.await;
    Ok(())
}
```

`Agent` owns resolved configuration and shared capabilities. `AgentSession`
binds them to one workspace and conversation. The event stream is the product
boundary: a host can render the same lifecycle that the runtime persists and
replays.

Agent execution also has one explicit lifetime tree. A host invocation admits
`Session -> Run`; model orchestration and each provider/Tool iteration own a
Turn, while Skill and Task children recurse as `Turn -> Subtask -> Turn`.
Tool effects and stream bridges settle with their Turn. Explicit background
Tasks and post-turn memory extraction are promoted only after the invoking
Turn is validated, then remain supervised by the Run until bounded close.

## Why A3S Code

| Requirement                             | Runtime mechanism                                                                                                                                                                 |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Govern every side effect**            | JSON argument validation, typed tool capabilities, permission policy, human confirmation, hooks, budgets, security providers, and cancellation share one invocation path.         |
| **Keep context bounded**                | Reads, searches, command output, Git results, and fetched pages expose ranges or cursors. Large evidence moves into bounded artifacts with previews, sizes, and hashes.           |
| **Own the UI without forking the loop** | Core emits `AgentEvent`; SDK streams and persisted runs use the lossless `EventEnvelopeV1` protocol. The host chooses presentation, identity, credentials, and deployment policy. |
| **Change model shape without authority drift** | A closed Tool-presentation Profile runs after permission visibility and before the model request; execution keeps the same pinned Tool values and governance. |
| **Resume from evidence, not guesswork** | `SessionSnapshotV1` can atomically commit session state, runs, artifacts, traces, verification reports, and child-task records as one generation.                                 |

One turn follows a visible chain of responsibility:

```text
user request
    │
    ▼
workspace-bound AgentSession
    │ context + memory
    ▼
model adapter
    │ proposed tool call
    ▼
validation → permission → confirmation → budget → sandbox
    │ governed result
    ▼
AgentEvent / EventEnvelopeV1
    │
    └── runs + traces + artifacts + SessionSnapshotV1
```

This separation lets an interactive terminal, an SDK application, and a
background service share the same execution semantics without sharing a UI.

## Capability map

The Core crate enables lazy Moli-backed search by default through
`a3s-search` v3.1.0. Moli is resolved from a packaged sidecar, the verified
per-user cache, or a pinned HTTPS download and is shared by all local Code
processes. Minimal embeddings can use `default-features = false`; Chrome and
Lightpanda remain explicit backends, while cloud backends, serving, and
telemetry remain opt-in.

| Area                    | What is available                                                                                                                                                                                                                       | Activation                                                                                                                                                                |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent runtime           | Async `Agent`, workspace-bound `AgentSession`, send, stream, resume, replace, cancel, close, replay, and safe-point `steer`/`interrupt` run control                                                                                                                                          | Baseline                                                                                                                                                                  |
| Governed tools          | Files, search, shell, Git, web, structured generation, batch, program, Skills, MCP, delegation, deterministic result projection, and evidence                                                                                           | Exposed only when workspace and policy allow                                                                                                                              |
| Evaluation substrate    | Provider-neutral execution targets/frames, digest-only fact journals, atomic bounded evidence snapshots, isolated auxiliary runs, host boundary supervision, restart-safe dispatch leases, durable result CAS, and strict versioned Rust/Node/Python/Go wire projections | Inject an `EvaluationPolicy`/`AuxiliaryExecutor` and optionally a dispatch/result store; Core supplies mechanisms and generated transport schemas, while reviewer rubrics, findings, authorization, and Cloud audit remain host-owned |
| Native research contracts | Versioned digest-bound research runs, evidence facts, provenance receipts, review findings, and project events with bounded fields and fail-closed lifecycle transitions | Hosts bind exact source/evidence snapshots and `RunCapabilityBindingV1`; A3S Use supplies package/environment identity and Desktop/Cloud own scientific policy, review decisions, retention, and publication |
| Code intelligence       | Saved-file symbols, definitions, declarations, references, implementations, diagnostics, revisions, and stale-state metadata                                                                                                            | Host-selected local workspace                                                                                                                                             |
| Workspace retrieval     | Asynchronous session-owned chunk catalog, official zvec-rust FTS/BM25 by default, Memory-backed exact vectors, hybrid RRF, optional deterministic CPU reranking, readiness/coverage metrics, and digest-verified current-source results | Explicit per-session opt-in for semantic/vector work; baseline lexical and symbol search needs no embedding model or vector database; native zvec builds require an attested platform library |
| Context and memory      | Ranked context, repeated compaction, three-tier V1 memory, typed stores, recall, extraction, non-destructive supersession, V2 candidate shadowing, audited active-only lexical/semantic/one-hop relation recall, deterministic RRF, verified revision-CAS snapshot refresh receipts, exact namespace-token acceleration, host-persisted safe refresh checkpoints, opt-in session-owned refresh scheduling, exact restart binding, and owned maintenance health | Host-selected; V2 requires an exact repository/namespace binding and evidence-backed activation; semantic recall additionally requires a typed embedding provider, caller-owned vector index, explicit refresh timing, and exact schema-5 generation identity |
| Cognitive packages      | Exact A3S Use generation binding, host-injected cited Markdown provider, bounded source verification, restart checks, and fail-closed retrieval                                                                                         | Rust host injects `CognitiveContextSession`; Code never installs or resolves packages                                                                                     |
| A3S Use Runtime Tasks   | Exact capability-snapshot v2 Runtime Tool projection and model-visible governed invocation through a host-owned dispatcher                                                                                                             | Stage `UseRuntimeTaskProjectionAdapter` in the atomic Use-backed `SessionCapabilityBatch`; Code never launches projected commands or acquires package state directly       |
| Model adapters          | Anthropic, Zhipu, OpenAI-compatible APIs, and custom `LlmClient` implementations                                                                                                                                                        | Configuration or host injection                                                                                                                                           |
| Structured output       | Native provider formats or schema-validated prompt, partial parse, and repair fallback                                                                                                                                                  | Baseline                                                                                                                                                                  |
| MCP and Skills          | Isolated MCP transports plus filesystem, registry, inline, and live session Skills                                                                                                                                                      | Configuration or live registration                                                                                                                                        |
| Planning and delegation | Optional plans and goals, foreground/background workers, bounded parallel tasks, progress, and targeted cancellation                                                                                                                    | Manual tools independently configurable; automation opt-in                                                                                                                |
| Priority scheduling     | Agent-wide `a3s-lane` priority/FIFO admission across sessions, direct tools, detached background children, and host workflows, with cancellation, starvation-safe aging, and occupancy snapshots                                        | Baseline; tune `task_scheduler`, select per-session `TaskPriority`, inspect `task_scheduler_stats()`                                                                      |
| Safe-point run control  | Typed, idempotent `steer` and cooperative `interrupt` requests with immutable Run identity, optimistic turn guards, bounded receipts, lifecycle Hooks, and durable event evidence                                                                 | Host invokes the Session control surface; requests never create a concurrent transcript operation and never change model, permissions, sandbox, or budget                    |
| Programmable workflows  | Bounded QuickJS `program` calls and replayable A3S Flow-backed dynamic workflows                                                                                                                                                        | `program` baseline; dynamic runtime explicitly registered                                                                                                                 |
| Persistence             | Atomic snapshots, run events, traces, artifacts, verification, checkpoints, and optional RL trajectories                                                                                                                                | Configured store and host policy                                                                                                                                          |
| State graph             | Hash-linked events, typed objects and relations, optimistic patches, strict replay, forks, diffs, and Flow 0.11 lifecycle projection including cancellation, terminal outcomes, progress, and child operations                          | Explicit application use                                                                                                                                                  |
| Agent release contract  | Bounded `.a3s/asset.acl` admission, canonical identity, provenance binding, and compatibility checks                                                                                                                                    | Baseline admission API                                                                                                                                                    |
| Headless Agent protocol | Exact release/session/run start, cancellation, checkpoint recovery, receipts, atomically observed bounded `EventEnvelopeV1` pages, per-conversation detached Git worktrees, and immutable `/v1/agent/changes` patches                   | `AgentProtocolHarness` multiplexes ordinary Code sessions and `AgentProtocolHost` executes through each `AgentSession`; the `a3s code` process supplies service transport |
| Headless web search     | `a3s-search` v3.1.0 with lazy Moli-backed Google/Baidu/Bing/Brave engines, shared-cache lifecycle, and typed diagnostics; Chrome/Chromium and Lightpanda remain configurable                                                                 | Default Cargo feature `headless-search`; disable with `default-features = false`                                                                                          |
| SDK capability contract | Ordered product capability inventory, schema discovery, Moli diagnostics/provisioning, and state-graph APIs are exposed by Rust, Node.js, Python, and Go                                                                                 | Call each SDK's capability discovery function before optional integrations                                                                                                 |
| S3 workspace            | S3-compatible object backend                                                                                                                                                                                                            | Cargo feature `s3`                                                                                                                                                        |
| Filesystem agent server | Agent-directory cron serving with post-preparation readiness, typed failure state, and bounded joined shutdown                                                                                                                          | Cargo feature `serve`                                                                                                                                                     |
| OpenTelemetry           | OTLP export in addition to baseline `tracing`                                                                                                                                                                                           | Cargo feature `telemetry`                                                                                                                                                 |

Availability never bypasses policy. Auto-save, automatic compaction, goals,
automatic delegation, sandboxing, human approval, trajectory recording, and
graph integration run only when a host configures them. Memory extraction is
configurable and can be disabled.

The common evaluation substrate follows the same boundary: Code records
digest-only execution facts, reads bounded evidence, supervises isolated
auxiliary runs, exposes an immutable result contract, and projects those
values through the strict `EvaluationWireEnvelopeV1` generated for Rust,
Node.js, Python, and Go. Optional file-backed result and dispatch adapters add
bounded atomic persistence and restart-safe fencing without taking ownership
of host authorization or business retention. A host can build a reviewer or
verifier by injecting its own policy and structured executor; Core does not
define a rubric, finding vocabulary, decision threshold, UI, or Cloud audit
workflow. See
[Evaluation Substrate](manual/EVALUATION_SUBSTRATE.md).

The default system prompt is assembled in layers: a compact agent loop, the
runtime authority/run-control contract, the canonical repository-tool schema,
and shared safety boundaries. The host runtime remains authoritative for every
permission, approval, budget, cancellation, and sandbox decision; prompt text
does not grant a capability that the current session has not exposed.

Scientific workflows use the same boundary. `a3s-code-core::research` binds a
run to exact project, source, evidence, and Code/Use capability identities;
records digest-only observations; and issues provenance and review shapes that
can be rendered by a host. It deliberately leaves package resolution,
reviewer rubrics, acceptance thresholds, human approval, retention, and
publication to A3S Use and the host application. See
[Native Research Contracts](manual/RESEARCH_CONTRACTS.md).

## Configure the runtime

A3S Code uses [A3S ACL](https://github.com/A3S-Lab/ACL) for product
configuration. Keep credentials in environment variables rather than source.

```acl
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = env("ANTHROPIC_API_KEY")

  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet"
    tool_call = true
    limit = {
      context = 200000
      output = 8192
    }
  }
}

storage_backend = "file"
sessions_dir = ".a3s/sessions"
memory_dir = ".a3s/memory"
skill_dirs = [".a3s/skills"]
agent_dirs = [".a3s/agents"]

task_scheduler {
  max_active = 4
  aging_interval_ms = 30000
}
```

Every session created by an `Agent` shares this scheduler. Priorities are
`urgent`, `interactive` (the default), `foreground`, `background`, and
`maintenance`; equal priorities remain FIFO. Older non-urgent work is promoted
one level per `aging_interval_ms`, up to interactive priority, so sustained
interactive traffic cannot permanently starve background work.

`Agent::new` accepts an ACL path or inline ACL. Build sessions asynchronously
so configuration, stores, queues, MCP sources, and workspace services are
resolved before the first turn.

```rust,no_run
use a3s_code_core::{Agent, PlanningMode, SessionOptions, TaskPriority};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let options = SessionOptions::new()
        .with_planning_mode(PlanningMode::Auto)
        .with_tool_timeout(120_000)
        .with_auto_compact(true)
        .with_max_context_tokens(200_000)
        .with_auto_compact_threshold(0.8);

    let options = options.with_task_priority(TaskPriority::Interactive);

    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder("/path/to/workspace")
        .options(options)
        .build()
        .await?;

    let stats = agent.task_scheduler_stats().await?;
    let same_scheduler = session.task_scheduler_stats().await?;
    println!("active={} pending={}", stats.active, stats.pending);
    assert_eq!(stats.max_active, same_scheduler.max_active);

    Ok(())
}
```

Typed session options accept custom model clients, context providers, memory
stores, session stores, workspace backends, security providers, confirmation
providers, permission checkers, and other host-owned extensions.

### Recommended governance configurations

Keep governance explicit at the session boundary. For an interactive host,
ask by default, enable a real confirmation channel, reject on timeout, and
enable output sanitization:

```rust,no_run
use a3s_code_core::{
    hitl::{ConfirmationPolicy, TimeoutAction},
    permissions::PermissionPolicy,
    SessionOptions,
};

let interactive = SessionOptions::new()
    .with_permission_policy(PermissionPolicy::strict())
    .with_confirmation_policy(
        ConfirmationPolicy::enabled().with_timeout(30_000, TimeoutAction::Reject),
    )
    .with_default_security();
```

For an unattended host, use an explicit allow-list and deny everything else.
Do not install a disabled confirmation policy: `enabled = false` intentionally
auto-approves `Ask` decisions for compatibility. Omitting the confirmation
provider makes any unexpected `Ask` or tool-level escalation fail closed.

```rust,no_run
use a3s_code_core::{
    permissions::{PermissionDecision, PermissionPolicy},
    SessionOptions,
};

let read_only = PermissionPolicy {
    default_decision: PermissionDecision::Deny,
    ..PermissionPolicy::default()
}
.allow("read(*)")
.allow("search(*)")
.allow("ls(*)");

let unattended = SessionOptions::new()
    .with_permission_policy(read_only)
    .with_default_security();
```

`DefaultSecurityProvider` performs taint tracking and output sanitization; it
is not process isolation. Attach a `BashSandbox` for shell isolation and choose
an appropriate workspace access policy for in-process file tools. Direct
`tool()` helpers are trusted control-plane calls; use `governed_tool()` when
the host has not already authorized the exact invocation.

### Bind one exact cognitive package

`CognitiveContextSession` is the dedicated boundary for Agentic Ontology
cognitive packages. The embedding host obtains and retains the A3S Use lease,
then injects a provider together with the reviewed package, lifecycle
generation, capability snapshot, and Knowledge-surface digests:

```rust,ignore
use a3s_code_core::{CognitiveContextSession, SessionOptions};

// `binding` is reconstructed from the exact A3S Use capability snapshot.
// `use_provider` implements CognitiveContextProvider and performs cited
// search -> bounded Markdown read through the host-owned generation lease.
let cognitive = CognitiveContextSession::new(binding, use_provider)?;
let options = SessionOptions::new().with_cognitive_context(cognitive);
```

Code repeats the complete binding in every provider request, validates source
and citation digests before prompt injection, persists the binding in
`SessionSnapshotV1`, and emits `cognitive_context_bound` into the ordinary run
event stream. A resumed session requires the host to inject the same binding.
Provider failure, generation drift, a missing citation, or an attempt to add a
general RAG/graph fallback aborts the turn; personal memory is not recalled for
a cognitive-package-bound turn. Registry lookup, installation, lifecycle,
package files, and the human-review ontology graph remain outside Code.

Hosts that already project a complete A3S Use generation can stage the same
value as `CapabilityValue::Knowledge`. Exactly one value is copied into each
Run-frozen configuration. `current_cognitive_package_binding()` reports the
owned binding visible to the next Run; the older
`cognitive_package_binding()` accessor reports only the Session-static recovery
seed. On resume, publish that exact seed once before advancing to a later
Knowledge generation.

Package hosts may separately stage multiple
`CapabilityValue::KnowledgeSurface` values. Each immutable binding contains
only a public surface name, OKF format, content digest, and canonical exact
projection digests. It is non-queryable and never enters Agent context; its
purpose is to close same-source readiness edges such as `Flow -> OKF` without
implicitly selecting a cognitive package. The singular `Knowledge` value above
remains the only Run-visible cognitive authority.

### Bind reviewed A3S Use Runtime Tasks

`UseRuntimeTaskProjectionAdapter` consumes one exact `toolTasks` entry from an
A3S Use capability snapshot. The embedding host supplies a
`UseRuntimeTaskDispatcher` adapter backed by A3S Use's leased
`RuntimeTaskDispatcher`, then stages the adapter under its matching Tool
`CapabilityId` in the same Use-backed `SessionCapabilityBatch` as the rest of
that generation. Code never executes the projected command itself or writes the
Tool into the mutable compatibility registry.

Each invocation repeats the snapshot, scope, package and manifest digests,
lifecycle generation, provider, and surface identity under bounded argv,
deadline, and output contracts. A mismatched response fails closed. The
`SessionCapabilityBatch` retains the exact Use generation lease for the Run,
while the dispatcher retains its package Registry lease through Runtime output
capture and cleanup.

## Tools that respect the workspace

A tool is registered only when its workspace exposes the capability it needs.
An object-only backend does not advertise local `bash` or `git` definitions to
the model.

| Concern                     | Built-in surface                                                                                                                                                                           |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Files and directories       | Budgeted single/multi-file `read`, `write`, previewable CAS `edit`, `patch`, `ls`, and unified `search` with `grep`, `glob`, zvec-rust FTS/BM25, semantic diagnostics, and hybrid retrieval modes |
| Commands and source control | Bounded `bash` plus typed `git` operations, cancellation, and Unix process-group termination                                                                                               |
| Code intelligence           | `code_symbols`, `code_navigation`, and `code_diagnostics`; source reading and mutation remain in file tools                                                                                |
| Web evidence                | Quality-gated headless → HTTP/RSS → API `web_search` with shared admission, session circuits, and request coalescing; plus bounded `web_fetch`, source normalization, and SSRF protections |
| Downloads                   | Workspace-confined binary `download` with strict range validation, bounded parallelism, retries, checksums, and atomic publication                                                         |
| Composition                 | Safe `batch`, sandboxed QuickJS `program`, structured `generate_object`, and unified `task` delegation; the hidden `parallel_task` alias remains host-compatible                           |
| Extensibility               | `Skill`, `search_skills`, namespaced `mcp__<server>__<tool>`, and explicit `dynamic_workflow`                                                                                              |

Every invocation declares `ToolCapabilities`, including read-only,
idempotent, resumable, cancellation-safe, paginated, output-kind, and parallel
limits. `batch` parallelizes only calls that declare safe read-only behavior;
mutations and unknown tools are serialized.

Every governed and direct Tool result also carries trusted
`metadata.a3s_tool_result_evidence` using schema
`a3s.code.tool-result-evidence.v1`. The bounded record distinguishes original
and model-visible byte/token estimates, binds exact repeated content with a
SHA-256 `repeat_key`, names the estimator, declares the loss mode, and points
to an authorized immutable full-output reference, a local compatibility
artifact, or the inline digest. It is observational evidence: Core does not
claim provider billing usage and does not rewrite Tool content from these
measurements.

Content projection is controlled separately by the session-pinned
`a3s.code.tool-result-transform-policy.v1` policy. The conservative default
retains a 100 KiB prefix. `ToolResultTransformPolicyV1::context_efficient()`
retains a UTF-8-safe 64 KiB head and 32 KiB tail, folds exact repeated lines,
and samples oversized top-level JSON arrays. Rust, Node.js, Python, and Go
expose the same policy fields. The policy persists in `SessionSnapshotV1`, and
resume rejects an explicitly different policy so replay cannot silently change
the model-visible Tool result.

Every result that crosses the real Tool executor also carries
`metadata.a3s_tool_result_transform_binding` with schema
`a3s.code.tool-result-transform-binding.v1`. The binding records the exact
algorithm, a domain-separated digest of the complete policy, and its own
binding digest. Code resolves and validates it before invoking the Tool, so an
unbound result cannot be released after a side effect. Snapshot loading
validates each retained binding against the Session policy and matching Tool
result evidence. The binding identifies Code's deterministic transform; it
does not claim Cloud policy authority, tenant identity, or provider selection.

### Retain original Tool content

A managed Rust host can bind a session to an already-authorized shared content
authority without passing provider credentials, tenant lookup, or a primitive
backend selector into Core:

```rust,no_run
use a3s_code_core::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1,
    ImmutableContentAdapterSession, ImmutableContentResult, SessionOptions,
};
use std::sync::Arc;

fn session_options(
    authority_digest: String,
    adapter: Arc<dyn ImmutableContentAdapter>,
) -> ImmutableContentResult<SessionOptions> {
    let binding = ImmutableContentAdapterBindingV1::new(
        authority_digest,
        16 * 1024 * 1024,
    )?;
    let retained_content = ImmutableContentAdapterSession::new(binding, adapter)?;

    Ok(SessionOptions::new().with_immutable_content_adapter(retained_content))
}
```

The authority digest is opaque and secret-free. The host adapter receives an
exact descriptor plus borrowed bytes and must create or resolve an immutable,
content-addressed object. Code validates the returned binding, URI, SHA-256,
media type, size, and reference digest before releasing the Tool result. Every
raw output returned by a Tool is retained, including lossless bounded results;
large change sides removed from inline metadata are retained separately.
Provider failure, cancellation, byte-ceiling overflow, or reference drift
fails closed without a local copy. Full references live in
`metadata.artifact.content_reference`, and
`metadata.a3s_tool_result_evidence.content_ref` points to the same URI.

`SessionSnapshotV1` persists only the immutable-content binding and requires
the exact adapter to be re-injected on resume. Delegated children inherit it.
If no adapter is configured, the existing bounded session-local
`ArtifactStore` remains a standalone compatibility path for lossy originals;
it is not a shared content or authorization authority. Cloud remains
responsible for authorization, provider and namespace selection, tenant
projections, retention, and object lifecycle. See
[Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md).

### Context-efficient repository tools

`read` can pack 1-32 known text files into one ordered response. The shared
budget includes headers and the continuation itself, so the result reaches the
model intact instead of relying on downstream truncation:

```json
{
  "files": [
    { "path": "src/lib.rs" },
    { "path": "src/config.rs", "offset": 40, "limit": 80 }
  ],
  "max_output_bytes": 65536
}
```

If the budget fills, copy `metadata.batch.continuation` back into `files`.
Offsets and remaining per-file limits are advanced without repeating completed
lines. One missing or unreadable member is reported in its own segment while
the other files continue.

For `search` calls with `mode: "grep"`, `output_mode` controls how much
evidence enters the context:

| Mode                 | Result                                                   |
| -------------------- | -------------------------------------------------------- |
| `content`            | Matching lines with optional context (default)           |
| `files_with_matches` | Lexically cursor-paginated matching paths only           |
| `count`              | Lexically cursor-paginated matching-line counts per file |
| `summary`            | Full-scan line and file totals without rendered matches  |

The non-content modes ask built-in workspace backends to count matches without
constructing discarded match text. In `mode: "glob"`, `search` retains a
backend's recency or relevance order by default; request `sort: "path"` when
cursor pages require stable lexical ordering. Use `mode: "bm25"` for bounded
zvec-rust FTS/BM25 lexical ranking over workspace text chunks. Retrieval-enabled
manifest-backed local workspaces build one bounded, session-local chunk catalog
asynchronously and reuse its FTS postings across queries. A typed, opt-in
`WorkspaceLexicalEngine::ZvecRust` selector can use the official `zvec-rust`
binding when the product build supplies a verified `libzvec_c_api`. Minimal
`--no-default-features` builds use the explicitly reported portable BM25
implementation; product builds never silently switch engines.
Session construction does not wait for indexing; BM25 transparently uses the
session-local catalog scorer while the first snapshot is being admitted. Once a
durable generation is ready, the same `bm25` call switches to its zvec postings
without changing the model-visible tool contract. The catalog route scores
without query-time file reads. The native route verifies its bounded result
candidates against the live filesystem so an edit cannot leak stale text.
Custom workspace backends use the same selected Code-local BM25 scorer.
The CPU-heavy tokenization and per-document normalization stages use Rust's
bounded Rayon worker pool and preserve input order, while native collection
publication remains serialized behind its atomic generation boundary.

When the automatic durable projection is enabled, cold catalog admission uses
the portable scorer as its verified fallback instead of opening one native
collection per source file. The workspace-wide zvec generation remains the
native serving path once ready, so startup cost scales with source bytes while
the model-facing search contract stays unchanged.

Hosts that need explicit zvec-grep-style restart persistence can use
`WorkspaceServices::local_with_indexed_retrieval`; default local Agent
workspaces use the same path automatically. This keeps the same manifest
watcher and chunk admission policy, writes versioned zvec generations under
`.a3s-code/index`, and lets the existing `search` `bm25` mode use that index
automatically. Default local Agent workspaces now make the same best-effort
configuration; an unavailable or read-only cache falls back to the catalog.
The explicit constructor remains a compatibility convenience, so framework
users do not need to opt in or know whether the cache is available. The
persistent index is workspace-owned and FTS-only; session semantic vectors
remain owned by A3S Memory. MCP is not required and is not part of the Core
dependency graph.
Generation publication is off the query path: a changed content snapshot is
built in staging and atomically promoted, while same-content source revisions
reuse the existing native postings. Catalog updates enqueue only the newest
snapshot through a short settle window, so an editor save burst does not
trigger one full native build per intermediate revision. Transient native or
filesystem failures retry with bounded backoff, the status surface reports
when a generation is building, and obsolete generations are collected after
the new `CURRENT` is published. The release qualification entry points are
`core/examples/workspace_persistent_index_benchmark.rs` for isolated index
timings and `core/examples/workspace_persistent_index_production.rs` for a
real manifest-backed workspace. The latter reports discovery/admission,
concurrent warm-query p50/p95, same-content generation reuse, changed-content
publication, generation cleanup, and restart reopen. The complete local gate
is `scripts/workspace_search_production.sh`; pass `--full` when a release
admission needs the complete Core unit suites.

Latency-sensitive hosts may construct
`ManifestWorkspaceBackend::new_deferred` or
`new_deferred_with_access_policy`. The backend keeps ordinary local fallback
search available while its manifest is empty; calling
`backend.manifest().activate()` opens a one-way gate that starts the initial
scan and platform watcher. This lets a terminal or GUI host render its first
interactive frame before repository-scale discovery begins without weakening
workspace access or changing the eager constructors.

The compatibility default is deterministic, non-overlapping, UTF-8-safe
line/byte chunking (80 lines or 64 KiB, at most 128 chunks per file). Typed
strategies also support fixed byte windows, recursive caller-ordered separators
with bounded overlap, and a Rust host-supplied custom range splitter. Code
validates complete coverage, forward progress, UTF-8 boundaries, and all size
budgets, then owns stable IDs, line anchors, digests, and revisions. Overlap is
charged to retained-text and vector-record budgets. Catalog snapshots are
immutable and exclude generated, non-text, oversized, credential, key, and
`.a3s` control paths. File changes are tombstoned before replacement work; a
failed read reduces indexed coverage instead of returning stale text. The
catalog is session-local and is released with its manifest-backed workspace
backend; its optional persistent zvec projection is generation-versioned under
the workspace root. Hosts that share a `ManifestWorkspaceBackend` across UI,
search, and sessions configure its catalog exactly once with
`configure_chunk_catalog` before attaching `local_with_retrieval_backend`;
session options cannot
silently replace that host-owned strategy or its budgets.

No embedding or reranking model is required for the baseline workspace search:
exact, glob, zvec-rust FTS/BM25, Code Intelligence, and RRF execute locally on
CPU and remain available when Workspace Retrieval is omitted. Dense semantic
search necessarily needs a text-to-vector function, but that function may be a
host-injected in-process CPU callback; it is not required to be remote or use a
GPU. The optional deterministic MMR reranker is also model-free CPU code.

Hosts can implement the public `EmbeddingProvider` trait without adding a
model runtime to A3S Memory or Code Core. `EmbeddingExecutor` validates the provider/model
descriptor, deterministically batches caller-admitted text, enforces text and
expected-vector byte budgets before calls, propagates cancellation, applies
typed bounded retries, and rejects partial, duplicate, unknown, dimension-
mismatched, non-finite, non-normalized, or descriptor-drifted responses. Input
text and vector values are redacted from Code-owned `Debug` output and errors.
`SessionOptions::with_workspace_retrieval(WorkspaceRetrievalOptions::new(...))`
binds that contract to a session. A3S Memory is the single exact, session-owned
semantic serving projection. Embeddings are validated once, inserted into
bounded Memory partitions, and released with the session; there is no duplicate
vector projection or hidden authority selector. The lexical projection is
independent and uses zvec-rust FTS/BM25, so a lexical failure can degrade only
lexical coverage while semantic results retain their Memory contract.
Code reuses the admitted chunk catalog,
starts indexing without delaying `session_async`, coalesces chunks from the
same catalog generation across files up to the configured input, text-byte,
and expected-vector-byte limits, and publishes completed files as atomic A3S
Memory partitions. A file split across provider batches remains unpublished
until every vector has passed response validation. A newer catalog revision
cancels and discards the unpublished generation without changing already valid
partitions. `WorkspaceRetrievalOptions::with_semantic_readiness_timeout(...)`
optionally gives a first semantic or hybrid query a bounded, event-driven wait
for the current generation to become ready or degraded. Omission keeps the
compatible immediate partial fallback; the hard maximum is 30 seconds, caller
cancellation and session close interrupt the wait, and session construction
remains asynchronous.

`AgentSession::workspace_retrieval_status` reports building, ready, degraded,
or closed state, revisions, coverage, queue depth, failures, and vector memory.
Its `batching` object adds current-generation document inputs and bytes, logical
batches, physical provider requests including retries, the three-limit request
lower bound, flush reasons, time to first file-atomic publication, and the
required zero non-text-input count. Closing the session cancels the provider,
joins the owned task within a configured deadline, stops Code-owned local
manifest work, and drops all vector state. Enabled sessions add
`mode: "semantic"` and `mode: "hybrid"` to the unified `search` tool; disabled
sessions retain the existing schema. Semantic queries use bounded provider
execution and report the exact catalog/vector revisions and partial-coverage
fallback. Each candidate is reread through `WorkspaceServices` to verify its
full-file digest and exact chunk byte range before source text is rendered. A
stale, deleted, unreadable, or concurrently superseded candidate is never
exposed.

Retrieval is an explicit host capability, not a model-controlled toggle. Rust
hosts enable it with `with_workspace_retrieval(...)` and can clear an earlier
layered choice with `without_workspace_retrieval()`; Node omits
`workspaceRetrieval`, Python assigns `None`, and Go uses `nil` to keep it
disabled. Clearing the option constructs no index and makes no provider call.
Only manifest-admitted UTF-8 text and source files enter the chunk catalog.
Non-text assets are excluded before chunking and embeddings; document parsing,
OCR, and knowledge-artifact compilation belong to the separate knowledge
compiler boundary. See
[Workspace Retrieval Chunking](manual/WORKSPACE_RETRIEVAL_CHUNKING.md) for
strategy selection, custom range invariants, asynchronous construction, and
the bounded overlap-aware reranker.

Node, Python, and Go expose typed line, fixed UTF-8 window, and recursive
separator-aware strategy objects. Omission keeps line chunking; no SDK accepts
a primitive strategy name. The shared cross-SDK fixture locks identical byte
ranges and invalid-window behavior, while arbitrary custom range callbacks
remain a trusted Rust-host extension. Strategy validation precedes provider
execution, and Go completes it before callback registration.

Hybrid mode creates independent exact-literal, zvec-rust FTS/BM25, optional Code
Intelligence symbol, and positive-similarity semantic candidate lists. It
fuses one-based ranks with reciprocal-rank fusion (`k=60`) instead of mixing
uncalibrated scores. Exact ASCII identifier tokens occupy a protected tier;
deterministic tie breakers and a two-chunk-per-file cap keep RRF-only results
stable. Rust hosts can explicitly enable a second, in-memory deterministic
stage with `WorkspaceRerankOptions::deterministic()`. It examines at most 100
fused candidates, samples at most 4 KiB and 128 lexical fingerprints per
candidate, combines interval/boilerplate similarity with MMR-style diversity,
and uses at most 4 MiB of checked scratch. Exact identifiers remain protected;
an invalid configuration or scratch-budget failure preserves RRF ordering.
Node and Python hosts opt in by passing a typed
`DeterministicWorkspaceReranker` to `WorkspaceRetrievalOptions`; Go assigns
`NewDeterministicWorkspaceReranker()` to the typed `Reranker` field. Omitting
that object keeps RRF-only, and no SDK accepts a raw mode or algorithm name.
All four limits are validated before embedding/source egress; Go additionally
validates them before callback registration.
RRF-only remains the compatibility default. The Core real-DeepSeek matrix now
qualifies line, fixed-window, and recursive chunking under the deterministic
stage; a valid whole-file Rust custom splitter remains an explicit negative
control. The real CLI ACL-host composition and the public Node.js, Python, and
Go SDKs now also pass recursive 512/64 plus deterministic reranking against one
versioned corpus and normalized report contract. Each SDK completes 3/3 exact
tasks and tool protocols with Recall@5 1.0, MRR 0.5, 1.0x document-request
amplification, zero non-text inputs, and complete post-close vector release.
These three-task parity runs do not qualify a new default.
A `v7.0.1` post-release rerun at Code `5aa9642` on 2026-08-17 repeated all nine
exact tasks and one-Search protocols through the public Node.js, Python, and Go
SDKs. The three arms consumed 14,540, 14,784, and 14,171 DeepSeek tokens,
respectively; Recall@5, MRR, request amplification, non-text egress, and
post-close release remained unchanged. The same checkout also passed all three
Core DeepSeek adversarial scenarios and the Node.js/Python real-config smoke
paths. See the
[cross-SDK evaluation](sdk/evaluation/README.md#v701-post-release-rerun) for the
full diagnostic timing table and reproduction commands.
The separate compile-gated generation matrix combines the locked multilingual
embedding model with the repository-authorized DeepSeek route and passes 9/9
target-only Rust edits across three tasks, with a 0.7008 Wilson lower bound,
Recall@5 1.0, hidden-test compilation, 1.0x provider amplification, incremental
replacement, and complete release. A 64-generation churn gate verifies that a
changed file replaces rather than accumulates vectors. These results qualify a
bounded opt-in generation workflow; they still do not justify automatic
enablement. See the [operations runbook](manual/WORKSPACE_RETRIEVAL_OPERATIONS.md)
for SLOs and rollback.

The A3S CLI also ships a qualified, default-off `local_cpu` host adapter on
Linux x64/ARM64, Windows x64, and Apple Silicon. It admits a separately
installed revision- and SHA-256-bound FastEmbed/ONNX artifact set, performs no
runtime download or source egress, uses two-input microbatches and one native
job per process, and fails before model loading on unsupported x64 CPUs. Native
[CLI CI](https://github.com/A3S-Lab/CLI/actions/runs/31917686424) performs real
offline inference, cancellation, recovery, and RSS checks on every enabled
target. The locked multilingual DeepSeek task remains 3/3 with Recall@5 1.0,
exact 1.0x request amplification, and zero non-text inputs. This adds an
embedding route, not a new ranking default: RRF-only remains compatible and
the deterministic reranker remains optional.

Results report the versioned algorithm, selection/redundancy scores,
candidate and byte accounting, truncation, and fallback without exposing query
or source text. Fusion and reranking precede authoritative source access, so
each selected path is reread at most once for full-digest and exact-byte-range
verification. This Code-specific policy is not part of the generic A3S Memory
vector kernel.

A separate revision-locked real embedding model matrix now proves why provider
compatibility and model fitness are different gates. English MiniLM misses the
CJK task, while multilingual MiniLM retrieves all three targets. On the same
real vectors, RRF-only preserves ranks 2/2/2 and deterministic reranking moves
them to 5/2/3, so model selection is host-owned and neither a model nor the
optional reranker is promoted globally from this small fixture.

The locked nine-query fixture preserves the original BM25 baseline and adds an
independent hybrid result set whose deterministic provider admits only
annotated query/document pairs. Hybrid Recall@10 and MRR are 1.0 on that
fixture, improving Recall@10 by 33.3 points without reducing identifier rank.
The opt-in deterministic stage also records nDCG@10 1.0 and zero selected
near-duplicate evidence on the locked fixture. On the 25,000-record release
profile its two end-to-end signed p95 differences versus RRF were -5.163 ms and
-2.322 ms (0 ms positive addition in both runs), with 75,346 conservatively
accounted scratch bytes and no fallback. This noisy paired measurement proves
the budget, not an algorithmic speedup.

Use `edit` with `dry_run: true` to receive the exact before/after diff without
writing. The dry run is declared read-only and can be safely batched. Apply the
result with `expected_replacements` and optionally `max_replacements` to reject
stale or unexpectedly broad changes before the compare-and-swap write.

Web evidence keeps retry decisions typed. `web_search` records tier quality,
engine outcomes, durations, circuit state, and retry context; `web_fetch`
classifies transport failures and HTTP 429 separately and preserves a
parseable `Retry-After` delay. Neither path infers retryability from rendered
error prose.

### Sandbox and credential boundaries

Local sessions automatically attach the A3S-owned, fail-closed
`sandbox::native::NativeBashSandbox`. Backed by the independent
[`a3s-sandbox`](https://github.com/A3S-Lab/Sandbox) crate, it limits writes to
the active workspace and private run scratch space, protects agent control
metadata, blocks common credential reads, scrubs ambient secrets, and denies
command network access, local binding, and Unix sockets. It uses Seatbelt on
macOS, user/mount/PID/IPC/UTS namespaces plus seccomp on Linux, and AppContainer
plus a Job Object on Windows.
No Node.js or npm sandbox runtime is involved, and an unavailable native
boundary is represented by an error-only sandbox handle and never falls back to
an unsandboxed host runner. Top-level tools, workflows, and delegated child runs
inherit the same handle. Hosts use `SessionOptions::with_sandbox_handle` only to
replace the default with another equivalent isolation boundary. Non-local
workspace runners retain their explicit host-owned contracts, and only an
explicitly authorized `require_escalated` invocation may use the local host
command runner.

Shell isolation does not automatically govern in-process file tools. Local
hosts should explicitly select `LocalWorkspaceAccessPolicy::CredentialBoundary`
when direct workspace operations need the same credential boundary. See the
[Advanced Developer Manual](manual/ADVANCED_DEVELOPER_MANUAL.md) for the full
contract and host responsibilities.

## Context, memory, and models

`ContextAssembler` ranks and budgets filesystem, recent-file, ripgrep, memory,
prompt-slot, project-instruction, Skill, and custom provider inputs. Automatic
compaction is opt-in and can re-arm across long sessions. It retains the latest
request and unresolved tool calls while treating generated summaries as
untrusted transcript data.

Memory separates working, short-term, and durable state. When memory is active,
semantic extraction is enabled by default and can be disabled. It records only
validated reusable memories with source, confidence, scope, reason, workspace,
session, and schema metadata rather than mechanically persisting every tool
result or conversation turn. Supersession preserves the old V1 item for audit
but excludes it from recall.

Hosts can additionally install a typed `DurableMemorySession` bound to one
exact A3S Memory V2 tenant, principal, and scope. The current
`ShadowCandidates` mode mirrors only successful V1 extraction writes as
content-addressed, evidence-backed `Candidate` nodes. It never activates or
recalls V2 nodes, so migration can be measured without changing model context.
The opt-in `ActiveRecall` mode additionally queries only explicitly activated
nodes under a bounded lexical policy. Hosts may opt into a bounded, one-hop
expansion over explicit `RelatedTo` edges; Code never follows conflict edges,
recurses through the graph, or widens the exact namespace. The public
`preview_recall` diagnostic is pure and cannot authorize prompt injection.
The bound `a3s.memory.lexical.word-cjk-bigram.v1` profile preserves lowercase
word matching and adds overlapping bigrams for contiguous Han, Kana, Hangul,
and related CJK runs. It improves same-language phrase variation without
itself claiming cross-language or no-token-overlap semantic retrieval. Rust
hosts can explicitly attach `DurableMemorySemanticRecall`: Code executes a
revision-pinned embedding provider, searches a caller-owned A3S Memory vector
index, then treats every vector hit as an untrusted candidate. It re-reads the
exact repository namespace and requires the current Active revision and content
digest before deterministic lexical/semantic RRF. Semantic failure preserves
the lexical result, and indexing remains inert unless the host explicitly
calls refresh or installs a typed schedule. `refresh_semantic_recall` obtains
and recomputes a complete Active-only A3S Memory snapshot under node and byte
budgets, embeds it off-index, atomically replaces the exact
namespace/generation partition, and verifies the source again. An exact
namespace change token lets built-in repositories perform that final proof
without rereading the snapshot; repositories that return `None` retain the
original second-snapshot check. Drift requires partition invalidation before
the call can succeed; an invalidation error is propagated and no receipt is
returned. Pre-publication failures preserve the previous complete partition.
Successful calls return a secret-free receipt binding source digest/bytes,
an optional content-free source change token, semantic generation, node count,
vector revision, mutation consistency, and an optional exact vector-index
history token.
All publication, recovery, and query fences read the fallible asynchronous
`VectorIndex::observe()` status/token pair. The synchronous `index_status()`
surface is retained only as a locally cached diagnostic hint, so a durable
backend never needs to block a Tokio worker to satisfy that compatibility API.
Cloned sessions serialize refresh and direct replacement through the same
live-generation lock. On a backend advertising atomic index-revision CAS, Code
captures the base revision before snapshot work, conditionally publishes, and
conditionally cleans up using the published revision. Delayed independent
runtimes therefore cannot overwrite or remove a newer generation. Production
hosts can call `refresh_semantic_recall_requiring(IndexRevisionCas, ...)` to
reject a weaker backend before repository or embedding work begins.
Rust hosts can install `ScheduledSemanticRefresh` in the existing owned memory
maintenance runtime. The host selects the interval; Code rejects a missing
semantic binding or non-CAS backend before spawning, skips missed ticks, retains
the latest successful receipt on the cloned schedule handle, and completes
post-publication verification during clean bounded session shutdown. After the
first publication, an equal exact namespace change token, semantic generation,
ownership-epoch receipt, CAS-captured revision, and full index status prove a
no-change tick without a snapshot, embedding, or vector mutation. A backend
without the token retains complete bounded snapshot verification. A token
change triggers one full Active snapshot; if only inactive state changed, Code
advances the receipt without republishing. Source or index drift performs a
full verified rebuild, and a replacement owner starts without the previous
process-local receipt. A host that needs recovery across owners can serialize
`receipt.checkpoint()` and pass the decoded value to
`ScheduledSemanticRefresh::try_new_with_checkpoint`. The checkpoint deliberately
omits the repository change token because it is meaningful only within one
repository history. Its first recovered run always verifies one complete Active
snapshot and the current index. Only an equal source identity, semantic
generation, full index status, and exact vector-index history token at the same
revision can avoid provider and publication work; after that promotion, the next
stable tick can use the normal zero-snapshot namespace-token path. A missing or
different vector token, unrelated repository or vector history, colliding index
status, or any drift triggers the complete verified rebuild. Until this proof
succeeds, `last_receipt()` remains empty. Rebuilds retain one bounded,
text-free vector set for the active ownership epoch. Exact semantic record IDs
bind reuse to namespace, generation, node, revision, and content digest, so
index-only drift can republish without provider-adapter input and a partial
source change embeds only misses before atomically publishing the complete
partition. Only a post-publication verified success replaces this cache. Close
releases its vectors while keeping the receipt observable; direct explicit refreshes
remain uncached and unconditional. Cloned schedule handles also expose bounded
`metrics()` for the current ownership epoch. Cumulative counters and the latest
64 runs distinguish settled published, unchanged, and failed attempts while
measuring change-token requests and valid observations, snapshot node/byte
reads, exact cache hits and misses,
provider-adapter invocations/input bytes including retries, publication work,
and elapsed time.
These observations contain no source text, node IDs, digests, vectors, provider
identity, or error bodies; close retains them for inspection and the next owner
starts a new empty epoch. Adapter-boundary counts do not prove remote
transmission or billing; hosts correlate them with provider telemetry.

Rust hosts that enable Code's `durable-memory-sqlite` feature can inject A3S
Memory's `SqliteVectorIndex`. It preserves the exact vector history, global
revision CAS, records, and integrity accounting across a real close/reopen, so
a matching host-persisted checkpoint can recover with one source snapshot and
no repeated embedding or publication. The backend is local SQLite durability;
on Unix and Windows, copying or atomically replacing the closed database forks
its history token on next open. Restore must replace the database file rather
than overwrite it in place. Distributed lease ownership, replicated remote
CAS, failover, and production cadence remain host qualifications.
The release-only `durable_memory_semantic_refresh_benchmark` locks a local
10,000-node, 384-dimensional durability profile across initial publication,
zero-snapshot stable ticks, one-node source drift, index-only drift, a
host-synchronized checkpoint, real file/SQLite close and reopen, one-snapshot
recovery, warm semantic-query percentiles, disk ceilings, and Linux RSS. It
uses a deterministic in-process adapter and explicitly does not claim real
embedding quality, remote CAS/leases, provider billing, or remote failover.
Activation requires independent Manual or Verification evidence. Code records
admission for the exact current revision after final context assembly; an
unrecordable or stale item is removed before the model call. Exact V1/V2
content duplicates prefer the audited V2 item. The locked synthetic retrieval
fixture improves Recall@5 from `0.60` to `0.90` with relation expansion, meeting
the predeclared gate without adding a vector serving dependency. A separate
product fixture drives the same no-memory, V1, and V2 arms through real
`AgentSession` turns: task success is `0.00`, `0.60`, and `0.90`; accepted-write
precision and evidence fidelity are both `1.00`; conflicts remain
non-destructive; and selected V2 revisions record admission before model use.
A separate versioned multilingual fixture drives real `AgentSession` turns for
English, Simplified Chinese, Japanese, and Korean. It locks Recall@3 and MRR at
`1.00`, one model call and at most one memory node per task, and zero Candidate
or foreign-namespace leakage.
A versioned semantic fixture then uses English Active memories and Chinese,
Japanese, Korean, and Arabic queries with no lexical overlap. The lexical
baseline returns zero positive hits; typed semantic recall reaches Recall@1
`1.00` through real sessions with one model call, at most one context node, four
persisted admissions, and zero Candidate, foreign-namespace, or stale-vector
hits. Its declared unit vectors verify serving mechanics, not production model
quality.
A versioned multi-agent fixture then binds the same exact
`DurableMemorySession` to two independent `Agent` instances backed by one file
repository. Separate deterministic host environments deliberately emit the
same local run-ID sequence; the
`a3s.code.memory.context.session-run-invocation-sequence-sha256.v2` profile records
all three session/run admissions, exposes no Candidate or foreign-principal
content, allows one agent to continue after the other closes, and replays all
three admissions after repository restart. Durable memory is never inherited
by a delegated child: sharing remains an explicit host authority decision.
A repeated-restart fixture then closes and resumes four independent agents over
three complete process epochs with one retained run per session. Every host ID
generator resets, so retained run IDs are deliberately reused after FIFO
eviction; all 24 distinct model contexts are still admitted. A verified Active
correction reaches only the final epoch, immutable history survives four file
repository opens, and Candidate, stale-revision, and foreign-principal content
remain absent. A collision with a run that is still retained now fails before
model use instead of replacing its history.
Memory construction itself starts no tasks. Configured V1 pruning, opt-in
verified semantic refresh, and host-supplied consolidation jobs run only inside
a session-owned `MemoryMaintenanceRuntime`; jobs are serialized per schedule,
missed ticks are skipped, verified no-change semantic ticks avoid embedding and
publication, verified rebuilds reuse exact committed embeddings, and bounded
per-epoch refresh work is observable. Clean
`session.close().await` lets a published refresh finish source verification
within the total close deadline before final extraction drain. Maintenance
requires asynchronous session construction. Consolidation jobs remain
responsible for evidence, optimistic revisions, and idempotency; A3S Memory
never invents that policy.
The secret-free V2 namespace, mode, recall policy, retrieval profile, and
context-identity profile are persisted in the session snapshot. Lexical-only
sessions use binding schema 4. Semantic sessions use schema 5 and additionally
freeze the semantic authority digest, exact embedding revision and execution
policy, vector descriptor, candidate policy, and fusion profile. The live
repository, provider, and vector index remain host-owned and must be injected
again after restart; resume rejects a missing or drifted binding, including a
query, semantic generation, or admission-identity algorithm change. A real
file-repository test also proves candidate isolation before activation,
post-activation serving, access-history replay, and release of the repository
lock at session teardown. See
[Durable Memory Integration](manual/DURABLE_MEMORY.md) and
[Durable Memory Retrieval Evaluation](manual/DURABLE_MEMORY_RETRIEVAL_EVAL.md),
[Durable Memory Product Evaluation](manual/DURABLE_MEMORY_PRODUCT_EVAL.md),
[Durable Memory Multilingual Evaluation](manual/DURABLE_MEMORY_MULTILINGUAL_EVAL.md),
[Durable Memory Semantic Evaluation](manual/DURABLE_MEMORY_SEMANTIC_EVAL.md),
[Durable Memory Semantic Refresh](manual/DURABLE_MEMORY_SEMANTIC_REFRESH.md),
[Durable Memory Multi-Agent Evaluation](manual/DURABLE_MEMORY_MULTI_AGENT_EVAL.md), and
[Durable Memory Restart Endurance Evaluation](manual/DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md)
for ownership, durability, migration rules, retrieval profiles, the vector
decision, end-to-end metrics, and declared evaluation limits.

Model adapters normalize text, reasoning, images, tool calls, token usage,
streaming, cancellation, and retries. Structured generation uses native
provider response formats when available and schema validation plus repair
otherwise. Provider-facing schemas remain available as host-only validation
metadata for composite streaming clients, and clients explicitly declare
whether a blocking structured call uses a transport independent from their
streaming path. MCP supports stdio, SSE, streamable HTTP, OAuth client
credentials, refresh, and live session-scoped add/remove operations.

## Orchestration without hidden authority

- Planning can be automatic, forced, or disabled; goal tracking is opt-in.
- `AgentDefinition` and `WorkerAgentSpec` describe reusable and disposable
  workers without weakening parent policy.
- Foreground and background tasks expose progress, sources, structured output,
  cancellation, and durable task records.
- Sequential, parallel, resumable, loop, budget, and checkpoint primitives are
  available for deterministic host workflows.
- `program` runs bounded JavaScript in QuickJS with explicit tool, time,
  recursion, call-count, and output limits.
- `dynamic_workflow` is absent from a plain session until the host registers an
  A3S Flow-backed runtime.

Dynamic workflows can bound independently session-forked structured generation
with `maxConcurrentGenerations` (1-4); providers without session forking remain
single-flight. Durable completed-step recovery is bound to the exact run id,
original query, and step id rather than acting as a cross-run query cache.

Delegated tasks, workflows, and Skill child runs retain the parent sandbox and
intersect local permission policy with the parent checker. A child auto-approve
setting cannot waive a host escalation boundary.

## Events, persistence, and replay

`AgentEvent` covers text, reasoning, tools, confirmation, planning, tasks,
memory, compaction, budgets, verification, and terminal state. SDKs and run
replay project these values through `EventEnvelopeV1`, which preserves its
version, event type, complete payload, and optional metadata. Older SDK clients
can retain future event names and payloads they do not yet understand.

Governed runs add five digest-bound audit events across the Tool and unified
provider-neutral model boundaries. `tool_request_bound` records the request
origin, serialized argument bytes, and domain-separated digests of the Tool
identifier, name, and exact post-hook arguments before permission,
confirmation, budget, or execution outcomes. Denied requests therefore remain
auditable without copying their argument plaintext into the new snapshot.
`run_capability_bound` records the actual model-visible tools, workspace service
surface, run-owned governance bindings, configured serializable policy
identities, execution ceilings, and current semantic readiness/generation; it
is repeated only when that surface changes. Before every completion, streaming,
structured, or streaming-structured input, `model_presentation_bound` binds the
frozen typed Profile, its permission-filtered source count/digest/token
estimate, and the exact presented definition count/digest/token estimate. The
subsequent `model_input_bound` carries the same unique positive call sequence,
bounded counters/serialized-byte measurements, and domain-separated SHA-256
digests of the actual messages, system input, tool definitions, provider-facing
structured directive, and identified semantic/hybrid Tool results. After each
successful call, `model_usage_bound` correlates Code's prompt estimate and
normalized `LlmClient` token/cache usage with that exact input snapshot and
measures exact repeated Tool-result content under different call IDs through
bounded byte/token counters and digests; it does not claim Gateway billing
authority. Host-only validation schemas are excluded because they are not sent
to the model. The new snapshots store no Tool arguments, prompt, Tool result,
source text, vector, credential, or endpoint plaintext and exact Run replay
preserves them without a parallel audit store. Existing lifecycle events retain
their documented payloads. Digests provide integrity and correlation, not
encryption; do not export them to a less-trusted boundary merely because
plaintext is absent. See [Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md).

A configured `SessionStore` can persist complete `SessionSnapshotV1`
generations. Runs expose status, active tools, ordered event replay, exclusive
pagination cursors, and retention-gap detection. File persistence uses atomic
replacement; artifacts are bounded by item count and bytes; verification keeps
claims separate from evidence.

Rust hosts can map one complete snapshot and an optional exact between-tool-
round `LoopCheckpoint` into `SessionCheckpointExportV1`, or inject a typed
`SessionCheckpointExportSink` to receive the same canonical artifact directly
from every completed live tool-round boundary. Code closes the capability Turn,
drains all causally preceding Run events, captures the semantic snapshot, and
acknowledges persistence before the loop advances. If the Session catalog cuts
over concurrently, this checkpoint view retains the source Run's frozen
cognitive binding and complete scoped capability identity rather than the next
Run's generation. The export contains a bounded canonical JSON payload plus a
secret-free
`SessionCheckpointDescriptorV1` that separately binds the snapshot component,
logical-resume component, and complete payload by size and SHA-256. Import
recomputes every binding and rejects non-canonical bytes, schema drift, changed
rounds, foreign sessions, missing or terminal source Runs, and descriptor
drift, including a Session/source-Run cognitive or capability mismatch. Runtime API keys
remain excluded by the existing persisted-session
contract, and the export's `Debug` representation redacts payload bytes. Code
does not assign an object URI, checkpoint ID, retention rule, approval, or fork
lineage; an authorized host stores the bytes and Cloud owns those business
records. See [Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md#portable-session-checkpoints).

For recovery admission, `AgentProtocolRunRecoverExactV1` carries the complete
`SessionCheckpointDescriptorV1`. `AgentProtocolHost` validates and pins the
matching local `LoopCheckpoint` under the Session execution lease before it
captures a workspace baseline or creates the target Run. The complete request
digest settles the receipt, while the descriptor digest is part of the target
Run's immutable input identity: an overwritten boundary is rejected without a
new Run, an identical request remains replayable after source retention, and
another checkpoint cannot reuse that target Run ID.

`AgentProtocolHarness::execute_checkpoint_recovery()` additionally matches the
descriptor to the exact supplied bytes, decodes the semantic snapshot and
logical boundary from that one payload, builds an unpublished Session, and
publishes it only after exact Run admission succeeds. It performs no
snapshot-plus-loop prewrites. A persisted Session without the target Run must
match the checkpoint's semantic generation; an already persisted target uses
the normal exact replay/conflict rules, and an unrelated live Session is never
replaced. This is one Harness-visible admission, not an external datastore
transaction: Cloud still owns checkpoint authorization and revision/CAS
fencing against other writers. The existing `AgentProtocolRunRecoverV1`
command and HTTP wire contract remain unchanged for hosts that intentionally
request the latest stored boundary.

Every new logical checkpoint also carries `RunCapabilityBindingV1`: the exact
Code catalog generation and digest, the canonical complete authority-ceiling
digest, and any exact A3S Use cursor. Recovery pins and compares that identity
before reserving the target Run, so an N checkpoint cannot resume through N+1,
even when cutover races preparation. A host restoring a missing Session can use
`execute_checkpoint_recovery_with_capability_batch()` to reconstruct one exact
historical generation from untouched generation zero. Code accepts no
`latest` lookup or partial batch; mismatch leaves both Session and target Run
unpublished.

The optional state graph is a complementary coordination runtime, not hidden
session state:

```text
external or agent event
        │
        ▼
hash-linked GraphEventRecord log
        │ strict projection
        ▼
typed objects + typed relations
        │ matching behaviors
        ▼
optimistic GraphPatch → new version or explicit rejection
```

Applications opt into graph replay, branches, diffs, and Flow projection when
multiple agents or behaviors need one auditable shared model.

## Runtime surfaces

| Surface  | Package                                                        | Intended use                                                                                                                         |
| -------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| Terminal | [`a3s code`](https://github.com/A3S-Lab/CLI)                   | Interactive coding product built on Core and the shared [A3S TUI](https://github.com/A3S-Lab/TUI)                                    |
| Rust     | [`a3s-code-core`](https://crates.io/crates/a3s-code-core)      | Complete runtime API and extension traits                                                                                            |
| Node.js  | [`@a3s-lab/code`](https://www.npmjs.com/package/@a3s-lab/code) | Native N-API bindings for async lifecycle, streams, tools, stores, orchestration, MCP, and state graph                               |
| Python   | [`a3s-code`](https://pypi.org/project/a3s-code/)               | Native PyO3/bootstrap package with sync and async application APIs                                                                   |
| Go       | [`github.com/A3S-Lab/Code/sdk/go/v8`](sdk/go/README.md)        | Pure-Go client with a versioned local bridge for sessions, streams, tools, ephemeral semantic retrieval, runs, verification, and MCP |

```bash
# Node.js
npm install @a3s-lab/code

# Python
python -m pip install a3s-code

# Go
go get github.com/A3S-Lab/Code/sdk/go/v8
```

The Python release workflow in v8.2.0 uses the stable `cp310-abi3` interface,
with Apple Silicon targeting macOS 11+, Intel targeting macOS 12+, and glibc
2.28+ Linux targets for both x86_64 and arm64. Windows x86_64 and arm64 wheels
are also published. Each native wheel carries the matching Moli sidecar, while
the pure-Python bootstrap extracts it into the shared verified cache; one wheel
therefore covers CPython 3.10–3.14 on each target.

If `python3.14 -m pip` reports `No module named pip`, repair that interpreter
before installing the SDK, then install into the same interpreter:

```bash
python3.14 -m ensurepip --upgrade
python3.14 -m pip install --upgrade pip
python3.14 -m pip install a3s-code
```

On Intel Macs, the native wheel is built for macOS 12 (`x86_64`). The optional
`local_cpu` ONNX embedding adapter is not included in the Intel CLI build;
keep retrieval model-free or configure an explicitly authorized remote
embedding provider instead.

The native SDK crates explicitly enable the Core `headless-search`, `s3`, and
`serve` features to preserve their complete product surface. Direct Rust
embedders receive the lazy Moli search tier by default and can omit the browser
dependency stack with `default-features = false`. The pure-Go package uses the
matching `a3s-code-go-bridge` release asset and requires no CGO; bridge bundles
for each supported GNU/macOS/Windows target include the matching Moli sidecar.
All official SDKs expose the same ordered `sdk-capabilities` inventory, event
protocol, state-graph operations, and Moli diagnostics/provisioning APIs. Use
that contract to negotiate optional features. Node.js, Python, and Go hosts can inject typed asynchronous embedding
providers for session-owned, Memory-backed semantic and hybrid workspace
retrieval. Provider cancellation follows query and session lifecycle, and
no SDK requires a vector database service. Remote embedding admits only
conservative source paths,
rejects hard-linked aliases, and revalidates logical and resolved paths at read
time before source can leave the workspace boundary. Returned snippets are
reread and digest-checked against current authoritative source. See the
[Node.js](sdk/node/README.md), [Python](sdk/python/README.md), and
[Go](sdk/go/README.md) guides for surface-specific examples and intentional API
differences.

## Architecture

```text
Rust host / Node SDK / Python SDK / Go SDK / a3s code
                         │
                       Agent
                         │
                    AgentSession
        ┌────────────────┼────────────────┐
        │                │                │
 context + memory   model adapters   governed tools
        │                │                │
        └────────────────┼────────────────┘
                         │
       events + runs + traces + artifacts + snapshots
                         │
              optional StateGraph / Flow bridge
```

Core owns lifecycle, ordering, and execution contracts. Public extension
boundaries include `LlmClient`, `ContextProvider`, `MemoryStore`,
`SessionStore`, workspace service traits, tools, permissions, confirmations,
hooks, security, MCP transports, and graph stores.

Installable cognitive packages remain owned by A3S Use. Code consumes their
exact immutable capability generations and projects local Tool, Skill, Agent,
Command, Hook, MCP, Context, Flow, Knowledge, and UI values onto typed
Session/Run scopes with atomic publication and reversible effects. The ownership,
generation, lifecycle, and compatibility contract is defined in the
[Scoped Capability Architecture](manual/SCOPED_CAPABILITY_ARCHITECTURE.md).

The identity slice is delivered in [`core/src/capability`](core/src/capability):
typed Use package/cursor and local catalog generations, sealed source-owned
descriptor batches, and a bounded canonical `CapabilitySet`. Construction
returns an immutable `Arc`; an empty product projection still retains its Use
cursor, while mixed cursors, Built-in shadowing, conflicts, missing
dependencies, and resource-bound overflow fail before a reader can pin the
set. Runtime values remain outside that deterministic identity type.

The lifecycle slice adds sealed `CapabilityScope<Session/Run/Turn/Subtask>`
markers and catalog-bound `CapabilityCeiling` values. Borrowed typed leases
cannot outlive or impersonate another scope kind; child scopes can only remove
capabilities, workspace operations, and execution budget while retaining every
required parent governance guard. A Run over a Use-backed catalog must consume
the exact non-clone Use snapshot lease. Its supervisor owns all child scopes,
tasks, reversible effects, and that upstream lease, then closes them in bounded
reverse order with the Use lease released last.

The execution-composition slice makes those scopes operational rather than
descriptive. One cancellation tree now roots the host invocation and admitted
capability hierarchy. Every provider response and its Tool calls share one
Turn; foreground delegation composes `Turn -> Subtask -> Turn`, while explicit
background delegation is promoted beyond the invoking Turn but remains
Run-supervised. Run close settles that work before releasing the exact Use
lease, so no task or reversible effect silently escapes its temporal owner.

The projection slice adds a closed `CapabilityValue` plane, immutable
`CapabilityProjection` generations, non-clone reader leases, and
`CapabilityTxn<Staged/Prepared/Validated>`. Only a validated transaction can
commit through the catalog's generation-and-digest CAS. Failed preparation,
validation, cancellation, or a lost commit race leaves the current generation
unchanged and moves prepared effects to bounded reverse cleanup. Retired effects
remain pinned until the last old projection lease is released. The closed value
plane includes bounded `UiBinding` documents; UI name, content-digest, surface-
digest, role, dependency-kind, and size drift fail before publication.

Delivered `HOST-CAP1` lets a Session apply a complete capability
generation through `SessionCapabilityBatch`. Publication atomically binds the
projection and its generation-specific A3S Use lease provider. Every Run pins
one projection, freezes the compatibility Tool/Skill maps, acquires a fresh
real Use snapshot lease for the exact cursor, and uses the same Tool `Arc` for
model definition and governed execution. Old Runs keep N while later Runs see
N+1; cancellation, close, preparation failure, and name conflict do not expose
a partial generation. Each admitted Run and live checkpoint retain a canonical
catalog-plus-ceiling binding; recovery verifies it before target admission or
performs one exact host-supplied bootstrap on a fresh Session. Compatibility
Tool, Skill, and MCP-wrapper APIs cannot
shadow a published projection. The CLI now uses the batch for resident
sessions and a short-lived Code Exec runtime that stops Use discovery before
Run admission. Desktop probes and requires that exact host contract, then
accepts success only with canonical Code catalog and Use snapshot evidence.
Knowledge is Run-frozen through the separately persisted exact cognitive
boundary described below. Flow and UI are host-consumed through the exact
`projected_flow` and `projected_ui` handles described below; neither is silently
converted into a model-visible Tool.

Delivered `HOST-AGENT1` extends that batch to typed Agent definitions without
moving package authority into Code. Every Run merges compatibility and
projected Agents into an independent `AgentRegistry` name map while sharing
their exact immutable `Arc<AgentDefinition>` values; automatic selection,
`task`, and `parallel_task` bind to that same registry. Canonical aliases
cannot shadow each other across the compatibility boundary, and later worker
or agent-directory registration cannot replace a published Agent. An admitted
N Run continues to delegate through N after N+1 publication and retains N's
exact A3S Use lease through foreground child completion.

Delivered `HOST-COMMAND1` extends the same batch and Run admission boundary to
slash Commands. Each blocking or streaming dispatch freezes the compatibility
Command map, merges the projected generation without cloning Command objects,
and executes through that snapshot. Built-in and compatibility name conflicts
fail before publication, including the legacy mutable registry path. An N
Command already executing during N+1 publication continues through N and
retains N's exact A3S Use lease until execution completes.

Delivered `HOST-HOOK1` extends the batch to immutable `HookBinding` values that
pair one Hook definition with its exact handler. Run admission merges projected
bindings with a frozen compatibility Hook snapshot and composes them after an
optional Session-static external executor; an external `Skip` cannot bypass
projected policy. Session/Skill lifecycle event types fail before publication,
official SDK registration updates definition and callback atomically, and
detached observations plus timed-out blocking callbacks settle under the Run
supervisor before its exact A3S Use lease is released within the configured
close deadline.

Delivered `HOST-MCP1` extends the Core batch to immutable per-server
`McpBinding` values. Each binding freezes one exact initialized `McpClient` and
one sorted, bounded `tools/list` result; Run wrappers call the raw tool through
that client instead of resolving a mutable `McpManager`. N Runs and foreground
delegated children therefore retain N definitions and N callers, while the
parent Run retains N's separate exact A3S Use snapshot lease across N+1
publication. Connection preparation is a reversible Code effect, cancellation
cannot advance the catalog, and cleanup closes the old connection only after
the final old projection reader drops.
The adapter consumes host-constructed configuration derived from already
selected Use Runtime/Gateway evidence. Code does not inspect packages, resolve
opaque `gateway:*` endpoint identities, choose providers, or own Use cutover,
route drain, and recovery. A3S Use and the official CLI now project each exact
extension MCP surface through this seam. The one-shot CLI/Desktop host composes
its trusted Runtime/private Gateway lazily only when an admitted Streamable HTTP
surface asks it to resolve opaque provider/reference/path evidence; stdio-only
generations start neither. It retains that process-owned host until the Session
closes the projected clients and then shuts the Gateway down. Adoption by the
remaining official hosts stays a separate integration boundary.

Delivered `HOST-CONTEXT1` admits general `ContextProvider` values through the
same batch and copies their exact `Arc` values into each Run-frozen Agent
configuration. A Run admitted on N keeps N providers and the exact N Use lease
after N+1 publication. Descriptor/provider name drift, collisions with
Session-static providers, and attempts to smuggle a persisted cognitive package
binding through the general Context category fail before catalog publication.
Delegated children intentionally keep isolated prompt context, so dropping the
parent Context surface is a monotonic child-scope narrowing rather than a lookup
of Session-latest providers. Knowledge remains a distinct exact-authority cut;
UI follows the distinct host-only cut below.

Delivered `HOST-FLOW1` replaces the anonymous Flow runtime value with a named
`FlowBinding`. `WorkflowSpec::name` is the public capability name and the
binding pairs that exact durable spec with the `FlowEngine` that owns its event
store, runtime, observer, replay, and runtime-build compatibility. A host calls
`AgentSession::projected_flow` to receive a non-clone handle retaining the exact
Code projection and A3S Use lease. An N handle continues through N after N+1
publication; incompatible runtime builds and descriptor/spec name drift fail
before publication, missing lookup acquires no lease, and Session close cancels
active replay. Flow remains host-only unless an explicit governed Tool adapter
is installed. The resident CLI now adapts dependency-free, Tool-dependent,
MCP-dependent, and OKF-dependent A3S Use Flows through this boundary: it re-verifies and
digest-stages source, completes workspace-local Native TypeScript preflight,
and publishes the exact binding with same-package Tool, MCP, and digest-bound
Knowledge Surface dependencies.
Failed preflight or cancelled workspace lock contention leaves the visible
generation unchanged. Dynamic multi-scope OKF search remains a separate
compatibility-owned query adapter rather than becoming Flow authority.

Delivered `HOST-KNOWLEDGE1` admits exactly one `CognitiveContextSession` through
the atomic Session batch and installs its exact provider only in the admitted
Run configuration. An N Run retains N's cognitive provider, package binding,
and A3S Use lease across N+1 publication. Every `RunSnapshot` records its own
exact cognitive binding, while `SessionSnapshotV1` records the binding visible
to the next Run, so old Run evidence remains valid after cutover. A resumed or
Session-static provider is a recovery seed: the first projected Knowledge value
must reproduce that exact binding before later generations can advance, and
removal cannot reveal the stale seed. Multiple Knowledge authorities and any
mix with general-purpose host Context fail before publication. The Knowledge
host retains OKF validation, indexing, retrieval, retention, and exact query
lease ownership.

The same gate also admits multiple immutable `KnowledgeSurfaceBinding` values
as readiness-only evidence. Their canonical digest binds format, content, and
exact projection digests; they expose no retrieval method and do not count as
cognitive authorities. An admitted Run pins them with the same Code and Use
generations, allowing dependent host capabilities to reject missing or mixed
OKF evidence before publication.

Delivered `HOST-UI1` admits immutable `UiBinding` values through the same
atomic Session batch. Each binding contains bounded, path-free entry HTML plus
ordered CSS and JavaScript bytes, verified content identities, presentation
metadata, and a canonical surface digest. Descriptor names and surface digests
must match the binding exactly, and UI readiness edges may target only Tool,
Skill, MCP, or Flow values in the same generation. A host calls
`AgentSession::projected_ui` to receive a non-clone handle retaining that exact
document, Code generation, and fresh A3S Use lease across N+1 publication;
missing lookup acquires no lease and Session close signals cancellation. Core
does not parse or render HTML, own origin/CSP/navigation/state, expose ambient
filesystem/network/process/secret authority, or route UI backend messages.
A3S Use now publishes versioned, complete canonical UI dependency and managed
MCP evidence, and the resident CLI revalidates it before staging eligible
managed MCP, Skill, provider-qualified Runtime Tool Task, dependency-closed
Flow, Knowledge Surface, and UI values in one batch. Tool-, MCP-, and
OKF-dependent Flow edges resolve against the same exact package generation;
provider absence or missing
evidence fails before publication, and neither Runtime Tool nor extension MCP
uses a compatibility registration. Official renderer-host adoption remains
separate integration work. The scoped CLI/Desktop host intentionally retains
its narrower managed-MCP/Skill/UI cut, with lazy trusted HTTP Runtime/Gateway
composition and an explicit Session-close-before-Gateway-shutdown lifetime.

Delivered `CAP-PROFILE1` adds one closed
`ToolPresentationProfileV1` to the Session and Run. Adaptive preserves the
historical prompt-sensitive selector, Direct presents every permission-visible
definition, Code presents the existing `program` Tool with a bounded compact
signature catalog, and Disabled presents none. Permission filtering always
runs first; Profile projection cannot add a Tool name or change its parameter
schema. The exact Profile persists across resume, delegated runs inherit the
parent ceiling, and Node.js, Python, and Go expose typed Profile objects. This
is a model-presentation plane only: A3S Use still owns package resolution,
Grants, generations, cutover, leases, and recovery, while governed execution
uses the same pinned Tool `Arc` values.

The readiness slice derives a bounded `CapabilityReadinessPlan` from only the
surface edges already present in that immutable set. Deterministic minimal
waves prepare prerequisites before dependents; cycles and incomplete staged
batches fail before any adapter starts, while a prerequisite failure blocks
dependent activation and rolls completed effects back in reverse order. The
plan retains the set's generation, digest, and exact Use cursor boundary. Code
does not inspect package manifests or perform Use dependency resolution,
installation, Grants, lifecycle cutover, or recovery.

Source is grouped by concern under `agent_api/`, `tools/`, `workspace/`,
`context/`, `llm/`, `mcp/`, `orchestration/`, `store/`, and `state_graph/`.
Node.js and Python bindings remain separate native crates over the same Core.
The Go SDK reaches that Core through a long-lived, capability-checked local
bridge process.

## Filesystem-first agents and releases

`AgentDir` keeps reusable agents reviewable as files:

```text
agent-dir/
├── instructions.md
├── agent.acl
├── skills/
├── tools/
└── schedules/
```

Tool specifications can connect MCP servers or bounded PTC scripts. With the
`serve` feature, a host can serve an agent directory and run cron schedules.
The observable daemon handle becomes ready only after schedules, sessions, and
tools are prepared; shutdown cancels in-flight work, closes owned sessions, and
joins within a bounded deadline. Files never bypass workspace, permission,
confirmation, or verification policy.

`AgentReleaseManifest` admits the versioned `.a3s/asset.acl` contract, derives
schema-aware canonical ACL and a SHA-256 identity, and verifies runtime
compatibility before activation. Secret declarations are typed injection slots;
values remain outside the release document. After an OCI image is built,
`bind_publication` replaces only its artifact digest and the exact declared
provenance references, then re-admits the final canonical manifest. This avoids
the impossible self-reference of embedding a manifest in the image whose digest
that manifest declares.

Release admission validates metadata. It does **not** build or run an OCI
artifact, implement health behavior, or own deployment lifecycle. The
[minimal publication fixture](fixtures/agent-release-contract/README.md)
packages the separate `a3s code harness` executable, publishes one OCI image
manifest, generates the final ACL after digest resolution, retains the exact
canonical builder provenance object bound by that ACL, and can verify the
digest-pinned lifecycle through local Docker. Read the
[Agent Release Contract](manual/AGENT_RELEASE_CONTRACT.md) before integrating
the v1 schema or claiming external Runtime certification.

## Explicit boundaries

- Core is an embeddable runtime, not a hosted agent service or a terminal
  widget library.
- The separate A3S CLI owns the interactive TUI, account adapters, presentation
  policy, and optional A3S OS integration.
- Hosts own user identity, credential access, deployment policy, and trust
  decisions around direct host tool calls.
- Sandboxing, persistence, automatic compaction, goals, delegation, and graph
  projection require explicit host configuration; memory extraction remains
  host-configurable.
- A session permits one transcript-affecting operation at a time; concurrent
  send, stream, attachment, command, or resume operations fail fast.

## Documentation

| Guide                                                                                                                | Focus                                                                                                                                                         |
| -------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [User Guide](manual/USER_GUIDE.md) · [Chinese](manual/USER_GUIDE_CN.md)                                              | Installation, configuration, sessions, tools, and common workflows                                                                                            |
| [Advanced Developer Manual](manual/ADVANCED_DEVELOPER_MANUAL.md) · [Chinese](manual/ADVANCED_DEVELOPER_MANUAL_CN.md) | Extension contracts, security, lifecycle, and production integration                                                                                          |
| [SDK API Design](manual/SDK_API_DESIGN.md)                                                                           | Cross-language API conventions and alignment                                                                                                                  |
| [Capability Verification](manual/CAPABILITY_VERIFICATION.md)                                                         | First-principles evidence ledger for every advertised capability, SDK runtime gates, evidence-gap closure, and performance policy                             |
| [Scoped Capability Architecture](manual/SCOPED_CAPABILITY_ARCHITECTURE.md)                                           | A3S Use ownership, typed scopes, immutable generations, reversible effects, migration gates, and verification invariants                                      |
| [Performance Qualification](manual/PERFORMANCE_QUALIFICATION.md)                                                     | Release-profile workloads, inclusion rules, p50/p95/max results, resource ceilings, hermetic integrations, run links, and artifact digests                    |
| [Harness Model-Call Evidence](manual/HARNESS_EVIDENCE.md)                                                            | Capability/input/usage snapshots, repeated-context diagnostics, event ordering, redaction boundary, validation, and replay                                    |
| [Evaluation Substrate](manual/EVALUATION_SUBSTRATE.md)                                                               | Provider-neutral execution facts, bounded evidence, isolated auxiliary runs, restart-safe dispatch, durable result CAS, and ownership boundaries                |
| [Go SDK](sdk/go/README.md)                                                                                           | Bridge installation, sessions, event streaming, direct tools, errors, and release compatibility                                                               |
| [Code Intelligence Design](manual/CODE_INTELLIGENCE_DESIGN.md)                                                       | Language runtime, capability boundary, lifecycle, and verification                                                                                            |
| [Workspace Retrieval Baseline](manual/WORKSPACE_RETRIEVAL_BASELINE.md)                                               | Architecture, quality budgets, lifecycle, and adversarial trust boundaries                                                                                    |
| [Workspace Retrieval Qualification](manual/WORKSPACE_RETRIEVAL_QA.md)                                                | Release tests, independent oracles, performance evidence, and DeepSeek E2E scope                                                                              |
| [Workspace Search Real-Model Qualification](manual/WORKSPACE_SEARCH_REAL_LLM.md)                                   | One bounded ACL-model gate for autonomous search-mode selection and transparent native zvec acceleration                                                     |
| [Workspace Search Production Qualification](manual/WORKSPACE_SEARCH_PRODUCTION.md)                               | Deterministic native/portable tests plus a real manifest-backed scale, concurrency, rebuild, cleanup, and restart gate                              |
| [Workspace Retrieval DeepSeek Evaluation](manual/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md)                               | Paired task/rerank ablations, built-in chunk matrix, cross-SDK real-model parity, custom negative control, non-text boundary, metrics, and batching follow-up |
| [Workspace Retrieval Chunking](manual/WORKSPACE_RETRIEVAL_CHUNKING.md)                                               | Built-in/custom strategies, validation, async lifecycle, non-text boundary, and rerank plan                                                                   |
| [Workspace Retrieval Operations](manual/WORKSPACE_RETRIEVAL_OPERATIONS.md)                                           | Production SLOs, telemetry, state response, generation gates, and configuration-only rollback                                                                 |
| [Workspace Retrieval Backends](manual/WORKSPACE_RETRIEVAL_BACKENDS.md)                                              | zvec-rust lexical indexing, Memory semantic vectors, resource bounds, packaging, and rollback                                                                       |
| [Agent Directory Tools](manual/AGENT_DIR_TOOLS_DESIGN.md)                                                            | Filesystem-first tool and agent definitions                                                                                                                   |
| [Agent Release Contract](manual/AGENT_RELEASE_CONTRACT.md)                                                           | Admission schema, identity, compatibility, and security boundary                                                                                              |
| [Changelog](CHANGELOG.md)                                                                                            | Release history and migration-relevant changes                                                                                                                |

## Development

Run checks from the A3S Code repository directory:

```bash
python3 scripts/check_scoped_capability_architecture.py
python3 scripts/check_capability_verification.py
cargo fmt --all -- --check
cargo test -p a3s-code-core
cargo test -p a3s-code-core --all-features
cargo clippy -p a3s-code-core --all-targets --all-features -- -D warnings
node scripts/sdk_api_alignment_check.mjs
cargo test -p a3s-code-go-bridge
go -C sdk/go test ./...
cargo run --release -p a3s-code-core --example workspace_retrieval_benchmark
```

The capability checker keeps all 27 advertised product areas connected to the
evidence ledger. Dedicated CI jobs build and load the Node.js and Python native
modules before running their host-language contracts; a successful Rust
`cargo check` alone is not counted as SDK runtime evidence.

The retrieval benchmark emits schema-v5 JSON. It keeps the locked 25,000 x 384
exact-vector gate separate from a four-file, 512-chunk native lexical/hybrid
profile, and fails when either p95 budget, batching, rerank, or cleanup gate is
exceeded. See the
[qualification report](manual/WORKSPACE_RETRIEVAL_QA.md) for the reference
profile, inclusion rules, and measured results.

The targeted [Performance Qualification](.github/workflows/performance.yml)
workflow runs release-mode convergence, retrieval, Flow/State Graph, Code
Intelligence, context/memory, durable semantic refresh/SQLite recovery, and
persistence profiles when their critical paths change and on a weekly schedule.
It retains machine-readable JSON artifacts. The
[qualification record](manual/PERFORMANCE_QUALIFICATION.md)
captures workload and inclusion rules, observed percentiles, resource results,
run links, and artifact digests. Ordinary CI gates deterministic work
amplification and resource ceilings; remote model and public search-engine
latency is reported separately rather than treated as a stable Core speed
measurement.

Real-provider and public search-engine tests are ignored unless their external
prerequisites are configured. Required hermetic CI separately drives pinned
MinIO, workflow-managed Chrome/CDP, and a local OpenTelemetry Collector through
the production integration boundaries.

Run the context-tool real-LLM suite through a local Codex login:

```bash
A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN=1 scripts/context_tools_real_llm.sh
```

Alternatively, point `A3S_CONFIG_FILE` at an ACL provider configuration and
run the same script without `A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN`.

Run the serial, quota-consuming DeepSeek adversarial E2E suite against an ACL
configuration whose `default_model` uses the `deepseek` provider:

```bash
A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  cargo test -p a3s-code-core --test test_deepseek_adversarial_e2e -- \
  --ignored --test-threads=1 --nocapture
```

The suite uses disposable workspaces and an in-memory memory store. It proves
model-driven prompt-injection containment, absolute-path workspace isolation,
secret redaction, and cancellation of an already-started command before its
post-cancellation side effect. It never logs provider credentials.

## License

[MIT](LICENSE)
