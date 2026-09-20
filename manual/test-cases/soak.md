# Soak cases

A soak proves a bound after repetition, duration, or crash/restart:

- resource: fds, processes, temp dirs, vectors, locks, bytes, queue depth
- invariant: the same oracle as the unit case still holds at cycle N
- recovery: kill at a defined point, restart, state is the previous commit or a typed failure, never a mix

Pass criteria are numeric. "No leak" is not an oracle. Each case states the
bound. Ignored soaks are not Required CI. Record evidence next to the run;
do not paste it into this file as if it were permanent.

Platforms: where the case touches filesystems, locks, or native runtimes, run
Linux, macOS, and Windows. Otherwise Linux is enough and the waiver says so.

## Host blockers (this Windows agent)

- Ubuntu / macOS matrix soaks: macOS still hosted-CI only. **Local WSL Ubuntu
  (DESKTOP WSL2) re-proven 2026-09-20** (`~/a3s-wsl-evidence/FINAL-GOAL4.txt`):
  hermetic `--lib` 3660/0 under `--test-threads=8`; S-WR-01 default+portable;
  S-CM-01; S-EI-01; S-S3-01; plus MS/PS/WS/SG/SC/delegation and the broader
  ignored soak batch. Parallel `--lib` flakes fixed by unique agent-dir fixtures
  (`config/agent_dir.rs` `unique_temp`) and catalog wait budgets
  (`retrieval/tests/lifecycle.rs` → `external_resource_start_timeout`).
- S-WS-01: unlocked via optional `SearchEngineConfig.endpoint` (loopback HTTP / HTTPS only; SSRF unchanged). Proven Windows 2026-09-20; WSL Ubuntu 2026-09-20.
- S-S3-01: unlocked via in-repo `fixtures/s3-compat` + `A3S_S3_TEST_*`. Proven Windows 2026-09-20; **WSL Ubuntu 2026-09-20** (access key `a3s-code-akid` so bucket names cannot false-positive the leak oracle).
- S-RX-01: blocked - no append-only evidence fact store to reopen.
- Do not soak the live Flash provider account for hermetic retry/load soaks.
- Flash Layer C (boyue/bailian/deepseek-v4-flash → v4.1-flash): prior-fail + prior-pass + optional `advanced-harness` extensibility suite PASS 2026-09-20. Evidence under `%TEMP%\a3s-layer-c-bailian-rerun*`. **WSL Ubuntu 2026-09-20:** full Layer C matrix PASS (`~/a3s-wsl-evidence/FINAL-GOAL6.txt`); export `NVM_DIR` so native sandbox can re-allow the nvm toolchain under masked `$HOME`; live suite outer budgets raised to 300s where WSL provider turns exceeded 180s. Catalog blockers that remain: macOS matrix (CI), S-RX-01 (no inventable evidence store). AgentDir HTTP serve is removed; do not treat historical serve soak notes as current gates.

## Program

There is no `just soak`. Run the rows marked **wired** in CI today, and treat
the rest as specified gaps (G7).

Wired today:

| Case | Command / workflow |
| --- | --- |
| S-WR-01 | `.github/workflows/workspace-retrieval-soak.yml` (20 min, three OS targets). Windows default-feature run passed locally 2026-09-19 (46.71s). **WSL Ubuntu 2026-09-20:** default ~35s + portable ~12s. |
| S-CM-01 | `.github/workflows/durable-memory-restart-soak.yml` (20 min, three OS targets). Windows test passed locally 2026-09-19. | Re-proven 2026-09-20 Windows; **WSL Ubuntu 2026-09-20**. |
| S-EI-01 | `cargo test -p a3s-code-core --lib soak_bind_discard -- --ignored` (100 cycles; passed locally 2026-09-19). **WSL Ubuntu 2026-09-20**. |

## Retrieval and memory

### S-WR-01

