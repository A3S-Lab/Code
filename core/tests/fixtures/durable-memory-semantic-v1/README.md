# Durable Memory Semantic Evaluation V1

This fixture locks the first deterministic semantic-recall gate for A3S Code.
The active memory content is English while the four positive queries are
Chinese, Japanese, Korean, and Arabic and intentionally share no lexical term
with their targets.

The fixture embedding provider maps each text to a declared unit vector. It is
not a model-quality benchmark. It verifies Code's serving architecture:

- the lexical baseline misses every positive query;
- typed semantic recall returns the current Active revision;
- real Agent context assembly admits only the selected revision;
- Candidate, foreign-namespace, and stale-index records produce zero hits;
- the exact embedding, execution, index, policy, authority, and fusion identity
  is persisted as durable-memory binding schema 5.

Run it from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_semantic_eval -- --nocapture
```
