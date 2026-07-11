// Prompt Registry
//
// Central registry for all system prompts and prompt templates used in A3S Code.
// Every LLM-facing prompt is externalized here as a compile-time `include_str!`
// so the full agentic design is visible in one place.
//
// Directory layout:
//   prompts/
//   ├── common/   — shared runtime prompts used by multiple subsystems
//   ├── analysis/ — intent classification and pre-analysis prompts
//   ├── planning/ — planner and plan execution prompts
//   └── agents/   — built-in delegated-agent role prompts

// ============================================================================
// Default System Prompt
// ============================================================================

use crate::llm::LlmClient;
use anyhow::Context;

/// Default agentic system prompt — injected when no system prompt is configured.
///
/// Instructs the LLM to behave as an autonomous coding agent: use tools to act,
/// verify results, and keep working until the task is fully complete.
pub const SYSTEM_DEFAULT: &str = include_str!("../prompts/common/system_default.md");

/// Continuation message — injected as a user turn when the LLM stops without
/// completing the task (i.e. stops calling tools mid-task).
pub const CONTINUATION: &str = include_str!("../prompts/common/continuation.md");

/// Safety boundaries (injection hygiene, secret handling, malicious-code refusal).
///
/// Single source of truth, appended to every assembled system prompt by
/// [`SystemPromptSlots::build_with_style`] so it applies uniformly across all
/// agent styles and delegated subagents (which build through the same path).
pub const BOUNDARIES: &str = include_str!("../prompts/common/boundaries.md");

// ============================================================================
// Delegated Run Prompts
// ============================================================================

/// Explore delegated run — read-only codebase exploration
pub const AGENT_EXPLORE: &str = include_str!("../prompts/agents/explore.md");

/// Plan delegated run — read-only planning and analysis
pub const AGENT_PLAN: &str = include_str!("../prompts/agents/plan.md");

/// Code review delegated run — issue finding and review focus
pub const AGENT_CODE_REVIEW: &str = include_str!("../prompts/agents/code_review.md");

// ============================================================================
// Session — Context Compaction
// ============================================================================

/// User template for context compaction. Placeholder: `{conversation}`
pub const CONTEXT_COMPACT: &str = include_str!("../prompts/common/context_compact.md");

/// Prefix for compacted summary messages
pub const CONTEXT_SUMMARY_PREFIX: &str =
    include_str!("../prompts/common/context_summary_prefix.md");

// ============================================================================
// LLM Planner — JSON-structured prompts
// ============================================================================

/// System prompt for LLM planner: plan creation (JSON output)
pub const LLM_PLAN_SYSTEM: &str = include_str!("../prompts/planning/llm_plan_system.md");

/// System prompt for LLM planner: goal extraction (JSON output)
pub const LLM_GOAL_EXTRACT_SYSTEM: &str =
    include_str!("../prompts/planning/llm_goal_extract_system.md");

/// System prompt for LLM planner: goal achievement check (JSON output)
pub const LLM_GOAL_CHECK_SYSTEM: &str =
    include_str!("../prompts/planning/llm_goal_check_system.md");

/// System prompt for pre-analysis: combined intent + goal + plan + input optimization.
pub const PRE_ANALYSIS_SYSTEM: &str = include_str!("../prompts/analysis/pre_analysis_system.md");

// ============================================================================
// Plan Execution Templates
// ============================================================================

/// Template for initial plan execution message
pub const PLAN_EXECUTE_GOAL: &str = include_str!("../prompts/planning/plan_execute_goal.md");

/// Template for per-step execution prompt
pub const PLAN_EXECUTE_STEP: &str = include_str!("../prompts/planning/plan_execute_step.md");

/// Template for fallback plan step description
pub const PLAN_FALLBACK_STEP: &str = include_str!("../prompts/planning/plan_fallback_step.md");

/// Skill catalog header injected before listing available skill names/descriptions.
pub const SKILLS_CATALOG_HEADER: &str = include_str!("../prompts/common/skills_catalog_header.md");

// ============================================================================
// Verification Agent
// ============================================================================

