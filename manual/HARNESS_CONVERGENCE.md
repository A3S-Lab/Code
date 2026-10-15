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
| `TB-QUAL1` | Deferred by product (2026-10-03); no verifier receipt on `439cc740` | [TERMINAL_BENCH.md](TERMINAL_BENCH.md) |
| `DM-PROD1` | Delivered on `3b2f72aa`; does not cover changelog fold `439cc740` | [DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md](DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md) |
| L6 on `439cc740` | Recorded; not Enterprise GA | CI [`37117714232`](https://github.com/A3S-Lab/Code/actions/runs/37117714232) attempt 2; performance [`37117721745`](https://github.com/A3S-Lab/Code/actions/runs/37117721745) nine `passed: true`; hermetic artifact `hermetic-integrations-37117714232-1` three `passed: true`. F-table and Layer C stay on parent `9f521169`. See [PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md) |
| L7 on `439cc740` | Not closed | `DM-PROD1` pack `3b2f72aa` does not cover this fold. `TB-QUAL1` has no verifier receipt. CAR stays out of scope |
| L6 on `9f521169` | Recorded; not Enterprise GA | CI [`37110650044`](https://github.com/A3S-Lab/Code/actions/runs/37110650044); performance [`37112230393`](https://github.com/A3S-Lab/Code/actions/runs/37112230393) nine `passed: true`; hermetic artifact `hermetic-integrations-37110650044-1` three `passed: true`; F-table 44/44 kernels ≥ 95%; Layer C `LAYER_C_PASS model=boyue/bailian/deepseek-v4.1-flash`. See [PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md) |
| L7 on `9f521169` | Not closed | `DM-PROD1` pack `3b2f72aa` does not cover this tip. `TB-QUAL1` has no verifier receipt. CAR stays out of scope |
| L6 on `2bd6a234` | Recorded; not Enterprise GA | CI [`37103577874`](https://github.com/A3S-Lab/Code/actions/runs/37103577874); performance [`37103577761`](https://github.com/A3S-Lab/Code/actions/runs/37103577761) nine `passed: true`; hermetic artifact `hermetic-integrations-37103577874-1` three `passed: true`; F-table 44/44 kernels ≥ 95%; Layer C `LAYER_C_PASS model=boyue/bailian/deepseek-v4.1-flash`. See [PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md) |
| L7 on `2bd6a234` | Not closed | `DM-PROD1` pack `3b2f72aa` does not cover this admission. `TB-QUAL1` has no verifier receipt. CAR stays out of scope |
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
| `-n` / `-k` | Diagnostic close: `-n 1 -k 1` on `bun-sourcemap-leak`. TB-QUAL1 is deferred by the 2026-10-03 product decision. Tips `2bd6a234`, `9f521169`, and changelog fold `439cc740` have no Harbor verifier receipt. The earlier job was stopped and is not a pass. |
| GPU sandbox | Docker Desktop on darwin host for tip Flash runs; prior WSL2+RTX 4090 evidence retained for GPU-tagged tasks |
| Trials with native `verifier_result` | Diagnostic `1/1` present (`reward: 0.0`); Harbor agent exception none. Full-matrix retention pending TB-QUAL1 job completion. |
| Failures classified | Diagnostic: other — task incorrect / incomplete under verifier; host completion-gate + verifier retention no longer blocked. Full-matrix classification pending. |
| ROADMAP link date | 2026-09-26 (diagnostic stamped; full TB-QUAL1 `-k 5` matrix **deferred by product** — Harbor job stopped) |

### DM-PROD1

| Field | Value |
| --- | --- |
| Host / environment | Host pack `/tmp/dm-prod1-host-3b2f72aa` on tip `3b2f72aa` |
| Embedding provider + model | Boyue `text-embedding-3-small` (1536-d probe, probe 1.64 s, latency p50 0.81 s, 19 requests, 124 inputs, 2146 tokens, 0 failures) |
| Remote CAS + lease policy | Redis `IndexRevisionCas` via `WATCH`/`MULTI`/`EXEC`; `SET NX EX` leases with `INCR` fences |
| Horizons / multi-agent load | 8 independent writers racing one prefix (1 commit / 7 conflicts, convergence to 8 records); drift/cache hits 0/48/51/48/0 against provider inputs 48/4/1/2/0; zero cross-namespace recall on a shared index |
| Restart + drift report path | `/tmp/dm-prod1-host-3b2f72aa/report.json` `sha256:b3f36cf99a27db359251620e846bf38e366a35f00b62cecaa4951f09b16f7938` (all seven rows pass); 2 restart cycles plus checkpoint resume settling `Unchanged` |
| Secret hygiene review | `HYGIENE_OK` |
| ROADMAP link date | 2026-10-02. This pack does not cover standing-context admission `9fb1cb79`, tip `2bd6a234`, or tip `9f521169` |

### CAR close

| Gate | External run / artifact | Blocking party cleared |
| --- | --- | --- |
| `CAR-01` | Out of scope: A3S Cloud retired by product decision (2026-09-26); last partial run was Cloud [`36180557882`](https://github.com/A3S-Lab/Cloud/actions/runs/36180557882) (Box profiles, recovery, Skill hydration green) | n/a |
| `CAR-03` | Out of scope (Cloud retired) | n/a |
| `CAR-04` | Out of scope (Cloud retired) | n/a |
| `CAR-05` | Out of scope (Cloud retired) | n/a |

When a row is complete, paste the secret-free link into the matching ROADMAP
exit cell and flip status to Delivered.
