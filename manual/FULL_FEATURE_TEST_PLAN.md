# A3S Code Full-Feature Test Plan (First Principles)

**Status:** planning contract (2026-09-17, Core line `8.5.12`)  
**Mission under test:** a **governed coding-agent harness** — loop, tools,
policy, events, retrieval, evidence, and the four official SDKs — not Desktop
chrome, not Cloud UI, not Use package markets.

This plan does **not** replace:

- [FIRST_PRINCIPLES_TEST_CASES.md](FIRST_PRINCIPLES_TEST_CASES.md) (F01–F30)
- [FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md) (Layers A–D)
- [CAPABILITY_VERIFICATION.md](CAPABILITY_VERIFICATION.md) (evidence ledger)
- [SDK_CAPABILITY_MATRIX.md](SDK_CAPABILITY_MATRIX.md) (`sdk_capabilities()`)

It **composes** them into one executable full-feature program: every public
product capability must have named evidence at the right layer, or an explicit
external-owner gap.

---

## 1. First principles

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

### 1.2 Evidence rules (non-negotiable)

1. **Kernel effect, not wording** — assert digests, schemas, sandbox writes,
   retention gaps, typed errors.
2. **Hermetic twin first** — every live/provider suite has a non-LLM twin for
   the same branch.
3. **Activation + absence** — enabled surfaces appear; disabled/default-thin
   surfaces stay absent (`local-code` tree, no `parallel_task`).
4. **Fail-closed defaults** — sandbox, mutation verify gate, Active-only
   memory, process-host opt-in, SSRF / Fake-IP.
5. **SDK parity** — a capability claimed on one language is constructed through
   every official wrapper (`scripts/sdk_api_alignment_check.mjs`).
6. **Ignored ≠ green** — `#[ignore]` / credentialed / Harbor / soak are
   **Layer E/F**, never counted as Required CI.
7. **Coverage claim** — “≥90%” means **F-table kernel files**, not crate TOTAL
   (see F-cases doc).

### 1.3 Verification dimensions

Reuse the CAPABILITY_VERIFICATION model for every capability:

Public contract · Activation · Correctness · Failure safety · Lifecycle ·
Concurrency · Resources · Performance (separate) · SDK parity · Documentation.

---

## 2. Capability universe (source of truth)

Discover at runtime (do not hand-maintain a second list):

```bash
cargo test -p a3s-code-core --lib sdk_capabilities -- --nocapture
node -e 'const c=require("@a3s-lab/code"); console.log(c.sdkCapabilities().map(x=>x.id).join("\n"))'
```

### 2.1 Baseline (`local-code`) — must ship in default product

