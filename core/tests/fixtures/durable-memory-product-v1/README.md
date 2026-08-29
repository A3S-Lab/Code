# Durable Memory Product Evaluation Fixture v1

This fixture closes the product-integration gate declared by A3S Memory V2.
It evaluates one frozen workload through the real A3S Code session path rather
than treating repository queries alone as evidence of model-facing behavior.

The evaluation has two parts:

1. A completed turn proposes one valid correction, one low-confidence item,
   and one item containing a synthetic credential. The accepted V1 write and
   V2 shadow candidate must preserve the old conflicting memory, reject both
   invalid proposals, and bind the candidate to the exact redacted extraction
   fields through `SessionTurn` evidence.
2. The ten independently labeled retrieval tasks from
   `durable-memory-retrieval-v1` run through no-memory, V1, and active V2
   sessions. A deterministic model succeeds only when the relevant memory is
   present in the actual model system input. The report records task success,
   memory-context tokens, model calls, a deterministic token-price estimate,
   and measured end-to-end turn latency.

The frozen success rates are `0.00`, `0.60`, and `0.90`. V2 therefore proves
that candidate activation, bounded one-hop relations, final context assembly,
and admission work together at the product boundary. The latency and price
ceilings are regression bounds, not performance claims about a remote model.

Run the machine-readable report from the Code workspace:

```text
cargo test -p a3s-code-core --test durable_memory_product_eval -- --nocapture
```

Future changes must add a new fixture version instead of rewriting labels or
thresholds after observing a result.
