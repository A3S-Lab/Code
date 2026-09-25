# Harness Convergence Wrap-up (`HARNESS-CONV4`–`CONV7`)

See also [FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md) for the full
Layer A–D verification matrix used by the E2E goal.

## Code-side status

| Gate | Status | Evidence |
| --- | --- | --- |
| `HARNESS-CONV4` | Delivered | Model-visible `parallel_task` unregistered; SDK helpers removed; durable-memory Active-only (`CAP-GA1`) |
| `HARNESS-CONV5` | Delivered | Core/SDK `default` ≈ `local-code` / a3s-vec; CLI pins `scientific` |
| `HARNESS-CONV6` | Delivered | Prompt bodies are a replaceable default pack; permission overlays stay Core-owned |
| `HARNESS-CONV7` | Code prep Done / external In progress | Runbooks below; close only with secret-free linked reports |
| `TB-QUAL1` | In progress | [TERMINAL_BENCH.md](TERMINAL_BENCH.md) |
| `DM-PROD1` | In progress | [DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md](DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md) |
| `CAR-01`…`CAR-05` | Checklist ready | [CLOUD_HARNESS_CONFORMANCE.md](CLOUD_HARNESS_CONFORMANCE.md) |

Refuse list (do not reopen): Core reviewer/rubric prompts; restore `server` /
`advanced-harness` to library default; Core unified approval UI; a second
*imperative* message loop or completion-gate / permission bypass; baseline
evaluation Gate semantics. **Allowed:** multi-component Meta Harness trees on
one fact log (`SessionOptions.harness` / `a3s-effect` compose) —
see [META_HARNESS.md](META_HARNESS.md).

## Local verification (Code)

```bash
# Thin default must stay clean of Advanced/server stacks
cargo check -p a3s-code-core
cargo tree -p a3s-code-core -e normal | rg -i 'evaluation|research|chromiumoxide|aws-sdk-s3' && exit 1 || true

# CI gate matrix
cargo check -p a3s-code-core --no-default-features --features local-code
cargo test -p a3s-code-core --no-default-features --features local-code --lib

# Dual-path regressions
cargo test -p a3s-code-core --lib parallel_task -- --nocapture
cargo test -p a3s-code-core --lib candidate_write_stays_inactive -- --nocapture

# SDK surface alignment (no parallel_task helpers)
node scripts/sdk_api_alignment_check.mjs
```

Optional live gates (not TB/DM/CAR substitutes):

```bash
A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  cargo test -p a3s-code-core --test test_deepseek_adversarial_e2e -- \
  --ignored --test-threads=1 --nocapture

A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  cargo test -p a3s-code-core --test test_prompt_capability_real_llm -- \
  --ignored --test-threads=1 --nocapture
```

## Evidence pack templates (external close)

Fill one row per retained report. Never commit credentials, tokens, or raw
prompts.

### TB-QUAL1

| Field | Value |
| --- | --- |
| Harbor dataset tag | `terminal-bench@4.0.0` (complete) |
| Job / artifact digests | Diagnostic only (not TB-QUAL1 close): install-only `2026-09-22__07-32-16` (`INSTALL_ONLY_RC=0`). Boyue wiring + pinned agent smoke: job `2026-09-22__09-41-11` / `terminal-bench/bun-sourcemap-leak` (`AGENT_SMOKE_RC=0`, Harbor exceptions 0, reward 0.0 — task unsolved). Prior layout mid-run `2026-09-22__08-27-10` (~68m live API + `/app/scratch` CV) remains supporting evidence. Native `terminal_bench_runner` + full `-k` matrix still required for TB-QUAL1. |
| `-n` / `-k` | diagnostic `-n 1 -k 1` (`A3S_TB_INCLUDE_TASK=terminal-bench/bun-sourcemap-leak`) |
| GPU sandbox | WSL2 Ubuntu + Docker; host NVIDIA GeForce RTX 4090 (24564 MiB) |
| Trials with native `verifier_result` | `1/1` present (`reward: 0.0`); Harbor agent exception none |
| Failures classified | other — task incorrect / incomplete solution under verifier; adapter + boyue key/base_url wiring no longer blocked |
| ROADMAP link date | 2026-09-22 |

### DM-PROD1

| Field | Value |
| --- | --- |
| Host / environment | |
| Embedding provider + model | |
| Remote CAS + lease policy | |
| Horizons / multi-agent load | |
| Restart + drift report path | |
| Secret hygiene review | pass / fail |
| ROADMAP link date | |

### CAR close

| Gate | External run / artifact | Blocking party cleared |
| --- | --- | --- |
| `CAR-01` | | |
| `CAR-03` | | |
| `CAR-04` | | |
| `CAR-05` | | |

When a row is complete, paste the secret-free link into the matching ROADMAP
exit cell and flip status to Delivered.
