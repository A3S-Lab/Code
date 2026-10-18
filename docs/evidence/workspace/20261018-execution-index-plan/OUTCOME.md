# Execution and workspace-index planning evidence

## Result

Planning only. No production change or optimization is claimed.
Source baseline: Code `f0977158` in an isolated Code git worktree.
Compiler: Rust 1.97.1. Host: macOS ARM64.

The following exact, individually executed test failed as expected:

```sh
CARGO_TARGET_DIR=/tmp/a3s-idx-target CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
rustup run 1.97.1 cargo test -p a3s-code-core --lib \
workspace::manifest::tests::grep_keeps_the_runtime_available_while_a_candidate_provider_is_slow \
-- --exact --nocapture
```

Result: 0 passed, 1 failed, 0 ignored; 0.39 s test execution.
Failure: `search blocked the runtime: timers and cancellation could not run`.
This test combines a real manifest search with a bounded slow candidate provider
and an independent timer on the same current-thread runtime. The matching
result is correct, but the timer cannot execute during the synchronous search.
This proves why a timeout alone cannot fix that execution path.

The temporary test source was restored after diagnosis. Reapply
[reproduction.patch](reproduction.patch) to this exact baseline to reproduce;
[red-test.log](red-test.log) retains compilation warnings and the complete result.
The warnings predate any production change and were not silently fixed.

The observed user session also showed search calls of approximately 11m54s,
5m19s and 7m27s. A live process sample located the hot path in
`ManifestWorkspaceBackend::ensure_auto_trigram_candidate_index` →
`TrigramGrepCandidateIndex::build_from_files` → `unique_trigrams`.
These are diagnostic observations, not controlled performance benchmarks.
The user's conversation and process dump are not copied into this evidence.

Further live-session diagnosis identified the direct completion failure:
`search_skills` returned `outcome=succeeded`, `exit_code=0`, with
`effects.completeness=incomplete` and reason
`tool_effect_observation_incomplete: workspace read or observation budget failed`.
The Bash refusal in the same Run had `outcome=denied`, no exit code and complete
empty effects. Subsequent directory calls also had complete effects.
The resulting completion gate named one unresolved invocation.

Current source inspection confirms `SearchSkillsTool` does not override
`Tool::capabilities`; the default is `read_only=false`. `prepare_tools` therefore
starts workspace observation for it. The permission assessment separately
classifies `search_skills` as read-only. This disagreement is R0 in the plan.
No claim is made that the full failure has already been independently reproduced
by an isolated contract test or repaired; those are R0's required gates.

Sandbox source `0f2bb18d` had a successful release qualification
[CI 39087183495](https://github.com/A3S-Lab/Sandbox/actions/runs/39087183495).
The subsequent same-source
[CI 39088099628](https://github.com/A3S-Lab/Sandbox/actions/runs/39088099628)
failed its isolated macOS CONNECT test with `PermissionError: Operation not permitted`.
That independent failure remains open; a successful release run does not prove
repeatability.

## What landed

- [Implementation and acceptance plan](../../../architecture/execution-and-indexing-plan.md).
- A planned ROADMAP gate, with the original Sandbox lifecycle requirements preserved
  and the observed false tool-effect debt promoted to the first repair.
- A reproducible liveness failure and an explicit cross-repository delivery boundary.

## Explicitly not claimed

- No source implementation, release, dependency upgrade, installed-app test,
  performance improvement or native Bash unblocking.
- No full suite, strict Clippy, Windows/Linux reproduction, or model-reliability result.
- No zero-impact CPU/I/O guarantee; the plan requires measured interference limits.
- No complete Sandbox S4/S5/S6 or Guest product delivery.
