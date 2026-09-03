# A3S Code Performance Qualification

Status: passed on 2026-08-30 for Code
[`9d17c63`](https://github.com/A3S-Lab/Code/commit/9d17c63803408a8aa32021cd8139256d52e0de0e).

This record is the human-readable companion to the machine-readable release
profiles. It documents what was measured, what was deliberately excluded, and
why the budgets are suitable regression ceilings rather than universal speed
claims.

## Authoritative runs and artifacts

| Evidence                                                        | GitHub Actions run                                                        | Artifact                              | Archive SHA-256                                                    |
| --------------------------------------------------------------- | ------------------------------------------------------------------------- | ------------------------------------- | ------------------------------------------------------------------ |
| Seven release performance profiles                              | [`33304362997`](https://github.com/A3S-Lab/Code/actions/runs/33304362997) | `performance-33304362997-1`           | `0c3e497f546ed3917555036fe972a051f38c89d9062f62e0727a6990665bdf42` |
| MinIO, controlled Chrome/CDP, and local OpenTelemetry Collector | [`32130843684`](https://github.com/A3S-Lab/Code/actions/runs/32130843684) | `hermetic-integrations-32130843684-1` | `7d6a3dea89bea20ffa9be82d0feab14f96a168196bba2f55b8cb2c7c89d2c975` |

GitHub reported both digests for the uploaded ZIP archives. The artifacts are
retained for 30 days; the workflow also runs weekly and whenever a measured
critical path changes, producing a refreshed independently downloadable record.

## Measurement boundary

- Every timed profile used a Rust release build on an x86-64 Linux runner with
  four logical CPUs. Profiles that resolve `/proc/cpuinfo` reported an AMD EPYC
  7763 64-Core Processor.
- Each percentile profile performed three warmups and 20 measured samples,
  except Workspace Retrieval, which performed 20 warmups and 100 measured
  samples.
- Provider and public-network latency was excluded. Workspace hybrid timing
  included authoritative source rereads from the warm operating-system cache.
  Durable semantic recall used a deterministic in-process embedding adapter,
  the reopened file-repository replay view, and SQLite reads from the warm
  operating-system cache.
- Context and memory corpus construction was measured and reported separately,
  but excluded from query latency. Code Intelligence workspace creation was
  excluded; its manifest scan, language-server process start, source read, and
  shutdown were measured explicitly.
- File persistence included JSON serialization, filesystem I/O, and `fsync`.
  The durable-memory profile separately measured source seeding, source reopen,
  each refresh phase, and semantic-query percentiles. It closed and reopened
  every repository/index/session owner but did not restart the operating-system
  process. One-off refresh timings are observations, not percentile claims.
  Flow/State Graph timing was an in-process CPU profile and excluded filesystem
  I/O.
- The convergence client was scripted. Its call, tool, token, and recovery
  counts are deterministic work-amplification gates; its sub-millisecond-scale
  fixture timings are not a model-latency claim.

## Latency results

All values below are milliseconds. A value written as `p50 / p95 / max` comes
directly from the retained JSON report.

| Profile and operation                            | Fixed workload                                               | Observed p50 / p95 / max                                    | Objective                          | Result                |
| ------------------------------------------------ | ------------------------------------------------------------ | ----------------------------------------------------------- | ---------------------------------- | --------------------- |
| Workspace Retrieval exact cosine                 | 25,000 records, 384 dimensions, Top 20                       | 9.699 / 9.842 / 9.982                                       | p95 <= 30                          | Pass                  |
| Workspace Retrieval hybrid, RRF-only             | Same corpus; current-source reads included                   | 33.140 / 38.412 / 40.641                                    | p95 <= 100                         | Pass                  |
| Workspace Retrieval hybrid, deterministic rerank | Same corpus; at most 100 rerank candidates                   | 33.161 / 37.347 / 38.682                                    | p95 <= 100 and added p95 <= 10     | Pass; added p95 0.000 |
| Flow projection                                  | 1,000 steps and 2,002 Flow events                            | 173.281 / 173.753 / 174.060                                 | p95 <= 2,000                       | Pass                  |
| State Graph replay                               | 11,008 graph records                                         | 163.627 / 164.376 / 166.547                                 | p95 <= 2,000                       | Pass                  |
| Code Intelligence cold document symbols          | 5,000-file workspace; process start and source read included | 753.704 single observation                                  | <= 5,000                           | Pass                  |
| Code Intelligence warm document symbols          | 20 source-reading samples                                    | 0.732 / 0.746 / 0.753                                       | p95 <= 250                         | Pass                  |
| Code Intelligence warm workspace symbols         | 20 source-reading samples                                    | 0.730 / 0.756 / 0.769                                       | p95 <= 250                         | Pass                  |
| Context assembly                                 | 25,000 inputs, 20,000 unique items, 10 providers             | 176.929 / 185.148 / 186.522                                 | p95 <= 500                         | Pass                  |
| In-memory recall through `AgentMemory`           | 2,500 memories, Top 20                                       | 0.142 / 0.153 / 0.161                                       | p95 <= 250                         | Pass                  |
| Durable semantic recall through SQLite           | 10,000 active nodes, 384 dimensions, Top 8                   | 162.017 / 164.508 / 165.871                                 | p95 <= 1,000                       | Pass                  |
| Memory session save / load                       | Approx. 1.25 MiB snapshot                                    | 0.093 / 0.107 / 0.383 save; 0.078 / 0.083 / 0.091 load      | each p95 <= 250                    | Pass                  |
| File session save / load                         | 1,272,756-byte persisted snapshot; `fsync` included          | 2.406 / 2.598 / 5.591 save; 0.964 / 1.041 / 1.058 load      | save p95 <= 1,000; load p95 <= 500 | Pass                  |

The file-save result is intentionally reported rather than normalized away.
Hosted-runner `fsync` timing varies substantially across otherwise successful
runs, so the contract uses a user-visible ceiling with headroom instead of
publishing the fastest sample as an SLA.

## Deterministic work and resource results

| Profile              | Evidence                                                                                                                                                                                                                                                                                        |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent convergence    | 4/4 cases passed with 9 scripted LLM calls, 8 tool attempts, 6 executed tool calls, and 235 accounted tokens. Duplicate calls stopped after the bounded guard path; checkpoint resume preserved accounting.                                                                                     |
| Workspace Retrieval  | Document request amplification was exactly 1.0x for 25,000 admitted inputs; non-text inputs were zero. Published vector bytes were 40,177,548 against a 128 MiB vector ceiling. Maximum rerank scratch was 75,346 bytes against 4 MiB, with zero fallbacks and at most 50 evaluated candidates. |
| Code Intelligence    | The manifest admitted 5,001 files in 55.522 ms. Cold start created one process; shutdown sent both protocol messages, observed one exit, and completed in 0.581 ms. Active and retained RSS deltas were both 8,089,600 bytes against 512 MiB and 256 MiB ceilings.                              |
| Context and memory   | Context output was bounded to 64 items, 2,048 selected tokens, and 10,878 rendered bytes. Active and retained RSS deltas were 53,432,320 and 49,991,680 bytes against 512 MiB and 256 MiB ceilings. Recall ranked the independently marked target first.                                        |
| Durable memory       | Initial publication read and embedded 10,000 nodes in 157 provider batches (569 ms). A stable tick did zero snapshot/provider/publication work (4 ms). One-node drift reused 9,999 vectors and made one provider request (517 ms); index-only drift reused all 10,000 vectors with zero provider calls (438 ms). Checkpoint recovery reopened both durable backends, read one 10,000-node snapshot with zero provider/publication work (118 ms), and the next stable tick again did zero work (4 ms). Source seed/reopen took 171.585/272.032 ms. The 915-byte secret-free checkpoint was synchronized before close. Total durable disk was 30,478,075 bytes against 193 MiB; active/retained RSS deltas were 136,953,856/136,921,088 bytes against 768/384 MiB. |
| Flow and State Graph | Replay preserved 1,001 objects and 1,000 relations. Serialized events occupied 9,657,128 bytes against a 64 MiB ceiling.                                                                                                                                                                        |
| Persistence          | Twenty-three generations overwrote one logical session without file accumulation. Memory and file stores both returned one session, preserved snapshot identity and byte shape, and left zero files and zero bytes after delete.                                                                |

The change-scoped 2026-09-02 workspace-retrieval schema-v4 run additionally
qualified the A3S Vec differential shadow: both 25,000-record hybrid arms
matched 120/120 Memory queries with zero mismatch/failure, reported 54,500,008
Vec accounted bytes, and released both engines on close. Exact, RRF-only, and
deterministic hybrid p95 were 6.7343, 50.7850, and 49.7348 ms. This is not an
RSS, disk, or Vec-serving-authority claim; see the
[migration contract](WORKSPACE_RETRIEVAL_VEC_MIGRATION.md).

The 2026-09-03 change-scoped rerun exercised the typed Vec-primary preview on
the same Windows x86-64 host. The default Memory and explicit Vec paths both
passed the existing 30/100/100 ms p95 budgets and the 120-query differential
gate:

| Serving engine | Exact p95 | Hybrid RRF p95 | Deterministic p95 | Differential result |
| --- | ---: | ---: | ---: | --- |
| `a3s_memory` (default) | 7.5379 ms | 49.0593 ms | 48.7440 ms | 120/120, zero failures |
| `a3s_vec` (typed preview) | 7.4195 ms | 47.4571 ms | 48.7821 ms | 120/120, zero failures |

The Vec-primary run retained 25,000 records per hybrid arm and reported
54,500,008 logical Vec-accounted bytes per arm; close released all records and
bytes. These are same-host directional measurements, not RSS, disk, recovery,
or macOS 12 Intel qualification.

## Hermetic integration results

| Boundary        | Controlled evidence                                                                                                                                                                                                                                                     | Result |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| S3 workspace    | A pinned local MinIO service completed write, read, list, edit, patch, and cleanup through `S3WorkspaceBackend`.                                                                                                                                                        | Pass   |
| Headless search | Workflow-managed Chrome used the production CDP path and Google parser against a controlled HTTPS fixture. The request log contains `www.google.com` search, completion, and async endpoints; the public search-engine network was excluded and browser cleanup passed. | Pass   |
| OpenTelemetry   | A pinned local Collector received service `a3s-code-hermetic` and span `a3s.telemetry.qualification`. Initialization took 0.651 ms and shutdown took 2.158 ms against a 10,000 ms deadline.                                                                             | Pass   |

These tests establish Code-owned transport, parsing, lifecycle, and cleanup.
They do not claim that a public search engine, arbitrary S3 deployment, or
remote Collector will always be available or have the same latency.

## Why these budgets are reasonable

The objectives correspond to user-visible boundaries and include substantial
hosted-runner headroom:

- interactive local retrieval should stay below 100 ms at the locked corpus;
- local durable semantic recall should stay below 1 second at 10,000 active
  nodes and 384 dimensions, while stable refresh ticks must perform zero
  snapshot, provider, and publication work;
- assembling a bounded model context should stay below 500 ms;
- a 1,000-step graph projection or replay should stay below 2 seconds;
- a cold controlled language-server request and clean shutdown each have a
  5-second ceiling;
- an atomic, synchronized approximately 1.25 MiB file snapshot should save
  within 1 second;
- memory ceilings prevent passing latency by retaining unbounded state.

Changing a workload, inclusion rule, or budget requires changing the emitted
profile and this record together. A job timeout remains only a hung-process
guard and must not be presented as product latency.

## Reproduce and inspect

Dispatch the remote workflow so native compilation and release execution stay
off the developer workstation:

```bash
gh workflow run performance.yml --repo A3S-Lab/Code
gh run watch <run-id> --repo A3S-Lab/Code --exit-status
gh run download <run-id> --repo A3S-Lab/Code
```

The current workflow requires exactly eight JSON files (including the
Vec-primary workspace-retrieval report) and rejects any report
whose top-level `passed` value is not `true`. The authoritative run above
contains all eight, including the Vec-primary workspace report and `DM-QUAL1`.
The capability ledger explains how
these profiles combine with deterministic correctness, SDK runtime, and
external qualification evidence.
