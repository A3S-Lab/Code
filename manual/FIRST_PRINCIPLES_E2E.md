# First-Principles E2E Matrix (`a3s-code`)

Mission under test: a **governed coding-agent harness** (loop / tools /
adapters / policy / events / retrieval / evidence) — not Cloud UI, not a
plugin market, not a scientific product chrome.

Efficiency rule: default surface stays thin (`local-code`); Advanced /
server / headless / Harbor / prod memory are explicit opt-in gates.

## Layer A — Hermetic baseline (must be green)

| ID | Mechanism | Command / artifact |
| --- | --- | --- |
| A1 | Thin default tree | `cargo check -p a3s-code-core` + tree free of evaluation/research/chromiumoxide/s3 |
| A2 | local-code CI matrix | `cargo check/test --no-default-features --features local-code --lib` |
| A3 | Dual-path removal | `parallel_task` unregistered; Active-only memory (`candidate_write_stays_inactive`) |
| A4 | SDK alignment | `node scripts/sdk_api_alignment_check.mjs` |
| A5 | Agent protocol / checkpoint / run control | `agent_protocol_*`, `*_checkpoint_*`, `run_control_runtime` integration tests |
| A6 | Tools + permissions | lib tests for builtin tools, style specialty overlays, `update_plan` |
| A7 | Events / evidence digests | `event_protocol_v1`, `content_digest`, durable-memory product eval hermetic |
| A8 | Retrieval baseline | workspace search / FTS paths without headless |

Orchestration: `just harness-convergence-check` covers A1–A4.

## Layer B — Hermetic Advanced (feature-gated)

| ID | Mechanism | Feature | Suites |
| --- | --- | --- | --- |
| B1 | Evaluation substrate | `advanced-harness` | `evaluation_*` |
| B2 | Research contracts | `advanced-harness` | `research_*` |
| B3 | State graph / dynamic workflow | `advanced-harness` | `test_state_graph_*`, `dynamic_workflow_*` |
| B4 | Serve / S3 | `server` / `s3` | `test_s3_backend`, serve suites |

## Layer C — Live provider E2E (ignored; config required)

| ID | Suite | Proves |
| --- | --- | --- |
| C1 | `test_prompt_capability_real_llm` | GP vs Explore permission/prompt hard gate |
| C2 | `test_deepseek_adversarial_e2e` | Cancel / hostile tools / secret hygiene |
| C3 | `test_update_plan_live_e2e` | Checklist tool live path |
| C4 | Orchestration / long-horizon / structured JSON / run-control real LLM | Multi-mechanism live |
| C5 | `test_harness_loop_live_e2e` / `test_harness_capabilities_live_e2e` | Gate + baseline tools (incl. download/web_fetch) |
| C6 | `test_workspace_retrieval_real_llm` / search / memory | Retrieval + `context_memory` live |
| C7 | `test_issue_fix_live_e2e` | `#139` streaming tool names, `#137` MCP stdio progress, `#138` oversized event page, `#140` process-host bash under live Flash |
| C8 | `test_agent_protocol_live_e2e` | Harness Start/replay, live tool→change set, protocol Cancel |

Run the full serial matrix with `just layer-c-live-e2e` (pins
`boyue/deepseek-v4-flash`; override with `A3S_TEST_MODEL`).

## Layer D — External qualification (not substituted by A–C)

| ID | Gate | Owner |
| --- | --- | --- |
| D1 | `TB-QUAL1` Harbor full matrix | Harbor + Code runner |
| D2 | `DM-PROD1` production memory | Host report |
| D3 | `CAR-01`…`CAR-05` | Cloud / Box |

See [HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md) evidence templates.

Feature → hermetic/live/cov kernel mapping:
[FIRST_PRINCIPLES_TEST_CASES.md](FIRST_PRINCIPLES_TEST_CASES.md).

## Efficiency checks

| Check | Pass criteria |
| --- | --- |
| Default compile surface | No Advanced/server in default dependency tree |
| Registry surface | No model-visible `parallel_task`; no shadow memory mode |
| CI cost | local-code gate is mandatory; Advanced is not default |
| Live budget | Ignored suites; run serially (`--test-threads=1`) |

## Status log

