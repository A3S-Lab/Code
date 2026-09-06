# Terminal-Bench evaluation

The native adapter runs A3S Code's existing Core session and workspace tools
inside Harbor's task container. Harbor remains responsible for the task image,
instruction delivery, time limits, artifacts, and verifier. The adapter does
not provide MCP tools, alter a task, install a solution, or replace the
verifier.

The benchmark task instruction is uploaded byte-for-byte to
`/run/a3s/instruction.md`. The runner explicitly pins the general writable
agent style and disables planning pre-analysis, so the model receives that
instruction as its single user message. The explicit style takes precedence
over task vocabulary (for example, `design` or `findall`) and therefore cannot
silently route the writable session to a read-only style. Search mode remains a
model decision through the normal A3S Code tool descriptions.

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

For a leaderboard-compatible run, use the complete tagged dataset, five
attempts per task, and a GPU-capable sandbox as required by Terminal-Bench:

```bash
harbor run -d terminal-bench/terminal-bench@4.0.0 \
  -a a3s_code_agent:A3SCodeAgent \
  -n 100 -k 5 -y
```

The exact `-n` value depends on the provider and available quota. A local
Docker run without GPU resources or a single `-t` task is a diagnostic result,
not a leaderboard score. Report the Harbor aggregate and each trial's native
`verifier_result`; a timeout, runner error, or missing verifier result is not a
pass. Keep the Harbor job directory as the reproducible evidence bundle. The
auth file is uploaded only to the ephemeral task container and is never written
to the workspace or trajectory.

The runner writes an atomic, machine-readable terminal report to
`/logs/agent/a3s-code.result.json`. It records the outcome (`succeeded`,
`failed`, or `timed_out`), the terminal reason, whether an `agent_end` event was
observed, turn/tool counters, provider status (when available), and a bounded
last error. The adapter exposes these fields in Harbor metadata and keeps the
report beside the stdout, stderr, and trajectory evidence. The default Code
execution budget is 840 seconds; set
`A3S_CODE_TERMINAL_BENCH_MAX_EXECUTION_MS` when the outer Harbor task has a
different deadline, leaving enough margin for result persistence. A benchmark
run that emits `agent_end` without any successful Tool execution is reported as
`failed/evidence_missing`; this prevents a text-only response from being
counted as a valid artifact-producing run.

Provider HTTP responses that are not retryable are represented as typed
terminal errors at the Core boundary. The report classifies these as
`failed/provider_rejected` and retains only bounded diagnostic text, so an
account, credential, or request-shape failure cannot consume the run budget via
blind retries.

Container command output is captured incrementally with the Core 100 KiB
head/tail limit before it is returned to the Tool loop. High-volume commands
therefore cannot make the benchmark runner buffer an unbounded stdout/stderr
payload; truncation and byte counts remain visible through the normal Tool
metadata observer.
