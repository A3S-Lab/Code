# Advanced capability cases

These ids are not in the default `local-code` product. Each case names the
Cargo feature or host injection it requires. A default-feature build must
fail closed (absence cases), not partially start the feature.

## code_intelligence (CI)

Host language service. Feature: host provides the service. Tools:
`code_symbols`, `code_navigation`, `code_diagnostics`.

### U-CI-01

- **Invariant:** with no language service, the three tools are not registered.
- **Pre:** `workspace_services.code_intelligence()` is `None`.
- **Stimulus:** `register_builtins` and list definitions.
- **Oracle:** the three names absent.
- **Fail:** a tool is listed and errors only when called.
- **Home:** `register_builtins_hides_disabled_capabilities` in `core/src/tools/builtin/mod.rs`. With `code_intelligence: false`, `code_symbols`, `code_navigation`, and `code_diagnostics` are absent from the registry.

### U-CI-02

- **Invariant:** symbols and navigation cite saved-file positions only. A dirty unsaved buffer the host did not save is not a result source.
- **Pre:** fixture service; on-disk file differs from a fake unsaved buffer.
- **Stimulus:** `code_symbols` / `code_navigation`.
- **Oracle:** positions match the saved file; no position points past EOF.
- **Fail:** a result only explainable by the unsaved buffer.
- **Home:** `core/src/tools/builtin/code_intelligence/tests.rs`.

### U-CI-03

- **Invariant:** diagnostics from a failed language service are a typed tool error, not an empty success.
- **Pre:** service returns a transport error.
- **Stimulus:** `code_diagnostics`.
- **Oracle:** typed error; no "no problems" payload.
- **Fail:** empty diagnostic list on transport failure.
- **Home:** `missing_and_failing_providers_return_typed_unavailable_errors` in `core/src/tools/builtin/code_intelligence/tests.rs`. A failing language service returns `CODE_INTELLIGENCE_UNAVAILABLE` with `success == false`, not an empty diagnostic list.

### I-CI-01

- **Invariant:** a governed call still applies path policy. Navigation to a file outside the root is denied.
- **Pre:** service would return an outside path.
- **Stimulus:** governed navigation.
- **Oracle:** outside path stripped or the call denied.
- **Fail:** outside path returned to the model.
- **Home:** `core/src/tools/builtin/code_intelligence/tests.rs` `registration_is_capability_gated_and_tools_are_structured_query_reads`. Registry execution of `../outside.rs` returns `Workspace boundary check failed` and does not call the provider.

## cognitive_packages (CP)

Host supplies cited knowledge generations.

### U-CP-01

- **Invariant:** with no host binding, `session.cognitive_package_binding` fail-closes and recall does not invent a package.
- **Pre:** default session, no host.
- **Stimulus:** read current binding and ask context assembly for package text.
- **Oracle:** binding absent; prompt contains no package body.
- **Fail:** an empty package id is treated as bound.
- **Home:** `core/src/agent_api/tests/cognitive.rs` `unbound_session_does_not_invent_a_cognitive_package`. Both binding accessors are `None`, so no package body is installed.

### U-CP-02

- **Invariant:** binding generation G is what retrieval cites. A newer host generation is not visible until bind.
- **Pre:** host has G and G+1; session bound to G.
- **Stimulus:** read binding and the injected context.
- **Oracle:** generation id G; bytes match G's digest.
- **Fail:** G+1 text in the prompt.
- **Home:** `cognitive_provider_failure_and_generation_drift_fail_closed_without_fallback` in `core/src/agent_api/tests/cognitive.rs`. `DriftGeneration` increments the provider generation; the turn fails closed, the model is not called, and no recalled-memory event is recorded.

### I-CP-01

- **Invariant:** replacing the binding drops G from the next turn's prompt and does not leave G's bytes in working memory.
- **Pre:** bound to G, one turn assembled, then bind G+1.
- **Stimulus:** assemble again.
- **Oracle:** G digest absent; G+1 digest present.
- **Fail:** both bodies present.
- **Home:** Product bind keeps one generation. A different generation returns `RunCognitiveBindingError::Conflict` and does not replace G. Proven by `soak_alternating_cognitive_binds_keep_one_generation` in `core/src/run/tests/run_cognitive_binding_soak.rs` (`#[ignore]`). Host CAR stays external.

Host CAR qualification (CAR-01…05) stays external. These cases do not replace it.

## use_runtime_tasks (UR)

### U-UR-01

