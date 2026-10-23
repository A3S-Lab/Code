//! Causal completion and mutation observation for the coding loop.
//!
//! A mutating run cannot succeed on assistant prose. Verification reports and
//! host waivers must bind the same effect digest. Diagnostics after a write
//! are an observation, never a pass.

use crate::verification::{VerificationReport, VerificationStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MUTATION_OBSERVATION_SCHEMA: &str = "a3s.code.mutation-observation.v1";
/// File tools whose settled result puts a workspace path on the mutation
/// ledger. The fact-spine continuation replay uses the same list so an
/// interrupted run's inherited debt matches what a live run would observe.
pub(crate) const MUTATING_FILE_TOOLS: &[&str] = &["write", "edit", "patch", "download"];

/// Why the run was cut short by something other than the model finishing.
/// Carried on the settled `End` event so clients can say why a turn ended
/// instead of presenting the cut as the model's confident final answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForcedStop {
    /// The tool-round budget was exhausted; the next call was denied.
    ToolBudget,
    /// The read-only evidence ceiling fired; the turn was forced to answer.
    ReadOnlyEvidence,
    /// The provider cut the final response at its length limit (stop_reason
    /// `length`/`max_tokens` with no tool call): the settled answer is
    /// truncated, not complete.
    ResponseTruncated,
}

impl ForcedStop {
    /// Wire name for the `x.ai/turn/outcome` payload; the pager's parser
    /// accepts exactly these values.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolBudget => "tool_budget",
            Self::ReadOnlyEvidence => "read_only_evidence",
            Self::ResponseTruncated => "response_truncated",
        }
    }
}

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
    /// Exact observed state. The content field above remains file bytes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_identity: Option<a3s_effect::ToolFileIdentityV1>,
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
    /// Durable invocation uncertainty is not cleared by a successful local
    /// scan. Only identity-bound receipt/reconciliation facts can discharge it.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    unresolved_tool_effects: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    applied_tool_receipts: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    applied_background_receipts: std::collections::BTreeSet<String>,
}

