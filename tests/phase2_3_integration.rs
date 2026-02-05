//! Integration tests for Phase 2 & 3 features
//!
//! Tests for reflection, adaptive strategies, and memory system

use a3s_box_code::memory::{AgentMemory, InMemoryStore, MemoryItem, MemoryStore, MemoryType};
use a3s_box_code::planning::Complexity;
use a3s_box_code::reflection::{
    ErrorCategory, ExecutionStrategy, ReflectionConfig, RetryPolicy, StrategySelector,
    ToolReflection,
};
use std::sync::Arc;

// ============================================================================
// Reflection System Tests
// ============================================================================

#[test]
fn test_error_categorization_comprehensive() {
    // Test all error categories
    let test_cases = vec![
        (1, "Permission denied", ErrorCategory::PermissionDenied),
        (1, "No such file or directory", ErrorCategory::NotFound),
        (127, "command not found", ErrorCategory::MissingDependency),
        (1, "syntax error near unexpected token", ErrorCategory::SyntaxError),
        (1, "connection refused", ErrorCategory::NetworkError),
        (1, "operation timed out", ErrorCategory::Timeout),
        (1, "invalid argument", ErrorCategory::InvalidArguments),
        (1, "file already exists", ErrorCategory::AlreadyExists),
        (130, "signal: interrupt", ErrorCategory::RuntimeError),
    ];

    for (exit_code, output, expected) in test_cases {
        let category = ErrorCategory::from_output(exit_code, output);
        assert_eq!(
            category, expected,
            "Failed for: {} (exit {})",
            output, exit_code
        );
    }
}

#[test]
fn test_tool_reflection_builder() {
    let reflection = ToolReflection::failure()
        .with_insight("File not found")
        .with_insight("Path may be incorrect")
        .with_alternative("Check the file path and try again")
        .with_error_category(ErrorCategory::NotFound)
        .with_confidence(0.3)
        .with_retry(true);

    assert!(!reflection.success);
    assert_eq!(reflection.insights.len(), 2);
    assert!(reflection.alternative.is_some());
    assert_eq!(reflection.error_category, Some(ErrorCategory::NotFound));
    assert_eq!(reflection.confidence, 0.3);
    assert!(reflection.should_retry);
}

#[test]
fn test_strategy_selection_comprehensive() {
    let selector = StrategySelector::new();

    // Test complexity-based selection
    assert_eq!(
        selector.select(Complexity::Simple),
        ExecutionStrategy::Direct
    );
    assert_eq!(
        selector.select(Complexity::Medium),
        ExecutionStrategy::Planned
    );
    assert_eq!(
        selector.select(Complexity::Complex),
        ExecutionStrategy::Iterative
    );
    assert_eq!(
        selector.select(Complexity::VeryComplex),
        ExecutionStrategy::Parallel
    );

    // Test prompt-based selection
    let test_cases = vec![
        ("Do this step by step", ExecutionStrategy::Planned),
        ("Please plan this carefully", ExecutionStrategy::Planned),
        ("Iterate and refine the solution", ExecutionStrategy::Iterative),
        ("Improve this code iteratively", ExecutionStrategy::Iterative),
        ("Run these tasks in parallel", ExecutionStrategy::Parallel),
        ("Execute simultaneously", ExecutionStrategy::Parallel),
    ];

    for (prompt, expected) in test_cases {
        let strategy = selector.select_from_prompt(prompt, Complexity::Simple);
        assert_eq!(
            strategy, expected,
            "Failed for prompt: {}",
            prompt
        );
    }
}

#[test]
fn test_retry_policy_comprehensive() {
    let mut policy = RetryPolicy::new(3);

    // Test retryable errors
    assert!(policy.should_retry(Some(ErrorCategory::NetworkError)));
    assert!(policy.should_retry(Some(ErrorCategory::Timeout)));
    assert!(policy.should_retry(Some(ErrorCategory::RuntimeError)));

    // Test non-retryable errors
    assert!(!policy.should_retry(Some(ErrorCategory::PermissionDenied)));

    // Test recoverable errors (should retry even if not in retryable list)
    assert!(policy.should_retry(Some(ErrorCategory::SyntaxError)));
    assert!(policy.should_retry(Some(ErrorCategory::NotFound)));

    // Test retry exhaustion
    policy.increment();
    policy.increment();
    policy.increment();
    assert!(policy.is_exhausted());
    assert!(!policy.should_retry(Some(ErrorCategory::NetworkError)));

    // Test reset
    policy.reset();
    assert!(!policy.is_exhausted());
    assert!(policy.should_retry(Some(ErrorCategory::NetworkError)));
}

