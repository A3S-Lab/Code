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
versioned events, ephemeral workspace retrieval, and durable evidence behind
explicit contracts. Use it from Rust, Node.js, Python, Go, or through the
`a3s code` terminal application.

<p align="center">
  <a href="#start-in-60-seconds">Start</a> ·
  <a href="#whats-new-in-70">v7</a> ·
  <a href="#why-a3s-code">Why Code</a> ·
  <a href="#capability-map">Capabilities</a> ·
  <a href="#configure-the-runtime">Configure</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#documentation">Documentation</a>
</p>

## What's new in 7.0

- **Session-owned workspace retrieval.** A bounded chunk catalog, incremental
  BM25, optional host-injected embeddings, exact in-memory vector partitions,
  and deterministic hybrid RRF are built asynchronously and released with the
  session. A3S Code does not require a vector database.
- **Useful without embedding or reranking models.** Exact search, glob, BM25,
  Code Intelligence, RRF fusion, and the optional deterministic MMR reranker
  run locally on CPU. Dense semantic search is opt-in and can use an in-process
  CPU callback supplied by the host.
- **Typed retrieval across every SDK.** Rust, Node.js, Python, and Go expose
  explicit enable/disable controls, line/fixed/recursive chunking, readiness,
  lifecycle metrics, source verification, and bounded cleanup. Non-text assets
  are rejected before chunking or embedding.
- **Evidence at the model boundary.** Versioned run records bind the effective
  tools, policy identities, retrieval generation, repeated Tool-result
  context, input shape, and normalized model usage without retaining new
  prompt, source, vector, credential, or endpoint plaintext.

Go consumers must update the module path to
`github.com/A3S-Lab/Code/sdk/go/v7`. See [CHANGELOG.md](CHANGELOG.md) for the
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

## Why A3S Code

| Requirement                             | Runtime mechanism                                                                                                                                                                 |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Govern every side effect**            | JSON argument validation, typed tool capabilities, permission policy, human confirmation, hooks, budgets, security providers, and cancellation share one invocation path.         |
| **Keep context bounded**                | Reads, searches, command output, Git results, and fetched pages expose ranges or cursors. Large evidence moves into bounded artifacts with previews, sizes, and hashes.           |
| **Own the UI without forking the loop** | Core emits `AgentEvent`; SDK streams and persisted runs use the lossless `EventEnvelopeV1` protocol. The host chooses presentation, identity, credentials, and deployment policy. |
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

The Core crate enables lazy Chrome/Chromium-backed search by default. Minimal
embeddings can use `default-features = false`; cloud backends, serving, and
telemetry remain opt-in.

