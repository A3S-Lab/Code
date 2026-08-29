# Durable memory restart endurance evaluation v1

This fixture qualifies the deterministic repeated-restart slice of
`DM-PROD1`. Four independent agents share one exact file-backed durable-memory
namespace for two turns in each of three complete process epochs. Every epoch
closes all sessions, agents, stores, and repository owners before the next one
reopens and resumes the persisted sessions.

Each session retains one run while its deterministic host ID generator resets
to the same sequence after every restart. Consequently, the retained run IDs
are deliberately reused after FIFO eviction. Binding schema `4` and
`a3s.code.memory.context.session-run-invocation-sequence-sha256.v2` must still
record all 24 distinct model contexts. Schema `3` collapsed the second epoch
into the first and reported only 8 admissions where 16 were required.

The gate also revises the Active node before the final epoch, verifies current
revision serving and immutable history, rejects Candidate, stale-revision, and
foreign-principal context, checks exact binding restoration, and reopens the
checksummed Memory journal for a final fourth read generation.

This is a bounded deterministic endurance contract, not a claim about real
provider quality, remote repository fencing, arbitrary agent counts, or
months-long retention.
