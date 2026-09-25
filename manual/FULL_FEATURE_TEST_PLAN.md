# A3S Code Full-Feature Test Plan (First Principles)

**Status:** planning contract + executed Core hermetic/L5 evidence (2026-09-21,
Core line `8.6.0`, post HARNESS-CONV5 slim)  
**Mission under test:** a **governed coding-agent harness** — loop, tools,
policy, events, retrieval, evidence, and the four official SDKs — not Desktop
chrome, not Cloud UI, not Use package markets.

**Live model pin (this plan):**
`A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` against monorepo
`./.a3s/config.acl`. The ACL does **not** declare that exact id; Layer C
remaps it to declared `boyue/bailian/deepseek-v4.1-flash` via
`core/tests/support/layer_c_model.rs`. Recipe default without override is
`boyue/deepseek-v4-flash` (`default_model` in the same ACL). Both Flash peers
are valid Layer C pins; this plan **requires** the bailian remapped route for
release-qualification L5 runs.

This plan does **not** replace:

- [FIRST_PRINCIPLES_TEST_CASES.md](FIRST_PRINCIPLES_TEST_CASES.md) (F01–F30)
- [FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md) (Layers A–D)
- [CAPABILITY_VERIFICATION.md](CAPABILITY_VERIFICATION.md) (evidence ledger)
- [CAPABILITY_INTEGRATED_USE_LEDGER.md](CAPABILITY_INTEGRATED_USE_LEDGER.md)
  (29-id effective / efficient / integrated-use snapshot)
- [SDK_CAPABILITY_MATRIX.md](SDK_CAPABILITY_MATRIX.md) (`sdk_capabilities()`)
- [test-cases/README.md](test-cases/README.md) (unit / integration / soak cases)

It **composes** them into one executable full-feature program: every public
product capability must have named evidence at the right layer, or an explicit
external-owner gap.

---

## 0. Why three test kinds (first principles)

A3S Code fails in three different ways. One suite cannot catch all three.

| Kind | Question it answers | What it must never become |
| --- | --- | --- |
| **Unit** | Does this module keep one invariant under controlled inputs? | A full agent boot with a real model |
| **Integration** | Do two or more modules keep a joint invariant across a real boundary? | A soak, or “the model said OK” |
| **End-to-end (E2E)** | Does the shipped harness loop, with a real provider, still produce kernel effects under policy? | Required CI; a substitute for hermetic twins |

Derivation:

1. **Correctness is local first.** Digests, schemas, typed errors, and
   fail-closed defaults live in single modules. If unit coverage is thin,
   live E2E only amplifies flaky prose.
2. **Safety is compositional.** Permissions × tools × sandbox × verify gate
   only fail when wired. Hermetic integration owns that seam.
3. **Provider risk is real but non-deterministic.** Tool-name empty streams,
   leaked markup, cancel races, and MCP progress need a live Flash pin — but
   every live branch has a non-LLM twin, or the live test is banned.
4. **Thin default is a product invariant.** `local-code` must not pull
   evaluation / research / Chromium / S3. Absence tests are first-class.
5. **Host-owned work stays host-owned.** Use, Harbor, CAR, Desktop: Code
   proves the contract and fail-closed without the host. It does not fake
   qualification.

---

## 1. First principles (evidence rules)

### 1.1 What “full feature” means

Full feature = every **product capability** returned by
`a3s_code_core::sdk_capabilities()` (baseline + advanced), plus the Core
mechanisms that make those capabilities safe:

| Axis | In scope | Out of scope |
| --- | --- | --- |
| Product | `sdk_capabilities()` IDs | Marketing copy, Desktop layouts |
| Host | Extension traits / host injection contracts | CLI TUI chrome, Cloud consoles |
| Correctness | Digests, tool names, terminal states, fail-closed | “Model prose said OK” |
| Languages | Rust Core + Node + Python + Go | Unofficial bindings |
| Environments | Hermetic CI, live Flash pin, Harbor / soak | Customer prod accounts |
| Removed (do not retest as product) | Filesystem-first `serve` / `AgentDir` HTTP serve | Historical CHANGELOG mentions only |

### 1.2 Evidence rules (non-negotiable)

1. **Kernel effect, not wording** — assert digests, schemas, sandbox writes,
   retention gaps, typed errors.
2. **Hermetic twin first** — every live/provider suite has a non-LLM twin for
   the same branch.
