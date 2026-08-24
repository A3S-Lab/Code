# Scoped Capability Architecture

Status: accepted foundation; A3S Use bridge, immutable set, scoped lifecycle, atomic runtime projection, surface readiness DAG, official Tool/Skill host adoption, Run-frozen Tool presentation, Agent delegation, Command dispatch, Hook execution, and Core MCP projection delivered

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
The remaining compatibility host still reconciles Flow, Knowledge, Runtime
Task, and legacy MCP surfaces independently. Tool, Skill, Agent, Command, Hook,
and exact-client MCP projections have moved to one atomic Core batch; later
gates must preserve that boundary as the remaining categories and official MCP
hosts migrate.

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

## Delivered immutable identity set

`CAP-SET1` is implemented under [`core/src/capability/`](../core/src/capability/).
It deliberately freezes the identity plane before attaching runtime values or
effects:

| Rust boundary | Delivered invariant |
| --- | --- |
| `UseCapabilityGeneration` | Binds `a3s.use.capability-snapshot-cursor.v1`, the Use capability revision, and the authoritative Extension Registry revision without aliasing the local Code generation |
| `UsePackageGeneration` | Binds package, component, route, version, lifecycle generation, package digest, and manifest digest from one exact Use cursor |
| `CapabilitySource` and `CapabilityContribution` | Assign source class through typed host constructors, keep Built-in construction crate-private, and require one non-empty complete descriptor batch per exact source |
| `CapabilitySet` | `from_use_projection` retains the upstream cursor even for an empty product projection; construction rejects mixed Use cursors, duplicate sources or identities, public-name conflicts, Built-in shadowing, unresolved dependencies, and bounded-input overflow before returning `Arc<CapabilitySet>` |
| `a3s.code.capability-set.v1` | Hashes canonical `BTreeMap` order through a bounded streaming, domain-separated SHA-256 writer; the local `CodeCatalogGeneration` is part of the identity |

The [public contract tests](../core/tests/capability_set.rs) lock typed input
validation, aggregate bounds, source ownership, insertion-order parity, one
golden digest, mixed-Use rejection, and `Send + Sync` `Arc` pinning. The
[crate-private set test](../core/src/capability/set.rs) proves an external
source cannot manufacture or shadow a sealed Built-in contribution.

`CapabilitySet` still does not publish into live Session registries or carry
runtime trait objects. `CAP-SCOPE1` owns lifetimes, ceilings, leases, and
supervised effects; `CAP-PROJ1` attaches closed category-specific values in a
separate immutable `CapabilityProjection`. Keeping those fallible and
asynchronous concerns out of `CapabilitySet` makes the identity plane small,
deterministic, and lock-free for readers.

## Delivered scoped lifecycle kernel

`CAP-SCOPE1` is implemented beside the identity plane without moving package
authority out of Use:

| Rust boundary | Delivered invariant |
| --- | --- |
| `CapabilityCeiling` | Binds one catalog digest, a canonical capability subset, workspace flags, required parent governance guards, and numeric execution maxima; every child dimension must be equal or narrower |
| `CapabilityScope<Session/Run/Turn/Subtask>` | Sealed marker types permit only Session-to-Run, Run-to-Turn/Subtask, and Turn-to-Subtask construction; every child shares the same immutable set and a derived cancellation token |
| `CapabilityLease<'scope, K>` | Borrows its typed owner, filters descriptors through the ceiling, and fails after cancellation; compile-fail tests prevent lifetime escape and marker substitution |
| `RetainedUseGeneration` | A trusted host adapter retains the real non-clone A3S Use `CapabilitySnapshotLease`; Run admission rejects missing, unexpected, or cursor-mismatched leases |
| Scope supervisor | Owns a bounded `JoinSet`, child registry, reverse-order effect stack, and final generation lease; one cancellation-safe close driver makes teardown idempotent and observable |

Close cancels first, settles or aborts tasks, recursively closes children in
reverse registration order, closes local effects in reverse order, and only
then drops the exact Use generation lease. One shared deadline bounds the
sequence. Failures and timeouts are counted in `ScopeCloseReport`; they do not
short-circuit older effects. Cancelling the first `close().await` waiter cannot
cancel the supervisor-owned close driver. `Drop` propagates cancellation and
aborts owned task futures synchronously, including descendants, but never
spawns an unowned Tokio task. Explicit close remains required for asynchronous
resource teardown.

The public tests in
[`capability_scope.rs`](../core/tests/capability_scope.rs) exercise all ceiling
dimensions, scoped filtering, exact Use lease mismatch and release order,
recursive Subtask/Turn/Run close, waiter cancellation, reverse effects, stuck
tasks/effects, descendant abort on parent drop, idempotence, and `Send + Sync`.
The compile-fail examples live with
[`CapabilityLease`](../core/src/capability/lease.rs).

