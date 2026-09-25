# Capability integrated-use ledger

Snapshot for Core line `9.0.0` (fact-log control), recorded 2026-09-25.

Every `sdk_capabilities()` id must be **effective**, **efficient**, and
**integrated/used** (not unit-only). Host-owned advanced surfaces prove the
Code boundary (activation + fail-closed); Use / CAR / Harbor remain external
(L7).

**Live model pin:** `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` → remapped
`boyue/bailian/deepseek-v4.1-flash` (ACL peers; monorepo `./.a3s/config.acl`
default alias `boyue/deepseek-v4-flash`).

See also: [FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md),
[CAPABILITY_VERIFICATION.md](CAPABILITY_VERIFICATION.md),
[SDK_CAPABILITY_MATRIX.md](SDK_CAPABILITY_MATRIX.md).

## Evidence roots (this run)

| Root | Path / fact |
| --- | --- |
| Layer A | `/tmp/a3s-goal-integ-use/layer-a.FINAL.txt` EXIT:0 (historical 29-cap snapshot); `typed_decisions` added via Core inventory + unit evidence below |
| Layer C | Tip RC `b91462d3` evidence `/tmp/a3s-layer-c-b91462d3/FINAL.txt` is exactly `LAYER_C_PASS model=boyue/bailian/deepseek-v4-flash` (full matrix, incl. `test_meta_harness_compose_live_e2e`). Pin remapped to `boyue/bailian/deepseek-v4.1-flash`. Prior tip `ee8f68ad` / `fa0a92ca` archives remain supporting. |
| Efficiency | `/tmp/a3s-goal-integ-use/efficiency.log` (THIN_OK, Active-only, hide-disabled, golden inventory) |
| Advanced hermetics | `/tmp/a3s-goal-integ-use/advanced-integ.log` + `cap-targeted.log` |
| SDK discovery | Node / Python / Go project `sdk_capabilities()`; Core inventory now has 30 ids including `typed_decisions` (schema `a3s-code/sdk-capabilities/v2`) |
| L3 hermetics | `/tmp/a3s-l3-hermetic/FINAL.txt` EXIT:0 |

## Per-capability matrix (30)

