# Baseline execution cases

Capabilities: `governed_tools` (GT), `workspace_tools` (WT), `governance` (GV),
`web_search` (WS), `web_fetch` (WF), `program` (PG).

Kernels: effect isolation (EI), sandbox (SB), safe HTTP (SH), verify gate (VG).

## governed_tools

Validation, policy, confirmation, hooks, budgets, tracing. Direct calls and
model calls share this path.

### U-GT-01

- **Invariant:** an unknown tool name is rejected before any handler runs.
- **Pre:** default builtin registry.
- **Stimulus:** `session.governed_tool` with name `not_a_tool`.
- **Oracle:** typed unknown-tool error; workspace unchanged; no hook `pre` fired for a missing tool, or the hook sees the rejection and cannot turn it into success.
- **Fail:** the call returns an empty success payload.
- **Home:** `core/src/tools/registry.rs`, `core/src/tools/tests.rs`.

### U-GT-02

- **Invariant:** invalid parameters never reach the tool body.
- **Pre:** `read` registered.
- **Stimulus:** call `read` with a missing path and with a non-string path.
- **Oracle:** schema validation error; `read` body is not entered (no filesystem error string from the OS).
- **Fail:** an OS-level "not found" is returned instead of a schema error.
- **Home:** `core/src/tools/builtin/read.rs` schema tests.

### U-GT-03

- **Invariant:** a denied confirmation does not execute, and the denial is the tool result.
- **Pre:** policy that requires confirmation for `write`.
- **Stimulus:** governed `write`, then deny `session.confirm_tool_use`.
- **Oracle:** target file absent; pending confirmation list empty; result is denial, not a timeout success.
- **Fail:** the file exists after deny.
- **Home:** `core/src/tool_confirmation.rs`, `core/src/hitl.rs`; SDK `test_confirmation_inheritance.mjs`.

### U-GT-04

- **Invariant:** a pre-hook deny is retryable only when the hook says so, and the tool does not run.
- **Pre:** hook that denies `bash` with retryable=false.
- **Stimulus:** governed `bash`.
- **Oracle:** tool process not spawned; error carries the hook reason and retryable flag.
- **Fail:** bash starts, or retryable defaults to true.
- **Home:** `hook_block_denies_before_permission_allow` in `core/src/safety_gate.rs`. A pre-tool denial with the default flag is `HookDenied { retryable: false }` and the output says `retryable=false`. The gate returns `Deny`, so the tool body is not entered. `hook_retry_surfaces_temporary_denial_feedback` covers the retryable case.

### U-GT-05

- **Invariant:** budget exhaustion fail-closes the next governed call.
- **Pre:** budget guard with remaining 0.
- **Stimulus:** any governed tool.
- **Oracle:** typed budget error; tool not executed; counter stays 0.
- **Fail:** the tool runs and the budget goes negative.
- **Home:** `core/src/budget.rs`; `sdk/python/tests/test_budget_guard_fail_closed.py`; `sdk/node/test_budget_guard.mjs`.

### I-GT-01

- **Invariant:** `session.governed_tool` applies the session permission policy. `session.tool` is the trusted host path and does not apply that policy again.
- **Pre:** a deny rule on the tool.
- **Stimulus:** both entry points.
- **Oracle:** `governed_tool` is denied and the tool body does not run; `tool` runs once.
- **Fail:** `governed_tool` executes, or `tool` is blocked after the host already authorized it.
- **Home:** `core/src/agent_api/direct_tools/governed_tests.rs` `trusted_host_call_preserves_explicit_control_plane_authority` and `governed_host_call_obeys_permission_before_side_effects`.

### I-GT-02

- **Invariant:** tool definitions advertised to the model are exactly the registered, capability-gated set.
- **Pre:** workspace with read and no exec.
- **Stimulus:** `session.tool_definitions`.
- **Oracle:** `read` present, `bash` absent, `parallel_task` absent, `web_fetch` present.
- **Fail:** a tool is listed that `register_builtins` would skip.
- **Home:** `core/src/tools/builtin/mod.rs` registration rules; capabilities hermetic tests.

### I-GT-L1

