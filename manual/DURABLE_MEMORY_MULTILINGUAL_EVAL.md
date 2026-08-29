# Durable Memory Multilingual Evaluation

This deterministic gate qualifies one narrow claim: the current lexical query
profile can retrieve same-language phrase variations from English and
no-space CJK text through real `AgentSession` context assembly. It also binds
that algorithm identity into persisted session state so an upgrade cannot
silently change recall semantics.

It is a product-contract test, not a general multilingual or semantic-search
claim.

## Versioned contract

The fixture is
[`core/tests/fixtures/durable-memory-multilingual-v1/evaluation.json`](../core/tests/fixtures/durable-memory-multilingual-v1/evaluation.json).
It declares schema version `1`, the exact
`a3s.memory.lexical.word-cjk-bigram.v1` retrieval profile, serving policy,
labeled queries, negative queries, and thresholds before the report is
calculated.

The profile retains lowercase alphanumeric-word matching. A contiguous Han,
Kana, Hangul, Bopomofo, or related CJK run contributes its whole span and each
overlapping character bigram. It does not contribute single-character
unigrams. The whole span rewards exact phrases; bigrams allow bounded partial
phrase overlap in languages that commonly omit spaces.

`DurableMemoryBindingV1.retrievalProfile` persists this identity. Old schema
`1` snapshots without the field remain readable as
`a3s.memory.lexical.word.v1`; schema `2` retains the current lexical profile but
the legacy host-generated context identity; new schema `3` bindings retain both
the current lexical and context-identity profiles. Only those exact combinations
validate. Exact resume rejects reinjection from a current-profile host into a
legacy session, while an old binary rejects schema `3` rather than ignoring the
new fields. The migration rule is to start a new session when query or admission
identity semantics change.

## Locked gate

The corpus contains one relevant Active procedure for each of English,
Simplified Chinese, Japanese, and Korean, plus same-namespace distractors, an
unverified Candidate, and an Active node in a foreign tenant. Each positive
query uses same-language phrase variation rather than copying the complete
stored sentence.

| Dimension | Gate | Current result |
| --- | ---: | ---: |
| Recall@3 | `1.00` minimum | `1.00` |
| Mean reciprocal rank | `1.00` minimum | `1.00` |
| Model calls per query | `1` maximum | `1` |
| Memory nodes in model context | `1` maximum | `1` |
| Exact-revision admissions | `4` | `4` |
| Negative queries with hits | `0` | `0` |
| Candidate or foreign-tenant leaks | `0` | `0` |

The test first runs pure `preview_recall` to lock ranking, then creates a fresh
real `AgentSession` for every positive query. A deterministic `LlmClient`
inspects the final system context, proving that the expected node, not merely a
repository hit, reaches model input. Repository usage summaries prove exactly
one persisted admission per task and no fabricated downstream use events.

The negative set locks three boundaries:

- an English translation of the Chinese migration concept does not match the
  Chinese node without lexical overlap;
- an exact Candidate query returns nothing;
- an exact foreign-tenant query returns nothing.

## Non-claims and remaining qualification

- This is lexical retrieval, not translation or cross-language semantic
  retrieval.
- Four synthetic tasks do not represent language, morphology, domain, or
  production-query distributions.
- The deterministic model checks context availability and isolation, not real
  model reasoning quality.
- Long-horizon retention, repeated restarts, semantic paraphrases, larger
  multi-agent distributions, drift, latency, and real provider cost remain
  `DM-PROD1` host qualification. The narrow explicit-sharing and collision
  contract is covered separately by `DM-SHARE1`.

These limits are intentional. A semantic-vector dependency remains deferred
until an independently labeled versioned corpus shows the lexical/relation
gate falling below its predeclared threshold.

## Reproduce

Run from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_multilingual_eval -- --nocapture
```

The command prints its retained JSON value after the stable
`A3S_DURABLE_MEMORY_MULTILINGUAL_EVAL=` marker.