3. **Activation + absence** — enabled surfaces appear; disabled/default-thin
   surfaces stay absent (`local-code` tree, no model-visible `parallel_task`).
4. **Fail-closed defaults** — sandbox, mutation verify gate, Active-only
   memory, process-host opt-in, SSRF / Fake-IP.
5. **SDK parity** — a capability claimed on one language is constructed through
   every official wrapper (`scripts/sdk_api_alignment_check.mjs`).
6. **Ignored ≠ green** — `#[ignore]` / credentialed / Harbor / soak are
   **Layer E/F / L5–L7**, never counted as Required CI.
7. **Coverage claim** — “≥95%” means **F-table kernel files**, not crate TOTAL
   (`just f-table-cov` / `scripts/f_table_coverage.sh`).
   (see F-cases doc).
8. **Post-slim honesty** — Cargo feature `server` means `local-code` + `s3` +
   `telemetry` (profile). It is **not** AgentDir HTTP serve. Do not plan
   serve-lifecycle E2E.

### 1.3 Verification dimensions

Reuse the CAPABILITY_VERIFICATION model for every capability:

Public contract · Activation · Correctness · Failure safety · Lifecycle ·
Concurrency · Resources · Performance (separate) · SDK parity · Documentation.

---

## 2. Model pin contract (Layer C / L5)

Source of truth: monorepo `./.a3s/config.acl` + `core/tests/support/layer_c_model.rs`.

| Symbol | Value | Role |
| --- | --- | --- |
| ACL `default_model` | `boyue/deepseek-v4-flash` | Declared default; `just layer-c-live-e2e` fallback |
| Declared bailian Flash | `boyue/bailian/deepseek-v4.1-flash` | Alternate peer for model-switch E2E |
| **This plan’s L5 pin** | `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` | Remaps → `boyue/bailian/deepseek-v4.1-flash` |
| Loader constants | `REQUIRED_DEFAULT_MODEL`, `ALTERNATE_FLASH_MODEL` | Asserted by live suites |

Operator command for this plan’s L5:

```bash
cd crates/code
export A3S_CONFIG_FILE="$(git rev-parse --show-toplevel)/.a3s/config.acl"
export A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash
export A3S_LAYER_C_EVIDENCE=/tmp/a3s-layer-c-bailian-flash-plan
just layer-c-live-e2e
# Expect FINAL.txt: LAYER_C_PASS and logs show remapped
# boyue/bailian/deepseek-v4.1-flash
```

Rules:

- Never assert provider prose; pin the model id only.
- Never count L5 as Required CI.
- Model-switch live tests must keep both Flash peers declared in ACL.
- Do not add a third “goal model” without updating `layer_c_model.rs` and this
  table in the same change.
- Typed System-1 E2E uses this same pin but must not treat DeepSeek as the
  decision engine. See [TYPED_DECISION_E2E_PLAN.md](TYPED_DECISION_E2E_PLAN.md).

---

## 3. Capability universe (source of truth)

Discover at runtime (do not hand-maintain a second list):

```bash
cargo test -p a3s-code-core --lib sdk_capabilities -- --nocapture
node -e 'const c=require("@a3s-lab/code"); console.log(c.sdkCapabilities().map(x=>x.id).join("\n"))'
```

### 3.1 Baseline (`local-code`) — must ship in default product