- **Invariant:** a live turn's tool calls are the governed names, and a denied command does not run on the host.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** prompt that tries a disallowed command and an allowed `read`.
- **Oracle:** denial event present; allowed read digest matches the file; host process list has no disallowed command.
- **Fail:** the disallowed command's side effect exists.
- **Home:** `core/tests/test_harness_capabilities_live_e2e.rs`, `test_prompt_capability_real_llm.rs`. Suite 23/23 remapped Flash 2026-09-20 (~373s). Prompt-capability Flash twins 2/2 re-proven 2026-09-20 (~28s).

## workspace_tools

`read`, `write`, `ls`, `edit`, `patch`, `bash`, `glob`, `grep`, `git`,
`download`, `batch`.

### U-WT-01

- **Invariant:** `read` of a path outside the workspace root fail-closes.
- **Pre:** workspace root R; file outside R.
- **Stimulus:** `read` with `../` and with an absolute path outside R.
- **Oracle:** typed path error; outside file not in the result.
- **Fail:** file contents returned.
- **Home:** `core/src/tools/builtin/read.rs` `test_read_rejects_parent_directory_escape`; virtual resolver in `core/tests/test_workspace_backend.rs`.

### U-WT-02

- **Invariant:** JPEG, PNG, GIF, and WebP `read` results are image attachments; other types stay text.
- **Pre:** one small file of each image type, plus a `.txt`.
- **Stimulus:** `read` each.
- **Oracle:** image results carry `image/jpeg`, `image/png`, `image/gif`, `image/webp`; text result has no image part. OpenAI-compatible tool-result encoding keeps `image_url` and does not flatten the image to a text placeholder.
- **Fail:** image bytes dropped, or a text file emitted as an image.
- **Home:** `core/src/tools/builtin/read.rs` `test_read_image_returns_attachment` and `test_read_image_media_types_follow_extension_and_text_stays_text`; OpenAI `image_url` encoding in `core/src/llm/tests.rs`.

### U-WT-03

- **Invariant:** `edit` and `patch` do not write when the preimage mismatches.
- **Pre:** file contents `A`.
- **Stimulus:** edit/patch expecting `B`.
- **Oracle:** file still `A`; typed mismatch; no partial write.
- **Fail:** file becomes `B` or a mix.
- **Home:** `core/src/tools/builtin/edit.rs`, `patch.rs`.

### U-WT-04

- **Invariant:** `bash` cwd cannot escape the workspace, including `cd /` and symlink roots.
- **Pre:** workspace with a symlink pointing outside.
- **Stimulus:** `bash` `cd` to the symlink and to `/`.
- **Oracle:** command rejected or cwd stays inside the workspace; outside file not created.
- **Fail:** a file appears outside the root.
- **Home:** `core/src/tools/builtin/bash/tests.rs`; isolation `isolation_shell_cd_does_not_leave_the_worktree`.

### U-WT-05

- **Invariant:** `batch` parameter schema contains no `examples` object and no application `$ref`.
- **Pre:** registered `batch` tool.
- **Stimulus:** read `parameters()`.
- **Oracle:** `examples` is absent; serialized schema has no `"$ref"` key.
- **Fail:** any `$ref` object under parameters.
- **Home:** `core/src/tools/builtin/batch.rs` (`params.get("examples").is_none()`).

### U-WT-06

- **Invariant:** `download` refuses non-http(s), credentials in the URL, and writes only under the workspace.
- **Pre:** local root present; write capability on.
- **Stimulus:** download `file://`, `http://user:pass@host/a`, and a hermetic 200 response.
- **Oracle:** first two typed errors and no file; third writes inside the root with query string stripped from stored source URL.
- **Fail:** credential persisted in tool metadata.
- **Home:** `core/src/tools/builtin/download/tests.rs`; `safe_http_source_url` in `builtin/mod.rs`.

### U-WT-07

- **Invariant:** `git` is absent when the workspace capability `git` is off, and `write` is absent when `write` is off.
- **Pre:** capabilities struct with those flags false.
- **Stimulus:** `register_builtins`.
- **Oracle:** definitions omit `git` and `write`; calling them is unknown-tool.
- **Fail:** the tool is registered anyway.
- **Home:** `register_builtins_hides_disabled_capabilities` in `core/src/tools/builtin/mod.rs`. Code intelligence absence is also pinned in `code_intelligence/tests.rs`.

