# Baseline runtime cases

Capabilities: `agent_runtime` (RT), `conversation` (CV), `run_control` (RC),
`persistence` (PE), `run_observability` (RO), `priority_scheduling` (PS).

Operations are the names in `sdk_capabilities()`. Oracles are kernel effects.

## agent_runtime

Create, bind, resume, replace, and close sessions.

### U-RT-01

- **Invariant:** `agent.session` binds exactly the supplied workspace root.
- **Pre:** empty temp directory.
- **Stimulus:** create one session on that directory.
- **Oracle:** session id is non-empty; recorded root canonicalizes to the temp directory; no file is written inside the workspace except Code-owned metadata the API documents.
- **Fail:** empty id, root points elsewhere, or workspace files appear that the caller did not request.
- **Home:** `core/src/agent.rs` / `core/src/agent_api.rs` (suite).

### U-RT-02

- **Invariant:** `agent.close` is idempotent and releases the session.
- **Pre:** one open session.
- **Stimulus:** close twice.
- **Oracle:** both calls succeed or the second returns a typed already-closed error that does not panic; a new session can bind the same root afterwards.
- **Fail:** panic, stuck lock, or the second close deletes another session's state.
- **Home:** `core/tests/test_session_close_lifecycle.rs`; SDK `test_session_close.mjs`, `tests/test_session_close.py`.

### U-RT-03

- **Invariant:** resume of an unknown id fail-closes.
- **Pre:** empty store.
- **Stimulus:** `agent.resume_session` with a random id.
- **Oracle:** typed not-found error; store directory count unchanged.
- **Fail:** a new empty session is created under the missing id, or the error is a panic.
- **Home:** `core/tests/test_resume_unknown_session.rs` `resume_unknown_session_does_not_create_a_store_file`. Memory-store coverage also lives in `core/src/agent_api/tests/persistence.rs` `test_resume_session_not_found`.

### U-RT-04

- **Invariant:** replace does not alias two sessions onto one workspace writer.
- **Pre:** session A bound to root R.
- **Stimulus:** replace or create session B on R while A is open, per the API contract (exclusive).
- **Oracle:** the loser gets a typed busy/lock error; the winner is the only writer.
- **Fail:** both sessions write R.
- **Home:** `core/src/store/tests.rs` `file_store_writer_lease_takeover_fences_stale_holder`. A second opener takes the next epoch. The previous holder then fails the write with a lease-lost error, so both sessions cannot commit. The lock does not return a busy error on the second acquire.

### I-RT-01

- **Invariant:** Node, Python, and Go create and close a session with the same id shape and the same close error on double-close.
- **Pre:** hermetic temp workspace; no model call.
- **Stimulus:** session create, close, close through each SDK.
- **Oracle:** alignment script passes; each SDK's second close matches the Rust typed outcome.
- **Fail:** one language creates a session the others cannot close, or a language swallows the error.
- **Home:** `scripts/sdk_api_alignment_check.mjs`; `sdk/node/test_session_close.mjs`; `sdk/python/tests/test_session_close.py`; `sdk/go/session_test.go`.

### I-RT-02

- **Invariant:** default `local-code` tree does not pull Advanced server, S3, headless, or evaluation crates.
- **Pre:** clean checkout.
- **Stimulus:** `cargo check -p a3s-code-core` and the tree check in `just harness-convergence-check`.
- **Oracle:** dependency tree has no `evaluation` / `chromiumoxide` / `s3` on the default features.
- **Fail:** an Advanced crate is in the default tree.
- **Home:** Layer A1 in [FIRST_PRINCIPLES_E2E.md](../FIRST_PRINCIPLES_E2E.md).

## conversation

Send, run, stream, attachments, history, cancel.

### U-CV-01

- **Invariant:** cancel of a turn that has not started is a no-op receipt, not a partial transcript write.
- **Pre:** session with empty history.
- **Stimulus:** `session.cancel` before any send.
- **Oracle:** history length stays 0; receipt says nothing was cancelled or a typed idle error.
- **Fail:** a synthetic user/assistant message is appended.
- **Home:** `core/tests/test_idle_cancel.rs` `idle_cancel_does_not_write_a_transcript`.

### U-CV-02

- **Invariant:** history returns the persisted turns, not the model-visible prompt after compaction, unless the API says so.
- **Pre:** session with two committed turns and a compaction watermark.
- **Stimulus:** `session.history`.
- **Oracle:** both committed turns are present, or the response includes an explicit compaction marker and the retained window matches the watermark. Secrets in tool payloads are redacted.
- **Fail:** silent drop of a committed turn, or a secret string in the history payload.
- **Home:** `core/src/compaction.rs`, `core/src/transcript.rs` (suite). Redaction: `core/tests/test_prompt_boundaries_and_log_redaction.rs`.