#[test]
fn test_retry_policy_backoff() {
    let mut policy = RetryPolicy {
        max_retries: 5,
        current_retries: 0,
        retry_delay_ms: 1000,
        backoff_multiplier: 2.0,
        retryable_errors: vec![],
    };

    // Test exponential backoff
    assert_eq!(policy.next_delay(), 1000); // 1000 * 2^0
    policy.increment();
    assert_eq!(policy.next_delay(), 2000); // 1000 * 2^1
    policy.increment();
    assert_eq!(policy.next_delay(), 4000); // 1000 * 2^2
    policy.increment();
    assert_eq!(policy.next_delay(), 8000); // 1000 * 2^3
}

#[test]
fn test_reflection_config() {
    let config = ReflectionConfig::new()
        .enabled()
        .only_failures()
        .with_confidence_threshold(0.9)
        .with_retry_policy(RetryPolicy::new(5));

    assert!(config.enabled);
    assert!(config.only_on_failure);
    assert_eq!(config.confidence_threshold, 0.9);
    assert_eq!(config.retry_policy.max_retries, 5);
}

#[test]
fn test_execution_strategy_properties() {
    assert!(!ExecutionStrategy::Direct.requires_planning());
    assert!(ExecutionStrategy::Planned.requires_planning());
    assert!(ExecutionStrategy::Iterative.requires_planning());
    assert!(ExecutionStrategy::Parallel.requires_planning());

    assert!(!ExecutionStrategy::Direct.uses_reflection());
    assert!(!ExecutionStrategy::Planned.uses_reflection());
    assert!(ExecutionStrategy::Iterative.uses_reflection());
    assert!(ExecutionStrategy::Parallel.uses_reflection());
}

// ============================================================================
// Memory System Tests
// ============================================================================

#[tokio::test]
async fn test_memory_item_relevance_decay() {
    use chrono::{Duration, Utc};

    // Create an old memory
    let mut old_item = MemoryItem::new("Old memory").with_importance(0.8);
    old_item.timestamp = Utc::now() - Duration::days(60); // 60 days old

    // Create a recent memory
    let recent_item = MemoryItem::new("Recent memory").with_importance(0.8);

    // Recent memory should have higher relevance
    assert!(recent_item.relevance_score() > old_item.relevance_score());
}

#[tokio::test]
async fn test_memory_store_operations() {
    let store = InMemoryStore::new();

    // Store memories
    let item1 = MemoryItem::new("How to create a file")
        .with_tag("file")
        .with_tag("create")
        .with_importance(0.7);
    let item2 = MemoryItem::new("How to delete a file")
        .with_tag("file")
        .with_tag("delete")
        .with_importance(0.6);
    let item3 = MemoryItem::new("How to create a directory")
        .with_tag("directory")
        .with_tag("create")
        .with_importance(0.8);

    store.store(item1.clone()).await.unwrap();
    store.store(item2.clone()).await.unwrap();
    store.store(item3.clone()).await.unwrap();

    // Test count
    assert_eq!(store.count().await.unwrap(), 3);

    // Test retrieve
    let retrieved = store.retrieve(&item1.id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().content, "How to create a file");

    // Test search
    let results = store.search("create", 10).await.unwrap();
    assert_eq!(results.len(), 2);

    // Test search by tags
    let results = store.search_by_tags(&["file".to_string()], 10).await.unwrap();
    assert_eq!(results.len(), 2);

    // Test get recent
    let results = store.get_recent(2).await.unwrap();
    assert_eq!(results.len(), 2);

    // Test get important
    let results = store.get_important(0.7, 10).await.unwrap();
    assert_eq!(results.len(), 2); // item1 (0.7) and item3 (0.8)

    // Test delete
    store.delete(&item1.id).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 2);

    // Test clear
    store.clear().await.unwrap();
    assert_eq!(store.count().await.unwrap(), 0);
}

#[tokio::test]
async fn test_agent_memory_comprehensive() {
    let memory = AgentMemory::in_memory();

    // Test remember success
    memory
        .remember_success(
            "Create a REST API",
            &["write".to_string(), "bash".to_string()],
            "API created successfully",
        )
        .await
        .unwrap();

    // Test remember failure
    memory
        .remember_failure(
            "Delete system file",
            "Permission denied",
            &["bash".to_string()],
        )
        .await
        .unwrap();

    // Test recall similar
    let results = memory.recall_similar("REST API", 5).await.unwrap();
    assert!(!results.is_empty(), "Should find memories about REST API");

    // Test recall by tags
    let results = memory
        .recall_by_tags(&["success".to_string()], 5)
        .await
        .unwrap();
    assert!(!results.is_empty());

    // Test get recent
    let results = memory.get_recent(5).await.unwrap();
    assert_eq!(results.len(), 2);

    // Test memory stats
    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.long_term_count, 2);
    assert_eq!(stats.short_term_count, 2);
}

