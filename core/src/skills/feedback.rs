//! Skill Feedback Loop
//!
//! Provides a scoring mechanism for skills based on usage feedback.
//! Skills that consistently perform poorly are automatically disabled
//! from the system prompt (soft-disable, not deleted).
//!
//! ## Extension Point
//!
//! `SkillScorer` is a trait — consumers can replace `DefaultSkillScorer`
//! with a persistent implementation (e.g., database-backed, LLM-evaluated).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;

/// Outcome of a skill usage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillOutcome {
    /// Skill contributed to a successful result
    Success,
    /// Skill did not help or caused failure
    Failure,
    /// Skill partially helped
    Partial,
}

/// Feedback record for a single skill usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFeedback {
    /// Skill name
    pub skill_name: String,
    /// Outcome of the usage
    pub outcome: SkillOutcome,
    /// Score adjustment (-1.0 to 1.0)
    pub score_delta: f32,
    /// Human-readable reason
    pub reason: String,
    /// Timestamp (Unix milliseconds)
    pub timestamp: i64,
}

/// Skill score summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillScore {
    /// Skill name
    pub skill_name: String,
    /// Current weighted score (0.0 to 1.0)
    pub score: f32,
    /// Total feedback count
    pub feedback_count: usize,
    /// Whether the skill is currently disabled
    pub disabled: bool,
}

/// Skill scorer trait (extension point)
///
/// Records feedback and computes scores for skills. Implementations
/// can use any scoring algorithm and persistence strategy.
pub trait SkillScorer: Send + Sync {
    /// Record a feedback entry for a skill
    fn record(&self, feedback: SkillFeedback);

    /// Get the current score for a skill (0.0 to 1.0, default 1.0 for unknown)
    fn score(&self, skill_name: &str) -> f32;

    /// Check if a skill should be disabled based on its score
    fn should_disable(&self, skill_name: &str) -> bool;

    /// Get score summaries for all tracked skills
    fn all_scores(&self) -> Vec<SkillScore>;
}

/// Default skill scorer using sliding window weighted average
pub struct DefaultSkillScorer {
    /// Feedback history per skill
    history: RwLock<HashMap<String, VecDeque<SkillFeedback>>>,
    /// Maximum feedback entries to keep per skill
    pub window_size: usize,
    /// Score threshold below which a skill is disabled
    pub disable_threshold: f32,
    /// Minimum feedback count before disabling is considered
    pub min_feedback_count: usize,
}

impl Default for DefaultSkillScorer {
    fn default() -> Self {
        Self {
            history: RwLock::new(HashMap::new()),
            window_size: 20,
            disable_threshold: 0.3,
            min_feedback_count: 3,
        }
    }
}

impl DefaultSkillScorer {
    /// Create a new scorer with custom parameters
    pub fn new(window_size: usize, disable_threshold: f32, min_feedback_count: usize) -> Self {
        Self {
            history: RwLock::new(HashMap::new()),
            window_size,
            disable_threshold,
            min_feedback_count,
        }
    }

    /// Compute weighted average score for a feedback window.
    /// More recent entries have higher weight (linear decay).
    fn compute_score(entries: &VecDeque<SkillFeedback>) -> f32 {
        if entries.is_empty() {
            return 1.0; // Default: trust unknown skills
        }

        let n = entries.len() as f32;
        let mut weighted_sum = 0.0f32;
        let mut weight_total = 0.0f32;

        for (i, entry) in entries.iter().enumerate() {
            // Linear weight: older entries get lower weight
            let weight = (i as f32 + 1.0) / n;
            // Convert score_delta from [-1, 1] to [0, 1]
            let normalized = (entry.score_delta + 1.0) / 2.0;
            weighted_sum += normalized * weight;
            weight_total += weight;
        }

        if weight_total == 0.0 {
            return 1.0;
        }

        (weighted_sum / weight_total).clamp(0.0, 1.0)
    }
}

