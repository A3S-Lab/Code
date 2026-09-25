# a3s-code 9.0.0 completion roadmap

**Status:** Planning authority for finishing the 9.0.0 fact-log line.  
**Date:** 2026-09-25  
**Does not replace:** [FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md),
[ENTERPRISE_GA_E2E_PLAN.md](ENTERPRISE_GA_E2E_PLAN.md),
[CAPABILITY_INTEGRATED_USE_LEDGER.md](CAPABILITY_INTEGRATED_USE_LEDGER.md),
[PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md),
[HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md).

This roadmap answers one question: what sequence of evidence closes 9.0.0 under
the repository's own definition of done.

---

## 0. First principles

1. **The product delta is control, not features.** 9.0.0 exists to make the
   immutable fact log the only coding control source (`a3s-effect`
   `ingest_coding` / `resume_coding`). Everything else is either support
   (a3s-vec 0.1.8 FTS) or out of cut ([Unreleased] Apofasi).
2. **Labels are not completion.** Version strings in `Cargo.toml` and a
   CHANGELOG section do not ship a channel. crates.io / npm / GitHub Release /
   docs line / tag do.
3. **Digests bind to a commit.** L5/L6 evidence on `fa0a92ca` does not cover
   later tip commits (TUI, ACL merge, model bind). An RC tip must re-earn
   L0–L6 (and L2 before a stable tag).
4. **Correctness before latency; Core green before Harbor.** L6 never rewrites
   Layer C oracles. Core CI never substitutes L7.
5. **Two honest end states.** Do not collapse them:
   - **Channel release:** multi-channel cut of the fact-log line with L0–L6 +
     L8 green, and either L7 receipts **or** a written product waiver naming
     which L7 rows are deferred.
   - **Enterprise GA:** every GA-board row true, including L7 close receipts
     (no silent waiver theater).
6. **Do not invent green.** Missing Harbor/CAR evidence stays open. A waiver
   is a product decision next to the tag, not an agent invention.

---

## 1. Current state (authoritative)

| Fact | Evidence |
| --- | --- |
| Fact-log control implemented | `core/src/fact_control.rs`, CHANGELOG `[9.0.0]`, `a3s-effect` |
| Versions bumped to 9.0.0 | Core / TUI / Node / Python / Go bridge Cargo + package metadata |
| Tip RC | `15d2a863` (L2 SessionClosed→Closed coverage close on top of `b8a13494` / `ee8f68ad` L5/L6) |
| L0/L1 + L5 + L6 on tip | harness-convergence PASS; Layer C `/tmp/a3s-layer-c-9.0.0-tip/FINAL.txt` `LAYER_C_PASS`; L6 Actions `36092780724` / `36122389323` |
| Integrated-use ledger | Refreshed 2026-09-25 for tip; Enterprise GA still not claimed |
| L2 F-kernel cov | **PASS** — `/tmp/a3s-llvm-cov-f95/FINAL.txt` `ALL_F_TABLE_KERNELS_GE_95_PASS scored=42`; `agent_protocol_harness.rs` **95.16%** |
| L8 §7 9.0.0 pins | **PASS (hermetic)** — fact_log 34/34; effect park + tool_round_cap; bm25 a3s-vec FTS 17/17 |
| L7 Harbor / CAR / DM | TB-QUAL1 / DM-PROD1 / CAR still open; diagnostic smoke `2026-09-25__19-11-53` on `bun-sourcemap-leak` (agent running after apt/`ca_certificates` skip fix) |
| No `v9.0.0` tag / Release | Latest published channels still **8.6.0** (crates.io / npm) |
| Docs site | Current archived line `docs/v8.7.0`; no `docs/v9.0.0` |
| Out of 9.0.0 CHANGELOG body | Apofasi typed decisions live under `[Unreleased]` |

---

## 2. Completion criteria (DoD)

### 2.1 Channel release DoD (minimum to call 9.0.0 “shipped”)

All must be true on **one** tagged commit `RC`:

