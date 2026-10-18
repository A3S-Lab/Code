# Terminal-Bench evaluation

The native adapter runs A3S Code's existing Core session and workspace tools
inside Harbor's task container. Harbor remains responsible for the task image,
instruction delivery, time limits, artifacts, and verifier. The adapter does
not provide MCP tools, alter a task, install a solution, or replace the
verifier.

Harbor task images typically lack bubblewrap. The runner injects Core's
`ProcessHostBashSandbox` and sets `A3S_CODE_ALLOW_PROCESS_HOST_SANDBOX=1` so
default `bash` (`use_default`) executes inside the already-isolated task
container without a `require_escalated` trial round. Native fail-closed
behavior on developer machines is unchanged.

The benchmark task instruction is uploaded byte-for-byte to
`/run/a3s/instruction.md`. The runner explicitly pins the general writable
agent style and disables planning pre-analysis, so the model receives that
instruction as its single user message. The explicit style takes precedence
over task vocabulary (for example, `design` or `findall`) and therefore cannot
silently route the writable session to a read-only style. Search mode remains a
model decision through the normal A3S Code tool descriptions.

Build the static runner from the Code crate. Match the Harbor task image
architecture (`uname -m` inside the container). Docker Desktop on Apple
Silicon often runs `x86_64` TB images; native Linux aarch64 hosts use
`aarch64-unknown-linux-musl`:

```bash
# x86_64 Harbor task images (common on Docker Desktop):
cargo zigbuild --locked --target x86_64-unknown-linux-musl \
  --release --no-default-features --example terminal_bench_runner

# aarch64 Harbor task images:
cargo zigbuild --locked --target aarch64-unknown-linux-musl \
  --release --no-default-features --example terminal_bench_runner
```

The runner binds Harbor host Passed verification reports to Core completion-gate
mutation digests (same contract as the Python Harbor smoke adapter). Harbor's
native verifier remains the task acceptance authority.

Run an official Terminal-Bench 4.0 task with the local Codex login:

```bash
TB_TARGET=x86_64-unknown-linux-musl   # or aarch64-unknown-linux-musl
A3S_CODE_TERMINAL_BENCH_BINARY="$PWD/target/${TB_TARGET}/release/examples/terminal_bench_runner" \
A3S_CODE_CONFIG="$PWD/../../.a3s/config.acl" \
A3S_CODEX_AUTH_FILE="$HOME/.codex/auth.json" \
A3S_CODEX_MODEL=gpt-6-astra \
A3S_CODEX_REASONING_EFFORT=max \
PYTHONPATH="$PWD/scripts/terminal_bench" \
harbor run -d terminal-bench/terminal-bench@4.0.0 \
  -a a3s_code_agent:A3SCodeAgent \
  -t terminal-bench/<task> -n 1 -k 1 -y
```

DeepSeek (or other ACL providers) can replace Codex by pointing
`A3S_CODE_CONFIG` at an ACL that sets `default_model` and `providers` from env
(see monorepo `scripts/harbor/native-tb.acl`) and omitting the Codex env vars.

For a leaderboard-compatible run, use the complete tagged dataset, five
attempts per task, and a GPU-capable sandbox as required by Terminal-Bench:

```bash
harbor run -d terminal-bench/terminal-bench@4.0.0 \
  -a a3s_code_agent:A3SCodeAgent \
  -n 100 -k 5 -y
```

## ACP lane (`TB-ACP1`)

ACP now defaults to engineering discipline and bound-goal continuation. An
explicit comparison-off arm must use `A3S_CODE_ACP_ENGINEERING_POLICY=none`
and `A3S_CODE_ACP_GOAL_CONTINUATION=0`; omitted values inherit product defaults.
The adapter forwards those as `--engineering-policy none` and
`--no-goal-continuation`. This default change does not itself run an evaluation.

`a3s-code-acp --unattended` runs the host admission loop of
`terminal_bench_runner`, behind Harbor's stock ACP runner, with every A3S Code
capability on (`TB-ACP2`): `a3s-vec` lexical FTS search, Code Intelligence
(rust-analyzer, typescript-language-server, basedpyright or pyright, clangd),
file memory, skills, web tools, and automatic plus manual delegation. Every tool
call is approved without approval learning, a model question gets a fixed
"nobody is available" reply, harness state goes to a private temporary root
instead of the graded workspace, and `--result-file` writes the same
`a3s.code.terminal-bench-result.v1` record. Without `--unattended` the agent is
unchanged.

The ACP lane builds only inside the A3S OS monorepo (`kense-labs/os`), from its
`apps/code` submodule. `a3s-code-acp` depends on `a3s-okf` and
`a3s-use-reconcile`, which live in that monorepo's `crates/` and are not on
crates.io, and `run_acp_tb.sh` is the monorepo's `scripts/harbor/`. A
standalone `A3S-Lab/Code` checkout builds `a3s-code-core` and the TUI after
`.github/setup-workspace.sh`, which drops the `acp` member.

```bash
# from <os checkout>/apps/code
CARGO_TARGET_DIR=$HOME/a3s-tb-target cargo zigbuild --locked \
  --target x86_64-unknown-linux-musl --release -p a3s-code-acp --bin a3s-code-acp
../../scripts/harbor/run_acp_tb.sh   # A3S_TB_INCLUDE_TASK=<task> for one task
```

