# Durable memory multi-agent evaluation v1

This fixture qualifies explicit host-authorized sharing of one durable-memory
binding across two independent `Agent` instances.

The evaluation report remains schema `1`; it requires durable-memory binding
schema `3`, which persists the context identity profile alongside retrieval
semantics.

Both agents intentionally use separate deterministic host environments that
emit the same local run-ID sequence. Their distinct session identities must
still produce distinct admission events. The gate also verifies that candidate
and foreign namespace nodes never enter model context, that one agent can
continue after the other closes, and that all admissions survive a
file-repository restart.

The context identity profile hashes the exact
`(session_id, run_id, context_sequence)` tuple with domain separation. The
sequence is shared by every clone of one immutable invocation, so multiple
model contexts in one run remain distinct. It does not inherit durable memory
into delegated children; sharing remains an explicit host binding decision.
