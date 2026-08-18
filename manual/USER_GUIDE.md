# A3S Code User and Developer Guide

This guide describes the current A3S Code 7.x contract. It is intentionally
shorter than the versioned website reference: examples here cover the stable
entry points, while the website documents every option and wire shape.

## 1. Choose a host surface

| Host    | Install                                    | Runtime boundary                                               |
| ------- | ------------------------------------------ | -------------------------------------------------------------- |
| Rust    | `cargo add a3s-code-core`                  | Native async Core API                                          |
| Node.js | `npm install @a3s-lab/code`                | N-API native module                                            |
| Python  | `pip install a3s-code`                     | PyO3 native module downloaded from the matching GitHub release |
| Go      | `go get github.com/A3S-Lab/Code/sdk/go/v7` | Pure-Go client plus the version-matched bridge process         |

Node.js and Python applications should prefer their async lifecycle methods.
Go applications must deploy a bridge asset from the same release as the Go
module. Rust session construction is async-first because stores, MCP discovery,
workspace services, and retrieval providers may require I/O.

## 2. Configure models with ACL

ACL is the supported product configuration format. `Agent.create` accepts
either a path to an `.acl` file or an inline ACL string.

```acl
default_model = "openai/my-model"

providers "openai" {
  api_key = env("OPENAI_API_KEY")
  base_url = "https://api.openai.com/v1"

  models "my-model" {
    name = "My Model"
    tool_call = true
    temperature = true
    limit = { context = 200000, output = 8192 }
  }
}

task_scheduler {
  max_active = 4
  aging_interval_ms = 30000
}
```

Keep credentials in environment-backed ACL values. Model identifiers use the
`provider/model` form. A chat model and an embedding model are independent
routes; configuring one does not silently configure the other.

## 3. Create, use, and close a session

### Node.js

```js
import { Agent } from "@a3s-lab/code";

const agent = await Agent.create("agent.acl");
const session = await agent.sessionAsync("/repo", {
  planningMode: "auto",
  goalTracking: true,
});

try {
  const result = await session.send("Find the authentication entry points.");
  console.log(result.text);
} finally {
  await session.closeAsync();
  await agent.closeAsync();
}
```

### Python

```python
from a3s_code import Agent, SessionOptions

agent = await Agent.create_async("agent.acl")
options = SessionOptions()
options.planning_mode = "auto"
options.goal_tracking = True
session = await agent.session_async("/repo", options)

try:
    result = await session.send_async("Find the authentication entry points.")
    print(result.text)
finally:
    await session.close_async()
    await agent.close_async()
```

### Rust

```rust
use a3s_code_core::{Agent, SessionOptions};

let agent = Agent::new("agent.acl").await?;
let session = agent
    .session_builder("/repo")
    .options(SessionOptions::new())
    .build()
    .await?;

let result = session.send("Find the authentication entry points.", None).await?;
println!("{}", result.text);
session.close().await;
```

Every session owns one conversation lifecycle. Transcript-changing operations
are fail-fast single-flight: overlapping `send`, `stream`, attachment, slash
command, or resume operations return a busy-session error instead of entering
an invisible queue. Fully consume or cancel a stream before starting the next
turn.

## 4. Understand the tool surface

The workspace backend determines which tools can be registered. Inspect
`toolNames()` and `toolDefinitions()` rather than assuming a local filesystem.

| Layer     | Model-visible tools                                                         | Notes                                                                                                      |
| --------- | --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| Workspace | `read`, `write`, `edit`, `patch`, `download`, `search`, `ls`, `bash`, `git` | Each tool is capability-gated; `download` additionally requires a writable local workspace.                |
| Runtime   | `web_fetch`, `web_search`, `batch`, `program`                               | Nested calls retain the current invocation scope, cancellation, and budgets.                               |
| Session   | `task`, `generate_object`, `search_skills`, `Skill`                         | Delegation can be disabled. The legacy `parallel_task` alias is callable but hidden from the model schema. |
| Dynamic   | MCP and host-registered tools                                               | MCP tools use the `mcp__<server>__<tool>` namespace.                                                       |