| # | Requirement | Proof |
| --- | --- | --- |
| C1 | Fact-log invariants hold | Hermetic + Layer C suites covering: log-only next transition; no oneshot/timer settle; checkpoint ≠ model chooser; missing tool result once on resume; steer = `user.message`; tool-round cap = empty tools |
| C2 | L0–L1 + L3 (artifact features) + L4 | `just harness-convergence-check`; feature suites for the published Cargo set; Node/Python/Go discovery + alignment |
| C3 | L5 | `just layer-c-live-e2e` with bailian Flash pin → exact `LAYER_C_PASS` on `RC` |
| C4 | Integrated-use ledger | Dated E+E+I snapshot for `RC` (refresh if tip ≠ prior ledger commit) |
| C5 | L2 | F-kernel coverage re-run → `ALL_F_TABLE_KERNELS_GE_90` (stable-tag gate) |
| C6 | L6 | Fresh `performance.yml` + `hermetic-integrations.yml` digests for `RC` in PERFORMANCE_QUALIFICATION |
| C7 | L8 | §7 pins green **plus** new 9.0.0 fact-log pins (below) |
| C8 | L7 disposition | Harbor TB-QUAL1 + DM-PROD1 + CAR-01…05 **close receipts**, **or** one written product waiver naming deferred rows |
| C9 | Multi-channel cut | Git tag `v9.0.0`, GitHub Release, crates.io `a3s-code-core` 9.0.0, npm `@a3s-lab/code` 9.0.0, Python wheels/bootstrap, Go `sdk/go/v9.0.0`, website `docs/v9.0.0` |
| C10 | Honest CHANGELOG | States what shipped; Enterprise GA only if §2.2 true |

### 2.2 Enterprise GA DoD (stricter)

Channel release DoD **and** L7 rows closed with receipts (waiver path forbidden
for the GA claim). See ENTERPRISE_GA_E2E_PLAN §2.

### 2.3 Explicit non-goals for this cut

| Item | Rule |
| --- | --- |
| Apofasi / `typed_decisions` host shipping | Keep `[Unreleased]` unless product expands the cut and re-runs L3 with `apofasi` |
| Closing G1/G2 (umbrella recipe / F-table CI join) | Useful; **not** required to tag 9.0.0 |
| Redefining “done” as “fa0a92ca was green once” | Forbidden |

---

## 3. Phased roadmap

Execute in order. Do not start a later phase while an earlier exit criterion
fails, except where noted as parallel.

```text
P0 Scope freeze
  → P1 Tip freeze + invariant audit
  → P2 Re-qualify L0–L6 + ledger on RC
  → P3 L2 + L8 pins
  → P4 L7 close OR product waiver          ⎫ parallel owners after P2 starts
  → P5 Multi-channel publish + docs        ⎭ requires P0–P3 + P4 disposition
  → P6 (optional) Enterprise GA remainder
```

### P0 — Scope freeze (product)

**Owner:** product + Code maintainers  
**Actions:**

1. Choose RC base: freeze **current tip**, or rewind/cut from `fa0a92ca` and
   land tip work as 9.0.x.
2. Confirm Apofasi stays out of 9.0.0 (default: yes).
3. Choose end-state A (channel release with L7 waiver) or B (wait for L7 /
   claim GA).

**Exit:** Written decision in CHANGELOG Notes (or adjacent release note) naming
RC intent and L7 disposition path.

### P1 — Tip freeze and fact-log invariant audit

**Owner:** Code  
**Actions:**

1. Stop merging non-blocker work onto the RC branch.
2. Audit against CHANGELOG invariants (C1): oneshot/timer absence, checkpoint
   non-authority, resume/tool-once, steer, empty-tool cap.
3. Add or confirm hermetics under `core/src/agent_api/tests/fact_log.rs`,
   `core/tests/test_fact_log_real_llm.rs`, and effect package suites.

**Exit:** Checklist of C1 invariants mapped to named tests; failures fixed on
RC.

### P2 — Re-qualify L0–L6 and ledger on RC

**Owner:** Code  
**Actions (serial where Live):**

