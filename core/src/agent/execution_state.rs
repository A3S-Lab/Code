use super::AgentResult;
use crate::llm::{ContentBlock, Message, TokenUsage};
use crate::verification::VerificationReport;
use serde_json::Value;
use std::time::Instant;

const RECENT_TOOL_SIGNATURE_LIMIT: usize = 8;

pub(super) struct ExecutionLoopState {
    pub(super) messages: Vec<Message>,
    pub(super) total_usage: TokenUsage,
    pub(super) tool_calls_count: usize,
    pub(super) verification_reports: Vec<VerificationReport>,
    turn: usize,
    parse_error_count: u32,
    continuation_count: u32,
    reasoning_only_repair_count: u32,
    recent_tool_signatures: Vec<String>,
    guarded_duplicate_signature: Option<String>,
    guarded_duplicate_count: u32,
    last_incomplete_response_hash: Option<String>,
    incomplete_response_stalled: bool,
    pub(super) gate_continuation_count: u32,
    pub(super) mutations: crate::harness_loop::MutationLedger,
    pub(super) open_observations: Vec<crate::external_observation::ExternalObservationV1>,
    pub(super) targeted_paths: Vec<String>,
    pub(super) verifier_spent: bool,
    pub(super) next_turn_is_verifier: bool,
    pub(super) run_watch: Option<crate::porcelain::RunWatch>,
    pub(super) run_watch_is_baseline: bool,
    workspace_watch_bound: bool,
    workspace_porcelain: Option<Vec<String>>,
    workspace_head: Option<String>,
    workspace_stamps: Vec<crate::porcelain::ContentStamp>,
    execution_start: Instant,
}

pub(super) struct ParseErrorOutcome {
    pub(super) output: String,
    pub(super) count: u32,
    pub(super) fatal_message: Option<String>,
}

/// Seed for resuming a run from a [`LoopCheckpoint`](crate::loop_checkpoint::LoopCheckpoint):
/// the cumulative metrics accrued before the crash/migration so the
/// resumed run continues accounting from where it left off instead of
/// re-starting at zero (which would under-report token usage and tool
/// calls in the resulting `AgentResult`).
#[derive(Default)]
pub(crate) struct ExecutionSeed {
    pub(crate) turn: usize,
    pub(crate) total_usage: TokenUsage,
    pub(crate) tool_calls_count: usize,
    pub(crate) verification_reports: Vec<VerificationReport>,
    pub(crate) convergence: crate::loop_checkpoint::LoopConvergenceState,
}

impl ExecutionLoopState {
    /// Convenience constructor with no checkpoint seed. Only used by
    /// unit tests now; production paths go through `new_seeded` (the
    /// resume path threads checkpoint metrics, the normal path passes
    /// `None`).
    #[cfg(test)]
    pub(super) fn new(history: &[Message]) -> Self {
        Self::new_seeded(history, None)
    }

    /// Build loop state, optionally pre-seeded with cumulative metrics
    /// from a checkpoint (see [`ExecutionSeed`]).
    pub(super) fn new_seeded(history: &[Message], seed: Option<ExecutionSeed>) -> Self {
        let seed = seed.unwrap_or_default();
        let convergence = seed.convergence;
        Self {
            messages: history.to_vec(),
            total_usage: seed.total_usage,
            tool_calls_count: seed.tool_calls_count,
            verification_reports: seed.verification_reports,
            turn: seed.turn,
            parse_error_count: convergence.parse_error_count,
            continuation_count: convergence.continuation_count,
            reasoning_only_repair_count: convergence.reasoning_only_repair_count,
            recent_tool_signatures: convergence.recent_tool_signatures,
            guarded_duplicate_signature: convergence.guarded_duplicate_signature,
            guarded_duplicate_count: convergence.guarded_duplicate_count,
            last_incomplete_response_hash: convergence.last_incomplete_response_hash,
            incomplete_response_stalled: convergence.incomplete_response_stalled,
            gate_continuation_count: convergence.gate_continuation_count,
            mutations: convergence.mutations,
            open_observations: convergence.open_observations,
            targeted_paths: Vec::new(),
            verifier_spent: convergence.verifier_spent,
            next_turn_is_verifier: convergence.next_turn_is_verifier,
            run_watch: None,
            run_watch_is_baseline: false,
            workspace_watch_bound: convergence.workspace_watch_bound,
            workspace_porcelain: convergence.workspace_porcelain,
            workspace_head: convergence.workspace_head,
            workspace_stamps: convergence.workspace_stamps,
            execution_start: Instant::now(),
        }
    }