Filled as this goal progresses; do not mark complete without requirement-level
evidence.

Evidence `/tmp/a3s-session-bailian-r14/` (2026-09-16):
- Requested id `boyue/bailian/deepseek-v4-flash` is **not** in
  `./.a3s/config.acl`. Declared bailian Flash is
  `boyue/bailian/deepseek-v4.1-flash` (Layer C maps the typo → declared id).
- Live pin: `boyue/bailian/deepseek-v4.1-flash` via `A3S_TEST_MODEL`.
- Session continue: run_control steer + interrupt **2/2**, plus
  `real_model_session_continues_after_interrupt` **PASS** (Cancelled →
  follow-up `CONTINUE_OK`).
- Session resume: cluster `resume_run` + `save`/`resume_session_async`
  **PASS**.
- Session model switch: `real_replace_session_switches_model_and_continues`
  **PASS** (bailian v4.1 → `boyue/deepseek-v4-flash`).
- Cluster **8/8**, run_control focus **3/3** under bailian Flash.
  Hermetic: `replacement_is_atomic`, `resume_run*`, `session_with_model*`.

Evidence `/tmp/a3s-session-flash-r13/` (2026-09-16):
- Model pin: `boyue/deepseek-v4-flash` via `./.a3s/config.acl` +
  `support/layer_c_model.rs` (honors `A3S_TEST_MODEL` when declared).
- WorkBuddy / leaked tool markup: hermetic `llm::text_tool_calls` **9/9**
  (attribute, WorkBuddy bare, WorkBuddy tagged, Claude invoke, DeepSeek
  DSML). Product fix recovers bare/tagged/`invoke` into structured
  tool-use and strips protocol wrappers from prose.
- Session continue: `test_run_control_real_llm` **2/2** (steer + interrupt)
  on Flash.
- Session resume: `test_real_llm_cluster_features` includes
  `real_resume_run_carries_checkpoint_metrics_forward` +
  `real_session_save_resume_round_trips_history` PASS.
- Session model switch: `real_replace_session_switches_model_and_continues`
  PASS (`boyue/deepseek-v4-flash` → `boyue/bailian/deepseek-v4.1-flash`).
- Cluster suite overall **8/8** on Flash. Issue-fix **4/4** on Flash
  (oversized tool_end can flake once under Flash; reconfirmed green).
  Hermetic: `replacement_is_atomic`, `resume_run*`, `session_with_model*`.

Evidence `/tmp/a3s-layer-c-sandbox013-r12/` (2026-09-16):
- Model pin: `boyue/bailian/deepseek-v4.1-flash` via `./.a3s/config.acl`.
- Fake-IP LAN (`example.com` → `198.18.0.95`) blocked direct `web_fetch` /
  `download`. Product fix: when every local answer is Clash/Surge Fake-IP
  (`198.18.0.0/15`) and no proxy is configured, resolve via Cloudflare DoH
  (`https://1.1.1.1/dns-query`) then re-apply the same public-address SSRF
  checks. Loopback / RFC1918 answers still fail closed (no DoH escape).
- Hermetic: `safe_http` DoH JSON parse + Fake-IP detection + proxy redirect
  wiremock tests; `test_stream_tool_round_checkpoint_flushes_session_before_end`
  for mid-run `auto_save` flush.
- Live reconfirmed: capabilities **23/23** (incl. web_fetch + download),
  native sandbox Flash write **1/1**, harness loop **8/8**, issue-fix **4/4**,
  agent protocol **3/3**, update_plan / prompt / structured_json / run_control
  PASS. CI: Windows check timeout raised to 45m for serial lib tests.

Clean Layer C matrix against `A3S_CONFIG_FILE=./.a3s/config.acl` with
`A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` (in-process pin remaps to
declared `boyue/bailian/deepseek-v4.1-flash` via `support/layer_c_model.rs`).
Recipe: `just layer-c-live-e2e`.

Evidence `/tmp/a3s-layer-c-bailian-flash-r19-full/` (2026-09-16):
- Full serial matrix **LAYER_C_PASS** on bailian Flash (`FINAL.txt`).
- All 23 Layer C suites PASS (21 core + `advanced-harness` extensibility +
  `serve` agent-dir).