### U-WT-08

- **Invariant:** `glob` and `grep` do not follow a symlink out of the root, and respect an explicit ignore.
- **Pre:** root with `secret` outside via symlink, and a gitignored file.
- **Stimulus:** grep for a token that exists only in those files.
- **Oracle:** zero hits.
- **Fail:** the outside token is returned.
- **Home:** `core/src/tools/builtin/grep.rs`, `glob_tool.rs`.

### I-WT-01

- **Invariant:** a governed `write` then `read` round-trips bytes inside the root and records one tool span each.
- **Pre:** hermetic session, write allowed.
- **Stimulus:** write `hello`, read the same path.
- **Oracle:** read content is `hello`; two tool events; digest stable.
- **Fail:** read sees old contents, or a third implicit tool ran.
- **Home:** capabilities hermetic tests.

### I-WT-L1

- **Invariant:** live baseline turn can use `download` and `web_fetch` only through the governed path, and `batch` is present without breaking the provider schema.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** one turn that lists tools (must include `batch`) and one turn that fetches a pinned local URL if the suite does that.
- **Oracle:** provider accepts the tool list (no HTTP 500 on `$ref`); fetch result bounded.
- **Fail:** provider rejects the request because of `batch` schema, as in 8.5.12.
- **Home:** `core/tests/test_harness_capabilities_live_e2e.rs`. GLM Coding is optional, not the default pin. Suite 23/23 remapped Flash 2026-09-20 (~373s).

## governance

Permissions, confirmations, hooks, budgets, verification, sanitization, sandbox.

### U-GV-01

- **Invariant:** default permission posture denies mutation outside the policy, and Explore/read-only mode denies `write` and `bash` mutation.
- **Pre:** Explore-style policy.
- **Stimulus:** assess `write` and `read`.
- **Oracle:** `write` deny; `read` allow; the decision record contains the rule id.
- **Fail:** `write` allowed in Explore.
- **Home:** `core/src/permissions/tests.rs`, `permissions/interactive.rs`.

### U-GV-02

- **Invariant:** sanitizer redacts secrets in logs and prompt echoes.
- **Pre:** a tool result containing a known secret pattern.
- **Stimulus:** render the prompt and the log line.
- **Oracle:** secret string absent; redaction token present.
- **Fail:** secret appears in the rendered prompt or log.
- **Home:** `core/tests/test_prompt_boundaries_and_log_redaction.rs`.

### U-GV-03

- **Invariant:** budget guard set via `session.set_budget_guard` rejects the next call at the boundary, including equal-to-limit.
- **Pre:** limit of 1 unit, one unit already spent.
- **Stimulus:** one more governed call.
- **Oracle:** reject; spent stays 1.
- **Fail:** spent becomes 2.
- **Home:** `core/src/budget.rs`.

### U-SB-01

- **Invariant:** native sandbox fail-closes when the seatbelt/backend cannot be applied, and does not run the command unsandboxed.
- **Pre:** sandbox required; backend forced to fail.
- **Stimulus:** `bash`.
- **Oracle:** typed sandbox error; command not executed; workspace unchanged.
- **Fail:** command runs on the host.
- **Home:** `core/src/sandbox/native.rs`; live `live_flash_bash_through_a3s_sandbox_writes_workspace_token` in `core/tests/test_native_sandbox_live_e2e.rs`. Re-proven 2026-09-20 with `boyue/bailian/deepseek-v4.1-flash`: sandboxed bash writes the workspace token. Outside-workspace write deny remains `a3s_sandbox_0_1_3_denies_outside_workspace_write_on_this_host`.

### U-SB-02

- **Invariant:** process-host sandbox is off unless opted in. Default bash does not delegate to a process host.
- **Pre:** default session, no process-host flag.
- **Stimulus:** `bash` `echo`.
- **Oracle:** execution stays in the native/in-process sandbox path; process-host binary not spawned.
- **Fail:** an unexpected host process appears.
- **Home:** `core/src/sandbox/process_host.rs`.