`a3s_code_acp_agent:A3SCodeAcpAgent` uploads the local binary and ACL instead
of fetching an archive, keeps provider credentials out of the launcher script,
and reaches the verifier on the same rule as the native adapter. Harbor's ACP
runner installs Python and `agent-client-protocol` in the task image (`apt-get`
plus `pip`, 225 s to over 360 s on a slow network). A runner bundle built once
replaces that step on glibc images:

```bash
../../scripts/harbor/prepare_runner_bundle.sh   # ~/.cache/a3s/acp-runner-linux-x86_64.tar.gz, 35 MB
```

`run_acp_tb.sh` passes it as `A3S_CODE_RUNNER_BUNDLE`; the adapter unpacks it at
`/opt/harbor-acp-venv` and skips Harbor's install when that Python imports
`acp` (agent setup then takes about 6 s). Elsewhere Harbor's install runs as
before. The model is selected through the `model` session config option
(Python ACP 0.12 has no `models` field). The native runner stays the baseline
until an A/B on the same tasks shows parity.

Derived indexes (`a3s-vec` FTS, grep trigram stamp) and file memory live in the
private state root, so the graded workspace holds only what the agent wrote.
Language servers come from a bundle built once on the host:

```bash
../../scripts/harbor/prepare_lsp_bundle.sh   # ~/.cache/a3s/lsp-linux-x86_64.tar.gz, 208 MB
```

`run_acp_tb.sh` passes it as `A3S_CODE_LSP_BUNDLE` on linux-x86_64; each trial
uploads and unpacks it into `/opt/a3s-lsp` with no network (about 3 s) and
links basedpyright, typescript-language-server, clangd and rust-analyzer into
`/usr/local/bin` unless the image already has them. The bundle needs glibc
(Node 24: glibc 2.28+); on Alpine the servers do not start.

Without a bundle the adapter falls back to installing missing language
servers best effort under one deadline inside Harbor's 360 s agent-setup
limit: basedpyright and
typescript-language-server through `npm` into `/opt/a3s-lsp`, else basedpyright
through a private Python venv, and clangd through `apt-get` (download bounded,
install from cache) or `apk`. A server that does not arrive leaves that language
unavailable; the trial continues.

| Variable | Effect |
| --- | --- |
| `A3S_CODE_ACP_LANGUAGE_SERVERS=0` | skip the language-server install |
| `A3S_CODE_LSP_BUNDLE` | prepared bundle to upload; a set but missing file refuses the install |
| `A3S_CODE_ACP_LANGUAGE_SERVER_BUDGET_SEC` | install deadline, default 150 |
| `A3S_CODE_ACP_REASONING_EFFORT` | `--reasoning-effort` for the agent: `low`, `medium`, `high`, `xhigh` or `max` (default `max`) |
| `A3S_CODE_LLM_STREAM_IDLE_TIMEOUT_MS` | forwarded to the agent; a provider stream silent this long is retried. `run_acp_tb.sh` sets 120000 (core default 300000) |
| `A3S_CODE_RUNNER_BUNDLE` | prepared runner Python to unpack at `/opt/harbor-acp-venv`; a set but missing file refuses the install |
| `A3S_CODE_SKILLS_DIR` | local skills directory uploaded to `/run/a3s/skills`; list that path in the evaluation ACL's `skill_dirs` |

## TB-QUAL1 qualification gate (`HARNESS-CONV7`)

See also the wrap-up index and evidence templates in
[HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md).

`TB-QUAL1` is In progress until a Harbor job retains the native verifier result
for every trial on the complete tagged dataset.

| Step | Requirement |
| --- | --- |
| 1. Diagnostic matrix | Run a small `-t` / limited `-n` matrix first; treat results as diagnostic only |
| 2. Full tagged dataset | Harbor `terminal-bench@4.0.0` complete tag; GPU sandbox if required by TB |
| 3. Attempts | Leaderboard-compatible `-k` (typically 5) |
| 4. Evidence | Keep the Harbor job directory; each trial must retain Harbor's native `verifier_result` |
| 5. Fail closed | Timeout, runner error, missing verifier result, or local-only aggregate is **not** a pass |
| 6. Publish | Link the Harbor job / artifact digest from ROADMAP `TB-QUAL1` when Delivered |

Optional Code live gates (not a TB substitute):

```bash
A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  cargo test -p a3s-code-core --test test_deepseek_adversarial_e2e -- \
  --ignored --test-threads=1 --nocapture

A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  cargo test -p a3s-code-core --test test_prompt_capability_real_llm -- \
  --ignored --test-threads=1 --nocapture
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

Retry authority is single-owner: a provider consumes its bounded HTTP retry
budget, then the Agent circuit breaker treats typed exhaustion as terminal
instead of replaying the same request through fallback. Streaming retries honor
the provider's `Retry-After` value, and cancellation interrupts that backoff;
the regression suite records the expected inner request count. Hosts can inspect
the public bounded `RetryExhaustedError` projection for its status and attempt
count without parsing provider diagnostics.

Container command output is captured incrementally with the Core 100 KiB
head/tail limit before it is returned to the Tool loop. High-volume commands
therefore cannot make the benchmark runner buffer an unbounded stdout/stderr
payload; truncation and byte counts remain visible through the normal Tool
metadata observer.
