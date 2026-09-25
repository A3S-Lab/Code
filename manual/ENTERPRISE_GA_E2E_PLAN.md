# Enterprise GA Deep E2E Plan (a3s-code)

**Status:** Planning authority for enterprise GA. Hermetic + L5 Layer C are
already closed on the current line. L6 release performance and L7 external
qualification are **not** claimed. Complex-case Flash host composition has
fresh measured evidence; that evidence is a provider-latency observation, not
an L6 digest substitute.

This plan joins:

- [V9_0_0_COMPLETION_ROADMAP.md](V9_0_0_COMPLETION_ROADMAP.md) — sequenced
  first-principles closure of the 9.0.0 fact-log line (channel release vs GA)
- [FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md) — L0–L8 train and
  release handoff
- [FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md) — Layers A–D
- [PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md) — L6 budgets
- [TYPED_DECISION_E2E_PLAN.md](TYPED_DECISION_E2E_PLAN.md) — optional Apofasi
  host composition
- [CAPABILITY_VERIFICATION.md](CAPABILITY_VERIFICATION.md) — done means
  runtime + external + measured performance

It does not invent a parallel green path. Enterprise GA requires the same
gates the full-feature plan already named.

## 1. First principles

1. **Capability surfaces are the universe.** `sdk_capabilities()` baseline and
   Advanced ids are the product under test. A green typed-decision suite does
   not close the other capabilities.
2. **Correctness before latency.** Kernel hermetics and Layer C live oracles
   prove behavior. Performance budgets never rewrite those oracles.
3. **Labels are not correctness.** Neural or Flash JSON that names `billing`
   is host evidence. It is never the typed `Answer` and never a pass criterion.
4. **Default gate stays 0.7.** Auto means zero generations. Escalate means at
   most one host generation after Apofasi sealed the decision. Do not lower
   `GatePolicy` or retune glosses to manufacture Auto.
5. **L6 excludes provider latency.** `performance.yml` profiles are local
   release builds without a live model. Flash wall times for typed decisions
   are a separate observation class (TD-PERF). They inform operators; they do
   not replace L6 digests.
6. **External owners stay external.** Harbor TB-QUAL1, DM-PROD1, and CAR-01…05
   are L7. Core CI green is not a Harbor waiver.
7. **Standalone CI must build.** Workspace `[patch]` tables that point at
   monorepo siblings (`../apofasi`, `../sandbox`, `../vec`) must be stripped
   and re-locked in `.github/setup-workspace.sh` /
   `.github/relock-standalone-deps.sh` before `--locked` jobs.

## 2. Enterprise GA definition

A channel is enterprise GA when **all** of the following are true on the
candidate commit:

| Gate | Evidence |
| --- | --- |
| L0–L1 hermetic | `just harness-convergence-check` + local-code lib |
| L3 for enabled features | Feature suites for the artifact under test |
| L4 SDK parity | Node/Python/Go discovery + fixtures |
| L5 Layer C | `LAYER_C_PASS` under bailian Flash pin |
| Integrated-use ledger | Dated E+E+I snapshot current |
| L6 | Fresh `performance.yml` + `hermetic-integrations.yml` digests in PERFORMANCE_QUALIFICATION |
| L7 | Harbor / CAR receipts or written product waiver |
| L8 | Version regression pins for the delta since last tag |
| Optional Apofasi (when shipping `apofasi`) | TD-A…TD-E green; TD-PERF Flash complex percentiles recorded |

Missing any row means **not enterprise GA**. Do not write the ledger as GA.

## 3. Deep train order

1. L0–L1 on every PR.
2. L4 when bindings or public API move.
3. L2 F-kernel coverage before a stable tag.
4. L3 for every feature in the release Cargo set (including `apofasi` when that
   artifact ships).
5. L5 `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash just layer-c-live-e2e`.
6. TD-D / TD-E ignored live typed-decision composition (serial, ACL required).
7. TD-PERF complex Flash percentiles (serial, ACL required).
8. L6 `gh workflow run performance.yml --repo A3S-Lab/Code` and download the
   nine JSON reports.
9. L7 Harbor / host receipts.
10. L8 pin table for this cut.

## 4. Complex-case real-model performance (TD-PERF)

### 4.1 Cases

| Case | Request | Engine | Gate |
| --- | --- | --- | --- |
| Billing triage Flash | Four-question object-state triage (Apofasi binary bench) | Lexical System-1 → host Flash escalate | Default 0.7; must escalate |
| Billing triage neural | Same request | Published english checkpoint | Record escalate rate; do not force Auto |
| Six single-question tasks | Existing `typed_decision_perf` table | Lexical vs Flash | Default 0.7 |

