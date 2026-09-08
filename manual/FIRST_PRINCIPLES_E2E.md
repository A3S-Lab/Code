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

Point `A3S_CONFIG_FILE` at a secret-bearing ACL; never commit keys.

## Layer D — External qualification (not substituted by A–C)

| ID | Gate | Owner |
| --- | --- | --- |
| D1 | `TB-QUAL1` Harbor full matrix | Harbor + Code runner |
| D2 | `DM-PROD1` production memory | Host report |
| D3 | `CAR-01`…`CAR-05` | Cloud / Box |

See [HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md) evidence templates.

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

Fresh live pass against `A3S_CONFIG_FILE=./.a3s/config.acl`
(`default_model=deepseek/deepseek-v4-pro`), serial `--ignored --test-threads=1`:

| Suite | Result |
| --- | --- |
| `test_prompt_capability_real_llm` | 2/2 |
| `test_deepseek_adversarial_e2e` | 3/3 |
| `test_update_plan_live_e2e` | 1/1 |
| `test_orchestration_real_llm` | 7/7 |
| `test_long_horizon_real_llm` | 1/1 |
| `test_structured_json_real_llm` | 6/6 |
| `test_run_control_real_llm` | 2/2 |
| `test_workspace_search_real_llm` | 1/1 (warm durable index via `local_with_indexed_retrieval`) |
| `test_workspace_retrieval_real_llm` | 3/3 |
| `test_context_tools_real_llm` | 4/4 |
| `test_workflow_facade_real_llm` | 4/4 |
| `test_auto_delegation_real_parallel` | 2/2 |
| `test_extensibility_real_llm` (`advanced-harness`) | 4/4 |
| `test_ultracode_discretion_real_llm` | 3/3 |
| `test_real_config_env_integration` | 3/3 (no longer hard-requires MiniMax `A3S_OPENAI_*`) |
| `test_real_llm_cluster_features` | 5/5 |
| `test_serve_agent_dir_real_llm` (`serve`) | 2/2 |

| Layer | Status | Evidence |
| --- | --- | --- |
| A | Pass | `just harness-convergence-check` green after SDK `set_output_language` / `outputLanguage` alignment (Node/Python/Go) + prior local-code lib fixes (3138+ lib tests under `local-code`) |
| B | Pass (hermetic) | `advanced-harness` lib: evaluation 52 / research 45 / state_graph 23 / dynamic_workflow 35; integration eval+research suites green; `s3` lib 75 green; live S3 ignored without endpoint |
| C | Pass (config.acl model) | Table above; fixes: Active-only task fan-out, config ACL without OpenAI env gate, workspace-search index warm for lazy zvec |
| D | Blocked externally until Harbor/host/CAR reports | |