| Capability ID | Unit (module) | Integration (boundary) | Live E2E (L5 / Layer C) | SDK |
| --- | --- | --- | --- | --- |
| `agent_runtime` | session/agent unit; close | session open/close integ | cluster features | Node/Python/Go create/close |
| `conversation` | agent stream unit | stream + cancel hermetic | cluster / run_control | stream + cancel fixtures |
| `governed_tools` | registry + confirmation + hooks | governed host call | `test_harness_capabilities_live_e2e` | `governedTool` / Go `GovernedTool` |
| `workspace_tools` | read/edit/bash/git/download/batch/grep | capabilities hermetic | capabilities live | workspace helpers |
| `web_search` | HTTP/RSS unit | safe_http join | capabilities when exercised | tool path |
| `web_fetch` | fetch + Fake-IP/DoH | safe_http join | capabilities live | — |
| `workspace_retrieval` | FTS/BM25/hybrid hermetic | retrieval integ | search + retrieval live | `test_workspace_retrieval*` |
| `context_memory` | durable_memory Active-only | memory refresh hermetic | `test_memory_store_real_llm` | memory methods |
| `planning_delegation` | task/subagent; no `parallel_task` | permission inheritance | auto_delegation live | `task` / `tasks` |
| `program` | QuickJS program unit | `test_program_script_quickjs_integration` | capabilities when exercised | program options |
| `priority_scheduling` | task_scheduler unit | scheduler integ | — (hermetic sufficient) | scheduler APIs |
| `persistence` | session_checkpoint; flock | checkpoint recovery integ | cluster save/resume | checkpoint export fixtures |
| `governance` | permissions, hitl, budget, verify | prompt boundaries | prompt + harness loop | confirmations/hooks |
| `run_control` | run_control unit | `run_control_runtime` | `test_run_control_real_llm` | steer/interrupt |
| `model_adapters` | llm/openai unit; base_url join | config load | config/cluster live | ACL / CodeConfig |
| `structured_output` | generate_object / structured | schema fail-closed | `test_structured_json_real_llm` | object options |
| `mcp_and_skills` | mcp stdio + skill registry | skill capability integ | issue-form MCP live | MCP examples |
| `run_observability` | event_protocol; harness_evidence | event page bounds | all live streams | snapshots/pages |

### 3.2 Advanced (explicit Cargo / host gate)

| Capability ID | Feature / host | Unit / hermetic | Live / external |
| --- | --- | --- | --- |
| `code_intelligence` | host language service | fake LSP integ + tool unit | PERF CI profile |
| `cognitive_packages` | Use host injection | capability_set / projection | host CAR / Use |
| `use_runtime_tasks` | Use host injection | capability scope / Use lease | host |
| `programmable_workflows` | `advanced-harness` (`dynamic-workflow`) | dynamic_workflow_* | extensibility live |
| `state_graph` | `advanced-harness` | `test_state_graph_integration` | PERF replay |
| `agent_release_contract` | always compiled | `agent_release_manifest` | release workflow |
| `agent_protocol` | always compiled | agent_protocol_* | `test_agent_protocol_live_e2e` |
| `evaluation_substrate` | `advanced-harness` | evaluation_* | host eval policy |
| `typed_decisions` | `apofasi` (not in `local-code`) | `typed_decision` lib tests | [TYPED_DECISION_E2E_PLAN.md](TYPED_DECISION_E2E_PLAN.md) TD-C..TD-E |
| `moli_runtime` | `headless-search` | `test_web_search_headless` | soak / packaging |
| `s3_workspace` | `s3` (also via profile `server`) | `test_s3_backend` | hermetic-integrations |
| `opentelemetry` | `telemetry` (also via profile `server`) | OTEL unit + collector-down | hermetic collector gate |

Research wire values remain a compiled contract under `feature = "research"`
([RESEARCH_CONTRACTS.md](RESEARCH_CONTRACTS.md)); they are **not** a
`sdk_capabilities()` id unless `CAPABILITY_SPECS` gains one.

### 3.3 Safety kernels (not capability ids)

| Kernel | Protects | Unit / integ home |
| --- | --- | --- |
| Effect isolation / orphan `.a3s-isolate-*` | workspace writes | `effect_isolation.rs` |
| Native sandbox + process-host opt-in | `bash` | `sandbox/*`; live native + issue-form |
| Safe HTTP / Fake-IP / SSRF | fetch/search/download | `safe_http` + live capabilities |
| Mutation verify gate | harness completion | `verification.rs` + harness_loop live |
| Active-only memory | durable writes | `candidate_write_stays_inactive` |
| No model-visible `parallel_task` | thin registry | Layer A3 / registry lookup |

---

## 4. Unit / integration / E2E taxonomy

### 4.1 Unit (`U-*` in test-cases/)

**Definition:** one module, no network, no real model. Temp dirs allowed.

**Must cover for every baseline capability:**

- Happy path kernel effect
- At least one fail-closed / typed-error path
- Schema or digest identity where the module owns one
- Absence / non-registration when the surface is thin-default

**Command surface:**

```bash
cargo test -p a3s-code-core --no-default-features --features local-code --lib
cargo test -p a3s-code-core --lib   # default local-code
```

**Catalog:** [test-cases/baseline-*.md](test-cases/), [retrieval.md](test-cases/retrieval.md),
[advanced.md](test-cases/advanced.md) `U-*` rows (~200 case specs total across
unit+integration+soak).

