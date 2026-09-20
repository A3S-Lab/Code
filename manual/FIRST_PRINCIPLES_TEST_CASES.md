# First-Principles Test Cases (`a3s-code`)

Mission under test: a **governed coding-agent harness** (loop / tools /
policy / events / retrieval / evidence) — not Cloud UI, not Desktop chrome,
not every Advanced scientific surface by default.

## Coverage scope (first principles)

“≥90% coverage for each feature” means **line coverage of the kernel files that
implement that feature**, measured with:

```bash
cargo llvm-cov -p a3s-code-core --tests --no-cfg-coverage \
  --ignore-filename-regex '(/\.cargo/registry/|/rustc/)' --summary-only
```

It does **not** mean crate-wide TOTAL ≥90% (includes login helpers, optional
transports, and Advanced-only arms). TOTAL is a health signal only.

Opt-in features (`advanced-harness`, `s3`, `headless-search`) keep
their own kernels gated; Layer B/C feature suites prove them when enabled.

Live model pin (full-feature L5): `A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash`
against `A3S_CONFIG_FILE=./.a3s/config.acl` (maps to declared
`boyue/bailian/deepseek-v4.1-flash` via `support/layer_c_model.rs`). Recipe
fallback without override remains ACL `default_model`
(`boyue/deepseek-v4-flash`).

---

## Feature → case matrix