- **Capability:** `workspace_retrieval`
- **Cycles:** 64 source generations, serial, `--test-threads=1`.
- **Pre:** fixture workspace, in-process vector runtime, ignored test.
- **Stimulus:** replace the source each generation and publish.
- **Oracle:** one live vector generation at the end; records released = 0 and bytes released = 0 on close; no growth of the on-disk index across the last generation versus a fresh index of the same tree.
- **Fail:** vector count increases with generation index, or close leaves a lock.
- **Home:** `agent_api::retrieval_qa_tests::repeated_source_generations_replace_vectors_without_accumulation` (`#[ignore]`). **Wired.** Windows default-feature run passed 2026-09-19 in 46.71s. Ubuntu and macOS still run only on the hosted matrix. Re-proven Windows 2026-09-20 default features (~52s).

### S-WR-02

- **Capability:** `workspace_retrieval`
- **Cycles:** 64, `--no-default-features` portable backend, same three OS targets.
- **Oracle:** same as S-WR-01. The portable backend must not pass by skipping publication.
- **Fail:** the no-default-features job is skipped or asserts a weaker count.
- **Home:** second step of `workspace-retrieval-soak.yml`. **Wired.** Windows `--no-default-features` run of `repeated_source_generations_replace_vectors_without_accumulation` passed 2026-09-19 in 29.90s. Ubuntu and macOS still run only on the hosted matrix. Re-proven Windows 2026-09-20.

### S-CM-01

- **Capability:** `context_memory`
- **Cycles:** the endurance fixture's restart count (four independent close/resume cycles is the manual gate in `DURABLE_MEMORY.md`; the eval test is the executable form).
- **Stimulus:** remember, close, reopen, recall, repeat.
- **Oracle:** each sentinel still recalls after the last restart; inactive candidates stay inactive; store byte size is O(active items), not O(restarts).
- **Fail:** a miss after restart, or store size grows on identical active set.
- **Home:** `repeated_restart_reuse_preserves_every_context_and_current_revision` in `core/tests/durable_memory_restart_endurance_eval.rs`. Three-OS gate: `.github/workflows/durable-memory-restart-soak.yml`, called from `ci.yml` and `release.yml`. Windows run of that test passed. Ubuntu and macOS run only on the hosted matrix. Re-proven Windows 2026-09-20 (`--features durable-memory-sqlite`, ~6s).

### S-CM-02

- **Capability:** `context_memory`
- **Cycles:** 32 semantic refresh attempts, half forced to fail, with concurrent reader recalls.
- **Oracle:** readers never observe a torn generation; last successful generation remains recallable; failed attempts do not append a generation.
- **Fail:** recall returns mixed chunks from two generations.
- **Home:** `core/tests/semantic_generation_soak.rs` `soak_failed_semantic_publications_do_not_tear_generations` (`#[ignore]`). Thirty-two `replace_namespace` publications, the refresh CAS kernel, reject every odd attempt before publish. A concurrent search never sees a mixed or partial generation, and the index revision advances only on the 16 successes. Re-proven Windows 2026-09-20 (`--test semantic_generation_soak --ignored`).

## Runtime and persistence

### S-RT-01

- **Capability:** `agent_runtime`
- **Cycles:** 200 open/close on one workspace, one process.
- **Oracle:** after the loop, fd count ≤ start + 5; no `.a3s-isolate-*` orphan left; a final open succeeds.
- **Fail:** fd growth linear with iterations, or the 201st open hits a lock held by a closed session.
- **Home:** `core/tests/test_session_open_close_soak.rs` `soak_two_hundred_session_open_close_cycles_stay_bounded` (`#[ignore]`). The +5 handle bound is measured after the first open, which installs process-lifetime runtime handles; later cycles must stay flat. Re-proven Windows 2026-09-20 (`--test test_session_open_close_soak --ignored`).

### S-CV-01