/// Verification agent — adversarial specialist that tries to break code
pub const AGENT_VERIFICATION: &str = include_str!("../prompts/agents/verification.md");

// ============================================================================
// Intent Classification
// ============================================================================

/// System prompt for LLM-based intent classification
pub const INTENT_CLASSIFY_SYSTEM: &str =
    include_str!("../prompts/analysis/intent_classify_system.md");

// ============================================================================
// Planning Mode (Auto-Detection)
// ============================================================================

use serde::{Deserialize, Serialize};

/// Planning mode — controls when planning phase is used.
///
/// When set to `Auto` (the default), the system detects from the user's
/// message whether planning should be enabled. When explicitly `Enabled`,
/// planning runs on every execution. When `Disabled`, planning is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlanningMode {
    /// Automatically detect from message content — enables planning when the
    /// message benefits from structured pre-analysis. Local keyword detection is
    /// only a fallback when pre-analysis is unavailable.
    #[default]
    Auto,
    /// Explicitly disabled — never use planning phase.
    Disabled,
    /// Explicitly enabled — always use planning phase.
    Enabled,
}

impl PlanningMode {
    /// Returns true for the local no-LLM fallback path.
    ///
    /// Normal agent execution runs pre-analysis in `Auto` mode and uses its
    /// structured `requires_planning` decision instead of this heuristic.
    pub fn should_plan(&self, message: &str) -> bool {
        match self {
            PlanningMode::Auto => AgentStyle::detect_from_message(message).requires_planning(),
            PlanningMode::Enabled => true,
            PlanningMode::Disabled => false,
        }
    }
}

// ============================================================================
// Agent Style (Intent-Based Prompt Selection)
// ============================================================================

/// Agent style — determines which system prompt template is used.
///
/// Each style has a different focus and behavior, selected based on the user's
/// apparent intent from their message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStyle {
    /// Default — general purpose coding agent for research and multi-step tasks.
    #[default]
    GeneralPurpose,
    /// Read-only planning and architecture analysis.
    /// Prohibited from modifying files, focuses on design and planning.
    Plan,
    /// Adversarial verification specialist — tries to break code, not confirm it works.
    Verification,
    /// Fast file search and codebase exploration.
    /// Read-only, optimized for finding files and patterns quickly.
    Explore,
    /// Code review focused — analyzes code quality, best practices, potential issues.
    CodeReview,
}

/// Detection confidence level for style detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionConfidence {
    /// High confidence — very specific keywords, skip LLM classification.
    High,
    /// Medium confidence — some indicators present, LLM classification helpful.
    Medium,
    /// Low confidence — no clear indicators, LLM classification recommended.
    Low,
}