### 4.2 Oracles

| Metric | Pass | Fail |
| --- | --- | --- |
| Generations on escalate | Exactly 1 per sample | 0 or >1 |
| Generations on Auto | 0 | Any generation |
| Host admission | Evidence or Rejected / GenerationFailed classified | Panics, or Answer forged from model text |
| Labels | Not asserted | Asserting department / noul / score |
| Gate | `GatePolicy::default()` min_confidence 0.7 | Lowered threshold |
| Latency | Reported p50 / min / max | Treated as L6 SLA |

### 4.3 Commands

```bash
cd crates/code
export A3S_CONFIG_FILE="$(git rev-parse --show-toplevel)/.a3s/config.acl"
export A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash
cargo test -p a3s-code-core --no-default-features --features apofasi \
  --test typed_decision_perf complex_billing_triage_flash_percentiles \
  -- --ignored --nocapture --test-threads=1

export APOFASI_CHECKPOINT=/path/to/published/english/checkpoint
cargo test -p a3s-code-core --release --no-default-features --features apofasi,apofasi-metal \
  --test typed_decision_perf complex_billing_triage_neural_percentiles \
  -- --ignored --nocapture --test-threads=1
```

### 4.4 Measured evidence (2026-09-21, remapped pin)

Host machine, release build, request identical to
`typed_decision_layer_c::billing_triage_request`, default gate 0.7,
`boyue/bailian/deepseek-v4.1-flash`, `max_tokens=1024`, warmup=1, measured=7
(Flash) / warmup=3, measured=20 (neural Metal):

| Run | p50 | min | max | outcome |
| --- | --- | --- | --- | --- |
| Flash host composition | 2531.9 ms | 1931.7 ms | 7344.8 ms | generations=1, admitted Evidence 7/7, prompt Σ 1449, completion Σ 1134 |
| Neural decide + gate | 45.5 ms | 44.2 ms | 47.5 ms | escalate 20/20 at default 0.7 |

Earlier exploratory runs used a divergent triage JSON (missing `other`, prose
glosses) and hit `logits must be finite` on Metal. The product fixture above is
authoritative. Do not treat Flash p50 as an L6 SLA. Do not treat neural
escalate-all as a defect; Auto requires confidence ≥ 0.7.

## 5. Current GA board

