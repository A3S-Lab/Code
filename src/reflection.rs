//! Reflection and adaptive strategy components for the agent
//!
//! This module provides self-reflection capabilities and adaptive strategy selection
//! to enable more intelligent and autonomous agent behavior.

use crate::planning::Complexity;
use serde::{Deserialize, Serialize};

// ============================================================================
// Tool Reflection
// ============================================================================

/// Reflection on tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReflection {
    /// Was the tool execution successful?
    pub success: bool,
    /// Insights learned from this execution
    pub insights: Vec<String>,
    /// Should we retry with a different approach?
    pub should_retry: bool,
    /// Alternative approach suggestion
    pub alternative: Option<String>,
    /// Confidence in the result (0.0 - 1.0)
    pub confidence: f32,
    /// Error category if failed
    pub error_category: Option<ErrorCategory>,
}

impl ToolReflection {
    /// Create a new successful reflection
    pub fn success() -> Self {
        Self {
            success: true,
            insights: Vec::new(),
            should_retry: false,
            alternative: None,
            confidence: 1.0,
            error_category: None,
        }
    }

    /// Create a new failed reflection
    pub fn failure() -> Self {
        Self {
            success: false,
            insights: Vec::new(),
            should_retry: true,
            alternative: None,
            confidence: 0.0,
            error_category: None,
        }
    }

    /// Add an insight
    pub fn with_insight(mut self, insight: impl Into<String>) -> Self {
        self.insights.push(insight.into());
        self
    }

    /// Add multiple insights
    pub fn with_insights(mut self, insights: Vec<String>) -> Self {
        self.insights.extend(insights);
        self
    }

    /// Set alternative approach
    pub fn with_alternative(mut self, alternative: impl Into<String>) -> Self {
        self.alternative = Some(alternative.into());
        self
    }

    /// Set confidence level
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Set error category
    pub fn with_error_category(mut self, category: ErrorCategory) -> Self {
        self.error_category = Some(category);
        self
    }

    /// Set should retry flag
    pub fn with_retry(mut self, should_retry: bool) -> Self {
        self.should_retry = should_retry;
        self
    }
}

impl Default for ToolReflection {
    fn default() -> Self {
        Self::success()
    }
}

/// Error categories for tool failures
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Syntax error in command or code
    SyntaxError,
    /// File or resource not found
    NotFound,
    /// Permission denied
    PermissionDenied,
    /// Network or connection error
    NetworkError,
    /// Timeout exceeded
    Timeout,
    /// Invalid arguments provided
    InvalidArguments,
    /// Resource already exists
    AlreadyExists,
    /// Dependency missing
    MissingDependency,
    /// Runtime error during execution
    RuntimeError,
    /// Unknown or unclassified error
    Unknown,
}

impl ErrorCategory {
    /// Determine error category from exit code and output
    pub fn from_output(exit_code: i32, output: &str) -> Self {
        let output_lower = output.to_lowercase();

        // Check for common error patterns (order matters - more specific first)
        if output_lower.contains("permission denied") || output_lower.contains("access denied") {
            return Self::PermissionDenied;
        }
        if output_lower.contains("not installed")
            || output_lower.contains("command not found")
            || output_lower.contains("module not found")
        {
            return Self::MissingDependency;
        }
        if output_lower.contains("not found")
            || output_lower.contains("no such file")
            || output_lower.contains("does not exist")
        {
            return Self::NotFound;
        }
        if output_lower.contains("syntax error")
            || output_lower.contains("parse error")
            || output_lower.contains("unexpected token")
        {
            return Self::SyntaxError;
        }
        if output_lower.contains("timeout")
            || output_lower.contains("timed out")
            || output_lower.contains("deadline exceeded")
        {
            return Self::Timeout;
        }
        if output_lower.contains("connection refused")
            || output_lower.contains("network")
            || output_lower.contains("unreachable")
        {
            return Self::NetworkError;
        }
        if output_lower.contains("already exists") || output_lower.contains("file exists") {
            return Self::AlreadyExists;
        }
        if output_lower.contains("invalid argument")
            || output_lower.contains("invalid option")
            || output_lower.contains("unrecognized")
        {
            return Self::InvalidArguments;
        }

        // Check exit codes
        match exit_code {
            126 => Self::PermissionDenied,
            127 => Self::MissingDependency,  // Command not found
            128..=255 => Self::RuntimeError, // Signal-based termination
            _ => Self::Unknown,
        }
    }

    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::SyntaxError
                | Self::InvalidArguments
                | Self::NotFound
                | Self::MissingDependency
                | Self::Timeout
                | Self::NetworkError
        )
    }

    /// Get suggested action for this error
    pub fn suggested_action(&self) -> &'static str {
        match self {
            Self::SyntaxError => "Fix the syntax error and retry",
            Self::NotFound => "Check the path or create the missing resource",
            Self::PermissionDenied => "Check permissions or use elevated privileges",
            Self::NetworkError => "Check network connectivity and retry",
            Self::Timeout => "Increase timeout or simplify the operation",
            Self::InvalidArguments => "Review and correct the arguments",
            Self::AlreadyExists => "Use a different name or remove existing resource",
            Self::MissingDependency => "Install the missing dependency first",
            Self::RuntimeError => "Debug the runtime error and fix the code",
            Self::Unknown => "Analyze the error output for more details",
        }
    }
}