| Area                    | What is available                                                                                                                                                                                                                       | Activation                                                                                                                                                                |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent runtime           | Async `Agent`, workspace-bound `AgentSession`, send, stream, resume, replace, cancel, close, and replay                                                                                                                                 | Baseline                                                                                                                                                                  |
| Governed tools          | Files, search, shell, Git, web, structured generation, batch, program, Skills, MCP, delegation, deterministic result projection, and evidence                                                                                           | Exposed only when workspace and policy allow                                                                                                                              |
| Code intelligence       | Saved-file symbols, definitions, declarations, references, implementations, diagnostics, revisions, and stale-state metadata                                                                                                            | Host-selected local workspace                                                                                                                                             |
| Workspace retrieval     | Asynchronous session-owned chunk catalog, incremental BM25, optional host-injected embeddings, exact in-memory vectors, hybrid RRF, optional deterministic CPU reranking, readiness metrics, and digest-verified current-source results | Explicit per-session opt-in for semantic/vector work; baseline lexical and symbol search needs no embedding model or vector database                                      |
| Context and memory      | Ranked context, repeated compaction, three-tier memory, typed stores, recall, extraction, relations, and pruning                                                                                                                        | Host-selected and configurable                                                                                                                                            |
| Cognitive packages      | Exact A3S Use generation binding, host-injected cited Markdown provider, bounded source verification, restart checks, and fail-closed retrieval                                                                                         | Rust host injects `CognitiveContextSession`; Code never installs or resolves packages                                                                                     |
| Model adapters          | Anthropic, Zhipu, OpenAI-compatible APIs, and custom `LlmClient` implementations                                                                                                                                                        | Configuration or host injection                                                                                                                                           |
| Structured output       | Native provider formats or schema-validated prompt, partial parse, and repair fallback                                                                                                                                                  | Baseline                                                                                                                                                                  |
| MCP and Skills          | Isolated MCP transports plus filesystem, registry, inline, and live session Skills                                                                                                                                                      | Configuration or live registration                                                                                                                                        |
| Planning and delegation | Optional plans and goals, foreground/background workers, bounded parallel tasks, progress, and targeted cancellation                                                                                                                    | Manual tools independently configurable; automation opt-in                                                                                                                |
| Priority scheduling     | Agent-wide `a3s-lane` priority/FIFO admission across sessions, direct tools, detached background children, and host workflows, with cancellation, starvation-safe aging, and occupancy snapshots                                        | Baseline; tune `task_scheduler`, select per-session `TaskPriority`, inspect `task_scheduler_stats()`                                                                      |
| Programmable workflows  | Bounded QuickJS `program` calls and replayable A3S Flow-backed dynamic workflows                                                                                                                                                        | `program` baseline; dynamic runtime explicitly registered                                                                                                                 |
| Persistence             | Atomic snapshots, run events, traces, artifacts, verification, checkpoints, and optional RL trajectories                                                                                                                                | Configured store and host policy                                                                                                                                          |
| State graph             | Hash-linked events, typed objects and relations, optimistic patches, strict replay, forks, diffs, and Flow 0.11 lifecycle projection including cancellation, terminal outcomes, progress, and child operations                          | Explicit application use                                                                                                                                                  |
| Agent release contract  | Bounded `.a3s/asset.acl` admission, canonical identity, provenance binding, and compatibility checks                                                                                                                                    | Baseline admission API                                                                                                                                                    |
| Headless Agent protocol | Exact release/session/run start, cancellation, checkpoint recovery, receipts, bounded `EventEnvelopeV1` pages, per-conversation detached Git worktrees, and immutable `/v1/agent/changes` patches                                       | `AgentProtocolHarness` multiplexes ordinary Code sessions and `AgentProtocolHost` executes through each `AgentSession`; the `a3s code` process supplies service transport |
| Headless web search     | Lazy Chrome/Chromium-backed Google/Baidu engines and managed browser lifecycle APIs; Lightpanda remains configurable                                                                                                                    | Default Cargo feature `headless-search`; disable with `default-features = false`                                                                                          |
| S3 workspace            | S3-compatible object backend                                                                                                                                                                                                            | Cargo feature `s3`                                                                                                                                                        |
| Filesystem agent server | Agent-directory cron serving with post-preparation readiness, typed failure state, and bounded joined shutdown                                                                                                                          | Cargo feature `serve`                                                                                                                                                     |
| OpenTelemetry           | OTLP export in addition to baseline `tracing`                                                                                                                                                                                           | Cargo feature `telemetry`                                                                                                                                                 |

Availability never bypasses policy. Auto-save, automatic compaction, goals,
automatic delegation, sandboxing, human approval, trajectory recording, and
graph integration run only when a host configures them. Memory extraction is
configurable and can be disabled.

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

## Tools that respect the workspace

A tool is registered only when its workspace exposes the capability it needs.
An object-only backend does not advertise local `bash` or `git` definitions to
the model.

| Concern                     | Built-in surface                                                                                                                                                                           |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Files and directories       | Budgeted single/multi-file `read`, `write`, previewable CAS `edit`, `patch`, `ls`, and unified `search` with `grep`, `glob`, native BM25, semantic diagnostics, and hybrid retrieval modes |
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
to either the persisted full-output artifact or the inline digest. It is
observational evidence: Core does not claim provider billing usage and does not
rewrite Tool content from these measurements.

