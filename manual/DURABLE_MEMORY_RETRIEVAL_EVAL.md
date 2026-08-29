# Durable Memory Retrieval Evaluation

This evaluation answers one narrow architectural question: does the serving
path need semantic vectors now, or can the smallest auditable retrieval design
meet its declared quality and safety floor?

## Decision rule

The versioned fixture declares labels and thresholds separately from runtime
results. A vector serving dependency is justified only when bounded lexical
plus one-hop relation retrieval falls below Recall@5 `0.90`. Changing ranking
code may produce a new versioned report, but must not rewrite the existing
labels or threshold to manufacture a pass.

Fixture v1 fixes the policy at five final results, minimum lexical query-token
coverage `0.40`, and eight exact relation-target reads. The corpus contains:

- six direct lexical queries;
- three queries whose relevant procedure is reachable through an explicit
  `RelatedTo` edge from a lexical anchor;
- one known paraphrase with no token overlap or curated edge;
- an Active conflict target, a hidden Candidate, and an exact matching node in
  a foreign namespace.

The source is
[`core/tests/fixtures/durable-memory-retrieval-v1/corpus.json`](../core/tests/fixtures/durable-memory-retrieval-v1/corpus.json).

## Metrics

Recall@5 is the fraction of labeled queries with a relevant node in the first
five results. Mean reciprocal rank (MRR) averages the reciprocal rank of the
first relevant result. Each fixture-v1 query has one independently declared
relevant node.

| Mode | Recall@5 | MRR |
| --- | ---: | ---: |
| No memory | 0.0000 | 0.0000 |
| Active lexical | 0.6000 | 0.6000 |
| Active lexical + one-hop `RelatedTo` | 0.9000 | 0.7500 |

Relation expansion recovers all three explicitly linked procedures. The
remaining miss is a paraphrase, which is retained as evidence of the known
lexical limitation rather than relabeled away.

## Safety assertions

The public-API integration test also proves that retrieval:

- returns only Active nodes from the exact tenant/principal/scope namespace;
- never expands `ConflictsWith` or recursively traverses the graph;
- respects both the final-result and exact-read bounds;
- leaves admission and use counters unchanged during `preview_recall`;
- still requires successful admission of the current revision before any hit
  enters model context.

These are serving invariants, not quality metrics. A higher recall score cannot
compensate for violating one of them.

## Current conclusion

Fixture v1 meets the predeclared `0.90` relation Recall@5 gate, so A3S Code does
not add embeddings, a vector index, model egress, or vector lifecycle state to
this path. This synthetic result is a deterministic correctness gate, not a
claim about every production distribution. Hosts should retain shadow
measurement and add a new versioned, independently labeled corpus when real
misses show that the decision should change.

The separate [Durable Memory Product Evaluation](DURABLE_MEMORY_PRODUCT_EVAL.md)
reuses these labels through real `AgentSession` turns and adds write precision,
evidence fidelity, conflict preservation, context, call, nominal cost, and
admission gates. Neither deterministic test substitutes for host qualification
on a representative production distribution.

Run the report from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_retrieval_eval -- --nocapture
```