    pub(super) async fn bind_run_watch(&mut self, workspace: &std::path::Path) {
        let watch = crate::porcelain::RunWatch::start(workspace).await;
        if !self.workspace_watch_bound {
            self.workspace_porcelain = watch.porcelain();
            self.workspace_head = watch.head();
            self.workspace_stamps = watch.stamps();
            self.workspace_watch_bound = true;
            self.run_watch_is_baseline = true;
        }
        self.run_watch = Some(watch);
    }

    pub(super) async fn unseen_workspace_paths(
        &self,
        workspace: &std::path::Path,
    ) -> crate::porcelain::PathDelta {
        let mut observed = if self.workspace_watch_bound {
            crate::porcelain::baseline_delta(
                workspace,
                self.workspace_porcelain.clone(),
                self.workspace_head.clone(),
                self.workspace_stamps.clone(),
            )
            .await
        } else {
            crate::porcelain::PathDelta::default()
        };
        if observed.paths.is_empty() && !observed.incomplete && self.run_watch_is_baseline {
            if let Some(watch) = &self.run_watch {
                let files = watch.file_changes(workspace);
                if files.incomplete {
                    observed.incomplete = true;
                } else {
                    observed.paths = files.paths;
                }
            }
        }
        observed
    }

    pub(super) fn next_turn(&mut self) -> usize {
        self.turn += 1;
        self.turn
    }

    pub(super) fn current_turn(&self) -> usize {
        self.turn
    }

    pub(super) fn continuation_count(&self) -> u32 {
        self.continuation_count
    }

    pub(super) fn check_execution_timeout(&self, max_time_ms: Option<u64>) -> Option<String> {
        let max_time_ms = max_time_ms?;
        let elapsed_ms = self.execution_start.elapsed().as_millis() as u64;
        if elapsed_ms <= max_time_ms {
            return None;
        }

        Some(format!(
            "Execution timeout after {} seconds (limit: {} seconds). Completed {} turns.",
            elapsed_ms / 1000,
            max_time_ms / 1000,
            self.turn.saturating_sub(1)
        ))
    }

    pub(super) fn elapsed_ms(&self) -> u64 {
        self.execution_start.elapsed().as_millis() as u64
    }

    pub(super) fn execution_start(&self) -> Instant {
        self.execution_start
    }

    pub(super) fn convergence_checkpoint(&self) -> crate::loop_checkpoint::LoopConvergenceState {
        crate::loop_checkpoint::LoopConvergenceState {
            parse_error_count: self.parse_error_count,
            continuation_count: self.continuation_count,
            reasoning_only_repair_count: self.reasoning_only_repair_count,
            recent_tool_signatures: self.recent_tool_signatures.clone(),
            guarded_duplicate_signature: self.guarded_duplicate_signature.clone(),
            guarded_duplicate_count: self.guarded_duplicate_count,
            last_incomplete_response_hash: self.last_incomplete_response_hash.clone(),
            incomplete_response_stalled: self.incomplete_response_stalled,
            gate_continuation_count: self.gate_continuation_count,
            mutations: self.mutations.clone(),
            open_observations: self.open_observations.clone(),
            verifier_spent: self.verifier_spent,
            next_turn_is_verifier: self.next_turn_is_verifier,
            workspace_watch_bound: self.workspace_watch_bound,
            workspace_porcelain: self.workspace_porcelain.clone(),
            workspace_head: self.workspace_head.clone(),
            workspace_stamps: self.workspace_stamps.clone(),
        }
    }

