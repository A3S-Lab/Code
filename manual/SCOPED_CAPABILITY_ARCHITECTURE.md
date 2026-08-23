# Scoped Capability Architecture

Status: accepted foundation; A3S Use bridge delivered; Code gates remain incremental

## Decision

A3S Code will adopt scoped capability lifecycle semantics inspired by
[Cordis](https://github.com/cordiverse/cordis) without becoming a general
dependency-injection or package-management framework.

A3S Use remains the only owner of cognitive-package discovery, verification,
dependency resolution, Grants, immutable package generations, capability
cutover, lifecycle journals, and crash recovery. A3S Code consumes an exact
Use capability snapshot and projects it into product-owned Session, Run, Turn,
and Subtask scopes. A3S Runtime, Gateway, Flow, and Knowledge continue to own
their native execution and data lifecycles.

The design adopts Cordis's useful lifecycle properties:

- context-local capability visibility;
- dependency-aware activation;
- reversible effects owned by one lifecycle instance; and
- deterministic teardown when a scope or provider is replaced.

It does not copy JavaScript Proxy lookup, arbitrary string service location,
ambient `any` values, or callback-defined package authority. Rust types,
ownership, immutable snapshots, and structured concurrency express the same
useful semantics at the Code boundary.

## Ownership boundary

| Owner | Owns | Must not own |
| --- | --- | --- |
| Host Plugin Manager | Registry selection, trust roots, policy, confirmation, secrets, provider injection, and product UX | Package storage or another lifecycle state machine |
| A3S Use | Package graphs, verification, receipts, Grants, lifecycle generations, capability generations, atomic cutover, drain evidence, and recovery | Code sessions, model presentation, or generic workload scheduling |
| A3S Code | Session projection, Run admission, scoped capability visibility, governance ceilings, model presentation, and local effect supervision | Package plan/apply, SemVer resolution, signature policy, Grants, or receipt retirement |
| Runtime and Gateway | Task and Service identity, process execution, health, routing, invocation, and drain | Package trust, Registry selection, or Code model policy |
| A3S Flow | Workflow compilation, execution, replay, and observation | A parallel package lifecycle |
| Knowledge host | OKF validation, indexing, promotion, cited retrieval, retention, and exact query leases | Process execution or Code prompt construction |

The authoritative Use design is the
[A3S Use Plugin Platform Architecture](https://github.com/A3S-Lab/Use/blob/main/docs/plugin-platform-architecture.md).
Code adapters must consume that contract instead of inferring capability state
from package directories or implementing another plan/apply path.

## Delivered A3S Use bridge

A3S Use
[`6ed0b4e`](https://github.com/A3S-Lab/Use/commit/6ed0b4eaf75c464f20690787e7d86471717158df)
delivers the upstream half of Run admission. The implementation exposes two
strict, canonical cursor protocols and retains package lifecycle authority in
Use:

| Contract | Public Use boundary | Verification evidence |
| --- | --- | --- |
| `a3s.use.extension-snapshot-cursor.v1` | `ExtensionRegistrySnapshot::cursor` and `ExtensionRegistry::acquire_published_snapshot` bind the Registry digest and every exact package, manifest, route, and lifecycle generation | [atomic Registry lease tests](https://github.com/A3S-Lab/Use/blob/6ed0b4eaf75c464f20690787e7d86471717158df/crates/extension/src/registry_tests/snapshot_lease.rs) |
| `a3s.use.capability-snapshot-cursor.v1` | An injected `CapabilityRegistry` projects one complete Registry and `CapabilityRegistry::acquire_snapshot_lease` returns its immutable projection plus all upstream route leases | [facade cursor and injected-Registry tests](https://github.com/A3S-Lab/Use/blob/6ed0b4eaf75c464f20690787e7d86471717158df/src/capability_registry/lease.rs) |
| Capability snapshot JSON v2 | The cursor remains an in-process Rust contract and is skipped by the separately released CLI serialization schema | [compatibility assertion](https://github.com/A3S-Lab/Use/blob/6ed0b4eaf75c464f20690787e7d86471717158df/src/capability_registry/lease.rs#L346) |

Acquisition sorts exact package identities, obtains every shared route lease,
and rechecks the immutable publication while the complete batch is held. A
hidden, mixed, contended, stale, digest-mismatched, or legacy unleasable route
cannot yield a partial lease. Rust RAII releases locks acquired by a failed
attempt. The successful `CapabilitySnapshotLease` is deliberately non-clone
and `Send + Sync`; Code will own it at Run scope. Its synchronous `Drop` only
releases route locks, while Use continues to own asynchronous drain,
retirement, and recovery.

## Current baseline

The current implementation already provides several pieces that the migration
must preserve:

| Existing behavior | Evidence | Migration requirement |
| --- | --- | --- |
| Built-in Tools cannot be replaced or removed through dynamic registration | [`ToolRegistry` tests](../core/src/tools/registry/tests.rs) | Built-ins become a sealed base contribution layer |
| Session MCP and Skill removal restores only the exact registration it still owns | [`session_extensions`](../core/src/agent_api/session_extensions.rs) and [`live_skill_lifecycle`](../core/tests/live_skill_lifecycle.rs) | Replace pointer shadow chains with source-owned contributions while preserving safe removal |
| Run admission is single-flight and released by an RAII lease | [`run_admission`](../core/src/agent_api/run_admission.rs) | Attach the immutable capability set and upstream generation lease to the admitted Run |
| Permission and confirmation providers can be snapshotted for a Run | [`invocation_context`](../core/src/agent/invocation_context.rs) | Fold them into one immutable Run governance scope |
| Model-visible capability and input evidence is content-addressed | [`harness_evidence`](../core/src/harness_evidence.rs) | Derive existing v1 evidence from the new internal scope snapshot during compatibility |
| Session close is idempotent, cancellation-aware, and bounded | [`session_close`](../core/src/agent_api/session_close.rs) and [`close integration tests`](../core/tests/test_session_close_lifecycle.rs) | Move capability cleanup under one supervisor without weakening existing close guarantees |

The baseline also exposes the migration pressure. Tool, Skill, Agent, Command,
Hook, and MCP state is held in separate mutable registries. Dynamic Tool and
Skill replacement uses pointer identity and manually maintained shadow chains.
The CLI host reconciles one Use generation through multiple independent live
Session mutations. A failure can leave prepared runtime resources and visible
surfaces at different convergence points even when the host has not advanced
its reported generation.

## Capability lifetimes

| Lifetime | Contents | Mutation rule |
| --- | --- | --- |
| Session-static | LLM client, workspace services, stores, sandbox, host environment | Fixed after Session construction; replacement creates a new Session |
| Session-live catalog | Tool, Skill, Agent, Command, Hook, MCP, Flow, Knowledge, UI, and context projections | Replaced only by an atomic immutable catalog generation |
| Run-frozen governance | Permission, confirmation, security, budget, execution ceiling, and presentation profile | Captured at admission; child scopes may only narrow it |
| Turn/Subtask ephemeral | Explicit temporary wrappers, scratch resources, and child-only presentation | Created through a child transaction and released with that scope |

Retrieval readiness and similar observation state may be sampled immediately
before each model call. It may change evidence for a call, but it cannot change
the structural catalog generation pinned by the Run.

## Stable identities and generations

Raw integers and display names must not cross generation boundaries without a
typed wrapper. The implementation will distinguish at least:

- `UsePackageGeneration`, identifying one immutable package artifact;
- `UseCapabilityGeneration`, identifying one atomic Use Registry snapshot;
- `CodeCatalogGeneration`, identifying one product- and Session-specific
  projection; and
- `CapabilityId`, identifying a stable source and surface independent of a
  process ID, route port, temporary path, or display label.

One Use capability generation can produce different Code catalog generations
because Sessions can have different workspace scopes, host providers,
governance ceilings, or product surfaces. Code therefore records the upstream
generation and digest but never aliases its local generation to the Use number.

Every immutable set uses canonical ordering and a domain-separated digest.
`BTreeMap` is the baseline representation. More specialized atomics or
`arc-swap` require benchmark evidence; they are not foundation dependencies.

## Rust model

The public model is strongly typed and does not expose `Any`:

```rust
pub struct CapabilityScope<K: sealed::ScopeKind> {
    set: Arc<CapabilitySet>,
    ceiling: CapabilityCeiling,
    _kind: PhantomData<K>,
}

pub struct CapabilityTxn<S> {
    staged: Vec<CapabilityContribution>,
    effects: EffectStack,
    _state: PhantomData<S>,
}

pub enum CapabilityValue {
    Tool(Arc<dyn Tool>),
    Skill(Arc<Skill>),
    Agent(Arc<AgentDefinition>),
    Command(Arc<dyn SlashCommand>),
    Hook(Arc<dyn HookHandler>),
    Mcp(Arc<McpBinding>),
}
```

Capability categories are closed so exhaustive matching remains possible.
Implementations inside a category remain open through `Send + Sync + 'static`
traits. Only `CapabilityTxn<Validated>` can commit. Scope marker types prevent
a Turn or Subtask lease from being passed where a Session lease is required.

Rust has no asynchronous `Drop`. A lease's `Drop` may only release an atomic
count, cancel a token, and notify its supervisor. The supervisor owns a
`CancellationToken`, `JoinSet`, retired-generation queue, and bounded,
idempotent `close().await`. `Drop` must not spawn an unowned Tokio task.

## Publication lifecycle

One source update follows a prepare, validate, commit, drain lifecycle:

1. Observe one exact upstream generation and digest.
2. Stage the source's complete contribution batch.
3. Prepare fallible resources without changing model-visible state.
4. Validate identities, conflicts, dependencies, ceilings, resource bounds,
   and upstream evidence for the complete batch.
5. Build a canonical immutable `CapabilitySet`.
6. Commit with one short writer critical section and publish the new `Arc`.
7. Stop new Run admission against the prior set.
8. Let already admitted Runs retain their exact set and upstream leases.
9. Teardown retired effects in reverse order after the final lease is released.

A failed prepare or validation leaves the current set untouched. Cancellation
rolls back staged effects. After commit, recovery completes retirement; it must
not reconstruct a mixed old/new visible set.

For A3S Use projections, Run admission requires an all-or-nothing upstream
snapshot lease. Acquisition uses the exact capability generation, revision,
and sorted package-generation identities, then rechecks the publication after
all underlying leases are held. If the old generation has already been hidden,
admission refreshes the source instead of starting a new Run with stale Tools.

## Contribution and conflict rules

Each contribution records its `CapabilityId`, source, surface, precedence
class, upstream generation, descriptor digest, runtime value, dependencies,
readiness, and owned effects.

- Built-ins form a sealed base layer.
- External sources cannot choose their own trust or precedence class.
- Security-sensitive public-name conflicts fail the complete transaction.
- Any allowed shadowing is decided by explicit host policy and deterministic
  source precedence, never insertion order.
- Removing a source removes its contributions and recomputes the visible set;
  it never restores a saved pointer.
- Package and SemVer dependency resolution remains in A3S Use. Code may validate
  and order a bounded readiness DAG from the published surface edges only.

## Architectural invariants

| ID | Invariant |
| --- | --- |
| `CAP-I01` | A3S Use is the sole authority for package plan/apply, verification, dependency resolution, Grants, lifecycle generation, capability cutover, and recovery. |
| `CAP-I02` | One source generation is projected into a Session as one atomic contribution batch. |
| `CAP-I03` | A failed transaction cannot change the visible catalog generation or leave a partially visible batch. |
| `CAP-I04` | A Run's model definition and execution resolve through the same pinned capability value. |
| `CAP-I05` | External hot-plug affects the next admitted Run; an admitted Run retains its exact local and upstream generation leases. |
| `CAP-I06` | A child scope cannot broaden its parent's permission, confirmation, security, budget, workspace, or execution ceiling. |
| `CAP-I07` | Required dependencies must be ready in the same upstream generation before a contribution becomes visible. |
| `CAP-I08` | Built-in capabilities cannot be replaced or removed by an external source. |
| `CAP-I09` | Steady-state Run execution performs no global catalog write and does not resolve through a mutable latest-value registry. |
| `CAP-I10` | Teardown is reverse-order, cancellation-safe, idempotent, bounded, and observable. |
| `CAP-I11` | Canonical capability identity and digest are deterministic across insertion order and supported platforms. |
| `CAP-I12` | SDKs expose stable descriptors and explicit registration methods, not Rust heterogeneous storage or package-manager authority. |

## Planned module boundary

The implementation belongs under `core/src/capability/`, split by concern:

```text
capability/
├── id.rs
├── descriptor.rs
├── value.rs
├── set.rs
├── transaction.rs
├── scope.rs
├── ceiling.rs
├── lease.rs
├── effect.rs
├── supervisor.rs
└── projection.rs
```

The directory is not a new generic framework. It is the Code-owned lifecycle
kernel for the fixed product capability categories above. Surface-specific
MCP, Tool, Skill, Flow, Knowledge, and UI adapters remain with their owning
concerns and produce typed contributions.

## Delivery gates

| Gate | State | Outcome | Exit criteria |
| --- | --- | --- | --- |
| `CAP-FND1` | Delivered | Accepted ownership, lifetime, identity, failure, verification, and migration contract | The contract and Roadmap are mechanically aligned; existing lifecycle and concurrency evidence is recorded and green |
| `USE-BRIDGE1` | Delivered | Use `6ed0b4e` publishes `a3s.use.extension-snapshot-cursor.v1`, `a3s.use.capability-snapshot-cursor.v1`, and a non-clone atomic exact-generation snapshot lease | Full Use tests and strict Clippy pass; acquisition is all-or-nothing and rejects hidden, mixed, contended, stale, unleasable, or digest-mismatched generations without changing capability snapshot JSON v2 |
| `CAP-SET1` | Planned | Typed identities, immutable `CapabilitySet`, canonical digest, and source contribution model | Digests are deterministic and readers pin an `Arc` without a global writer lock |
| `CAP-SCOPE1` | Planned | Session/Run/Turn/Subtask scopes, ceilings, leases, effects, and structured-concurrency supervisor | Temporary capabilities cannot escape their scope or broaden a child; `HARNESS-SCOPE1` is delivered |
| `CAP-PROJ1` | Planned | Typestate contribution transaction and projection adapters | Failed prepare/validate/commit races never publish a partial generation |
| `CAP-DEP1` | Planned | Bounded surface readiness DAG | Only published surface edges are ordered; Code does not resolve packages or become general DI |
| `HOST-CAP1` | Planned | CLI and Desktop apply each Use generation to each Session as one batch | Old Runs retain the old lease, new Runs see the new generation, and host generation never advances after partial reconciliation |
| `CAP-PROFILE1` | Planned | Typed presentation profiles over the same governed executor | Presentation can change token cost and model shape but never authority; `HARNESS-PROFILE1` is delivered |
| `CAP-GA1` | Planned | Legacy shadow ownership and piecemeal reconciliation removed after one major compatibility period | Official hosts and SDKs use the scoped architecture and the complete verification matrix passes |

`CAP-SET1` now consumes the delivered Use cursor and lease. Tool and Skill
projection migrate first, Agent/Command/Hook second, and MCP or
other asynchronous resources last. Host integration follows the atomic Core
transaction instead of adding another reconciliation abstraction.

## Verification matrix

Every implementation gate must add deterministic evidence for its changed
behavior. The complete program requires:

- add/remove racing with Run admission and Session close;
- cancellation during prepare, validation, commit handoff, and drain;
- no generation advance after partial reconciliation;
- an old Run retaining N while a new Run observes N+1;
- compile-fail and runtime tests that child scopes cannot expand ceilings;
- definition/execution identity equality for governed Tool calls;
- canonical digest parity across insertion orders and supported platforms;
- failed transactions leaving no visible contribution or orphaned effect;
- no retained task, process, socket, file lock, temporary file, or generation
  after bounded close and churn/soak;
- Rust, Node.js, Python, and Go descriptor/snapshot parity; and
- Cloud contract fixtures and `compat/cloud-stack.acl` updates only when a
  cross-Cloud protocol changes.

Use `trybuild` or compile-fail doctests for type-state and scope misuse. Use
`loom` only if the implementation introduces custom atomic protocols that
ordinary deterministic interleaving tests cannot cover.

## Observability and performance

The capability kernel will expose generation, projection lag, prepare/commit
outcomes, rollback, active leases by scope, retired generations, drain latency,
and teardown failures without recording capability payloads or secrets.

After Run admission, Tool definition lookup and execution must use the pinned
set and acquire no global catalog write lock. Mutation cost is measured off the
Run hot path. Performance thresholds will be based on the checked-in baseline;
the architecture does not select a specialized synchronization primitive before
measurement demonstrates a need.

## Compatibility

The existing `RunCapabilitySnapshotV1` uses strict deserialization and remains
unchanged. The new internal snapshot initially derives the existing v1 event.
Once stable, Code may add a separately versioned `CapabilityScopeSnapshotV1`
and event rather than appending fields to v1.

Node.js, Python, and Go receive serializable descriptors, snapshot identities,
and explicit typed APIs. They do not receive a Rust capability map. Existing
single-surface registration methods remain compatibility adapters for one major
release and must delegate to the same transaction path before removal.
