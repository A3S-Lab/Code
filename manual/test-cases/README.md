# A3S Code Deep Test-Case Catalog

**Line:** Core `8.6.0` (`6b3e4f70`)
**Source of truth:** `core/src/sdk_capabilities.rs` `CAPABILITY_SPECS`
**Status:** case specification. A row is not a passing test. `Home` names the
nearest existing suite when one was found in-tree; it does not prove the
oracle. `gap` means this pass did not find a dedicated case.

This catalog is the case layer under:

- [FULL_FEATURE_TEST_PLAN.md](../FULL_FEATURE_TEST_PLAN.md) — train order and gates
- [FIRST_PRINCIPLES_TEST_CASES.md](../FIRST_PRINCIPLES_TEST_CASES.md) — F01–F30 kernels
- [FIRST_PRINCIPLES_E2E.md](../FIRST_PRINCIPLES_E2E.md) — Layers A–D
- [CAPABILITY_VERIFICATION.md](../CAPABILITY_VERIFICATION.md) — dimension ledger

Do not add a second capability id list. If `sdk_capabilities()` changes, update
the inventory table here in the same change.

## Mission

The system under test is a governed coding-agent harness: session loop, tools,
policy, events, retrieval, and evidence. Desktop chrome, Cloud consoles, and
the Use package market are out of scope. Host-owned capabilities are tested at
the contract boundary: Code fail-closes without the host, and does not fake the
host's external qualification.

## Case schema

Every case uses this shape:

| Field | Meaning |
| --- | --- |
| ID | `U-` unit, `I-` integration, `S-` soak, then capability code, then number |
| Invariant | The property that must stay true. Not a UI string. |
| Pre | State required before the stimulus. |
| Stimulus | The single action under test. |
| Oracle | Kernel effect: digest, path, typed error, absence, counter, exit state. |
| Fail | The observation that fails the case. |
| Home | Existing suite, or `gap`. |

Layers:

| Layer | What it is | What it is not |
| --- | --- | --- |
| Unit | One module. No network. No real model. Temp dirs allowed. | A test that boots the full agent loop. |
| Integration | Crosses a boundary: session+tool+policy, protocol+harness, or an official SDK. Hermetic unless marked **live**. | A soak, and not "the model said OK". |
| Soak | Repetition, duration, or crash/restart with a resource bound. | Re-running a unit test with a bigger timeout. |

Live integration (`I-*-L*`) is `#[ignore]`, serial, and pinned to
`A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` via `.a3s/config.acl`. It is
never Required CI. See `just layer-c-live-e2e`.

## Evidence rules

1. Assert digests, tool names, sandbox writes, typed errors, retention gaps,
   and terminal states. Do not assert provider prose.
2. Every live case has a hermetic twin for the same branch.
3. Enabled surfaces appear. Disabled surfaces stay absent (`local-code` has no
   model-visible `parallel_task`).
4. Fail-closed defaults are negative cases: sandbox, mutation verify gate,
   Active-only memory, process-host opt-in, SSRF / Fake-IP.
5. An official SDK claim is constructed through Node, Python, and Go
   (`scripts/sdk_api_alignment_check.mjs`), not only Rust.
6. `#[ignore]`, Harbor, and soak are not counted as Required CI green.

## Capability inventory

Baseline (default product):

| ID | Code | Catalog |
| --- | --- | --- |
| `agent_runtime` | RT | [baseline-runtime.md](baseline-runtime.md) |
| `conversation` | CV | [baseline-runtime.md](baseline-runtime.md) |
| `run_control` | RC | [baseline-runtime.md](baseline-runtime.md) |
| `persistence` | PE | [baseline-runtime.md](baseline-runtime.md) |
| `run_observability` | RO | [baseline-runtime.md](baseline-runtime.md) |
| `priority_scheduling` | PS | [baseline-runtime.md](baseline-runtime.md) |
| `governed_tools` | GT | [baseline-execution.md](baseline-execution.md) |
| `workspace_tools` | WT | [baseline-execution.md](baseline-execution.md) |
| `governance` | GV | [baseline-execution.md](baseline-execution.md) |
| `web_search` | WS | [baseline-execution.md](baseline-execution.md) |
| `web_fetch` | WF | [baseline-execution.md](baseline-execution.md) |
| `program` | PG | [baseline-execution.md](baseline-execution.md) |
| `model_adapters` | MA | [baseline-model.md](baseline-model.md) |
| `structured_output` | SO | [baseline-model.md](baseline-model.md) |
| `mcp_and_skills` | MS | [baseline-model.md](baseline-model.md) |
| `planning_delegation` | PD | [baseline-model.md](baseline-model.md) |
| `context_memory` | CM | [baseline-model.md](baseline-model.md) |
| `workspace_retrieval` | WR | [retrieval.md](retrieval.md) |

