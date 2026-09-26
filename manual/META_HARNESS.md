# Meta Harness: Composable Harness Components

A3S Code is a **Meta Harness**: hosts compose coding behavior as a Moore
component tree over one immutable fact log. Governance (permission projection
and completion-gate enrollment) stays Core-owned and non-bypassable.

Related: [HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md),
[ADR-003](../../use/docs/adr-003-native-code-harness-boundary.md) (addendum),
`a3s-effect` `compose` module, first-principles gate **F31**.

## Positioning

```text
{ view, transitions } = f(log)
```

Hosts mount ordered components via `components: [...]`.
The runtime folds the log, enables keyed transitions, and runs them until
nothing remains. Dual *imperative* message loops are forbidden; **one log +
many components** is the intended orchestration model.

Product default remains today's `coding_actor` sugar so existing Agent /
Session behavior does not regress when `SessionOptions.harness` is omitted.

## Invariants

1. Single fact log is the only control source (`ingest` / `resume`).
2. Components are Moore machines: `initial` / `step` / `output` → view + keyed
   transitions (`a3s-effect` actor).
3. Transition keys are unique across the tree (`DuplicateTransition`); finished
   keys are not re-run.
4. Confirm / question park until answer facts (no timers).
5. Kernel wrappers always apply: the permission policy decides every tool call
   before it executes;
   mutating success still requires the completion gate / host waiver bound to
   an effect digest. Host components cannot disable these. A host may install
   `SessionOptions::with_completion_attestor` to supply a Passed, digest-bound
   report once the live digest exists — that is evidence, not an `Observe`
   bypass (#160).

## Stock components

| Part id | Role |
| --- | --- |
| `system` | System-prompt slot |
| `tools` | Tool-catalog slot |
| `budget` | Tool-call budget policy |
| `compact` | Compaction threshold slot |
| `infer` | Scheduler subtree (stock mount has **no** key prefix for log stability) |

## Host components (`host:<id>`)

Hosts register Moore factories via `HostHarnessRegistry` and mount them from
the same `components: [...]` list as `host:<id>`. Builtin registry ships
`intent_stamp` (injects `a3s.meta_harness.intent_stamp.v1` into the system
view) for hermetic / Layer C proofs. Full custom trees use
`HostHarnessAssembler` / `admit_component_tree` (Rust embedders).

SDK surfaces pass **ids + config**, not JS/Python Effect runtimes.

Nested host `infer([...])` mounts **do** namespace child keys under `infer/` so
siblings cannot collide. CI / unit tests assert that discipline.

## Composition cookbook

### Rust (`a3s-effect` + Core)

```rust
use a3s_effect::{budget, coding_actor, compact, compose_coding_actor, system, tools, HarnessGraph};
use a3s_code_core::{admit_component_tree, BuiltinHostHarnessRegistry, HarnessComposeOptions};

// Sugar for the stock tree (bit-compatible default):
let actor = coding_actor(config);

// Ordered assemble (stock + host):
let recipe = HarnessComposeOptions::compose(
    vec![
        "system".into(),
        "tools".into(),
        "host:intent_stamp".into(),
        "budget".into(),
        "infer".into(),
    ],
    Some(4),
    None,
    vec!["careful coding agent".into()],
)?;
```

### SessionOptions (Core / SDKs)

Omit `harness` → legacy `coding_actor`.

```js
// Node — prefer `components`
agent.session('.', {
  harness: Harness.compose({
    components: [
      Harness.system(),
      Harness.tools(),
      Harness.host('intent_stamp'),
      Harness.budget(),
      Harness.infer(),
    ],
    toolBudget: 4,
    system: ['You are a careful coding agent.'],
  }),
});
```

```python
# Python
opts = SessionOptions()
opts.harness = Harness.compose(
    components=[
        Harness.system(),
        Harness.tools(),
        Harness.host("intent_stamp"),
        Harness.budget(),
        Harness.infer(),
    ],
    tool_budget=4,
    system=["You are a careful coding agent."],
)
```

```go
// Go
budget := uint32(4)
opts := &code.SessionOptions{
	Harness: &code.HarnessOptions{
		Components: []string{
			code.HarnessSystem, code.HarnessTools, code.HarnessHost("intent_stamp"),
			code.HarnessBudget, code.HarnessInfer,
		},
		ToolBudget: &budget,
		System:     []string{"You are a careful coding agent."},
	},
}
```

Unknown part names fail closed when the session is created; unknown `host:<id>`
values fail closed on the first run. Rust embedders supply
`SessionOptions::with_host_harness_registry`. Node, Python, and Go sessions that
set `harness` install `BuiltinHostHarnessRegistry`, so SDK `host:<id>` mounts
resolve against Core's builtin components (currently `intent_stamp`); custom
host components need a Rust embedder.

## Verification

| Layer | Gate |
| --- | --- |
| Hermetic | `cargo test -p a3s-code-core --lib meta_harness` |
| F-kernel ≥95% | `meta_harness.rs` + `completion_attestor.rs` in `scripts/f_table_coverage.sh` (F31) |
| Layer C live | `test_meta_harness_compose_live_e2e` (file/digest/gate oracles only) |

F31 tip evidence (2026-09-25): `A3S_F_TABLE_EVIDENCE=/tmp/a3s-f31-meta-harness
A3S_F_TABLE_MIN_LINE_PCT=95 scripts/f_table_coverage.sh` →
`ALL_F_TABLE_KERNELS_GE_95` with `meta_harness.rs` **99.20%** and
`completion_attestor.rs` **95.00%**. Hermetic suites cover stock/host partition,
fail-closed unknown mounts, default/parts/components resolve paths, and mixed
trees including `compact` — not golden assistant prose.

## Non-goals

- Porting Effect TS into Code.
- Letting host components forge completion waivers or skip permission projection.
- Restoring `parallel_task` as a second orchestration engine.
- Making `advanced-harness` the library default.
- Embedding DSH / Cordis / a JavaScript Effect runtime.
- Letting Node/Python author Moore `step`/`output` closures inside the SDK
  (host factories stay Rust-side; SDKs pass `host:<id>` only).

## Safety checks

- Default composition == today's `coding_actor` until hosts opt in.
- Kernel middleware must still enforce the permission policy and enroll the completion gate
  even if a malicious host component enables `model.turn` without them.
- Nested `infer` mounts must not advertise un-prefixed child transition keys.
- Live suites must not assert provider-specific assistant prose.
