# Manifest grep responsiveness

## Result

The first I1 slice passes local contract and affected-family checks. Delivery
gates remain open. This is not completion of the execution/indexing plan or
a product release.

Base: Code `ed786057`. Host: macOS ARM64. Toolchain: Rust 1.97.1.

## Contract and scope

Manifest-backed grep previously ran candidate selection, automatic trigram
construction, Git dirty ordering and exact file scanning directly in its async
future. The ACP current-thread runtime could not poll unrelated file reads or
the search deadline while a candidate provider was busy.

The same search work now runs on the blocking pool. One owned semaphore permit
per manifest backend stays with the actual worker until it exits, including
when the caller drops or times out. A cancellation token is signalled when the
request future drops; the worker checks it before work, after candidate
selection, before files, while matching lines and while emitting matches.
A cached-index inspection uses `try_lock` so it does not wait behind a build.
Existing file admission, exact regex matching, ordering, oversize reports and
output truncation are preserved.

The two regressions use a real manifest and filesystem with a bounded gated
candidate provider on a current-thread runtime. One reads a real file while
selection is busy and then checks the exact hit path. The other requires a
deadline to return before the provider is released, requires a second query to
stay admitted behind the still-running worker, cancels that queued request,
then releases the provider and checks a later exact search succeeds.

The deterministic gate's two-second emergency release prevents a failed test
from hanging the test process. Its purpose is ordering and runtime progress;
50 ms deadlines are fixture controls, not a performance percentile claim.

The design follows Tokio's documented requirement to bound CPU-heavy blocking
work and its warning that abort cannot stop an already-running blocking task:
[spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html).
Cancellation reuses the existing
[CancellationToken drop guard](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html#method.drop_guard).

## Checks

| Check | Result | Retained output |
| --- | --- | --- |
| Regressions before production changes | 0 passed / 2 failed; file reads and the search deadline both wait for the emergency release | [baseline-regressions.log](baseline-regressions.log) |
| Independent baseline reruns | Each fails SOLO for the same reason | [read](baseline-read-solo.log), [deadline](baseline-deadline-solo.log) |
| Fixed regressions | 2 passed / 0 failed | [fixed-regressions.log](fixed-regressions.log) |
| Manifest, local search, candidate, grep/search tools, skill, receipt and fact-log families | 257 passed / 0 failed / 0 ignored | [fixed-family.log](fixed-family.log) |
| Core lib test target compilation | `--no-run` succeeds | [test-compile.log](test-compile.log) |
| Core lib without default features | `cargo check --lib --no-default-features --locked` succeeds; existing warnings retained | [no-default-features.log](no-default-features.log) |
| Production size / unwrap ratchets | 4 passed / 0 failed | [size](size-ratchet.log), [unwrap](unwrap-ratchet.log) |
| Changed-file formatting | All three changed/new Rust files pass | Command below |
| Whole-tree formatting | Same two unchanged baseline-file failures | [whole-format.log](whole-format.log) |
| Official landing checker | Fails to locate `apps/code/scripts/check_landing_allowlist.txt` in this standalone Code repository; the checker still assumes the monorepo prefix | [landing-check.log](landing-check.log) |
| Strict Core Clippy, all targets | Same 48 unique baseline diagnostic/location pairs; zero introduced | [strict-clippy.log](strict-clippy.log), [comparison](diagnostics-comparison.txt), [baseline](../20261018-read-only-settlement/baseline-clippy.log) |

Exact changed-source and retained-log hashes, original output hashes and command
exit codes are in [source-identities.json](source-identities.json). Retained logs
normalize trailing whitespace and redundant final blank lines. No Rust sources
changed between the green build, family execution and final checks.

The ratchet executables were already compiled in the preceding R0 step. Their
unchanged source and support walker read the current production source graph at
runtime, including the new `manifest/grep.rs`; these runs do not claim a fresh
ratchet-target compilation.

All Cargo commands use:

```sh
export CARGO_TARGET_DIR=/tmp/a3s-idx-target
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0
rustup run 1.97.1 cargo test -p a3s-code-core --lib responsiveness_tests -- --nocapture
rustup run 1.97.1 cargo test -p a3s-code-core --lib --no-run
rustup run 1.97.1 cargo check -p a3s-code-core --lib --no-default-features --locked
rustup run 1.97.1 cargo clippy -p a3s-code-core --all-targets --locked -- -D warnings
rustup run 1.97.1 cargo fmt --all -- --check
rustup run 1.97.1 rustfmt --edition 2021 --check --config skip_children=true \
  core/src/workspace/manifest.rs core/src/workspace/manifest/grep.rs \
  core/src/workspace/manifest/responsiveness_tests.rs
```

The observed test executable was emitted by the green build and confirmed by
`--no-run`. The family command was:

```sh
/tmp/a3s-idx-target/debug/deps/a3s_code_core-72a400686c50f27a \
  workspace::manifest:: workspace::local:: workspace::grep_candidates:: \
  tools::builtin::grep:: tools::builtin::search:: tools::skill:: \
  fact_control::receipts:: agent_api::tests::fact_log:: \
  tools::builtin::generate_object --test-threads=4 --nocapture
/tmp/a3s-idx-target/debug/deps/production_file_size_ratchet-c525e0a47203ec7f --nocapture
/tmp/a3s-idx-target/debug/deps/production_unwrap_ratchet-f8ef58415f74b1e7 --nocapture
```

Use the executables emitted by the local build on another host.

## Explicitly not claimed

- No complete I1, asynchronous incremental trigram build, total query-time or
  memory bound, cross-workspace CPU quota, or measured latency percentile. The
  admission bound is per backend instance, not across independent instances
  pointed at the same workspace.
- The existing trigram constructor and host-supplied synchronous candidate
  providers still do not accept cancellation. A running selection/build or
  blocking filesystem/Git operation must return before this worker can stop.
  The permit remains occupied; no 500 ms physical-worker-exit claim is made.
- The initial-manifest fallback keeps the existing LocalWorkspaceBackend
  cancellation/worker implementation. This slice does not add a worker-held
  semaphore to that fallback or refactor its existing lifecycle.
- No green whole-tree formatting, strict Clippy or official landing checker;
  no merge or release. Code, tests, evidence and the ROADMAP row are checked
  together in the isolated staged change, without claiming that replaces the
  repository-wide landing gate.
- No permission expansion, Bash unblocking, Guest integration, app restart,
  installed-app E2E, live-model result, publication, or Sandbox S4/S5/S6 closure.
- No clearing of original invocation-bound write debt, and no claim that the
  user's already-running old binary contains either local fix.