impl AgentStyle {
    /// Returns the base system prompt for this style.
    pub fn base_prompt(&self) -> &'static str {
        match self {
            AgentStyle::GeneralPurpose => SYSTEM_DEFAULT,
            AgentStyle::Plan => AGENT_PLAN,
            AgentStyle::Verification => AGENT_VERIFICATION,
            AgentStyle::Explore => AGENT_EXPLORE,
            AgentStyle::CodeReview => AGENT_CODE_REVIEW,
        }
    }

    /// Returns style-specific guidelines if any.
    pub fn guidelines(&self) -> Option<&'static str> {
        match self {
            AgentStyle::GeneralPurpose => None,
            AgentStyle::Plan => None, // Already embedded in agents/plan.md
            AgentStyle::Verification => None, // Already embedded in agents/verification.md
            AgentStyle::Explore => None, // Already embedded in agents/explore.md
            AgentStyle::CodeReview => None,
        }
    }

    /// Returns a one-line description of this style.
    pub fn description(&self) -> &'static str {
        match self {
            AgentStyle::GeneralPurpose => {
                "General purpose coding agent for research and multi-step tasks"
            }
            AgentStyle::Plan => "Read-only planning and architecture analysis agent",
            AgentStyle::Verification => "Adversarial verification specialist — tries to break code",
            AgentStyle::Explore => "Fast read-only file search and codebase exploration agent",
            AgentStyle::CodeReview => "Code review focused — analyzes quality and best practices",
        }
    }

    /// Returns the canonical built-in delegated-agent name for this style.
    pub fn builtin_agent_name(&self) -> &'static str {
        match self {
            AgentStyle::GeneralPurpose => "general",
            AgentStyle::Plan => "plan",
            AgentStyle::Verification => "verification",
            AgentStyle::Explore => "explore",
            AgentStyle::CodeReview => "review",
        }
    }

    /// Returns the stable runtime mode label for UI/event consumers.
    pub fn runtime_mode(&self) -> &'static str {
        match self {
            AgentStyle::GeneralPurpose => "general",
            AgentStyle::Plan => "planning",
            AgentStyle::Verification => "verification",
            AgentStyle::Explore => "explore",
            AgentStyle::CodeReview => "code_review",
        }
    }

    /// Returns true if this style benefits from a planning phase.
    ///
    /// Planning is beneficial for styles that involve multi-step execution
    /// or where a structured approach improves outcomes.
    pub fn requires_planning(&self) -> bool {
        matches!(self, AgentStyle::Plan)
    }

    /// Detects the most appropriate agent style based on user message content,
    /// along with a confidence level.
    ///
    /// This is a local fallback for environments where LLM pre-analysis is
    /// unavailable. Normal execution uses structured pre-analysis first.
    pub fn detect_with_confidence(message: &str) -> (Self, DetectionConfidence) {
        // Chinese text has high ambiguity in intent classification due to
        // compound verb structures and context-dependent meaning.
        // Bypass keyword matching entirely and route to LLM classification.
        if message
            .chars()
            .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        {
            return (AgentStyle::GeneralPurpose, DetectionConfidence::Low);
        }

        let lower = message.to_lowercase();

        // === HIGH CONFIDENCE: Very specific patterns ===

        // Strong verification indicators
        if lower.contains("try to break")
            || lower.contains("find vulnerabilities")
            || lower.contains("adversarial")
            || lower.contains("security audit")
        {
            return (AgentStyle::Verification, DetectionConfidence::High);
        }

        // Strong plan indicators
        if lower.contains("help me plan")
            || lower.contains("help me design")
            || lower.contains("create a plan")
            || lower.contains("implementation plan")
            || lower.contains("step-by-step plan")
        {
            return (AgentStyle::Plan, DetectionConfidence::High);
        }

        // Strong exploration indicators
        if lower.contains("find all files")
            || lower.contains("search for all")
            || lower.contains("locate all")
        {
            return (AgentStyle::Explore, DetectionConfidence::High);
        }

        // === MEDIUM CONFIDENCE: Specific but less definitive ===

        // Verification keywords
        if lower.contains("verify")
            || lower.contains("verification")
            || lower.contains("break")
            || lower.contains("debug")
            || lower.contains("test")
            || lower.contains("check if")
        {
            return (AgentStyle::Verification, DetectionConfidence::Medium);
        }

        // Plan keywords
        if lower.contains("plan")
            || lower.contains("design")
            || lower.contains("architecture")
            || lower.contains("approach")
        {
            return (AgentStyle::Plan, DetectionConfidence::Medium);
        }

        // Explore keywords
        if lower.contains("find")
            || lower.contains("search")
            || lower.contains("where is")
            || lower.contains("where's")
            || lower.contains("locate")
            || lower.contains("explore")
            || lower.contains("look for")
        {
            return (AgentStyle::Explore, DetectionConfidence::Medium);
        }

        // Code review keywords
        if lower.contains("review")
            || lower.contains("code review")
            || lower.contains("analyze")
            || lower.contains("assess")
            || lower.contains("quality")
            || lower.contains("best practice")
        {
            return (AgentStyle::CodeReview, DetectionConfidence::Medium);
        }

        // No clear indicators
        (AgentStyle::GeneralPurpose, DetectionConfidence::Low)
    }

    /// Detects the most appropriate agent style based on user message content.
    ///
    /// This is a local fallback heuristic. Normal execution uses structured
    /// pre-analysis first; users can also explicitly set the style via
    /// `SystemPromptSlots::with_style()`.
    pub fn detect_from_message(message: &str) -> Self {
        Self::detect_with_confidence(message).0
    }

    /// Classifies user intent using LLM when keyword confidence is low.
    ///
    /// This helper is available to callers that want explicit one-shot intent
    /// classification outside the main pre-analysis path.
    ///
    /// Uses a lightweight classification prompt that returns a single word.
    pub async fn detect_with_llm(llm: &dyn LlmClient, message: &str) -> anyhow::Result<Self> {
        use crate::llm::Message;

        let system = INTENT_CLASSIFY_SYSTEM;
        let messages = vec![Message::user(message)];

        let response = llm
            .complete(&messages, Some(system), &[])
            .await
            .context("LLM intent classification failed")?;

        let text = response.text().trim().to_lowercase();

        let style = match text.as_str() {
            "plan" => AgentStyle::Plan,
            "explore" => AgentStyle::Explore,
            "verification" => AgentStyle::Verification,
            "codereview" | "code review" => AgentStyle::CodeReview,
            _ => AgentStyle::GeneralPurpose,
        };

        Ok(style)
    }
}

