# Durable Memory Restart Endurance Evaluation

This versioned deterministic gate qualifies the repeated-restart and retained
run-ID reuse slice of `DM-PROD1`. It exercises the real `AgentSession`,
`FileSessionStore`, and `FileMemoryRepository` boundaries; it does not replace
the Memory kernel's journal, revision, or concurrency contract tests.

The fixture is
[`core/tests/fixtures/durable-memory-restart-endurance-v1/evaluation.json`](../core/tests/fixtures/durable-memory-restart-endurance-v1/evaluation.json).
It fixes the corpus, process epochs, agent count, retention limit, binding
schema, identity profile, and pass thresholds before execution.

## Failure found from first principles

`IdGenerator` promises unique values inside one hosting process. It
intentionally does not promise that a newly started process continues the old
sequence. Binding schema `3` nevertheless derived memory context identity only
from `(session_id, run_id, context_sequence)`. Once normal FIFO retention
removed an old run record, a resumed process could legally reuse that run ID.
The repository then saw a different model context as an exact replay and did
not increment admission history.

The fail-first gate ran two epochs against the same Active revision. At the end
of epoch one it recorded the expected 8 admissions. At the end of epoch two it
required 16 but schema `3` still reported 8.

There was a second failure before retention: if the repeated run ID was still
present, ordinary `send` used a replacing insert and silently overwrote the old
run. The focused `durable_memory_restart` regression observed a second model
call instead of `RUN_IDENTITY_CONFLICT`.

## Corrected ownership model

Binding schema `4` pins
`a3s.code.memory.context.session-run-invocation-sequence-sha256.v2`. Code adds a
fresh internal invocation incarnation to the session, run, and monotonic
context sequence. It does not ask A3S Memory to understand Code run lifecycles,
and it does not strengthen the public host ID contract retroactively.

The incarnation has one purpose: two actual invocations must never collapse
because process-local host IDs restarted. Clones of one live invocation share
the incarnation and sequence. A reconstructed invocation gets a new
incarnation. Only the hash enters the bounded repository context ID; the raw
incarnation, session ID, and run ID do not.

For a duplicate run that is still retained, the ordinary run path now performs
an atomic reservation. It returns `RUN_IDENTITY_CONFLICT` before model use and
does not mutate the prior snapshot, events, or memory admission count. Exact
external command replay continues to use its separate idempotent reservation
protocol.

Schemas `1`, `2`, and `3` remain readable with their exact legacy profiles, but
new code constructs only schema `4`. Exact session resume rejects profile
drift; migration requires a new session.

## Locked endurance gate

Four independently constructed agents share one exact repository namespace.
Each owns a distinct persisted session and completes two concurrent waves per
epoch. Every session retains only one run. After each epoch, all sessions,
agents, session-store handles, bindings, and repository handles close. The next
epoch reopens the files, resumes all sessions, and resets each deterministic
host generator to its original sequence.

The first two epochs use Active revision 1. Before the third epoch, verified
evidence corrects the node to revision 2 while retaining revision 1 in immutable
history. Candidate content, the stale revision, and an Active node under a
foreign principal are forbidden in model context.

| Dimension | Gate | Current result |
| --- | ---: | ---: |
| Durable-memory binding schema | `4` | `4` |
| Process epochs | `3` | `3` |
| Independent agents per epoch | `4` | `4` |
| Session resumes | `8` | `8` |
| Model calls | `24` | `24` |
| Active admissions | `24` | `24` |
| Candidate admissions | `0` | `0` |
| Candidate, stale, or foreign context hits | `0` | `0` |
| File-repository opens | `4` | `4` |
| Retained run IDs reused after restart | `true` | `true` |
| Final node revision/history entries | `2 / 1` | `2 / 1` |

The fourth repository open is a read generation after the final full shutdown.
It verifies the checksummed journal, all admission counts, exact current node,
immutable history, and namespace isolation independently of the live sessions.

## Non-claims

- Three epochs and 24 calls are a deterministic lifecycle gate, not a long
  wall-clock soak or an arbitrary-scale concurrency result.
- The client verifies actual model input context but is not a real provider and
  does not qualify reasoning quality, latency, billed cost, or provider drift.
- The fixture is English and lexical. Cross-language semantic paraphrases
  remain a separate `DM-PROD1` distribution.
- The local file repository does not qualify remote fencing, leases, failover,
  or multi-host storage consistency.
- The gate performs one verified correction; long-horizon consolidation,
  pruning quality, decay, and many-revision distributions remain outstanding.

These limits keep `DM-PROD1` in progress rather than declaring the entire
memory system production-complete.

## Reproduce

Run from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_restart_endurance_eval -- --nocapture
cargo test -p a3s-code-core --test durable_memory_restart -- --nocapture
```

The first command emits the retained JSON report after
`A3S_DURABLE_MEMORY_RESTART_ENDURANCE_EVAL=`.