// ============================================================================
// Execution Strategy
// ============================================================================

/// Execution strategy for agent tasks
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStrategy {
    /// Direct execution without planning (for simple tasks)
    #[default]
    Direct,
    /// Plan then execute (for medium complexity tasks)
    Planned,
    /// Iterative refinement with reflection (for complex tasks)
    Iterative,
    /// Parallel execution of independent steps (for very complex tasks)
    Parallel,
}

impl ExecutionStrategy {
    /// Get description of this strategy
    pub fn description(&self) -> &'static str {
        match self {
            Self::Direct => "Execute directly without planning",
            Self::Planned => "Create a plan then execute step by step",
            Self::Iterative => "Execute with reflection and iterative refinement",
            Self::Parallel => "Execute independent steps in parallel",
        }
    }

    /// Check if this strategy requires planning
    pub fn requires_planning(&self) -> bool {
        !matches!(self, Self::Direct)
    }

    /// Check if this strategy uses reflection
    pub fn uses_reflection(&self) -> bool {
        matches!(self, Self::Iterative | Self::Parallel)
    }
}

// ============================================================================
// Strategy Selector
// ============================================================================

/// Strategy selector for choosing execution approach
#[derive(Debug, Clone)]
pub struct StrategySelector {
    /// Minimum complexity for planned execution
    pub planned_threshold: Complexity,
    /// Minimum complexity for iterative execution
    pub iterative_threshold: Complexity,
    /// Minimum complexity for parallel execution
    pub parallel_threshold: Complexity,
    /// Force a specific strategy (overrides automatic selection)
    pub forced_strategy: Option<ExecutionStrategy>,
}

impl StrategySelector {
    /// Create a new strategy selector with default thresholds
    pub fn new() -> Self {
        Self {
            planned_threshold: Complexity::Medium,
            iterative_threshold: Complexity::Complex,
            parallel_threshold: Complexity::VeryComplex,
            forced_strategy: None,
        }
    }

    /// Force a specific strategy
    pub fn with_forced_strategy(mut self, strategy: ExecutionStrategy) -> Self {
        self.forced_strategy = Some(strategy);
        self
    }

    /// Set planned threshold
    pub fn with_planned_threshold(mut self, threshold: Complexity) -> Self {
        self.planned_threshold = threshold;
        self
    }

    /// Set iterative threshold
    pub fn with_iterative_threshold(mut self, threshold: Complexity) -> Self {
        self.iterative_threshold = threshold;
        self
    }

    /// Select strategy based on complexity
    pub fn select(&self, complexity: Complexity) -> ExecutionStrategy {
        // Check for forced strategy
        if let Some(strategy) = self.forced_strategy {
            return strategy;
        }

        // Select based on complexity thresholds
        if complexity >= self.parallel_threshold {
            ExecutionStrategy::Parallel
        } else if complexity >= self.iterative_threshold {
            ExecutionStrategy::Iterative
        } else if complexity >= self.planned_threshold {
            ExecutionStrategy::Planned
        } else {
            ExecutionStrategy::Direct
        }
    }

