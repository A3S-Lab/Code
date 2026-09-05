# Workspace Search Real-Model Qualification

`scripts/workspace_search_real_llm.sh` is the bounded end-to-end gate for the
framework-owned workspace search path. It loads the default model and provider
from `.a3s/config.acl`; it does not require MCP and does not expose provider
credentials to the model or the command line.

The gate follows the smallest useful chain of evidence:

1. Deterministic Core tests construct the normal local workspace services,
   prove that the durable `persistent_zvec_fts` projection is used, verify
   that a replaced source cannot leak through a stale generation, and check
   that a burst of saves produces one newest generation. They also cover
   transient publication retry, `Building` status, and obsolete-generation
   cleanup.
2. One ignored live test creates a temporary workspace with a known answer,
   waits for `.a3s-code/index/CURRENT`, and gives the model only the unified
   `search` tool.
3. The prompt leaves mode selection to the model. A natural-language relevance
   question should produce one `bm25` call; the framework keeps the public mode
   stable and routes it to zvec internally.
4. The test rejects extra tools, failed calls, incorrect paths, stale or
   unverified metadata, missing native-index metadata, and an answer not found
   in the returned evidence. The request, tool rounds, context, and timeout
   are bounded to keep provider cost predictable.

If a native query has missing or stale hits while the catalog has a newer
source revision, the public `bm25` call searches the catalog and verifies its
candidates against the live filesystem. The same verification applies when
the native generation is absent or a native operation fails. This keeps newly
admitted content searchable during generation replacement without returning
stale catalog text. Unified search metadata keeps `mode=bm25` for the model
while exposing the internal `execution_mode` for host diagnostics.

When an older immutable generation still serves a verified hit while a newer
snapshot is building, metadata reports `freshness=rebuilding` and includes the
catalog source revision. This distinguishes a safe, source-verified result
from a fully caught-up native generation without making the model coordinate a
refresh.

Index publication is driven by a single background coordinator. It receives
immutable catalog snapshots through a latest-only queue and waits briefly for
an editor save burst to settle before starting one staged build. Catalog
reconciliation and queries therefore never wait for zvec construction; if a
newer snapshot arrives during a build, the old generation remains readable and
the queued newest snapshot is built next. Transient build failures retry with
bounded backoff, and successful publication removes obsolete generations after
the atomic `CURRENT` switch. CPU-heavy document tokenization and normalization
use Rayon across available cores for sufficiently large generations, while
small generations stay serial to avoid scheduler overhead; input order remains
stable for deterministic BM25 ties.

Run it from `crates/code`:

```bash
scripts/workspace_search_real_llm.sh
```

Use another ACL location without changing the test:

```bash
A3S_CONFIG_FILE=/absolute/path/.a3s/config.acl \
  scripts/workspace_search_real_llm.sh
```

Compile the live test without making a provider request:

```bash
scripts/workspace_search_real_llm.sh --dry-run
```

The live result prints only the selected `provider/model`, mode, index kind,
token count, and elapsed turn time. A successful run must report
`mode=bm25 index=persistent_zvec_fts result=pass`.

The latest release benchmark reports (64 chunks, warm native queries)
approximately 0.39 ms p50, 0.51 ms p95, 219 ms initial build, 0.26 ms for a
same-content revision reuse, and 221 ms for a changed-content rebuild. These
are native-index timings; the tool also performs bounded source verification,
and a complete model turn includes provider latency. Changed-content rebuilds
replace a full generation asynchronously, so larger workspaces need their own
refresh-cost measurement.
