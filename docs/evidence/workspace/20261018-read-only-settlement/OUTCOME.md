# Read-only tool settlement qualification

## Result

The R0 production change declares the existing workspace read-only behavior of
`search_skills` and `generate_object`. This is a local, isolated-worktree result,
not a release or completion of the execution/index optimization plan.

Base: Code `f0977158`. Host: macOS ARM64. Toolchain: Rust 1.97.1.
The exact changed-source and retained-log hashes are in
[source-identities.json](source-identities.json).
Retained logs remove trailing whitespace and redundant final blank lines;
the original `/tmp` output hashes are recorded separately.
The test provider is deterministic; the session, tools, filesystem, fact log,
receipts and completion gate are real. No live-model result is claimed.

| Check | Result | Retained output |
| --- | --- | --- |
| Regressions with both production files restored to HEAD | 0 passed / 3 failed; both sessions reproduce `tool_effect_unresolved`; the lookup with earlier write debt is denied | [baseline-contracts.log](baseline-contracts.log) |
| Fixed affected families | 139 passed / 0 failed / 0 ignored | [fixed-family.log](fixed-family.log) |
| Production size and unwrap ratchets | 4 passed / 0 failed | [ratchets.log](ratchets.log) |
| Affected lib test target compilation | `cargo test -p a3s-code-core --lib --no-run` succeeds; new sibling suites are compiled and registered | [test-compile.log](test-compile.log) |
| Changed-file Rust formatting | All six changed/new Rust files pass `rustfmt --check` | Command below |
| Whole-tree Rust formatting | Fails on two unchanged baseline files: import ordering in `workspace/mod.rs`, empty array formatting in `production_file_size_ratchet.rs` | [whole-format.log](whole-format.log) |
| Strict Core Clippy, all targets | 48 unique diagnostics both before and after the fix, with zero new diagnostic/location pairs | [baseline-clippy.log](baseline-clippy.log), [fixed-clippy.log](fixed-clippy.log), [diagnostics-comparison.txt](diagnostics-comparison.txt) |

The original two regressions also failed independently. After finding the same
missing declaration on structured generation, that session regression failed
independently too. Finally, both production files were temporarily restored to
HEAD together: all three final regressions failed. A `finally` block restored the
fixed sources before the final compile and family run.

The large-file fixture initializes a real isolated Git repository with the
existing hardened Git helper and creates a sparse untracked file larger than
the 64 MiB observation budget. It proves its baseline is incomplete before the
tool call. The sessions return actual skill metadata / schema-validated JSON,
and assert complete, empty effects on identity-bound durable receipts. The
missing-directory read remains a settled tool failure rather than effect debt.

Commands use this environment:

```sh
export CARGO_TARGET_DIR=/tmp/a3s-idx-target
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0
```

Regression and family commands:

```sh
rustup run 1.97.1 cargo test -p a3s-code-core --lib -- \
  read_only_settlement skill_lookup_remains --nocapture
rustup run 1.97.1 cargo test -p a3s-code-core --lib --no-run
/tmp/a3s-idx-target/debug/deps/a3s_code_core-72a400686c50f27a \
  tools::skill:: fact_control::receipts:: agent_api::tests::fact_log:: \
  tools::builtin::generate_object --test-threads=4 --nocapture
rustup run 1.97.1 cargo clippy -p a3s-code-core --all-targets --locked -- -D warnings
rustup run 1.97.1 cargo fmt --all -- --check
rustup run 1.97.1 cargo test -p a3s-code-core \
  --test production_unwrap_ratchet --test production_file_size_ratchet -- --nocapture
rustup run 1.97.1 rustfmt --edition 2021 --check \
  core/src/tools/skill.rs core/src/tools/builtin/generate_object.rs \
  core/src/agent_api/tests/fact_log.rs \
  core/src/agent_api/tests/fact_log/read_only_settlement.rs \
  core/src/fact_control/receipts/tests.rs \
  core/src/fact_control/receipts/tests/skill_lookup.rs
```

The executable path is the one emitted by this host's `--no-run`; on another
host use its emitted executable, or the equivalent Cargo filters. The final
family run follows the final compile without source mutations.

## What landed

- `SearchSkillsTool` declares read-only, serial execution through the existing
  helper. Its live catalog refresh only changes private in-memory registry state.
- `GenerateObjectTool` declares workspace read-only while preserving its
  conservative non-idempotent, non-resumable, serial scheduling properties.
  Existing model admission, budget, timeout and cancellation remain in force.
- A real-session regression covers found and absent skills, a missing directory,
  and complete receipts in a workspace whose byte observation is incomplete.
- A second real-session regression verifies the generated schema-bound object
  and successful settlement under the same workspace condition.
- A receipt contract proves lookup can continue with an earlier unknown write,
  while the completion gate still refuses to discharge that original write.
- The 139-test family includes actual Skill writes, child host permission
  boundaries, pre-spawn refusals, post-dispatch failures and cancellation.
- The original optimization plan and ROADMAP track the remaining delivery gates.

## Explicitly not claimed

- Strict Clippy or whole-tree formatting passing: baseline failures are retained,
  not suppressed, repaired incidentally or treated as a green gate.
- No actual terminal app restart, consumer deployment, publication, live-model
  reliability result, or installed-app end-to-end qualification.
- No automatic removal of old-session effect debt. It still requires evidence
  bound to the original invocation; a later read cannot erase it.
- No general read-only declaration for execution-capable `Skill`, third-party
  tools or external writes. Their conservative treatment remains unchanged.
- No macOS Bash unblocking, qualified Guest networking, indexing optimization,
  or completion of Sandbox S4/S5/S6.