- **Invariant:** without a host projection, `use_runtime_task` is not registered.
- **Pre:** no Use host.
- **Stimulus:** list tools.
- **Oracle:** name absent.
- **Fail:** the tool is listed and fails only at call time with a network error.
- **Home:** `core/src/use_runtime_tasks.rs`.

### U-UR-02

- **Invariant:** a projected task runs as a governed tool: policy deny wins over the host, and the result is the host payload's digest.
- **Pre:** host returns a fixed payload; deny rule on the task name.
- **Stimulus:** call, then allow and call.
- **Oracle:** deny does not call the host; allow returns the digest.
- **Fail:** deny still calls the host.
- **Home:** `soak_governed_use_calls_skip_denied_host_dispatch` in `core/src/use_runtime_tasks.rs` (`#[ignore]`). Forty governed calls, every fourth denied, host dispatcher count 30, and the deny text is `Permission denied`. Allow returns exit 0. The soak does not assert a host payload digest; the governed result is the tool output.

### I-UR-01

- **Invariant:** task ids in the projection match `session.tool_definitions` and no extra host task appears.
- **Pre:** host projects two task ids.
- **Stimulus:** list definitions.
- **Oracle:** exactly those two, namespaced so they cannot shadow `read`.
- **Fail:** shadowing `read`.
- **Home:** `core/tests/capability_projection.rs`, `capability_set.rs`.

## programmable_workflows (PW)

Feature: `advanced-harness`. Operations: `session.parallel`,
`session.parallel_resumable`, `session.workflow_step`.

### U-PW-01

- **Invariant:** default features do not expose `session.parallel`. Calling it is unknown, not a silent no-op.
- **Pre:** `local-code` / default build.
- **Stimulus:** invoke parallel.
- **Oracle:** typed unavailable.
- **Fail:** parallel runs without the feature.
- **Home:** Layer A1/A3.

### U-PW-02

- **Invariant:** a workflow step is idempotent for the same step id. A second delivery does not re-run the side effect.
- **Pre:** one step that writes a counter file through a tool.
- **Stimulus:** deliver the same step twice.
- **Oracle:** counter is 1; second receipt is duplicate.
- **Fail:** counter is 2.
- **Home:** `core/tests/dynamic_workflow_control_recovery.rs`.

### U-PW-03

- **Invariant:** resume after a missing checkpoint fail-closes.
- **Pre:** `parallel_resumable` id that was never saved.
- **Stimulus:** resume.
- **Oracle:** typed not-found; no new side effect.
- **Fail:** resume starts the workflow from step 0 and repeats completed steps.
- **Home:** `dynamic_workflow_control_recovery.rs`.

### I-PW-01

- **Invariant:** feature-on hermetic workflow runs two steps in order, and a failed step stops the graph.
- **Pre:** `advanced-harness` enabled; step 2 depends on step 1; step 1 fails.
- **Stimulus:** run.
- **Oracle:** step 2 not started; terminal state failed.
- **Fail:** step 2 runs.
- **Home:** `core/src/dynamic_workflow.rs` tests.

### I-PW-L1

- **Invariant:** live workflow facade does not register `parallel_task` and completes or fails closed under the Flash pin.
- **Pre:** feature on, Layer C pin. Ignored.
- **Stimulus:** `test_workflow_facade_real_llm`, `test_extensibility_real_llm`.
- **Oracle:** no `parallel_task` event; terminal state set.
- **Fail:** pass because the model described a workflow without a step receipt.
- **Home:** those live tests. Re-proven 2026-09-20 under remapped Flash: extensibility 4/4 (`advanced-harness`), orchestration 7/7. Same day: long_horizon 1/1, ultracode 3/3, workflow_facade 4/4, cluster_features 8/8.

## state_graph (SG)

Feature: `advanced-harness`.

### U-SG-01

- **Invariant:** events are hash-linked. Tampering one event breaks restore.
- **Pre:** graph with 3 events.
- **Stimulus:** flip a byte in event 2; restore.
- **Oracle:** typed integrity error; state not applied.
- **Fail:** restore succeeds.
- **Home:** `core/tests/test_state_graph_integration.rs`.

### U-SG-02

- **Invariant:** fork does not mutate the parent. Diff is deterministic for the same pair of nodes.
- **Pre:** parent graph.
- **Stimulus:** fork, patch the fork, diff twice.
- **Oracle:** parent head unchanged; both diffs equal.
- **Fail:** parent head moves, or diffs differ.
- **Home:** `test_state_graph_integration.rs`; Go `sdk/go/state_graph_test.go`.

