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
| Job / artifact digests | Diagnostic only (not TB-QUAL1 close): tip RC `b91462d3` Flash job `2026-09-26__02-14-59` / `bun-sourcemap-leak__QqXF2Yf` — Harbor exceptions 0, native `verifier_result.rewards.reward=0.0` retained, host completion-gate binds exercised (`boyue/bailian/deepseek-v4-flash`). Prior install-only `2026-09-22__07-32-16` and smoke `2026-09-22__09-41-11` remain supporting. Full tagged dataset `-k 5` still required for TB-QUAL1. |
| `-n` / `-k` | Diagnostic close: `-n 1 -k 1` on `bun-sourcemap-leak`. TB-QUAL1 in flight: job dir `.harbor-tb-qual1/jobs/2026-09-26__02-24-52` with `-n 2 -k 5` on complete `terminal-bench@4.0.0` (Flash). |
| GPU sandbox | Docker Desktop on darwin host for tip Flash runs; prior WSL2+RTX 4090 evidence retained for GPU-tagged tasks |
| Trials with native `verifier_result` | Diagnostic `1/1` present (`reward: 0.0`); Harbor agent exception none. Full-matrix retention pending TB-QUAL1 job completion. |
| Failures classified | Diagnostic: other — task incorrect / incomplete under verifier; host completion-gate + verifier retention no longer blocked. Full-matrix classification pending. |
| ROADMAP link date | 2026-09-26 (diagnostic stamped; full TB-QUAL1 `-k 5` matrix **deferred by product** — Harbor job stopped) |

### DM-PROD1

| Field | Value |
| --- | --- |
| Host / environment | darwin host + kense-redis `127.0.0.1:6379` DB 15; tip Code `b7b239a8` (RC `b91462d3` stack) |
| Embedding provider + model | Boyue OpenAI-compatible `text-embedding-3-small` (1536-d); pack `/tmp/dm-prod1-host-be467457` |
| Remote CAS + lease policy | Redis `VectorIndex` IndexRevisionCas + `SET NX EX` lease with fence tokens; failover via CLIENT KILL |
| Horizons / multi-agent load | Five minutes-scale horizons (initial publication, candidate activation, single-node drift, consolidation/decay, steady state) + 8 independent Redis writers racing one prefix (1 commit / 7 `RevisionConflict`, convergence to 8 records); caveats retained per row in the report |
| Restart + drift report path | `/tmp/dm-prod1-host-be467457/report.json` `sha256:208e333fedb188eccd64475cbf5c493d4c06c96fc42db2e9ebf0148af145664a` (`passed: true`, all seven dimensions PASS); 2 restart cycles with stable history/binding/serving digests plus checkpoint resume settling `Unchanged` at 0 provider requests |
| Secret hygiene review | pass (`HYGIENE_OK`; 4 credential markers and 25 plaintext strings scanned across every pack file) |
| ROADMAP link date | 2026-09-26 (ROADMAP `DM-PROD1` row Delivered with this path, report digest, and caveats) |

### CAR close

| Gate | External run / artifact | Blocking party cleared |
| --- | --- | --- |
| `CAR-01` | Partial: Cloud tip pin PR [#273](https://github.com/A3S-Lab/Cloud/pull/273) run [`36169895230`](https://github.com/A3S-Lab/Cloud/actions/runs/36169895230) — Box Runtime profiles **success**; Cloud recovery/control-plane lib compile failed under `RUSTFLAGS=-D warnings` (unused imports / dead_code). Not CERTIFIED. | Cloud control-plane tip hygiene |
| `CAR-03` | | |
| `CAR-04` | | |
| `CAR-05` | Box advertised Runtime profiles green on same run (provider pin Box **3.2.5**); full CAR-05 workload matrix still skipped after step-22 failure. | Cloud + Box |

When a row is complete, paste the secret-free link into the matching ROADMAP
exit cell and flip status to Delivered.