### U-CV-03

- **Invariant:** attachment send stores the attachment digest and does not inline unbounded bytes into the transcript.
- **Pre:** a small PNG and a file over the attachment cap.
- **Stimulus:** `session.send_with_attachments` for each.
- **Oracle:** small PNG is accepted with a content digest; over-cap file is a typed size error and is not written into history.
- **Fail:** over-cap bytes land in the transcript or the digest changes across a reload of the same bytes.
- **Home:** `reloaded_png_keeps_the_same_content_digest` and `over_cap_attachment_is_rejected_before_history` in `core/src/agent_api/tests/attachment_cap.rs`. A reloaded PNG keeps the same SHA-256. `send_with_attachments` rejects a body over `MAX_ATTACHMENT_BYTES` with `SessionConfiguration` before history, and the error text does not contain `OVERCAP-BYTES-91`.

### U-CV-04

- **Invariant:** stream cancellation stops tool execution that has not been committed.
- **Pre:** hermetic fake model that emits a tool call, then a cancel flag before the tool starts.
- **Stimulus:** `session.stream` plus cancel.
- **Oracle:** tool effect is discarded (see EI); transcript terminal state is cancelled; no workspace file from that tool remains.
- **Fail:** the tool's write is visible in the source tree.
- **Home:** `core/src/effect_isolation.rs` discard tests; live twin `deepseek_stream_cancellation_settles_the_run_and_tool_state` in `core/tests/test_deepseek_adversarial_e2e.rs`. Re-proven 2026-09-20 with `boyue/bailian/deepseek-v4.1-flash` after the Windows host-shell Job Object: cancel settles the run and `cancel-leak.txt` stays absent. Full adversarial suite 3/3 re-proven 2026-09-20 (~61s): injection gate, outside-path read deny, stream cancel.

### I-CV-01

- **Invariant:** a hermetic fake-model turn appends one user turn and one assistant terminal state.
- **Pre:** scripted model fixture, no network.
- **Stimulus:** `session.send` with a prompt that triggers no tool.
- **Oracle:** history length +2; run snapshot terminal state is completed; event page contains the same run id.
- **Fail:** duplicate assistant messages, or a completed state with an open tool.
- **Home:** agent loop unit tests under `core/src/harness_loop.rs` / `core/src/agent.rs`.

### I-CV-L1

- **Invariant:** live Flash stream emits tool names and ids, never empty names, for a turn that uses a baseline tool.
- **Pre:** Layer C config pin. Ignored.
- **Stimulus:** one streamed turn that must call `read` or `bash` inside the sandbox.
- **Oracle:** every tool-call event has a non-empty name and id; final snapshot is terminal.
- **Fail:** empty tool name, or a hung run past the 420s harness-loop budget.
- **Home:** `core/tests/test_issue_form_live_e2e.rs`, `core/tests/test_harness_loop_live_e2e.rs`. Harness-loop Flash twins re-proven 2026-09-20 under remapped `boyue/bailian/deepseek-v4.1-flash`: read-only succeed, verified mutation, and verify_commands host-shell effect. Same day `test_issue_form_live_e2e` 4/4 passed (~112s): streaming tool names, MCP stdio progress, oversized tool-end page projection, process-host bash.

## run_control

Steer and cooperative interrupt with idempotent receipts.

### U-RC-01

- **Invariant:** interrupt of an idle session returns an idempotent idle receipt and does not append a turn.
- **Pre:** open session, no active run.
- **Stimulus:** `session.interrupt` twice with the same client token.
- **Oracle:** both receipts match; history unchanged; snapshot shows no active run.
- **Fail:** two different receipts for the same token, or a fabricated cancelled turn.
- **Home:** `core/tests/run_control_runtime.rs`.

### U-RC-02

- **Invariant:** steer during an active run is ordered after the current tool boundary and does not apply twice.
- **Pre:** fake model in a tool call; steer payload with token T.
- **Stimulus:** deliver T twice.
- **Oracle:** one steer applied; second receipt is duplicate; tool that already started finishes or is cancelled as a unit, not half-applied.
- **Fail:** the steer text appears twice in the next model request.
- **Home:** `core/src/run_control.rs`, `core/tests/run_control_runtime.rs`. Live twin `real_model_applies_steer_after_an_in_flight_tool` in `core/tests/test_run_control_real_llm.rs` re-proven 2026-09-20 with `boyue/bailian/deepseek-v4.1-flash`: steer is accepted after bash starts, `steer-finished.txt` appears, and the run does not narrative-complete.

### U-RC-03

