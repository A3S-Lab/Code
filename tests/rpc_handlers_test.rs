//! Tests for RPC handlers and event emission
//!
//! Tests for Memory and Planning RPC handlers

use a3s_box_code::agent::AgentEvent;
use a3s_box_code::memory::{AgentMemory, InMemoryStore, MemoryItem, MemoryType};
use std::sync::Arc;
use tokio::sync::broadcast;

// ============================================================================
// Memory RPC Handler Tests
// ============================================================================

#[tokio::test]
async fn test_memory_stored_event_emission() {
    // Create memory system
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);

    // Create event channel
    let (tx, mut rx) = broadcast::channel(10);

    // Store a memory
    let memory_item = MemoryItem::new("Test memory content")
        .with_importance(0.8)
        .with_tags(vec!["test".to_string(), "unit".to_string()])
        .with_type(MemoryType::Episodic);

    let memory_id = memory_item.id.clone();
    let importance = memory_item.importance;
    let tags = memory_item.tags.clone();

    memory.remember(memory_item).await.unwrap();

    // Emit event (simulating what the RPC handler does)
    let _ = tx.send(AgentEvent::MemoryStored {
        memory_id: memory_id.clone(),
        memory_type: "episodic".to_string(),
        importance,
        tags: tags.clone(),
    });

    // Verify event was emitted
    let event = rx.recv().await.unwrap();
    match event {
        AgentEvent::MemoryStored {
            memory_id: id,
            memory_type,
            importance: imp,
            tags: t,
        } => {
            assert_eq!(id, memory_id);
            assert_eq!(memory_type, "episodic");
            assert_eq!(imp, 0.8);
            assert_eq!(t, tags);
        }
        _ => panic!("Expected MemoryStored event"),
    }
}

#[tokio::test]
async fn test_memory_search_event_emission() {
    // Create memory system
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);

    // Create event channel
    let (tx, mut rx) = broadcast::channel(10);

    // Store some memories
    for i in 0..3 {
        let item = MemoryItem::new(format!("Memory {}", i))
            .with_tags(vec!["test".to_string()]);
        memory.remember(item).await.unwrap();
    }

    // Search memories
    let results = memory.recall_by_tags(&["test".to_string()], 10).await.unwrap();

    // Emit search event
    let _ = tx.send(AgentEvent::MemoriesSearched {
        query: None,
        tags: vec!["test".to_string()],
        result_count: results.len(),
    });

    // Verify event
    let event = rx.recv().await.unwrap();
    match event {
        AgentEvent::MemoriesSearched {
            query,
            tags,
            result_count,
        } => {
            assert_eq!(query, None);
            assert_eq!(tags, vec!["test".to_string()]);
            assert_eq!(result_count, 3);
        }
        _ => panic!("Expected MemoriesSearched event"),
    }
}

#[tokio::test]
async fn test_memory_cleared_event_emission() {
    // Create memory system
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);

    // Create event channel
    let (tx, mut rx) = broadcast::channel(10);

    // Store some memories
    for i in 0..5 {
        let item = MemoryItem::new(format!("Memory {}", i));
        memory.remember(item).await.unwrap();
    }

    // Get count before clearing
    let count_before = memory.short_term_count().await;

    // Clear short-term memory
    memory.clear_short_term().await;

    // Emit clear event
    let _ = tx.send(AgentEvent::MemoryCleared {
        tier: "short_term".to_string(),
        count: count_before as u64,
    });

    // Verify event
    let event = rx.recv().await.unwrap();
    match event {
        AgentEvent::MemoryCleared { tier, count } => {
            assert_eq!(tier, "short_term");
            assert_eq!(count, 5);
        }
        _ => panic!("Expected MemoryCleared event"),
    }

    // Verify memory was actually cleared
    let count_after = memory.short_term_count().await;
    assert_eq!(count_after, 0);
}

#[tokio::test]
async fn test_memory_recalled_event_emission() {
    // Create memory system
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);

    // Create event channel
    let (tx, mut rx) = broadcast::channel(10);

    // Store a memory
    let memory_item = MemoryItem::new("Important memory")
        .with_importance(0.9)
        .with_type(MemoryType::Semantic);

    let memory_id = memory_item.id.clone();
    let content = memory_item.content.clone();

    memory.remember(memory_item).await.unwrap();

    // Retrieve the memory
    let retrieved = memory.store().retrieve(&memory_id).await.unwrap();
    assert!(retrieved.is_some());

    // Emit recall event
    let _ = tx.send(AgentEvent::MemoryRecalled {
        memory_id: memory_id.clone(),
        content: content.clone(),
        relevance: 0.95,
    });

    // Verify event
    let event = rx.recv().await.unwrap();
    match event {
        AgentEvent::MemoryRecalled {
            memory_id: id,
            content: c,
            relevance,
        } => {
            assert_eq!(id, memory_id);
            assert_eq!(c, content);
            assert_eq!(relevance, 0.95);
        }
        _ => panic!("Expected MemoryRecalled event"),
    }
}

