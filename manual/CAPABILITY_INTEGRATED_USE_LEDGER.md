# Capability integrated-use ledger

Snapshot for Core line `8.6.0` (post HARNESS-CONV5 slim), recorded 2026-09-21.

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
| Layer C | `/tmp/a3s-layer-c-bailian-flash-combined/FINAL.txt` `LAYER_C_PASS` (22/22 final after fail-recovery) |
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
| L4 SDK | alignment + Node/Python/Go discovery | PASS |
| L5 Layer C bailian Flash | combined FINAL `LAYER_C_PASS` | PASS |
| Efficiency (thin / absence) | THIN_OK; no model-visible `parallel_task`; Active-only | PASS |
| L2 F-kernel cov | plan §9 coverage-gated / weekly | Not remeasured this run |
| L6 / L7 | plan §9 owner-gated (perf / Harbor / CAR) | Not claimed |

## Verdict

All **30** product capabilities have current-state evidence of effective
kernels, efficiency/absence constraints, and integrated or host-boundary use.
Layer C under bailian Flash is green. This closes the Core full-feature
hermetic + L5 + integrated-use bar. It does **not** by itself authorize a
crates.io / GitHub Release cut — see plan §9 / §14 for remaining release gates.