- **Capability:** `conversation`
- **Cycles:** 100 turns with a fake model, compaction enabled, each turn carrying a unique secret that must not survive compaction.
- **Oracle:** history API length ≤ compaction window; none of the expired secrets appear in history or logs; transcript file size ≤ bound derived from the window.
- **Fail:** transcript file grows by the full turn size after compaction, or a secret from turn 1 remains.
- **Home:** `soak_compaction_drops_expired_secrets` in `core/src/agent/conversation_compaction_soak.rs` (`#[ignore]`). The fixture answers `ack` and never echoes the prompt. History length is bounded by `KEEP_RECENT_MESSAGES`. Re-proven Windows 2026-09-20 (`--lib --ignored`, ~56s).

### S-RC-01

- **Capability:** `run_control`
- **Cycles:** 50 interrupt storms on a fake tool that blocks.
- **Oracle:** every run reaches exactly one terminal state; tool side effect count equals the number of runs that passed the tool boundary before interrupt, not 50 if all were interrupted in time; duplicate steer tokens never double-apply.
- **Fail:** a run left in `running`, or a side effect after the interrupt receipt.
- **Home:** `soak_interrupt_storm_settles_once_without_late_effects` in `core/tests/test_run_control_interrupt_soak.rs` (`#[ignore]`). The tool counts an effect only after it observes that the invocation was not cancelled. Duplicate steer and interrupt tokens must keep the same receipt sequence. Re-proven Windows 2026-09-20 (`--test test_run_control_interrupt_soak --ignored`, ~28s).

### S-PE-01

- **Capability:** `persistence`
- **Cycles:** 40 save / kill-process / resume. Kill during rename on 5 of the cycles (fault injection).
- **Oracle:** every successful resume matches the last durable snapshot digest; killed-during-rename resumes the previous digest; no cycle panics on the next open.
- **Fail:** a resume that mixes fields from two snapshots.
- **Home:** `soak_snapshot_resume_keeps_one_generation_across_rename_crashes` in `core/src/store/persistence_soak.rs` (`#[ignore]`). Five of the forty cycles append a WAL intent and a temp snapshot, then stop before rename. Resume must match the last durable digest. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-RO-01

- **Capability:** `run_observability`
- **Cycles:** append events until 20 pages, then project.
- **Oracle:** each page ≤ size bound; cursor walk reconstructs the log; disk of the event log ≤ retention cap after a retention pass.
- **Fail:** a page over the bound, or retention never deletes.
- **Home:** `soak_event_log_retention_deletes_after_twenty_pages` in `core/tests/test_run_observability_soak.rs` (`#[ignore]`). The byte cap is two pages of the measured event record. The cumulative count stays at twenty pages; the retained window is smaller and the cursor reports the gap. Re-proven Windows 2026-09-20 (`--test test_run_observability_soak --ignored`).

### S-PS-01

- **Capability:** `priority_scheduling`
- **Cycles:** 1000 admit/finish with capacity 2, mixed priorities, plus 50 handler replacements.
- **Oracle:** depth never negative and never above the queued remainder; no deadlock within the test timeout; high priority does not starve past the documented fairness window (state the window in the test, do not leave it implicit).
- **Fail:** test timeout, or a job that waits longer than the fairness window while lower-priority jobs start.
- **Home:** `core/tests/test_priority_scheduler_soak.rs` `soak_priority_admission_stays_within_capacity` (`#[ignore]`). The fairness window is three `aging_interval_ms` steps, the distance from maintenance to interactive. Re-proven Windows 2026-09-20 (`--test test_priority_scheduler_soak --ignored`).

## Execution, sandbox, web, program

### S-EI-01

- **Capability:** kernel for `workspace_tools` / `governance`
- **Cycles:** 100 bind / mutate / discard, and 20 bind / promote, on one git root.
- **Oracle:** source digest equals the promoted set only; discard cycles leave source digest unchanged; zero orphan `.a3s-isolate-*` directories after close; no symlink adoption.
- **Fail:** source drift on a discard cycle, or orphan count > 0.
- **Home:** `soak_bind_discard_does_not_leak_into_source` in `core/src/effect_isolation.rs` (`#[ignore]`, 100 cycles). Single-shot discard tests cover the same oracle once. Re-proven Windows 2026-09-20 (`--lib --ignored`, ~79s).