### I-SG-01

- **Invariant:** propose_patch rejected by policy does not append an event.
- **Pre:** policy deny.
- **Stimulus:** propose_patch.
- **Oracle:** head unchanged; no new hash.
- **Fail:** a rejected patch is still in the log.
- **Home:** `core/src/state_graph/tests.rs` `stale_patch_is_rejected_atomically_without_partial_mutation` (`--features state-graph`). A rejected patch does not mutate objects. The log records `PatchProposed` and `PatchRejected`; that rejection record is the kernel, not a dropped write.

## agent_release_contract (AR)

Always compiled. Host owns publication.

### U-AR-01

- **Invariant:** `release.verify` rejects a manifest with a bad digest, a future schema, or a missing asset.
- **Pre:** three broken manifests.
- **Stimulus:** verify each.
- **Oracle:** three typed errors; none admitted.
- **Fail:** any broken manifest admitted.
- **Home:** `core/tests/agent_release_manifest.rs`.

### U-AR-02

- **Invariant:** `release.admit` of a verified manifest is idempotent for the same publication id.
- **Pre:** one valid manifest.
- **Stimulus:** admit twice.
- **Oracle:** one binding; second call is the same receipt.
- **Fail:** two active bindings.
- **Home:** `agent_release_manifest.rs`.

### I-AR-01

- **Invariant:** Node release-manifest script and Rust verify agree on the fixture.
- **Pre:** shared fixture.
- **Stimulus:** both verifiers.
- **Oracle:** same accept/reject.
- **Fail:** Node accepts what Rust rejects.
- **Home:** `sdk/node/scripts/release-manifest.test.mjs`, `agent_release_manifest.rs`.

## agent_protocol (AP)

Always compiled. Start, cancel, recover, events.

### U-AP-01

- **Invariant:** start is versioned. A client speaking a future major version is rejected.
- **Pre:** request with major = current+1.
- **Stimulus:** `agent_protocol.start`.
- **Oracle:** typed version error; no session created.
- **Fail:** session created.
- **Home:** `core/tests/agent_protocol_v1.rs`.

### U-AP-02

- **Invariant:** cancel is idempotent. Recover after cancel does not resume the cancelled run.
- **Pre:** started run, then cancel.
- **Stimulus:** cancel again, then recover.
- **Oracle:** second cancel duplicate; recover yields a terminal cancelled run, not running.
- **Fail:** recover restarts the run.
- **Home:** `agent_protocol_v1.rs`, `core/tests/agent_protocol_harness.rs`.

### U-AP-03

- **Invariant:** event pages match the in-process observability page for the same run (same seq, same truncation).
- **Pre:** run with an oversized tool result.
- **Stimulus:** read protocol events and `session.run_event_page`.
- **Oracle:** same seq bounds and truncation marker.
- **Fail:** protocol page omits the truncation marker.
- **Home:** `protocol_page_matches_observability_seq_and_marks_truncation` in `core/src/agent_protocol.rs`. The retained `event_page` keeps the full `TRUNC-SRC-91` output. The protocol projection of that same page matches sequence bounds, and the protocol `output` ends with the truncation mark.

### I-AP-01

- **Invariant:** harness start executes a tool and the protocol change set equals the isolated digest.
- **Pre:** hermetic start that calls `write`.
- **Stimulus:** finish; read protocol events.
- **Oracle:** change-set digest equals EI digest; source matches after promote.
- **Fail:** protocol reports a change the source does not have.
- **Home:** `core/tests/agent_protocol_harness.rs`.

### I-AP-L1

- **Invariant:** live protocol start, tool, cancel, and event replay under the Flash pin.
- **Pre:** Layer C pin. Ignored.
- **Stimulus:** `test_agent_protocol_live_e2e`.
- **Oracle:** tool event present; cancel stops further tools; replay cursor is stable.
- **Fail:** replay returns a different tool name.
- **Home:** `live_harness_start_projects_events_and_replays`, `live_harness_tool_mutation_exports_change_set`, and `live_harness_cancel_stops_in_flight_tool` in `core/tests/test_agent_protocol_live_e2e.rs`. Re-proven 2026-09-20 with `boyue/bailian/deepseek-v4.1-flash`: start/replay, write change-set export, and Cancel without a late leak file.

## evaluation_substrate (EV)

Feature: `advanced-harness`.

### U-EV-01