    pub(super) fn turn_limit_error(&self, max_tool_rounds: usize) -> Option<String> {
        (self.turn > max_tool_rounds)
            .then(|| format!("Max tool rounds ({}) exceeded", max_tool_rounds))
    }

    pub(super) fn record_usage(&mut self, usage: &TokenUsage) {
        self.total_usage.accumulate(usage);
    }

    pub(super) fn record_tool_call(&mut self) {
        self.tool_calls_count += 1;
    }

    pub(super) fn duplicate_tool_call(
        &self,
        tool_name: &str,
        args: &Value,
        threshold: u32,
    ) -> Option<(usize, String)> {
        let signature = Self::tool_signature(tool_name, args);
        let duplicate_count = self
            .recent_tool_signatures
            .iter()
            .filter(|sig| sig.starts_with(&signature))
            .count();

        if duplicate_count < threshold as usize {
            return None;
        }

        Some((
            duplicate_count,
            format!(
                "Tool '{}' has been called {} times with identical arguments. \
                 Aborting to prevent infinite loop. Consider modifying your approach.",
                tool_name, duplicate_count
            ),
        ))
    }

    pub(super) fn record_duplicate_guard(&mut self, tool_name: &str, args: &Value) -> u32 {
        let signature = Self::tool_signature(tool_name, args);
        if self.guarded_duplicate_signature.as_deref() == Some(&signature) {
            self.guarded_duplicate_count += 1;
        } else {
            self.guarded_duplicate_signature = Some(signature);
            self.guarded_duplicate_count = 1;
        }
        self.guarded_duplicate_count
    }

    pub(super) fn reset_duplicate_guards(&mut self) {
        self.guarded_duplicate_signature = None;
        self.guarded_duplicate_count = 0;
    }

    /// Returns true when the model repeats the same incomplete no-tool reply.
    pub(super) fn repeated_incomplete_response(&mut self, text: &str) -> bool {
        let normalized = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let fingerprint = sha256::digest(normalized.as_bytes());
        let repeated = self.last_incomplete_response_hash.as_deref() == Some(&fingerprint);
        self.last_incomplete_response_hash = Some(fingerprint);
        self.incomplete_response_stalled = repeated;
        repeated
    }

    pub(super) fn incomplete_response_stalled(&self) -> bool {
        self.incomplete_response_stalled
    }

    pub(super) fn record_parse_error(
        &mut self,
        parse_error: &str,
        max_parse_retries: u32,
    ) -> ParseErrorOutcome {
        self.parse_error_count += 1;
        let output = format!("Error: {}", parse_error);
        let fatal_message = (self.parse_error_count > max_parse_retries).then(|| {
            format!(
                "LLM produced malformed tool arguments {} time(s) in a row \
                 (max_parse_retries={}); giving up",
                self.parse_error_count, max_parse_retries
            )
        });

        ParseErrorOutcome {
            output,
            count: self.parse_error_count,
            fatal_message,
        }
    }

    pub(super) fn reset_parse_errors(&mut self) {
        self.parse_error_count = 0;
    }

    pub(super) fn recent_tool_signatures(&self) -> Vec<String> {
        self.recent_tool_signatures.clone()
    }

    pub(super) fn remember_tool_signature(
        &mut self,
        tool_name: &str,
        args: &Value,
        is_error: bool,
    ) {
        self.recent_tool_signatures.push(format!(
            "{} => {}",
            Self::tool_signature(tool_name, args),
            if is_error { "error" } else { "ok" }
        ));

        if self.recent_tool_signatures.len() > RECENT_TOOL_SIGNATURE_LIMIT {
            let overflow = self.recent_tool_signatures.len() - RECENT_TOOL_SIGNATURE_LIMIT;
            self.recent_tool_signatures.drain(0..overflow);
        }
    }

