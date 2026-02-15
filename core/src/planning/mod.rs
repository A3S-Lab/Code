// Planning and Goal Tracking Structures for Phase 1
//
// This file contains the new types and structures for agentic loop enhancements.
// These will be integrated into agent.rs.

pub mod llm_planner;

pub use llm_planner::{AchievementResult, LlmPlanner};

use serde::{Deserialize, Serialize};

// ============================================================================
// Planning Structures
// ============================================================================

/// Task complexity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Complexity {
    /// Simple task (1-2 steps)
    Simple,
    /// Medium complexity (3-5 steps)
    Medium,
    /// Complex task (6-10 steps)
    Complex,
    /// Very complex task (10+ steps)
    VeryComplex,
}

/// Step status in execution plan
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Step is pending execution
    Pending,
    /// Step is currently being executed
    InProgress,
    /// Step completed successfully
    Completed,
    /// Step failed
    Failed,
    /// Step was skipped
    Skipped,
}

/// A single step in an execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Unique step identifier
    pub id: String,
    /// Step description
    pub description: String,
    /// Tool to use (if any)
    pub tool: Option<String>,
    /// IDs of steps that must complete before this one
    pub dependencies: Vec<String>,
    /// Current status
    pub status: StepStatus,
    /// Expected output or success criteria
    pub success_criteria: Option<String>,
}

impl PlanStep {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            tool: None,
            dependencies: Vec::new(),
            status: StepStatus::Pending,
            success_criteria: None,
        }
    }

    pub fn with_tool(mut self, tool: impl Into<String>) -> Self {
        self.tool = Some(tool.into());
        self
    }

    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    pub fn with_success_criteria(mut self, criteria: impl Into<String>) -> Self {
        self.success_criteria = Some(criteria.into());
        self
    }
}

/// Execution plan for a task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// High-level goal
    pub goal: String,
    /// Decomposed steps
    pub steps: Vec<PlanStep>,
    /// Estimated complexity
    pub complexity: Complexity,
    /// Required tools
    pub required_tools: Vec<String>,
    /// Estimated total steps
    pub estimated_steps: usize,
}

impl ExecutionPlan {
    pub fn new(goal: impl Into<String>, complexity: Complexity) -> Self {
        Self {
            goal: goal.into(),
            steps: Vec::new(),
            complexity,
            required_tools: Vec::new(),
            estimated_steps: 0,
        }
    }

    pub fn add_step(&mut self, step: PlanStep) {
        self.steps.push(step);
        self.estimated_steps = self.steps.len();
    }

    pub fn add_required_tool(&mut self, tool: impl Into<String>) {
        let tool_str = tool.into();
        if !self.required_tools.contains(&tool_str) {
            self.required_tools.push(tool_str);
        }
    }

    /// Get steps that are ready to execute (dependencies met)
    pub fn get_ready_steps(&self) -> Vec<&PlanStep> {
        self.steps
            .iter()
            .filter(|step| {
                step.status == StepStatus::Pending
                    && step.dependencies.iter().all(|dep_id| {
                        self.steps
                            .iter()
                            .find(|s| &s.id == dep_id)
                            .map(|s| s.status == StepStatus::Completed)
                            .unwrap_or(false)
                    })
            })
            .collect()
    }

    /// Get progress as a fraction (0.0 - 1.0)
    pub fn progress(&self) -> f32 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let completed = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count();
        completed as f32 / self.steps.len() as f32
    }
}

// ============================================================================
// Goal Tracking Structures
// ============================================================================

/// Agent goal with success criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentGoal {
    /// Goal description
    pub description: String,
    /// Success criteria (list of conditions)
    pub success_criteria: Vec<String>,
    /// Current progress (0.0 - 1.0)
    pub progress: f32,
    /// Is goal achieved?
    pub achieved: bool,
    /// Timestamp when goal was created
    pub created_at: i64,
    /// Timestamp when goal was achieved (if achieved)
    pub achieved_at: Option<i64>,
}

impl AgentGoal {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            success_criteria: Vec::new(),
            progress: 0.0,
            achieved: false,
            created_at: chrono::Utc::now().timestamp(),
            achieved_at: None,
        }
    }

    pub fn with_criteria(mut self, criteria: Vec<String>) -> Self {
        self.success_criteria = criteria;
        self
    }

    pub fn update_progress(&mut self, progress: f32) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    pub fn mark_achieved(&mut self) {
        self.achieved = true;
        self.progress = 1.0;
        self.achieved_at = Some(chrono::Utc::now().timestamp());
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_step_creation() {
        let step = PlanStep::new("step-1", "Test step")
            .with_tool("bash")
            .with_dependencies(vec!["step-0".to_string()])
            .with_success_criteria("Command exits with 0");

        assert_eq!(step.id, "step-1");
        assert_eq!(step.description, "Test step");
        assert_eq!(step.tool, Some("bash".to_string()));
        assert_eq!(step.dependencies, vec!["step-0".to_string()]);
        assert_eq!(step.status, StepStatus::Pending);
    }

    #[test]
    fn test_execution_plan() {
        let mut plan = ExecutionPlan::new("Test goal", Complexity::Medium);

        plan.add_step(PlanStep::new("step-1", "First step"));
        plan.add_step(
            PlanStep::new("step-2", "Second step").with_dependencies(vec!["step-1".to_string()]),
        );

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.estimated_steps, 2);
        assert_eq!(plan.progress(), 0.0);

        // Mark first step as completed
        plan.steps[0].status = StepStatus::Completed;
        assert_eq!(plan.progress(), 0.5);

        // Check ready steps
        let ready = plan.get_ready_steps();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "step-2");
    }

    #[test]
    fn test_agent_goal() {
        let mut goal = AgentGoal::new("Complete task")
            .with_criteria(vec!["Criterion 1".to_string(), "Criterion 2".to_string()]);

        assert_eq!(goal.description, "Complete task");
        assert_eq!(goal.success_criteria.len(), 2);
        assert_eq!(goal.progress, 0.0);
        assert!(!goal.achieved);

        goal.update_progress(0.5);
        assert_eq!(goal.progress, 0.5);

        goal.mark_achieved();
        assert!(goal.achieved);
        assert_eq!(goal.progress, 1.0);
        assert!(goal.achieved_at.is_some());
    }

    #[test]
    fn test_complexity_levels() {
        assert_eq!(
            serde_json::to_string(&Complexity::Simple).unwrap(),
            "\"Simple\""
        );
        assert_eq!(
            serde_json::to_string(&Complexity::Complex).unwrap(),
            "\"Complex\""
        );
    }

    #[test]
    fn test_step_status() {
        let status = StepStatus::InProgress;
        assert_eq!(serde_json::to_string(&status).unwrap(), "\"InProgress\"");
    }
}