| Step | Command / workflow | Sink |
| --- | --- | --- |
| L0/L1 | `just harness-convergence-check` + local-code lib | FINAL / CI |
| L3 | Feature suites for published Cargo set | advanced-integ / L3 FINAL |
| L4 | Node `npm test`, Python pytest, Go `test ./...`, alignment | SDK logs |
| L5 | `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash just layer-c-live-e2e` | `LAYER_C_PASS` on **RC** SHA |
| Ledger | Refresh CAPABILITY_INTEGRATED_USE_LEDGER for RC | dated PASS |
| L6 | `gh workflow run performance.yml` + hermetic-integrations on RC | digests in PERFORMANCE_QUALIFICATION |

**Exit:** Every row cites **RC** SHA (not only `fa0a92ca`). FULL_FEATURE §13
L5/L6 cells updated to RC digests.

### P3 — L2 coverage and L8 pins

**Owner:** Code  
**Actions:**

1. `just f-table-cov` (or documented llvm-cov recipe) →
   `ALL_F_TABLE_KERNELS_GE_90`.
2. Extend FULL_FEATURE §7 with **9.0.0 fact-log pins**, at minimum:

| Pin | Mechanism | Hermetic evidence |
| --- | --- | --- |
| **9.0.0 log-only control** | Next transition only from fold | fact_log / effect harness |
| **9.0.0 park-until-fact** | Confirm/question not timer/oneshot | park suites |
| **9.0.0 resume tool-once** | Missing tool result runs once | resume hermetic |
| **9.0.0 empty-tool cap** | Cap is empty tool list | tool_round_cap |
| **9.0.0 a3s-vec FTS** | `a3s_vec_fts_v1`; old zvec gens rebuild | FTS generation tests |

3. Confirm prior §7 pins (8.5.x / 8.6.x / serve absence) still green on RC.

**Exit:** L2 FINAL present; §7 table includes 9.0.0 rows; pins green.

### P4 — L7 disposition

**Owner:** Harbor / Durable Memory / Cloud (external) + product for waiver  
**Fork:**

| Path | Required evidence | When to use |
| --- | --- | --- |
| **Close** | TB-QUAL1 full matrix close; DM-PROD1 host report; CAR-01…05 receipts in HARNESS_CONVERGENCE | End-state B / GA |
| **Defer** | Single product waiver listing each deferred ID and why Core green is insufficient substitute | End-state A only; **forbids** “Enterprise GA achieved” wording |

**Exit:** Either filled receipt table **or** waiver committed next to the
release notes. ENTERPRISE_GA board updated honestly.

### P5 — Multi-channel publish and docs

**Owner:** release workflow  
**Prerequisites:** P0–P3 exit + P4 disposition  
**Actions:**

1. Tag `v9.0.0` on RC; run `release.yml` (and language publish workflows).
2. Verify crates.io / npm / PyPI / Go module / GitHub Release all show 9.0.0
   (do not claim multi-channel cut if one channel fails — see 8.5.10 lesson).
3. Archive website docs to `docs/v9.0.0`; update current-line pointers.
4. CHANGELOG Notes: full multi-channel cut list; Enterprise GA status exact.

**Exit:** External registries match tag; docs line exists; README “Prefer
9.0.0” accurate.

### P6 — Enterprise GA remainder (optional track)

Only if P4 took the defer path or GA was never claimed.

**Actions:** Drive TB-QUAL1 / DM-PROD1 / CAR-01…05 to close; then update
CHANGELOG / README / ENTERPRISE_GA board to **achieved** with receipt links.
No new Core feature work required unless L7 fails expose a Code contract bug.

**Exit:** GA board all PASS with receipts.

---

## 4. Workstream owners (RACI sketch)

| Workstream | Responsible | Accountable | Consulted |
| --- | --- | --- | --- |
| Fact-log invariants / Core RC | Code | Code lead | Effect maintainers |
| L5 Flash Layer C | Code | Code lead | Model ACL owners |
| L6 Actions digests | Code CI | Code lead | — |
| L7 Harbor TB-QUAL1 | Harbor + Code runner | Product | TERMINAL_BENCH owners |
| L7 DM-PROD1 | Memory host owner | Product | Code |
| L7 CAR-01…05 | Cloud | Product | Code agent protocol |
| L7 waiver text | Product | Product | Code (must not invent) |
| Multi-channel publish | Release | Code lead | SDK owners |
| Docs `v9.0.0` | Docs | Code lead | — |