### 4.2 Integration (`I-*`, hermetic unless `I-*-L*`)

**Definition:** crosses a boundary without a provider (session+tool+policy,
protocol+harness, SDK↔Core, workspace backend, MCP fixture process).

**Must cover:**

- Permission before side effect
- Sandbox / isolation bind before bash write
- Checkpoint resume identity
- SDK construction parity for every claimed API
- Feature-off fail-closed for Advanced ids

**Command surface:**

```bash
just harness-convergence-check          # A1–A4 thin + SDK align
cargo test -p a3s-code-core --test agent_protocol_v1
cargo test -p a3s-code-core --test session_checkpoint_v1
cargo test -p a3s-code-core --features advanced-harness --test test_state_graph_integration
cargo test -p a3s-code-core --features s3 --test test_s3_backend
node scripts/sdk_api_alignment_check.mjs
# SDK runtime: see sdk-runtime CI / package test scripts
```

### 4.3 End-to-end live (`I-*-L*` / Layer C)

**Definition:** real provider, ignored, serial, pinned Flash, kernel-effect
oracles only.

**Default Layer C suite list** (`just layer-c-live-e2e`):

1. `test_native_sandbox_live_e2e`
2. `test_issue_fix_live_e2e`
3. `test_agent_protocol_live_e2e`
4. `test_harness_loop_live_e2e`
5. `test_harness_capabilities_live_e2e`
6. `test_deepseek_adversarial_e2e`
7. `test_update_plan_live_e2e`
8. `test_prompt_capability_real_llm`
9. `test_orchestration_real_llm`
10. `test_long_horizon_real_llm`
11. `test_structured_json_real_llm`
12. `test_run_control_real_llm`
13. `test_workspace_search_real_llm`
14. `test_workspace_retrieval_real_llm`
15. `test_context_tools_real_llm`
16. `test_workflow_facade_real_llm`
17. `test_auto_delegation_real_parallel`
18. `test_ultracode_discretion_real_llm`
19. `test_real_config_env_integration`
20. `test_real_llm_cluster_features`
21. `test_memory_store_real_llm`
22. optional `test_extensibility_real_llm` (`advanced-harness`)

**Not in default Layer C (explicit release checklist when shipping):**

- `headless-search` / Moli packaging soak
- `s3` / MinIO hermetic-integrations (prefer L3/L6 hermetic)
- Harbor TB-QUAL1, DM-PROD1, CAR-01…05 (L7)

### 4.4 Soak (`S-*`)

Repetition, duration, crash/restart, or resource bound. Not “bigger timeout
unit”. Catalog: [test-cases/soak.md](test-cases/soak.md). Umbrella recipe is
still gap G7.

---

## 5. Execution layers (full program)

| Layer | Name | Gate | Command / owner |
| --- | --- | --- | --- |
| **L0** | Contract discovery | PR | `sdk_api_alignment_check.mjs` + `sdk_capabilities` golden order |
| **L1** | Thin hermetic Core | PR (Required CI) | `just harness-convergence-check` + `cargo test -p a3s-code-core --lib` (local-code) |
| **L2** | F-table kernel coverage | Release / scheduled | llvm-cov F01–F30 kernels ≥95% (`just f-table-cov`) |
| **L3** | Advanced hermetic | Path / feature CI | `advanced-harness`, `s3`, `telemetry`, `headless-search` suites |
| **L4** | SDK runtime parity | PR / `sdk-runtime.yml` | Node/Python/Go unit + capability/checkpoint/immutable fixtures |
| **L5** | Live provider E2E | Manual / release | `just layer-c-live-e2e` with **bailian Flash pin** (§2) |
| **L6** | Performance + hermetic integrations | Release qual | `performance.yml`, `hermetic-integrations.yml` |
| **L7** | External qualification | External owner | Harbor TB-QUAL1, DM-PROD1, CAR-01…05, retrieval soak |
| **L8** | Version regression pins | Every cut that touches the bug | Named hermetics for shipped fixes (§7) |

Orchestration recipes already in-tree:

```bash
just harness-convergence-check   # L1 / Layer A1–A4
just layer-c-live-e2e            # L5 / Layer C (set A3S_TEST_MODEL for bailian)
just test                        # broad Core suite
just test-cov                    # coverage UX
```

