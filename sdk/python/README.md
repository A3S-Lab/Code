# A3S Code — Python SDK

Native Python bindings for the A3S Code AI coding agent, built with PyO3.

## Installation

```bash
pip install a3s-code
```

From v3.2.1 onwards the PyPI `a3s-code` package is a small pure-Python
bootstrap. On first `import a3s_code` it downloads the matching native
wheel from [GitHub Releases](https://github.com/A3S-Lab/Code/releases),
verifies the wheel's sha256 against the release manifest, and caches
the compiled extension under `~/.cache/a3s-code/<version>/`. Subsequent
imports use the cache.

Override the cache location with `A3S_CODE_CACHE_DIR`, the source URL
with `A3S_CODE_RELEASES_BASE_URL`, or skip the sha256 verification with
`A3S_CODE_SKIP_HASH_CHECK=1` (not recommended outside CI). See the
bootstrap README at `sdk/python-bootstrap/` for the full list.

Air-gapped or hermetic install? Grab the wheel matching your
interpreter directly:

```bash
pip install \
  'https://github.com/A3S-Lab/Code/releases/download/v<VERSION>/a3s_code-<VERSION>-cp312-cp312-manylinux_2_28_x86_64.whl'
```

Replace `<VERSION>` with the release to install, for example `6.4.0`.

## Quick Start

```python
from a3s_code import Agent

agent = Agent.create("agent.acl")
session = agent.session("/my-project")

result = session.send({"prompt": "What files handle authentication?"})
print(result.text)
```

## Async Lifecycle APIs

Asyncio applications should use the awaitable lifecycle methods so session
stores, workspace setup, cancellation, and shutdown do not block the current
event-loop task:

```python
agent = await Agent.create_async("agent.acl")
session = await agent.session_async("/my-project", options)
resumed = await agent.resume_session_async(session_id, resume_options)
replacement = await agent.replace_session_async(session, replacement_options)

result = await session.send_async("Inspect the authentication flow")
tool_result = await session.tool_async(
    "read", {"file_path": "src/auth.py", "offset": 0, "limit": 200}
)
governed_result = await session.governed_tool_async(
    "write", {"file_path": "notes.txt", "content": "reviewed content"}
)
runs = await session.runs_async()
events = await session.run_events_async(run_id)
page = await session.run_event_page_async(run_id, after_sequence=cursor, limit=256)
admitted = await session.spawn_run_with_id_async(run_id, "Verify the release")
recovered = await session.spawn_recovery_with_run_id_async(
    checkpoint_run_id, recovery_run_id
)

await session.save_async()
await session.cancel_async()
await session.close_async()
await agent.close_async()
```

`replace_session_async()` atomically reconfigures an idle persisted session. A
failed replacement leaves the current object live; a successful replacement
returns the same session ID and closes the previous object.

`session_async()` intentionally accepts a typed `SessionOptions` object instead
of the legacy collection of primitive keyword overrides. The synchronous
methods remain available for compatibility.

`tool_async(name, args)` is the generic trusted-host direct-tool API. Like
`tool()`, it treats the embedding application as the permission/HITL authority
while retaining hooks, budgets, output sanitization, timeouts, and session
cancellation. Use `governed_tool_async(name, args)` when the host coordinates a
call that must still pass the session permission and confirmation gates.
Synchronous callers can use `governed_tool(name, args)`.

## Ephemeral Workspace Retrieval

Semantic retrieval is opt-in and belongs to one session. The host injects an
async embedding callback; A3S Code owns chunking, bounded in-memory vectors,
hybrid ranking, source-digest verification, and shutdown. Nothing is persisted
to a vector database.

```python
from a3s_code import (
    CallbackEmbeddingProvider,
    DeterministicWorkspaceReranker,
    RecursiveWorkspaceChunkingStrategy,
    SessionOptions,
    WorkspaceRetrievalOptions,
)

async def embed(request):
    response = await embedding_client.embed(
        model="text-embedding-model",
        inputs=[item["text"] for item in request["inputs"]],
    )
    return {
        "vectors": [
            {"id": item["id"], "values": vector}
            for item, vector in zip(request["inputs"], response.vectors)
        ]
    }

provider = CallbackEmbeddingProvider(
    "host-provider",
    "text-embedding-model",
    1536,
    embed,
    normalization="unit",
)
reranker = DeterministicWorkspaceReranker()
reranker.max_candidates = 100
chunking = RecursiveWorkspaceChunkingStrategy(
    8 * 1024,
    512,
    ["\n\n", "\n", ". ", " "],
)
retrieval = WorkspaceRetrievalOptions(
    provider,
    reranker,
    chunking_strategy=chunking,
)
retrieval.max_records = 100_000
retrieval.max_bytes = 128 * 1024 * 1024

options = SessionOptions()
options.workspace_retrieval = retrieval
session = await agent.session_async("/my-project", options)

status = session.workspace_retrieval_status()
semantic = await session.semantic_search_async({"query": "session cleanup"})
hybrid = await session.hybrid_search_async({"query": "terminate_owned_tasks"})
await session.close_async()
```

Create, resume, and replace retrieval-enabled sessions through their async
APIs so the provider can bind to the current event loop. Cancelling or closing
the session cancels the active embedding coroutine. The exported
`EmbeddingBatchRequest`, `WorkspaceRetrievalStatus`, and semantic/hybrid result
`TypedDict` declarations provide the static callback and DTO contract.

The reranker argument is optional; omit it to preserve RRF-only. Its typed
fields bound candidates, sampled feature bytes, fingerprints, and checked
scratch memory. Invalid bounds fail during session construction before the
embedding coroutine runs, and raw mode or algorithm strings are not accepted.

`chunking_strategy` is also optional. Pass a
`LineWorkspaceChunkingStrategy`, `FixedWindowWorkspaceChunkingStrategy`, or
`RecursiveWorkspaceChunkingStrategy`; omission preserves compatible line
chunking. Targets, overlap, and recursive separator lists are immutable and
validated by Core before indexing or provider execution. Primitive strategy
names are rejected, and arbitrary custom range callbacks remain available only
to trusted Rust hosts.

## Agent-Wide Priority Scheduling

Every session created from one `Agent` shares the same execution capacity.
Conversation runs, direct tools, detached children, and host workflows enter
one priority/FIFO scheduler.

```python
options = SessionOptions()
options.task_priority = "background"
background = agent.session("/my-project", options)

stats = agent.task_scheduler_stats()
same_scheduler = background.task_scheduler_stats()
print(stats["active"], stats["pendingByPriority"]["background"])
```

Priorities are `urgent`, `interactive` (the default), `foreground`,
`background`, and `maintenance`. Equal priorities remain FIFO; waiting
non-urgent work ages toward interactive priority. Configure global capacity
and the aging interval in ACL:

```acl
task_scheduler {
  max_active = 4
  aging_interval_ms = 30000
}
```

The returned dictionaries report `maxActive`, active and pending totals,
per-priority counts, and shutdown state. They are point-in-time diagnostic
snapshots, not capacity reservations.

## Deterministic Tool-result projection

Pin the context-efficient profile when long Tool output should retain both its
beginning and end, fold exact repeated lines, and sample oversized JSON arrays:

```python
from a3s_code import SessionOptions, ToolResultTransformPolicy

options = SessionOptions()
options.tool_result_transform_policy = ToolResultTransformPolicy.context_efficient()
projected = agent.session("/my-project", options)
```

The exact policy persists in the session snapshot, and resume rejects policy
drift. Read `ToolResult.metadata["a3s_tool_result_evidence"]` for the
original/projected sizes and token estimates, SHA-256 digests, loss mode,
repeat key, transform algorithm, and immutable inline or artifact reference.

## Session Operation Concurrency

A session admits one transcript-affecting operation at a time. `send`, `stream`,
attachment requests, slash commands, and run resumption share a fail-fast gate.
An overlap raises a busy-session error (`CodeError::SessionBusy` in Rust)
instead of waiting in a hidden queue. A stream retains admission until its
producer stops, even if the public iterator is dropped. Finish or cancel the
active operation before starting another one.

Fully consuming `EventStream` is a lifecycle barrier: iteration does not finish
ahead of core cleanup, so an immediate next conversation operation is not
rejected because the prior stream still owns admission.

## Streaming Event Protocol

Every streamed `AgentEvent` carries the stable version-1 envelope fields
`version`, `type`, `payload`, and optional `metadata`. `event_type` remains a
compatibility alias for `type`. The payload is complete and is preserved for
unknown future event names; convenience fields such as `text`, `tool_name`,
and `exit_code` are derived from the same core projection.

```python
from a3s_code import EventType

active_turn = None
attempt_text = []
for event in session.stream("Explain the current test failures"):
    if event.type == EventType.TURN_START:
        # A repeated turn number replaces an interrupted attempt.
        active_turn = event.turn
        attempt_text.clear()
    elif event.type == EventType.TEXT_DELTA:
        attempt_text.append(event.text or "")
    elif event.type == EventType.TURN_END and event.turn == active_turn:
        print("".join(attempt_text), end="", flush=True)
        attempt_text.clear()
    elif event.type == EventType.AGENT_END:
        print(event.verification_summary_text or "")
    print(event.version, event.type, event.payload, event.metadata)
```

`turn_start` may repeat with the same `turn` when an established provider
stream is interrupted. Treat each turn as provisional until `turn_end`; reset
text, reasoning, and tool-call drafts when that turn restarts.

`agent_event_types_v1()` returns the ordered catalog known by the native
runtime. `AgentEventTypeV1` and `EventType` are generated from the core catalog;
callers should still retain a default branch for future values.

## Slash Commands

Every session includes built-in slash commands dispatched before the LLM:

```python
# List all available commands
commands = session.list_commands()
for cmd in commands:
    print(f"/{cmd['name']:15s} {cmd['description']}")

# Built-in commands
result = session.send("/help")       # List all commands
result = session.send("/model")      # Show current model
result = session.send("/cost")       # Token usage and cost
result = session.send("/history")    # Conversation stats
```

### Custom Commands

```python
def my_handler(args: str, ctx: dict) -> str:
    return f"Model: {ctx['model']}, History: {ctx['history_len']} msgs, args: {args!r}"

session.register_command("status", "Show session info", my_handler)
result = session.send("/status hello")
```

## Full API

```python
from a3s_code import (
    Agent,
    ArtifactStoreLimits,
    ConfirmationPolicy,
    PermissionPolicy,
    SessionOptions,
    WorkerAgentSpec,
    DefaultSecurityProvider,
    FileMemoryStore,
    FileSessionStore,
    HostEnvConfig,
    LocalWorkspaceBackend,
    S3WorkspaceBackend,
)

agent = Agent.create("agent.acl")
session = agent.session("/my-project",
    model="openai/gpt-4o",
    planning_mode="auto",  # "enabled" forces planning, "disabled" turns it off
)

# Send / Stream
result = session.send({"prompt": "Explain the auth module"})
active_turn = None
attempt_text = []
for event in session.stream({"prompt": "Refactor auth"}):
    if event.event_type == "turn_start":
        active_turn = event.turn
        attempt_text.clear()
    elif event.event_type == "text_delta":
        attempt_text.append(event.text or "")
    elif event.event_type == "turn_end" and event.turn == active_turn:
        print("".join(attempt_text), end="", flush=True)
        attempt_text.clear()

# Streams with no custom history update session history and verification evidence
# when the stream completes. Passing explicit history keeps the stream isolated.
# send(...) and stream(...) accept prompt strings or object-shaped requests
# with optional history and attachments.

# Planning events
# Prefer planning_mode="auto" | "enabled" | "disabled". The legacy planning
# bool still works: True forces planning, False disables it. In streaming mode,
# render task_updated as the current task list; step_start and step_end are
# per-step progress events.

# Run replay
runs = session.runs()
if runs:
    print(runs[-1]["id"], runs[-1]["status"])
    for event in session.run_events(runs[-1]["id"]):
        print(event["version"], event["type"], event["payload"], event["metadata"]["sequence"])
    print(session.active_tools())
    # Cancels only if that run is still active; stale IDs are ignored.
    session.cancel_run(runs[-1]["id"])

# Headless hosts can start detached work under immutable run IDs. Repeating a
# compatible ID returns replayed=True instead of starting duplicate work.
admitted = session.spawn_run_with_id("release-42/run-7", "Verify the release")
print(admitted["snapshot"]["id"], admitted["replayed"])

# run_events() uses the same versioned envelope as live streams. Replay
# metadata carries run_id, session_id, sequence, and timestamp_ms.

# Incremental replay uses an exclusive cursor. retention_gap means the
# requested cursor predates the retained FIFO window.
page = session.run_event_page(runs[-1]["id"], limit=256) if runs else None
if page and page["retention_gap"]:
    raise RuntimeError("Requested run events were evicted")

# RuntimeError instances originating in the core expose a stable `.code`, such
# as SESSION_BUSY, SESSION_CLOSED, or BUDGET_EXHAUSTED. Do not parse messages.

# Direct tools (bypass LLM)
opts = SessionOptions()
opts.workspace_backend = LocalWorkspaceBackend("/my-project")
opts.artifact_store_limits = ArtifactStoreLimits(max_artifacts=64, max_bytes=8 * 1024 * 1024)
opts.tool_timeout_ms = 120_000
opts.llm_api_timeout_ms = 120_000
opts.circuit_breaker_threshold = 4
opts.duplicate_tool_call_threshold = 5
opts.manual_delegation_enabled = True
opts.auto_compact = True
opts.auto_compact_threshold = 0.8
opts.max_context_tokens = 128_000
opts.host_env = HostEnvConfig(
    sequential_id_prefix="replay",
    fixed_time_ms=1_700_000_000_000,
)
session = agent.session("/my-project", opts)
session.write_file("notes.txt", "one\ntwo\n")
session.read_file("src/main.py")
session.read_file("src/main.py", offset=2000, limit=2000)
session.ls()
session.edit_file("notes.txt", "one", "uno")
session.patch_file("notes.txt", "@@ -1,2 +1,2 @@\n uno\n-two\n+dos")
session.bash("pytest")
session.glob("**/*.py")
session.grep("TODO")
session.tool_names()
session.tool_definitions()
artifact = session.get_artifact("a3s://tool-output/read/abc123")

# Set max_context_tokens when the active model is not declared in the agent
# configuration. Rolling auto-compaction can then repeat before later requests
# overflow that model window. The bundled Core retains recent history by token
# budget and rejects a replacement that would not reduce estimated usage.

# Direct helpers are trusted host-control-plane operations. They skip
# model-facing permission/HITL, while hooks, budget, queue/timeout,
# cancellation, recursion protection, and output sanitization remain active.
# Authorize end users before exposing them. They do not claim the
# transcript-operation gate.
#
# Use governed_tool() when the host has not already authorized the operation.
governed = session.governed_tool(
    "write", {"file_path": "reviewed.txt", "content": "reviewed content"}
)

# Dynamic workflow is opt-in for SDK sessions.
session.register_dynamic_workflow_runtime()
session.tool("dynamic_workflow", {
    "source": """
        export default async function run(ctx, inputs) {
          if (inputs.kind === 'workflow') {
            return { type: 'complete', output: { text: inputs.input.message } };
          }
          return { type: 'fail', error: 'unexpected step invocation' };
        }
    """,
    "input": {"message": "hello from Flow"},
})
session.unregister_dynamic_tool("dynamic_workflow")

# Folder-style skills
workspace = "/my-project"
skill_dir = f"{workspace}/skills"
session = agent.session(workspace, skill_dirs=[skill_dir])
matches = session.tool("search_skills", {"query": "review database schema", "limit": 5})
print(matches.output)

skill_result = session.tool("Skill", {
    "skill_name": "db-review",
    "prompt": "Review the migrations and summarize correctness risks.",
})
print(skill_result.output)

# Or configure skill directories through SessionOptions.
opts = SessionOptions()
opts.skill_dirs = [skill_dir]
session = agent.session(workspace, opts)

# S3-compatible workspace — point the same direct tools at object storage.
# `bash`, `git`, `grep`, `glob` are automatically hidden because object
# storage cannot service them. Works with AWS S3, MinIO, RustFS, R2, B2.
s3_opts = SessionOptions()
s3_opts.workspace_backend = S3WorkspaceBackend(
    bucket="workspace",
    prefix="users/u1/sessions/s1",
    access_key_id="AKIA...",
    secret_access_key="...",
    endpoint="https://minio.local:9000",         # omit for AWS S3
    region="us-east-1",
    force_path_style=True,                       # True for MinIO/RustFS/R2
)
s3_session = agent.session("s3://workspace/users/u1/sessions/s1", s3_opts)
s3_session.write_file("notes/hello.txt", "one\ntwo\n")
s3_session.read_file("notes/hello.txt")
s3_session.read_file("notes/hello.txt", offset=1, limit=1)
s3_session.ls("notes")

# Programmatic Tool Calling (embedded QuickJS)
program = session.program({
    "source": """
        export default async function run(ctx, inputs) {
          const hits = await ctx.grep(inputs.query, { glob: '*.py' });
          const files = await ctx.glob('src/**/*.py');
          return { hits, files: files.slice(0, 10) };
        }
    """,
    "inputs": {"query": "PermissionPolicy"},
    "allowed_tools": ["grep", "glob"],
    "limits": {"timeoutMs": 30000, "maxToolCalls": 20, "maxOutputBytes": 65536},
})
print(program.output)

# Delegation helpers (both use the unified task tool)
session.task({
    "agent": "explore",
    "description": "Find auth entry points",
    "prompt": "Inspect the repository and summarize the auth-related files.",
})
session.tasks([
    {"agent": "explore", "description": "Find tests", "prompt": "Locate auth tests."},
    {"agent": "verification", "description": "Check risk", "prompt": "Review auth edge cases."},
])

# Automatic subagent delegation controls
opts = SessionOptions()
opts.auto_delegation = AutoDelegationConfig(enabled=True, max_tasks=4)
opts.max_parallel_tasks = 8
opts.auto_parallel = False  # disables automatic fan-out; manual session.tasks(...) still works
session = agent.session("/my-project", opts)

# Disposable worker agents (cattle mode)
opts = SessionOptions()
frontend = WorkerAgentSpec.implementer("frontend-cow", "Small verified frontend fixes")
frontend.model = "openai/gpt-4o"
frontend.max_steps = 24
frontend.prompt = "Keep patches focused and run the narrowest relevant check."
frontend.confirmation_inheritance = "auto_approve"  # child runs auto-approve Ask decisions
opts.add_worker_agent(frontend)
session = agent.session("/my-project", opts)
session.task({
    "agent": "frontend-cow",
    "description": "Fix admin chat loading state",
    "prompt": "Find and fix the loading-state regression, then summarize verification.",
})

# Confirmation inheritance controls how child runs resolve Ask decisions:
# - "auto_approve" (default): Child runs auto-approve all Ask decisions
# - "deny_on_ask": Child runs fail immediately when encountering an Ask
# - "inherit_parent": Child runs inherit the parent's confirmation policy
restricted = WorkerAgentSpec("restricted-writer", "Write files with parent confirmation", "implementer")
restricted.confirmation_inheritance = "inherit_parent"  # requires parent approval
opts.add_worker_agent(restricted)

# Object-shaped direct tools
session.git({"command": "status"})
session.git({"command": "worktree", "subcommand": "list"})
# git_command(...) and positional git(...) remain for compatibility.

# Live registration and top-level worker sessions are also supported.
session.register_worker_agent(WorkerAgentSpec.verifier("verify-cow", "Run focused checks"))
worker_session = agent.session_for_worker(
    "/my-project",
    WorkerAgentSpec.reviewer("review-cow", "Adversarial code review"),
)

# Slash commands
session.list_commands()
session.register_command("ping", "Pong!", lambda args, ctx: "pong")

# Memory
session.remember_success("task", ["tool"], "result")
session.recall_similar("auth", 5)

# Hooks
session.register_hook("audit", "pre_tool_use", handler_fn)

# MCP
session.add_mcp({
    "name": "github",
    "transport": {
        "type": "stdio",
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-github"],
    },
    "timeout_ms": 30_000,
})
session.mcps()
session.tool_names()
session.remove_mcp("github")
# Live add/remove mutates only this session's private manager. Global and
# host-supplied managers are inherited read-only capability sources.

# Evidence
session.record_verification_reports([{
    "schema": "a3s.verification_report.v1",
    "subject": "sdk:tests",
    "status": "passed",
    "checks": [{
        "id": "check:sdk",
        "kind": "test",
        "description": "Run SDK tests",
        "status": "passed",
        "required": True,
    }],
}])
session.verification_reports()
session.verification_summary_text()

# Persistence
opts = SessionOptions()
opts.session_store = FileSessionStore('./sessions')
opts.session_id = 'my-session'
opts.auto_save = True
session2 = agent.session(".", opts)
resumed = agent.resume_session('my-session', opts)
```

`save()` and `auto_save` publish one versioned `SessionSnapshotV1` generation:
conversation, artifacts, traces, run records, verification reports, and
subagent task snapshots are committed together. Built-in file and memory stores
publish the aggregate atomically; legacy fragmented records remain readable for
migration.

## HITL Confirmations

Use `PermissionPolicy` to decide which tools ask, then `ConfirmationPolicy` to
control confirmation runtime behavior such as timeout and YOLO lanes. Invalid
permission decisions, timeout actions, and lane names are rejected when the
session is created so unsafe fallbacks do not silently change policy.

```python
opts = SessionOptions()
opts.permission_policy = PermissionPolicy(ask=["bash*"], default_decision="allow")
opts.confirmation_policy = ConfirmationPolicy(
    enabled=True,
    default_timeout_ms=30_000,
    timeout_action="reject",
    yolo_lanes=["query"],
)
session = agent.session(".", opts)

for pending in session.pending_confirmations():
    session.confirm_tool_use(pending["tool_id"], approved=True, reason="Reviewed")
```

For the streaming event-driven loop used by UIs, see
`examples/hitl_confirmation_loop.py`.

## Delegation

Routine multi-agent work uses the model-visible `task` tool. Use
`session.task(...)` for one `tasks` item and `session.tasks(...)` for concurrent
fan-out, or `session.tool("task", {"tasks": [...]})` when you need raw access.
`session.parallel_task(...)` remains an explicit compatibility helper for the
hidden legacy alias.
The old standalone lifecycle control-plane API is intentionally removed from
the 2.0 SDK surface.

## Filesystem-First Agents

Define a durable agent as a **directory** — `instructions.md` (required) plus
optional `agent.acl`, `skills/`, `schedules/` (cron), and `tools/` (`kind: mcp` or
`kind: script` sandboxed QuickJS) — and serve its schedules. Each fire is a full
harness turn (context, tool visibility, safety gate, verification).
`serve_agent_dir` returns only after schedule validation and session/tool
preparation, so the handle is already ready. Startup failures raise
`RuntimeError` with a stable `code` attribute.

```python
opts = SessionOptions()
# Optional: pass a session_store so each schedule resumes its accumulated
# context across daemon restarts.
opts.session_store = FileSessionStore("./sessions")

handle = agent.serve_agent_dir("./my-agent", "./workspace", opts)
print(handle.is_ready(), handle.state())  # True, "ready"
# ... runs in the background until:
handle.stop()
print(handle.is_stopped(), handle.state())  # True, "stopped"
```

`stop()` cancels in-flight schedule work, closes daemon-owned sessions, and
waits for the bounded shutdown deadline. `failure_code()` exposes a stable code
when the daemon reaches `failed`.

## License

MIT
