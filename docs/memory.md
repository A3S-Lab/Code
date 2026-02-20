# Memory System

A3S Code uses a two-layer memory architecture split across two crates.

## Layers

```
a3s-memory                          a3s-code
──────────────────────────────      ──────────────────────────────
MemoryStore (trait)                 AgentMemory
InMemoryStore                         ├── working    (Vec, max 10)
FileMemoryStore                       ├── short_term (VecDeque, max 100)
MemoryItem                            └── long_term  (→ MemoryStore)
MemoryType
RelevanceConfig                     MemoryConfig
                                    MemoryStats
                                    MemoryContextProvider
```

**`a3s-memory`** owns the storage abstraction and its default implementations. It has no knowledge of agents, sessions, or context injection.

**`a3s-code`** owns the three-tier session memory (`AgentMemory`) and the bridge that injects memories into agent prompts (`MemoryContextProvider`).

## Memory Types

| Type | Purpose | Typical importance |
|------|---------|-------------------|
| `Episodic` | Specific events and experiences | 0.5 (default) |
| `Semantic` | Facts and knowledge | 0.5–0.8 |
| `Procedural` | Successful patterns (auto-stored) | 0.8 |
| `Working` | Active context, current task | varies |

Failures are stored as `Episodic` with importance `0.9` — higher than successes so the agent avoids repeating them.

## Relevance Scoring

```
score = importance × importance_weight + decay × recency_weight
decay = exp(−age_days / decay_days)
```

Default config: `importance_weight = 0.7`, `recency_weight = 0.3`, `decay_days = 30`.

`AgentMemory` uses its own `RelevanceConfig` (from `MemoryConfig`) for working memory trimming. `MemoryItem::relevance_score_at()` accepts a `&RelevanceConfig` so scoring is always explicit.

## Three-Tier Session Memory

```
Working memory   — active context, auto-trimmed by relevance when over capacity
Short-term       — current session, FIFO trim when over capacity
Long-term        — persisted via MemoryStore, survives session restarts
```

`remember()` writes to both long-term and short-term simultaneously.
`remember_success()` / `remember_failure()` are convenience methods that set appropriate importance and tags automatically.

## Configuration

Via `SessionOptions`:

```rust
// File-backed long-term memory
SessionOptions::new()
    .with_file_memory("./memory")

// Custom backend
SessionOptions::new()
    .with_memory(Arc::new(MyVectorStore::new()))
```

Via HCL:

```hcl
memory {
  relevance {
    decay_days        = 30.0
    importance_weight = 0.7
    recency_weight    = 0.3
  }
  max_short_term = 100
  max_working    = 10
}
```

## Context Injection

`MemoryContextProvider` implements `ContextProvider` and is registered alongside other RAG providers. On each turn:

1. `query()` — retrieves up to 5 relevant memories by substring match, injects into system prompt
2. `on_turn_complete()` — stores the successful interaction as a `Procedural` memory

## Implementing a Custom Backend

```rust
use a3s_memory::{MemoryItem, MemoryStore};

struct MyVectorStore { /* ... */ }

#[async_trait::async_trait]
impl MemoryStore for MyVectorStore {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        // embed item.content and upsert into vector DB
    }

    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        // embed query, ANN search, return top-k
    }

    // ... remaining methods
}

SessionOptions::new().with_memory(Arc::new(MyVectorStore::new()))
```

The `search()` method in `FileMemoryStore` uses substring matching. A vector store implementation would replace this with semantic similarity — no other changes needed.