Advanced (explicit Cargo feature or host injection):

| ID | Code | Feature / owner | Catalog |
| --- | --- | --- | --- |
| `code_intelligence` | CI | host language service | [advanced.md](advanced.md) |
| `cognitive_packages` | CP | Use host | [advanced.md](advanced.md) |
| `use_runtime_tasks` | UR | Use host | [advanced.md](advanced.md) |
| `programmable_workflows` | PW | `advanced-harness` | [advanced.md](advanced.md) |
| `state_graph` | SG | `advanced-harness` | [advanced.md](advanced.md) |
| `agent_release_contract` | AR | always compiled | [advanced.md](advanced.md) |
| `agent_protocol` | AP | always compiled | [advanced.md](advanced.md) |
| `evaluation_substrate` | EV | `advanced-harness` | [advanced.md](advanced.md) |
| `moli_runtime` | MO | `headless-search` | [advanced.md](advanced.md) |
| `s3_workspace` | S3 | `s3` | [advanced.md](advanced.md) |
| `filesystem_agent_server` | SV | `serve` | [advanced.md](advanced.md) |
| `opentelemetry` | OT | `telemetry` | [advanced.md](advanced.md) |

`sdk_capabilities()` is the product id list. One compiled contract is not an
id: research wire values (`feature = "research"`,
[RESEARCH_CONTRACTS.md](../RESEARCH_CONTRACTS.md)). Cases are `RX` in
[advanced.md](advanced.md). Do not add `research` to the capability inventory
unless `CAPABILITY_SPECS` gains that id.

Safety kernels that are not capability ids, but are required for the harness
to be safe, live next to the capability they protect:

| Kernel | Protects | Catalog |
| --- | --- | --- |
| Effect isolation / orphan `.a3s-isolate-*` | workspace writes | execution `EI` |
| Native sandbox and process-host opt-in | `bash` | execution `SB` |
| Safe HTTP / Fake-IP / SSRF | `web_fetch`, `web_search`, `download` | execution `SH` |
| Mutation verify gate | completion | execution `VG` |
| `batch` schema pin (no application `$ref` in `examples`) | `workspace_tools` | execution `WT` |
| Image `read` attachments | `workspace_tools` / `conversation` | execution `WT` |
| Event envelope and oversized projection | `run_observability` / `agent_protocol` | runtime `RO`, advanced `AP` |

Soak dispositions for every id are in [soak.md](soak.md). A capability with no
retained resource still has a row: either a resource-bound soak of the owner
it sits on, or an explicit "no retained resource" waiver. Do not invent a
duration test for a pure function.

## How to execute

Hermetic required path (does not run soak or live):

```bash
just harness-convergence-check
cargo test -p a3s-code-core --lib
node scripts/sdk_api_alignment_check.mjs
```

Feature-gated hermetic: `advanced-harness`, `s3`, `serve`, `headless-search`,
`telemetry` suites named in each file.

Live: `just layer-c-live-e2e`.

Soak: cases in [soak.md](soak.md) whose Home is an ignored test or a workflow.
There is no single `just soak` recipe. That missing umbrella is gap G1 in
[FULL_FEATURE_TEST_PLAN.md](../FULL_FEATURE_TEST_PLAN.md), extended here as
**G7**: soak cases are specified per capability but not one command.

## Completion rule for this catalog

The catalog is complete as a plan when every inventory id has:

- at least one unit case, one fail-closed unit case, and one integration case
- one soak row (case or waiver) in [soak.md](soak.md)
- oracles that name a kernel effect

It is not complete as a test program until each `gap` row has a named test
and a recorded run. Do not mark the program green from this file.