- **Invariant:** default build does not export evaluation operations as available.
- **Pre:** default features.
- **Stimulus:** call `evaluation.result_store` or the feature-gated entry.
- **Oracle:** unavailable, not a writable store in the default data dir.
- **Fail:** default build writes evaluation records.
- **Home:** Layer A1 tree check.

### U-EV-02

- **Invariant:** a dispatch claim is single-winner across restart. Replaying the same claim does not double-award.
- **Pre:** ledger with one claim.
- **Stimulus:** award, restart process, replay claim.
- **Oracle:** one award; second is duplicate.
- **Fail:** two awards.
- **Home:** `core/tests/evaluation_substrate.rs`, `evaluation_protocol_v1.rs`.

### U-EV-03

- **Invariant:** result store rejects a record over the cap and does not evict a newer record to store an older one out of order.
- **Pre:** store at cap.
- **Stimulus:** insert one more, then read.
- **Oracle:** documented eviction of the oldest, or typed full; never a torn record.
- **Fail:** unreadable store.
- **Home:** `evaluation_qualification.rs`.

### I-EV-01

- **Invariant:** auxiliary lifecycle is isolated from the coding session. Closing the session does not delete an evaluation result the host still owns, and closing the auxiliary does not delete the session.
- **Pre:** both open.
- **Stimulus:** close one, then the other.
- **Oracle:** the survivor's digest remains.
- **Fail:** cross-delete.
- **Home:** `session_close_leaves_the_host_evaluation_digest` in `core/src/evaluation/result.rs` (`--features evaluation`). Host-owned result digest `record_digest` survives `session.close()`, and `SESSION-SURVIVOR-91` survives dropping the result store.

SDK: `sdk/python/tests/test_evaluation_protocol.py`, `sdk/go/evaluation_protocol_v1_test.go`.

## moli_runtime (MO)

Feature: `headless-search`.

### U-MO-01

- **Invariant:** `moli.ensure` takes a cross-process lock. A second ensure waits or fails typed, and does not unpack over the first.
- **Pre:** two callers, empty cache.
- **Stimulus:** both ensure.
- **Oracle:** one unpack; lock released; runtime digest matches the packaged digest.
- **Fail:** two overlapping unpacks, or a lock left held after error.
- **Home:** `core/src/moli_runtime.rs` (`try_lock_exclusive`).

### U-MO-02

- **Invariant:** a digest mismatch refuses to start the runtime.
- **Pre:** cache file with flipped bytes.
- **Stimulus:** `moli.packaged` / ensure.
- **Oracle:** typed integrity error; process not spawned.
- **Fail:** mismatched binary starts.
- **Home:** `soak_concurrent_moli_install_publishes_one_binary` in `core/src/moli_runtime/tests/install_soak.rs` (`#[ignore]`, `--features headless-search`). A flipped digest returns `SHA-256 mismatch` and the published bytes stay the first install.

### I-MO-01

- **Invariant:** headless web search uses the ensured runtime and still obeys safe-HTTP denylist.
- **Pre:** feature on; fixture that would navigate to a metadata IP.
- **Stimulus:** `test_web_search_headless` negative case.
- **Oracle:** navigation denied; no fetch of the metadata IP.
- **Fail:** headless bypasses `safe_http`.
- **Home:** `core/tests/test_web_search_headless.rs`. Add the denylist case if the file only checks that a page title parsed.

## s3_workspace (S3)

Feature: `s3`.

### U-S3-01

- **Invariant:** default build has no S3 backend. Selecting `workspace_backend:s3` without the feature is a typed error.
- **Pre:** default features.
- **Stimulus:** open an S3 workspace config.
- **Oracle:** typed unavailable; no network call.
- **Fail:** the client attempts a connection.
- **Home:** feature cfg on `core/src/workspace/s3.rs`.

### U-S3-02

- **Invariant:** reads and writes are bounded. A key that escapes the prefix is denied.
- **Pre:** hermetic S3 (`test_s3_backend`) with prefix `p/`.
- **Stimulus:** get `../secret` and put an object over the size cap.
- **Oracle:** both denied; no object written outside `p/`.
- **Fail:** object at the raw key.
- **Home:** `core/tests/test_s3_backend.rs`.

### I-S3-01