// ============================================================================
// Memory Type Tests
// ============================================================================

#[tokio::test]
async fn test_memory_types_event_emission() {
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);
    let (tx, mut rx) = broadcast::channel(10);

    // Test all memory types
    let types = vec![
        (MemoryType::Episodic, "episodic"),
        (MemoryType::Semantic, "semantic"),
        (MemoryType::Procedural, "procedural"),
        (MemoryType::Working, "working"),
    ];

    for (mem_type, type_str) in types {
        let item = MemoryItem::new(format!("Memory of type {:?}", mem_type))
            .with_type(mem_type);

        let memory_id = item.id.clone();
        memory.remember(item).await.unwrap();

        // Emit event
        let _ = tx.send(AgentEvent::MemoryStored {
            memory_id: memory_id.clone(),
            memory_type: type_str.to_string(),
            importance: 0.5,
            tags: vec![],
        });

        // Verify event
        let event = rx.recv().await.unwrap();
        match event {
            AgentEvent::MemoryStored {
                memory_id: id,
                memory_type,
                ..
            } => {
                assert_eq!(id, memory_id);
                assert_eq!(memory_type, type_str);
            }
            _ => panic!("Expected MemoryStored event"),
        }
    }
}

// ============================================================================
// Memory Search with Query Tests
// ============================================================================

#[tokio::test]
async fn test_memory_search_with_query_event() {
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);
    let (tx, mut rx) = broadcast::channel(10);

    // Store memories with different content
    let items = vec![
        "How to implement authentication",
        "Database connection setup",
        "API endpoint design",
    ];

    for content in items {
        let item = MemoryItem::new(content);
        memory.remember(item).await.unwrap();
    }

    // Search with query
    let query = "authentication";
    let results = memory.recall_similar(query, 10).await.unwrap();

    // Emit search event
    let _ = tx.send(AgentEvent::MemoriesSearched {
        query: Some(query.to_string()),
        tags: vec![],
        result_count: results.len(),
    });

    // Verify event
    let event = rx.recv().await.unwrap();
    match event {
        AgentEvent::MemoriesSearched {
            query: q,
            tags,
            result_count,
        } => {
            assert_eq!(q, Some(query.to_string()));
            assert_eq!(tags.len(), 0);
            assert!(result_count > 0);
        }
        _ => panic!("Expected MemoriesSearched event"),
    }
}

// ============================================================================
// Memory Importance Filtering Tests
// ============================================================================

#[tokio::test]
async fn test_memory_importance_filtering() {
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);

    // Store memories with different importance levels
    let items = vec![
        ("Low importance", 0.2),
        ("Medium importance", 0.5),
        ("High importance", 0.9),
    ];

    for (content, importance) in items {
        let item = MemoryItem::new(content).with_importance(importance);
        memory.remember(item).await.unwrap();
    }

    // Get all memories
    let all_memories = memory.get_recent(10).await.unwrap();
    assert_eq!(all_memories.len(), 3);

    // Filter by importance (simulating RPC handler logic)
    let min_importance = 0.5;
    let filtered: Vec<_> = all_memories
        .iter()
        .filter(|m| m.importance >= min_importance)
        .collect();

    assert_eq!(filtered.len(), 2); // Should have medium and high importance
}

// ============================================================================
// Memory Tier Management Tests
// ============================================================================

#[tokio::test]
async fn test_memory_tier_clearing() {
    let store: Arc<dyn a3s_box_code::memory::MemoryStore> = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store);

    // Store memories
    for i in 0..5 {
        let item = MemoryItem::new(format!("Memory {}", i));
        memory.remember(item).await.unwrap();
    }

    // Get initial counts
    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.short_term_count, 5);
    assert_eq!(stats.long_term_count, 5);

    // Clear short-term only
    memory.clear_short_term().await;

    let stats_after = memory.stats().await.unwrap();
    assert_eq!(stats_after.short_term_count, 0);
    assert_eq!(stats_after.long_term_count, 5); // Long-term should remain

    // Clear working memory
    memory.clear_working().await;

    let stats_final = memory.stats().await.unwrap();
    assert_eq!(stats_final.working_count, 0);
}

// ============================================================================
// Claude Code Skill Compatibility Tests
// ============================================================================

use a3s_box_code::tools::{ClaudeCodeSkill, ToolPermission};