## Delivered atomic runtime projection

`CAP-PROJ1` is implemented as a parallel Core catalog. `HOST-CAP1` binds
official Tool/Skill hosts to it, and `HOST-AGENT1` binds Core Agent delegation
to the same Run generation. `HOST-COMMAND1` binds Core slash-command dispatch
to that generation, `HOST-HOOK1` binds Hook definitions, handlers, and
observational work, and `HOST-MCP1` binds one exact initialized MCP client and
tool catalog without claiming end-to-end A3S Use or official-host adoption.

| Rust boundary | Delivered invariant |
| --- | --- |
| `CapabilityValue` | A closed enum accepts Tool, Skill, Agent, Command, Hook, per-server MCP binding, Flow, Knowledge, and context values without `Any`; UI fails closed until Core owns a typed UI runtime contract |
| `CapabilityProjection` | Pairs one immutable `Arc<CapabilitySet>` with exactly one canonically ordered value per descriptor and rejects missing, extra, kind-mismatched, unsupported, or public-name-mismatched values |
| `CapabilityTxn<Staged/Prepared/Validated>` | Surface-owned adapters prepare in dependency-first readiness order; only `Validated` exposes `commit`, and a compile-fail contract prevents early publication |
| `CapabilityCatalog` | Readers pin one non-clone immutable generation; writers compare both base generation and digest under one short mutex before swapping the complete projected `Arc` |
| Retirement and rollback | Transaction ownership synchronously transfers all completed effects to a catalog cleanup queue on prepare failure, cancellation, validation failure, dropped transaction, or lost CAS; an `Arc`-owned published generation transfers its effects only after the final old reader lease drops |

The catalog never parses a package, selects a version, computes Grants, or
advances an A3S Use lifecycle generation. Its `CapabilitySet` retains the
complete Use cursor produced by `USE-BRIDGE1`. Run admission pairs the Code
projection lease with a fresh real non-clone Use `CapabilitySnapshotLease`;
neither lease substitutes for the other.

The public tests in
[`capability_projection.rs`](../core/tests/capability_projection.rs) cover
complete value validation, unsupported UI rejection, cancellation during
prepare, reverse rollback, validation failure, an exact commit CAS race,
definition/execution pointer identity, an old generation surviving cutover,
final-lease effect retirement, dropped-transaction recovery, and `Send + Sync`
catalog readers. Cleanup is explicit and bounded through
`drain_cleanup_with_policy`; `Drop` only transfers effect ownership and never
spawns an asynchronous task.

## Delivered bounded surface readiness DAG

`CAP-DEP1` adds `CapabilityReadinessPlan` between the immutable identity set
and adapter preparation. The plan is derived only from
`CapabilityDescriptor::dependencies()` in one complete `CapabilitySet` and is
bound to that set's `CodeCatalogGeneration` and digest. It never reads a Use
manifest, selects a package version, computes Grants, or performs service
injection.

An iterative Kahn traversal over `BTreeMap` and `BTreeSet` produces canonical
minimal readiness waves and a deterministic flattened activation order.
Planning is bounded by 4,096 capabilities, 32,768 surface edges, 128 direct
dependencies per capability, and at most 4,096 waves. A maximum-depth chain
does not recurse. Empty sets are valid; any multi-node cycle fails closed with
the first canonical blocked identity and total blocked count before a runtime
projection or transaction can exist.

`CapabilityCatalog::begin` freezes the plan beside the target set. Before the
first adapter starts, `prepare` verifies that every target descriptor has
exactly one staged adapter. It then prepares dependency waves in canonical
order. Preparation is deliberately sequential in this gate so effect transfer
and rollback order remain deterministic. A prerequisite failure prevents all
dependent adapters from starting, and already completed prerequisite effects
close in reverse order through the existing rollback queue.

The plan is retained in `CapabilityProjection`, so a pinned Run can inspect
the exact readiness ordering associated with its catalog generation. A
cross-package surface edge remains legal only after A3S Use has already
published one complete cursor and Code has retained that cursor in the set.
This surface DAG is not the Use package DAG and cannot install, resolve,
activate, hide, retire, or recover a package.

The public tests in
[`capability_readiness.rs`](../core/tests/capability_readiness.rs) cover empty,
wide, deep, diamond, cyclic, incomplete, failed, and cross-package graphs;
insertion-order determinism; dependency-first adapter execution; reverse
rollback; exact Use cursor retention; and `Send + Sync` readers.

## Rust model

The public model is strongly typed and does not expose `Any`:

```rust
pub struct CapabilityScope<K: sealed::ScopeKind> {
    inner: Arc<ScopeInner>,
    _kind: PhantomData<K>,
}

pub struct CapabilityLease<'scope, K: sealed::ScopeKind> {
    inner: &'scope ScopeInner,
    _kind: PhantomData<K>,
}

pub trait RetainedUseGeneration: Send + Sync + 'static {
    fn use_generation(&self) -> &UseCapabilityGeneration;
}

pub struct CapabilityTxn<S> {
    staged: BTreeMap<CapabilityId, Box<dyn CapabilityProjectionAdapter>>,
    prepared: BTreeMap<CapabilityId, CapabilityValue>,
    projection: Option<Arc<CapabilityProjection>>,
    _state: PhantomData<S>,
}

pub struct CapabilityReadinessPlan {
    generation: CodeCatalogGeneration,
    digest: Sha256Digest,
    waves: Vec<Vec<CapabilityId>>,
    activation_order: Vec<CapabilityId>,
}

pub enum CapabilityValue {
    Tool(Arc<dyn Tool>),
    Skill(Arc<Skill>),
    Agent(Arc<AgentDefinition>),
    Command(Arc<dyn SlashCommand>),
    Hook(Arc<HookBinding>),
    Mcp(Arc<McpBinding>),
    Flow(Arc<DynamicWorkflowRuntime>),
    Knowledge(Arc<CognitiveContextSession>),
    Context(Arc<dyn ContextProvider>),
}
```

Capability categories are closed so exhaustive matching remains possible.
Implementations inside a category remain open through `Send + Sync + 'static`
traits. Only `CapabilityTxn<Validated>` can commit. Scope marker types prevent
a Turn or Subtask lease from being passed where a Session lease is required.

Rust has no asynchronous `Drop`. A scope's `Drop` may only cancel tokens and
abort owned task futures. The delivered supervisor owns a `CancellationToken`,
`JoinSet`, child registry, effect stack, exact upstream generation lease, and
bounded idempotent `close().await`. Projection `Drop` only moves owned effects
into the delivered cleanup queue; explicit bounded drain performs asynchronous
close. Neither path spawns an unowned Tokio task.

## Publication lifecycle

One source update follows a prepare, validate, commit, drain lifecycle:

1. Observe one exact upstream generation and digest.
2. Build the complete canonical `CapabilitySet`, retaining its exact Use
   cursor after product filtering.
3. Build and validate the bounded surface readiness plan, begin against the
   current Code generation/digest, and stage one adapter for every target
   descriptor.
4. Verify staged completeness, then prepare fallible resources in canonical
   dependency-first waves without changing model-visible state.
5. Validate the complete descriptor/value pairing, kinds, public identities,
   resource bounds, and unsupported categories.
6. Commit with one short generation-and-digest CAS and publish the new `Arc`.
7. Stop new Run admission against the prior set.
8. Let already admitted Runs retain their exact set and upstream leases.
9. Teardown retired effects in reverse order after the final lease is released.

A failed prepare, cancellation, validation, dropped transaction, or commit CAS
leaves the current set untouched and transfers every completed effect for
reverse cleanup. After commit, `Arc` ownership delays retirement until the last
old projection lease is released; cleanup never reconstructs a mixed old/new
visible set.

For A3S Use projections, Run admission requires an all-or-nothing upstream
snapshot lease. Acquisition uses the exact capability generation, revision,
and sorted package-generation identities, then rechecks the publication after
all underlying leases are held. If the old generation has already been hidden,
admission fails before Run execution; the host must refresh and publish the
new source instead of starting a Run with stale Tools.

### HOST-CAP1 host runtime cut

The first host-runtime cut established Session Tool and Skill values.
`SessionCapabilityBatch` owns the complete next projection plus a
generation-specific `UseGenerationLeaseProvider`. `AgentSession` prepares the
whole batch without changing visibility, validates it against the frozen
compatibility Tool and Skill maps, and publishes the projection and provider
in one catalog CAS. This gate did not claim the other capability kinds;
`HOST-AGENT1` extends the same boundary to Agent definitions and
`HOST-COMMAND1` extends it to slash Commands. `HOST-HOOK1` extends it to Hook
bindings, and `HOST-MCP1` extends it to exact-client MCP bindings. Flow,
Knowledge, Context, and UI still fail before preparation.

Run admission pins three things at one linearization point: the immutable Code
projection, frozen compatibility Tool/Skill/Agent/Command/Hook maps plus exact
MCP bindings, and the governance ceiling. It then asks the published provider
for a fresh, non-clone retained generation. The provider implementation must
own the concrete A3S Use `CapabilitySnapshotLease`; a generation number or
cached projection pointer is not a substitute. Code rechecks generation,
capability revision, and Registry revision before the Run scope accepts the
lease.