#[tokio::test]
async fn test_working_memory_management() {
    let memory = AgentMemory::in_memory();

    // Add items to working memory
    for i in 0..15 {
        let item = MemoryItem::new(format!("Task {}", i))
            .with_type(MemoryType::Working)
            .with_importance(i as f32 / 15.0);
        memory.add_to_working(item).await.unwrap();
    }

    // Should be trimmed to max_working (10)
    let working = memory.get_working().await;
    assert_eq!(working.len(), 10);

    // Should keep most relevant items
    for item in &working {
        assert!(item.importance >= 0.3); // Lower importance items should be trimmed
    }

    // Test clear
    memory.clear_working().await;
    let working = memory.get_working().await;
    assert_eq!(working.len(), 0);
}

#[tokio::test]
async fn test_short_term_memory_management() {
    let memory = AgentMemory::in_memory();

    // Add many items
    for i in 0..150 {
        let item = MemoryItem::new(format!("Memory {}", i))
            .with_type(MemoryType::Episodic)
            .with_importance(0.5);
        memory.remember(item).await.unwrap();
    }

    // Short-term should be trimmed to max_short_term (100)
    let short_term = memory.get_short_term().await;
    assert_eq!(short_term.len(), 100);

    // Long-term should have all items
    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.long_term_count, 150);
}

#[tokio::test]
async fn test_memory_types() {
    let memory = AgentMemory::in_memory();

    // Store different types of memories
    let episodic = MemoryItem::new("I created a file yesterday")
        .with_type(MemoryType::Episodic)
        .with_tag("event");

    let semantic = MemoryItem::new("Files are stored in directories")
        .with_type(MemoryType::Semantic)
        .with_tag("fact");

    let procedural = MemoryItem::new("To create a file, use touch command")
        .with_type(MemoryType::Procedural)
        .with_tag("howto");

    memory.remember(episodic).await.unwrap();
    memory.remember(semantic).await.unwrap();
    memory.remember(procedural).await.unwrap();

    // Verify all stored
    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.long_term_count, 3);

    // Test recall by tags
    let events = memory.recall_by_tags(&["event".to_string()], 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].memory_type, MemoryType::Episodic);

    let facts = memory.recall_by_tags(&["fact".to_string()], 10).await.unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].memory_type, MemoryType::Semantic);

    let howtos = memory.recall_by_tags(&["howto".to_string()], 10).await.unwrap();
    assert_eq!(howtos.len(), 1);
    assert_eq!(howtos[0].memory_type, MemoryType::Procedural);
}

// ============================================================================
// Integration Tests
// ============================================================================

#[tokio::test]
async fn test_reflection_and_memory_integration() {
    let memory = AgentMemory::in_memory();

    // Simulate a tool execution with reflection
    let reflection = ToolReflection::failure()
        .with_insight("File not found")
        .with_error_category(ErrorCategory::NotFound)
        .with_alternative("Create the file first");

    // Store the failure in memory
    if !reflection.success {
        memory
            .remember_failure(
                "Read non-existent file",
                "File not found",
                &["read".to_string()],
            )
            .await
            .unwrap();
    }

    // Later, recall similar failures
    let similar_failures = memory
        .recall_by_tags(&["failure".to_string()], 5)
        .await
        .unwrap();

    assert!(!similar_failures.is_empty());
    assert!(similar_failures[0].content.contains("File not found"));
}

#[tokio::test]
async fn test_strategy_and_memory_integration() {
    let memory = AgentMemory::in_memory();
    let selector = StrategySelector::new();

    // Store successful strategy usage
    let strategy = selector.select(Complexity::Complex);
    assert_eq!(strategy, ExecutionStrategy::Iterative);

    // Remember the successful execution
    memory
        .remember_success(
            "Complex refactoring task",
            &["edit".to_string(), "bash".to_string()],
            "Successfully refactored with iterative approach",
        )
        .await
        .unwrap();

    // Later, recall successful patterns
    let patterns = memory
        .recall_similar("refactoring", 5)
        .await
        .unwrap();

    assert!(!patterns.is_empty());
    assert!(patterns[0].content.contains("iterative"));
}

#[test]
fn test_complexity_ordering() {
    // Test that Complexity enum has proper ordering
    assert!(Complexity::Simple < Complexity::Medium);
    assert!(Complexity::Medium < Complexity::Complex);
    assert!(Complexity::Complex < Complexity::VeryComplex);

    // Test with strategy selector thresholds
    let selector = StrategySelector::new();
    assert_eq!(selector.planned_threshold, Complexity::Medium);
    assert_eq!(selector.iterative_threshold, Complexity::Complex);
    assert_eq!(selector.parallel_threshold, Complexity::VeryComplex);
}