- **Invariant:** optimistic turn guard rejects a steer stamped for a stale turn.
- **Pre:** turn N active; client sends a steer stamped N-1.
- **Stimulus:** `session.steer`.
- **Oracle:** typed conflict; turn N is unchanged.
- **Fail:** stale steer is concatenated into turn N.
- **Home:** `stale_turn_and_duplicate_conflict_are_rejected` in `core/src/run_control.rs`. After the turn advances, a steer stamped with the previous turn id and revision returns `RunControlError::StaleTurn` and is not queued.

### I-RC-01

- **Invariant:** snapshot, interrupt, and the event page agree on the terminal state.
- **Pre:** hermetic run that blocks in a fake tool until interrupt.
- **Stimulus:** interrupt, then `run_control_snapshot` and `run_event_page`.
- **Oracle:** one terminal state across all three; no later event reopens the run.
- **Fail:** snapshot says cancelled while the page says running.
- **Home:** `core/tests/run_control_runtime.rs`.

### I-RC-L1

- **Invariant:** a live run stops when interrupted and does not keep calling tools.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** start a run, interrupt, wait for terminal.
- **Oracle:** no tool event after the interrupt receipt; terminal state is interrupted or cancelled.
- **Fail:** a workspace write after the receipt.
- **Home:** `real_model_interrupt_cancels_an_in_flight_tool_without_late_effects` and `real_model_session_continues_after_interrupt` in `core/tests/test_run_control_real_llm.rs`. Re-proven 2026-09-20 with `boyue/bailian/deepseek-v4.1-flash` after the Windows host-shell Job Object: interrupt settles the run, `interrupt-leak.txt` stays absent, and the same session still accepts a follow-up turn that yields `CONTINUE_OK`.

## persistence

Atomic save and restore of session, runs, artifacts, traces, verification.

### U-PE-01

- **Invariant:** `session.save` is atomic: a crash mid-write leaves the previous snapshot readable.
- **Pre:** one committed snapshot S0.
- **Stimulus:** start a save of S1 and truncate the temp file before rename.
- **Oracle:** resume reads S0, not a partial S1; no mixed fields.
- **Fail:** resume returns a struct that fails schema validation or mixes S0 and S1 fields.
- **Home:** `soak_snapshot_resume_keeps_one_generation_across_rename_crashes` in `core/src/store/persistence_soak.rs` (`#[ignore]`). Forty generations, five crash temps written after the WAL intent and before rename. Resume digest and prompt stay the last durable generation; the name never starts with `TORN`.

### U-PE-02

- **Invariant:** artifact get returns the bytes whose digest was stored, or not-found.
- **Pre:** save an artifact with digest D.
- **Stimulus:** `session.get_artifact` for D and for a random digest.
- **Oracle:** D returns the original bytes; unknown digest is typed not-found; neither call writes the workspace.
- **Fail:** digest mismatch, or unknown digest returns empty success.
- **Home:** `core/tests/immutable_content_adapter.rs`; SDK immutable-content fixtures.

### U-PE-03

- **Invariant:** checkpoint schema rejects an unknown version instead of best-effort loading.
- **Pre:** a checkpoint file with `schema` set to a future version.
- **Stimulus:** resume.
- **Oracle:** typed schema error; store not rewritten.
- **Fail:** silent skip of unknown fields that drops verification evidence.
- **Home:** `core/tests/test_persisted_schema_roundtrip.rs`.

### I-PE-01

- **Invariant:** save, process exit, resume restores run id, artifact digest, and verification evidence together.
- **Pre:** hermetic session that completed one verified turn.
- **Stimulus:** save, drop the in-memory session, `agent.resume_session`.
- **Oracle:** run id, artifact digest, and verification record match the pre-exit snapshot.
- **Fail:** run restored without its verification record, or the reverse.
- **Home:** `core/tests/agent_exact_checkpoint_recovery_v1.rs`, `agent_portable_checkpoint_recovery_v1.rs`, `agent_live_checkpoint_export_v1.rs`.

### I-PE-02

- **Invariant:** SDK checkpoint export fixtures match the Rust digest.
- **Pre:** shared fixture bytes.
- **Stimulus:** export through Node, Python, and Go.
- **Oracle:** the three exports and the Rust export have the same digest and schema id.
- **Fail:** a language drops a field the others keep.
- **Home:** `sdk/node/test_sdk_checkpoint_export_fixture.mjs`, `sdk/python/tests/test_sdk_checkpoint_export_fixture.py`, `sdk/go/sdk_checkpoint_export_fixture_test.go`.

## run_observability

Run snapshots, event pages, active tools, traces, child tasks.

### U-RO-01

