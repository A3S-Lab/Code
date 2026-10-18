# Native capability consumption and early host availability

## Result

The first E1 slice is locally verified in the isolated Code worktree. Base:
`ac87557f`. Host: macOS ARM64. Toolchain: Rust 1.97.1. This is not a qualified
product release or completion of E1 and the broader execution/indexing plan.

## Changes and boundaries

- Core consumes Sandbox `=0.3.0` at the immutable public release commit
  `0f2bb18dc4f16a1ebe12bc282db5c1499ad5fe05`. The root lock changes only the
  Sandbox package identity; Cargo also normalizes several dependency edges to
  other already-locked versions. No unrelated root package version is upgraded.
- `NativeBashSandbox` exposes advertised and probed typed reports. The existing
  `probe() -> Result<()>` API is preserved. Foreground/background admission
  requires both declared process-tree and host-exit cleanup, replacing the
  operating-system-name decision. It does not yet require or cache a successful
  runtime launcher probe; that E1 obligation remains open.
- The private adapter-bound refusal retains its typed capability cause.
  Only that refusal or the existing exact Guest refusal proves non-execution;
  public reason enums, matching text and execution failures cannot be substituted
  for that proof. Bash/background retain conservative post-dispatch settlement.
- macOS keeps its compatibility refusal text. Windows remains unavailable while
  Sandbox declares host-exit cleanup unqualified; this is intentionally a release
  hold until S4 is completed, not a claimed supported Windows execution result.
- A session-owned context provider reads the host's effect-free availability
  check on every query. A bounded refusal is required prompt context, independent
  of retrieval relevance/budget. It starts no process and changes no permission.
  A real fact-log session with a deterministic client proves the first model
  request sees the reason and a later ready state replaces the stale warning.
- Node/Python manifests no longer override Core's declared dependencies with
  monorepo source directories. Their locks now match Core for Effect, Memory,
  Sandbox and Search; the SDK Search version aligns with Core's existing 3.4.0
  declaration. This removes source drift rather than selecting new latest releases.
- Node's default build exposed a per-method `cfg`/N-API registration error.
  Moving that feature gate to its own complete impl preserves the evidence API
  under `advanced-harness` and removes the dangling registration in default builds.

## Checks

| Check | Result | Retained output |
| --- | --- | --- |
| First model request before the context fix | 0 passed / 1 failed, including SOLO | [baseline](context-baseline.log), [SOLO](context-baseline-solo.log) |
| Final five added contracts | 5 passed / 0 failed | [fixed-contracts.log](fixed-contracts.log) |
| Expanded affected families | 479 passed / 0 failed / 1 pre-existing ignored routing soak | [fixed-family.log](fixed-family.log) |
| Ignored routing soak, explicitly selected | 1 passed / 0 failed / 0 ignored | [routing-soak.log](routing-soak.log) |
| Initial fixture and context-count failures | Retained, reproduced SOLO and corrected without weakening native early-refusal guards | [initial contracts](initial-contracts.log), [probe SOLO](probe-fixture-solo.log), [initial family](initial-family.log), [context SOLO](context-sections-solo.log), [fixed context](context-sections-fixed.log) |
| Core lib test target | Final `--no-run --locked` succeeds and registers new sibling suites | [lib-test-compile.log](lib-test-compile.log) |
| Node SDK default | Initial macro error, then success after whole-impl feature gate | [initial](sdk-node-initial-check.log), [fixed](sdk-node-check.log) |
| Node SDK advanced-harness | `cargo check --lib --features advanced-harness --locked` succeeds | [sdk-node-advanced-check.log](sdk-node-advanced-check.log) |
| Python SDK, Python 3.12 | `cargo check --lib --locked` succeeds | [sdk-python-check.log](sdk-python-check.log) |
| Shared dependency source identity | Core/Node/Python resolve identical declared Effect/Memory/Sandbox/Search sources | [dependency-sources.json](dependency-sources.json), [Core relock](relock-core.log), [Node metadata](sdk-node-metadata.log), [Python metadata](sdk-python-metadata.log) |
| Size / unwrap ratchets | 4 passed / 0 failed | [size](size-ratchet.log), [unwrap](unwrap-ratchet.log) |
| Changed Rust-file formatting | All 11 changed/new Rust files pass | `rustfmt --check --config skip_children=true`, source hashes below |
| Whole-tree formatting | Same two unchanged baseline-file failures | [whole-format.log](whole-format.log) |
| Strict Core Clippy | Same 48 unique baseline diagnostic/location pairs; zero introduced | [strict-clippy.log](strict-clippy.log), [comparison](diagnostics-comparison.txt), [baseline](../20261018-read-only-settlement/baseline-clippy.log) |
| Standalone CI bootstrap, disposable copies | Baseline and candidate fail identically before build | [setup baseline](ci-setup-baseline.log), [candidate](ci-setup-candidate.log), [relock baseline](ci-relock-baseline.log), [candidate](ci-relock-candidate.log) |
| Official landing checker | Existing monorepo-prefix failure | [landing-check.log](landing-check.log) |