/// Normalize a mutation path to its workspace-relative form: a model may
/// hand a mutating tool an ABSOLUTE path under the workspace, and the
/// ledger's consumers (contract covers patterns, existence-check matching,
/// the gate's path list) are workspace-relative. Paths outside the
/// workspace and already-relative paths pass through unchanged — the
/// normalization is an identity for everything but the under-workspace
/// absolute spelling, so ledger digests stay stable across the two
/// spellings of the same edit.
pub(crate) fn workspace_relative_path(workspace: &std::path::Path, path: &str) -> String {
    let trimmed = path.trim();
    if !trimmed.starts_with('/') {
        return trimmed.to_string();
    }
    let workspace = workspace.to_string_lossy();
    let workspace = workspace.trim_end_matches('/');
    if let Some(stripped) = trimmed.strip_prefix(&format!("{workspace}/")) {
        if !stripped.is_empty() {
            return stripped.to_string();
        }
    }
    trimmed.to_string()
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

    pub(crate) fn observes_current_workspace(&self, root: &std::path::Path) -> bool {
        let expected = self
            .records
            .iter()
            .map(|record| (record.path.as_str(), record.after_identity.as_ref()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let observed = crate::porcelain::observe_path_effects(
            root,
            crate::porcelain::PathDelta {
                paths: expected.keys().map(|path| (*path).to_string()).collect(),
                incomplete: false,
            },
        );
        observed.completeness == a3s_effect::ToolObservationCompleteness::Complete
            && observed.changes.len() == expected.len()
            && observed.changes.iter().all(|change| {
                expected.get(change.path.as_str()).copied().flatten() == Some(&change.after)
            })
    }

    /// Latest content digest recorded for a mutated path (path-boundary match).
    pub fn content_digest_for_path(&self, path: &str) -> Option<&str> {
        self.records.iter().rev().find_map(|record| {
            if crate::verification::mutation_path_matches(record.path.as_str(), path) {
                Some(record.content_digest.as_str())
            } else {
                None
            }
        })
    }

    /// True when any record for `path` stored a non-empty content digest.
    ///
    /// A later `changed_paths` row hashes tool metadata, not file bytes. Callers
    /// that need the write's bytes must not treat that later row as the only
    /// digest.
    pub fn has_content_digest(&self, path: &str) -> bool {
        self.records.iter().any(|record| {
            !record.content_digest.is_empty()
                && crate::verification::mutation_path_matches(record.path.as_str(), path)
        })
    }

    /// True when a recorded content digest for `path` is exactly `digest`.
    pub fn content_digest_matches(&self, path: &str, digest: &str) -> bool {
        let digest = digest.trim();
        if digest.is_empty() {
            return false;
        }
        self.records.iter().any(|record| {
            record.content_digest == digest
                && crate::verification::mutation_path_matches(record.path.as_str(), path)
        })
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

    #[cfg(test)]
    pub(crate) fn set_unresolved_tool_effects(
        &mut self,
        unresolved: std::collections::BTreeSet<String>,
    ) {
        self.restore_unresolved_tool_effects(unresolved, &std::collections::BTreeSet::new());
    }

    /// Recovery may add evidence of uncertainty, but a cache or turn marker
    /// cannot erase it. The caller's validated, exact durable background
    /// terminals can settle matching cached children without a local registry.
    pub(crate) fn restore_unresolved_tool_effects(
        &mut self,
        unresolved: std::collections::BTreeSet<String>,
        settled_workspace_children: &std::collections::BTreeSet<String>,
    ) {
        self.unresolved_tool_effects.extend(unresolved);
        self.open_children.retain(|child| {
            !settled_workspace_children.contains(&format!("workspace-child:{}", child.task_id))
        });
        self.unresolved_tool_effects.retain(|key| {
            !key.starts_with("workspace-child:") || !settled_workspace_children.contains(key)
        });
    }

    /// Compare cached claims, including uncertainty, with a projection made
    /// only from validated original receipts. The caller owns log/admission
    /// validation and prefix selection; cached keys or current filesystem
    /// scans cannot stand in for that projection.
    pub(crate) fn checkpoint_cache_covered_by(&self, original: &Self) -> bool {
        // A receipt's honest Unknown/Incomplete and exact external-operation
        // ids already exist in the original projection. Extra cached debt
        // still needs provenance. Local observation failures and child
        // snapshot fields have no exact typed receipt projection here.
        if !self.open_children.is_empty()
            || self.observation_incomplete
            || !self
                .unresolved_tool_effects
                .is_subset(&original.unresolved_tool_effects)
        {
            return false;
        }
        if original.applied_tool_receipts.is_empty()
            && original.applied_background_receipts.is_empty()
        {
            return false;
        }
        if !self.records.is_empty()
            && (self.applied_tool_receipts.is_empty()
                && self.applied_background_receipts.is_empty())
        {
            // Legacy cached paths have no receipt provenance. They remain
            // conservative debt, even when an unrelated read is in the log.
            return false;
        }
        if !self
            .applied_tool_receipts
            .is_subset(&original.applied_tool_receipts)
            || !self
                .applied_background_receipts
                .is_subset(&original.applied_background_receipts)
        {
            return false;
        }
        let observed = original
            .records
            .iter()
            .map(|record| (record.path.as_str(), record))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut paths = std::collections::BTreeSet::new();
        self.records.iter().all(|record| {
            record.after_identity.is_some()
                && paths.insert(record.path.as_str())
                && observed.get(record.path.as_str()).is_some_and(|observed| {
                    observed.after_identity == record.after_identity
                        && observed.content_digest == record.content_digest
                })
        }) && (self.records.is_empty() || self.digest == effect_digest(&self.records))
    }

    pub(crate) fn checkpoint_receipt_keys(
        &self,
    ) -> (
        &std::collections::BTreeSet<String>,
        &std::collections::BTreeSet<String>,
    ) {
        (
            &self.applied_tool_receipts,
            &self.applied_background_receipts,
        )
    }

    /// Cached negative claims must be present in the selected original-log
    /// prefix before that prefix can be compared as the cache candidate.
    pub(crate) fn checkpoint_uncertainty_keys(&self) -> &std::collections::BTreeSet<String> {
        &self.unresolved_tool_effects
    }

    /// A cache containing only negative claims needs no invented receipt.
    /// The caller supplies exact validated typed uncertainty from the chosen
    /// original-log boundary, never a cached or generic ledger debt set.
    pub(crate) fn checkpoint_negative_cache_covered_by(
        &self,
        validated_original_uncertainty: &std::collections::BTreeSet<String>,
    ) -> bool {
        self.records.is_empty()
            && self.applied_tool_receipts.is_empty()
            && self.applied_background_receipts.is_empty()
            && self.open_children.is_empty()
            && !self.observation_incomplete
            && !self.unresolved_tool_effects.is_empty()
            && self
                .unresolved_tool_effects
                .is_subset(validated_original_uncertainty)
    }

    /// A checkpoint is a conservative cache of the log projection. Fill
    /// missing observations without replacing a later live projection, and
    /// allow the original log to reapply every receipt after this merge.
    pub(crate) fn restore_checkpoint_cache(
        &mut self,
        cached: &Self,
        original_cache_coverage_complete: bool,
    ) {
        let cached_has_evidence = cached.has_checkpoint_evidence();
        let mut records_changed = false;
        for record in &cached.records {
            if !self
                .records
                .iter()
                .any(|current| current.path == record.path)
            {
                self.records.push(record.clone());
                records_changed = true;
            }
        }
        for child in &cached.open_children {
            if !self
                .open_children
                .iter()
                .any(|current| current.task_id == child.task_id)
            {
                self.open_children.push(child.clone());
            }
        }
        self.observation_incomplete |= cached.observation_incomplete;
        self.unresolved_tool_effects
            .extend(cached.unresolved_tool_effects.iter().cloned());
        if cached_has_evidence && !original_cache_coverage_complete {
            self.unresolved_tool_effects
                .insert("tool_effect_cache_evidence_missing".into());
        }
        self.begin_receipt_replay();
        if records_changed {
            self.rehash();
        }
    }

    pub(crate) fn has_checkpoint_evidence(&self) -> bool {
        !self.records.is_empty()
            || !self.open_children.is_empty()
            || !self.unresolved_tool_effects.is_empty()
            || self.observation_incomplete
            || !self.applied_tool_receipts.is_empty()
            || !self.applied_background_receipts.is_empty()
    }

    pub(crate) fn has_uncertain_tool_invocations(&self) -> bool {
        self.unresolved_tool_effects
            .iter()
            .any(|key| !key.starts_with("workspace-child:"))
    }

    /// A full log projection must apply every typed event in log order. An
    /// eagerly observed foreground receipt must not hide a later replay step
    /// after an older background receipt first becomes visible locally.
    pub(crate) fn begin_receipt_replay(&mut self) {
        self.applied_tool_receipts.clear();
        self.applied_background_receipts.clear();
    }

    pub(crate) fn observe_tool_receipt(
        &mut self,
        tool: &str,
        result: &a3s_effect::ToolExecutionResultV1,
    ) {
        if result.validate().is_err() {
            self.unresolved_tool_effects
                .insert("tool_effect_schema_unsupported".into());
            return;
        }
        let key = match result.identity.key() {
            Ok(key) => key,
            Err(_) => {
                self.unresolved_tool_effects
                    .insert("tool_effect_identity_mismatch".into());
                return;
            }
        };
        if !self.applied_tool_receipts.insert(key.clone()) {
            return;
        }
        for change in &result.effects.changes {
            self.observe_receipt_path(tool, &change.path, &change.after);
        }
        if result.outcome == a3s_effect::ToolExecutionOutcome::Unknown
            || result.effects.completeness == a3s_effect::ToolObservationCompleteness::Incomplete
        {
            self.unresolved_tool_effects.insert(key);
        }
        self.unresolved_tool_effects.extend(
            result
                .effects
                .unresolved_external_operations
                .iter()
                .cloned(),
        );
    }

    /// Project byte-bound observations from an existing background terminal.
    /// Its immutable fact key supplies deduplication only; this cannot admit,
    /// execute or manufacture a tool invocation receipt.
    pub(crate) fn observe_background_receipt(
        &mut self,
        fact_key: &str,
        effects: &a3s_effect::ToolEffectObservationV1,
    ) {
        if fact_key.trim().is_empty() {
            self.unresolved_tool_effects
                .insert("background-receipt-identity-missing".into());
            return;
        }
        if effects.validate().is_err() {
            self.unresolved_tool_effects
                .insert(format!("background-receipt-invalid:{fact_key}"));
            return;
        }
        if !self.applied_background_receipts.insert(fact_key.into()) {
            return;
        }
        for change in &effects.changes {
            self.observe_receipt_path("background", &change.path, &change.after);
        }
        if effects.completeness == a3s_effect::ToolObservationCompleteness::Incomplete {
            self.unresolved_tool_effects
                .insert(format!("background-receipt-incomplete:{fact_key}"));
        }
        self.unresolved_tool_effects
            .extend(effects.unresolved_external_operations.iter().cloned());
    }

    fn observe_receipt_path(
        &mut self,
        tool: &str,
        path: &str,
        after: &a3s_effect::ToolFileIdentityV1,
    ) {
        if path.is_empty() || crate::porcelain::is_harness_path(path) {
            return;
        }
        // Whole-workspace observations overlap within a prepared batch. The
        // observer and invocation are provenance, not a new file state.
        if self
            .records
            .iter()
            .any(|record| record.path == path && record.after_identity.as_ref() == Some(after))
        {
            return;
        }
        // An authoritative after-state supersedes older byte-only observations
        // for this exact path, including a stale content digest after deletion.
        self.records.retain(|record| record.path != path);
        self.records.push(MutationRecord {
            tool: tool.to_string(),
            path: path.to_string(),
            content_digest: after
                .content_digest
                .as_deref()
                .map(|digest| digest.strip_prefix("sha256:").unwrap_or(digest))
                .unwrap_or_default()
                .to_string(),
            after_identity: Some(after.clone()),
        });
        self.rehash();
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
                after_identity: None,
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
            after_identity: None,
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
            crate::porcelain::request_workspace_child_settlement(&child.task_id);
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
    if records.iter().any(|record| record.after_identity.is_some()) {
        let mut canonical = b"a3s.code.workspace-effect-state.v1\0".to_vec();
        for record in records {
            match &record.after_identity {
                Some(after) => {
                    canonical.push(1);
                    append_effect_field(&mut canonical, record.path.as_bytes());
                    append_effect_field(&mut canonical, after.kind.as_str().as_bytes());
                    match after.mode {
                        Some(mode) => {
                            canonical.push(1);
                            canonical.extend_from_slice(&mode.to_be_bytes());
                        }
                        None => canonical.push(0),
                    }
                    match &after.content_digest {
                        Some(digest) => {
                            canonical.push(1);
                            append_effect_field(&mut canonical, digest.as_bytes());
                        }
                        None => canonical.push(0),
                    }
                }
                None => {
                    canonical.push(0);
                    append_effect_field(&mut canonical, record.path.as_bytes());
                    append_effect_field(&mut canonical, record.tool.as_bytes());
                    append_effect_field(&mut canonical, record.content_digest.as_bytes());
                }
            }
        }
        return sha256::digest(canonical);
    }
    // Legacy checkpoints retain their original digest until typed state is
    // observed; absent fields do not invent a stronger historical identity.
    let canonical = records
        .iter()
        .map(|record| format!("{}|{}|{}", record.tool, record.path, record.content_digest))
        .collect::<Vec<_>>()
        .join("\n");
    sha256::digest(canonical.as_bytes())
}

fn append_effect_field(canonical: &mut Vec<u8>, field: &[u8]) {
    canonical.extend_from_slice(field.len().to_string().as_bytes());
    canonical.push(b':');
    canonical.extend_from_slice(field);
}

#[cfg(test)]
mod receipt_tests;

#[cfg(test)]
mod receipt_restore_tests;

#[cfg(test)]
mod background_receipt_tests;

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

/// Host-selected policy for a missing verification record. Never grants tool permission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompletionVerificationPolicy {
    #[default]
    RequireVerification,
    WaiveMissingVerification,
}

impl CompletionVerificationPolicy {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "require-verification" => Ok(Self::RequireVerification),
            "waive-missing-verification" => Ok(Self::WaiveMissingVerification),
            _ => Err(format!("invalid CompletionVerificationPolicy: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequireVerification => "require-verification",
            Self::WaiveMissingVerification => "waive-missing-verification",
        }
    }
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
    decide_with_verification_policy(
        ledger,
        reports,
        waivers,
        allow_continuation,
        observations,
        CompletionVerificationPolicy::RequireVerification,
    )
}

/// Per-session host policy; existing callers retain strict completion semantics.
pub fn decide_with_verification_policy(
    ledger: &MutationLedger,
    reports: &[VerificationReport],
    waivers: &[CompletionWaiverV1],
    allow_continuation: bool,
    observations: &[crate::external_observation::ExternalObservationV1],
    policy: CompletionVerificationPolicy,
) -> CompletionGate {
    if !ledger.unresolved_tool_effects.is_empty() {
        return CompletionGate::Incomplete {
            message: format!(
                "tool_effect_unresolved: {} invocation obligation(s) require identity-bound reconciliation; local scans, final text, and ordinary waivers cannot discharge them.",
                ledger.unresolved_tool_effects.len()
            ),
        };
    }
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
    // Missing evidence may be waived, but an existing report for this effect
    // (including a failed, pending, or incomplete report) must be resolved.
    if policy == CompletionVerificationPolicy::WaiveMissingVerification
        && !reports
            .iter()
            .any(|report| report.effect_digest.as_deref() == Some(digest.as_str()))
    {
        return CompletionGate::Allow(CompletionTerminal::Waived {
            effect_digest: digest,
        });
    }
    let paths = mutation_paths_for_gate(ledger);
    let next = unbound_mutation_next_step(ledger);
    let message = format!(
        "{UNBOUND_MUTATION_GATE}{digest}{paths} has no bound Passed verification and no host waiver. Assistant text does not count. Bind a verification_report.effect_digest to this digest with required checks Passed, or obtain a host waiver for this digest.{next}"
    );
    // A host waiver is not model-grantable, and editor-authored reports are
    // rejected. Built-in bash may bind a digest only when an existence check
    // matches a mutated path *and* on-disk content matches the ledger digest.
    // The optional verifier turn already ran or was skipped before this
    // decision. Spending the one continuation here cannot close the gate; it
    // only invites another tool call. An open external observation still
    // continues, because a workspace write can satisfy that subject.
    CompletionGate::Incomplete { message }
}

const UNBOUND_MUTATION_GATE: &str = "completion gate: workspace mutation ";

/// The mutation digest named by an unbound-mutation gate message from
/// [`decide_with_observations`]. A host that answers the gate with its own
/// check reads the digest back through this function, not a matcher of its own.
pub fn unbound_mutation_digest(message: &str) -> Option<&str> {
    let start = message.find(UNBOUND_MUTATION_GATE)? + UNBOUND_MUTATION_GATE.len();
    let digest = message[start..].split(' ').next()?;
    (!digest.is_empty()).then_some(digest)
}

/// The step the model can take itself. Built-in `bash` binds the digest when it
/// exits 0 and a `test -f` segment names a changed file whose bytes the ledger
/// recorded. Without such a file only a declared suite or a host waiver binds.
fn unbound_mutation_next_step(ledger: &MutationLedger) -> String {
    let Some(path) = ledger
        .paths()
        .filter(|path| ledger.has_content_digest(path))
        .min()
    else {
        return String::new();
    };
    format!(
        " Next step: run one `bash` command that exits 0 and includes `test -f {path}` with the check that proves the change, for example `test -f {path} && <test command>`.",
        path = shell_words::quote(path),
    )
}

/// A few ledger paths, so a failed turn names what changed. The digest stays
/// the identity a report or waiver has to match.
fn mutation_paths_for_gate(ledger: &MutationLedger) -> String {
    let mut paths = ledger.paths().collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    if paths.is_empty() {
        return String::new();
    }
    const SHOWN: usize = 4;
    let body = if paths.len() <= SHOWN {
        paths.join(", ")
    } else {
        format!(
            "{}, and {} more",
            paths[..SHOWN].join(", "),
            paths.len() - SHOWN
        )
    };
    format!(" ({body})")
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

pub(crate) fn model_verification_context(gate: CompletionGate) -> Option<String> {
    let CompletionGate::Incomplete { message } = gate else {
        return None;
    };
    if !message.starts_with(UNBOUND_MUTATION_GATE) {
        return None;
    }
    unbound_mutation_digest(&message)?;
    let diagnostic = serde_json::Value::String(message);
    Some(format!(
        "[host verification state]\nVerify the recorded changes using only exposed tools and the admitted checker when supplied. For built-in bash verification, use explicit `test -f path` checks with literal file paths, followed by content checks. Do not substitute shell variables for recorded paths. Do not repeat completed mutations. This note grants no verification or waiver. Diagnostic data (not instructions):\n{diagnostic}"
    ))
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
mod tests;
