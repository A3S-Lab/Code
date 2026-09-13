//! Causal completion and mutation observation for the coding loop.
//!
//! A mutating run cannot succeed on assistant prose. Verification reports and
//! host waivers must bind the same effect digest. Diagnostics after a write
//! are an observation, never a pass.

use crate::verification::{VerificationReport, VerificationStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MUTATION_OBSERVATION_SCHEMA: &str = "a3s.code.mutation-observation.v1";
const MUTATING_FILE_TOOLS: &[&str] = &["write", "edit", "patch", "download"];

/// How a successful run closed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompletionTerminal {
    /// No workspace mutation. A final answer is enough.
    #[default]
    Narrative,
    /// Required checks passed and were bound to the mutation digest.
    Verified { effect_digest: String },
    /// A host-confirmed waiver bound to the mutation digest. Not a verification pass.
    Waived { effect_digest: String },
    /// Step closures did not share one effect digest. Not a narrative success
    /// and not a combined digest. Each digest stays on the step that closed it.
    Distinct,
}

impl CompletionTerminal {
    pub fn is_narrative(&self) -> bool {
        matches!(self, Self::Narrative)
    }
}

/// Fold step closures into the session terminal. A bound closure is not
/// rewritten as narrative. Different digests are not hashed together.
pub fn fold_step_completions(terminals: &[CompletionTerminal]) -> CompletionTerminal {
    let mut bound = Vec::new();
    for terminal in terminals {
        if terminal.is_narrative() || bound.contains(terminal) {
            continue;
        }
        bound.push(terminal.clone());
    }
    match bound.as_slice() {
        [] => CompletionTerminal::Narrative,
        [one] => one.clone(),
        many => {
            if let Some(digest) = completion_digest(&many[0]) {
                if many
                    .iter()
                    .all(|terminal| completion_digest(terminal) == Some(digest))
                {
                    if many
                        .iter()
                        .any(|terminal| matches!(terminal, CompletionTerminal::Verified { .. }))
                    {
                        return CompletionTerminal::Verified {
                            effect_digest: digest.to_string(),
                        };
                    }
                    return many[0].clone();
                }
            }
            CompletionTerminal::Distinct
        }
    }
}

fn completion_digest(terminal: &CompletionTerminal) -> Option<&str> {
    match terminal {
        CompletionTerminal::Narrative | CompletionTerminal::Distinct => None,
        CompletionTerminal::Verified { effect_digest }
        | CompletionTerminal::Waived { effect_digest } => Some(effect_digest.as_str()),
    }
}

/// Host- or user-confirmed waiver. The model cannot mint this from prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionWaiverV1 {
    pub effect_digest: String,
    pub reason: String,
}

impl CompletionWaiverV1 {
    pub fn new(effect_digest: impl Into<String>, reason: impl Into<String>) -> Option<Self> {
        let effect_digest = effect_digest.into();
        let reason = reason.into();
        if effect_digest.trim().is_empty() || reason.trim().is_empty() {
            return None;
        }
        Some(Self {
            effect_digest,
            reason,
        })
    }
}

/// Whether this run is an ordinary execution or the admitted exit from plan mode.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlanRunAdmission {
    pub claims_implementation: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
}

impl PlanRunAdmission {
    pub fn ordinary() -> Self {
        Self::default()
    }

    pub fn implementation(plan_digest: impl Into<String>) -> Self {
        Self {
            claims_implementation: true,
            plan_digest: Some(plan_digest.into()),
        }
    }

    /// A claim without a non-empty accepted-plan digest is an ordinary run.
    pub fn label(&self) -> &'static str {
        match (
            self.claims_implementation,
            self.plan_digest.as_deref().map(str::trim),
        ) {
            (true, Some(digest)) if !digest.is_empty() => "plan_implementation",
            _ => "ordinary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationRecord {
    pub tool: String,
    pub path: String,
    pub content_digest: String,
}

/// A background child that shares this workspace and has not been observed yet.
/// Paths land on this ledger when the child settles. This is not a second digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OpenWorkspaceChild {
    task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    porcelain: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    nongit: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationLedger {
    records: Vec<MutationRecord>,
    #[serde(default)]
    digest: String,
    /// Background writers still sharing the workspace. Empty means none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    open_children: Vec<OpenWorkspaceChild>,
    /// The latest workspace re-read failed. Not part of the effect digest:
    /// a partial list is not an identity a waiver can close.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    observation_incomplete: bool,
}

impl MutationLedger {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.records.iter().map(|record| record.path.as_str())
    }