| Item | State |
| --- | --- |
| L0/L1/L3/L4/L5 + integrated-use | Closed on the prior FULL_FEATURE status for this line |
| L2 F-kernel cov (≥95%) | **PASS** — `/tmp/a3s-llvm-cov-f95/FINAL.txt` `ALL_F_TABLE_KERNELS_GE_95_PASS scored=42`; `core/src/agent_protocol_harness.rs` **95.16%** after SessionClosed→Closed mapper unit test |
| L6 performance.yml on tip `ee8f68ad` | **PASS** — run [`36092780724`](https://github.com/A3S-Lab/Code/actions/runs/36092780724), nine `passed: true`; digest `731386c0407e38bf8a14bcec939c438fdcc86a1cea9fff1e4b2a1a5eec21656b` |
| L6 hermetic-integrations on tip `ee8f68ad` | **PASS** — run [`36122389323`](https://github.com/A3S-Lab/Code/actions/runs/36122389323); digest `4ac6d54e8867a5365e12ab70124949756121de5ad1984f6a5be2d6e7492906d3` |
| L6 performance.yml on tip `91d34757` | **PASS** — run [`36089389830`](https://github.com/A3S-Lab/Code/actions/runs/36089389830), nine `passed: true` JSON reports; digest `6f6abcd483e49cbe6ca26adea9bbab461fa4c51cab60c0961fde5f52b4e06349` in PERFORMANCE_QUALIFICATION |
| L6 hermetic-integrations on tip `91d34757` | **PASS** — run [`36089393353`](https://github.com/A3S-Lab/Code/actions/runs/36089393353); S3/CDP/OTLP `passed: true`; digest `18f3d7a42e5a8bba0e828e1d4d4be8e472f18818993a3036da5712d23ad82620` in PERFORMANCE_QUALIFICATION |
| L6 performance.yml on `fa0a92ca` | **PASS** — run [`35947891976`](https://github.com/A3S-Lab/Code/actions/runs/35947891976), nine `passed: true` JSON reports; digest `b5ce0281d5bad5ab47003bd34d93ab11bc7eab1ef8f37c24b10b8fe619010b73` in PERFORMANCE_QUALIFICATION |
| L6 hermetic-integrations on `fa0a92ca` | **PASS** — CI run [`35947892108`](https://github.com/A3S-Lab/Code/actions/runs/35947892108) job Hermetic integrations; S3/CDP/OTLP `passed: true`; digest `00ee5ea7eb32a09a9fd4cf8eacc40642c4d548319d01822ef3e809f753207269` in PERFORMANCE_QUALIFICATION |
| L6 performance.yml on prior tip `c9e26504` | **PASS** — run [`35667548466`](https://github.com/A3S-Lab/Code/actions/runs/35667548466), nine `passed: true` JSON reports; digest `c4c96e0bf2544319354b09bdbf4e3954931eac4cd46d77b7a33ce044f1849c87` in PERFORMANCE_QUALIFICATION |
| L6 hermetic-integrations on prior tip `c9e26504` | **PASS** — run [`35667552718`](https://github.com/A3S-Lab/Code/actions/runs/35667552718); S3/CDP/OTLP `passed: true`; digest `308370110f9f69cf8b71c75084e5853a0adbead77a46ca171338b5f9d9bef764` in PERFORMANCE_QUALIFICATION |
| L6 performance.yml on `9b28f066` | **PASS** — prior RC [`35639843682`](https://github.com/A3S-Lab/Code/actions/runs/35639843682); superseded by tip digests above |
| Standalone patch strip + relock | **Landed** on `9b28f066` |
| L7 Harbor / CAR | **Not claimed** — TB-QUAL1 still In progress (diagnostic Harbor install-only green on wheel 8.6.0 job `2026-09-22__07-32-16`; agent smoke hits completion-gate without host waiver). DM-PROD1 / CAR checklist empty. No product waiver recorded. |
| TD-A…TD-E | Implemented; live Flash complex cases previously green |
| TD-PERF Flash billing triage | **Measured** (§4.4) — p50 2531.9 ms, Evidence 7/7 |
| TD-PERF neural billing triage | **Measured** (§4.4) — p50 45.5 ms, escalate 20/20 at 0.7 |
| 9.0.0 fact-log cut, local L5 through `74356768` | **PASS** — earlier `LAYER_C_PASS model=boyue/bailian/deepseek-v4-flash` (22/22). That FINAL does not cover `fa0a92ca`. |
| L5 Layer C on tip `ee8f68ad` | **PASS** — `/tmp/a3s-layer-c-9.0.0-tip/FINAL.txt` is exactly `LAYER_C_PASS model=boyue/bailian/deepseek-v4-flash`. 22 suites passed, including `test_extensibility_real_llm`. `test_context_tools_real_llm` recovered after stream-worker join timeout fix. |
| L5 Layer C on `fa0a92ca` | **PASS** — `just layer-c-live-e2e` wrote exactly `LAYER_C_PASS model=boyue/bailian/deepseek-v4-flash`. 22 suites passed, including `test_extensibility_real_llm`. The runner log ends `LAYER:0`. Pin remapped to `boyue/bailian/deepseek-v4.1-flash`. |
| L6 on the 9.0.0 commit `fa0a92ca` | **PASS** — performance.yml `35947891976` and hermetic-integrations via CI `35947892108`, both `passed: true`, archived above |
| Enterprise GA | **Not achieved** — L7 TB-QUAL1, DM-PROD1, and CAR-01…CAR-05 still have no close receipt and no product waiver. |

## 6. Closure checklist

- [x] Land standalone CI patch strip/relock; confirm Required CI Check green
  after aligning `CODE-APOFASI1` in the scoped-capability gate table
- [x] Re-run `performance.yml`; archive nine `passed: true` JSON digests into
      PERFORMANCE_QUALIFICATION (`35639843682` / `9b28f066`)
- [x] Re-run hermetic-integrations on tip `c9e26504` (`35667552718`); archive
      digest into PERFORMANCE_QUALIFICATION
- [x] Re-run `performance.yml` on tip `c9e26504` (`35667548466`); archive nine
      `passed: true` digests (`c4c96e0bf2544319354b09bdbf4e3954931eac4cd46d77b7a33ce044f1849c87`)
- [ ] Collect L7 Harbor TB-QUAL1, DM-PROD1, CAR-01…05 or write a product waiver
- [x] Re-measure TD-PERF neural on release Metal without changing the gate
- [x] Update FULL_FEATURE §13 L6 status with current performance digests

Do not mark enterprise GA complete from typed-decision green alone.
Do not invent an L7 waiver without a product decision.