    pub(super) fn should_inject_continuation(
        &mut self,
        looks_incomplete: bool,
        enabled: bool,
        max_continuation_turns: u32,
        max_tool_rounds: usize,
    ) -> bool {
        if enabled
            && self.continuation_count < max_continuation_turns
            && self.turn < max_tool_rounds
            && looks_incomplete
        {
            self.continuation_count += 1;
            return true;
        }

        false
    }

    pub(super) fn should_inject_reasoning_only_repair(
        &mut self,
        enabled: bool,
        max_tool_rounds: usize,
    ) -> bool {
        if enabled && self.reasoning_only_repair_count == 0 && self.turn < max_tool_rounds {
            self.reasoning_only_repair_count += 1;
            return true;
        }

        false
    }

    pub(super) fn finish(self, text: String) -> AgentResult {
        AgentResult {
            text,
            messages: self.messages,
            usage: self.total_usage,
            tool_calls_count: self.tool_calls_count,
            verification_reports: self.verification_reports,
            completion: crate::harness_loop::CompletionTerminal::Narrative,
            run_admission: "ordinary".to_string(),
        }
    }

    pub(super) fn finish_with(
        self,
        text: String,
        completion: crate::harness_loop::CompletionTerminal,
        run_admission: String,
    ) -> AgentResult {
        let mut result = self.finish(text);
        result.completion = completion;
        result.run_admission = run_admission;
        result
    }

    pub(super) fn finish_failed(self, error: anyhow::Error) -> anyhow::Error {
        anyhow::Error::new(super::AgentExecutionFailure::new(
            error,
            self.total_usage,
            self.tool_calls_count,
        ))
    }

