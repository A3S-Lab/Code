# Durable Memory Multi-Agent Evaluation

This deterministic gate qualifies one narrow production mechanism: a host can
explicitly share one exact durable-memory namespace across independent agents
without collapsing their admission history or granting implicit authority to
other principals or delegated children.

It is not evidence that arbitrary multi-agent workloads, retention horizons,
or production providers are fully qualified.

## Authority and identity contract

The fixture is
[`core/tests/fixtures/durable-memory-multi-agent-v1/evaluation.json`](../core/tests/fixtures/durable-memory-multi-agent-v1/evaluation.json).
It declares schema version `1`, the exact context identity profile, corpus,
agent session identities, and thresholds before the report is calculated.

Sharing is explicit: the host injects clones of one `DurableMemorySession` into
two independently constructed `Agent` instances. That typed binding contains
the same repository object and exact tenant/principal/scope namespace. Code
does not use a global backend and does not inherit memory into delegated
children.

The profile
`a3s.code.memory.context.session-run-sequence-sha256.v1` hashes the exact
`(session_id, run_id, context_sequence)` tuple with domain separation. The
sequence belongs to the immutable invocation and is shared across its clones.
It has four properties:

- the same exact tuple deterministically produces the same context ID;
- different sessions produce different context IDs even when independent
  deterministic host environments emit the same process-local run ID;
- multiple model contexts assembled inside one planned run remain separate;
- the opaque `a3s-code-context-v1-*` value is bounded for the Memory repository
  and does not expose raw session or run identifiers.

`DurableMemoryBindingV1.contextIdProfile` persists this profile in schema `3`.
Schema `1` and `2` snapshots remain readable with the explicit legacy
`a3s.code.memory.context.host-id.v0` identity, but a current host cannot silently
resume them with new admission keys. Migration requires a new session.

If Code cannot obtain the scoped invocation identity, it removes the recalled
V2 items before the model call. The context ID is correlation evidence, not an
authentication token; the exact namespace binding remains the authority.

## Locked gate

Two real agents share one `FileMemoryRepository`. Each has a unique explicit
session ID but a separate `SequentialIdGenerator` with the same prefix and
counter. Their first turns run concurrently behind a model barrier, so both
generate the same local run ID. The second agent then closes and the first
agent completes another turn. Finally, every owner closes and the repository
is reopened from its checksummed journal.

The shared namespace contains one Active procedure and one Candidate shortcut.
The same repository also contains an Active procedure under a foreign
principal.

| Dimension | Gate | Current result |
| --- | ---: | ---: |
| Durable-memory binding schema | `3` | `3` |
| Independent agents | `2` | `2` |
| Real model calls | `3` | `3` |
| First process-local run IDs collide | `true` | `true` |
| Shared Active admissions | `3` | `3` |
| Candidate admissions | `0` | `0` |
| Candidate or foreign-principal context hits | `0` | `0` |
| Admissions after repository replay | `3` | `3` |

Before the context-identity correction, the first two admissions collapsed and
the gate observed only `2` total admissions. The locked result proves that the
collision is removed and journal replay preserves the resulting history. It
also proves that closing one agent does not revoke the other agent's explicit
binding and that foreign-principal state cannot cross the namespace boundary.

The profile alone does not claim that an admission reconstructed later with a
different timestamp is the same event. Memory's idempotency contract compares
the complete access event; conflicting replay fails closed.

## Non-claims and remaining qualification

- The deterministic client verifies final context and lifecycle behavior, not
  real-model reasoning quality.
- Two agents and three turns are not a concurrency soak or a representative
  organization workload.
- One close/reopen boundary is not a repeated-restart endurance test.
- The gate does not qualify semantic cross-language retrieval, consolidation
  policy, remote repository fencing, latency, provider drift, or cost.
- Delegated children remain isolated. A future inheritance policy would need a
  separate explicit authority design and versioned test; this gate does not
  authorize it.

Those remaining distributions stay under `DM-PROD1`.

## Reproduce

Run from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_multi_agent_eval -- --nocapture
```

The command prints the retained JSON value after the stable
`A3S_DURABLE_MEMORY_MULTI_AGENT_EVAL=` marker.
