# Baseline model, extension, and memory cases

Capabilities: `model_adapters` (MA), `structured_output` (SO),
`mcp_and_skills` (MS), `planning_delegation` (PD), `context_memory` (CM).

## model_adapters

Anthropic, OpenAI-compatible, and custom host adapters. The live pin is one
OpenAI-compatible route. Adapter correctness is hermetic first.

### U-MA-01

- **Invariant:** base URL join does not drop or duplicate a path prefix.
- **Pre:** base `https://example.test/v1/` and `https://example.test/v1`, endpoint `chat/completions`.
- **Stimulus:** build the request URL.
- **Oracle:** exactly one `/v1/chat/completions`; no `//`.
- **Fail:** the prefix is stripped, so the client calls the wrong host path.
- **Home:** `core/src/llm/openai.rs` tests.

### U-MA-02

- **Invariant:** streamed tool calls with an empty name or id are recovered or rejected, never executed under an empty name.
- **Pre:** SSE fixture with a tool call whose first delta has an empty name.
- **Stimulus:** parse the stream.
- **Oracle:** the assembled call has the final non-empty name, or the turn errors before tool dispatch.
- **Fail:** a tool is dispatched with `name=""`.
- **Home:** `core/src/llm/openai.rs` streaming tests; F03 kernel `llm/openai/streaming.rs`.

### U-MA-03

- **Invariant:** leaked tool markup (WorkBuddy / Claude / DSML) is recovered into a structured tool call or stripped, and is not shown as a successful assistant final with the markup still executable.
- **Pre:** fixture assistant text that contains a leaked tool-call block and no structured tool call.
- **Stimulus:** parse.
- **Oracle:** either one structured tool call matching the block, or text with the block removed and no execution.
- **Fail:** the markup is executed as shell, or duplicated (structured call plus the raw block executed).
- **Home:** `llm::text_tool_calls` tests in `core/src/llm/tests.rs`.

### U-MA-04

- **Invariant:** OpenAI-compatible tool results keep image parts as `image_url` when the tool result is an image attachment.
- **Pre:** a `read` result with `image/png` bytes.
- **Stimulus:** encode the next model request.
- **Oracle:** message contains an `image_url` part; the text part is not the only representation of the image.
- **Fail:** image flattened to a text placeholder, which is the 8.6.0 bug.
- **Home:** `core/src/llm/openai.rs`, `core/src/llm/tests.rs`.

### U-MA-05

- **Invariant:** a 401, a timeout, and a 5xx do not retry a non-idempotent tool execution. Retries apply to the model HTTP call only, and stop at the retry cap.
- **Pre:** HTTP fixture: two 500s then 200; separate case: 500 after a tool already ran.
- **Stimulus:** one turn.
- **Oracle:** model request retried ≤ cap; tool body ran once.
- **Fail:** the tool runs once per retry.
- **Home:** `soak_http_retries_do_not_multiply_tool_execution` in `core/src/agent/model_retry_soak.rs` (`#[ignore]`). Forty turns each see two HTTP 500s then 200; the tool counter stays 40, not 120. `RetryConfig::is_retryable_status` rejects 401, so a 401 is not a model retry and cannot re-enter the tool.

### U-MA-06

- **Invariant:** an unknown adapter id fail-closes at session create.
- **Pre:** config whose model provider is `not-an-adapter`.
- **Stimulus:** `agent.create`.
- **Oracle:** typed config error; no session directory left behind.
- **Fail:** create succeeds and fails later on first send with a generic network error.
- **Home:** `unknown_provider_fail_closes_before_a_session_directory` in `core/tests/test_session_close_lifecycle.rs`. OpenAI-compatible provider names that are declared in config stay valid; an undeclared provider is the fail-close.

### I-MA-01

- **Invariant:** custom host adapter is invoked with the same message list the built-in adapter would send, including tool results.
- **Pre:** host adapter recording requests; fake tool result.
- **Stimulus:** one turn.
- **Oracle:** recorded request contains the tool result digest; host adapter errors surface as typed model errors.
- **Fail:** the host adapter receives a prompt that omits the tool result.
- **Home:** `custom_host_adapter_receives_the_tool_result` in `core/src/agent_api/tests/host_adapter.rs`. The follow-up `LlmClient` request contains tool output `HOST-RESULT-91`. Host adapter failures already surface as run errors in `test_stream_error_does_not_update_history_or_auto_save` and `test_non_retryable_stream_error_skips_fallback_and_circuit_retries`. Built-in adapters convert the same `Message::tool_result` shape in `core/src/llm/tests.rs`.