- **Invariant:** search on an S3 workspace returns only keys under the prefix, and credentials do not appear in errors.
- **Pre:** hermetic bucket; forced 403.
- **Stimulus:** search and a denied get.
- **Oracle:** error text has no access key; hits are prefix-local.
- **Fail:** access key in the error string.
- **Home:** `denied_get_error_omits_credentials` in `core/src/workspace/s3/tests.rs` (`--features s3`). A local listener returns a mixed `ListObjectsV2` page and a 403 `GetObject`. Glob keeps `visible.txt` and drops `outside/secret.txt`. The classified get error contains `Failed to read S3 object` and neither the access key nor the secret. Live `A3S_S3_TEST_ENDPOINT` remains the ignored round-trip in `core/tests/test_s3_backend.rs`.

## filesystem_agent_server (SV)

Feature: `serve`.

### U-SV-01

- **Invariant:** an agent directory with an invalid schedule fails ready and does not start the ticker.
- **Pre:** agent dir with a bad cron.
- **Stimulus:** `agent.serve_agent_dir`.
- **Oracle:** typed validation error; status is not ready; no tick fired.
- **Fail:** server listens and skips the bad schedule.
- **Home:** serve unit tests; `core/tests/test_serve_agent_dir_real_llm.rs` is live, not this hermetic case.

### U-SV-02

- **Invariant:** `serve.stop` is idempotent and joins in-flight work before returning.
- **Pre:** one request in flight.
- **Stimulus:** stop twice.
- **Oracle:** in-flight request finishes or is cancelled; port released; second stop succeeds.
- **Fail:** port still bound, or second stop panics.
- **Home:** serve tests. SDK `sdk/node/test_serve.mjs`, `sdk/python/tests/test_serve.py`.

### I-SV-01

- **Invariant:** tools declared in the agent dir are the tools registered, and a tool not in the dir is absent.
- **Pre:** agent dir listing `read` only.
- **Stimulus:** serve and list definitions.
- **Oracle:** `read` present; `bash` absent unless the dir allows exec.
- **Fail:** the full builtin set is exposed.
- **Home:** `install_script_collision_fails_startup_without_shadowing` in `core/src/serve/tools.rs` (`--features serve`). An agent-dir script cannot replace builtin `bash`. Builtins stay registered. Hiding `bash` because the dir lists only `read` would contradict that kernel.

### I-SV-L1

- **Invariant:** live serve answers one turn from the agent dir and stops cleanly.
- **Pre:** feature `serve`, Layer C pin. Ignored.
- **Stimulus:** `test_serve_agent_dir_real_llm`.
- **Oracle:** one terminal run; process exited; no leftover listener.
- **Fail:** listener remains.
- **Home:** `core/tests/test_serve_agent_dir_real_llm.rs`.

## opentelemetry (OT)

Feature: `telemetry`.

### U-OT-01

- **Invariant:** default build does not open an OTLP socket.
- **Pre:** default features; env `OTEL_EXPORTER_OTLP_ENDPOINT` set.
- **Stimulus:** create a session and finish a turn.
- **Oracle:** no connection to that endpoint.
- **Fail:** a connection attempt.
- **Home:** `default_build_does_not_open_an_otlp_socket` in `core/src/telemetry.rs`. Default features only. Sets `OTEL_EXPORTER_OTLP_ENDPOINT` to a live loopback listener, finishes a `read` turn, and accepts no connection. The OTLP module is `#[cfg(feature = "telemetry")]`.

### U-OT-02

- **Invariant:** exported spans redact secrets and tool argument values that the sanitizer marks.
- **Pre:** feature on; hermetic collector; tool argument contains a secret.
- **Stimulus:** `telemetry.init` and one tool turn.
- **Oracle:** span received; secret string absent; trace id matches `session.trace_events`.
- **Fail:** secret in the collector payload.
- **Home:** `exported_span_omits_tool_argument_secrets` in `core/tests/test_telemetry_span_redaction.rs` (`--features telemetry`). A loopback HTTP collector receives the OTLP protobuf. The tool argument is the file `OTLP-SECRET-91.txt`. The payload contains `a3s.tool.execute` and `read`, and does not contain that secret. `TraceEvent` has no trace id; the in-memory trace is matched on the tool name. Default export stays gRPC. The HTTP transport exists so the test can read the same `ExportTraceServiceRequest` bytes.

### I-OT-01