### U-SB-03

- **Invariant:** process-host opt-in does not add a path filter. It sets the shell cwd to the workspace and passes the command to `bash -c` unchanged. Path isolation belongs to the outer container or VM.
- **Pre:** process-host enabled; command text names a path outside the workspace.
- **Stimulus:** build the process-host invocation.
- **Oracle:** cwd is the workspace; argv is `-c` plus the original command, including the outside path.
- **Fail:** Code rewrites or rejects the command as if it were the path boundary.
- **Home:** `process_host_does_not_filter_commands_that_name_paths_outside_the_workspace` in `core/src/sandbox/process_host.rs`. Live twin `live_process_host_bash_runs_under_flash_tool_use` checks tool execution, not a path fence. A Code-level deny would contradict the module contract.

### U-VG-01

- **Invariant:** a turn that mutates the workspace cannot complete until verification passes.
- **Pre:** verification command that fails.
- **Stimulus:** finish a turn after a successful `write`.
- **Oracle:** terminal state is not success; verification failure is on the run; the failure is visible to the model as a tool/verify result, not hidden.
- **Fail:** run snapshot says completed while the verify command exited non-zero.
- **Home:** `core/src/verification.rs`, `core/src/harness_loop.rs`. Windows host checks use the PowerShell shim: `windows_host_shell_test_and_grep_use_posix_exit_codes` and `bash_existence_check_metadata_enables_verified_completion` in `core/src/tools/builtin/bash/tests.rs`. Live twin `deepseek_flash_verified_mutation_can_complete` re-proven 2026-09-20 (~59s) with remapped `boyue/bailian/deepseek-v4.1-flash` after `test -f` returned the file's real exit code. Same day: `deepseek_flash_read_only_run_can_succeed` (~5s) and `deepseek_flash_verify_commands_reports_host_shell_effect` (~11s) also passed under the same pin. Plan-mode live twin `deepseek_flash_plan_mode_denies_an_attempted_write` passed after stimulus required an immediate write tool call (PermissionDenied; workspace unchanged).

### U-VG-02

- **Invariant:** a read-only turn does not require a mutation verify command.
- **Pre:** verify commands configured; turn only calls `read`.
- **Stimulus:** complete the turn.
- **Oracle:** success without running the mutation gate, or the gate is recorded as skipped.
- **Fail:** the turn is blocked on an unrelated compile.
- **Home:** `core/src/read_only_verifier.rs`.

### U-EI-01

- **Invariant:** discard leaves the source tree unchanged and is idempotent.
- **Pre:** git workspace; isolated worktree with an extra file.
- **Stimulus:** discard twice.
- **Oracle:** source digest unchanged; second discard succeeds; worktree unregistered.
- **Fail:** the extra file appears in source, or the second discard errors as fatal.
- **Home:** `discard_leaves_the_source_tree_unchanged`, `discard_is_idempotent_when_worktree_is_already_unregistered` in `core/src/effect_isolation.rs`.

### U-EI-02

- **Invariant:** an empty orphan `.a3s-isolate-*` sibling is removed before bind, and a symlink is not adopted as the worktree.
- **Pre:** empty `.a3s-isolate-{session}` directory; separate case with a symlink at that path.
- **Stimulus:** bind.
- **Oracle:** empty directory is replaced by a real worktree; symlink case fail-closes and does not write the source.
- **Fail:** bind adopts the symlink or fails forever on the empty orphan.
- **Home:** `bind_clears_a_stale_empty_isolation_directory`, `bind_does_not_reuse_a_symlink_as_the_isolation_worktree`.

### U-EI-03

- **Invariant:** promote applies only when the source revision is unchanged, and the same digest does not apply twice.
- **Pre:** isolated edit; source moved in one case; same digest replayed in another.
- **Stimulus:** promote, then replay the digest.
- **Oracle:** conflict case writes nothing; replay case is a no-op.
- **Fail:** promote overwrites a moved source, or replay double-applies.
- **Home:** `promote_conflicts_when_source_moves_and_does_not_apply`, `replay_of_the_same_digest_is_idempotent`.