impl SkillScorer for DefaultSkillScorer {
    fn record(&self, feedback: SkillFeedback) {
        let mut history = self.history.write().unwrap();
        let entries = history
            .entry(feedback.skill_name.clone())
            .or_insert_with(VecDeque::new);

        entries.push_back(feedback);

        // Trim to window size
        while entries.len() > self.window_size {
            entries.pop_front();
        }
    }

    fn score(&self, skill_name: &str) -> f32 {
        let history = self.history.read().unwrap();
        match history.get(skill_name) {
            Some(entries) => Self::compute_score(entries),
            None => 1.0, // Unknown skill = full trust
        }
    }

    fn should_disable(&self, skill_name: &str) -> bool {
        let history = self.history.read().unwrap();
        match history.get(skill_name) {
            Some(entries) => {
                if entries.len() < self.min_feedback_count {
                    return false; // Not enough data
                }
                Self::compute_score(entries) < self.disable_threshold
            }
            None => false,
        }
    }

    fn all_scores(&self) -> Vec<SkillScore> {
        let history = self.history.read().unwrap();
        history
            .iter()
            .map(|(name, entries)| {
                let score = Self::compute_score(entries);
                SkillScore {
                    skill_name: name.clone(),
                    score,
                    feedback_count: entries.len(),
                    disabled: entries.len() >= self.min_feedback_count
                        && score < self.disable_threshold,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn make_feedback(skill: &str, outcome: SkillOutcome, delta: f32) -> SkillFeedback {
        SkillFeedback {
            skill_name: skill.to_string(),
            outcome,
            score_delta: delta,
            reason: "test".to_string(),
            timestamp: now_ms(),
        }
    }

    // --- Basic scoring ---

    #[test]
    fn test_unknown_skill_score_is_1() {
        let scorer = DefaultSkillScorer::default();
        assert_eq!(scorer.score("nonexistent"), 1.0);
    }

    #[test]
    fn test_all_success_high_score() {
        let scorer = DefaultSkillScorer::default();
        for _ in 0..5 {
            scorer.record(make_feedback("good-skill", SkillOutcome::Success, 1.0));
        }
        let score = scorer.score("good-skill");
        assert!(score > 0.9, "Expected high score, got {}", score);
    }

    #[test]
    fn test_all_failure_low_score() {
        let scorer = DefaultSkillScorer::default();
        for _ in 0..5 {
            scorer.record(make_feedback("bad-skill", SkillOutcome::Failure, -1.0));
        }
        let score = scorer.score("bad-skill");
        assert!(score < 0.1, "Expected low score, got {}", score);
    }

    #[test]
    fn test_mixed_feedback_moderate_score() {
        let scorer = DefaultSkillScorer::default();
        scorer.record(make_feedback("mixed", SkillOutcome::Success, 1.0));
        scorer.record(make_feedback("mixed", SkillOutcome::Failure, -1.0));
        scorer.record(make_feedback("mixed", SkillOutcome::Success, 1.0));
        let score = scorer.score("mixed");
        // Weighted toward recent (last entry is success), should be > 0.5
        assert!(
            score > 0.4 && score < 0.8,
            "Expected moderate score, got {}",
            score
        );
    }

    // --- Disable logic ---

    #[test]
    fn test_should_not_disable_unknown() {
        let scorer = DefaultSkillScorer::default();
        assert!(!scorer.should_disable("unknown"));
    }

    #[test]
    fn test_should_not_disable_insufficient_data() {
        let scorer = DefaultSkillScorer::default();
        // Only 2 entries, min is 3
        scorer.record(make_feedback("new-skill", SkillOutcome::Failure, -1.0));
        scorer.record(make_feedback("new-skill", SkillOutcome::Failure, -1.0));
        assert!(!scorer.should_disable("new-skill"));
    }

    #[test]
    fn test_should_disable_consistently_bad() {
        let scorer = DefaultSkillScorer::default();
        for _ in 0..5 {
            scorer.record(make_feedback("terrible", SkillOutcome::Failure, -1.0));
        }
        assert!(scorer.should_disable("terrible"));
    }

    #[test]
    fn test_should_not_disable_good_skill() {
        let scorer = DefaultSkillScorer::default();
        for _ in 0..5 {
            scorer.record(make_feedback("great", SkillOutcome::Success, 1.0));
        }
        assert!(!scorer.should_disable("great"));
    }

    // --- Window trimming ---

    #[test]
    fn test_window_trimming() {
        let scorer = DefaultSkillScorer::new(5, 0.3, 3);
        // Fill with failures
        for _ in 0..5 {
            scorer.record(make_feedback("recover", SkillOutcome::Failure, -1.0));
        }
        assert!(scorer.should_disable("recover"));

        // Now add successes — old failures should be trimmed
        for _ in 0..5 {
            scorer.record(make_feedback("recover", SkillOutcome::Success, 1.0));
        }
        assert!(!scorer.should_disable("recover"));
        assert!(scorer.score("recover") > 0.9);
    }

    // --- all_scores ---

    #[test]
    fn test_all_scores_empty() {
        let scorer = DefaultSkillScorer::default();
        assert!(scorer.all_scores().is_empty());
    }

    #[test]
    fn test_all_scores_multiple_skills() {
        let scorer = DefaultSkillScorer::default();
        for _ in 0..3 {
            scorer.record(make_feedback("skill-a", SkillOutcome::Success, 1.0));
            scorer.record(make_feedback("skill-b", SkillOutcome::Failure, -1.0));
        }

        let scores = scorer.all_scores();
        assert_eq!(scores.len(), 2);

        let a = scores.iter().find(|s| s.skill_name == "skill-a").unwrap();
        let b = scores.iter().find(|s| s.skill_name == "skill-b").unwrap();

        assert!(a.score > 0.9);
        assert!(!a.disabled);
        assert_eq!(a.feedback_count, 3);

        assert!(b.score < 0.1);
        assert!(b.disabled);
        assert_eq!(b.feedback_count, 3);
    }

    // --- Custom parameters ---

    #[test]
    fn test_custom_threshold() {
        let scorer = DefaultSkillScorer::new(20, 0.8, 3);
        // Partial feedback (delta=0.0 → normalized 0.5)
        for _ in 0..5 {
            scorer.record(make_feedback("mediocre", SkillOutcome::Partial, 0.0));
        }
        // Score ~0.5, threshold 0.8 → should disable
        assert!(scorer.should_disable("mediocre"));
    }

    // --- Outcome serialization ---

    #[test]
    fn test_outcome_serialization() {
        let json = serde_json::to_string(&SkillOutcome::Success).unwrap();
        assert_eq!(json, "\"success\"");

        let parsed: SkillOutcome = serde_json::from_str("\"failure\"").unwrap();
        assert_eq!(parsed, SkillOutcome::Failure);
    }

    // --- SkillFeedback serialization ---

    #[test]
    fn test_feedback_serialization() {
        let fb = make_feedback("test", SkillOutcome::Success, 0.8);
        let json = serde_json::to_string(&fb).unwrap();
        assert!(json.contains("\"skill_name\":\"test\""));
        assert!(json.contains("\"outcome\":\"success\""));

        let parsed: SkillFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.skill_name, "test");
        assert_eq!(parsed.outcome, SkillOutcome::Success);
    }

    // --- compute_score edge cases ---

    #[test]
    fn test_compute_score_empty() {
        let empty = VecDeque::new();
        assert_eq!(DefaultSkillScorer::compute_score(&empty), 1.0);
    }

    #[test]
    fn test_compute_score_single_entry() {
        let mut entries = VecDeque::new();
        entries.push_back(make_feedback("s", SkillOutcome::Success, 1.0));
        let score = DefaultSkillScorer::compute_score(&entries);
        assert!((score - 1.0).abs() < f32::EPSILON);
    }
}
