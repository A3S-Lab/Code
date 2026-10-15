//! Explicit host policy selection; Core owns admission, rendering and enforcement.

use a3s_code_core::session_review::{ReviewFindingDraft, ReviewScenario, ReviewSubjectV1};
use a3s_code_core::{AgentSession, WorkPolicyV1};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Component, Path};

const INSTRUCTIONS: &str = include_str!("../prompts/engineering.md");

enum EngineeringPolicy {
    V1,
}

impl EngineeringPolicy {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "engineering-v1" => Ok(Self::V1),
            _ => Err(format!(
                "unknown EngineeringPolicy {value:?}; expected engineering-v1"
            )),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::V1 => "engineering-v1",
        }
    }
}

pub(crate) fn policy_from_meta(
    meta: Option<&Map<String, Value>>,
) -> Result<Option<WorkPolicyV1>, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selection {
        policy: String,
    }

    let Some(value) = meta.and_then(|meta| meta.get("a3sEngineering")) else {
        return Ok(None);
    };
    let input =
        Selection::deserialize(value).map_err(|error| format!("a3sEngineering: {error}"))?;
    named_policy(&input.policy).map(Some)
}

/// The engineering policy a host selects by name (`_meta` or a launch flag).
pub(crate) fn named_policy(name: &str) -> Result<WorkPolicyV1, String> {
    let selected = EngineeringPolicy::parse(name)?;
    WorkPolicyV1::new(selected.as_str(), 1, INSTRUCTIONS).map_err(|error| error.to_string())
}

/// Subject kind every admitted engineering draft names; its uri is the target digest.
const REVIEW_TARGET_KIND: &str = "engineering.target";
/// Subject kind a goal-step Finding stores; its uri digests the target and the scope.
const REVIEW_GOAL_STEP_KIND: &str = "engineering.goal-step";
const MAX_TARGET_PATHS: usize = 256;
const MAX_TARGET_BYTES: u64 = 64 * 1024 * 1024;

enum EngineeringScenario {
    Change,
    Workspace,
}

impl EngineeringScenario {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "engineering.change" => Ok(Self::Change),
            "engineering.workspace" => Ok(Self::Workspace),
            _ => Err(format!(
                "unknown EngineeringScenario {value:?}; expected engineering.change or engineering.workspace"
            )),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Change => "engineering.change",
            Self::Workspace => "engineering.workspace",
        }
    }
}

/// Explicit-request only (the original default trigger): registration never
/// starts a review. Pending findings stay visible to the host's injection query.
struct EngineeringReviewScenario(EngineeringScenario);

impl ReviewScenario for EngineeringReviewScenario {
    fn scenario_id(&self) -> &str {
        self.0.as_str()
    }
}