    /// Build a result from a turn that was cancelled mid-generation. Keeps the
    /// conversation accumulated so far (the user's message above all) so the next
    /// turn remembers it. Appends a short assistant marker when the log would
    /// otherwise end on a user message, so the next user turn still alternates.
    pub(super) fn finish_interrupted(mut self) -> AgentResult {
        let ends_on_user = self
            .messages
            .last()
            .map(|m| m.role != "assistant")
            .unwrap_or(false);
        if ends_on_user {
            self.messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "(Response interrupted by the user.)".to_string(),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            });
        }
        AgentResult {
            text: String::new(),
            messages: self.messages,
            usage: self.total_usage,
            tool_calls_count: self.tool_calls_count,
            verification_reports: self.verification_reports,
            completion: crate::harness_loop::CompletionTerminal::Narrative,
            run_admission: "ordinary".to_string(),
        }
    }

    fn tool_signature(tool_name: &str, args: &Value) -> String {
        let encoded = serde_json::to_vec(args).unwrap_or_default();
        format!("{}:{}", tool_name, sha256::digest(encoded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn baseline_sees_an_unpublished_write_and_ignores_harness_metadata() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init"])
            .current_dir(workspace.path())
            .status()
            .unwrap()
            .success());
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(workspace.path()).await;
        std::fs::write(workspace.path().join("guest.txt"), "hello\n").unwrap();
        std::fs::create_dir_all(workspace.path().join(".a3s")).unwrap();
        std::fs::write(workspace.path().join(".a3s/note"), "harness\n").unwrap();
        std::fs::create_dir_all(workspace.path().join(".a3s-code/grep-trigram")).unwrap();
        std::fs::write(
            workspace.path().join(".a3s-code/grep-trigram/stamp.txt"),
            "index\n",
        )
        .unwrap();
        let paths = state.unseen_workspace_paths(workspace.path()).await.paths;
        assert!(
            paths.iter().any(|path| path == "guest.txt"),
            "baseline missed an unpublished write: {paths:?}"
        );
        assert!(
            paths.iter().all(|path| !path.contains(".a3s")),
            "harness metadata was treated as source: {paths:?}"
        );
    }

    #[tokio::test]
    async fn baseline_keeps_every_unpublished_path() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init"])
            .current_dir(workspace.path())
            .status()
            .unwrap()
            .success());
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(workspace.path()).await;
        for index in 0..33 {
            std::fs::write(workspace.path().join(format!("f{index:02}.txt")), "x\n").unwrap();
        }
        let observed = state.unseen_workspace_paths(workspace.path()).await;
        assert!(
            !observed.incomplete,
            "a readable git status is a complete observation"
        );
        assert!(
            observed.paths.iter().any(|path| path == "f32.txt"),
            "suffix past the old 32-path cap was dropped: {:?}",
            observed.paths
        );
        assert_eq!(observed.paths.len(), 33, "{:?}", observed.paths);
    }

    #[tokio::test]
    async fn baseline_sees_a_quoted_path() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init"])
            .current_dir(workspace.path())
            .status()
            .unwrap()
            .success());
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(workspace.path()).await;
        std::fs::write(workspace.path().join("my file.txt"), "x\n").unwrap();
        std::fs::write(workspace.path().join("说明.md"), "x\n").unwrap();
        let observed = state.unseen_workspace_paths(workspace.path()).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert!(
            observed.paths.iter().any(|path| path == "my file.txt"),
            "{:?}",
            observed.paths
        );
        assert!(
            observed.paths.iter().any(|path| path == "说明.md"),
            "{:?}",
            observed.paths
        );
    }

    #[tokio::test]
    async fn baseline_sees_a_rewrite_of_an_already_untracked_file() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init"])
            .current_dir(workspace.path())
            .status()
            .unwrap()
            .success());
        std::fs::write(workspace.path().join("guest.txt"), "one\n").unwrap();
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(workspace.path()).await;
        std::fs::write(workspace.path().join("guest.txt"), "one\ntwo\n").unwrap();
        let observed = state.unseen_workspace_paths(workspace.path()).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert!(
            observed.paths.iter().any(|path| path == "guest.txt"),
            "a stable porcelain line hid a rewrite: {:?}",
            observed.paths
        );
    }

    #[tokio::test]
    async fn baseline_sees_a_mode_change_on_an_already_dirty_file() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success(),
                "{args:?}"
            );
        };
        git(&["init"]);
        git(&["config", "user.email", "tests@a3s.local"]);
        git(&["config", "user.name", "A3S Tests"]);
        std::fs::write(root.join("guest.txt"), "one\n").unwrap();
        git(&["add", "guest.txt"]);
        git(&["commit", "-m", "init"]);
        std::fs::write(root.join("guest.txt"), "two\n").unwrap();
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(root).await;
        let path = root.join("guest.txt");
        let mut permissions = std::fs::symlink_metadata(&path).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o111);
        }
        #[cfg(not(unix))]
        {
            permissions.set_readonly(true);
        }
        std::fs::set_permissions(&path, permissions).unwrap();
        let observed = state.unseen_workspace_paths(root).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert!(
            observed.paths.iter().any(|path| path == "guest.txt"),
            "a mode change on a dirty file was not a mutation: {:?}",
            observed.paths
        );
    }

    #[tokio::test]
    async fn baseline_sees_a_write_git_status_hides() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success(),
                "{args:?}"
            );
        };
        git(&["init"]);
        git(&["config", "user.email", "tests@a3s.local"]);
        git(&["config", "user.name", "A3S Tests"]);
        std::fs::write(root.join("skipped.txt"), "one\n").unwrap();
        std::fs::write(root.join("assumed.txt"), "one\n").unwrap();
        git(&["add", "skipped.txt", "assumed.txt"]);
        git(&["commit", "-m", "init"]);
        git(&["update-index", "--skip-worktree", "skipped.txt"]);
        git(&["update-index", "--assume-unchanged", "assumed.txt"]);
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(root).await;
        std::fs::write(root.join("skipped.txt"), "two\n").unwrap();
        std::fs::write(root.join("assumed.txt"), "two\n").unwrap();
        let observed = state.unseen_workspace_paths(root).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert!(
            observed.paths.iter().any(|path| path == "skipped.txt"),
            "skip-worktree write was not a mutation: {:?}",
            observed.paths
        );
        assert!(
            observed.paths.iter().any(|path| path == "assumed.txt"),
            "assume-unchanged write was not a mutation: {:?}",
            observed.paths
        );
    }

    #[tokio::test]
    async fn nongit_baseline_sees_an_unpublished_write() {
        let workspace = tempfile::tempdir().unwrap();
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(workspace.path()).await;
        std::fs::write(workspace.path().join("guest.txt"), "hello\n").unwrap();
        let observed = state.unseen_workspace_paths(workspace.path()).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert_eq!(observed.paths, vec!["guest.txt".to_string()]);
    }

    #[tokio::test]
    async fn nongit_baseline_sees_a_mode_change_that_preserves_mtime() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let path = root.join("guest.txt");
        std::fs::write(&path, "hello\n").unwrap();
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(root).await;
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        let modified = metadata.modified().unwrap();
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() ^ 0o111);
        }
        #[cfg(not(unix))]
        {
            permissions.set_readonly(true);
        }
        std::fs::set_permissions(&path, permissions).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let observed = state.unseen_workspace_paths(root).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert!(
            observed.paths.iter().any(|entry| entry == "guest.txt"),
            "a mode change that preserved mtime was not a mutation: {:?}",
            observed.paths
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nongit_baseline_sees_a_new_symlink() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("guest.txt"), "hello\n").unwrap();
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(root).await;
        std::os::unix::fs::symlink("guest.txt", root.join("alias")).unwrap();
        let observed = state.unseen_workspace_paths(root).await;
        assert!(!observed.incomplete, "{observed:?}");
        assert!(
            observed.paths.iter().any(|entry| entry == "alias"),
            "a new symlink was not a mutation: {:?}",
            observed.paths
        );
    }

    #[tokio::test]
    async fn nongit_walk_past_the_file_cap_is_incomplete_not_an_empty_mutation() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        for index in 0..4_096 {
            std::fs::write(root.join(format!("f{index:04}.txt")), "x").unwrap();
        }
        let mut state = ExecutionLoopState::new(&[]);
        state.bind_run_watch(root).await;
        std::fs::write(root.join("overflow.txt"), "hidden\n").unwrap();
        let observed = state.unseen_workspace_paths(root).await;
        assert!(
            observed.incomplete,
            "a truncated non-git walk was treated as a complete observation: {observed:?}"
        );
        assert!(
            observed.paths.is_empty(),
            "a truncated walk must not invent paths: {:?}",
            observed.paths
        );
    }

    #[test]
    fn finish_interrupted_keeps_user_message_and_alternates() {
        // A cancelled turn must keep the user's message (so the next turn
        // remembers it) and end on an assistant message (so it still alternates).
        let mut state = ExecutionLoopState::new(&[]);
        state.messages.push(Message::user("what is the plan?"));
        let result = state.finish_interrupted();
        assert!(
            result
                .messages
                .iter()
                .any(|m| m.role == "user" && m.text().contains("what is the plan?")),
            "user message must survive the interrupt"
        );
        assert_eq!(
            result.messages.last().unwrap().role,
            "assistant",
            "history must end on an assistant message to alternate"
        );
        assert!(result.text.is_empty());
    }

    #[test]
    fn finish_interrupted_keeps_history_when_it_ends_on_assistant() {
        // A partial assistant reply was already recorded: don't append a marker.
        let state =
            ExecutionLoopState::new(&[Message::user("do X"), Message::assistant("partial answer")]);
        let result = state.finish_interrupted();
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.messages[1].role.as_str(), "assistant");
        assert_eq!(result.messages[1].text(), "partial answer");
    }

    #[test]
    fn usage_accumulation_keeps_cache_tokens_and_saturates() {
        let mut state = ExecutionLoopState::new(&[]);
        state.record_usage(&TokenUsage {
            prompt_tokens: usize::MAX,
            completion_tokens: 2,
            total_tokens: usize::MAX,
            cache_read_tokens: Some(3),
            cache_write_tokens: None,
        });
        state.record_usage(&TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 4,
            total_tokens: 1,
            cache_read_tokens: Some(5),
            cache_write_tokens: Some(7),
        });

        assert_eq!(state.total_usage.prompt_tokens, usize::MAX);
        assert_eq!(state.total_usage.completion_tokens, 6);
        assert_eq!(state.total_usage.total_tokens, usize::MAX);
        assert_eq!(state.total_usage.cache_read_tokens, Some(8));
        assert_eq!(state.total_usage.cache_write_tokens, Some(7));
    }

    #[test]
    fn duplicate_tool_call_uses_recent_success_and_error_signatures() {
        let mut state = ExecutionLoopState::new(&[]);
        let args = json!({"path":"src/lib.rs"});

        state.remember_tool_signature("read_file", &args, false);
        state.remember_tool_signature("read_file", &args, true);

        let duplicate = state.duplicate_tool_call("read_file", &args, 2).unwrap();
        assert_eq!(duplicate.0, 2);
        assert!(duplicate.1.contains("read_file"));
    }

    #[test]
    fn parse_error_becomes_fatal_after_retry_budget() {
        let mut state = ExecutionLoopState::new(&[]);

        let first = state.record_parse_error("bad json", 1);
        assert_eq!(first.count, 1);
        assert!(first.fatal_message.is_none());

        let second = state.record_parse_error("bad json", 1);
        assert_eq!(second.count, 2);
        assert!(second
            .fatal_message
            .unwrap()
            .contains("max_parse_retries=1"));
    }

    #[test]
    fn continuation_budget_is_consumed_only_when_incomplete() {
        let mut state = ExecutionLoopState::new(&[]);
        state.next_turn();

        assert!(state.should_inject_continuation(true, true, 1, 5));
        assert!(!state.should_inject_continuation(true, true, 1, 5));
        assert!(!state.should_inject_continuation(false, true, 2, 5));
    }

    #[test]
    fn repeated_incomplete_response_ignores_whitespace_and_case() {
        let mut state = ExecutionLoopState::new(&[]);
        assert!(!state.repeated_incomplete_response("Let me inspect the code..."));
        assert!(state.repeated_incomplete_response("  LET   ME inspect the code...  "));
    }

    #[test]
    fn duplicate_guard_count_resets_after_progress() {
        let mut state = ExecutionLoopState::new(&[]);
        let args = json!({"path": "README.md"});
        assert_eq!(state.record_duplicate_guard("read", &args), 1);
        assert_eq!(state.record_duplicate_guard("read", &args), 2);
        state.reset_duplicate_guards();
        assert_eq!(state.record_duplicate_guard("read", &args), 1);
    }

    #[test]
    fn checkpoint_seed_restores_turn_budgets_and_redacted_convergence_fingerprints() {
        let args = json!({"token": "must-not-persist", "path": "README.md"});
        let mut original = ExecutionLoopState::new(&[]);
        original.next_turn();
        original.next_turn();
        original.remember_tool_signature("read", &args, false);
        original.remember_tool_signature("read", &args, true);
        assert_eq!(original.record_duplicate_guard("read", &args), 1);
        assert!(original.should_inject_continuation(true, true, 1, 10));
        assert!(original
            .record_parse_error("bad json", 1)
            .fatal_message
            .is_none());
        assert!(!original.repeated_incomplete_response("secret response must-not-persist"));
        let convergence = original.convergence_checkpoint();
        let encoded = serde_json::to_string(&convergence).unwrap();
        assert!(!encoded.contains("must-not-persist"));

        let mut resumed = ExecutionLoopState::new_seeded(
            &[],
            Some(ExecutionSeed {
                turn: original.current_turn(),
                convergence,
                ..ExecutionSeed::default()
            }),
        );
        assert_eq!(resumed.next_turn(), 3);
        assert!(resumed.duplicate_tool_call("read", &args, 2).is_some());
        assert_eq!(resumed.record_duplicate_guard("read", &args), 2);
        assert!(!resumed.should_inject_continuation(true, true, 1, 10));
        assert!(resumed
            .record_parse_error("bad json", 1)
            .fatal_message
            .is_some());
    }
}