### S-SB-01

- **Capability:** `governance` sandbox
- **Cycles:** 100 sandboxed `bash` echoes, then 20 expected sandbox-init failures.
- **Oracle:** zero unsandboxed executions; zero child processes left; failure cycles do not fall back to host execution.
- **Fail:** a child alive after the session drop, or a failure cycle that still ran the command.
- **Home:** `core/src/tools/builtin/bash/sandbox_soak.rs` `soak_sandboxed_bash_does_not_fall_back_to_the_host` (`#[ignore]`). Re-proven Windows 2026-09-20 (`--lib --ignored`, ~58s).

### S-WT-01

- **Capability:** `workspace_tools`
- **Cycles:** 100 `bash` calls in one shell session, including `cd` attempts out of the root.
- **Oracle:** cwd stays inside the root at every sample; zombie count 0; a write outside the root never appears.
- **Fail:** cwd escapes, or process count grows per call (session not reused, and not reaped).
- **Home:** `soak_rooted_shell_keeps_cwd_inside_one_hundred_commands` in `core/src/shell_session.rs` (`#[ignore]`). Re-run 2026-09-20 passed in 0.04s. `cd` admission is the session kernel; detached jobs stay at zero across the 100 commands. Windows host shells bind a Job Object so cancel, explicit kill, and timeout stop descendants before a later write: `test_dropping_bash_execution_kills_shell_before_later_side_effects`, `killing_a_detached_job_stops_its_descendant_before_a_later_write`, and `timing_out_a_shell_stops_its_descendant_before_a_later_write`. Capture timeouts use the same Job Object through `ProcessGroupGuard`: `process_group_guard_kills_a_windows_descendant_before_a_later_write` and `read_process_output_timeout_kills_a_windows_descendant_before_a_later_write` in `core/src/tools/process.rs`. Live twin `live_harness_cancel_stops_in_flight_tool` asserts the late leak file is absent after protocol Cancel.

### S-GT-01

- **Capability:** `governed_tools`
- **Cycles:** 200 governed calls after the budget is exhausted, mixed with 50 denies.
- **Oracle:** execution count stays 0; budget counter does not go negative; pending confirmation list empty at the end.
- **Fail:** any tool body entered.
- **Home:** `core/tests/test_governed_budget_soak.rs` `soak_exhausted_budget_rejects_two_hundred_governed_calls` (`#[ignore]`). Re-proven Windows 2026-09-20 (`--test test_governed_budget_soak --ignored`).

### S-WS-01

- **Capability:** `web_search`
- **Cycles:** 100 searches against a local HTTP fixture, connection pool enabled.
- **Oracle:** open connection count ≤ pool cap at idle; result bytes per call ≤ cap; no file descriptor growth after idle.
- **Fail:** connections = 100 after idle.
- **Home:** `soak_web_search_loopback_fixture_stays_connection_and_byte_capped` in `core/src/tools/builtin/web_search/tests.rs` (`#[ignore]`). Native API engines (`tavily`/`anysearch`) accept optional `SearchEngineConfig.endpoint`; validation stays in `a3s-search` (`validate_provider_endpoint`: HTTPS or loopback HTTP). Proven Windows 2026-09-20 (`--lib --ignored`).

### S-WF-01

- **Capability:** `web_fetch`
- **Cycles:** 50 fetches, 10 of which redirect toward a private address on hop 2.
- **Oracle:** those 10 denied; accepted bodies ≤ size cap; no private connect in the fixture log.
- **Fail:** a private connect, or body storage growing without bound in the transcript.
- **Home:** `soak_private_redirect_never_connects_and_body_stays_capped` in `core/src/tools/builtin/web_fetch/tests.rs` (`#[ignore]`). Hop 2 is rejected by `redirect_target` before connect. A public hop-1 fixture cannot be bound without bypassing the private-address check, so the body cap is asserted on `content_range`. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-PG-01

