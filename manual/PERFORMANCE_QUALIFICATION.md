# A3S Code Performance Qualification

Status: passed on 2026-08-18 for Code
[`a9bfac2`](https://github.com/A3S-Lab/Code/commit/a9bfac2ad3c726252aa61314bcd098c40e07b43a).

This record is the human-readable companion to the machine-readable release
profiles. It documents what was measured, what was deliberately excluded, and
why the budgets are suitable regression ceilings rather than universal speed
claims.

## Authoritative runs and artifacts

| Evidence                                                        | GitHub Actions run                                                        | Artifact                              | Archive SHA-256                                                    |
| --------------------------------------------------------------- | ------------------------------------------------------------------------- | ------------------------------------- | ------------------------------------------------------------------ |
| Six release performance profiles                                | [`32130843443`](https://github.com/A3S-Lab/Code/actions/runs/32130843443) | `performance-32130843443-1`           | `c13cbf90051973c3037ee68394c23b364064c0efe2a07cbcd54dfdf29bf73bea` |
| MinIO, controlled Chrome/CDP, and local OpenTelemetry Collector | [`32130843684`](https://github.com/A3S-Lab/Code/actions/runs/32130843684) | `hermetic-integrations-32130843684-1` | `7d6a3dea89bea20ffa9be82d0feab14f96a168196bba2f55b8cb2c7c89d2c975` |

GitHub reported both digests for the uploaded ZIP archives. The artifacts are
retained for 30 days; the workflow also runs weekly and whenever a measured
critical path changes, producing a refreshed independently downloadable record.

## Measurement boundary

- Every timed profile used a Rust release build on an x86-64 Linux runner with
  four logical CPUs. Profiles that resolve `/proc/cpuinfo` reported an Intel
  Xeon 6973P-C.
- Each percentile profile performed three warmups and 20 measured samples,
  except Workspace Retrieval, which performed 20 warmups and 100 measured
  samples.
- Provider and public-network latency was excluded. Workspace hybrid timing
  included authoritative source rereads from the warm operating-system cache.
- Context and memory corpus construction was measured and reported separately,
  but excluded from query latency. Code Intelligence workspace creation was
  excluded; its manifest scan, language-server process start, source read, and
  shutdown were measured explicitly.
- File persistence included JSON serialization, filesystem I/O, and `fsync`.
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
| Workspace Retrieval exact cosine                 | 25,000 records, 384 dimensions, Top 20                       | 7.424 / 15.590 / 15.957                                     | p95 <= 30                          | Pass                  |
| Workspace Retrieval hybrid, RRF-only             | Same corpus; current-source reads included                   | 35.816 / 38.119 / 39.485                                    | p95 <= 100                         | Pass                  |
| Workspace Retrieval hybrid, deterministic rerank | Same corpus; at most 100 rerank candidates                   | 36.345 / 38.506 / 42.755                                    | p95 <= 100 and added p95 <= 10     | Pass; added p95 0.387 |
| Flow projection                                  | 1,000 steps and 2,002 Flow events                            | 127.567 / 130.067 / 132.914                                 | p95 <= 2,000                       | Pass                  |
| State Graph replay                               | 11,008 graph records                                         | 123.675 / 125.526 / 125.615                                 | p95 <= 2,000                       | Pass                  |
| Code Intelligence cold document symbols          | 5,000-file workspace; process start and source read included | 754.397 single observation                                  | <= 5,000                           | Pass                  |
| Code Intelligence warm document symbols          | 20 source-reading samples                                    | 0.418 / 0.447 / 0.448                                       | p95 <= 250                         | Pass                  |
| Code Intelligence warm workspace symbols         | 20 source-reading samples                                    | 0.506 / 0.519 / 0.542                                       | p95 <= 250                         | Pass                  |
| Context assembly                                 | 25,000 inputs, 20,000 unique items, 10 providers             | 132.301 / 136.740 / 139.442                                 | p95 <= 500                         | Pass                  |
| In-memory recall through `AgentMemory`           | 2,500 memories, Top 20                                       | 0.107 / 0.123 / 0.136                                       | p95 <= 250                         | Pass                  |
| Memory session save / load                       | Approx. 1.25 MiB snapshot                                    | 0.102 / 0.118 / 0.125 save; 0.085 / 0.099 / 0.111 load      | each p95 <= 250                    | Pass                  |
| File session save / load                         | 1,272,624-byte persisted snapshot; `fsync` included          | 44.826 / 338.887 / 437.697 save; 0.905 / 1.028 / 1.265 load | save p95 <= 1,000; load p95 <= 500 | Pass                  |

The file-save result is intentionally reported rather than normalized away. A
previous runner observed a much lower value; the successful qualification above
shows why the contract uses a user-visible ceiling with headroom instead of
publishing the fastest hosted-runner sample as an SLA.

## Deterministic work and resource results

| Profile              | Evidence                                                                                                                                                                                                                                                                                        |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent convergence    | 4/4 cases passed with 9 scripted LLM calls, 8 tool attempts, 6 executed tool calls, and 235 accounted tokens. Duplicate calls stopped after the bounded guard path; checkpoint resume preserved accounting.                                                                                     |
| Workspace Retrieval  | Document request amplification was exactly 1.0x for 25,000 admitted inputs; non-text inputs were zero. Published vector bytes were 41,397,932 against a 128 MiB vector ceiling. Maximum rerank scratch was 75,346 bytes against 4 MiB, with zero fallbacks and at most 50 evaluated candidates. |
| Code Intelligence    | The manifest admitted 5,001 files in 44.131 ms. Cold start created one process; shutdown sent both protocol messages, observed one exit, and completed in 0.471 ms. Active and retained RSS deltas were 8,359,936 bytes against 512 MiB and 256 MiB ceilings.                                   |
| Context and memory   | Context output was bounded to 64 items, 2,048 selected tokens, and 10,878 rendered bytes. Active and retained RSS deltas were 52,822,016 and 49,381,376 bytes against 512 MiB and 256 MiB ceilings. Recall ranked the independently marked target first.                                        |
| Flow and State Graph | Replay preserved 1,001 objects and 1,000 relations. Serialized events occupied 9,657,128 bytes against a 64 MiB ceiling.                                                                                                                                                                        |
| Persistence          | Twenty-three generations overwrote one logical session without file accumulation. Memory and file stores both returned one session, preserved snapshot identity and byte shape, and left zero files and zero bytes after delete.                                                                |

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
- assembling a bounded model context should stay below 500 ms;
- a 1,000-step graph projection or replay should stay below 2 seconds;
- a cold controlled language-server request and clean shutdown each have a
  5-second ceiling;
- an atomic, synchronized 1–2 MiB file snapshot should save within 1 second;
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

The current workflow requires exactly seven JSON files and rejects any report
whose top-level `passed` value is not `true`. The authoritative run above
predates `DM-QUAL1` and therefore contains the previous six-profile set; the
first successful seven-profile run will supersede that record. The capability
ledger explains how these profiles combine with deterministic correctness, SDK
runtime, and external qualification evidence.