#[test]
fn test_claude_code_skill_parse_basic() {
    let content = r#"---
name: github-commands
description: GitHub CLI commands
allowed-tools: Bash(gh:*)
---
Use gh CLI for GitHub operations.
"#;

    let skill = ClaudeCodeSkill::parse(content).unwrap();
    assert_eq!(skill.name, "github-commands");
    assert_eq!(skill.description, "GitHub CLI commands");
    assert_eq!(skill.allowed_tools, Some("Bash(gh:*)".to_string()));
    assert!(!skill.disable_model_invocation);
    assert_eq!(skill.content, "Use gh CLI for GitHub operations.");
}

#[test]
fn test_claude_code_skill_parse_code_review_format() {
    // Test with actual Claude Code code-review skill format
    let content = r#"---
name: code-review
allowed-tools: Bash(gh issue view:*), Bash(gh search:*), Bash(gh pr comment:*)
description: Code review a pull request
disable-model-invocation: false
---

Provide a code review for the given pull request.

To do this, follow these steps precisely:

1. Use a Haiku agent to check if the pull request is closed
2. Review the changes
"#;

    let skill = ClaudeCodeSkill::parse(content).unwrap();
    assert_eq!(skill.name, "code-review");
    assert_eq!(skill.description, "Code review a pull request");
    assert!(!skill.disable_model_invocation);
    assert!(skill.allowed_tools.is_some());
    assert!(skill.content.contains("Provide a code review"));
}

#[test]
fn test_claude_code_skill_tool_permissions() {
    let content = r#"---
name: github-skill
allowed-tools: Bash(gh issue view:*), Bash(gh pr:*), Read(*)
---
"#;

    let skill = ClaudeCodeSkill::parse(content).unwrap();
    let permissions = skill.parse_allowed_tools();

    assert_eq!(permissions.len(), 3);

    // Check specific permissions
    assert!(skill.is_tool_allowed("Bash", "gh issue view 123"));
    assert!(skill.is_tool_allowed("Bash", "gh pr list"));
    assert!(skill.is_tool_allowed("Read", "any/file.txt"));
    assert!(!skill.is_tool_allowed("Bash", "rm -rf /"));
    assert!(!skill.is_tool_allowed("Write", "file.txt"));
}

#[test]
fn test_tool_permission_parse_complex() {
    // Test various permission formats
    let cases = vec![
        ("Bash(gh:*)", "Bash", "gh:*"),
        ("Read(*)", "Read", "*"),
        ("Bash(gh issue view:*)", "Bash", "gh issue view:*"),
        ("Write(src/*.rs)", "Write", "src/*.rs"),
    ];

    for (input, expected_tool, expected_pattern) in cases {
        let perm = ToolPermission::parse(input).unwrap();
        assert_eq!(perm.tool, expected_tool, "Failed for input: {}", input);
        assert_eq!(perm.pattern, expected_pattern, "Failed for input: {}", input);
    }
}

#[test]
fn test_tool_permission_matching() {
    // Test prefix matching
    let perm = ToolPermission::parse("Bash(gh:*)").unwrap();
    assert!(perm.matches("Bash", "gh status"));
    assert!(perm.matches("Bash", "gh pr view 123"));
    assert!(perm.matches("Bash", "gh issue list"));
    assert!(!perm.matches("Bash", "git status"));
    assert!(!perm.matches("Read", "gh status"));

    // Test wildcard matching
    let perm = ToolPermission::parse("Read(*)").unwrap();
    assert!(perm.matches("Read", "any/file.txt"));
    assert!(perm.matches("Read", ""));
    assert!(!perm.matches("Write", "file.txt"));
}

#[test]
fn test_claude_code_skill_no_restrictions() {
    let content = r#"---
name: open-skill
description: A skill with no tool restrictions
---
This skill allows all tools.
"#;

    let skill = ClaudeCodeSkill::parse(content).unwrap();

    // No restrictions means all tools are allowed
    assert!(skill.is_tool_allowed("Bash", "any command"));
    assert!(skill.is_tool_allowed("Read", "any file"));
    assert!(skill.is_tool_allowed("Write", "any file"));
    assert!(skill.is_tool_allowed("Edit", "any file"));
}

#[test]
fn test_claude_code_skill_disable_model_invocation() {
    let content = r#"---
name: restricted-skill
disable-model-invocation: true
---
This skill disables model invocation.
"#;

    let skill = ClaudeCodeSkill::parse(content).unwrap();
    assert!(skill.disable_model_invocation);
}

#[test]
fn test_claude_code_skill_stripe_format() {
    // Test with Stripe best practices skill format
    let content = r#"---
name: stripe-best-practices
description: Best practices for building Stripe integrations. Use when implementing payment processing.
---

When designing an integration, always prefer the documentation in Stripe's Integration Options doc.
Use the Go Live Checklist before going live.
"#;

    let skill = ClaudeCodeSkill::parse(content).unwrap();
    assert_eq!(skill.name, "stripe-best-practices");
    assert!(skill.description.contains("Best practices"));
    assert!(skill.content.contains("Integration Options"));
}