The original first-request regression fails at the prior 0.2.3 production
baseline, then fails independently SOLO. The first fixed group passed four
contracts but failed an invalid fixture assertion that a successful probe left
an empty workspace. SOLO and the standalone Sandbox CLI confirmed `.a3s/loops`
creation. The final contract instead executes a sentinel-writing user command
after the successful probe, requires typed refusal and verifies no sentinel.
The original early-refusal zero-process/zero-output tests remain in the family.

The first expanded family also exposed an old assumption that context had only
one total provider. Its SOLO failure is retained. The test now requires one
section for the named injected provider, stable digests for all unchanged
sources, and a changed digest for the edited source; the new host source remains
included. It no longer pins the total provider count or provider array position.

The routing soak uses supplied sandbox implementations: 100 successful dispatch
routes and 20 initialization failures leave the real host-write sentinel absent.
This verifies Code routing, not OS isolation or native lifetime qualification.
The four size/unwrap ratchets reuse previously compiled, unchanged executables
whose support walker reads the current source graph, including the new provider.
No fresh ratchet-target compilation is claimed.

The baseline/current CI setup and relock checks run only in disposable manifest
copies, so they do not mutate the owned worktree or another session. Both versions
fail identically: setup requires absent monorepo patch tables; relock requires an
old Effect Git declaration. The official landing checker separately still assumes
an `apps/code` prefix absent from this standalone repository. These existing
release gates remain open, not bypassed or presented as passing.

## Reproduction and identity

All Rust commands use:

```sh
export CARGO_TARGET_DIR=/tmp/a3s-idx-target
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0
rustup run 1.97.1 cargo test -p a3s-code-core --lib --no-run --locked
rustup run 1.97.1 cargo clippy -p a3s-code-core --all-targets --locked -- -D warnings
rustup run 1.97.1 cargo check --manifest-path sdk/node/Cargo.toml --lib --locked
rustup run 1.97.1 cargo check --manifest-path sdk/node/Cargo.toml --lib --features advanced-harness --locked
PYO3_PYTHON=/Users/roylin/.local/bin/python3.12 rustup run 1.97.1 cargo check \
  --manifest-path sdk/python/Cargo.toml --lib --locked
```

The final `--no-run` emits the observed Core executable below; use the locally
emitted executable on another host. The final family command is:

```sh
/tmp/a3s-idx-target/debug/deps/a3s_code_core-869d46ff8ed07805 \
  sandbox::native:: sandbox::guest::fail_closed_tests:: \
  agent_api::session_sandbox::tests:: context:: tools::builtin::bash:: \
  tools::builtin::background:: fact_control::receipts:: \
  agent_api::tests::fact_log:: tools::skill:: tools::builtin::generate_object \
  workspace::manifest:: workspace::local:: workspace::grep_candidates:: \
  tools::builtin::grep:: tools::builtin::search:: --test-threads=4 --nocapture
/tmp/a3s-idx-target/debug/deps/a3s_code_core-869d46ff8ed07805 \
  tools::builtin::bash::tests::sandbox_soak::soak_sandboxed_bash_does_not_fall_back_to_the_host \
  --exact --ignored --nocapture
```

Cargo metadata for the root and both SDK manifests proves the actual Core
resolution, rather than just version labels or manifest text. The retained
[dependency-sources.json](dependency-sources.json) records the four shared
package sources, the clean tracked Sandbox Git checkout, exact release revision
and library hash. [source-identities.json](source-identities.json) records source,
manifest, lock and log hashes. Retained logs remove trailing whitespace and
redundant final blank lines; original output hashes are retained separately.

## Explicitly not claimed

- No complete E1: runtime-probe admission/cache/invalidation, structured ACP/TUI
  startup diagnostics, catalog readiness projection and real-model retry behavior
  remain open. The context provider reads an existing pure admission check; it
  does not initiate or cache the launcher's process-producing probe.
- No zero-effect launcher probe: Sandbox creates `.a3s/loops`. A successful probe
  reports launcher availability and cannot waive absent lifetime capabilities.
- No Windows/Linux native execution or new native lifecycle qualification on
  this macOS host. Windows S4 cross-host ownership and durable recovery remain
  required before its host-exit capability can become true.
- No macOS Bash unblocking, packaged Guest networking, installed-app or live-model
  E2E, native Node/Python module loading/runtime qualification, publication, merge
  or replacement/restart of the user's running application.
- No green strict Clippy, full formatting, standalone CI/bootstrap or official
  landing gate. No suppression, hook bypass or unrelated baseline cleanup.
- No full Code suite, measured performance SLO, complete indexing redesign,
  old-session debt clearance or Sandbox S4/S5/S6 completion.