---

## 5. Suggested calendar shape (not a deadline)

Durations are capacity sketches, not SLAs.

| Phase | Typical span | Depends on |
| --- | --- | --- |
| P0 | Same day | Product decision |
| P1 | 1–2 days | Freeze |
| P2 | 2–4 days (L5 serial; L6 Actions queue) | P1 |
| P3 | 1 day parallel with late P2 | P2 L1 green |
| P4 close path | Days–weeks (external) | Can start after P2 L5 |
| P4 waiver path | Same day as P0 if chosen | Product |
| P5 | 1 day after P3 + P4 | Channels healthy |
| P6 | External cadence | Only if GA still open |

---

## 6. Locked decisions (2026-09-25 goal)

| Decision | Value | Notes |
| --- | --- | --- |
| End-state | **B — Enterprise GA** | L7 close receipts required; waiver path forbidden for the GA claim |
| RC base | **Current tip** (`91d34757` and successors on this line) | Re-qualify; do not ship on `fa0a92ca`-only digests |
| Apofasi | **Out of 9.0.0** | Remains `[Unreleased]` unless a later cut expands scope |
| Publish | Only after P0–P4 (close path) + P3 | No crates.io 9.0.0 before L7 receipts |

### C1 invariant → test map (P1)

| Invariant | Evidence |
| --- | --- |
| Log-only next transition; stored model turn not resent | `packages/effect` `a_text_turn_runs_once…`; Code `fact_log_session_send_and_resume_use_the_stored_model_turn`; `fact_log_session_stream_…` |
| Confirm/question park until fact (no timer/oneshot) | Effect `confirmation_parks_until_a_fact…`; `a_question_stays_parked…`; `file_reopen` parked suites |
| Missing tool result runs once on resume | Effect `a_failed_tool_is_not_recorded_and_resume_runs_it_once`; `failed_tool_without_a_result_runs_once_after_reopen` |
| Tool-round cap = empty tool list | Effect `tool_round_cap_completes_once_with_an_empty_tool_list` |
| Steer / attachments on the log | Code `fact_log_session_attachments_steer_and_history_stay_on_the_log` |
| Checkpoint is not the model chooser | Code resume with ignored checkpoint id still folds the log (`…use_the_stored_model_turn`) |

P1 exit for this goal: Effect suite green on tip (recorded 2026-09-25: 40 tests
passed). Code `fact_log` module tests must pass under local-code before L5.

## 7. Immediate next actions (execution)

1. ~~Product path~~ — locked to Enterprise GA above.
2. **Code:** keep RC tip freeze; land only blocker fixes.
3. **Code:** L0–L4 hermetic on tip → L5 Layer C → L6 Actions on tip SHA.
4. **External:** TB-QUAL1 full Harbor close; DM-PROD1 host pack; CAR-01/03/04/05
   Cloud/Box receipts into HARNESS_CONVERGENCE (no invented green).
5. **Release:** multi-channel `v9.0.0` only after L7 rows are Delivered.

---

## 8. Relationship map

```text
This roadmap                 ──► sequenced closure of 9.0.0
FULL_FEATURE_TEST_PLAN       ──► L0–L8 train, §7 pins, §13/§14 status
ENTERPRISE_GA_E2E_PLAN       ──► GA board + L7 fork rules
CAPABILITY_INTEGRATED_USE    ──► E+E+I ledger for RC
PERFORMANCE_QUALIFICATION    ──► L6 digest sink
HARNESS_CONVERGENCE          ──► L7 receipt sink
a3s-effect README            ──► control-model axioms behind C1
CHANGELOG [9.0.0]            ──► shipped product claims
```

When a phase exits, update FULL_FEATURE §13, ENTERPRISE_GA §5–§6, the ledger
date/SHA, and PERFORMANCE_QUALIFICATION in the same change set as the evidence.
Do not mark this roadmap “complete” until §2.1 (or §2.2) is proven on the
tagged commit.
