# Enterprise GA Deep E2E Plan (a3s-code)

**Status:** Planning authority for enterprise GA. Hermetic + L5 Layer C are
already closed on the current line. L6 release performance and L7 external
qualification are **not** claimed. Complex-case Flash host composition has
fresh measured evidence; that evidence is a provider-latency observation, not
an L6 digest substitute.

This plan joins:

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
| L6 performance.yml on `9b28f066` | **PASS** — run [`35639843682`](https://github.com/A3S-Lab/Code/actions/runs/35639843682), nine `passed: true` JSON reports; digest in [PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md) |
| L6 hermetic-integrations on `515a50b0` | **PASS** — run [`35642286881`](https://github.com/A3S-Lab/Code/actions/runs/35642286881); S3/CDP/OTLP `passed: true`; digest in PERFORMANCE_QUALIFICATION |
| Standalone patch strip + relock | **Landed** on `9b28f066` |
| L7 Harbor / CAR | **Not claimed** — TB-QUAL1 still In progress (diagnostic Harbor install-only green; agent smoke hits completion-gate without host waiver). DM-PROD1 / CAR checklist empty. No product waiver recorded. |
| TD-A…TD-E | Implemented; live Flash complex cases previously green |
| TD-PERF Flash billing triage | **Measured** (§4.4) — p50 2531.9 ms, Evidence 7/7 |
| TD-PERF neural billing triage | **Measured** (§4.4) — p50 45.5 ms, escalate 20/20 at 0.7 |
| Enterprise GA | **Not achieved** — tip CI + L6 digests must be current on the candidate commit; L7 Harbor/CAR receipts or a written product waiver remain open |

## 6. Closure checklist

- [x] Land standalone CI patch strip/relock; confirm Required CI Check green
  after aligning `CODE-APOFASI1` in the scoped-capability gate table
- [x] Re-run `performance.yml`; archive nine `passed: true` JSON digests into
      PERFORMANCE_QUALIFICATION (`35639843682` / `9b28f066`)
- [x] Re-run hermetic-integrations on the same line (`35642286881` / `515a50b0`)
- [ ] Collect L7 Harbor TB-QUAL1, DM-PROD1, CAR-01…05 or write a product waiver
- [x] Re-measure TD-PERF neural on release Metal without changing the gate
- [x] Update FULL_FEATURE §13 L6 status with current performance digests

Do not mark enterprise GA complete from typed-decision green alone.
Do not invent an L7 waiver without a product decision.