| ID | Feature | Hermetic cases (must) | Live Layer C suite | Cov kernel (≥90%) |
| --- | --- | --- | --- | --- |
| F01 | Native sandbox | session_sandbox unit; native seatbelt fail-closed | `test_native_sandbox_live_e2e` | `sandbox/native.rs`, `agent_api/session_sandbox.rs` |
| F02 | Process-host sandbox | process_host unit; Harbor opt-in | `test_issue_fix_live_e2e` (#140) | `sandbox/process_host.rs` |
| F03 | Streaming tool names / ids | streaming empty-name/id unit | `test_issue_fix_live_e2e`, retrieval | `llm/openai/streaming.rs` |
| F04 | MCP stdio progress | stdio transport unit | `test_issue_fix_live_e2e` (#137) | `mcp/transport/stdio.rs` |
| F05 | Agent protocol v1 | `agent_protocol_*` + host/harness integration | `test_agent_protocol_live_e2e` | `agent_protocol.rs`, `_host`, `_harness` |
| F06 | Oversized event projection | from_run_page / bound payload unit | issue-fix / protocol live | `agent_protocol.rs`, `event_protocol.rs` |
| F07 | Harness loop + verify gate | harness_loop + verification unit | `test_harness_loop_live_e2e` | `harness_loop.rs`, `verification.rs` |
| F08 | Baseline tools | builtin tool unit; capabilities hermetic | `test_harness_capabilities_live_e2e` | `tools/builtin/*` (core), `bash.rs` |
| F09 | Adversarial cancel / hygiene | effect_isolation discard; cancel unit | `test_deepseek_adversarial_e2e` | `effect_isolation.rs` |
| F10 | Update plan | update_plan tool unit | `test_update_plan_live_e2e` | plan tool + `agent/plan_execution.rs` (kernel paths) |
| F11 | Prompt / style gates | permissions specialty unit | `test_prompt_capability_real_llm` | `permissions/interactive.rs` |
| F12 | Orchestration / workflow | orchestration unit | `test_orchestration_real_llm`, `test_workflow_facade_real_llm` | orchestration modules |
| F13 | Long-horizon | loop checkpoint unit | `test_long_horizon_real_llm` | `loop_checkpoint.rs` |
| F14 | Structured JSON | generate_object / structured unit | `test_structured_json_real_llm` | `llm/structured.rs` (kernel) |
| F15 | Run control | run_control unit | `test_run_control_real_llm` | `run_control.rs` |
| F16 | Workspace search | FTS/grep unit | `test_workspace_search_real_llm` | retrieval lexical kernels |
| F17 | Workspace retrieval | semantic/hybrid hermetic | `test_workspace_retrieval_real_llm` | retrieval runtime (≥90% on exercised kernels) |
| F18 | Context tools | context assembler unit | `test_context_tools_real_llm` | `context/*` |
| F19 | Auto-delegation | task/subagent unit | `test_auto_delegation_real_parallel` | `tools/task.rs`, `subagent` |
| F20 | Ultracode discretion | permission discretionary unit | `test_ultracode_discretion_real_llm` | permissions |
| F21 | Config / cluster | config load unit | `test_real_config_env_integration`, `test_real_llm_cluster_features` | `config/*` |
| F22 | Memory store | memory/durable hermetic | `test_memory_store_real_llm` | `durable_memory.rs`, memory kernels |
| F23 | Extensibility | advanced-harness suites | `test_extensibility_real_llm` | gated Advanced kernels |
| F25 | Checkpoints / resume | session_checkpoint unit | cluster save/resume paths | `session_checkpoint.rs` |
| F26 | Safety / budget / HITL | safety_gate, budget, ask_user, hitl unit | capabilities / prompt live | `safety_gate.rs`, `budget.rs`, `ask_user.rs`, `hitl.rs` |
| F27 | PTC program | program executor unit | capabilities when exercised | `program.rs` |
| F28 | Capability scopes | capability ceiling/scope unit | Use-backed live when host injects | `capability/*` |
| F29 | Evidence digests | harness_evidence + content_digest unit | governed live runs | `harness_evidence/*`, `content_digest.rs` |
| F30 | Event envelope | event_protocol unit | all live streams | `event_protocol.rs` |

Layer A orchestration: `just harness-convergence-check` (A1–A4).  
Layer C orchestration: `just layer-c-live-e2e` with bailian Flash pin.

Full-feature join across `sdk_capabilities()`, SDKs, and external gates:
[FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md).

Case-level unit, integration, and soak oracles (not a second capability list):
[test-cases/README.md](test-cases/README.md).

---

## Case design rules

1. **Kernel effect, not surface wording** — assert digests, tool names, terminal
   states, sandbox writes, retention gaps — not “model said OK”.
2. **Hermetic first** — every live suite has a non-LLM twin for the same branch.
3. **Fail-closed defaults** — sandbox, mutation gate, Active-only memory, and
   process-host opt-in must have negative tests.
4. **No overfit** — do not assert provider-specific prose; pin model id only.
5. **Evidence dirs** — live runs write `/tmp/a3s-layer-c-*` with `FINAL.txt`.

---

## Status (this goal)

| Gate | Evidence | Status |
| --- | --- | --- |
| Layer C full matrix (bailian Flash) | `/tmp/a3s-layer-c-bailian-flash-combined/FINAL.txt` (`LAYER_C_PASS`; first pass + serial fail-recovery rerun; pin → `boyue/bailian/deepseek-v4.1-flash`; live outer/API budgets raised for bailian latency; retrieval hang fixed via cancel-on-timeout) | PASS |
| Layer C prior clean matrix | `/tmp/a3s-layer-c-bailian-flash-r19-full/FINAL.txt` (23/23 clean orchestrator) | PASS |
| All F-table kernels ≥90% LINE | `/tmp/a3s-llvm-cov-r24/FINAL.txt` (`ALL_F_TABLE_KERNELS_GE_90`; debuginfo=0 instrumented lib + protocol/checkpoint integ) — prior recorded PASS; not re-measured this execution goal | PRIOR |
| Layer A `harness-convergence-check` | `/tmp/a3s-goal-integ-use/layer-a.FINAL.txt` EXIT:0 (3632 lib + SDK alignment). Prior: `/tmp/a3s-layer-a-bailian-exec/FINAL3.txt` | PASS |
| 29-cap integrated use | [CAPABILITY_INTEGRATED_USE_LEDGER.md](CAPABILITY_INTEGRATED_USE_LEDGER.md) | PASS |
| Release remaining | L2 remeasure + L6 digests + L7 receipts/waivers + L8 pins (plan §14) | OPEN |
| This matrix | `manual/FIRST_PRINCIPLES_TEST_CASES.md` | THIS FILE |