    /// Record a workspace mutation. Ignores reads and tools that did not
    /// publish a path. `changed_paths` means the workspace already differed,
    /// including a failed or timed-out wrapper. Command text is not parsed.
    /// A `file_path` on a failed write is not applied, so it does not count.
    pub fn observe_tool(&mut self, tool: &str, exit_code: i32, metadata: Option<&Value>) {
        let Some(metadata) = metadata else {
            return;
        };
        let tool_key = tool.to_ascii_lowercase();
        if metadata.get("changed_paths").is_some() {
            record_changed_paths(self, &tool_key, metadata);
        }
        if let Some(task_id) = metadata.get("workspace_child").and_then(Value::as_str) {
            self.observe_workspace_child(task_id);
        }
        if exit_code != 0 {
            record_nested_tool_effects(self, metadata);
            return;
        }
        if MUTATING_FILE_TOOLS.contains(&tool_key.as_str()) {
            if let Some(path) = metadata.get("file_path").and_then(Value::as_str) {
                self.push(tool_key, path, content_digest(metadata));
            }
            return;
        }
        record_nested_tool_effects(self, metadata);
    }

    fn observe_workspace_child(&mut self, task_id: &str) {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return;
        }
        if let Some(paths) = crate::porcelain::take_settled_workspace_child(task_id) {
            self.record_child_paths(task_id, &paths);
            return;
        }
        let Some(marker) = crate::porcelain::child_marker(task_id) else {
            return;
        };
        if self
            .open_children
            .iter()
            .any(|child| child.task_id == marker.task_id)
        {
            return;
        }
        self.open_children.push(OpenWorkspaceChild {
            task_id: marker.task_id,
            porcelain: marker.porcelain,
            head: marker.head,
            nongit: marker.nongit,
        });
    }

    pub fn has_open_children(&self) -> bool {
        !self.open_children.is_empty()
    }

    pub fn observation_incomplete(&self) -> bool {
        self.observation_incomplete
    }

    /// The current re-read is authoritative. A later successful read clears
    /// a previous failure so resume does not stick on a transient git miss.
    pub fn set_observation_incomplete(&mut self, incomplete: bool) {
        self.observation_incomplete = incomplete;
    }

    /// Record a workspace delta the tools did not publish. Paths already on
    /// the ledger are not added again, so a bound digest stays stable.
    pub fn observe_unseen_paths(&mut self, paths: &[String]) {
        let seen = self
            .records
            .iter()
            .map(|record| record.path.clone())
            .collect::<std::collections::HashSet<_>>();
        let accepted: Vec<String> = paths
            .iter()
            .map(|path| path.trim().trim_start_matches("./"))
            .filter(|path| !path.is_empty() && !seen.contains(*path))
            .filter(|path| {
                *path != ".a3s"
                    && !path.starts_with(".a3s/")
                    && *path != ".git"
                    && !path.starts_with(".git/")
            })
            .map(str::to_string)
            .collect();
        if accepted.is_empty() {
            return;
        }
        for path in accepted {
            self.records.push(MutationRecord {
                tool: "workspace".to_string(),
                path,
                content_digest: String::new(),
            });
        }
        self.rehash();
    }

    fn record_child_paths(&mut self, task_id: &str, paths: &[String]) {
        self.open_children.retain(|child| child.task_id != task_id);
        for path in paths {
            self.push("task".to_string(), path, String::new());
        }
    }

    fn push(&mut self, tool: String, path: &str, content_digest: String) {
        if crate::porcelain::is_harness_path(path) {
            return;
        }
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        self.records.push(MutationRecord {
            tool,
            path: path.to_string(),
            content_digest,
        });
        self.rehash();
    }

    fn rehash(&mut self) {
        self.records
            .sort_by(|left, right| left.path.cmp(&right.path).then(left.tool.cmp(&right.tool)));
        self.digest = effect_digest(&self.records);
    }
}