The same projected Tool `Arc` creates the model definition and services the
governed execution call. Skill catalog context, `search_skills`, and nested
Skill execution are rebound to the same frozen Skill registry. A concurrent
N+1 commit cannot rewrite an admitted N Run, and it cannot release N's Use
lease before Run teardown. Compatibility registration cannot shadow a name
already owned by the published projection. Slash-command lookup and execution
resolve through one frozen Command registry. Preparation cancellation, name
conflict, Session close, or a lost CAS leaves the visible generation unchanged
and transfers every prepared effect to bounded cleanup. A non-clean Run scope
close is a typed Run failure rather than a warning-only condition.

The official
[CLI adapter](https://github.com/A3S-Lab/CLI/commit/df2e081aff168bb0d5d7db895962583559979ad6)
and
[Desktop adapter](https://github.com/A3S-Lab/Desktop/commit/23f00e2bc8b555b406f6bcfe2df92b7c8c17a163)
complete the first host cut over Core
[`d1f70d5`](https://github.com/A3S-Lab/Code/commit/d1f70d57bcdbb3459d3205b647d61ebc0a00a940).
The resident CLI watcher builds one complete verified Skill batch for every
Use cursor and lets each admitted Run acquire its own real non-clone Use
snapshot lease. One-shot Code Exec uses an `AtomicToolSkill` projection mode,
so MCP, Flow, Knowledge, and Runtime Task compatibility effects are never
started by that path.

Ordinary Code Exec performs read-only discovery of an already-ready Use
component and never installs it. A missing component preserves the no-Use
execution path. An incompatible optional component can be skipped only after
its Registry watcher has been cancelled and joined and the Session catalog is
proven equal to its pre-setup stamp. Any visible catalog change or incomplete
shutdown fails closed.

A3S Desktop requires the reserved `scoped-v1` process contract. Discovery
probes the exact side-effect-free help route rather than trusting SemVer alone,
and every launch passes the mode as shell-free arguments. Required setup may
perform policy-authorized first-use installation, but offline and
no-auto-install boundaries fail before provider egress. Before Run admission,
Code waits for the atomic receipt, stops discovery, and verifies that the final
Session stamp still matches it. A successful result returns
`a3s.code.scoped-capability-runtime.v1` with the positive Code generation and
digest plus the complete `a3s.use.capability-snapshot-cursor.v1` cursor.
Desktop rejects absent or malformed evidence as
`protocol.capability-runtime-invalid`.

Cancellation during setup remains `operation.cancelled`. Stream creation,
worker failure, normal completion, and setup failure all converge on explicit
Session close. Integration tests cover first-use installation, installed-only
offline reuse, missing and incompatible optional Use, no provider egress when
required setup is forbidden, exact Skill visibility and evidence, watcher
cleanup, and the absence of MCP startup.

### CAP-PROFILE1 model-presentation cut

`CAP-PROFILE1` is a Code-owned view over the exact Tool values already admitted
by `HOST-CAP1`. It introduces no package, Registry, capability-generation, or
execution ownership. The complete path is:

```text
A3S Use capability snapshot + non-clone Run lease
                         │
                         ▼
        Code Run-frozen Tool Arc values
                         │
                         ▼
          permission visibility ceiling
                         │
                         ▼
        ToolPresentationProfileV1 projection
                         │
                         ▼
              provider Tool definitions

model Tool call ──▶ existing ToolInvoker ──▶ same pinned Tool Arc
```

`ToolPresentationProfileV1` is a serializable Rust closed value with schema
`a3s.code.tool-presentation-profile.v1`. Its four exhaustive modes are
Adaptive, Direct, Code, and Disabled. Adaptive preserves the prior
prompt-sensitive selector. Direct exposes every permission-visible definition.
Code exposes only the already registered `program` Tool and replaces only its
description with a bounded compact signature catalog. Disabled sends no Tool
definitions. All outputs are canonical by Tool name; no mode can add a name or
change a parameter schema.

Permission filtering runs before projection. This order is part of the
security boundary because a code-mode catalog must never reveal a hidden Tool
name. The Profile receives definitions, not `Arc<dyn Tool>` values, and main
turn execution continues through the existing Run-owned `ToolExecutor`,
permission, confirmation, budget, hook, cancellation, security, and audit
paths. Host-direct execution is unchanged because Disabled is a presentation
choice rather than a deny policy.

The Session resolves and persists one exact Profile. Run construction copies
it into `AgentConfig`; delegated tasks and workflows copy the parent's value
exactly. `ensure_within` defines the version-1 partial order for future
child-local selection: Disabled is always narrower, equal modes are valid, and
Direct may narrow to another mode; Adaptive and Code are otherwise
incomparable. Resume inherits the persisted value or rejects an explicitly
different Profile before a new Session becomes live.

`HARNESS-PROFILE1` records `ModelPresentationSnapshotV1` before every model
input. A profiled call binds the exact Profile identity, permission-filtered
source count/digest/token estimate, and actual presented count/digest/token
estimate. The capture re-derives the expected projection and fails before
provider use if the request contains an unknown name or changes a schema or
description. Auxiliary helper protocols record an identity projection instead
of pretending that the Session Profile governed their host-owned Tool list.
The event is part of the existing `EventEnvelopeV1` journal and SDK catalogs.

Node.js exposes a typed Profile object with a closed generated mode enum.
Python exposes four static Profile constructors with read-only identity. Go
exposes a named mode type, constants, and a Profile constructor. These SDK
values all map to the same Rust type; no SDK accepts a primitive Profile name
as the Session extension option.

This cut deliberately does not consume an A3S Use cursor, publish a Code
catalog generation, resolve a package, compute a Grant, install a Tool, or own
drain and recovery. A3S Use remains authoritative for all of those operations;
Profile evidence only explains which definitions from the already admitted
Run were placed in one model request.

### HOST-AGENT1 Agent runtime cut

`HOST-AGENT1` admits `CapabilityValue::Agent` through the existing
`SessionCapabilityBatch`; it does not add an Agent package manager or a second
generation counter. A3S Use still selects, verifies, grants, publishes, drains,
and recovers the package generation. Code receives the exact projected
`AgentDefinition` values and the generation-specific lease provider in the
same transaction as Tool and Skill values.

At Run admission, Code snapshots the compatibility `AgentRegistry` into an
independent name map and atomically merges the projected Agent definitions.
The new map shares immutable `Arc<AgentDefinition>` values instead of copying
prompts or mutable registry state. Exact names and the normalization aliases
already accepted by Agent lookup share one conflict domain. A conflict fails
validation before the catalog CAS, and `register_worker_agent`, batch worker
registration, and `register_agent_dir` cannot shadow an Agent in the published
projection.

```text
A3S Use capability generation + exact Run lease
                         │
                         ▼
          Code immutable capability projection
                         │
                         ▼
 compatibility Agents + projected Agents ──▶ Run-owned AgentRegistry
                                              │
                              ┌───────────────┴───────────────┐
                              ▼                               ▼
                    automatic selection             task / parallel_task
                                                              │
                                                              ▼
                                                    delegated child Agent
```

`AgentConfig` and the rebuilt delegation Tools receive the same Run-owned
registry. The Tool definitions shown to the parent and the executor that later
looks up the child therefore cannot observe different Agent generations. The
delegation parent context also carries the Run-frozen Skill registry, so a
nested child cannot escape into a newer Skill map through this rebind.

An N+1 publication changes only later Run admission. An already admitted N Run
continues to expose the N Agent catalog, resolves `task` through the N registry,
and retains the exact N A3S Use lease until the foreground child and parent Run
settle. Hook now follows the same Run generation through `HOST-HOOK1`. MCP and
its delegated wrappers now follow it through `HOST-MCP1`; Flow, Knowledge,
Context, and UI remain separate migration work.

### HOST-COMMAND1 Command runtime cut

`HOST-COMMAND1` admits `CapabilityValue::Command` through the existing
`SessionCapabilityBatch`. A3S Use remains responsible for selecting,
verifying, granting, publishing, draining, and recovering the package
generation. Code receives the exact projected `Arc<dyn SlashCommand>` values
and the generation-specific lease provider; it does not clone Command objects
or create another package lifecycle.

Blocking `send` and streaming `stream` first acquire the Session single-flight
admission lease. Slash-command syntax then enters the same capability Run used
by model-backed execution. At that boundary, Code snapshots the compatibility
`CommandRegistry`, merges the projected generation into an independent name
map, and builds `CommandContext` Tool names from the same pinned
`ToolExecutor`. The capability Run and exact A3S Use generation lease remain
alive until synchronous Command execution has produced its output.

```text
A3S Use capability generation + exact Run lease
                         │
                         ▼
          Code immutable capability projection
                         │
                         ▼
 compatibility Commands + projected Commands ──▶ Run-owned CommandRegistry
                                                   │
                                    ┌──────────────┴──────────────┐
                                    ▼                             ▼
                              blocking send                streaming dispatch
                                    │                             │
                                    └──────── execute ────────────┘
```

Built-in and compatibility Command names form one fail-closed conflict domain
with projected names. Validation holds the legacy registry lock through the
catalog CAS because the public `command_registry()` guard remains mutable for
SDK compatibility. A direct legacy mutation therefore linearizes before
validation or after publication; it cannot enter between them. After
publication, both `register_command` and the legacy guard reject a projected
name.

An N Command already executing when N+1 is published continues to resolve and
execute the exact N `Arc`. Its N A3S Use lease remains retained through that
execution, while the next blocking or streaming dispatch admits N+1. Hook is
bound to the same Run by `HOST-HOOK1`; MCP is bound by `HOST-MCP1`, while the
remaining asynchronous resources stay gated on product-owned supervised
effects.

### HOST-HOOK1 Hook runtime cut

`HOST-HOOK1` admits `CapabilityValue::Hook(Arc<HookBinding>)` through the
existing `SessionCapabilityBatch`. A `HookBinding` is one immutable ownership
atom containing `Arc<Hook>` metadata and its exact `Arc<dyn HookHandler>`.
Code therefore cannot combine a definition from Use generation N with a
callback from N+1. A3S Use still selects, verifies, grants, publishes, drains,
and recovers the package generation; Code receives only the already selected
binding and the generation-specific lease provider.

Publication and Run pinning share the Session's short extension-mutation gate.
Validation snapshots compatibility Hook definitions and handlers in one fixed
`hooks`-then-`handlers` lock order. A conflict with either map, an orphaned
compatibility handler name, or a duplicate projected name fails before the
catalog CAS. Equal-priority execution is canonical by Hook ID. The official
Node.js, Python, and Go bridges publish and remove complete registrations
through Core's atomic `register_hook_registration` and
`unregister_hook_registration` methods; piecemeal Rust methods remain only for
compatibility.

```text
A3S Use capability generation + exact Run lease
                         │
                         ▼
        Code immutable HookBinding projection
                         │
                         ▼
 compatibility snapshot + projected bindings ──▶ Run-owned HookEngine
                                                    │
 optional Session-static external executor ─────────┤
                                                    ▼
                           gating decisions + supervised observations
```

An optional Session-static external `HookExecutor` remains the outer layer and
retains its prior replacement semantics for the compatibility registry. Its
terminal block, retry, or escalation stops protected work. Its `Skip` applies
only to its own chain and cannot bypass the projected layer. Projected Hooks
always execute from the Run snapshot. With no external executor, compatibility
Hooks and projected bindings share that frozen snapshot; subsequent Session
mutation affects only later Runs.

Projected `SessionStart` and `SessionEnd` events fail before publication because
Session construction and teardown are outside a capability Run. Projected
`SkillLoad` and `SkillUnload` also fail because their package lifecycle belongs
to A3S Use and Code has no Run-scoped production emitter for them. Other Hook
points execute only where the Agent loop produces the matching Run event.

Observational dispatch no longer creates an untracked Tokio task. The
Run-owned executor registers PostToolUse, PostResponse, OnError,
`async_execution`, and timed-out blocking-handler settlement with the
capability supervisor before returning. Accepted work therefore retains the
exact N Use lease until it settles or the bounded close policy expires. A
timed-out callback already running in Tokio's blocking pool cannot be forcibly
cancelled by Rust. If it exceeds the shared scope-close deadline, the close
report is non-clean and the lease can be released while that host callback
finishes; host callbacks must be bounded rather than relying on asynchronous
`Drop` or forced thread termination.

An admitted N Run retains the N Hook definition, N handler, and exact N Use
lease across N+1 publication. Later Runs receive N+1. This cut does not inspect
package manifests, perform SemVer resolution, compute Grants, publish or retire
a Use generation, or replace Use drain and recovery.

### HOST-MCP1 MCP runtime cut

`HOST-MCP1` admits `CapabilityValue::Mcp(Arc<McpBinding>)` through the existing
`SessionCapabilityBatch`. The binding is one immutable runtime atom containing
an exact server name, one initialized `Arc<McpClient>`, and the canonical
`Arc<[McpTool]>` returned by its readiness barrier. Construction rejects an
uninitialized or disconnected client, a mismatched client identity, duplicate
or invalid tool names, more than 1,024 tools, and more than 16 MiB of serialized
definitions. The catalog is sorted once and never refreshed inside an admitted
generation.

Projected wrappers derive their public `mcp__server__tool` definitions from
that frozen catalog and call the raw tool name directly on the exact client.
They never ask `McpManager` to resolve the latest client by server name. A
mutable compatibility reconnect therefore cannot route an N Run through an
N+1 client or pair an N definition with an N+1 caller. Delegated children
receive the same binding `Arc` after compatibility sources are assembled, so
child execution preserves the parent Run generation as well.

```text
A3S Use generation + exact non-clone Run route lease
                         │
                         ▼
          Code immutable capability projection
                         │
                         ▼
     exact client + frozen tools/list ──▶ Run-owned MCP wrappers
              │                                  │
              │                                  └──▶ delegated child wrappers
              │
              └── Code projection effect ──▶ close after final old reader
```

[`McpProjectionAdapter`](../core/src/mcp/projection.rs) performs the Code-owned
readiness sequence: connect one configured transport, complete MCP
initialization, fetch `tools/list`, validate the immutable binding, and return
the value together with a reversible connection effect. Nothing becomes
visible until the complete capability transaction validates and wins its CAS.
Prepare failure, cancellation, a compatibility conflict, or a lost CAS moves
the effect to bounded rollback. Published retirement moves it only after the
final old `CapabilityProjectionLease` drops. Stdio owns a process-group RAII
backstop; HTTP transports synchronously abort their listener task on `Drop`
when an async close cannot run.

This introduces two deliberately different leases. The Code projection lease
pins the exact local client, definitions, caller, and close effect. Every
executing Run separately acquires and retains the non-clone A3S Use
`CapabilitySnapshotLease`, whose canonical route leases govern upstream
admission and drain. N+1 publication can replace the Session-visible Code
binding without closing N while an N Run reads it, and cannot release N's Use
route lease while that Run or an accepted foreground delegated child is
executing. Code cleanup closes the local connection; Use remains authoritative
for route visibility, asynchronous drain, retirement, journals, and recovery.

The adapter input is a trusted host boundary, not a package contract. Before
constructing `McpServerConfig`, the host must already have selected the exact
Use generation and resolved its Runtime/Gateway evidence. Persistent Services
must retain the exact provisioning and readiness evidence required by Use;
opaque `gateway:*` endpoint identities are resolved by the owning
Runtime/Gateway integration, not by Code. Stdio configuration represents the
exact per-connection launcher selected for that generation. Code never scans a
package directory, chooses a provider, derives a command or URL from package
metadata, computes Grants, or reconnects by mutable name after failure.

Compatibility server identities and fully qualified wrapper names share a
fail-closed conflict boundary with projected MCP. A configured compatibility
server or wrapper blocks publication, and later live add/remove or Tool
registration cannot masquerade as mutation of the projected value.
`mcp_status` includes the current projected binding as a diagnostic snapshot;
it is not call routing authority.

The deterministic evidence in
[`mcp_projection`](../core/src/agent_api/capability_runtime_tests/mcp_projection.rs),
[`McpBinding` tests](../core/src/mcp/binding.rs), and
[`TaskExecutor` tests](../core/src/tools/task/tests.rs) covers exact raw calls,
catalog bounds, N/N+1 definition and client isolation, separate Use lease
retention, final-reader retirement, compatibility conflicts, stdio rollback
without an orphan process, and delegated child inheritance.

This Core gate does not claim that the authoritative A3S Use capability
projection already emits every MCP surface into Code or that official hosts
have adopted this adapter. That upstream projection and host wiring remain a
separate integration gate. Extending it must preserve Use-owned generation,
Runtime/Gateway, route-lease, drain, and recovery evidence rather than teaching
Code to infer missing lifecycle state.

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
| `CAP-I04` | A Run's model definition and execution resolve through the same pinned capability value; delegated Agent definitions and lookup share one Run-frozen registry, slash-command selection and execution share another, Hook metadata and handlers remain one exact binding with supervised observations, MCP definitions and raw calls share one exact client binding, and a presentation Profile can only remove Tool definitions or rephrase the existing code gateway. |
| `CAP-I05` | External hot-plug affects the next admitted Run; an admitted Run retains its exact local and upstream generation leases. |
| `CAP-I06` | A child scope cannot broaden its parent's permission, confirmation, security, budget, workspace, Tool-presentation, or execution ceiling. |
| `CAP-I07` | Required dependencies must be ready in the same upstream generation before a contribution becomes visible. |
| `CAP-I08` | Built-in capabilities cannot be replaced or removed by an external source. |
| `CAP-I09` | Steady-state Run execution performs no global catalog write and does not resolve through a mutable latest-value registry. |
| `CAP-I10` | Teardown is reverse-order, cancellation-safe, idempotent, bounded, and observable. |
| `CAP-I11` | Canonical capability identity and digest are deterministic across insertion order and supported platforms. |
| `CAP-I12` | SDKs expose stable descriptors and explicit registration methods, not Rust heterogeneous storage or package-manager authority. |

## Incremental module boundary

The implementation belongs under `core/src/capability/`, split by concern:

```text
capability/
├── id.rs
├── descriptor.rs
├── value.rs
├── set.rs
├── readiness.rs
├── runtime.rs
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
| `CAP-SET1` | Delivered | Typed Use package/cursor and Code catalog generations, sealed source classes, complete source-owned descriptor batches, and a bounded immutable `CapabilitySet` | `BTreeMap` ordering plus a domain-separated golden digest is insertion-order independent; mixed Use cursors, conflicts, missing edges, forged Built-in precedence, and every configured bound fail before an `Arc` can escape |
| `CAP-SCOPE1` | Delivered | Session/Run/Turn/Subtask markers, catalog-bound ceilings, borrowed leases, reversible effects, exact Use Run leases, and a structured-concurrency supervisor | Compile-fail and runtime tests prevent lease escape or child expansion; close is reverse-order, cancellation-safe, idempotent, bounded, and releases the Use lease last |
| `CAP-PROJ1` | Delivered | Closed typed runtime values, immutable projected catalogs, typestate contribution transactions, generation/digest CAS publication, and final-lease retirement | Failed prepare, validation, cancellation, dropped transaction, and commit-race paths leave the current generation unchanged and retain every prepared effect for reverse cleanup |
| `CAP-DEP1` | Delivered | Bounded surface readiness DAG | Only published surface edges are ordered; Code does not resolve packages or become general DI |
| `HOST-CAP1` | Delivered | Core, CLI, and Desktop use one atomic Tool/Skill projection per Session or one-shot execution | Old Runs retain N and its exact Use lease, new Runs see N+1, failed preparation never advances the generation, one-shot watchers stop before Run admission, and Desktop requires exact Code/Use evidence |
| `CAP-PROFILE1` | Delivered | Run-frozen typed Tool presentation over the same pinned executor values | Permission filtering precedes Profile projection; name/schema identity and deterministic order are preserved, code mode rephrases only the existing `program` definition, child runs cannot broaden, and exact Session resume plus Rust/Node.js/Python/Go parity pass |
| `HOST-AGENT1` | Delivered | Core projects Agent definitions into one Run-frozen registry shared by automatic and Tool-driven delegation | Canonical alias conflicts fail before publication, compatibility registration cannot shadow a published Agent, N Runs delegate through N after an N+1 cutover, and the exact N Use lease remains held through foreground child completion |
| `HOST-COMMAND1` | Delivered | Core dispatches blocking and streaming slash Commands through one Run-frozen registry | Built-in and compatibility conflicts fail before publication, legacy registration cannot shadow a published Command, N execution remains on N after an N+1 cutover, and the exact N Use lease remains held through Command execution |
| `HOST-HOOK1` | Delivered | Core composes projected Hook bindings through one Run-frozen executor | Definition/handler pairs remain generation-exact, invalid Run event scopes and compatibility conflicts fail before publication, external `Skip` cannot bypass projected policy, and supervised observations retain the exact Use lease through bounded settlement |
| `HOST-MCP1` | Delivered | Core projects each MCP server as one immutable exact-client binding and freezes its wrappers per Run | Initialization and `tools/list` finish before publication; N definitions, raw calls, foreground delegated children, and the parent Run's N Use lease remain generation-exact across N+1; rollback and final-reader retirement close the Code-owned connection effect without mutable-manager fallback |
| `CAP-GA1` | Planned | Legacy shadow ownership and piecemeal reconciliation removed after one major compatibility period | Official hosts and SDKs use the scoped architecture and the complete verification matrix passes |

`CAP-PROJ1` attaches runtime values and atomic typestate transactions to the
identity and scope kernels. `CAP-DEP1` now orders only their published surface
edges through a bounded generation-bound readiness plan. Delivered
`HOST-CAP1` hosts Tool and Skill projections atomically in Core, the resident
CLI, one-shot Code Exec, and Desktop. `HOST-AGENT1` adds Core Agent projection
and delegation to that same transaction and Run lease. `HOST-COMMAND1` adds
blocking and streaming slash-command dispatch to that admission boundary.
`HOST-HOOK1` adds generation-exact Hook bindings, composed policy, and
supervised observations to that same Run. `HOST-MCP1` adds exact-client MCP
bindings, reversible connection effects, and delegated inheritance without
moving Use route or package lifecycle into Code. The authoritative Use MCP
projection and official-host adoption remain separate integration work; Flow,
Knowledge, Context, and UI migrate next. Host integration must use the atomic
Core transaction instead of adding another reconciliation abstraction.

`CAP-PROFILE1` operates only after that atomic admission boundary. It projects
the Run-frozen permission-visible definition list, never the Session-latest
registry, and preserves the Tool name/schema identity used by the same pinned
executor. Profile selection therefore cannot advance or weaken the A3S Use or
Code catalog generation.

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