### I-MA-L1

- **Invariant:** the Flash pin answers a no-tool turn and a one-tool turn without schema rejection.
- **Pre:** `A3S_CONFIG_FILE` and `A3S_TEST_MODEL` pin. Ignored. Outer budget 420s where the suite documents it.
- **Stimulus:** Layer C cluster/config suites.
- **Oracle:** HTTP 200; terminal run; no tool-schema 500.
- **Fail:** timeout treated as pass, or assertions weakened to fit latency.
- **Home:** `test_real_config_env_integration.rs`, `test_real_llm_cluster_features.rs`.

## structured_output

Schema-constrained output, validation, bounded repair.

### U-SO-01

- **Invariant:** output that does not match the schema is not returned as success.
- **Pre:** schema `{n: number}`; model fixture returns `"nope"`.
- **Stimulus:** `generate_object` / structured send.
- **Oracle:** typed validation error after repair attempts are exhausted; attempt count ≤ bound.
- **Fail:** `"nope"` returned as the object, or repair loops past the bound.
- **Home:** `core/src/llm/structured.rs`, `core/src/tools/builtin/generate_object_contract_tests.rs`.

### U-SO-02

- **Invariant:** repair uses the validator error and stops when the object validates.
- **Pre:** first response missing a field; second response valid.
- **Stimulus:** one structured call.
- **Oracle:** one repair request; final object validates; no third request.
- **Fail:** unbounded repairs, or the invalid first object is returned.
- **Home:** `generate_object.rs` contract tests.

### U-SO-03

- **Invariant:** additional properties are rejected when the schema says so.
- **Pre:** schema `additionalProperties: false`.
- **Stimulus:** object with an extra field.
- **Oracle:** validation failure.
- **Fail:** extra field silently stripped and reported as the model's object without a flag. Stripping is allowed only if the API documents it and the test asserts the flag.
- **Home:** `test_validate_resolves_local_refs_without_ambient_io` in `core/src/llm/structured_tests.rs`. A schema with `additionalProperties: false` rejects `{"name":"Ada","secret":true}`.

### I-SO-01

- **Invariant:** `session.task` with an output schema uses the same validator as `session.send`.
- **Pre:** same schema, hermetic model fixture.
- **Stimulus:** both entry points.
- **Oracle:** same accept/reject decision; task result digest equals send result digest for the same fixture output.
- **Fail:** task skips validation.
- **Home:** `task_and_generate_object_reject_the_same_invalid_object` in `core/src/agent_api/tests/scheduler.rs`. `session.tool("generate_object")` is the send-side structured entry; `session.tool("task")` on tool-free `loop-planner` is the task entry. Both reject `nope` and both accept `{"n":1}` as the same object. Rust has no `AgentSession::task` / `AgentSession::send` schema pair.

### I-SO-L1

- **Invariant:** live model returns an object that validates, or the run fails closed. Prose that merely looks like JSON is not success.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** `test_structured_json_real_llm`.
- **Oracle:** parsed object matches the schema; run terminal.
- **Fail:** success based on substring match.
- **Home:** `core/tests/test_structured_json_real_llm.rs`.

## mcp_and_skills

### U-MS-01

- **Invariant:** `session.add_mcp` with a bad command fail-closes and does not leave a child process.
- **Pre:** command path that does not exist.
- **Stimulus:** add, then `session.mcps`, then remove.
- **Oracle:** typed error or a server entry in failed state; process table has no child; remove of an unknown server is idempotent not-found.
- **Fail:** a zombie process, or remove panics.
- **Home:** `core/src/mcp/transport/stdio.rs` unit tests.

### U-MS-02

