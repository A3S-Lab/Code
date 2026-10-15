# Release profiles (KRN-10)

`HARNESS-CONV5` already published the Cargo feature profiles on
`a3s-code-core`. This note records their size, startup, memory, and feature
behavior after the kernel split. It does not redefine the features.

`a3s-code-core` remains the compatibility facade. `a3s-code-kernel` owns
operation identity, the logical clock, and domain-separated digests. Agent
event adaptation, the digest-only event log, and capability projection stay
on the facade. The other KRN-10 layer names (`a3s-code-engine`,
`a3s-code-data`, `a3s-code-evaluation`, `a3s-code-host`) stay roadmap names.

Measurements below are from the local macOS tree on 2026-09-30, using
`cargo tree -e normal`. They are dependency and artifact facts. They are not
a new process-RSS or admission-latency run. Workload latency stays in
[PERFORMANCE_QUALIFICATION.md](PERFORMANCE_QUALIFICATION.md).

## Kernel

Normal dependency closure, 23 packages: `a3s-code-kernel`, `serde`,
`serde_json`, `sha2`, `thiserror`, and their build dependencies. The closure
does not contain `a3s-search`, `a3s-vec`, `aws-sdk-s3`, `chromiumoxide`,
`rquickjs`, or `opentelemetry`.

| | |
| --- | --- |
| Debug rlib linked by `a3s-code-core` | 1,085,184 bytes |
| Release rlib (`opt-level = z`, fat LTO, strip) | 307,136 bytes |
| Startup | No tokio runtime, HTTP client, browser, S3 client, or vector engine. `SystemLogicalClock::now_ms` reads the system clock when called. |
| Memory | Identity values stay inside the published ceilings: 256-byte ids, 128-byte event types, 4 MiB payload bytes, 256-byte media types, and a 4 GiB artifact size field. The kernel stores digests, not browser, S3, or zvec buffers. |

The same kernel crate is linked by every facade profile. Profile features do
not add browser, S3, or zvec dependencies to it.

## Facade profiles

`a3s-search` (HTTP/RSS) and `rquickjs` are unconditional facade dependencies.
Browser engines are the `headless-search` feature (`a3s-search` features
`headless`, `moli`, `lightpanda`, which also link `chromiumoxide`). QuickJS
is not selected by a profile and is not part of the kernel. `a3s-lane` brings
`opentelemetry` 0.21 into every facade profile; the Code OTLP exporter
(`opentelemetry-otlp` 0.27 and `tracing-opentelemetry`) is only the
`telemetry` feature.

Package counts are unique normal-dependency names.

| Profile | Normal packages | zvec (`a3s-vec`) | Browser (`headless` / `moli` / `lightpanda`, `chromiumoxide`) | S3 (`aws-sdk-s3`) | Code OTLP exporter | Startup and memory behavior |
| --- | --- | --- | --- | --- | --- | --- |
| `minimal` | 324 | no | no | no | no | Governed loop and identity only. No vector index, no S3 client, no browser download. QuickJS stays linked for program tools. |
| `local-code` | 357 | yes | no | no | no | Adds the persistent lexical index, trigram grep pruning, and PDF text extraction. `a3s-vec` loads when that index opens. |
| `scientific` | 403 | yes | yes | no | no | Adds durable SQLite memory, Advanced harness modules, and browser-backed search. The first headless use can download Moli. |
| `server` | 413 | yes | no | yes | yes | Adds the S3 workspace backend and OpenTelemetry OTLP export. Those clients start when the host enables them. |
| `full` | 456 | yes | yes | yes | yes | Union of `scientific` and `server`. |

`default` remains `local-code`. `server` is not the removed AgentDir HTTP
serve surface.