`search` is the single model-facing repository search contract. Always pass a
mode and query.

```json
{
  "mode": "bm25",
  "query": "workspace permission policy",
  "path": "core/src",
  "include": "*.rs",
  "limit": 8
}
```

| Mode       | Purpose                                                     | Embeddings required                            |
| ---------- | ----------------------------------------------------------- | ---------------------------------------------- |
| `grep`     | Regular-expression content search                           | No                                             |
| `glob`     | Path discovery                                              | No                                             |
| `bm25`     | Native lexical relevance ranking                            | No                                             |
| `semantic` | Exact cosine ranking over session vectors                   | Yes                                            |
| `hybrid`   | RRF fusion of exact, lexical, symbol, and semantic evidence | Optional; non-vector channels remain available |

Use `read.files` for a bounded ordered batch when paths are already known.
Preview mechanical edits with `dry_run: true`, then apply the same edit with
`expected_replacements` and an independent `max_replacements` when appropriate.

## 5. Enable asynchronous in-memory workspace retrieval

Dense retrieval is an explicit host capability. It is not a durable vector
database and it is not enabled by the selected chat model. The host supplies a
typed embedding provider; A3S Code owns text admission, chunking, batching,
response validation, exact in-memory vector partitions, hybrid ranking,
current-source verification, accounting, and cleanup.

```js
import {
  CallbackEmbeddingProvider,
  RecursiveWorkspaceChunkingStrategy,
  WorkspaceRetrievalOptions,
} from "@a3s-lab/code";

const provider = new CallbackEmbeddingProvider(
  {
    provider: "host-embeddings",
    model: "code-search-v1",
    dimension: 768,
    normalization: "unit",
  },
  async ({ inputs, signal }) => {
    const response = await embeddingClient.embed(
      inputs.map(({ text }) => text),
      { signal },
    );
    return {
      vectors: inputs.map((input, index) => ({
        id: input.id,
        values: response[index],
      })),
    };
  },
);

const retrieval = new WorkspaceRetrievalOptions(
  provider,
  null,
  new RecursiveWorkspaceChunkingStrategy(8 * 1024, 512, [
    "\n\n",
    "\n",
    ". ",
    " ",
  ]),
);
retrieval.maxRecords = 100_000;
retrieval.maxBytes = 128 * 1024 * 1024;

const session = await agent.sessionAsync("/repo", {
  workspaceRetrieval: retrieval,
});

console.log(session.workspaceRetrievalStatus());
console.log(
  await session.hybridSearch({
    query: "where session shutdown releases temporary indexes",
    limit: 8,
  }),
);
await session.closeAsync();
```

Session construction returns before corpus embedding finishes. Status moves
through `building`, `ready`, `degraded`, and `closed`. Completed file partitions
publish atomically, so queries may use partial coverage while building. Closing
the session cancels provider work, joins the indexer within a deadline, and
releases all accounted vector records and bytes.

Only manifest-admitted UTF-8 text enters chunking and embeddings. Generated,
oversized, credential-bearing, `.a3s` control, and non-text files are excluded.
Before returning a hit, Code rereads the source and verifies its full-file
digest and exact chunk range. Stale, deleted, unreadable, or superseded chunks
are not exposed.

The built-in index is an exact, bounded `InMemoryVectorIndex`. Recreating a
session rebuilds its projection; sessions do not share it. If persistence or a
shared vector service is required, the embedding host owns that separate
system and must preserve Code's source-verification boundary.

## 6. Apply governance at the correct boundary

Model-selected calls pass through active-skill restrictions, permission policy,
confirmation, hooks, budget checks, lane admission, timeouts, cancellation,
recursive-call protection, output sanitization, artifact limits, and workspace
path checks.

Direct SDK calls such as `session.tool(...)` are trusted host control-plane
operations. They skip model-facing permission and confirmation because the
embedding application selected the call. Use the governed direct API when a
host-coordinated call must still pass session permission and confirmation.

```python
from a3s_code import PermissionPolicy, SessionOptions

options = SessionOptions()
options.permission_policy = PermissionPolicy(
    allow=["read(*)", "search(*)"],
    deny=["bash(*)"],
    default_decision="deny",
)
```