- llvm-cov `--tests` `/tmp/a3s-llvm-cov-r20/`: E2E surfaces all ≥90% lines —
  `agent_protocol` **93.50%**, `agent_protocol_host` **90.91%**,
  `agent_protocol_harness` **90.14%**, `session_sandbox` **95.72%**,
  `streaming` **92.78%**, `stdio` **92.76%**, `process_host` **95.71%**.

Evidence `/tmp/a3s-layer-c-boyue-r11/`:
- Full serial matrix on `boyue/bailian/deepseek-v4.1-flash` via
  `just layer-c-live-e2e`. First pass failed only
  `test_deepseek_adversarial_e2e` cancel assert when session created a
  workspace `.a3s/` root (harness-owned, same class as `.a3s-code`). Fixed by
  ignoring `.a3s` / `.a3s-code` / `.git` in the cancel workspace filter
  (aligned with harness loop/capabilities live helpers). Adversarial retry
  **3/3 PASS**; `FINAL.txt`: `LAYER_C_PASS … (adversarial-retry after .a3s
  harness filter)`.
- Coverage delta (non-overfit): `store_persists_main_agent_reply_until_reopen`
  for session-review address reply persistence.

Evidence `/tmp/a3s-issue-fix-live-r10/`:
- `test_issue_fix_live_e2e` **4/4 PASS** on `boyue/bailian/deepseek-v4.1-flash`
  (`FINAL.txt`: `ISSUE_FIX_LIVE_PASS`). Kernel checks only: non-empty streamed
  tool names across write→read turns; ≥2 MCP `notifications/progress` during
  live `tools/call`; oversized `tool_end` still projects via
  `AgentProtocolEventPageV1::from_run_page`; `#140` live Flash bash through an
  injected `ProcessHostBashSandbox` writes `host_token.txt` (Harbor path, not
  native Seatbelt success masking).

Evidence `/tmp/a3s-agent-protocol-live-r10/`:
- `test_agent_protocol_live_e2e` **3/3 PASS** on `boyue/bailian/deepseek-v4.1-flash`
  (`FINAL.txt`: `AGENT_PROTOCOL_LIVE_PASS`). Kernel checks only: Harness Start
  receipt + terminal event page + replayed Start; live `write` projects
  non-empty tool names and a validating change set; protocol Cancel reaches
  `Cancelled` without the late leak file.

Hermetic coverage deltas (non-overfit, kernel branches of the same fixes):
- `streaming_empty_continuation_name_does_not_wipe_tool_name` + snapshot/delta
  empty-name unit tests (`#139`)
- `streaming_empty_or_missing_id_still_emits_usable_tool_call` synthesizes
  `call_{index}` when the gateway never provides a tool-call id (`#139` class)
- `from_run_page_projects_oversized_tool_end_instead_of_400` (`#138`)
- Prompt contract updated to native-default / process-host opt-in wording (`#140`)
- llvm-cov `--tests` `/tmp/a3s-fix-cov-r10b/`: `agent_protocol` **85.9%**,
  `agent_protocol_host` **89.4%**, `agent_protocol_harness` **79.6%**, weighted
  protocol ≈ **85.2%** (prior ≈ **82.8%**); `stdio` **92.5%**, `process_host`
  **89.7%**, `session_sandbox` **89.8%**, `streaming` **69.3%**.

Evidence `/tmp/a3s-layer-c-boyue-r10/`:
- Full serial matrix via `just layer-c-live-e2e` on
  `boyue/bailian/deepseek-v4.1-flash`. First pass failed only
  `test_workspace_retrieval_real_llm` (3/3) with kernel evidence
  `tool call id must not be empty` — Flash/gateway streamed empty
  `tool_calls[].id` deltas that wiped accumulated ids (same class as `#139`
  empty names). Fixed by ignoring empty id continuations and synthesizing
  `call_{index}` when the stream never provides an id.
- After the empty-id fix: retrieval **3/3 PASS**
  (`/tmp/a3s-layer-c-boyue-r10/retry2/`); issue-fix reconfirmed **4/4**
  (`/tmp/a3s-issue-fix-live-r10b/`). `FINAL.txt`:
  `LAYER_C_PASS … (retrieval-retry after empty tool-call id fix)`.