- **Capability:** `program`
- **Cycles:** 30 QuickJS runs, 10 of which infinite-loop until the time bound.
- **Oracle:** each infinite run returns by the bound; RSS after the loop ≤ RSS at start + a fixed slack (set the slack in the test from a baseline measurement, do not use "looks fine"); no workspace file written except via a governed tool.
- **Fail:** a run exceeds the bound, or RSS steps up every iteration.
- **Home:** `soak_program_script_timeout_cycles_stay_bounded` in `core/tests/test_program_script_quickjs_integration.rs` (`#[ignore]`). Slack is four times the larger of the idle resident-set span and one infinite run's growth. Re-proven Windows 2026-09-20 (`--test test_program_script_quickjs_integration --ignored`).

## Model, MCP, planning

### S-MA-01

- **Capability:** `model_adapters`
- **Cycles:** 40 turns where the HTTP fixture returns 500 twice then 200, and the turn includes one tool.
- **Oracle:** tool execution count = 40, not 120; retry count per turn ≤ cap.
- **Fail:** tool count tracks retries.
- **Home:** `soak_http_retries_do_not_multiply_tool_execution` in `core/src/agent/model_retry_soak.rs` (`#[ignore]`). The fixture returns HTTP 500 twice and then 200. Retry stays inside the model client (`max_retries = 2`). Re-proven Windows 2026-09-20 (`--lib --ignored`, ~24s).

This is not a live provider soak. Do not soak the Flash account.

### S-SO-01

- **Capability:** `structured_output`
- **Cycles:** 25 invalid objects that never validate.
- **Oracle:** each call stops at the repair bound; total model requests = calls × (1 + repair cap); no success result.
- **Fail:** a call that keeps repairing, or an invalid object returned as success.
- **Home:** `soak_invalid_objects_stop_at_the_repair_bound` in `core/src/llm/structured_repair_soak.rs` (`#[ignore]`). The fixture returns `not-an-object`. Model calls must equal `25 × (1 + repair cap)`. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-MS-01

- **Capability:** `mcp_and_skills`
- **Cycles:** 30 add / call / remove of a stdio fixture server.
- **Oracle:** child process count 0 after each remove; 30th add still works; no growth in `session.mcps` after remove.
- **Fail:** zombie count > 0.
- **Home:** `core/tests/test_mcp_stdio_soak.rs` `soak_mcp_stdio_add_call_remove_leaves_no_child` (`#[ignore]`). Re-proven Windows 2026-09-20 (`--test test_mcp_stdio_soak --ignored`).

### S-PD-01

- **Capability:** `planning_delegation`
- **Cycles:** 20 parent runs that each fan out to the cap and cancel halfway.
- **Oracle:** no child alive at the end; source digest unchanged for cancelled work; child count per parent ≤ cap.
- **Fail:** a child process or session left running.
- **Home:** `core/src/tools/task/delegation_soak.rs` `soak_parent_fanout_cancel_leaves_no_child_or_source_change`. Re-proven Windows 2026-09-20 (`--lib --ignored`, ~30s).

## Advanced

### S-CI-01

- **Capability:** `code_intelligence`
- **Cycles:** 50 symbol queries against a fixture service that exits on query 30.
- **Oracle:** queries 31–50 are typed errors; service process reaped; no query hangs past the tool timeout.
- **Fail:** a hung call.
- **Home:** `core/src/code_intelligence/language_runtime/integration_tests.rs` `soak_symbol_queries_fail_closed_after_server_exit` (`#[ignore]`). The fixture exits on `workspace/symbol` query `terminate-process` (cycle 30). Later queries must be typed errors, not a respawned server. Re-proven Windows 2026-09-20 (`--lib --ignored`, ~2s).

### S-CP-01