    /// Select strategy based on prompt analysis
    pub fn select_from_prompt(&self, prompt: &str, complexity: Complexity) -> ExecutionStrategy {
        // Check for forced strategy
        if let Some(strategy) = self.forced_strategy {
            return strategy;
        }

        let prompt_lower = prompt.to_lowercase();

        // Check for keywords that suggest specific strategies
        if prompt_lower.contains("step by step")
            || prompt_lower.contains("carefully")
            || prompt_lower.contains("plan")
        {
            return ExecutionStrategy::Planned;
        }

        if prompt_lower.contains("iterate")
            || prompt_lower.contains("refine")
            || prompt_lower.contains("improve")
        {
            return ExecutionStrategy::Iterative;
        }

        if prompt_lower.contains("parallel")
            || prompt_lower.contains("simultaneously")
            || prompt_lower.contains("at the same time")
        {
            return ExecutionStrategy::Parallel;
        }

        // Fall back to complexity-based selection
        self.select(complexity)
    }
}

impl Default for StrategySelector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Retry Policy
// ============================================================================

/// Retry policy for failed operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Current retry count
    pub current_retries: u32,
    /// Delay between retries in milliseconds
    pub retry_delay_ms: u64,
    /// Exponential backoff multiplier
    pub backoff_multiplier: f32,
    /// Error categories that should be retried
    pub retryable_errors: Vec<ErrorCategory>,
}

impl RetryPolicy {
    /// Create a new retry policy
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            current_retries: 0,
            retry_delay_ms: 1000,
            backoff_multiplier: 2.0,
            retryable_errors: vec![
                ErrorCategory::NetworkError,
                ErrorCategory::Timeout,
                ErrorCategory::RuntimeError,
            ],
        }
    }

    /// Check if we should retry
    pub fn should_retry(&self, error_category: Option<ErrorCategory>) -> bool {
        if self.current_retries >= self.max_retries {
            return false;
        }

        match error_category {
            Some(category) => {
                self.retryable_errors.contains(&category) || category.is_recoverable()
            }
            None => true, // Retry unknown errors
        }
    }

    /// Get the delay for the next retry
    pub fn next_delay(&self) -> u64 {
        let multiplier = self.backoff_multiplier.powi(self.current_retries as i32);
        (self.retry_delay_ms as f32 * multiplier) as u64
    }

    /// Increment retry count
    pub fn increment(&mut self) {
        self.current_retries += 1;
    }

    /// Reset retry count
    pub fn reset(&mut self) {
        self.current_retries = 0;
    }

    /// Check if retries are exhausted
    pub fn is_exhausted(&self) -> bool {
        self.current_retries >= self.max_retries
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(3)
    }
}

// ============================================================================
// Reflection Config
// ============================================================================

/// Configuration for reflection behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Enable reflection after tool execution
    pub enabled: bool,
    /// Reflect only on failures
    pub only_on_failure: bool,
    /// Minimum confidence threshold to skip reflection
    pub confidence_threshold: f32,
    /// Maximum reflections per turn
    pub max_reflections_per_turn: usize,
    /// Retry policy for failed operations
    pub retry_policy: RetryPolicy,
}