// ============================================================================
// System Prompt Slots
// ============================================================================

/// Slot-based system prompt customization with intent-based style selection.
///
/// Users can customize specific parts of the system prompt without overriding
/// the core agentic capabilities (tool usage, autonomous behavior, completion
/// criteria). The default agentic core is ALWAYS included.
///
/// ## Assembly Order
///
/// ```text
/// [role]            ← Custom identity/role (e.g. "You are a Python expert")
/// [CORE]            ← Always present: Core Behaviour + Tool Usage Strategy + Completion Criteria
/// [guidelines]      ← Custom coding rules / constraints
/// [response_style]  ← Custom response format (replaces default Response Format section)
/// [extra]           ← Freeform additional instructions
/// ```
///
/// ## Intent-Based Selection
///
/// When `style` is left as `AgentStyle::GeneralPurpose` (the default), the
/// system will attempt to detect the user's intent from their first message and
/// automatically select an appropriate style. To override this behavior, explicitly
/// set the `style` field.
#[derive(Debug, Clone, Default)]
pub struct SystemPromptSlots {
    /// Agent style — determines which base prompt template is used.
    ///
    /// When `None` (default), the style is auto-detected from the user's message.
    /// Explicitly set this to force a particular style regardless of message content.
    pub style: Option<AgentStyle>,

    /// Custom role/identity prepended before the core prompt.
    ///
    /// Example: "You are a senior Python developer specializing in FastAPI."
    /// When set, replaces the default "You are A3S Code, an expert AI coding agent" line.
    pub role: Option<String>,

    /// Custom coding guidelines appended after the core prompt sections.
    ///
    /// Example: "Always use type hints. Follow PEP 8. Prefer dataclasses over dicts."
    pub guidelines: Option<String>,

    /// Custom response style that replaces the default "Response Format" section.
    ///
    /// When `None`, the default response format is used.
    pub response_style: Option<String>,

    /// Freeform extra instructions appended at the very end.
    pub extra: Option<String>,
}

/// The default role line in SYSTEM_DEFAULT that gets replaced when `role` slot is set.
const DEFAULT_ROLE_LINE: &str = include_str!("../prompts/common/system_default_role_line.md");

/// The default response format section.
const DEFAULT_RESPONSE_FORMAT: &str =
    include_str!("../prompts/common/system_default_response_format.md");

impl SystemPromptSlots {
    /// Build the final system prompt by assembling slots around the core prompt.
    ///
    /// The core agentic behavior (Core Behaviour, Tool Usage Strategy, Completion
    /// Criteria) is always preserved. Users can only customize the edges.
    ///
    /// Note: This uses `AgentStyle::GeneralPurpose` as the base. Use
    /// `build_with_message()` to enable automatic intent-based style detection.
    pub fn build(&self) -> String {
        self.build_with_style(self.style.unwrap_or_default())
    }

