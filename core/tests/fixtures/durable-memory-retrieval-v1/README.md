# Durable Memory Retrieval Fixture v1

This synthetic fixture is the deterministic quality and safety contract for
A3S Code V2 memory recall. Relevance labels are declared independently from
returned results. The corpus covers direct lexical matches, explicit
`RelatedTo` hops, a conflict edge, a hidden candidate, and one known
paraphrase miss.

The serving policy is part of the versioned fixture: at most five results,
minimum lexical query-token coverage of `0.40`, and at most eight exact
relation-target reads. The coverage floor prevents a single generic token from
seeding unrelated graph expansion.

The production path runs lexical retrieval first. An opt-in relation policy may
perform a bounded number of exact reads for one-hop `RelatedTo` targets. It
never follows `ConflictsWith`, never recursively walks the graph, never returns
non-Active targets, and never widens the exact namespace. Final context still
requires a successful admission event for the current revision.

The locked v1 result is:

| Mode | Recall@5 | MRR |
| --- | ---: | ---: |
| No memory | 0.0000 | 0.0000 |
| Lexical | 0.6000 | 0.6000 |
| Lexical + `RelatedTo` | 0.9000 | 0.7500 |

Relation expansion recovers all three explicitly linked procedures. The
remaining miss is an English paraphrase with neither token overlap nor a
curated relation. The predeclared vector gate requires relation Recall@5 below
0.90 before Code adds embedding infrastructure to this serving path, so this
fixture does not justify vectors. This is a correctness gate, not a claim that
all production memory distributions have the same quality. Hosts should retain
shadow evaluation and version this corpus when real labeled failures establish
a different requirement.

Reproduce the locked report with:

```text
cargo test -p a3s-code-core --test durable_memory_retrieval_eval -- --nocapture
```

Future ranking changes must add a versioned result set or fixture. They must not
rewrite relevance labels or thresholds to manufacture an improvement.