### I-GV-01

- **Invariant:** child task inherits the parent confirmation and permission ceiling, and cannot widen it.
- **Pre:** parent denies `bash`; child task requests `bash`.
- **Stimulus:** run the child.
- **Oracle:** child bash denied; parent policy unchanged.
- **Fail:** child runs bash.
- **Home:** `core/tests/test_task_permission_inheritance.rs`; SDK confirmation inheritance tests.

### I-GV-L1

- **Invariant:** live prompt hard-gate keeps GP vs Explore differences, and ultracode discretion does not bypass a hard deny.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** the prompt-capability and ultracode suites.
- **Oracle:** Explore cannot mutate; a hard deny stays deny.
- **Fail:** a mutating tool succeeds under Explore.
- **Home:** `test_prompt_capability_real_llm.rs`, `test_ultracode_discretion_real_llm.rs`.

## web_search

HTTP, native API, and RSS on the baseline. Headless is `moli_runtime`, not this id.

### U-WS-01

- **Invariant:** billed/native providers are not called unless configured. Default engine set does not include them.
- **Pre:** empty provider config.
- **Stimulus:** `web_search` with a query.
- **Oracle:** only the default HTTP/RSS path runs; no API key header is sent; missing key is not a panic.
- **Fail:** a request to a billed host, or a panic on empty config.
- **Home:** `core/src/tools/builtin/web_search/tests.rs`, `engines.rs`.

### U-WS-02

- **Invariant:** a non-2xx or oversized body becomes a typed tool error, not model-visible HTML dumps beyond the bound.
- **Pre:** hermetic HTTP fixture.
- **Stimulus:** 500 response, then a body over the cap.
- **Oracle:** typed errors; result size ≤ cap; URL stored without query secrets.
- **Fail:** unbounded HTML in the tool result.
- **Home:** `web_search/tests.rs`.

### U-WS-03

- **Invariant:** headless/JS engines are absent unless `headless-search` is on.
- **Pre:** default features.
- **Stimulus:** request an engine that needs Moli.
- **Oracle:** typed unavailable; Moli binary not spawned.
- **Fail:** chromium or Moli starts.
- **Home:** `core/src/tools/builtin/web_search/tests.rs` `headless_engine_request_does_not_spawn_moli` (default features). Feature-on twin: `core/tests/test_web_search_headless.rs`.

### I-WS-01

- **Invariant:** search results that enter the transcript are redacted and bounded the same way as other tool results.
- **Pre:** fixture engine returning a secret-looking string and 50 hits.
- **Stimulus:** governed `web_search`.
- **Oracle:** hit count ≤ bound; secret redacted if it matches the sanitizer; one tool event.
- **Fail:** 50 raw hits appended.
- **Home:** `json_search_result_includes_only_requested_bounded_sanitized_full_text` and `json_search_result_collection_stays_valid_below_tool_transport_limit` in `core/src/tools/builtin/web_search/tests.rs`. The JSON payload the tool emits is bounded and strips URL userinfo, query secrets, and fragments. There is no second transcript copy to assert.

## web_fetch

### U-WF-01

- **Invariant:** fetch allows only public http(s), blocks private and link-local addresses, and bounds redirects.
- **Pre:** resolver fixture that maps the host to `127.0.0.1`, `169.254.169.254`, and a public IP in three cases.
- **Stimulus:** `web_fetch` each, plus a redirect loop.
- **Oracle:** private and metadata IPs denied before connect; redirect loop typed error at the cap; public fixture returns text within the size cap.
- **Fail:** a connection to the metadata IP, or unbounded redirects.
- **Home:** `core/src/tools/builtin/web_fetch/tests.rs`, `safe_http.rs`.

### U-SH-01

- **Invariant:** DNS rebinding / Fake-IP: the address checked is the address connected. A name that resolves to a public IP then to a private IP on connect is denied.
- **Pre:** resolver that changes answer between check and connect.
- **Stimulus:** `web_fetch`.
- **Oracle:** deny; no bytes from the private answer stored.
- **Fail:** body from the private target is returned.
- **Home:** `pinned_connect_ignores_a_later_private_answer` in `core/src/tools/builtin/safe_http.rs`. The check lookup returns `1.1.1.1`; the connect-time resolver returns `127.0.0.1`. `build_pinned_client` pins the checked addresses with `resolve_to_addrs`. The private listener receives no connection and `PRIVATE-BODY-91` is not stored.