The host must authenticate and authorize its own user before translating a
request into a trusted direct call. Tool visibility is not an authorization
grant.

## 7. Use typed stores and bounded context

Backend choices are typed objects, not primitive backend names.

```python
from a3s_code import FileMemoryStore, FileSessionStore, SessionOptions

options = SessionOptions()
options.memory_store = FileMemoryStore("./.a3s/memory")
options.session_store = FileSessionStore("./.a3s/sessions")
options.session_id = "review-session"
options.auto_save = True
options.auto_compact = True
options.auto_compact_threshold = 0.75
```

Session snapshots persist the conversation and supported runtime contracts;
they do not serialize live provider callbacks, credentials, MCP processes, or
the ephemeral workspace vector index. Resume must re-inject required live
resources and rejects incompatible persisted policy.

Memory stores hold learned items. Workspace retrieval indexes current source.
They are separate systems and should not be described as interchangeable.

## 8. Compose skills, MCP, delegation, and workflows

- Skills are Markdown packages loaded from explicit directories or registries.
  A3S Code ships no hidden default Skill catalog.
- MCP managers discover external tools over their configured transports.
  Child sessions inherit only the managers and policy the parent host supplies.
- `task` starts one bounded specialist run. Independent fan-out uses the typed
  host helpers or the hidden compatibility alias rather than expanding the
  model-visible schema.
- Agent-wide priority scheduling applies one capacity boundary across sessions,
  direct tools, detached children, and host workflows.
- `program` executes bounded QuickJS orchestration. Dynamic workflows integrate
  with A3S Flow for replay and shared budgets.

Always set explicit worker count, depth, step, token, timeout, and output limits
for unattended workloads. Closing the parent session must cancel or join owned
children before returning.

## 9. Verify outcomes and observe runs

Verification commands turn a completion claim into recorded evidence.

```js
const report = await session.verifyCommands("release-readiness", [
  {
    id: "tests",
    kind: "test",
    description: "Focused tests pass",
    command: "npm test",
    required: true,
    timeoutMs: 120000,
  },
]);

console.log(report);
console.log(session.verificationSummaryText());
```

Run and event APIs expose stable versioned records for headless hosts. Trace
events, verification reports, tool-result evidence, artifacts, and scheduler
snapshots are observations, not permissions or capacity reservations.

## 10. Test and qualify changes

Run focused tests from the affected crate or SDK. The repository's required
remote gates include formatting, strict Clippy, default and all-feature Rust
tests, Go race tests, native Node.js and Python runtime suites, workspace
retrieval churn, documentation checks, and capability-ledger validation.

Performance evidence uses two different kinds of gates:

1. deterministic work and resource ceilings, such as provider requests,
   retries, records, bytes, candidates, queue depth, tool rounds, and complete
   post-close release;
2. release-build latency qualification with a fixed corpus, warmups, repeated
   samples, p50/p95/maximum, machine metadata, and retained JSON artifacts.

Remote model, public search-engine, and third-party service latency must be
reported separately from local Core latency. A job timeout prevents hangs; it
is not a product-performance measurement.

See [Capability Verification](CAPABILITY_VERIFICATION.md) for the 20-area
evidence ledger and unresolved gaps. See
[Workspace Retrieval QA](WORKSPACE_RETRIEVAL_QA.md) for vector quality,
resource, lifecycle, and DeepSeek qualification evidence.

## 11. Authoritative references

- [Versioned website documentation](https://a3s-lab.github.io/Code/)
- [Node.js SDK](../sdk/node/README.md)
- [Python SDK](../sdk/python/README.md)
- [Go SDK](../sdk/go/README.md)
- [Advanced Developer Manual](ADVANCED_DEVELOPER_MANUAL.md)
- [SDK API Design](SDK_API_DESIGN.md)
- [Workspace Retrieval Operations](WORKSPACE_RETRIEVAL_OPERATIONS.md)

When this overview and a versioned SDK declaration disagree, treat the SDK
declaration and executable contract tests for that release as authoritative.
