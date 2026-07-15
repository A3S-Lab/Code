# A3S Code

<p align="center">
  <strong>Governed Agent Runtime for A3S</strong>
</p>

<p align="center">
  <em>Build auditable coding agents with bounded tools, resumable sessions, and deterministic coordination</em>
</p>

<p align="center">
  <a href="#overview">Overview</a> •
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#application-model">Application Model</a> •
  <a href="#tui-and-sdks">TUI and SDKs</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Code** is an async Rust runtime for building governed coding agents. It
owns the agent loop, workspace tools, model adapters, context assembly,
permissions, human approval, memory, delegation, persistence, verification,
and replay. Hosts can use the same runtime from Rust, Node.js, Python, or the
`a3s code` terminal application.

A3S Code is not a hosted agent service or a terminal widget library. The
`a3s-code-core` crate provides the execution contracts. The interactive TUI is
shipped by the separate [A3S CLI](https://github.com/A3S-Lab/CLI), and its
account integrations and presentation policy remain host concerns.

### Basic usage

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

An `Agent` owns resolved configuration and shared capabilities. An
`AgentSession` binds them to one workspace and conversation; a configured
`SessionStore` makes that state durable. Awaiting the stream lifecycle releases
the session for its next transcript operation.

## Features

- **Async Agent Sessions**: Create, resume, atomically replace, cancel, close,
  send, stream, attach images, dispatch slash commands, and replay runs
- **Governed Tool Runtime**: Validate arguments, enforce permissions and human
  approval, apply budgets and hooks, sanitize events, and propagate cancellation
- **Bounded Workspace Tools**: Read, search, edit, patch, execute, fetch, batch,
  and paginate without placing unbounded observations into model context
- **Native Code Intelligence**: Query saved-file symbols, definitions,
  declarations, references, implementations, and diagnostics through one
  workspace-scoped semantic runtime
- **Managed Context and Memory**: Assemble ranked context, compact repeatedly,
  preserve tool evidence, recall durable memory, and extract significant facts
- **Model and MCP Adapters**: Use OpenAI-compatible, Anthropic, Zhipu, custom
  `LlmClient`, and session-isolated MCP capabilities through one event model
- **Structured Generation**: Request schema-constrained objects through native
  provider formats or validated prompt-and-repair fallbacks
- **Agent Orchestration**: Plan, track goals, delegate foreground or background
  tasks, run bounded parallel work, and resume deterministic workflows
- **Programmable Workflows**: Execute sandboxed QuickJS tool programs and
  explicitly registered A3S Flow-backed dynamic workflows
- **Durable Evidence**: Persist atomic snapshots, versioned events, traces,
  artifacts, verification reports, checkpoints, and optional RL trajectories
- **Event-Sourced State Graph**: Coordinate typed objects and relations through
  hash-linked records, optimistic patches, strict replay, forks, and diffs

### Feature matrix

The core crate has no default Cargo features. Baseline capabilities are
compiled without optional cloud or daemon dependencies; session policy remains
explicit even when the supporting types are available.

| Area | Availability | Included capability |
| --- | --- | --- |
| Agent runtime | Baseline | Async `Agent`, `SessionBuilder`, and workspace-bound `AgentSession` |
| Conversation | Baseline | Send, stream, history, image attachments, slash commands, continuation, and cancellation |
| Providers | Baseline | OpenAI-compatible, Anthropic, Zhipu, and custom `LlmClient` implementations |
| Structured output | Baseline | Native or prompted JSON generation, JSON Schema validation, partial parsing, and repair |
| Workspace tools | Baseline | Capability-gated file, search, shell, Git, web, batch, program, skill, and delegation tools |
| Code Intelligence | Host-selected local workspace | Saved-file symbols, semantic navigation, diagnostics, typed status, and bounded result evidence |
| Context | Baseline and host-selected | Ranked filesystem, recent-file, ripgrep, memory, prompt-slot, skill, project, and custom sources |
| Memory | Baseline and configurable | Three-tier memory, a default workspace file store, typed overrides, recall, extraction, relations, and pruning |
| Persistence | Configured store | Atomic `SessionSnapshotV1`, file or memory stores, run replay, traces, tasks, and checkpoints |
| Planning | Baseline session policy | Automatic by default, forced or disabled modes, pre/post hooks, task events, and optional goals |
| Delegation | Manual by default, automatic opt-in | Agent definitions, worker specs, `task`, `parallel_task`, tracking, cancellation, and source anchors |
| Session queue | Config or session opt-in | Priority lanes, bounded concurrency, internal/external/hybrid handlers, retry, DLQ, metrics, alerts, rate limits, and timeouts |
| Dynamic workflow | Explicit registration | QuickJS-authored commands executed by an A3S Flow-backed runtime |
| State graph | Explicit application use | Event-sourced objects, relations, behaviors, patches, replay, forks, diffs, and Flow projection |
| MCP | Config or session registration | stdio, SSE, streamable HTTP, OAuth, refresh, and live add/remove management |
| Skills | Filesystem, registry, or live session registration | Search/execute tools, model-visible catalog, live add/remove with shadow restoration, and child-run inheritance |
| Search runtime | Explicit host operation | Browser status, install, update, and repair APIs for managed search runtimes |
| S3 workspace | Cargo feature `s3` | S3-compatible object storage backend with capability-aware tool visibility |
| Agent daemon | Cargo feature `serve` | Filesystem-first agent serving and cron schedules |
| OpenTelemetry | Cargo feature `telemetry` | OTLP trace export; normal `tracing` remains available without it |
| Native SDKs | Separate packages | N-API Node.js and PyO3 Python bindings over the Core runtime |
| Terminal product | Separate A3S CLI | Streaming TUI, editors, account models, effort profiles, research, assets, and optional A3S OS views |

Feature availability does not bypass policy. Auto-save, automatic compaction,
goals, automatic delegation, security providers, human approval, trajectory
recording, and graph integration run only when a host configures them. The
`dynamic_workflow` tool is not registered in a plain Core session.

## Quick Start

### Install the terminal application

```bash
brew install A3S-Lab/tap/a3s

# or install from crates.io
cargo install a3s

a3s code
```

Run the TUI from the workspace the agent should inspect. Resume a saved session
with `a3s code resume` or `a3s code resume <session-id>`.

### Install the Rust runtime

```bash
cargo add a3s-code-core
cargo add tokio --features macros,rt-multi-thread
```

Use ACL configuration and keep credentials in environment variables:

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
```

`Agent::new` accepts either an ACL file path or inline ACL. Do not commit real
API keys, access tokens, private endpoints, or tenant identifiers.

### Configure a session

```rust,no_run
use a3s_code_core::{Agent, PlanningMode, SessionOptions};

# async fn run() -> a3s_code_core::Result<()> {
let agent = Agent::new("agent.acl").await?;
let options = SessionOptions::new()
    .with_planning_mode(PlanningMode::Auto)
    .with_tool_timeout(120_000)
    .with_auto_compact(true)
    .with_max_context_tokens(200_000)
    .with_auto_compact_threshold(0.8);

let session = agent
    .session_builder("/path/to/workspace")
    .options(options)
    .build()
    .await?;

let result = session.send("Explain the test strategy.", None).await?;
println!("{}", result.text);
# Ok(())
# }
```

`SessionBuilder::build` is the primary construction path. It resolves merged
configuration and initializes filesystem stores, queues, trajectories, and MCP
sources asynchronously. Synchronous constructors remain compatibility APIs for
already initialized resources and never start or block a Tokio runtime.

## Application Model

### Sessions and lifecycle

A session owns conversation history, workspace services, tool registrations,
context providers, policies, stores, and active child work. The runtime can:

- create sessions with generated or host-supplied identities;
- resume complete snapshots from a `SessionStore`;
- atomically replace an idle persisted session with new runtime options;
- list and close live sessions owned by an `Agent`;
- cancel and settle an active operation before safely reusing its session;
- add, replace, list, and remove lifecycle-owned Skills while restoring an
  exactly shadowed registration on removal;
- add and remove session-local MCP servers without disturbing inherited or
  later host-owned tools;
- save, cancel, or close without abandoning active child work; and
- inspect runs, paginated run events, active tools, tasks, and verification.

Only one transcript-affecting operation may run at a time. Send, stream,
attachment requests, slash commands, and run resumption fail fast with
`CodeError::SessionBusy` when another such operation is active. Direct trusted
host tool calls use a separate control-plane path.

### Event protocol

Core streams `AgentEvent` values. SDK live streams and persisted-run replay
project them through `EventEnvelopeV1`: a stable version, event type, complete
payload, and optional metadata. The protocol covers text and reasoning, tools,
planning, tasks, memory, compaction, budgets, verification, and terminal state.
SDKs preserve future event names and payloads for older clients.

### Configuration

`CodeConfig` is the typed source of truth for providers, models, stores, memory,
queues, search, agents, skills, MCP, and execution limits. Hosts that provide a
config editor can call `rewrite_acl_sections` to replace selected top-level ACL
sections while preserving unrelated blocks, unknown fields, external comments,
and raw expressions such as `env("TOKEN")`.

## Tools and Workspaces

### Built-in tools

Tools are registered only when the selected workspace exposes the capability
they require. A model never receives a local `bash` or `git` definition from an
object-only backend that cannot execute it.

| Concern | Tools | Behavior |
| --- | --- | --- |
| Files and directories | `read`, `write`, `edit`, `patch`, `ls` | Ranged reads, resumable writes, compare-and-swap edits, strict unified patches, and bounded listings |
| Workspace search | `glob`, `grep` | Bounded matching with explicit result metadata and continuation |
| Code Intelligence | `code_symbols`, `code_navigation`, `code_diagnostics` | Saved-file semantic metadata and locations with bounded results; source retrieval and mutation stay in the existing file tools |
| Commands and source control | `bash`, `git` | Bounded output, cancellation, process-group termination on Unix, and typed Git operations |
| Web evidence | `web_search`, `web_fetch` | Ranked multi-engine search, normalized sources, SSRF protections, extraction, and bounded pages |
| Structured output | `generate_object` | Schema-constrained model generation with validation and repair |
| Composition | `batch`, `program` | Safe batch scheduling and sandboxed JavaScript programmatic tool calling |
| Delegation | `task`, `parallel_task` | Foreground/background workers, bounded parallelism, partial results, and task tracking |
| Skills | `Skill`, `search_skills` | Filesystem or inline skill discovery and execution with optional tool restrictions |
| MCP | `mcp__<server>__<tool>` | Namespaced tools owned by their source manager |
| Dynamic workflows | `dynamic_workflow` | Explicitly registered A3S Flow-backed, replayable per-turn workflows |

The built-in skill registry starts empty; skills come from configured
directories, `AgentDir`, inline host input, or live registration. The
model-visible `program` tool executes JavaScript in QuickJS, not arbitrary
Python or a shell-script catalog.

Long-running hosts can call `AgentSession::add_skill`, `remove_skill`, and
`skill_names` without rebuilding the session. The model-visible Skill catalog
reads the same live registry, so a successful mutation is visible on the next
turn. Live registrations use pointer identity as their ownership token: an
upgrade preserves the original shadow chain, and removal never deletes a Skill
installed later by another host.

### Code Intelligence

Local hosts can attach native Code Intelligence with
`WorkspaceServices::local_with_code_intelligence` or the shared-backend
variant. The provider reuses the existing workspace manifest, filesystem,
path resolver, cancellation, and tool policy. It does not create a competing
file index, search implementation, editing path, memory store, or MCP server.

The initial profiles require `rust-analyzer` for Rust and
`typescript-language-server --stdio` for TypeScript and JavaScript. Install the
executables before launching the host:

```sh
rustup component add rust-analyzer
npm install --global typescript typescript-language-server
```

Queries use saved files only. Public lines and characters are zero-based, and
characters count UTF-16 code units. Document results include the saved-content
revision and hash plus a stale flag when the file changes while a query is in
flight. The first navigation request for a saved revision includes a bounded,
cancellable stabilization pass so a protocol-ready but still-indexing server
does not leak a cold partial result. The semantic tools return metadata and
workspace-relative locations; agents continue to use `read`, `grep`, `edit`,
and `patch` for source text and mutations.

See the [Code Intelligence design](manual/CODE_INTELLIGENCE_DESIGN.md) for the
capability boundary, shared runtime architecture, lifecycle, and verification
plan.

### Tool governance

Every invocation carries declared `ToolCapabilities`, including read-only,
idempotent, resumable, cancellation-safe, paginated, output kind, and parallel
limits. Model, nested-program, and session calls validate JSON arguments before
permission prompts or side effects.

`batch` runs children concurrently only when every call declares safe
read-only parallel behavior; mutations and unknown tools are serialized.
Segmented writes require the expected UTF-8 byte offset, so retries are
idempotent and stale appends fail. Patch hunks validate positions, counts,
context, removals, whitespace, and line endings before one compare-and-swap
write; fuzzy application is intentionally rejected.

An optional session queue routes agent tool work through the Query and Execute
lanes with bounded concurrency, handler modes, retry, DLQ, metrics, alerts,
rate limits, and per-lane timeouts. Control and Generate remain reserved lane
types; conversation runs use a separate single-flight admission guard, so lane
priority orders pending work but does not interrupt an active model future.

Observations are bounded at their source. Reads, listings, globs, Git results,
and fetched pages expose offsets or cursors. Large tool output and file changes
move into bounded artifacts with previews, exact sizes, and hashes. Tool and
model deadlines propagate cancellation; on Unix, shell cancellation terminates
the process group rather than only its parent.

Optional permission policies resolve deny, allow, ask, and default behavior.
Interactive Code hosts can additionally share `InteractiveToolGuardrail`, a
conservative argument-aware classifier that quietly permits a narrow read-only
subset, asks for ordinary side effects, denies workspace escapes and catastrophic
shell operations, and keeps a non-bypassable safety floor in streamlined modes.
Confirmation managers, hooks, budget guards, security providers, stream
sanitization, retention limits, circuit breakers, duplicate-call guards, and
no-progress detection compose around the same invocation path. Trusted direct
host calls skip model-facing permission prompts, so an embedding application
must authorize its callers.

### Workspace backends

The default `LocalWorkspaceBackend` provides filesystem, search, command, and
Git services beneath a workspace root. `ManifestWorkspaceBackend` adds live
file status and recent-file tracking. `RemoteGitBackend` exposes remote Git
operations, and the `s3` Cargo feature provides an S3-compatible object backend
for file tools.

Applications can implement typed `WorkspaceFileSystem`, `WorkspaceSearch`,
`WorkspaceCommandRunner`, `WorkspaceGit`, path resolution, stash, and worktree
contracts. `WorkspaceServicesBuilder` combines only the services a backend can
actually support.

## Context, Memory, and Providers

### Context and compaction

`ContextAssembler` ranks and budgets items from static, filesystem, recent-file,
ripgrep, memory, and host-defined `ContextProvider` implementations. Project
instructions and skill catalogs join them during session prompt assembly.
`SystemPromptSlots` keeps role, rules, workspace instructions, and host guidance
separate. Filesystem and recent-file providers are installed only when selected.

Automatic compaction is opt-in. It monitors estimated or provider-reported
usage against the configured model window, bounds oversized tool results,
summarizes older history, and targets a safe post-compaction watermark after
subtracting the fixed system-prompt and tool-schema cost. Recent raw messages
are retained only within that token budget, while the latest user instruction
and unresolved tool calls remain intact. The policy re-arms after success so a
long session can compact repeatedly. The summary is treated as untrusted
transcript data, not as a new source of executable instructions.

### Memory

The memory layer separates working, short-term, and durable long-term state.
It supports relevance and recency scoring, typed memories, relations,
successful and failed pattern capture, file or custom stores, optional pruning,
and context injection. Significant completed turns can use the active model to
extract a bounded set of durable memories; hosts can disable or tune extraction.

### Models and MCP

Core includes clients for Anthropic, Zhipu, and OpenAI-compatible APIs, plus the
`LlmClient` trait for host adapters. The common model contract normalizes text,
reasoning, tool calls, usage, images, retries, cancellation, and streaming where
the provider supports them. Structured generation uses native response formats
when available and a schema-validated prompt fallback otherwise.

MCP connections support stdio, HTTP SSE, streamable HTTP, and OAuth client
credentials. Global managers can seed new sessions and refresh cached tools;
session managers can connect, disconnect, and live-add or remove isolated
servers. Tools remain source qualified so separate servers do not silently
share ownership.

Hosts can add or replace a Skill in a running session with
`AgentSession::add_skill`, inspect the effective names with `skill_names`, and
remove host-owned entries with `remove_skill`. The search tools and
model-visible Skill catalog observe the live registry on the next query or
turn. Removing a live Skill restores the session definition it shadowed;
closing the session removes all host-owned entries. Delegated child runs share
that same effective registry, so a capability attached after session creation
does not disappear at the worker boundary.

The browser search runtime exposes read-only status separately from explicit
install, update, and repair operations. Inspecting runtime status never performs
an installation as a side effect.

## Orchestration

### Planning and goals

Planning has three states: automatic, forced, and disabled. `PrePlanning` can
block or modify planner input, while `PostPlanning` reports the outcome. Streams
expose planning, task-snapshot, step, and optional goal-progress events so a
host can render and replay the same lifecycle it supervises.

Goal tracking and automatic delegation are opt-in. Bounded continuation is
enabled by default and can be tuned or disabled. Manual `task` and
`parallel_task` tools remain independently configurable, allowing a host to
expose deliberate delegation without enabling heuristic fan-out.

### Agents and deterministic workflows

`AgentDefinition` loads reusable roles from Markdown or YAML. Explicit
`WorkerAgentSpec` values describe reproducible disposable workers. Delegated
tasks inherit bounded parent policy, can run foreground or background, can
request structured output, record source anchors, expose progress snapshots,
and accept targeted cancellation.

The orchestration module provides sequential pipelines, bounded parallel steps,
resumable parallel execution, loops, workflow events, token and step budgets,
and checkpoints. Completed steps can be restored from a checkpoint instead of
repeating successful model work after a restart.

`program` runs a bounded JavaScript module in QuickJS with a host-provided
`ctx` surface. Time, nested tool calls, output bytes, recursion, and allowed
tools are limited. Recursive `program`, `dynamic_workflow`, and
`parallel_task` calls are excluded from its normal allow-list.

`DynamicWorkflowRuntime` is a separate, explicit capability. A sandboxed
program returns commands such as `complete`, `fail`, `schedule_step`, or
`schedule_steps`; A3S Flow records workflow and step history, while the host
executes allowed steps. A plain Core session must call
`session.register_dynamic_workflow_runtime()` before invoking the
`dynamic_workflow` tool.

### Filesystem-first agents

`AgentDir` keeps an agent's instructions, ACL, skills, tool specifications, and
schedules in reviewable files:

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
Files do not bypass workspace, permission, confirmation, or verification policy.

## Persistence and Observability

`SessionSnapshotV1` commits the session, artifacts, trace events, run records,
verification reports, and subagent tasks as one generation when the selected
store advertises atomic snapshots. The built-in memory and file stores provide
that guarantee; custom stores declare their own capabilities. Loop and workflow
checkpoints use dedicated store APIs.

Runs expose status, active tools, ordered event replay, exclusive pagination
cursors, and retention-gap detection. File persistence uses temporary files and
atomic replacement. Artifact stores bound both item count and bytes, while
verification reports keep claims and evidence separate from final prose.

`tracing` works in the baseline build. Enable `telemetry` for OpenTelemetry OTLP
trace export. Training-oriented hosts can opt into JSONL RL trajectories
covering prompts, model turns, tools, observations, usage, and termination
reasons, and can request token log probabilities from compatible providers.

## State Graph

The state graph is a complementary coordination runtime, not hidden mutable
state inside every session. Applications opt into it when several agents,
behaviors, or workflow decisions need a shared auditable model.

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
optimistic GraphPatch values
        │ version checks
        ▼
new graph version or explicit rejection
```

`GraphRuntime` supports typed objects and relations, event filters, behaviors,
per-event limits, optimistic object and relation versions,
correlation and causation metadata, and custom external-event projection.
Records carry previous-record and resulting-state hashes, making semantic or
ordering changes detectable during strict replay.

A runtime can fork at an event point and produce a structural diff without
mutating its parent. `MemoryGraphEventStore` and `FileGraphEventStore` persist
branches; capable stores publish extending generations with compare-and-swap
head checks. The A3S Flow bridge projects workflow state into the graph and
uses memory or file decision ledgers to claim, renew, complete, and recover
durable decisions with at-least-once dispatch. Sinks must be idempotent by
decision ID.

Node.js and Python expose `StateGraphRuntime` operations for event emission,
patching, restore through strict replay, fork, and JSON diff exchange.

## TUI and SDKs

### Terminal application

The [A3S CLI](https://github.com/A3S-Lab/CLI) builds the `a3s code` product on
Core and the shared [A3S TUI](https://github.com/A3S-Lab/TUI) framework.

| Area | TUI capability |
| --- | --- |
| Coding workspace | Streaming text, reasoning, tool cards, approvals, task progress, image/file input, session resume/fork, and bounded Markdown/diff rendering |
| Workspace UI | Full-screen `/ide` file browser/editor with saved-file symbols, semantic navigation, and diagnostics, plus a `/config` ACL editor with shared workspace and permission boundaries |
| Models and effort | ACL providers, signed-in Claude Code, Codex, and WorkBuddy accounts, host-provided models, and effort profiles from `low` through `ultracode` |
| Long sessions | A single footer context meter, repeated model-aware compaction, past-session search, durable memory, daily consolidation, and local knowledge |
| Orchestration | Planning, goals, tracked child tasks, parallel work, Ultracode workflows, bounded DeepResearch, and persistent maker/checker loops |
| Local assets | Filesystem-first agents, MCP servers, skills, workflows, and knowledge packages with optional publishing and deployment |
| Optional host integration | A3S OS login, runtime tools, activity, progressive responses, and trusted RemoteUI view links when configured |

Account-backed Claude Code, Codex, and WorkBuddy clients are CLI adapters, not
Core provider defaults. Local chat, tools, skills, MCP, memory, delegation, and
dynamic workflows remain usable without an A3S OS account.

### Native SDKs

| Language | Package | Runtime surface |
| --- | --- | --- |
| Rust | [`a3s-code-core`](https://crates.io/crates/a3s-code-core) | Complete Core API and extension traits |
| Node.js | [`@a3s-lab/code`](sdk/node/README.md) | N-API bindings with async lifecycle, streaming, tools, orchestration, stores, MCP, and state graph |
| Python | [`a3s-code`](sdk/python/README.md) | PyO3/bootstrap package with sync and async APIs, streaming, tools, orchestration, stores, MCP, and state graph |

Both native SDK crates enable the Core `s3` and `serve` features. Typed option
objects carry memory, session, workspace, and security extensions, while the
alignment check documents intentional language-specific omissions.

## Architecture

```text
Rust host / Node SDK / Python SDK / a3s code TUI
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

The public extension boundaries are `LlmClient`, `ContextProvider`,
`MemoryStore`, `SessionStore`, workspace service traits, permissions,
confirmation, hooks, security, budget guards, tools, MCP transports, and graph
stores. Core owns lifecycle and ordering; hosts own product UI, identity,
credential access, and deployment policy.

Source is split by concern under `agent_api/`, `tools/`, `workspace/`,
`context/`, `llm/`, `mcp/`, `orchestration/`, `store/`, and `state_graph/`.
Node and Python bindings remain separate crates over the same Rust core.

## Development

Run checks from the A3S Code repository directory:

```bash
cargo fmt --all -- --check
cargo test -p a3s-code-core
cargo test -p a3s-code-core --all-features
cargo clippy -p a3s-code-core --all-targets --all-features -- -D warnings
node scripts/sdk_api_alignment_check.mjs
```

Real-provider, browser-runtime, and S3 tests are intentionally ignored unless
their credentials or external services are configured. Run only the focused
ignored test whose prerequisites are available; the normal suite is hermetic.

See the [Node SDK](sdk/node/README.md), [Python SDK](sdk/python/README.md), and
[Changelog](CHANGELOG.md) for surface-specific details and release history.

## License

MIT
