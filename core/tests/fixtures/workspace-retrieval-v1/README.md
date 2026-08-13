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
aggregate Recall@10 and mean reciprocal rank. Future semantic and hybrid
retrieval must add a new result set to this fixture or version the corpus; it
must not rewrite the BM25 baseline to claim an improvement.

`lifecycle.json` is the independent state-transition contract for unchanged
reconciliation, create, change, rename, delete, and lag recovery. Those cases
are properties of the incremental catalog rather than relevance labels.

All fixture documents are synthetic and contain no repository source or
credentials. The measurement context and adversarial review are recorded in
`manual/WORKSPACE_RETRIEVAL_BASELINE.md`.
