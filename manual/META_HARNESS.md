# Meta Harness: Composable Harness Components

A3S Code is a **Meta Harness**: hosts compose coding behavior as a Moore
component tree over one immutable fact log. Governance (permission projection
and completion-gate enrollment) stays Core-owned and non-bypassable.

Related: [HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md),
[ADR-003](../../use/docs/adr-003-native-code-harness-boundary.md) (addendum),
`a3s-effect` `compose` module.

## Positioning

```text
{ view, transitions } = f(log)
```

Hosts mount stock or custom components. The runtime folds the log, enables
keyed transitions, and runs them until nothing remains. Dual *imperative*
message loops are forbidden; **one log + many components** is the intended
orchestration model.

Product default remains today's `coding_actor` sugar so existing Agent /
Session behavior does not regress when `SessionOptions.harness` is omitted.

## Invariants

1. Single fact log is the only control source (`ingest` / `resume`).
2. Components are Moore machines: `initial` / `step` / `output` → view + keyed
   transitions (`a3s-effect` actor).
3. Transition keys are unique across the tree (`DuplicateTransition`); finished
   keys are not re-run.
4. Confirm / question park until answer facts (no timers).
5. Kernel wrappers always apply: permission strip before the model catalog;
   mutating success still requires the completion gate / host waiver bound to
   an effect digest. Host components cannot disable these.

## Stock components

| Part id | Role |
| --- | --- |
| `system` | System-prompt slot |
| `tools` | Tool-catalog slot |
| `budget` | Tool-call budget policy |
| `compact` | Compaction threshold slot |
| `infer` | Scheduler subtree (stock mount has **no** key prefix for log stability) |

Nested host `infer([...])` mounts **do** namespace child keys under `infer/` so
siblings cannot collide. CI / unit tests assert that discipline.

## Composition cookbook

### Rust (`a3s-effect`)

```rust
use a3s_effect::{budget, coding_actor, compact, compose_coding_actor, system, tools, HarnessGraph};

// Sugar for the stock tree (bit-compatible default):
let actor = coding_actor(config);

// Explicit graph from a MetaHarnessSpec / HarnessGraph:
let graph = HarnessGraph::coding(config);
```

### SessionOptions (Core / SDKs)

Omit `harness` → legacy `coding_actor`.

```js
// Node
agent.session('.', {
  harness: Harness.compose({
    parts: [Harness.system(), Harness.tools(), Harness.budget(), Harness.compact(), Harness.infer()],
    toolBudget: 4,
    system: ['You are a careful coding agent.'],
  }),
});
```

```python
# Python
opts = SessionOptions()
opts.harness = Harness.compose(
    parts=[Harness.system(), Harness.tools(), Harness.budget(), Harness.compact(), Harness.infer()],
    tool_budget=4,
    system=["You are a careful coding agent."],
)
```

Unknown part names fail closed at compose / session conversion.

## Non-goals

- Porting Tardigrade / Effect TS into Code.
- Letting host components forge completion waivers or skip permission projection.
- Restoring `parallel_task` as a second orchestration engine.
- Making `advanced-harness` the library default.
- Embedding DSH / Cordis / a JavaScript Effect runtime.
- Node/Python host-authored Moore components in this cut (`META-HARNESS2`).
  Rust already supports `compose_coding_actor(vec![component(...), ...])`.

## Safety checks

- Default composition == today's `coding_actor` until hosts opt in.
- Kernel middleware must still strip permissions and enroll the completion gate
  even if a malicious host component enables `model.turn` without them.
- Nested `infer` mounts must not advertise un-prefixed child transition keys.