Content projection is controlled separately by the session-pinned
`a3s.code.tool-result-transform-policy.v1` policy. The conservative default
retains a 100 KiB prefix. `ToolResultTransformPolicyV1::context_efficient()`
retains a UTF-8-safe 64 KiB head and 32 KiB tail, folds exact repeated lines,
and samples oversized top-level JSON arrays. Rust, Node.js, Python, and Go
expose the same policy fields. The policy persists in `SessionSnapshotV1`, and
resume rejects an explicitly different policy so replay cannot silently change
the model-visible Tool result.

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
dependency-free lexical ranking over workspace text chunks. Retrieval-enabled
manifest-backed local workspaces build one bounded, session-local chunk catalog
asynchronously and reuse its incremental BM25 postings across queries. Session
construction does not wait for indexing; BM25 transparently uses the existing
query-time scanner until the first catalog revision is ready. Catalog metadata
reports `mode: "incremental_catalog"` and zero query-time file reads. Custom
workspace backends keep the compatible scanner path unless they provide a
catalog capability. Plain manifest and Code Intelligence sessions do not start
this additional catalog work.

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
catalog is ephemeral and is released with its manifest-backed workspace
backend. Hosts that share a `ManifestWorkspaceBackend` across UI, search, and
sessions configure its catalog exactly once with `configure_chunk_catalog`
before attaching `local_with_retrieval_backend`; session options cannot
silently replace that host-owned strategy or its budgets.

No embedding or reranking model is required for the baseline workspace search:
exact, glob, incremental BM25, Code Intelligence, and RRF execute locally on
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
binds that contract to a session. The resulting vector index is an exact,
session-owned in-memory projection: it is neither durable nor shared across
sessions, and recreating a session rebuilds it from admitted source. Code
reuses the admitted chunk catalog,
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

Hybrid mode creates independent exact-literal, incremental BM25, optional Code
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

Hosts can attach a `BashSandbox`. The fail-closed local `SrtBashSandbox` limits
writes to the active workspace and private run scratch space, protects agent
control metadata, blocks common credential reads, scrubs ambient secrets, and
denies command network access, local binding, and Unix sockets. It never falls
back to an unsandboxed host runner if its configured runtime fails.

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
result or conversation turn.

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

Governed model calls add three audit events at the same unified provider-neutral
boundary. `run_capability_bound` records a versioned digest-bound snapshot of
the actual model-visible tools, workspace service surface, run-owned governance
bindings, configured serializable policy identities, execution ceilings, and
current semantic readiness/generation; it is repeated only when that
surface changes. Every completion, streaming, structured, or streaming-
structured call emits `model_input_bound` with a unique positive call sequence,
bounded counters/serialized-byte measurements, and domain-separated SHA-256
digests of the actual messages, system input, tool definitions, provider-facing
structured directive, and identified semantic/hybrid Tool results. After each
successful call, `model_usage_bound` correlates Code's prompt estimate and
normalized `LlmClient` token/cache usage with that exact input snapshot and
measures exact repeated Tool-result content under different call IDs through
bounded byte/token counters and digests; it does not claim Gateway billing
authority. Host-only validation schemas are excluded because they are not sent
to the model. The new evidence stores no prompt, Tool result, source text,
vector, credential, or endpoint plaintext and exact Run replay preserves it
without a parallel audit store. Digests provide integrity and correlation, not
encryption; do not export them to a less-trusted boundary merely because
plaintext is absent. See
[Harness Model-Call Evidence](manual/HARNESS_EVIDENCE.md).

A configured `SessionStore` can persist complete `SessionSnapshotV1`
generations. Runs expose status, active tools, ordered event replay, exclusive
pagination cursors, and retention-gap detection. File persistence uses atomic
replacement; artifacts are bounded by item count and bytes; verification keeps
claims separate from evidence.

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
| Go       | [`github.com/A3S-Lab/Code/sdk/go/v7`](sdk/go/README.md)        | Pure-Go client with a versioned local bridge for sessions, streams, tools, ephemeral semantic retrieval, runs, verification, and MCP |

```bash
# Node.js
npm install @a3s-lab/code

# Python
python -m pip install a3s-code

# Go
go get github.com/A3S-Lab/Code/sdk/go/v7
```