impl ReflectionConfig {
    /// Create a new reflection config
    pub fn new() -> Self {
        Self {
            enabled: true,
            only_on_failure: false,
            confidence_threshold: 0.8,
            max_reflections_per_turn: 5,
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Enable reflection
    pub fn enabled(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Disable reflection
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Only reflect on failures
    pub fn only_failures(mut self) -> Self {
        self.only_on_failure = true;
        self
    }

    /// Set confidence threshold
    pub fn with_confidence_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set retry policy
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_reflection_success() {
        let reflection = ToolReflection::success()
            .with_insight("Command executed successfully")
            .with_confidence(0.95);

        assert!(reflection.success);
        assert!(!reflection.should_retry);
        assert_eq!(reflection.insights.len(), 1);
        assert_eq!(reflection.confidence, 0.95);
    }

    #[test]
    fn test_tool_reflection_failure() {
        let reflection = ToolReflection::failure()
            .with_insight("File not found")
            .with_alternative("Create the file first")
            .with_error_category(ErrorCategory::NotFound);

        assert!(!reflection.success);
        assert!(reflection.should_retry);
        assert!(reflection.alternative.is_some());
        assert_eq!(reflection.error_category, Some(ErrorCategory::NotFound));
    }

    #[test]
    fn test_error_category_from_output() {
        assert_eq!(
            ErrorCategory::from_output(1, "Permission denied"),
            ErrorCategory::PermissionDenied
        );
        assert_eq!(
            ErrorCategory::from_output(1, "No such file or directory"),
            ErrorCategory::NotFound
        );
        assert_eq!(
            ErrorCategory::from_output(127, "command not found"),
            ErrorCategory::MissingDependency
        );
        assert_eq!(
            ErrorCategory::from_output(1, "syntax error near unexpected token"),
            ErrorCategory::SyntaxError
        );
    }

    #[test]
    fn test_error_category_recoverable() {
        assert!(ErrorCategory::SyntaxError.is_recoverable());
        assert!(ErrorCategory::NotFound.is_recoverable());
        assert!(ErrorCategory::NetworkError.is_recoverable());
        assert!(!ErrorCategory::PermissionDenied.is_recoverable());
    }

    #[test]
    fn test_execution_strategy() {
        assert!(!ExecutionStrategy::Direct.requires_planning());
        assert!(ExecutionStrategy::Planned.requires_planning());
        assert!(ExecutionStrategy::Iterative.uses_reflection());
        assert!(!ExecutionStrategy::Planned.uses_reflection());
    }

    #[test]
    fn test_strategy_selector() {
        let selector = StrategySelector::new();

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
    }

    #[test]
    fn test_strategy_selector_forced() {
        let selector = StrategySelector::new().with_forced_strategy(ExecutionStrategy::Iterative);

        // Should always return forced strategy regardless of complexity
        assert_eq!(
            selector.select(Complexity::Simple),
            ExecutionStrategy::Iterative
        );
        assert_eq!(
            selector.select(Complexity::VeryComplex),
            ExecutionStrategy::Iterative
        );
    }

    #[test]
    fn test_strategy_selector_from_prompt() {
        let selector = StrategySelector::new();

        assert_eq!(
            selector.select_from_prompt("Do this step by step", Complexity::Simple),
            ExecutionStrategy::Planned
        );
        assert_eq!(
            selector.select_from_prompt("Iterate and refine", Complexity::Simple),
            ExecutionStrategy::Iterative
        );
        assert_eq!(
            selector.select_from_prompt("Run in parallel", Complexity::Simple),
            ExecutionStrategy::Parallel
        );
    }

    #[test]
    fn test_retry_policy() {
        let mut policy = RetryPolicy::new(3);

        assert!(policy.should_retry(Some(ErrorCategory::NetworkError)));
        assert!(!policy.is_exhausted());

        policy.increment();
        policy.increment();
        policy.increment();

        assert!(policy.is_exhausted());
        assert!(!policy.should_retry(Some(ErrorCategory::NetworkError)));
    }

    #[test]
    fn test_retry_policy_backoff() {
        let policy = RetryPolicy {
            max_retries: 5,
            current_retries: 0,
            retry_delay_ms: 1000,
            backoff_multiplier: 2.0,
            retryable_errors: vec![],
        };

        assert_eq!(policy.next_delay(), 1000);

        let mut policy = policy;
        policy.increment();
        assert_eq!(policy.next_delay(), 2000);

        policy.increment();
        assert_eq!(policy.next_delay(), 4000);
    }

    #[test]
    fn test_reflection_config() {
        let config = ReflectionConfig::new()
            .enabled()
            .only_failures()
            .with_confidence_threshold(0.9);

        assert!(config.enabled);
        assert!(config.only_on_failure);
        assert_eq!(config.confidence_threshold, 0.9);
    }
}