/// The host re-registers its scenarios on every new or reopened policy session;
/// the registry is process-local while Findings persist in the session snapshot.
pub(crate) fn register_scenarios(session: &AgentSession) -> Result<(), String> {
    for scenario in [EngineeringScenario::Change, EngineeringScenario::Workspace] {
        session
            .register_review_scenario(std::sync::Arc::new(EngineeringReviewScenario(scenario)))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// `a3s/review/target`: pin the current bytes of a review target.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewTargetRequest {
    session_id: String,
    paths: Vec<String>,
}

pub(crate) enum ReviewRequest {
    Target(ReviewTargetRequest),
    Admit(ReviewAdmission),
    Settle(ReviewSettlement),
}

impl ReviewRequest {
    pub(crate) fn parse(method: &str, params: &str) -> Result<Self, String> {
        let parsed = match method {
            "a3s/review/target" => serde_json::from_str(params).map(Self::Target),
            "a3s/review/admit" => serde_json::from_str(params).map(Self::Admit),
            "a3s/review/settle" => serde_json::from_str(params).map(Self::Settle),
            _ => return Err(format!("unknown review method {method:?}")),
        };
        parsed.map_err(|error| error.to_string())
    }

    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::Target(request) => &request.session_id,
            Self::Admit(request) => &request.session_id,
            Self::Settle(request) => &request.session_id,
        }
    }

    pub(crate) async fn answer(self, session: &AgentSession) -> Result<Value, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        match self {
            Self::Target(request) => Ok(serde_json::json!({
                "digest": target_digest(session.workspace(), &request.paths).await?
            })),
            Self::Admit(request) => {
                let admitted = admit_review(session, request, now).await?;
                session.save().await.map_err(|error| error.to_string())?;
                Ok(serde_json::json!({ "admitted": admitted }))
            }
            Self::Settle(request) => {
                let status = settle_review(session, request, now)?;
                session.save().await.map_err(|error| error.to_string())?;
                Ok(serde_json::json!({ "status": status }))
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewTarget {
    paths: Vec<String>,
    digest: String,
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ReviewScope {
    SessionTarget,
    GoalStep {
        goal_id: String,
        goal_revision: u32,
        plan_revision: u32,
        step_id: String,
        run_id: String,
    },
}

impl ReviewScope {
    /// The subject a Finding stores for this scope. A session-target subject is
    /// the target alone, as before goal-step scopes were kept; settlement
    /// recomputes it, so a Finding settles only under the scope it was admitted for.
    fn subject(&self, target_digest: &str) -> Result<ReviewSubjectV1, String> {
        let (kind, uri) = match self {
            Self::SessionTarget => (REVIEW_TARGET_KIND, target_digest.to_owned()),
            Self::GoalStep {
                goal_id,
                goal_revision,
                plan_revision,
                step_id,
                run_id,
            } => (
                REVIEW_GOAL_STEP_KIND,
                a3s_code_core::digest_json(
                    "a3s.code.review-scope.v1",
                    &(
                        target_digest,
                        goal_id,
                        goal_revision,
                        plan_revision,
                        step_id,
                        run_id,
                    ),
                )
                .map_err(|error| error.to_string())?,
            ),
        };
        ReviewSubjectV1::opaque(kind, uri).map_err(|error| error.to_string())
    }

    /// A goal-step scope still names the current step and its original Run.
    fn check_current(&self, session: &AgentSession) -> Result<(), String> {
        if let Self::GoalStep {
            goal_id,
            goal_revision,
            plan_revision,
            step_id,
            run_id,
        } = self
        {
            session
                .check_goal_step_run(goal_id, *goal_revision, *plan_revision, step_id, run_id)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// `a3s/review/admit`: the original drafts plus the binding the host verifies.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewAdmission {
    session_id: String,
    scenario_id: String,
    source_review_id: String,
    work_policy_digest: String,
    target: ReviewTarget,
    scope: ReviewScope,
    findings: Vec<ReviewFindingDraft>,
}

/// Digest the current workspace bytes of `paths` (relative, inside the workspace).
async fn target_digest(workspace: &Path, paths: &[String]) -> Result<String, String> {
    if paths.is_empty() || paths.len() > MAX_TARGET_PATHS {
        return Err(format!("review target needs 1..={MAX_TARGET_PATHS} paths"));
    }
    let root = tokio::fs::canonicalize(workspace)
        .await
        .map_err(|error| format!("workspace: {error}"))?;
    let unique: BTreeSet<&String> = paths.iter().collect();
    if unique.len() != paths.len() {
        return Err("review target repeats a path".into());
    }
    let mut total = 0u64;
    let mut files = Vec::with_capacity(unique.len());
    for path in unique {
        let relative = Path::new(path);
        if path.is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(format!(
                "review target path {path:?} is not workspace-relative"
            ));
        }
        let resolved = tokio::fs::canonicalize(root.join(relative))
            .await
            .map_err(|error| format!("review target {path:?}: {error}"))?;
        if !resolved.starts_with(&root) {
            return Err(format!("review target {path:?} leaves the workspace"));
        }
        let bytes = tokio::fs::read(&resolved)
            .await
            .map_err(|error| format!("review target {path:?}: {error}"))?;
        total += bytes.len() as u64;
        if total > MAX_TARGET_BYTES {
            return Err("review target exceeds 64MiB".into());
        }
        files.push((
            path.as_str(),
            a3s_code_core::digest_bytes("a3s.code.review-target-file.v1", &bytes),
        ));
    }
    a3s_code_core::digest_json("a3s.code.review-target.v1", &files).map_err(|e| e.to_string())
}

/// Admit drafts only for this session's current policy, target bytes and,
/// when scoped, the current Goal step and its original Run. No model call,
/// no permission change and no Goal fact: Findings never accept a step.
async fn admit_review(
    session: &AgentSession,
    request: ReviewAdmission,
    observed_at_ms: u64,
) -> Result<usize, String> {
    if session.is_closed() || request.session_id != session.session_id() {
        return Err("review admission names another or a closed session".into());
    }
    let scenario = EngineeringScenario::parse(&request.scenario_id)?;
    if request.findings.is_empty() {
        return Err("review admission has no findings".into());
    }
    let subject = ReviewSubjectV1::opaque(REVIEW_TARGET_KIND, &request.target.digest)
        .map_err(|error| error.to_string())?;
    if request
        .findings
        .iter()
        .any(|draft| draft.subject != subject)
    {
        return Err("every finding must name the bound review target".into());
    }
    let current = session
        .work_policy_digest()
        .map_err(|error| error.to_string())?;
    if current.as_deref() != Some(request.work_policy_digest.as_str()) {
        return Err("review policy binding is missing or stale".into());
    }
    request.scope.check_current(session)?;
    if target_digest(session.workspace(), &request.target.paths).await? != request.target.digest {
        return Err("review target changed since it was reviewed".into());
    }
    let bound = request.scope.subject(&request.target.digest)?;
    let mut findings = request.findings;
    for draft in &mut findings {
        draft.subject = bound.clone();
    }
    session
        .admit_session_review_drafts(
            scenario.as_str(),
            &request.source_review_id,
            observed_at_ms,
            findings,
        )
        .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum SettleAction {
    Address { run_id: String },
    Accept { review_id: String },
    Waive,
}

/// `a3s/review/settle`: move an engineering Finding through the original
/// lifecycle only under the target and scope it was admitted for.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewSettlement {
    session_id: String,
    finding_id: String,
    target_digest: String,
    scope: ReviewScope,
    action: SettleAction,
}

fn settle_review(
    session: &AgentSession,
    request: ReviewSettlement,
    at_ms: u64,
) -> Result<&'static str, String> {
    if session.is_closed() || request.session_id != session.session_id() {
        return Err("review settlement names another or a closed session".into());
    }
    let store = session.session_review_store();
    let finding = store
        .findings
        .iter()
        .find(|finding| finding.finding_id == request.finding_id)
        .ok_or("review finding not found")?;
    EngineeringScenario::parse(&finding.scenario_id)?;
    if finding.subject != request.scope.subject(&request.target_digest)? {
        return Err("review finding was admitted for another target or scope".into());
    }
    request.scope.check_current(session)?;
    let id = &request.finding_id;
    let settled = match request.action {
        SettleAction::Address { run_id } => session
            .mark_session_review_addressed(id, run_id, at_ms)
            .map(|()| "addressed"),
        SettleAction::Accept { review_id } => session
            .accept_session_review_finding(id, review_id, at_ms)
            .map(|()| "accepted"),
        SettleAction::Waive => session
            .waive_session_review_finding(id, at_ms)
            .map(|()| "waived"),
    };
    settled.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