- **Invariant:** MCP stdio progress events are forwarded and do not block the tool result.
- **Pre:** fixture server that emits progress then a result.
- **Stimulus:** call the MCP tool.
- **Oracle:** progress events ordered before the result; result payload matches; timeout if progress never ends and no result arrives.
- **Fail:** progress treated as the final result.
- **Home:** stdio transport tests; live twin `test_issue_fix_live_e2e` (#137).

### U-MS-03

- **Invariant:** a skill name collision does not replace a built-in tool.
- **Pre:** skill named `read`.
- **Stimulus:** `session.add_skill`, then list tool definitions and `session.skill_names`.
- **Oracle:** builtin `read` still the tool; skill is listed as a skill or rejected as a reserved name. It must not shadow `read`.
- **Fail:** the skill body runs when the model calls `read`.
- **Home:** `skill_named_read_does_not_replace_the_builtin_tool` in `core/src/agent_api/tests/skill_collision.rs`. `session.add_skill` may accept the name `read`; `session.tool("read")` still returns the file bytes `READ-BUILTIN-91` and not the skill body.

### U-MS-04

- **Invariant:** skill files are read from the skill root only.
- **Pre:** skill root with a symlink to a file outside.
- **Stimulus:** load skills.
- **Oracle:** outside target not loaded.
- **Fail:** outside file becomes a skill body.
- **Home:** `core/src/skills/registry/tests.rs` `test_load_from_dir_does_not_follow_symlinks_outside_the_skill_root` (`#[cfg(unix)]`). The walk compares two canonical paths, so a symlink whose target leaves the skill root is not loaded. Creating that symlink from the Windows test process was denied here, so the fixture itself was not executed on Windows. Non-symlink `load_from_dir` tests passed after the canonical-root fix.

### I-MS-01

- **Invariant:** MCP tools go through governed_tools policy. A deny rule blocks the MCP tool.
- **Pre:** stdio fixture server; deny rule on its tool name.
- **Stimulus:** model or direct call.
- **Oracle:** server not receiving the call, or receiving it only if policy runs after transport — the contract is: deny means no side effect on the server. Prefer deny before write. The test must show the server did not apply a mutation.
- **Fail:** server applied the mutation.
- **Home:** `core/src/mcp/binding.rs` `denied_governed_mcp_call_does_not_reach_the_server`. A governed deny returns `Permission denied` and the recording server call list stays empty. The following allow records exactly one `lookup` call.

### I-MS-L1

- **Invariant:** live MCP stdio session records progress and a final tool result under the Flash pin.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** issue-fix MCP step.
- **Oracle:** progress event then result; server process reaped at session close.
- **Fail:** process left running, or missing progress.
- **Home:** `core/tests/test_issue_form_live_e2e.rs`. Re-proven 2026-09-20: MCP progress + update_plan checklist under remapped `boyue/bailian/deepseek-v4.1-flash`.

## planning_delegation

`session.task`, `session.tasks`, worker agents. No model-visible `parallel_task`.

### U-PD-01

- **Invariant:** `parallel_task` is not registered. Multi-item fan-out is `session.tasks` / multi-item `task`.
- **Pre:** default registry.
- **Stimulus:** lookup `parallel_task`.
- **Oracle:** unknown tool.
- **Fail:** the tool is present.
- **Home:** Layer A3; `core/src/tools/task/parallel_task.rs` must stay off the model registry. A test that the symbol still exists in source is not a failure; registration is the oracle.

### U-PD-02

- **Invariant:** fan-out is bounded. Over-cap task lists are rejected before any child starts.
- **Pre:** cap N; request N+1 tasks.
- **Stimulus:** `session.tasks`.
- **Oracle:** typed limit error; zero child sessions.
- **Fail:** N+1 children run.
- **Home:** `core/src/tools/task/tests.rs`.

### U-PD-03

- **Invariant:** cancelling the parent cancels children and does not apply their uncommitted writes.
- **Pre:** two children, one already writing in an isolated tree.
- **Stimulus:** cancel parent.
- **Oracle:** children terminal cancelled; source tree digest unchanged for uncommitted child writes.
- **Fail:** a child write lands after cancel.
- **Home:** `soak_parent_fanout_cancel_leaves_no_child_or_source_change` in `core/src/tools/task/delegation_soak.rs` (`#[ignore]`). Cancelling the parent stops admitted children and leaves the source tree unchanged. `task_tool_parent_cancellation_reaches_child_llm_call` is the single-child twin.

### U-PD-04

- **Invariant:** a worker agent cannot widen the parent permission ceiling.
- **Pre:** parent read-only; worker definition requests bash.
- **Stimulus:** `session.register_worker_agent` and run it.
- **Oracle:** bash denied; registration either rejects the widen or clamps it.
- **Fail:** worker runs bash.
- **Home:** `core/tests/test_task_permission_inheritance.rs`.

### U-PD-05

- **Invariant:** task result projection does not include another task's transcript.
- **Pre:** two tasks with different sentinel strings.
- **Stimulus:** finish both; read each result.
- **Oracle:** each result contains only its sentinel.
- **Fail:** cross-inclusion.
- **Home:** `core/src/tools/task/result_projection.rs`.

### I-PD-01

- **Invariant:** hermetic auto-delegation runs a child and returns its result to the parent tool event.
- **Pre:** fake model that delegates once.
- **Stimulus:** one parent turn.
- **Oracle:** one child terminal; parent tool result digest equals the child output digest; subagent task list empty afterwards.
- **Fail:** orphan child still running.
- **Home:** task/subagent unit tests; `core/src/subagent.rs`.

### I-PD-L1

- **Invariant:** live delegation and orchestration complete with bounded children and a cancelled sibling does not finish as success.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** `test_auto_delegation_real_parallel`, `test_orchestration_real_llm`.
- **Oracle:** child count ≤ cap; terminal states match; no `parallel_task` in the tool trace.
- **Fail:** unbounded children or a `parallel_task` event.
- **Home:** those two live files. `test_workflow_facade_real_llm` belongs to `programmable_workflows`, not this id.

## context_memory

Working, short-term, durable, and semantic memory. Active-only durable writes.

### U-CM-01

- **Invariant:** a candidate durable write stays inactive until explicit activation.
- **Pre:** empty durable store.
- **Stimulus:** candidate write, then recall, then activate, then recall.
- **Oracle:** first recall misses; second recall hits; inactive row is not in the default recall set.
- **Fail:** recall returns the candidate before activation.
- **Home:** `candidate_write_stays_inactive_until_explicit_activation` in `core/src/durable_memory/tests.rs`.

### U-CM-02

- **Invariant:** `session.remember` then `session.recall` returns the item by the documented query, and `memory_stats` counts it once.
- **Pre:** empty memory.
- **Stimulus:** remember one item, recall, remember the same item again if the API is idempotent.
- **Oracle:** one logical item; stats match; duplicate remember does not create a second active row unless the API says versions.
- **Fail:** stats say 2 for one fact, or recall misses.
- **Home:** `core/src/memory.rs`, `core/src/durable_memory.rs`.

### U-CM-03

- **Invariant:** recall does not cross session or namespace boundaries.
- **Pre:** two sessions, same text stored in A.
- **Stimulus:** recall from B.
- **Oracle:** miss.
- **Fail:** B receives A's memory.
- **Home:** `active_recall_does_not_cross_namespaces` in `core/src/durable_memory/tests.rs`. Shared repository; session B's active recall and repository query both miss `NAMESPACE-ISOLATION-91`.

### U-CM-04

- **Invariant:** semantic refresh CAS rejects a stale writer and keeps the previous index generation.
- **Pre:** index generation G; two refreshers.
- **Stimulus:** both commit.
- **Oracle:** one winner; loser gets conflict; readers see either G or G+1, never a mix.
- **Fail:** torn index.
- **Home:** `core/tests/durable_memory_semantic_refresh_cas.rs`.

### U-CM-05

- **Invariant:** failed semantic refresh does not drop the last good index.
- **Pre:** good index; refresher returns an error mid-way.
- **Stimulus:** refresh.
- **Oracle:** recall still serves the last good generation; health reports the failure.
- **Fail:** empty index after a failed refresh.
- **Home:** `core/tests/durable_memory_semantic_refresh_failure.rs`.

### I-CM-01

- **Invariant:** file-backed memory survives process restart.
- **Pre:** remember item, close store.
- **Stimulus:** reopen and recall.
- **Oracle:** item present; digest unchanged.
- **Fail:** empty store after reopen.
- **Home:** `core/tests/durable_memory_restart.rs`. Live twin: `test_memory_store_real_llm.rs`.

### I-CM-02

- **Invariant:** context assembler injects only active, in-budget memory, and records what was injected.
- **Pre:** one active item, one inactive item, budget that fits only a short item.
- **Stimulus:** build context for a turn.
- **Oracle:** active short item present; inactive absent; over-budget item absent or truncated with a marker; evidence lists the injected ids.
- **Fail:** inactive item in the prompt.
- **Home:** `core/src/context/*` tests; `core/tests/test_context_tools_real_llm.rs` is the live twin, not the hermetic proof.

### I-CM-L1

- **Invariant:** live extract-and-reopen still recalls the sentinel, and does not recall unrelated session text.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** `test_memory_store_real_llm`.
- **Oracle:** sentinel hit after reopen; a control string that was only in the prompt and not stored is a miss.
- **Fail:** pass on "model said remembered".
- **Home:** `core/tests/test_memory_store_real_llm.rs`. Memory Flash twins 2/2 re-proven 2026-09-20 (~27s).