    /// Build the final system prompt, auto-detecting style from the initial message.
    ///
    /// If `self.style` is explicitly set, that style is used regardless of message content.
    /// Otherwise, the style is detected from `initial_message` using keyword analysis.
    pub fn build_with_message(&self, initial_message: &str) -> String {
        let style = self
            .style
            .unwrap_or_else(|| AgentStyle::detect_from_message(initial_message));
        self.build_with_style(style)
    }

    /// Build the prompt with an explicitly specified style.
    fn build_with_style(&self, style: AgentStyle) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Normalize line endings: strip \r so string matching works on Windows
        // where include_str! may produce \r\n if the file has CRLF endings.
        let base_prompt = style.base_prompt().replace('\r', "");
        let default_role_line = DEFAULT_ROLE_LINE.replace('\r', "");
        let default_response_format = DEFAULT_RESPONSE_FORMAT.replace('\r', "");

        // 1. Role: for GeneralPurpose, replace the default role line.
        // For other styles (Plan, Explore, Verification), prepend custom role since
        // those prompts have their own identity embedded.
        let core = if let Some(ref role) = self.role {
            if style == AgentStyle::GeneralPurpose {
                let custom_role = format!(
                    "{}. You operate in an agentic loop: inspect, act with tools, observe results, and continue until the user's request is genuinely complete.",
                    role.trim_end_matches('.')
                );
                base_prompt.replace(&default_role_line, &custom_role)
            } else {
                // Prepend custom role for other styles
                format!("{}\n\n{}", role, base_prompt)
            }
        } else {
            base_prompt
        };

        // 2. Core: strip the default response format section if custom one is provided
        let core = if self.response_style.is_some() {
            core.replace(&default_response_format, "")
                .trim_end()
                .to_string()
        } else {
            core.trim_end().to_string()
        };

        parts.push(core);

        // 2b. Safety boundaries — single source of truth, appended uniformly so
        // every style and delegated subagent (which build through this path)
        // carries injection-hygiene, secret-handling, and malware-refusal rules.
        parts.push(BOUNDARIES.replace('\r', "").trim_end().to_string());

        // 3. Custom response style (replaces default Response Format)
        if let Some(ref style) = self.response_style {
            parts.push(format!("## Response Format\n\n{}", style));
        }

        // 4. Guidelines: style-specific + custom
        let style_guidelines = style.guidelines();
        if style_guidelines.is_some() || self.guidelines.is_some() {
            let mut guidelines_parts = Vec::new();
            if let Some(sg) = style_guidelines {
                guidelines_parts.push(sg.to_string());
            }
            if let Some(ref g) = self.guidelines {
                guidelines_parts.push(g.clone());
            }
            parts.push(format!(
                "## Guidelines\n\n{}",
                guidelines_parts.join("\n\n")
            ));
        }

        // 5. Extra freeform instructions.
        if let Some(ref extra) = self.extra {
            parts.push(extra.clone());
        }

        parts.join("\n\n")
    }

    /// Returns true if all slots are empty (use pure default prompt).
    pub fn is_empty(&self) -> bool {
        self.style.is_none()
            && self.role.is_none()
            && self.guidelines.is_none()
            && self.response_style.is_none()
            && self.extra.is_none()
    }

    /// Set the agent style explicitly.
    pub fn with_style(mut self, style: AgentStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Set the role/identity.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Set custom guidelines.
    pub fn with_guidelines(mut self, guidelines: impl Into<String>) -> Self {
        self.guidelines = Some(guidelines.into());
        self
    }

    /// Set custom response style.
    pub fn with_response_style(mut self, style: impl Into<String>) -> Self {
        self.response_style = Some(style.into());
        self
    }

    /// Set extra instructions.
    pub fn with_extra(mut self, extra: impl Into<String>) -> Self {
        self.extra = Some(extra.into());
        self
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Render a template by replacing `{key}` placeholders with values
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{}}}", key), value);
    }
    result
}

#[cfg(test)]
#[path = "prompts/tests.rs"]
mod tests;