/// Fold settled background writers into this ledger before the gate runs.
/// A child that is still running is waited on. Cancellation leaves it open
/// so narrative success cannot hide the write.
pub async fn absorb_open_workspace_children(
    ledger: &mut MutationLedger,
    workspace: &std::path::Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> bool {
    let pending = ledger.open_children.to_vec();
    let mut incomplete = false;
    for child in pending {
        if let Some(paths) = crate::porcelain::take_settled_workspace_child(&child.task_id) {
            ledger.record_child_paths(&child.task_id, &paths);
            continue;
        }
        if crate::porcelain::workspace_child_pending(&child.task_id) {
            if let Some(paths) =
                crate::porcelain::await_workspace_child(&child.task_id, workspace, cancel).await
            {
                ledger.record_child_paths(&child.task_id, &paths);
            }
            continue;
        }
        if child.nongit && child.porcelain.is_none() && child.head.is_none() {
            continue;
        }
        let observed = crate::porcelain::delta(
            workspace,
            crate::porcelain::snapshot_from_parts(
                child.porcelain.clone(),
                child.head.clone(),
                Vec::new(),
            ),
        )
        .await;
        if observed.incomplete {
            incomplete = true;
        }
        ledger.record_child_paths(&child.task_id, &observed.paths);
    }
    incomplete
}

fn record_changed_paths(ledger: &mut MutationLedger, tool: &str, metadata: &Value) {
    let Some(paths) = metadata.get("changed_paths").and_then(Value::as_array) else {
        return;
    };
    for path in paths {
        if let Some(path) = path.as_str() {
            ledger.push(tool.to_string(), path, content_digest(metadata));
        }
    }
}

fn record_nested_tool_effects(ledger: &mut MutationLedger, metadata: &Value) {
    for (name, nested) in nested_tool_calls(metadata) {
        ledger.observe_tool(name, 0, nested);
    }
}

/// Child tool effects published by a wrapper. Search hits also use `results`,
/// but they have no `exit_code` and no tool name, so they are not mutations.
pub(crate) fn nested_tool_calls(metadata: &Value) -> Vec<(&str, Option<&Value>)> {
    let mut calls = Vec::new();
    push_nested_calls(
        &mut calls,
        metadata.pointer("/program/tool_calls"),
        "tool_name",
    );
    push_nested_calls(&mut calls, metadata.get("results"), "tool");
    calls
}

fn push_nested_calls<'a>(
    out: &mut Vec<(&'a str, Option<&'a Value>)>,
    calls: Option<&'a Value>,
    name_key: &str,
) {
    let Some(calls) = calls.and_then(Value::as_array) else {
        return;
    };
    for call in calls {
        if call.get("success").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        if call.get("exit_code").and_then(Value::as_i64) != Some(0) {
            continue;
        }
        let Some(name) = call.get(name_key).and_then(Value::as_str) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        out.push((name, call.get("metadata")));
    }
}

fn content_digest(metadata: &Value) -> String {
    if let Some(after) = metadata.get("after").and_then(Value::as_str) {
        return sha256::digest(after.as_bytes());
    }
    sha256::digest(metadata.to_string().as_bytes())
}

