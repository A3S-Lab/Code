# Workspace Retrieval Fixture v1

This directory is the versioned relevance contract for the Workspace Retrieval
program (`WSR-00`). `corpus.json` intentionally contains both strengths and
known limitations of the current native BM25 implementation:

- exact multi-term queries;
- full and split code identifiers;
- exact CJK queries;
- English paraphrases with no lexical overlap;
- one cross-language paraphrase.

The BM25 baseline test executes the real tool and locks every result path plus
aggregate Recall@10 and mean reciprocal rank. The hybrid test independently
locks `expected_hybrid_paths`, aggregate metrics, identifier protection, and
the required Recall@10 improvement. Its deterministic embedding provider maps
only annotated query/document pairs; zero-similarity records are rejected and
cannot manufacture recall by filling Top-10. Future retrieval changes must add
a new result set or version the corpus; they must not rewrite either baseline
to claim an improvement.

The locked v1 result is:

| Mode | Recall@10 | MRR | Paraphrase Recall@10 |
| --- | ---: | ---: | ---: |
| BM25 | 0.6667 | 0.6667 | 0.0000 |
| Hybrid | 1.0000 | 1.0000 | 1.0000 |

Hybrid therefore improves whole-fixture Recall@10 by 0.3333 while preserving
first-rank identifier behavior.

`lifecycle.json` is the independent state-transition contract for unchanged
reconciliation, create, change, rename, delete, and lag recovery. Those cases
are properties of the incremental catalog rather than relevance labels.

All fixture documents are synthetic and contain no repository source or
credentials. The measurement context and adversarial review are recorded in
`manual/WORKSPACE_RETRIEVAL_BASELINE.md`.