- **Capability:** `cognitive_packages`
- **Cycles:** 20 bind replacements, generation G then G+1 alternating.
- **Oracle:** after each bind, prompt assembly contains only the current digest; retained package bytes = one generation.
- **Fail:** both generations retained.
- **Home:** `core/src/run/tests/run_cognitive_binding_soak.rs` `soak_alternating_cognitive_binds_keep_one_generation` (`#[ignore]`). A second generation is `RunCognitiveBindingError::Conflict`; the snapshot keeps one digest. Host CAR qualification stays external. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-UR-01

- **Capability:** `use_runtime_tasks`
- **Cycles:** 40 governed calls, host latency injected, deny on every 4th call.
- **Oracle:** denied calls do not reach the host counter; host counter = 30; no extra task shadows `read`.
- **Fail:** host counter 40.
- **Home:** `core/src/use_runtime_tasks.rs` `soak_governed_use_calls_skip_denied_host_dispatch` (`#[ignore]`). Forty governed calls deny every fourth before dispatch. The host counter stays 30, and registering the task does not replace `read`. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-PW-01

- **Capability:** `programmable_workflows`
- **Cycles:** 15 resumable workflows, kill the process after step 1, resume.
- **Oracle:** step 1 side effect count = 15 not 30; step 2 runs once after resume; missing checkpoint fail-closes rather than restarting.
- **Fail:** step 1 runs again.
- **Home:** `core/src/orchestration/combinators/tests/resume_soak.rs` `soak_resume_does_not_repeat_completed_steps` (`#[ignore]`). Fifteen workflows journal step 1, lose step 2 in flight, then resume. Step 1 runs 15 times, not 30. Step 2 succeeds once on resume. An unreadable checkpoint fails closed and runs nothing. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-SG-01

- **Capability:** `state_graph`
- **Cycles:** 100 patches and 10 forks.
- **Oracle:** hash chain verifies; parent heads of forks unchanged; disk ≤ bound proportional to events, no orphan fork dirs.
- **Fail:** restore of the head fails, or a fork mutates its parent.
- **Home:** `core/src/state_graph/graph_soak.rs` `soak_patches_and_forks_keep_parent_heads_and_bounded_disk`. Re-proven Windows 2026-09-20 (`--features state-graph --lib --ignored`).

### S-AR-01

- **Capability:** `agent_release_contract`
- **Cycles:** 50 verify/admit of one good manifest and 50 of a bad digest.
- **Oracle:** 50 admits collapse to one binding; 50 bad verifies never admit; receipt id stable.
- **Fail:** binding count ≠ 1.
- **Home:** `core/tests/agent_release_manifest.rs` `soak_release_admits_collapse_to_one_binding`. Re-proven Windows 2026-09-20 (`--test agent_release_manifest --ignored`).

### S-AP-01

- **Capability:** `agent_protocol`
- **Cycles:** 30 start / tool / cancel / recover.
- **Oracle:** no cancelled run returns to running; event seq strictly increases per run; session count at the end equals open sessions, not 30 leaked.
- **Fail:** session directory count grows by 30.
- **Home:** `core/src/run/tests/run_protocol_soak.rs` `soak_cancelled_runs_stay_terminal_and_do_not_leak_sessions` (`#[ignore]`). Thirty in-memory start, tool, cancel, and recover cycles stay `Cancelled`, and event sequences stay strictly increasing. This does not open protocol sessions, so it does not by itself prove the Harness session map. Re-proven Windows 2026-09-20 (`--lib --ignored`).

### S-EV-01

- **Capability:** `evaluation_substrate`
- **Cycles:** 20 dispatch claims restarted mid-award.
- **Oracle:** one award per claim; result store readable after each restart.
- **Fail:** double award.
- **Home:** `core/src/evaluation/dispatch_ledger/tests/eval_award_soak.rs` `soak_restarted_dispatch_awards_once` (`#[ignore]`, Cargo feature `evaluation`). Each of 20 claims is dropped before award, reopened, awarded once, and reopened again. A second receipt conflicts, and a later claim stays `Completed`. Re-proven Windows 2026-09-20 (`--features evaluation --lib --ignored`).