The native SDK crates explicitly enable the Core `headless-search`, `s3`, and
`serve` features to preserve their complete product surface. Direct Rust
embedders receive the lazy Chrome/Chromium search tier by default and can omit
the browser dependency stack with `default-features = false`. The pure-Go
package uses the matching `a3s-code-go-bridge` release asset and requires no
CGO. Node.js, Python, and Go hosts can inject typed asynchronous embedding
providers for session-owned in-memory semantic and hybrid workspace retrieval;
provider cancellation follows query and session lifecycle, and no SDK requires
a vector database. Remote embedding admits only conservative source paths,
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
values remain outside the release document.

Release admission validates metadata. It does **not** build or run an OCI
artifact, implement health behavior, or own deployment lifecycle. Read the
[Agent Release Contract](manual/AGENT_RELEASE_CONTRACT.md) before integrating
the v1 schema.

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
| [Performance Qualification](manual/PERFORMANCE_QUALIFICATION.md)                                                     | Release-profile workloads, inclusion rules, p50/p95/max results, resource ceilings, hermetic integrations, run links, and artifact digests                    |
| [Harness Model-Call Evidence](manual/HARNESS_EVIDENCE.md)                                                            | Capability/input/usage snapshots, repeated-context diagnostics, event ordering, redaction boundary, validation, and replay                                    |
| [Go SDK](sdk/go/README.md)                                                                                           | Bridge installation, sessions, event streaming, direct tools, errors, and release compatibility                                                               |
| [Code Intelligence Design](manual/CODE_INTELLIGENCE_DESIGN.md)                                                       | Language runtime, capability boundary, lifecycle, and verification                                                                                            |
| [Workspace Retrieval Baseline](manual/WORKSPACE_RETRIEVAL_BASELINE.md)                                               | Architecture, quality budgets, lifecycle, and adversarial trust boundaries                                                                                    |
| [Workspace Retrieval Qualification](manual/WORKSPACE_RETRIEVAL_QA.md)                                                | Release tests, independent oracles, performance evidence, and DeepSeek E2E scope                                                                              |
| [Workspace Retrieval DeepSeek Evaluation](manual/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md)                               | Paired task/rerank ablations, built-in chunk matrix, cross-SDK real-model parity, custom negative control, non-text boundary, metrics, and batching follow-up |
| [Workspace Retrieval Chunking](manual/WORKSPACE_RETRIEVAL_CHUNKING.md)                                               | Built-in/custom strategies, validation, async lifecycle, non-text boundary, and rerank plan                                                                   |
| [Workspace Retrieval Operations](manual/WORKSPACE_RETRIEVAL_OPERATIONS.md)                                           | Production SLOs, telemetry, state response, generation gates, and configuration-only rollback                                                                 |
| [Agent Directory Tools](manual/AGENT_DIR_TOOLS_DESIGN.md)                                                            | Filesystem-first tool and agent definitions                                                                                                                   |
| [Agent Release Contract](manual/AGENT_RELEASE_CONTRACT.md)                                                           | Admission schema, identity, compatibility, and security boundary                                                                                              |
| [Changelog](CHANGELOG.md)                                                                                            | Release history and migration-relevant changes                                                                                                                |

## Development

Run checks from the A3S Code repository directory:

```bash
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

The capability checker keeps all 20 advertised product areas connected to the
evidence ledger. Dedicated CI jobs build and load the Node.js and Python native
modules before running their host-language contracts; a successful Rust
`cargo check` alone is not counted as SDK runtime evidence.

The retrieval benchmark emits JSON and fails when the locked 25,000 x 384
exact-vector or hybrid p95 budgets are exceeded. See the
[qualification report](manual/WORKSPACE_RETRIEVAL_QA.md) for the reference
profile, inclusion rules, and measured results.

The targeted [Performance Qualification](.github/workflows/performance.yml)
workflow runs release-mode convergence, retrieval, Flow/State Graph, Code
Intelligence, context/memory, and persistence profiles when their critical
paths change and on a weekly schedule. It retains machine-readable JSON
artifacts. The [qualification record](manual/PERFORMANCE_QUALIFICATION.md)
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
