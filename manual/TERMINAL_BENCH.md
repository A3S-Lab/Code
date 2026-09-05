# Terminal-Bench evaluation

The native adapter runs A3S Code's existing Core session and workspace tools
inside Harbor's task container. Harbor remains responsible for the task image,
instruction delivery, time limits, artifacts, and verifier. The adapter does
not provide MCP tools, alter a task, install a solution, or replace the
verifier.

The benchmark task instruction is uploaded byte-for-byte to
`/run/a3s/instruction.md`. The runner uses the general writable agent style and
disables planning pre-analysis, so the model receives that instruction as its
single user message. This prevents task words such as `findall` from selecting
the read-only Explore style and avoids a planner-generated rewrite. Search
mode remains a model decision through the normal A3S Code tool descriptions.

Build the static runner from the Code crate:

```bash
cargo zigbuild --locked --target aarch64-unknown-linux-musl \
  --release --no-default-features --example terminal_bench_runner
```

Run an official Terminal-Bench 4.0 task with the local Codex login:

```bash
A3S_CODE_TERMINAL_BENCH_BINARY="$PWD/target/aarch64-unknown-linux-musl/release/examples/terminal_bench_runner" \
A3S_CODE_CONFIG="$PWD/../../.a3s/config.acl" \
A3S_CODEX_AUTH_FILE="$HOME/.codex/auth.json" \
A3S_CODEX_MODEL=gpt-6-astra \
A3S_CODEX_REASONING_EFFORT=max \
PYTHONPATH="$PWD/scripts/terminal_bench" \
harbor run -d terminal-bench/terminal-bench@4.0.0 \
  -a a3s_code_agent:A3SCodeAgent \
  -t terminal-bench/<task> -n 1 -k 1 -y
```

Use the official Terminal-Bench command and dataset version for leaderboard
work. A single task is a diagnostic result, not a benchmark score. Report the
Harbor aggregate and each trial's native `verifier_result`; a timeout, runner
error, or missing verifier result is not a pass. Keep the Harbor job directory
as the reproducible evidence bundle. The auth file is uploaded only to the
ephemeral task container and is never written to the workspace or trajectory.