### S-MO-01

- **Capability:** `moli_runtime`
- **Cycles:** 10 concurrent `moli.ensure` across processes, then a digest-mismatch cache.
- **Oracle:** one installed runtime; lock released; mismatch does not spawn.
- **Fail:** lock file still held, or two different binaries in the cache.
- **Home:** `core/src/moli_runtime/tests/install_soak.rs` `soak_concurrent_moli_install_publishes_one_binary` (`#[ignore]`, Cargo feature `headless-search`). Ten concurrent installs in one process download once, release `.install.lock`, and a later digest mismatch leaves that one binary in place. Separate OS processes are not spawned. Re-proven Windows 2026-09-20 (`--features headless-search --lib --ignored`).

### S-S3-01

- **Capability:** `s3_workspace`
- **Cycles:** 40 put/get/delete against the hermetic backend, including 5 oversize puts.
- **Oracle:** object count returns to the prefix baseline; oversize puts absent; error strings contain no key id.
- **Fail:** object count climbs, or credentials in an error.
- **Home:** `soak_s3_put_get_delete_returns_to_prefix_baseline` in `core/tests/test_s3_backend.rs` (`#[ignore]`, `--features s3`). Start `fixtures/s3-compat`, set `A3S_S3_TEST_*`, then run the soak. Writes larger than `max_read_bytes` fail closed and leave no object. Proven Windows 2026-09-20.

### S-SV-01


- **Capability:** `opentelemetry`
- **Cycles:** 100 spans with the collector refusing connections.
- **Oracle:** coding turns still succeed; exporter retry buffer ≤ cap; process RSS does not grow linearly with spans after the cap.
- **Fail:** turn failures, or unbounded queue.
- **Home:** Turn success with a refusing collector is `collector_down_does_not_fail_the_tool_turn` in `core/tests/test_telemetry_collector_down.rs` (`#[ignore]`, `--features telemetry`). `TelemetryConfig::init` uses the SDK batch exporter. The retry-queue length and process RSS are not readable from Code, so those bounds stay unasserted. Re-proven Windows 2026-09-20 (`--features telemetry --test test_telemetry_collector_down --ignored`).

### S-RX-01

- **Capability:** research contracts (feature `research`, not an `sdk_capabilities()` id)
- **Cycles:** 40 append-only evidence facts, reopen, then 10 rejected orphan-citation graphs.
- **Oracle:** fact sequence monotonic and gap-free after reopen; rejected graphs do not append; store contains digests, not source plaintext.
- **Fail:** a sequence gap, or plaintext retained.
- **Home:** Blocked. `mixed_run_or_orphan_citation_fail_closed` is single-shot. There is no append-only evidence fact store to reopen. Do not invent one.

## Waivers

These are not missing soaks. They have no retained resource of their own.
The soak that protects them is the owner case.

| ID | Why no extra soak | Owner case |
| --- | --- | --- |
| Image `read` media types | Pure encoding of one result. | S-WT-01 does not cover images. Add no duration test. The unit oracle U-WT-02 is sufficient. |
| `batch` schema pin | Static schema. Repeating it cannot leak. | U-WT-05. |
| `priority_scheduling` "not required for correctness" | The counters can still drift. | S-PS-01 is required. This row is not a waiver of S-PS-01. |

No capability id is waived entirely. The inventory in
[README.md](README.md) lists 30 ids; this file has a `S-` case for each:

`RT CV RC PE RO PS GT WT WS WF PG MA SO MS PD CM WR CI CP UR PW SG AR AP EV MO S3 SV OT`,
plus `RX` for the `research` feature.

`GV` is covered by S-SB-01 and S-GT-01 (sandbox and budget are the retained
resources). `SH` is covered by S-WF-01. `EI` and `VG` are covered by S-EI-01
and S-GT-01 / S-PD-01 (uncommitted writes). If a future capability id is
added to `CAPABILITY_SPECS`, add a soak row in the same change.