Evidence `/tmp/a3s-issue-fix-live/`:
- Prior `test_issue_fix_live_e2e` 3/3 PASS on `boyue/bailian/deepseek-v4.1-flash`
  (`FINAL.txt`: `ISSUE_FIX_LIVE_PASS`). Kernel checks only: non-empty streamed
  tool names across write→read turns; ≥2 MCP `notifications/progress` during
  live `tools/call`; oversized `tool_end` still projects via
  `AgentProtocolEventPageV1::from_run_page`.

Evidence `/tmp/a3s-agent-protocol-live/`:
- `test_agent_protocol_live_e2e` **3/3 PASS** on `boyue/bailian/deepseek-v4.1-flash`
  (`FINAL.txt`: `AGENT_PROTOCOL_LIVE_PASS`). Kernel checks only: Harness Start
  receipt + terminal event page + replayed Start; live `write` projects
  non-empty tool names and a validating change set; protocol Cancel reaches
  `Cancelled` without the late leak file.
- Measurement note: `--lib` llvm-cov under-reports this surface (integration
  suites own most of it). With `--tests` after C8 hermetic metadata bound:
  `agent_protocol` **84.0%**, `agent_protocol_host` **89.1%**,
  `agent_protocol_harness` **74.6%** (remaining misses are mostly exact-recovery
  / capacity / closed error arms — not live LLM paths). Weighted protocol
  surface ≈ **82.8%** lines.

Evidence `/tmp/a3s-layer-c-boyue-v858-r9/`:
- Full serial matrix green on `boyue/bailian/deepseek-v4.1-flash`
  (`FINAL.txt`: `LAYER_C_PASS … (capabilities-retry)`).
- Initial capabilities fail was DNS lookup for `example.com` on `download`;
  retry 23/23. Loop 8/8 includes live `verify_commands`. Cluster 7/7 includes
  save/resume + scheduler + `run_event_page`. Memory extract hard-fail 2/2.
- `web_search` asserts kernel effect (query/results/engine success), not
  structural exit_code alone. `batch` live dual-read covered. Shared pin loader
  for loop/capabilities.

| Suite | Result |
| --- | --- |
| `test_issue_fix_live_e2e` | 4/4 (r10 / r10b) |
| `test_agent_protocol_live_e2e` | 3/3 |
| `test_harness_loop_live_e2e` | 8/8 |
| `test_harness_capabilities_live_e2e` | 23/23 |
| `test_deepseek_adversarial_e2e` | 3/3 |
| `test_update_plan_live_e2e` | 1/1 |
| `test_prompt_capability_real_llm` | 2/2 |
| `test_orchestration_real_llm` | 7/7 |
| `test_long_horizon_real_llm` | 1/1 |
| `test_structured_json_real_llm` | 6/6 |
| `test_run_control_real_llm` | 2/2 |
| `test_workspace_search_real_llm` | 1/1 |
| `test_workspace_retrieval_real_llm` | 3/3 (retry2 after empty-id fix) |
| `test_context_tools_real_llm` | 4/4 |
| `test_workflow_facade_real_llm` | 4/4 |
| `test_auto_delegation_real_parallel` | 2/2 |
| `test_ultracode_discretion_real_llm` | 3/3 |
| `test_real_config_env_integration` | 3/3 |
| `test_real_llm_cluster_features` | 7/7 |
| `test_memory_store_real_llm` | 2/2 |
| `test_extensibility_real_llm` (`advanced-harness`) | 4/4 |
| `test_serve_agent_dir_real_llm` (`serve`) | 2/2 |

Prior r6 under `/tmp/a3s-layer-c-boyue-v858-r6/` is the last clean pass before
these coverage deltas. r7/r8 aborted mid-matrix after probe fixes.

| Layer | Status | Evidence |
| --- | --- | --- |
| A | Pass | content-bind Verify + hermetic + SDK matrix (+ batch) + zh-CN honesty |
| B | Prior hermetic pass | Advanced feature suites unchanged this pass |
| C | Pass (boyue bailian Flash) | r19 `/tmp/a3s-layer-c-bailian-flash-r19-full/` LAYER_C_PASS; llvm-cov r20 E2E surfaces ≥90% |
| D | Blocked externally until Harbor/host/CAR reports | |
