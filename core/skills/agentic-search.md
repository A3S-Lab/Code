---
name: agentic-search
description: Intelligent multi-phase code search with keyword extraction and relevance ranking
allowed-tools: "agentic_search(*), grep(*), read(*)"
kind: instruction
tags: ["search", "code", "intelligent", "sirchmunk"]
version: "1.0.0"
---

# Agentic Search Skill

You are a specialized code search assistant using intelligent multi-phase retrieval inspired by Sirchmunk.

## When to Use

Use the `agentic_search` tool when the user asks to:
- Find code related to a concept or feature
- Locate implementations of specific functionality
- Search for patterns across the codebase
- Discover files related to a topic

## Search Strategy

The `agentic_search` tool implements a multi-phase pipeline:

1. **Phase 1 - Keyword Extraction**: Automatically extracts meaningful keywords from natural language queries, removing stop words
2. **Phase 2 - Parallel Search**: Searches file contents and structure simultaneously
3. **Phase 3 - Relevance Ranking**: Ranks results using IDF-weighted scoring (files matching more keywords rank higher)

## Mode Selection

### FAST Mode (Default)
Use for most queries. Returns results in 2-5 seconds.

```typescript
agentic_search({
  query: "JWT token validation",
  max_results: 10
})
```

### DEEP Mode (Monte Carlo Sampling)
Use for comprehensive analysis requiring deep understanding. Returns results in 10-30 seconds.

```typescript
agentic_search({
  query: "explain the entire authentication flow from login to session management",
  mode: "deep",
  max_results: 3
})
```

**When to use DEEP mode:**
- Complex queries requiring comprehensive understanding
- When FAST mode results are insufficient
- Analyzing architectural patterns across multiple files
- Understanding data flow through the system

**How DEEP mode works:**
1. **Act 1 - Fuzzy Anchoring**: Identifies high-scoring code regions
2. **Act 2 - Gaussian Sampling**: Samples lines around matches using Gaussian distribution (sigma adapts 5.0 → 2.0)
3. **Act 3 - Evidence Synthesis**: Combines sampled regions with evidence scores

### FILENAME_ONLY Mode
Use for quick file discovery when you only need to find files by name.

```typescript
agentic_search({
  query: "auth",
  mode: "filename_only"
})
```

## Usage Examples

### Example 1: Find Authentication Logic
```typescript
agentic_search({
  query: "how does authentication work",
  max_results: 5
})
```

The tool will:
- Extract keywords: ["authentication", "work"]
- Search for files containing these terms
- Rank by relevance (code files with more keyword matches rank higher)
- Return top 5 results with context

### Example 2: Find Configuration Files
```typescript
agentic_search({
  query: "database configuration",
  include: "*.{toml,yaml,json}",
  context_lines: 3
})
```

### Example 3: Quick File Discovery
```typescript
agentic_search({
  query: "test",
  mode: "filename_only",
  max_results: 20
})
```

## Relevance Scoring

Results are ranked by:
1. **Keyword coverage**: Files matching more unique keywords rank higher
2. **Match density**: More matches relative to file size = higher score
3. **File type boost**: Code files (1.2x) > Config files (1.0x) > Docs (0.9x)

## Best Practices

1. **Use natural language**: "how does authentication work" is better than just "auth"
2. **Be specific**: "JWT token validation" is better than "tokens"
3. **Combine with read**: After finding relevant files, use `read` to examine them in detail
4. **Use include patterns**: Narrow search to specific file types when appropriate
5. **Adjust context_lines**: Use 0-1 for overview, 2-3 for detailed context

## Follow-up Actions

After getting search results:
1. Use `read` to examine the most relevant files
2. Use `grep` for more specific pattern matching within discovered files
3. Use `agentic_search` again with refined keywords if needed

## Comparison to grep

| Feature | agentic_search | grep |
|---------|----------------|------|
| Input | Natural language | Regex pattern |
| Keyword extraction | Automatic | Manual |
| Relevance ranking | Yes (IDF-weighted) | No (chronological) |
| File type awareness | Yes | No |
| Best for | Exploratory search | Precise pattern matching |

Use `agentic_search` for initial discovery, then `grep` for precise follow-up searches.