**Missing recipe (gap G1):** a single `just full-feature-hermetic` that runs
L0+L1+L3(default features for the artifact)+L4 without network.

---

## 6. Host / product surfaces beyond Core crates

| Surface | Owner | Code-owned evidence | Not Code’s job |
| --- | --- | --- | --- |
| `a3s code` TUI | CLI | Core session/tool contracts used by CLI hermetics | Panel chrome, slash UX |
| Desktop workbench | Desktop | SDK host contract + capability digests | Windowing, branding |
| Cloud harness | Cloud | Agent protocol + CAR fixtures | Multi-tenant policy |
| Harbor Terminal-Bench | Harbor + Code runner | [TERMINAL_BENCH.md](TERMINAL_BENCH.md) | Cluster fleet |

Full-feature **Code** testing stops at the host boundary: prove the contract,
fail closed without the host, and record host qualification under L7.

---

## 7. Version regression pins (shipped product hazards)

These are first-class cases because they broke real providers or builds:

| Pin | Mechanism | Hermetic evidence | Live |
| --- | --- | --- | --- |
| **8.6.0 image `read`** | JPEG/PNG/GIF/WebP tool results stay `image_url` on the OpenAI-compatible path | `tools/builtin/read.rs`, `llm/openai.rs` (#156) | Optional |
| **8.6.0 orphan isolate** | Empty `.a3s-isolate-*` sibling is cleared before bind | `effect_isolation.rs` `bind_clears_a_stale_empty_isolation_directory` (#155) | — |
| **8.6.0 slim serve removal** | No AgentDir HTTP serve / filesystem-first mode in Core or SDKs | tree/API absence + CHANGELOG; do not revive serve suites | — |
| **8.5.12 batch schema** | No application `$ref` objects under `batch` parameters `examples` | `tools/builtin/batch.rs` schema asserts (#147) | Optional: GLM Coding turn with `batch` presented |
| **8.5.11 musl** | `a3s-sandbox` 0.1.4 rlimit typing | Node musl optional native build in Release | Release matrix |
| **8.5.10 Gate 7 + Fake-IP** | Native sandbox + DoH fallback | `safe_http` + `test_native_sandbox_live_e2e` | Layer C capabilities |
| **Leaked tool markup** | WorkBuddy / Claude / DSML recovery | `llm::text_tool_calls` | issue-form / Flash |
| **Unverified mutation** | Completion gate | verification / harness_loop unit | `test_harness_loop_live_e2e` |
| **Session reopen flock** | Cross-process reopen | session persistence unit | cluster resume |
| **9.0.0 log-only control** | Next transition only from fact-log fold; stored model turn not resent | `packages/effect` text-turn/resume; `core/src/agent_api/tests/fact_log.rs` send/stream/resume | Layer C |
| **9.0.0 park-until-fact** | Confirm/question settle only on answer facts | Effect confirmation/question + file_reopen park suites | — |
| **9.0.0 resume tool-once** | Missing tool result runs once on resume | Effect failed-tool resume + file_reopen | — |
| **9.0.0 empty-tool cap** | Tool-round cap is empty tool list on next completion | `packages/effect/tests/tool_round_cap.rs` | — |
| **9.0.0 a3s-vec FTS** | `a3s_vec_fts_v1`; incompatible `zvec_rust_fts_v1` rebuilds | Workspace FTS generation / retrieval hermetics | — |

Add new pins to this table on every user-visible fix; do not rely on CHANGELOG
alone.

---

## 8. Explicit gaps (honest backlog)

| ID | Gap | Why it matters | Closure |
| --- | --- | --- | --- |
| G1 | No `just full-feature-hermetic` umbrella | Operators reassemble L0–L4 by hand | Add recipe; wire to CI optional job |
| G2 | F-table IDs ≠ `sdk_capabilities()` IDs | Two matrices drift | Keep §3 as join table; CI check that every baseline ID appears in §3 |
| G3 | Advanced live not in default Layer C | Feature suites optional | Release checklist: extensibility (`advanced-harness`); headless/S3 via L3/L6 — **not** removed AgentDir serve |
| G4 | GLM Coding live not in Layer C pin | batch schema was provider-specific | Keep hermetic pin; optional L5 provider matrix row |
| G5 | Harbor / CAR / DM-PROD are external | Cannot fake in Core CI | Keep L7 owners; never mark full-feature complete without their latest receipt or explicit waiver |
| G6 | Performance numbers age | Runner variance | Re-run `performance.yml` on release candidates; update PERFORMANCE_QUALIFICATION digests |
| G7 | No single soak command | Unit and integration cases do not prove resource bounds | [test-cases/soak.md](test-cases/soak.md); only retrieval soak and memory restart endurance are wired |
| G8 | Stale docs still mention serve suites | Misleads operators after slim | Primary manuals scrubbed 2026-09-20 (E2E B4, test-cases README/soak, SDK matrix, CAPABILITY_VERIFICATION Node row). Residual CHANGELOG history may still mention serve — keep as history only |
| G9 | `S-RX-01` evidence-store soak blocked | No inventable append-only fact store | Keep blocked; do not invent a store for coverage theater |

As of `8.6.0` post-slim, G1–G9 are **program gaps** (orchestration, join, soak
wiring, doc scrub, external owners). They are not a claim that every case in
[test-cases/](test-cases/README.md) already has a named passing test.

---

## 9. Train order (how to run a full pass)

1. **L0–L1** on every PR (`harness-convergence-check` + local-code lib).
2. **L4** SDK runtime on every PR that touches bindings or Core public API.
3. **L2** F-kernel coverage before a stable tag (or weekly).
4. **L3** for any feature flag in the release Cargo features set.
5. **L5** `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash just layer-c-live-e2e`
   against monorepo `.a3s/config.acl`; archive evidence dir `FINAL.txt`.
6. **L6** performance + hermetic-integrations on RC.
7. **L7** Harbor / host receipts before calling a channel “qualified”.
8. **L8** confirm version pins for the delta since last tag.

Pass criteria for “full-feature hermetic green”:

- L0, L1, L4 green
- L3 green for every feature enabled in the artifact under test
- L2 green if this is a coverage-gated release
- L8 pins for that version present and green
- No revival of removed serve/AgentDir product tests

Pass criteria for “full-feature release qualified”:

- Hermetic criteria above, **plus** L5 `LAYER_C_PASS` under the bailian Flash
  pin (§2), L6 digests current, L7 receipts or written waivers.

---

## 10. Evidence layout

| Layer | Evidence location |
| --- | --- |
| L1 / A | CI logs; optional `/tmp/a3s-layer-a-*/FINAL.txt` |
| L2 | `/tmp/a3s-llvm-cov-*/FINAL.txt` (`ALL_F_TABLE_KERNELS_GE_95`) |
| L5 / C | `$A3S_LAYER_C_EVIDENCE` or `/tmp/a3s-layer-c-live/FINAL.txt` |
| L6 | Actions artifacts + PERFORMANCE_QUALIFICATION.md |
| L7 | Harbor / Cloud / host report paths in HARNESS_CONVERGENCE.md |

Never promote a draft GitHub Release or crates.io cut on L5 failure without an
explicit product decision recorded next to the tag.

---

## 11. Inventory snapshot (planning audit, 2026-09-20)

Authoritative counts used while writing this plan (working tree):

| Surface | Count / fact |
| --- | --- |
| `sdk_capabilities()` baseline+advanced | 30 ids in `CAPABILITY_SPECS` |
| `core/tests/*.rs` | 106 integration/live/soak entrypoints |
| Case catalog `### U-/I-/S-*` | runtime 32 + execution 44 + model 34 + retrieval 12 + advanced 45 + soak 33 = **200** |
| Layer C default suites | 21 + optional extensibility |
| ACL Flash peers | `boyue/deepseek-v4-flash`, `boyue/bailian/deepseek-v4.1-flash` |
| Removed product surface | AgentDir HTTP `serve` / filesystem-first (absent from Cargo product deps; profile `server` ≠ serve) |
| F01–F30 + Layer A/C recipes | Exist; last recorded PASS in FIRST_PRINCIPLES_TEST_CASES status table |

---

## 12. Relationship map

```text
sdk_capabilities()          ──► §3 capability universe
FIRST_PRINCIPLES_TEST_CASES ──► F01–F30 kernels (L2)
FIRST_PRINCIPLES_E2E        ──► Layers A–D ↔ L1/L3/L5/L7
CAPABILITY_VERIFICATION     ──► dimension checklist + ledger
CAPABILITY_INTEGRATED_USE   ──► dated 29-id E+E+I snapshot
SDK_CAPABILITY_MATRIX       ──► language entrypoints
THIS PLAN                   ──► join + unit/integ/E2E taxonomy + train + gaps + pin
ENTERPRISE_GA_E2E_PLAN      ──► L0–L8 GA board + TD-PERF complex Flash latency
V9_0_0_COMPLETION_ROADMAP   ──► sequenced 9.0.0 fact-log channel/GA closure
test-cases/                 ──► unit / integration / soak case oracles
layer_c_model.rs            ──► L5 Flash pin + bailian typo remap
```

For finishing the 9.0.0 cut itself, follow
[V9_0_0_COMPLETION_ROADMAP.md](V9_0_0_COMPLETION_ROADMAP.md) (scope freeze →
re-qualify tip → L7 disposition → multi-channel publish).

---

## 13. Status

| Item | State |
| --- | --- |
| This plan file | **Rewritten** for post-slim first principles + bailian Flash L5 pin |
| Deep case catalog | **Specified** in [test-cases/README.md](test-cases/README.md). Rows are cases, not green tests. |
| F01–F30 + Layer C recipes | **Exist**; Layer C execution PASS 2026-09-21 under bailian Flash (`/tmp/a3s-layer-c-bailian-flash-combined/FINAL.txt`) |
| L0/L1 hermetic | **PASS** `/tmp/a3s-goal-integ-use/layer-a.FINAL.txt` (re-verify EXIT:0; 3632 + SDK alignment). Prior: `/tmp/a3s-layer-a-bailian-exec/FINAL3.txt` |
| L3 feature hermetics | **PASS** `/tmp/a3s-l3-hermetic/FINAL.txt` (+ ignored s3/otel green; advanced-integ recheck under `/tmp/a3s-goal-integ-use/`) |
| L4 SDK | **PASS** Node/Python/Go discovery 29 caps; alignment ok in Layer A |
| L5 Layer C bailian Flash | **PASS** `/tmp/a3s-layer-c-bailian-flash-combined/FINAL.txt` (`LAYER_C_PASS`, 22/22 final) |
| 29-cap integrated use | **PASS** [CAPABILITY_INTEGRATED_USE_LEDGER.md](CAPABILITY_INTEGRATED_USE_LEDGER.md) |
| L2 F-kernel cov | **Prior PASS** (not re-measured this execution) |
| L6/L7 | L6 **PASS**: `performance.yml` [`35639843682`](https://github.com/A3S-Lab/Code/actions/runs/35639843682) + hermetic-integrations [`35642286881`](https://github.com/A3S-Lab/Code/actions/runs/35642286881). L7 Harbor/CAR **Not claimed**. See [ENTERPRISE_GA_E2E_PLAN.md](ENTERPRISE_GA_E2E_PLAN.md) |
| `just full-feature-hermetic` | **Gap G1** — not implemented |
| Baseline ID ↔ F-table CI join | **Gap G2** — manual join in §3 only |
| Serve-doc scrub | **G8 mostly closed** for active manuals; CHANGELOG history may retain serve notes |
| External L7 | **Owner-gated** — not substituted by Core green |
| Release cut | **Blocked** until §14 remaining gates close (or product waiver) |

When G1/G2 close, update this status table and add the recipe/check under
`justfile` / CI — do not mark an *implementation* goal complete by inventing
green. L5 execution for the bailian Flash pin is recorded above.

---

## 14. Release handoff (remaining gates)

Core full-feature **hermetic + L5 + integrated-use** is closed for this line.
A formal crates.io / GitHub Release still requires:

| Gate | Owner action | Evidence sink |
| --- | --- | --- |
| L2 | Re-run F-kernel coverage before a **stable** tag (or weekly) | `/tmp/a3s-llvm-cov-*/FINAL.txt` → `ALL_F_TABLE_KERNELS_GE_90` |
| L6 | RC: `performance.yml` + `hermetic-integrations.yml` | Update [PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md) digests |
| L7 | Harbor TB-QUAL1, DM-PROD1, CAR-01…05 — or written waiver | Paths in [HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md) |
| L8 | Confirm version regression pins for the delta since last tag | Named hermetics in §7 |
| Product | Land post-slim working tree; bump version if cutting past `8.6.0` | CHANGELOG + tag |

Do **not** promote a draft Release or crates.io cut on L5 failure without an
explicit product decision next to the tag. Current Core green does **not**
substitute L6/L7.
