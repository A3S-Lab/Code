# Durable Memory Production Qualification (`DM-PROD1`)

Hermetic product-contract gates live in
[`DURABLE_MEMORY_PRODUCT_EVAL.md`](DURABLE_MEMORY_PRODUCT_EVAL.md) and related
`DURABLE_MEMORY_*` manuals. Those prove session/API invariants with
deterministic clients.

`DM-PROD1` (`HARNESS-CONV7`) is the production qualification overlay. It remains
**In progress** until host reports cover the items below without weakening
namespace, evidence, history, admission, or lifecycle invariants.

Evidence templates live in [HARNESS_CONVERGENCE.md](HARNESS_CONVERGENCE.md).

## Required host report dimensions

| Dimension | Pass criteria |
| --- | --- |
| Long-horizon consolidation / decay | Representative multi-session horizons; no silent Active-node loss |
| Real embedding provider | Exact provider/model identity recorded; billed-cost and latency distributions retained |
| Larger multi-agent load | Shared-index revision-CAS holds under concurrent writers |
| Repeated restart | Exact binding resume; semantic generation identity stable |
| Durable remote CAS + leases | Remote backend + distributed lease policy; failover exercised |
| Drift / cache | Cache-hit vs rebuild distributions; no unauthorized namespace widening |
| Secret hygiene | Reports and diagnostics contain no provider credentials or prompt plaintext |

## Host harness

`core/examples/durable_memory_prod1_host` produces a report pack covering every
row above against real backends. It is gated behind the `dm-prod1-host` Cargo
feature, which is the only thing that pulls in the `redis` client; no library
module depends on Redis.

```bash
./scripts/harbor/run_dm_prod1_host.sh
```

The script sources credentials from `scripts/harbor/.env` (materializing it from
`.a3s/config.acl` when absent), preflights Redis, builds `--release`, runs the
harness, and prints the pack path.

| Input | Default | Notes |
| --- | --- | --- |
| `BOYUE_API_KEY`, `BOYUE_BASE_URL` | required | Read from the environment; never written to the pack |
| `A3S_DM_PROD1_EMBED_MODEL` | `text-embedding-3-small` | Served dimension is probed, not assumed |
| `A3S_DM_PROD1_REDIS_URL` | `redis://127.0.0.1:6379/15` | Database 0 is refused unless `A3S_DM_PROD1_ALLOW_DEFAULT_DB=1` |
| `A3S_DM_PROD1_WRITERS` | `8` | Independent sockets racing one shared index prefix |
| `A3S_DM_PROD1_LEASE_TTL_SECONDS` | `600` | Not renewed, so it must outlive the run |
| `A3S_DM_PROD1_PACK_DIR` | `/tmp/dm-prod1-host-<code sha>` | Contains `report.json`, `MANIFEST.json`, `HYGIENE_OK` |

Every Redis key the harness creates lives under a per-run prefix
(`a3s:dm-prod1:<random>`) and is removed by scoped `DEL` on exit. The harness
never issues `FLUSHDB`.

What the harness injects, rather than what ships in `a3s-memory`:

- `RedisVectorIndex`: a `VectorIndex` whose `mutation_consistency` is
  `IndexRevisionCas`, implemented with `WATCH` / `MULTI` / `EXEC` on a revision
  hash and a `sha256` history digest chained over published revisions.
- `RedisLeasePolicy`: `SET key value NX EX` with `INCR` fence tokens and an
  owner-compare Lua release.
- `BoyueEmbeddingProvider`: OpenAI-compatible `POST /embeddings` with recorded
  per-request latency, token usage, and returned-vector L2 norms.

### Caveats retained in the pack

Each satisfied row carries its own `caveat` in `report.json`. These narrow the
row without weakening it, and they are the honest residue of a single-host run:

- Horizons are single-process and minutes-scale. Multi-day wall-clock decay and
  cross-deployment session horizons are not covered.
- Concurrent-writer load is one process over one Redis database. Multi-host
  writer fleets are not covered.
- Failover is a client-side connection drop (`CLIENT KILL`) plus an
  independently recreated keyspace. Redis Sentinel or Cluster primary promotion
  is not exercised.
- The epoch lease is not renewed; its TTL simply outlives the run. There is no
  lease-renewal loop to qualify.
- Semantic recall places the paraphrased target query inside the bounded
  candidate set but not always at rank 0. The observed rank is recorded as
  evidence rather than gated on, so the row grades the durable-memory machinery
  instead of the embedding model.

## Active-only binding

Production hosts must use `DurableMemorySession::active_recall` only.
`ShadowCandidates` / `shadow()` were removed (`HARNESS-CONV4` / `CAP-GA1`).
Extraction may still write evidence-backed Candidate nodes; recall admits only
explicitly activated nodes.

## Exit

Mark `DM-PROD1` Delivered in `ROADMAP.md` only when a linked, reproducible host
report pack satisfies every row above. Hermetic Core tests alone do not close
the gate.