### U-WF-02

- **Invariant:** `file:`, `javascript:`, and credential URLs are rejected.
- **Pre:** none.
- **Stimulus:** fetch those URLs.
- **Oracle:** typed scheme/credential error; no file read of the local path.
- **Fail:** local file contents returned.
- **Home:** `web_fetch/tests.rs`.

### I-WF-01

- **Invariant:** markdown conversion does not execute HTML script content, and the stored source URL has no query or fragment.
- **Pre:** fixture page with `<script>` and a URL with `?token=1#x`.
- **Stimulus:** governed fetch.
- **Oracle:** result text has no script execution side effect (no network from the script); stored URL has no query.
- **Fail:** token persisted.
- **Home:** `markdown_conversion_drops_script_source_and_source_url_drops_query` in `core/src/tools/builtin/web_fetch/tests.rs`. Markdown conversion drops `SCRIPT-TOKEN-91` and style bodies before `htmd`. `safe_http_source_url` stores `https://example.com/report` and drops the user, query token, and fragment.

### I-WF-L1

- **Invariant:** live fetch of an allowed URL returns bounded text and does not follow a redirect to a private host.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** capabilities suite fetch step.
- **Oracle:** bounded text; no private-host connect in the trace.
- **Fail:** unbounded body or a private connect.
- **Home:** `test_harness_capabilities_live_e2e.rs`. Suite 23/23 remapped Flash 2026-09-20 (~373s).

## program

Bounded in-process QuickJS. Not a second workflow engine.

### U-PG-01

- **Invariant:** a program that exceeds the time or memory bound is killed and cannot touch the workspace except through governed tools.
- **Pre:** script with an infinite loop; script that calls a host write directly.
- **Stimulus:** `session.program`.
- **Oracle:** timeout typed error; no direct filesystem write; a script that uses the tool bridge still hits GT policy.
- **Fail:** the loop runs past the bound, or the script writes a file without a tool event.
- **Home:** `core/tests/test_program_script_quickjs_integration.rs` `program_script_times_out_without_writing_the_workspace` and `program_script_cannot_read_outside_the_workspace`. Interrupt exceptions are caught so the deadline is reported as a timeout, not a generic QuickJS error.

### U-PG-02

- **Invariant:** program output is size-bounded and does not echo host environment secrets.
- **Pre:** env contains a secret; script prints `process.env` or the host equivalent.
- **Stimulus:** run the script.
- **Oracle:** secret absent from the tool result; output truncated at the cap with a marker.
- **Fail:** secret in the result.
- **Home:** env and direct write: `program_script_cannot_dump_host_environment_or_write_directly`. Output cap: `program_script_output_is_truncated_at_the_byte_cap`. Both in `core/tests/test_program_script_quickjs_integration.rs`.

### I-PG-01

- **Invariant:** model-visible fan-out is `task`, not `program`. `program` remains callable but is not registered as `parallel_task`.
- **Pre:** default registry.
- **Stimulus:** list definitions.
- **Oracle:** `program` present if the baseline includes it; `parallel_task` absent.
- **Fail:** `parallel_task` is model-visible.
- **Home:** Layer A3, `candidate` absence checks in harness-convergence.

### I-PG-02

- **Invariant:** a program that calls `read` outside the root is denied by the same policy as a direct tool call.
- **Pre:** script invokes `read` on `../`.
- **Stimulus:** run program.
- **Oracle:** same typed path error as U-WT-01; source unchanged.
- **Fail:** the script receives the outside file.
- **Home:** `core/tests/test_program_script_quickjs_integration.rs` `program_script_cannot_read_outside_the_workspace`. `ctx.readFile("../secret.txt")` returns the workspace-boundary error text and not the outside bytes. The script result stays a normal program return because nested tool failures are values, not thrown exceptions.