| Capability | Tier | Effective | Efficient | Integrated use | Status |
| --- | --- | --- | --- | --- | --- |
| `agent_runtime` | baseline | Layer A lib + cluster live | thin `local-code`; SDK create/close | `test_real_llm_cluster_features` PASS | PASS |
| `conversation` | baseline | stream/cancel unit + Layer A | bounded cancel | cluster / run_control live PASS | PASS |
| `run_control` | baseline | run_control unit + runtime integ | idempotent receipts | `test_run_control_real_llm` PASS | PASS |
| `governed_tools` | baseline | registry / confirmation / hooks | `register_builtins` hides disabled | `test_harness_capabilities_live_e2e` PASS (rerun) | PASS |
| `workspace_tools` | baseline | read/edit/bash/git unit | thin registry | harness_capabilities + issue_fix live PASS | PASS |
| `workspace_retrieval` | baseline | retrieval hermetic + Layer A | bounded session state | retrieval + search live PASS (rerun) | PASS |
| `model_adapters` | baseline | llm/openai unit | ACL pin remap | config + cluster live PASS | PASS |
| `structured_output` | baseline | structured unit | bounded repair | `test_structured_json_real_llm` PASS (rerun) | PASS |
| `mcp_and_skills` | baseline | mcp/skills unit | isolation bounds | `test_issue_fix_live_e2e` PASS | PASS |
| `planning_delegation` | baseline | task unit; `parallel_task` unregistered | HARNESS-CONV4 absence oracle | auto_delegation + update_plan live PASS | PASS |
| `priority_scheduling` | baseline | task_scheduler + queue unit | aging/capacity bounds | hermetic sufficient (plan §3.1) | PASS |
| `persistence` | baseline | checkpoint/store unit | atomic flock | cluster save/resume live PASS | PASS |
| `governance` | baseline | permissions/hitl/verify unit | fail-closed defaults | prompt + harness_loop live PASS | PASS |
| `run_observability` | baseline | event_protocol unit | bounded pages | all Layer C live streams PASS | PASS |
| `context_memory` | baseline | durable_memory Active-only | candidate inactive until activation | `test_memory_store_real_llm` PASS | PASS |
| `web_search` | baseline | web_search / safe_http unit | SSRF Fake-IP | harness_capabilities live PASS (rerun) | PASS |
| `web_fetch` | baseline | web_fetch / safe_http unit | size/redirect bounds | harness_capabilities live PASS (rerun) | PASS |
| `program` | baseline | QuickJS program integ | escape hatch, not second engine | harness_capabilities live PASS | PASS |
| `code_intelligence` | advanced | LSP/tool unit + fail-closed | host language service required | advanced-integ fail-closed PASS; PERF owner | PASS |
| `cognitive_packages` | advanced | cognitive_context unit | host-owned fail-closed | Code boundary CI; Use/CAR external | PASS |
| `use_runtime_tasks` | advanced | use_runtime_tasks unit | host-owned fail-closed | Code adapter CI; Use host external | PASS |
| `programmable_workflows` | advanced | dynamic_workflow + advanced-harness | feature-gated | extensibility + workflow_facade live PASS | PASS |
| `state_graph` | advanced | state_graph integration | advanced feature | advanced-integ PASS | PASS |
| `agent_release_contract` | advanced | agent_release_manifest | admission bounds | advanced-integ PASS | PASS |
| `agent_protocol` | advanced | agent_protocol hermetic | event page bounds | `test_agent_protocol_live_e2e` PASS | PASS |
| `evaluation_substrate` | advanced | evaluation_* hermetic | host eval policy external | advanced-integ PASS | PASS |
| `typed_decisions` | advanced | `typed_decision` unit (feature `apofasi`) | fail-closed empty; feature opt-in | Code substrate CI; host product routing external | PASS |
| `moli_runtime` | advanced | `test_web_search_headless` | `headless-search` feature | L3 hermetic PASS | PASS |
| `s3_workspace` | advanced | S3 unit + ignored live green | `s3` feature; profile `server` ≠ serve | advanced-integ PASS | PASS |
| `opentelemetry` | advanced | telemetry unit + ignored green | `telemetry` feature | advanced-integ PASS | PASS |

## Layer gates

| Gate | Evidence | Status |
| --- | --- | --- |
| L0/L1 Layer A | `/tmp/a3s-goal-integ-use/layer-a.FINAL.txt` | PASS |
| L3 advanced / s3 / otel / headless | advanced-integ + L3 FINAL | PASS |
| L4 SDK | Node `npm test`, Python pytest, Go `test ./...`, alignment + discovery | PASS |
| L5 Layer C bailian Flash | Tip RC `b91462d3` `/tmp/a3s-layer-c-b91462d3/FINAL.txt` (`LAYER_C_PASS`, full matrix incl. meta_harness compose; pin → `boyue/bailian/deepseek-v4.1-flash`) | PASS |
| Efficiency (thin / absence) | `just harness-convergence-check` on this cut; `local-code` lib 3825 passed; `ci-all` lib 4100 passed | PASS |
| L2 F-kernel cov | `/tmp/a3s-llvm-cov-f95/FINAL.txt` `ALL_F_TABLE_KERNELS_GE_95_PASS scored=42`; worst prior miss `agent_protocol_harness.rs` now **95.16%** on tip `15d2a863`/`cdba052b` | PASS |
| L8 §7 9.0.0 pins | Tip hermetics: `fact_log` 34/34; effect `tool_round_cap` + park 5/5; bm25/a3s-vec FTS 17/17 | PASS (hermetic) |
| L6 Actions | Tip `ee8f68ad` performance.yml `36092780724` and hermetic-integrations `36122389323`, both `passed: true`, archived in PERFORMANCE_QUALIFICATION | PASS |
| L7 Harbor / CAR | TB-QUAL1, DM-PROD1, CAR-01…CAR-05 still in progress; no product waiver | Not met |

## Verdict

All **30** product capabilities have current-state evidence of effective
kernels, efficiency constraints, and integrated or host-boundary use on the
9.0.0 fact-log tip `ee8f68ad`. Layer C under bailian Flash and L6 Actions are
green for this tip. Enterprise GA is not achieved: L7 has no Harbor or CAR
receipt and no product waiver.