fn effect_digest(records: &[MutationRecord]) -> String {
    let canonical = records
        .iter()
        .map(|record| format!("{}|{}|{}", record.tool, record.path, record.content_digest))
        .collect::<Vec<_>>()
        .join("\n");
    sha256::digest(canonical.as_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionGate {
    Allow(CompletionTerminal),
    /// Ask the model once more, naming the missing evidence. Not a success.
    Continue {
        message: String,
    },
    /// Do not return `AgentResult` success.
    Incomplete {
        message: String,
    },
}

pub fn decide_completion(
    ledger: &MutationLedger,
    reports: &[VerificationReport],
    waivers: &[CompletionWaiverV1],
    allow_continuation: bool,
) -> CompletionGate {
    decide_with_observations(ledger, reports, waivers, allow_continuation, &[])
}

pub fn decide_with_observations(
    ledger: &MutationLedger,
    reports: &[VerificationReport],
    waivers: &[CompletionWaiverV1],
    allow_continuation: bool,
    observations: &[crate::external_observation::ExternalObservationV1],
) -> CompletionGate {
    let open = crate::external_observation::still_open(observations, waivers, ledger.digest());
    if crate::external_observation::blocks_success(&open) {
        let digest = open
            .iter()
            .map(|observation| observation.digest.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let message = format!(
            "completion gate: external observation {digest} still requires a workspace change. A final answer does not clear it. Bind a newer observation of the same subject or a host waiver for the observation digest."
        );
        return if allow_continuation {
            CompletionGate::Continue { message }
        } else {
            CompletionGate::Incomplete { message }
        };
    }
    if ledger.has_open_children() {
        let ids = ledger
            .open_children
            .iter()
            .map(|child| child.task_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let message = format!(
            "completion gate: background workspace task {ids} has not been observed. A final answer does not observe its writes."
        );
        return CompletionGate::Incomplete { message };
    }
    if ledger.observation_incomplete() {
        return CompletionGate::Incomplete {
            message: "completion gate: workspace observation is incomplete, so this digest is not the effect. A final answer, a waiver, or a verification of a partial list does not close it.".to_string(),
        };
    }
    if ledger.is_empty() {
        return CompletionGate::Allow(CompletionTerminal::Narrative);
    }
    let digest = ledger.digest().to_string();
    if waivers.iter().any(|waiver| waiver.effect_digest == digest) {
        return CompletionGate::Allow(CompletionTerminal::Waived {
            effect_digest: digest,
        });
    }
    if reports
        .iter()
        .any(|report| report_binds_pass(report, &digest))
    {
        return CompletionGate::Allow(CompletionTerminal::Verified {
            effect_digest: digest,
        });
    }
    let message = format!(
        "completion gate: workspace mutation {digest} has no bound Passed verification and no host waiver. Assistant text does not count. Bind a verification_report.effect_digest to this digest with required checks Passed, or obtain a host waiver for this digest."
    );
    // A host waiver is not model-grantable, editor-authored reports are
    // rejected, and no built-in tool attaches a bound effect digest. The
    // optional verifier turn already ran or was skipped before this decision.
    // Spending the one continuation here cannot close the gate; it only
    // invites another tool call. An open external observation still continues,
    // because a workspace write can satisfy that subject.
    CompletionGate::Incomplete { message }
}

fn report_binds_pass(report: &VerificationReport, digest: &str) -> bool {
    if report.effect_digest.as_deref() != Some(digest) {
        return false;
    }
    let required: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.required)
        .collect();
    if required.is_empty() {
        return false;
    }
    required
        .iter()
        .all(|check| check.status == VerificationStatus::Passed)
        && !matches!(
            report.status,
            VerificationStatus::Failed | VerificationStatus::NeedsReview
        )
}

/// Model-visible observation attached after a mutation. Never a verification pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationObservationV1 {
    pub schema: String,
    pub path: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
}

impl MutationObservationV1 {
    pub fn unavailable(path: impl Into<String>) -> Self {
        Self {
            schema: MUTATION_OBSERVATION_SCHEMA.to_string(),
            path: path.into(),
            status: "unavailable".to_string(),
            workspace_revision: None,
            items: Vec::new(),
        }
    }

    pub fn stale(path: impl Into<String>, workspace_revision: Option<u64>) -> Self {
        Self {
            schema: MUTATION_OBSERVATION_SCHEMA.to_string(),
            path: path.into(),
            status: "stale".to_string(),
            workspace_revision,
            items: Vec::new(),
        }
    }

    pub fn diagnostics(
        path: impl Into<String>,
        workspace_revision: Option<u64>,
        items: Vec<String>,
    ) -> Self {
        Self {
            schema: MUTATION_OBSERVATION_SCHEMA.to_string(),
            path: path.into(),
            status: "diagnostics".to_string(),
            workspace_revision,
            items,
        }
    }

    pub fn render(&self) -> String {
        let items = if self.items.is_empty() {
            String::new()
        } else {
            format!(" items={}", self.items.join(" | "))
        };
        format!(
            "[mutation observation] path={} status={}{items}",
            self.path, self.status
        )
    }
}

/// Build the observation attached to a mutation. A stale snapshot drops
/// diagnostic items so a previous revision cannot be presented as current.
pub fn observation_from_diagnostics(
    path: &str,
    revision: Option<u64>,
    stale: bool,
    items: Vec<String>,
) -> MutationObservationV1 {
    if stale {
        return MutationObservationV1::stale(path, revision);
    }
    MutationObservationV1::diagnostics(path, revision, items)
}

/// Tool-result text the next model call sees. This is the observation
/// attachment; it is not a `code_diagnostics` tool invocation.
pub fn model_visible_observation(output: &str, observation: &MutationObservationV1) -> String {
    let rendered = observation.render();
    if output.contains("[mutation observation]") {
        output.to_string()
    } else if output.is_empty() {
        rendered
    } else {
        format!("{output}\n{rendered}")
    }
}

pub fn attach_observation(metadata: &mut Option<Value>, observation: &MutationObservationV1) {
    let value = serde_json::to_value(observation).unwrap_or(Value::Null);
    match metadata {
        Some(Value::Object(map)) => {
            map.insert("mutation_observation".to_string(), value);
        }
        Some(other) => {
            *metadata = Some(serde_json::json!({
                "previous": other,
                "mutation_observation": value,
            }));
        }
        None => {
            *metadata = Some(serde_json::json!({ "mutation_observation": value }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::VerificationCheck;

    fn ledger_with_write() -> MutationLedger {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "write",
            0,
            Some(&serde_json::json!({"file_path": "src/lib.rs", "after": "fn main() {}"})),
        );
        ledger
    }

    #[test]
    fn read_only_tool_does_not_open_the_gate() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "read",
            0,
            Some(&serde_json::json!({"file_path": "src/lib.rs"})),
        );
        assert!(ledger.is_empty());
        assert!(matches!(
            decide_completion(&ledger, &[], &[], false),
            CompletionGate::Allow(CompletionTerminal::Narrative)
        ));
    }

    #[test]
    fn assistant_prose_does_not_satisfy_a_mutation() {
        let ledger = ledger_with_write();
        let decision = decide_completion(&ledger, &[], &[], false);
        match decision {
            CompletionGate::Incomplete { message } => {
                assert!(message.starts_with("completion gate:"));
                assert!(!message.contains("tests passed"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn open_external_observation_still_continues_once() {
        let observation = crate::external_observation::ExternalObservationV1::new(
            "review",
            "src/lib.rs",
            "obs-digest",
            "needs a workspace change",
            crate::external_observation::RequiredAction::WorkspaceChange,
        )
        .expect("observation");
        match decide_with_observations(&MutationLedger::default(), &[], &[], true, &[observation]) {
            CompletionGate::Continue { message } => {
                assert!(message.starts_with("completion gate:"));
                assert!(message.contains("external observation"));
            }
            other => panic!("expected continue, got {other:?}"),
        }
    }

    #[test]
    fn unbound_mutation_does_not_spend_a_continuation_the_model_cannot_close() {
        let ledger = ledger_with_write();
        match decide_completion(&ledger, &[], &[], true) {
            CompletionGate::Incomplete { message } => {
                assert!(message.starts_with("completion gate:"));
                assert!(message.contains("Assistant text does not count"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn bound_passed_report_allows_verified_terminal() {
        let ledger = ledger_with_write();
        let report = VerificationReport::new(
            "edit",
            vec![VerificationCheck::required("build", "command", "compiles")
                .with_status(VerificationStatus::Passed)],
        )
        .with_effect_digest(ledger.digest());
        match decide_completion(&ledger, &[report], &[], false) {
            CompletionGate::Allow(CompletionTerminal::Verified { effect_digest }) => {
                assert_eq!(effect_digest, ledger.digest());
            }
            other => panic!("expected verified, got {other:?}"),
        }
    }

    #[test]
    fn empty_required_checks_are_not_a_pass() {
        let ledger = ledger_with_write();
        let report = VerificationReport::new("edit", vec![]).with_effect_digest(ledger.digest());
        assert!(matches!(
            decide_completion(&ledger, &[report], &[], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn incomplete_observation_is_not_closed_by_a_waiver_of_a_partial_digest() {
        let mut ledger = ledger_with_write();
        ledger.set_observation_incomplete(true);
        let waiver = CompletionWaiverV1::new(ledger.digest(), "user accepted residual risk")
            .expect("waiver");
        match decide_completion(&ledger, &[], &[waiver], false) {
            CompletionGate::Incomplete { message } => {
                assert!(message.contains("observation is incomplete"));
                assert!(!message.contains("tests passed"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
        let mut empty = MutationLedger::default();
        empty.set_observation_incomplete(true);
        assert!(matches!(
            decide_completion(&empty, &[], &[], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn waiver_is_distinct_and_does_not_transfer_to_another_digest() {
        let ledger = ledger_with_write();
        let waiver = CompletionWaiverV1::new(ledger.digest(), "user accepted residual risk")
            .expect("waiver");
        match decide_completion(&ledger, &[], &[waiver.clone()], false) {
            CompletionGate::Allow(CompletionTerminal::Waived { effect_digest }) => {
                assert_eq!(effect_digest, ledger.digest());
            }
            other => panic!("expected waiver, got {other:?}"),
        }
        let mut other = MutationLedger::default();
        other.observe_tool(
            "write",
            0,
            Some(&serde_json::json!({"file_path": "other.rs", "after": "different"})),
        );
        assert!(matches!(
            decide_completion(&other, &[], &[waiver], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn diagnostic_appears_on_the_next_model_visible_tool_result() {
        let observation = observation_from_diagnostics(
            "src/lib.rs",
            Some(4),
            false,
            vec!["src/lib.rs:3: unused variable".to_string()],
        );
        let visible = model_visible_observation("wrote src/lib.rs", &observation);
        assert!(visible.contains("unused variable"));
        assert!(!visible.contains("code_diagnostics"));
        let message = crate::llm::Message::tool_result("write-1", &visible, false);
        let model_input = message
            .content
            .iter()
            .find_map(|block| match block {
                crate::llm::ContentBlock::ToolResult {
                    content: crate::llm::ToolResultContentField::Text(text),
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .expect("tool result is the next model input");
        assert!(model_input.contains("unused variable"));
        let stale = observation_from_diagnostics(
            "src/lib.rs",
            Some(3),
            true,
            vec!["src/lib.rs:1: previous revision".to_string()],
        );
        assert!(!stale.render().contains("previous revision"));
        assert!(stale.render().contains("stale"));
    }

    #[test]
    fn unavailable_observation_does_not_satisfy_the_gate() {
        let ledger = ledger_with_write();
        let observation = MutationObservationV1::unavailable("src/lib.rs");
        assert!(observation.render().contains("unavailable"));
        assert!(matches!(
            decide_completion(&ledger, &[], &[], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn unseen_workspace_path_opens_the_gate_without_a_second_digest() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "write",
            0,
            Some(&serde_json::json!({ "file_path": "src/lib.rs" })),
        );
        let digest = ledger.digest().to_string();
        ledger.observe_unseen_paths(&[
            "src/lib.rs".to_string(),
            ".a3s/tui/outcomes/v1/id.json".to_string(),
            "guest.txt".to_string(),
        ]);
        assert_ne!(ledger.digest(), digest);
        assert!(ledger.paths().any(|path| path == "guest.txt"));
        assert!(!ledger.paths().any(|path| path.contains(".a3s")));
        assert_eq!(
            ledger.paths().filter(|path| *path == "src/lib.rs").count(),
            1,
            "an already recorded path must not mint a second effect identity"
        );
        assert!(matches!(
            decide_completion(&ledger, &[], &[], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[tokio::test]
    async fn background_workspace_child_blocks_narrative_until_its_delta_is_observed() {
        let workspace = tempfile::tempdir().unwrap();
        let task_id = "task-open-child";
        crate::porcelain::reserve_workspace_child(task_id);
        crate::porcelain::begin_workspace_child(task_id, workspace.path()).await;
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "task",
            0,
            Some(&serde_json::json!({ "workspace_child": task_id })),
        );
        match decide_completion(&ledger, &[], &[], true) {
            CompletionGate::Incomplete { message } => {
                assert!(message.contains("background workspace task"));
                assert!(message.contains(task_id));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }

        std::fs::write(workspace.path().join("guest.txt"), "hello\n").unwrap();
        crate::porcelain::settle_workspace_child(task_id, workspace.path()).await;
        absorb_open_workspace_children(
            &mut ledger,
            workspace.path(),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(
            ledger.paths().any(|path| path == "guest.txt"),
            "settled background write was not a parent mutation"
        );
        assert!(matches!(
            decide_completion(&ledger, &[], &[], false),
            CompletionGate::Incomplete { .. }
        ));
    }

    #[test]
    fn failed_wrapper_with_changed_paths_opens_the_gate() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "bash",
            1,
            Some(&serde_json::json!({
                "exit_code": 1,
                "changed_paths": ["guest.txt"]
            })),
        );
        match decide_completion(&ledger, &[], &[], false) {
            CompletionGate::Incomplete { message } => {
                assert!(message.starts_with("completion gate:"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn failed_write_path_without_changed_paths_is_not_a_mutation() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "write",
            1,
            Some(&serde_json::json!({"file_path": "guest.txt"})),
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn bash_without_changed_paths_is_not_a_mutation() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "bash",
            0,
            Some(&serde_json::json!({"command": "rm -rf src"})),
        );
        assert!(ledger.is_empty());
    }

    #[test]
    fn stale_observation_omits_items() {
        let observation = MutationObservationV1::stale("src/lib.rs", Some(4));
        assert!(observation.items.is_empty());
        assert_eq!(observation.status, "stale");
        assert!(!observation.render().contains("error"));
    }

    #[test]
    fn nested_batch_write_opens_the_gate() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "batch",
            0,
            Some(&serde_json::json!({
                "status": "complete",
                "results": [{
                    "tool": "write",
                    "success": true,
                    "exit_code": 0,
                    "metadata": {"file_path": "src/lib.rs", "after": "fn main() {}"}
                }, {
                    "title": "not a tool call",
                    "success": true
                }]
            })),
        );
        match decide_completion(&ledger, &[], &[], false) {
            CompletionGate::Incomplete { message } => {
                assert!(message.starts_with("completion gate:"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn retrieval_index_stamp_is_not_a_source_mutation() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "search",
            0,
            Some(&serde_json::json!({
                "changed_paths": [".a3s-code/grep-trigram/stamp.txt"]
            })),
        );
        assert!(
            matches!(
                decide_completion(&ledger, &[], &[], false),
                CompletionGate::Allow(CompletionTerminal::Narrative)
            ),
            "a retrieval index stamp opened the completion gate"
        );
    }

    #[test]
    fn skill_changed_paths_open_the_gate() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "skill",
            0,
            Some(&serde_json::json!({
                "skill_name": "writer",
                "tool_calls": 1,
                "changed_paths": ["guest.txt"]
            })),
        );
        match decide_completion(&ledger, &[], &[], false) {
            CompletionGate::Incomplete { message } => {
                assert!(message.starts_with("completion gate:"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn nested_program_write_opens_the_gate() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool(
            "program",
            0,
            Some(&serde_json::json!({
                "program": {
                    "tool_calls": [{
                        "tool_name": "write",
                        "success": true,
                        "exit_code": 0,
                        "metadata": {"file_path": "src/lib.rs", "after": "fn main() {}"}
                    }]
                }
            })),
        );
        match decide_completion(&ledger, &[], &[], false) {
            CompletionGate::Incomplete { message } => {
                assert!(message.starts_with("completion gate:"));
            }
            other => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn bound_step_closure_is_not_rewritten_as_narrative() {
        let verified = CompletionTerminal::Verified {
            effect_digest: "digest-a".to_string(),
        };
        assert_eq!(
            fold_step_completions(&[CompletionTerminal::Narrative, verified.clone()]),
            verified
        );
        assert_eq!(
            fold_step_completions(&[
                verified.clone(),
                CompletionTerminal::Waived {
                    effect_digest: "digest-a".to_string(),
                },
            ]),
            verified
        );
        assert_eq!(
            fold_step_completions(&[
                CompletionTerminal::Verified {
                    effect_digest: "digest-a".to_string(),
                },
                CompletionTerminal::Verified {
                    effect_digest: "digest-b".to_string(),
                },
            ]),
            CompletionTerminal::Distinct
        );
        assert!(CompletionTerminal::Distinct != CompletionTerminal::Narrative);
    }

    #[test]
    fn plan_claim_without_digest_is_ordinary() {
        let claimed = PlanRunAdmission {
            claims_implementation: true,
            plan_digest: None,
        };
        assert_eq!(claimed.label(), "ordinary");
        assert_eq!(
            PlanRunAdmission::implementation("abc").label(),
            "plan_implementation"
        );
    }
}