- **Invariant:** event pages are ordered, gap-free for a single run, and bounded.
- **Pre:** a run log longer than one page.
- **Stimulus:** `session.run_event_page` from cursor 0, then from the returned cursor.
- **Oracle:** concatenation equals the full log; no duplicate seq; page size ≤ bound; oversized payload is projected, not dropped silently (projection marks truncation).
- **Fail:** overlap, gap, or an unbounded payload in one page.
- **Home:** `core/tests/event_protocol_v1.rs`; oversized projection is F06 (`agent_protocol.rs`, `event_protocol.rs`).

### U-RO-02

- **Invariant:** `session.active_tools` is empty when the run is terminal.
- **Pre:** run moved to completed.
- **Stimulus:** read active tools and the snapshot.
- **Oracle:** active set is empty; snapshot state is terminal.
- **Fail:** a tool id remains active after completion.
- **Home:** `core/src/agent_api/runtime_events/tests.rs` `terminal_run_clears_active_tools`.

### U-RO-03

- **Invariant:** child-task state is not visible as a root run, and a root cancel lists children as cancelled.
- **Pre:** one parent run and one child task id.
- **Stimulus:** `session.runs` and `session.subagent_tasks`.
- **Oracle:** child id is only under subagent tasks; after parent cancel, child state is cancelled or absent, not running.
- **Fail:** child id appears as a user-facing root run still running.
- **Home:** `core/src/subagent_task_tracker.rs`, `core/src/child_run.rs` (suite).

### I-RO-01

- **Invariant:** a hermetic tool turn produces a snapshot whose tool names equal the event page tool names.
- **Pre:** fake model calls `read` on a known file.
- **Stimulus:** finish the turn; read snapshot and page.
- **Oracle:** both contain `read` and the same tool-call id; digest of the read output matches.
- **Fail:** snapshot tool name differs from the page.
- **Home:** harness evidence tests, `core/src/harness_evidence.rs`.

### I-RO-L1

- **Invariant:** live event page for an oversized turn is truncated by projection, and the client can still resume from the cursor.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** a turn that emits a large tool result.
- **Oracle:** page carries a truncation marker; following cursor returns the next events; run id stable.
- **Fail:** the connection drops the cursor or returns an empty page while the run is still open.
- **Home:** `core/tests/test_issue_form_live_e2e.rs` (#138). Re-proven 2026-09-20: oversized tool-end page under remapped Flash.

## priority_scheduling

Admission, occupancy, fairness counters. Not required for ordinary coding-loop correctness, but the counters must not lie.

### U-PS-01

- **Invariant:** queue depth matches admitted-minus-finished, and never goes negative.
- **Pre:** scheduler capacity 1.
- **Stimulus:** admit 3 jobs, finish 1, read `task_scheduler_stats` and `queue_stats`.
- **Oracle:** depth is 2; health is saturated, not failed; finished count is 1.
- **Fail:** depth 0 while two jobs are queued, or a panic on the third admit.
- **Home:** `core/src/task_scheduler.rs` tests, `core/src/queue.rs`.

### U-PS-02

- **Invariant:** a lower-priority job does not start while a higher-priority job is waiting and capacity is full.
- **Pre:** capacity 1; one running low-priority job is not the case under test — queue one high and one low while busy.
- **Stimulus:** free the slot.
- **Oracle:** the high-priority job starts next.
- **Fail:** FIFO overrides the documented priority order.
- **Home:** `strict_priority_and_fifo_are_enforced_globally` in `core/src/task_scheduler/tests.rs`. With capacity 1, the start order is urgent, then the two interactive jobs in FIFO order, then foreground, then background.

### U-PS-03

- **Invariant:** lane handler replacement does not drop an in-flight lane receipt.
- **Pre:** one lane job running.
- **Stimulus:** `session.set_lane_handler` to a new handler.
- **Oracle:** the running job completes on the old handler or is cancelled with a receipt; it is not run twice.
- **Fail:** double execution.
- **Home:** `replacing_the_lane_handler_does_not_rerun_the_in_flight_command` in `core/src/session_lane_queue.rs`. The in-flight command keeps the handler captured at submit and finishes once. The next command uses the replacement.

### I-PS-01

- **Invariant:** model generation pool health and middleware health report closed when the pool is shut down.
- **Pre:** session with a generation pool.
- **Stimulus:** close the session; read both health operations.
- **Oracle:** health is closed/not-running; no further generation is accepted.
- **Fail:** health stays ready after close.
- **Home:** SDK fixtures `test_model_generation_pool_health_fixture.*`, `test_model_middleware_health_fixture.*`.

### I-PS-02

- **Invariant:** scheduling counters are not required for a one-shot coding turn to succeed.
- **Pre:** local-code session, fake model, no lane handler.
- **Stimulus:** one send that does not touch the scheduler API.
- **Oracle:** turn completes; absence of scheduler configuration is not an error.
- **Fail:** the default session refuses to run without a lane handler.
- **Home:** baseline agent tests. This is an absence case: do not require hosts to configure PS.
