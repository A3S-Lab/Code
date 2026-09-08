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

## Active-only binding

Production hosts must use `DurableMemorySession::active_recall` only.
`ShadowCandidates` / `shadow()` were removed (`HARNESS-CONV4` / `CAP-GA1`).
Extraction may still write evidence-backed Candidate nodes; recall admits only
explicitly activated nodes.

## Exit

Mark `DM-PROD1` Delivered in `ROADMAP.md` only when a linked, reproducible host
report pack satisfies every row above. Hermetic Core tests alone do not close
the gate.
