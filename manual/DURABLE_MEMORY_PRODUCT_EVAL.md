# Durable Memory Product Evaluation

This evaluation tests the product path that the retrieval-only fixture cannot:
whether durable memory changes the result of a real `AgentSession` turn while
preserving the write, evidence, conflict, context, lifecycle, and cost
boundaries declared by the integration design.

It compares three serving arms through the same session API:

- no memory;
- V1 memory recall;
- V2 active-only lexical recall with bounded one-hop `RelatedTo` expansion.

The test uses deterministic `LlmClient` implementations and performs no
network calls. It exercises the production session builder, turn-context
assembly, V1 recall, V2 repository query, admission recording, model input,
post-turn extraction, redaction, and candidate shadow write. It is therefore a
hermetic product-contract gate, not a real-model quality claim.

## Versioned inputs

The product fixture is
[`core/tests/fixtures/durable-memory-product-v1/evaluation.json`](../core/tests/fixtures/durable-memory-product-v1/evaluation.json).
It reuses the independently labeled
[`durable-memory-retrieval-v1`](../core/tests/fixtures/durable-memory-retrieval-v1/corpus.json)
corpus instead of defining a second retrieval truth set. The deterministic
model harness is isolated in
[`core/tests/durable_memory_product_eval/model.rs`](../core/tests/durable_memory_product_eval/model.rs).

The capture case presents three proposals:

1. one high-confidence workspace correction that conflicts with an existing
   V1 item;
2. one low-confidence ambiguous statement;
3. one credential-bearing procedural statement.

Only the first proposal is admissible. The test requires the old V1 item to
remain available for audit, the replacement to retain its conflict link, and
the V2 shadow to remain a `Candidate` with one exact `SessionTurn` evidence
reference over the bounded redacted turn.

## Locked gates

The fixture declares the following gates before the runtime report is
calculated:

| Dimension | Gate |
| --- | ---: |
| No-memory task success | `0.00` |
| V1 task success | `0.60` |
| V2 task success | `0.90` |
| Accepted-write precision | `1.00` minimum |
| Evidence fidelity | `1.00` minimum |
| Conflict preservation | Required |
| Memory context per task | `512` estimated tokens maximum |
| Model calls per retrieval task | `1` maximum |
| Model calls for capture and extraction | `2` maximum |
| Estimated model cost per retrieval task | `$0.05` maximum |
| Development-test p95 | `10,000 ms` maximum |

Token counts are a deterministic character-based proxy used to catch prompt
amplification. Cost is derived from those proxy counts using the fixture's
nominal input and output rates; it is not a provider invoice. The broad p95
ceiling catches hangs and accidental extra work, but does not qualify a
production latency SLO. Release performance claims still require the profile,
machine metadata, warmups, sample count, and retained artifact defined by
[Capability Verification](CAPABILITY_VERIFICATION.md).

## Current deterministic result

The current report is:

| Arm | Successes | Success rate | Model calls | Memory context tokens | Maximum per task | Admissions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| No memory | 0/10 | `0.00` | 10 | 0 | 0 | 0 |
| V1 | 6/10 | `0.60` | 10 | 171 | 45 | 0 |
| V2 | 9/10 | `0.90` | 10 | 174 | 25 | 12 |

Capture accepts one of three proposals, produces one V2 candidate, achieves
write precision `1.00` and evidence fidelity `1.00`, preserves the conflicting
old memory, and uses exactly two model calls. V2 records more admissions than
successful tasks because bounded relation expansion can admit both the lexical
anchor and its related procedure. Use remains zero: the deterministic client
checks model input but does not claim that it cited or selected a memory. This
keeps admission and downstream use as separate facts.

The missing tenth V2 task is the fixture's known no-token-overlap paraphrase.
It is not relabeled away. Together with the retrieval-only result, this keeps
semantic vectors deferred until a new versioned corpus demonstrates failures
below the declared `0.90` gate.

## Consolidation evidence

Product serving is not allowed to erase history during consolidation.
`owned_host_job_applies_verified_atomic_v2_supersession` runs a real
session-owned `MemoryMaintenanceJob` against the exact V2 binding and proves
one atomic change set can:

- create a replacement Candidate with `SessionTurn` proposal evidence;
- activate that exact revision with separate `Verification` evidence;
- add inverse `Supersedes` and `SupersededBy` relations;
- mark the old node `Superseded` without deleting its revision history;
- report affected-item health and stop running after session close.

The test demonstrates the mechanism and integrity boundary. The host still
owns the semantic decision to propose and verify a replacement; neither Code's
scheduler nor A3S Memory's repository invents that policy.

## Known limits

- The corpus is small, English, synthetic, and coding-workflow focused.
- The model is deterministic and tests context availability, not reasoning
  quality or robustness to a real provider.
- Cost and tokens are normalized proxies, not billed usage.
- File-repository crash recovery, concurrency, and revision integrity are
  covered in A3S Memory's kernel tests rather than duplicated here.
- Long-horizon personalization, multi-agent sharing, decay quality, and
  production-distribution drift require host-owned versioned evaluations.

These limits prevent the deterministic gate from being misrepresented as
production qualification.

## Reproduce

Run from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_product_eval -- --nocapture
cargo test -p a3s-code-core --test memory_maintenance_lifecycle owned_host_job_applies_verified_atomic_v2_supersession -- --nocapture
```

The first command prints the complete report after the stable
`A3S_DURABLE_MEMORY_PRODUCT_EVAL=` marker.