| Capability ID | F-table / primary hermetic | Live Layer C | SDK fixture |
| --- | --- | --- | --- |
| `agent_runtime` | session/agent unit; close lifecycle | cluster features | Node/Python/Go session create/close |
| `conversation` | agent stream unit | cluster / run_control | stream + cancel fixtures |
| `governed_tools` | tools registry + governed unit | `test_harness_capabilities_live_e2e` | `governedTool` / Go `GovernedTool` |
| `workspace_tools` | builtin file/shell/git/download/**batch** | capabilities live | workspace helpers |
| `web_search` | web_search unit (HTTP/RSS) | capabilities when exercised | Tool path |
| `web_fetch` | web_fetch + safe_http (Fake-IP/DoH) | capabilities live | — |
| `workspace_retrieval` | retrieval hermetic + F16/F17 | retrieval/search live | `test_workspace_retrieval*` |
| `context_memory` | durable_memory_* hermetic | `test_memory_store_real_llm` | memory methods |
| `planning_delegation` | task/subagent; no `parallel_task` | auto_delegation live | `task` / `tasks` |
| `program` | program / QuickJS integration | capabilities when exercised | program options |
| `priority_scheduling` | task_scheduler unit | — | scheduler APIs |
| `persistence` | session_checkpoint_*; flock reopen | cluster save/resume | checkpoint export fixtures |
| `governance` | permissions, hitl, budget, verify | prompt + harness loop | confirmations/hooks |
| `run_control` | run_control_runtime | `test_run_control_real_llm` | steer/interrupt |
| `model_adapters` | llm/openai unit; base_url join | config/cluster live | ACL / CodeConfig |
| `structured_output` | structured / generate_object | `test_structured_json_real_llm` | object options |
| `mcp_and_skills` | mcp stdio + skill registry | issue-fix MCP live | MCP examples |
| `run_observability` | event_protocol; harness_evidence | all live streams | snapshots/pages |

### 2.2 Advanced (explicit Cargo / host gate)

| Capability ID | Feature / host | Hermetic | Live / external |
| --- | --- | --- | --- |
| `code_intelligence` | host language service | code_intelligence unit | PERF CI profile |
| `cognitive_packages` | Use host injection | capability_set / projection | host CAR / Use |
| `use_runtime_tasks` | Use host injection | capability scope / Use lease | host |
| `programmable_workflows` | `advanced-harness` | dynamic_workflow_* | extensibility live |
| `state_graph` | `advanced-harness` | `test_state_graph_integration` | PERF replay |
| `agent_release_contract` | always compiled | `agent_release_manifest` | release workflow |
| `agent_protocol` | always compiled | agent_protocol_* | `test_agent_protocol_live_e2e` |
| `evaluation_substrate` | `advanced-harness` | evaluation_* | host eval policy |
| `moli_runtime` | `headless-search` | `test_web_search_headless` | soak / packaging |
| `s3_workspace` | `s3` / `server` | `test_s3_backend` | hermetic-integrations |
| `filesystem_agent_server` | `serve` | serve unit | `test_serve_agent_dir_real_llm` |
| `opentelemetry` | `telemetry` / `server` | OTEL unit | hermetic collector gate |

---

## 3. Execution layers (full program)

| Layer | Name | Gate | Command / owner |
| --- | --- | --- | --- |
| **L0** | Contract discovery | PR | `sdk_api_alignment_check.mjs` + `sdk_capabilities` golden order |
| **L1** | Thin hermetic Core | PR (Required CI) | `just harness-convergence-check` + `cargo test -p a3s-code-core --lib` (local-code) |
| **L2** | F-table kernel coverage | Release / scheduled | llvm-cov F01–F30 kernels ≥90% ([TEST_CASES](FIRST_PRINCIPLES_TEST_CASES.md)) |
| **L3** | Advanced hermetic | Path / feature CI | `advanced-harness`, `s3`, `serve`, `headless-search` suites |
| **L4** | SDK runtime parity | PR / `sdk-runtime.yml` | Node/Python/Go unit + capability/checkpoint/immutable fixtures |
| **L5** | Live provider E2E | Manual / release | `just layer-c-live-e2e` (Flash pin; serial; ignored) |
| **L6** | Performance + hermetic integrations | Release qual | `performance.yml`, `hermetic-integrations.yml` |
| **L7** | External qualification | External owner | Harbor TB-QUAL1, DM-PROD1, CAR-01…05, retrieval soak |
| **L8** | Version regression pins | Every cut that touches the bug | Named hermetics for shipped fixes (see §5) |

Orchestration recipes already in-tree:

```bash
just harness-convergence-check   # L1 / Layer A1–A4
just layer-c-live-e2e            # L5 / Layer C
just test                        # broad Core suite
just test-cov                    # coverage UX
```

**Missing recipe (gap G1):** a single `just full-feature-hermetic` that runs
L0+L1+L3(default features)+L4 without network. Track until added.

---

## 4. Host / product surfaces beyond Core crates

| Surface | Owner | Code-owned evidence | Not Code’s job |
| --- | --- | --- | --- |
| `a3s code` TUI | CLI | Core session/tool contracts used by CLI hermetics | Panel chrome, slash UX |
| Desktop workbench | Desktop | SDK host contract + capability digests | Windowing, branding |
| Cloud harness | Cloud | Agent protocol + CAR fixtures | Multi-tenant policy |
| Harbor Terminal-Bench | Harbor + Code runner | [TERMINAL_BENCH.md](TERMINAL_BENCH.md) | Cluster fleet |

Full-feature **Code** testing stops at the host boundary: prove the contract,
fail closed without the host, and record host qualification under L7.

---

## 5. Version regression pins (shipped product hazards)

These are first-class cases because they broke real providers or builds:

| Pin | Mechanism | Hermetic evidence | Live |
| --- | --- | --- | --- |
| **8.5.12 batch schema** | No application `$ref` objects under `batch` parameters `examples` | `tools/builtin/batch.rs` schema asserts (#147) | Optional: GLM Coding turn with `batch` presented |
| **8.5.11 musl** | `a3s-sandbox` 0.1.4 rlimit typing | Node musl optional native build in Release | Release matrix |
| **8.5.10 Gate 7 + Fake-IP** | Native sandbox + DoH fallback | `safe_http` + `test_native_sandbox_live_e2e` | Layer C capabilities |
| **Leaked tool markup** | WorkBuddy / Claude / DSML recovery | `llm::text_tool_calls` | issue-fix / Flash |
| **Unverified mutation** | Completion gate | verification / harness_loop unit | `test_harness_loop_live_e2e` |
| **Session reopen flock** | Cross-process reopen | session persistence unit | cluster resume |

Add new pins to this table on every user-visible fix; do not rely on CHANGELOG
alone.

---

## 6. Explicit gaps (honest backlog)

| ID | Gap | Why it matters | Closure |
| --- | --- | --- | --- |
| G1 | No `just full-feature-hermetic` umbrella | Operators reassemble L0–L4 by hand | Add recipe; wire to CI optional job |
| G2 | F-table IDs ≠ `sdk_capabilities()` IDs | Two matrices drift | Keep §2 as join table; CI check that every baseline ID appears in §2 |
| G3 | Advanced live not in default Layer C | Feature suites optional in `layer-c-live-e2e` | Document required release checklist: extensibility + serve when shipping those features |
| G4 | GLM Coding live not in Layer C pin | batch schema was provider-specific | Keep hermetic pin; optional L5 provider matrix row |
| G5 | Harbor / CAR / DM-PROD are external | Cannot fake in Core CI | Keep L7 owners; never mark full-feature complete without their latest receipt or explicit waiver |
| G6 | Performance numbers age | Runner variance | Re-run `performance.yml` on release candidates; update PERFORMANCE_QUALIFICATION digests |

As of `8.5.12`, CAPABILITY_VERIFICATION still claims **no unresolved Code-owned
ledger gap** for the prior closure set; G1–G6 are **program gaps** (orchestration /
join / external), not missing Core kernels.

---

## 7. Train order (how to run a full pass)

1. **L0–L1** on every PR (`harness-convergence-check` + local-code lib).
2. **L4** SDK runtime on every PR that touches bindings or Core public API.
3. **L2** F-kernel coverage before a stable tag (or weekly).
4. **L3** for any feature flag in the release Cargo features set.
5. **L5** `just layer-c-live-e2e` against monorepo `.a3s/config.acl` Flash pin;
   archive `/tmp/a3s-layer-c-*` / `FINAL.txt`.
6. **L6** performance + hermetic-integrations on RC.
7. **L7** Harbor / host receipts before calling a channel “qualified”.
8. **L8** confirm version pins for the delta since last tag.

Pass criteria for “full-feature hermetic green”:

- L0, L1, L4 green
- L3 green for every feature enabled in the artifact under test
- L2 green if this is a coverage-gated release
- L8 pins for that version present and green

Pass criteria for “full-feature release qualified”:

- Hermetic criteria above, **plus** L5 `LAYER_C_PASS`, L6 digests current,
  L7 receipts or written waivers.

---

## 8. Evidence layout

| Layer | Evidence location |
| --- | --- |
| L1 / A | CI logs; optional `/tmp/a3s-layer-a-*/FINAL.txt` |
| L2 | `/tmp/a3s-llvm-cov-*/FINAL.txt` (`ALL_F_TABLE_KERNELS_GE_90`) |
| L5 / C | `$A3S_LAYER_C_EVIDENCE` or `/tmp/a3s-layer-c-live/FINAL.txt` |
| L6 | Actions artifacts + PERFORMANCE_QUALIFICATION.md |
| L7 | Harbor / Cloud / host report paths in HARNESS_CONVERGENCE.md |

Never promote a draft GitHub Release or crates.io cut on L5 failure without an
explicit product decision recorded next to the tag.

---

## 9. Relationship map

```text
sdk_capabilities()          ──► §2 capability universe
FIRST_PRINCIPLES_TEST_CASES ──► F01–F30 kernels (L2)
FIRST_PRINCIPLES_E2E        ──► Layers A–D ↔ L1/L3/L5/L7
CAPABILITY_VERIFICATION     ──► dimension checklist + ledger
SDK_CAPABILITY_MATRIX       ──► language entrypoints
THIS PLAN                   ──► join + train order + gaps + pins
```

---

## 10. Status

| Item | State |
| --- | --- |
| This plan file | **Written** |
| F01–F30 + Layer C recipes | **Exist** (see linked manuals; last recorded PASS in TEST_CASES status table) |
| `just full-feature-hermetic` | **Gap G1** — not implemented |
| Baseline ID ↔ F-table CI join | **Gap G2** — manual join in §2 only |
| External L7 | **Owner-gated** — not substituted by Core green |

When G1/G2 close, update this status table and add the recipe/check under
`justfile` / CI — do not mark the planning goal complete by inventing green.