- **Invariant:** collector down does not fail the coding turn, and the in-memory trace still records the tool.
- **Pre:** exporter endpoint refusing connections.
- **Stimulus:** one hermetic turn.
- **Oracle:** turn success; trace event present; no unbounded retry loop in the turn's latency (retries capped).
- **Fail:** the turn errors because telemetry failed.
- **Home:** `core/tests/test_telemetry_collector_down.rs` `collector_down_does_not_fail_the_tool_turn` (`#[ignore]`, `--features telemetry`). Passed in 2.07s. Init against a refused port returns `Ok`. Host `read` exits 0 and the in-memory trace records `read`. Export wait is capped at 1s. Shutdown from a Tokio worker uses `block_in_place`, so `futures_executor::block_on` cannot stall the batch task. A canceled export is logged and does not change the tool result.

## research contracts (RX)

Not an `sdk_capabilities()` id. Cargo feature `research`. Code owns digests,
lifecycle, and fail-closed wire validation. Use owns packages. Hosts own
review decisions. See [RESEARCH_CONTRACTS.md](../RESEARCH_CONTRACTS.md).

### U-RX-01

- **Invariant:** a future schema or unknown wire kind is rejected.
- **Pre:** envelope with unsupported schema, and one with an unknown kind.
- **Stimulus:** decode.
- **Oracle:** `UnsupportedSchema` / `UnknownKind`; no partial value returned.
- **Fail:** decode succeeds by skipping fields.
- **Home:** `core/tests/research_protocol_v1.rs`.

### U-RX-02

- **Invariant:** claim status digests are mutually exclusive. A `supported` claim cannot also carry a conflict digest, and a citation stores digests, not source plaintext.
- **Pre:** claim with both support and conflict digests; citation whose payload includes the source text.
- **Stimulus:** validate.
- **Oracle:** both rejected.
- **Fail:** either value validates.
- **Home:** `supported_claim_cannot_also_carry_a_conflict_digest` in `core/src/research/claim.rs` and `citation_rejects_source_plaintext` in `core/src/research/citation.rs` (`--features research`). A supported claim that also carries a conflict digest fails validation. A citation wire object with `sourceText` is rejected, and the stored citation does not contain that plaintext.

### U-RX-03

- **Invariant:** the evidence graph rejects an orphan citation and a support digest that no claim links.
- **Pre:** graph with a citation whose claim id is absent.
- **Stimulus:** validate.
- **Oracle:** typed rejection; completeness is not `complete`.
- **Fail:** graph validates.
- **Home:** `core/src/research/evidence_graph.rs`; `core/tests/research_review_qualification.rs`.

### U-RX-04

- **Invariant:** the research module is absent unless Cargo feature `research` is on.
- **Pre:** check without that feature.
- **Stimulus:** resolve `a3s_code_core::research`.
- **Oracle:** the module is not in the build. `lib.rs` gates it with `#[cfg(feature = "research")]`.
- **Fail:** the module is compiled into the default feature set.
- **Home:** `research_module_is_absent_from_the_default_build` in `core/tests/research_feature_absence.rs`. Default features only (`#![cfg(not(feature = "research"))]`). Asserts `cfg!(feature = "research")` is false and `CARGO_FEATURE_RESEARCH` is unset. `lib.rs` keeps `pub mod research` behind that feature.

### I-RX-01

- **Invariant:** a provenance receipt round-trips through the wire envelope and stays equal after reopen.
- **Pre:** one valid receipt.
- **Stimulus:** encode, decode, persist, read back.
- **Oracle:** decoded value equals the original; digest unchanged.
- **Fail:** a field dropped on the wire.
- **Home:** `provenance_receipt_round_trips_after_reopen` in `core/tests/research_protocol_v1.rs` (`--features research`). `ResearchWireEnvelopeV1::from_provenance_receipt` is written, read back, and `receipt_digest` is unchanged. Go: `TestProvenanceReceiptEnvelopeSurvivesReopen` in `sdk/go/research_protocol_v1_test.go`. The Go projection keeps the payload opaque; the test persists the envelope and checks the digest bytes survive decode. Python: `sdk/python/tests/test_research_protocol.py`.

### I-RX-02

- **Invariant:** attaching a review finding does not approve the run. Code records the finding and leaves the run status unchanged.
- **Pre:** run not approved; one finding.
- **Stimulus:** attach the finding.
- **Oracle:** run status unchanged; finding digest stored.
- **Fail:** the run becomes approved because a finding exists.
- **Home:** `attaching_a_finding_does_not_approve_the_run` in `core/tests/research_review_qualification.rs` (`--features research`). The research run stays `Admitted` after `ResearchReviewBatchV1::new_for_run`, and the finding digest is stored on the batch.
