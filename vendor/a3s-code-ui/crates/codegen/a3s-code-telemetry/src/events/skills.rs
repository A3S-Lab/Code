//! Skill lifecycle product telemetry events.

use serde::Serialize;

#[derive(Serialize)]
pub struct SkillAdded {
    pub added_count: u32,
    pub total_skills: u32,
    pub success: bool,
}

#[derive(Serialize)]
pub struct SkillRemoved {
    pub success: bool,
}

#[derive(Serialize, Clone, Copy, strum::AsRefStr, strum::IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SkillTrigger {
    /// The user ran `/skill-name`, at turn start or mid-turn.
    SlashCommand,
    /// The model read the skill's `SKILL.md` with `read_file`.
    SkillMdRead,
    /// The model called the skill tool, which only vendor-compat toolsets register.
    SkillTool,
}

#[derive(Serialize)]
pub struct SkillDispatched {
    pub skill_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_source: Option<String>,
    pub trigger: SkillTrigger,
    /// None = skill-tool unclassified; omit rather than invent a source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_source: Option<String>,
    /// The validated frontmatter `origin` slug (the tool that wrote the skill). None = hand-written, or no frontmatter in hand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_origin: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSurfaceKind {
    Skill,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessChangeOp {
    Added,
    Removed,
}

/// One item of the user's harness changed; emitted once per item alongside the count-only `skill_added` / `skill_removed`.
#[derive(Serialize)]
pub struct HarnessChanged {
    pub kind: HarnessSurfaceKind,
    pub op: HarnessChangeOp,
    pub name: String,
    /// Where the skill is loaded from, same vocabulary as `SkillDispatched::skill_source`; independent of `origin`.
    pub skill_source: String,
    /// The validated frontmatter `origin` slug. None = hand-written or unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_source: Option<String>,
    pub success: bool,
}
