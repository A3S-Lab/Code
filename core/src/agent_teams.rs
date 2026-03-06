//! Agent Teams — Peer-to-peer multi-agent coordination
//!
//! Enables multiple `AgentSession` instances to collaborate on complex tasks
//! through a shared task board and message passing. Each agent has a role
//! (Lead, Worker, Reviewer) and can post/claim tasks on the board.
//!
//! ## Architecture
//!
//! ```text
//! AgentTeam
//!   +-- TeamTaskBoard (shared task queue)
//!   +-- TeamMember[] (role + session reference)
//!   +-- mpsc channels (peer-to-peer messaging)
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use a3s_code_core::agent_teams::{AgentTeam, TeamConfig, TeamRole};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let config = TeamConfig::default();
//! let mut team = AgentTeam::new("refactor-auth", config);
//!
//! // Add members (each wraps an AgentSession)
//! team.add_member("lead", TeamRole::Lead);
//! team.add_member("worker-1", TeamRole::Worker);
//! team.add_member("reviewer", TeamRole::Reviewer);
//!
//! // Post a task to the board
//! team.task_board().post("Refactor auth module", "lead", None);
//!
//! // Worker claims and works on it
//! let task = team.task_board().claim("worker-1");
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// Team configuration.
#[derive(Debug, Clone)]
pub struct TeamConfig {
    /// Maximum concurrent tasks on the board.
    /// Default: 50
    pub max_tasks: usize,
    /// Message channel buffer size.
    /// Default: 128
    pub channel_buffer: usize,
    /// Maximum coordination rounds before `run_until_done` exits.
    /// Default: 10
    pub max_rounds: usize,
    /// Worker/Reviewer polling interval in milliseconds.
    /// Default: 200
    pub poll_interval_ms: u64,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            max_tasks: 50,
            channel_buffer: 128,
            max_rounds: 10,
            poll_interval_ms: 200,
        }
    }
}

/// Role of a team member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TeamRole {
    /// Decomposes goals into tasks, assigns work.
    Lead,
    /// Executes assigned tasks.
    Worker,
    /// Reviews completed work, provides feedback.
    Reviewer,
}

impl std::fmt::Display for TeamRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamRole::Lead => write!(f, "lead"),
            TeamRole::Worker => write!(f, "worker"),
            TeamRole::Reviewer => write!(f, "reviewer"),
        }
    }
}

/// A message passed between team members.
#[derive(Debug, Clone)]
pub struct TeamMessage {
    /// Sender member ID.
    pub from: String,
    /// Recipient member ID.
    pub to: String,
    /// Message content.
    pub content: String,
    /// Optional task ID this message relates to.
    pub task_id: Option<String>,
    /// Timestamp (Unix epoch seconds).
    pub timestamp: i64,
}

/// Task status on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Waiting to be claimed.
    Open,
    /// Claimed by a worker.
    InProgress,
    /// Work done, awaiting review.
    InReview,
    /// Approved by reviewer.
    Done,
    /// Rejected, needs rework.
    Rejected,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Open => write!(f, "open"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::InReview => write!(f, "in_review"),
            TaskStatus::Done => write!(f, "done"),
            TaskStatus::Rejected => write!(f, "rejected"),
        }
    }
}

/// A task on the team board.
#[derive(Debug, Clone)]
pub struct TeamTask {
    /// Unique task ID.
    pub id: String,
    /// Task description.
    pub description: String,
    /// Who posted it.
    pub posted_by: String,
    /// Who is working on it (if claimed).
    pub assigned_to: Option<String>,
    /// Current status.
    pub status: TaskStatus,
    /// Optional result/output when completed.
    pub result: Option<String>,
    /// Created timestamp.
    pub created_at: i64,
    /// Last updated timestamp.
    pub updated_at: i64,
}

/// Shared task board for team coordination.
#[derive(Debug)]
pub struct TeamTaskBoard {
    tasks: RwLock<Vec<TeamTask>>,
    max_tasks: usize,
    next_id: RwLock<u64>,
}

impl TeamTaskBoard {
    /// Create a new task board.
    pub fn new(max_tasks: usize) -> Self {
        Self {
            tasks: RwLock::new(Vec::new()),
            max_tasks,
            next_id: RwLock::new(1),
        }
    }

    /// Post a new task to the board. Returns the task ID.
    pub fn post(
        &self,
        description: &str,
        posted_by: &str,
        assign_to: Option<&str>,
    ) -> Option<String> {
        let mut tasks = self.tasks.write().unwrap();
        if tasks.len() >= self.max_tasks {
            return None;
        }

        let mut id_counter = self.next_id.write().unwrap();
        let id = format!("task-{}", *id_counter);
        *id_counter += 1;

        let now = chrono::Utc::now().timestamp();
        let status = if assign_to.is_some() {
            TaskStatus::InProgress
        } else {
            TaskStatus::Open
        };

        tasks.push(TeamTask {
            id: id.clone(),
            description: description.to_string(),
            posted_by: posted_by.to_string(),
            assigned_to: assign_to.map(|s| s.to_string()),
            status,
            result: None,
            created_at: now,
            updated_at: now,
        });

        Some(id)
    }

    /// Claim the next open or rejected task for a member. Returns the task if available.
    ///
    /// Rejected tasks are treated as retriable: a worker can claim them again
    /// for another execution attempt.
    pub fn claim(&self, member_id: &str) -> Option<TeamTask> {
        let mut tasks = self.tasks.write().unwrap();
        let task = tasks
            .iter_mut()
            .find(|t| t.status == TaskStatus::Open || t.status == TaskStatus::Rejected)?;
        task.assigned_to = Some(member_id.to_string());
        task.status = TaskStatus::InProgress;
        task.updated_at = chrono::Utc::now().timestamp();
        Some(task.clone())
    }

    /// Mark a task as complete with a result.
    pub fn complete(&self, task_id: &str, result: &str) -> bool {
        let mut tasks = self.tasks.write().unwrap();
        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TaskStatus::InReview;
            task.result = Some(result.to_string());
            task.updated_at = chrono::Utc::now().timestamp();
            true
        } else {
            false
        }
    }

    /// Approve a task (reviewer action).
    pub fn approve(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.write().unwrap();
        if let Some(task) = tasks
            .iter_mut()
            .find(|t| t.id == task_id && t.status == TaskStatus::InReview)
        {
            task.status = TaskStatus::Done;
            task.updated_at = chrono::Utc::now().timestamp();
            true
        } else {
            false
        }
    }

    /// Reject a task back to open (reviewer action).
    pub fn reject(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.write().unwrap();
        if let Some(task) = tasks
            .iter_mut()
            .find(|t| t.id == task_id && t.status == TaskStatus::InReview)
        {
            task.status = TaskStatus::Rejected;
            task.assigned_to = None;
            task.updated_at = chrono::Utc::now().timestamp();
            true
        } else {
            false
        }
    }

    /// Get all tasks with a given status.
    pub fn by_status(&self, status: TaskStatus) -> Vec<TeamTask> {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .filter(|t| t.status == status)
            .cloned()
            .collect()
    }

    /// Get all tasks assigned to a member.
    pub fn by_assignee(&self, member_id: &str) -> Vec<TeamTask> {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .filter(|t| t.assigned_to.as_deref() == Some(member_id))
            .cloned()
            .collect()
    }

    /// Get a task by ID.
    pub fn get(&self, task_id: &str) -> Option<TeamTask> {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .find(|t| t.id == task_id)
            .cloned()
    }

    /// Number of tasks on the board.
    pub fn len(&self) -> usize {
        self.tasks.read().unwrap().len()
    }

    /// Whether the board is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.read().unwrap().is_empty()
    }

    /// Summary stats: (open, in_progress, in_review, done, rejected).
    pub fn stats(&self) -> (usize, usize, usize, usize, usize) {
        let tasks = self.tasks.read().unwrap();
        let open = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Open)
            .count();
        let progress = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let review = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InReview)
            .count();
        let done = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .count();
        let rejected = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Rejected)
            .count();
        (open, progress, review, done, rejected)
    }
}

/// A team member.
#[derive(Debug, Clone)]
pub struct TeamMember {
    /// Unique member ID.
    pub id: String,
    /// Member role.
    pub role: TeamRole,
}

/// Multi-agent team coordinator.
pub struct AgentTeam {
    /// Team name.
    name: String,
    /// Configuration.
    config: TeamConfig,
    /// Registered members.
    members: HashMap<String, TeamMember>,
    /// Shared task board.
    task_board: Arc<TeamTaskBoard>,
    /// Message senders per member.
    senders: HashMap<String, mpsc::Sender<TeamMessage>>,
    /// Message receivers per member (taken on first access).
    receivers: HashMap<String, mpsc::Receiver<TeamMessage>>,
}

impl AgentTeam {
    /// Create a new team.
    pub fn new(name: &str, config: TeamConfig) -> Self {
        Self {
            name: name.to_string(),
            config,
            members: HashMap::new(),
            task_board: Arc::new(TeamTaskBoard::new(50)),
            senders: HashMap::new(),
            receivers: HashMap::new(),
        }
    }

    /// Team name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Add a member to the team.
    pub fn add_member(&mut self, id: &str, role: TeamRole) {
        let (tx, rx) = mpsc::channel(self.config.channel_buffer);
        self.members.insert(
            id.to_string(),
            TeamMember {
                id: id.to_string(),
                role,
            },
        );
        self.senders.insert(id.to_string(), tx);
        self.receivers.insert(id.to_string(), rx);
    }

    /// Remove a member from the team.
    pub fn remove_member(&mut self, id: &str) -> bool {
        self.senders.remove(id);
        self.receivers.remove(id);
        self.members.remove(id).is_some()
    }

    /// Get a reference to the shared task board.
    pub fn task_board(&self) -> &TeamTaskBoard {
        &self.task_board
    }

    /// Get a cloneable Arc to the task board.
    pub fn task_board_arc(&self) -> Arc<TeamTaskBoard> {
        Arc::clone(&self.task_board)
    }

    /// Send a message to a team member.
    pub async fn send_message(
        &self,
        from: &str,
        to: &str,
        content: &str,
        task_id: Option<&str>,
    ) -> bool {
        let sender = match self.senders.get(to) {
            Some(s) => s,
            None => return false,
        };

        let msg = TeamMessage {
            from: from.to_string(),
            to: to.to_string(),
            content: content.to_string(),
            task_id: task_id.map(|s| s.to_string()),
            timestamp: chrono::Utc::now().timestamp(),
        };

        sender.send(msg).await.is_ok()
    }

    /// Take the message receiver for a member (can only be called once per member).
    pub fn take_receiver(&mut self, member_id: &str) -> Option<mpsc::Receiver<TeamMessage>> {
        self.receivers.remove(member_id)
    }

    /// Broadcast a message to all members except the sender.
    pub async fn broadcast(&self, from: &str, content: &str, task_id: Option<&str>) {
        for (id, sender) in &self.senders {
            if id == from {
                continue;
            }
            let msg = TeamMessage {
                from: from.to_string(),
                to: id.clone(),
                content: content.to_string(),
                task_id: task_id.map(|s| s.to_string()),
                timestamp: chrono::Utc::now().timestamp(),
            };
            let _ = sender.send(msg).await;
        }
    }

    /// Get all members.
    pub fn members(&self) -> Vec<&TeamMember> {
        self.members.values().collect()
    }

    /// Get members by role.
    pub fn members_by_role(&self, role: TeamRole) -> Vec<&TeamMember> {
        self.members.values().filter(|m| m.role == role).collect()
    }

    /// Number of members.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }
}

impl std::fmt::Debug for AgentTeam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTeam")
            .field("name", &self.name)
            .field("members", &self.members.len())
            .field("tasks", &self.task_board.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// AgentExecutor — abstraction over AgentSession for testability
// ---------------------------------------------------------------------------

/// Minimal execution interface for a team member.
///
/// `AgentSession` implements this trait. In tests, a mock can be used instead.
#[async_trait::async_trait]
pub trait AgentExecutor: Send + Sync {
    /// Execute a prompt and return the text response.
    async fn execute(&self, prompt: &str) -> crate::error::Result<String>;
}

#[async_trait::async_trait]
impl AgentExecutor for crate::agent_api::AgentSession {
    async fn execute(&self, prompt: &str) -> crate::error::Result<String> {
        let result = self.send(prompt, None).await?;
        Ok(result.text)
    }
}

// ---------------------------------------------------------------------------
// TeamRunResult
// ---------------------------------------------------------------------------

/// Result returned by `TeamRunner::run_until_done`.
#[derive(Debug)]
pub struct TeamRunResult {
    /// Tasks that reached the `Done` state.
    pub done_tasks: Vec<TeamTask>,
    /// Tasks that are still `Rejected` after all rounds (could not be approved).
    pub rejected_tasks: Vec<TeamTask>,
    /// Number of reviewer polling rounds completed.
    pub rounds: usize,
}

// ---------------------------------------------------------------------------
// TeamRunner — binds AgentSession execution to AgentTeam coordination
// ---------------------------------------------------------------------------

const LEAD_PROMPT: &str = "You are the lead agent in a team. Your goal is: {goal}

Decompose this goal into concrete, self-contained tasks for your team workers.
Each task should be independently executable by an AI coding agent.

Respond with ONLY valid JSON in this exact format:
{{\"tasks\": [\"task description 1\", \"task description 2\", ...]}}

No markdown, no explanation, just the JSON.";

const REVIEWER_PROMPT: &str = "Review the following completed task:

Task: {task}
Result: {result}

If the result satisfactorily completes the task, respond with \"APPROVED: <brief reason>\".
If the result is incomplete or incorrect, respond with \"REJECTED: <specific feedback for improvement>\".";

/// Binds an `AgentTeam` to concrete `AgentExecutor` sessions, enabling
/// Lead → Worker → Reviewer automated workflows.
pub struct TeamRunner {
    team: AgentTeam,
    sessions: HashMap<String, Arc<dyn AgentExecutor>>,
}

impl TeamRunner {
    /// Create a new runner wrapping the given team.
    pub fn new(team: AgentTeam) -> Self {
        Self {
            team,
            sessions: HashMap::new(),
        }
    }

    /// Bind an executor to a team member.
    ///
    /// Returns an error if `member_id` is not registered in the team.
    pub fn bind_session(
        &mut self,
        member_id: &str,
        executor: Arc<dyn AgentExecutor>,
    ) -> crate::error::Result<()> {
        if !self.team.members.contains_key(member_id) {
            return Err(anyhow::anyhow!(
                "member '{}' not found in team '{}'",
                member_id,
                self.team.name()
            )
            .into());
        }
        self.sessions.insert(member_id.to_string(), executor);
        Ok(())
    }

    /// Bind a team member to a session created from a named agent definition.
    ///
    /// Looks up `agent_name` in `registry`, applies the definition's prompt,
    /// permissions, model, and `max_steps` to a new [`AgentSession`] via
    /// [`Agent::session_for_agent`], then binds it to `member_id`.
    ///
    /// This is the primary integration point between the `AgentTeam` coordination
    /// layer and the markdown/YAML-defined subagent capability layer.
    ///
    /// # Errors
    ///
    /// Returns an error if `member_id` is not in the team, `agent_name` is not
    /// in `registry`, or session creation fails.
    pub fn bind_agent(
        &mut self,
        member_id: &str,
        agent: &crate::agent_api::Agent,
        workspace: &str,
        agent_name: &str,
        registry: &crate::subagent::AgentRegistry,
    ) -> crate::error::Result<()> {
        let def = registry
            .get(agent_name)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in registry", agent_name))?;
        let session = agent.session_for_agent(workspace, &def, None)?;
        self.bind_session(member_id, Arc::new(session))
    }

    /// Access the shared task board.
    pub fn task_board(&self) -> Arc<TeamTaskBoard> {
        self.team.task_board_arc()
    }

    /// Run the full Lead → Worker → Reviewer workflow until all tasks are done
    /// or `max_rounds` is exceeded.
    ///
    /// Steps:
    /// 1. Lead decomposes `goal` into tasks via JSON response.
    /// 2. Workers concurrently claim and execute tasks.
    /// 3. Reviewer approves or rejects completed tasks.
    /// 4. Rejected tasks re-enter the work queue for retry.
    pub async fn run_until_done(&self, goal: &str) -> crate::error::Result<TeamRunResult> {
        // --- Step 1: Lead decomposes the goal ---
        let lead = self
            .team
            .members_by_role(TeamRole::Lead)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("team has no Lead member"))?;

        let lead_executor = self
            .sessions
            .get(&lead.id)
            .ok_or_else(|| anyhow::anyhow!("no executor bound for lead member '{}'", lead.id))?;

        let lead_prompt = LEAD_PROMPT.replace("{goal}", goal);
        let raw = lead_executor.execute(&lead_prompt).await?;
        let task_descriptions = parse_task_list(&raw)?;

        let board = self.team.task_board_arc();
        for desc in &task_descriptions {
            board.post(desc, &lead.id, None);
        }

        // --- Step 2 & 3: Spawn workers and reviewer concurrently ---
        let poll = Duration::from_millis(self.team.config.poll_interval_ms);
        let max_rounds = self.team.config.max_rounds;

        let workers: Vec<(String, Arc<dyn AgentExecutor>)> = self
            .team
            .members_by_role(TeamRole::Worker)
            .into_iter()
            .filter_map(|m| {
                self.sessions
                    .get(&m.id)
                    .map(|e| (m.id.clone(), Arc::clone(e)))
            })
            .collect();

        let reviewer: Option<(String, Arc<dyn AgentExecutor>)> = self
            .team
            .members_by_role(TeamRole::Reviewer)
            .into_iter()
            .next()
            .and_then(|m| {
                self.sessions
                    .get(&m.id)
                    .map(|e| (m.id.clone(), Arc::clone(e)))
            });

        let mut worker_handles = Vec::new();
        for (id, executor) in workers {
            let b = Arc::clone(&board);
            let handle = tokio::spawn(async move {
                run_worker(id, executor, b, max_rounds, poll).await;
            });
            worker_handles.push(handle);
        }

        let reviewer_rounds = if let Some((id, executor)) = reviewer {
            let b = Arc::clone(&board);
            let handle =
                tokio::spawn(async move { run_reviewer(id, executor, b, max_rounds, poll).await });
            for h in worker_handles {
                let _ = h.await;
            }
            handle.await.unwrap_or(0)
        } else {
            for h in worker_handles {
                let _ = h.await;
            }
            0
        };

        let done_tasks = board.by_status(TaskStatus::Done);
        let rejected_tasks = board.by_status(TaskStatus::Rejected);

        Ok(TeamRunResult {
            done_tasks,
            rejected_tasks,
            rounds: reviewer_rounds,
        })
    }
}

impl std::fmt::Debug for TeamRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamRunner")
            .field("team", &self.team.name())
            .field("bound_sessions", &self.sessions.len())
            .finish()
    }
}

/// Parse `{"tasks": ["...", "..."]}` from a Lead response.
fn parse_task_list(response: &str) -> crate::error::Result<Vec<String>> {
    // Find JSON object boundaries in case there is extra whitespace or newlines.
    let start = response
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("lead response contains no JSON object: {}", response))?;
    let end = response
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("lead response JSON object is unclosed"))?;
    let json_str = &response[start..=end];

    let value: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| anyhow::anyhow!("failed to parse lead JSON response: {e}"))?;

    let tasks: Vec<String> = value["tasks"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("lead JSON response missing 'tasks' array"))?
        .iter()
        .filter_map(|v: &serde_json::Value| v.as_str().map(|s| s.to_string()))
        .collect();

    Ok(tasks)
}

/// Worker loop: claim and execute tasks until the board is quiescent.
async fn run_worker(
    member_id: String,
    executor: Arc<dyn AgentExecutor>,
    board: Arc<TeamTaskBoard>,
    max_rounds: usize,
    poll: Duration,
) {
    let mut idle = 0usize;
    loop {
        if let Some(task) = board.claim(&member_id) {
            idle = 0;
            let result = executor
                .execute(&task.description)
                .await
                .unwrap_or_else(|e| format!("execution error: {e}"));
            board.complete(&task.id, &result);
        } else {
            let (open, in_progress, in_review, _, rejected) = board.stats();
            // No claimable work and no tasks that could re-enter the queue → stop.
            // Include in_review so we wait for the reviewer's verdict (which may
            // produce a Rejected task for us to retry).
            if open == 0 && in_progress == 0 && in_review == 0 && rejected == 0 {
                break;
            }
            idle += 1;
            if idle >= max_rounds {
                break;
            }
            tokio::time::sleep(poll).await;
        }
    }
}

/// Reviewer loop: review InReview tasks and approve or reject them.
/// Returns the number of rounds completed.
async fn run_reviewer(
    _member_id: String,
    executor: Arc<dyn AgentExecutor>,
    board: Arc<TeamTaskBoard>,
    max_rounds: usize,
    poll: Duration,
) -> usize {
    let mut rounds = 0usize;
    loop {
        let in_review = board.by_status(TaskStatus::InReview);
        for task in in_review {
            let result_text = task.result.as_deref().unwrap_or("");
            let prompt = REVIEWER_PROMPT
                .replace("{task}", &task.description)
                .replace("{result}", result_text);
            let verdict = executor
                .execute(&prompt)
                .await
                .unwrap_or_else(|_| "REJECTED: execution error".to_string());
            if verdict.contains("APPROVED") {
                board.approve(&task.id);
            } else {
                board.reject(&task.id);
            }
            // Yield after each decision so workers can pick up rejected tasks.
            tokio::task::yield_now().await;
        }

        let (open, in_progress, in_review_count, _, rejected) = board.stats();
        if open == 0 && in_progress == 0 && in_review_count == 0 && rejected == 0 {
            break;
        }
        rounds += 1;
        if rounds >= max_rounds {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    rounds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_team_creation() {
        let team = AgentTeam::new("test-team", TeamConfig::default());
        assert_eq!(team.name(), "test-team");
        assert_eq!(team.member_count(), 0);
    }

    #[test]
    fn test_add_remove_members() {
        let mut team = AgentTeam::new("test", TeamConfig::default());
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);
        team.add_member("w2", TeamRole::Worker);
        team.add_member("rev", TeamRole::Reviewer);
        assert_eq!(team.member_count(), 4);
        assert_eq!(team.members_by_role(TeamRole::Worker).len(), 2);

        assert!(team.remove_member("w2"));
        assert_eq!(team.member_count(), 3);
        assert!(!team.remove_member("nonexistent"));
    }

    #[test]
    fn test_task_board_post_and_claim() {
        let board = TeamTaskBoard::new(10);
        let id = board.post("Fix auth bug", "lead", None).unwrap();
        assert_eq!(board.len(), 1);

        let task = board.claim("worker-1").unwrap();
        assert_eq!(task.id, id);
        assert_eq!(task.assigned_to.as_deref(), Some("worker-1"));
        assert_eq!(task.status, TaskStatus::InProgress);

        // No more open tasks
        assert!(board.claim("worker-2").is_none());
    }

    #[test]
    fn test_task_board_workflow() {
        let board = TeamTaskBoard::new(10);
        let id = board.post("Write tests", "lead", None).unwrap();

        // Claim
        board.claim("worker-1");

        // Complete
        assert!(board.complete(&id, "Added 5 tests"));
        let task = board.get(&id).unwrap();
        assert_eq!(task.status, TaskStatus::InReview);

        // Approve
        assert!(board.approve(&id));
        let task = board.get(&id).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
    }

    #[test]
    fn test_task_board_reject() {
        let board = TeamTaskBoard::new(10);
        let id = board.post("Refactor module", "lead", None).unwrap();
        board.claim("worker-1");
        board.complete(&id, "Done");

        assert!(board.reject(&id));
        let task = board.get(&id).unwrap();
        assert_eq!(task.status, TaskStatus::Rejected);
        assert!(task.assigned_to.is_none());
    }

    #[test]
    fn test_task_board_max_capacity() {
        let board = TeamTaskBoard::new(2);
        assert!(board.post("Task 1", "lead", None).is_some());
        assert!(board.post("Task 2", "lead", None).is_some());
        assert!(board.post("Task 3", "lead", None).is_none()); // Full
    }

    #[test]
    fn test_task_board_stats() {
        let board = TeamTaskBoard::new(10);
        board.post("T1", "lead", None);
        board.post("T2", "lead", None);
        let id3 = board.post("T3", "lead", Some("w1")).unwrap();
        board.complete(&id3, "done");

        let (open, progress, review, done, rejected) = board.stats();
        assert_eq!(open, 2);
        assert_eq!(progress, 0);
        assert_eq!(review, 1);
        assert_eq!(done, 0);
        assert_eq!(rejected, 0);
    }

    #[test]
    fn test_task_board_by_assignee() {
        let board = TeamTaskBoard::new(10);
        board.post("T1", "lead", Some("w1"));
        board.post("T2", "lead", Some("w2"));
        board.post("T3", "lead", Some("w1"));

        let w1_tasks = board.by_assignee("w1");
        assert_eq!(w1_tasks.len(), 2);
    }

    #[tokio::test]
    async fn test_send_message() {
        let mut team = AgentTeam::new("msg-test", TeamConfig::default());
        team.add_member("lead", TeamRole::Lead);
        team.add_member("worker", TeamRole::Worker);

        let mut rx = team.take_receiver("worker").unwrap();

        assert!(
            team.send_message("lead", "worker", "Please fix the bug", Some("task-1"))
                .await
        );

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.from, "lead");
        assert_eq!(msg.to, "worker");
        assert_eq!(msg.content, "Please fix the bug");
        assert_eq!(msg.task_id.as_deref(), Some("task-1"));
    }

    #[tokio::test]
    async fn test_broadcast() {
        let mut team = AgentTeam::new("broadcast-test", TeamConfig::default());
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);
        team.add_member("w2", TeamRole::Worker);

        let mut rx1 = team.take_receiver("w1").unwrap();
        let mut rx2 = team.take_receiver("w2").unwrap();

        team.broadcast("lead", "New task available", None).await;

        let m1 = rx1.recv().await.unwrap();
        let m2 = rx2.recv().await.unwrap();
        assert_eq!(m1.content, "New task available");
        assert_eq!(m2.content, "New task available");
    }

    #[test]
    fn test_role_display() {
        assert_eq!(TeamRole::Lead.to_string(), "lead");
        assert_eq!(TeamRole::Worker.to_string(), "worker");
        assert_eq!(TeamRole::Reviewer.to_string(), "reviewer");
    }

    #[test]
    fn test_task_status_display() {
        assert_eq!(TaskStatus::Open.to_string(), "open");
        assert_eq!(TaskStatus::InProgress.to_string(), "in_progress");
        assert_eq!(TaskStatus::InReview.to_string(), "in_review");
        assert_eq!(TaskStatus::Done.to_string(), "done");
        assert_eq!(TaskStatus::Rejected.to_string(), "rejected");
    }

    // -----------------------------------------------------------------------
    // TeamRunner tests (mock executor)
    // -----------------------------------------------------------------------

    /// Minimal mock executor for unit tests.
    struct MockExecutor {
        response: String,
    }

    impl MockExecutor {
        fn new(response: impl Into<String>) -> Arc<Self> {
            Arc::new(Self {
                response: response.into(),
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentExecutor for MockExecutor {
        async fn execute(&self, _prompt: &str) -> crate::error::Result<String> {
            Ok(self.response.clone())
        }
    }

    #[test]
    fn test_team_runner_session_binding() {
        let mut team = AgentTeam::new("bind-test", TeamConfig::default());
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);

        let mut runner = TeamRunner::new(team);

        // Binding to a known member succeeds.
        assert!(runner.bind_session("lead", MockExecutor::new("ok")).is_ok());
        assert!(runner.bind_session("w1", MockExecutor::new("ok")).is_ok());

        // Binding to an unknown member fails.
        assert!(runner
            .bind_session("nobody", MockExecutor::new("ok"))
            .is_err());
    }

    #[test]
    fn test_parse_task_list() {
        let json = r#"{"tasks": ["Write tests", "Fix lints", "Update docs"]}"#;
        let tasks = parse_task_list(json).unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0], "Write tests");
        assert_eq!(tasks[2], "Update docs");
    }

    #[test]
    fn test_parse_task_list_no_json() {
        assert!(parse_task_list("no json here").is_err());
    }

    #[test]
    fn test_claim_rejected_tasks() {
        let board = TeamTaskBoard::new(10);
        let id = board.post("Refactor module", "lead", None).unwrap();

        // Simulate workflow: claim → complete → reject
        board.claim("worker-1");
        board.complete(&id, "initial attempt");
        board.reject(&id);

        assert_eq!(board.get(&id).unwrap().status, TaskStatus::Rejected);

        // A new worker can re-claim the rejected task.
        let task = board.claim("worker-2");
        assert!(task.is_some());
        let task = task.unwrap();
        assert_eq!(task.id, id);
        assert_eq!(task.assigned_to.as_deref(), Some("worker-2"));
        assert_eq!(task.status, TaskStatus::InProgress);
    }

    #[tokio::test]
    async fn test_team_runner_goal_decomposition() {
        let config = TeamConfig {
            poll_interval_ms: 1,
            max_rounds: 3,
            ..TeamConfig::default()
        };
        let mut team = AgentTeam::new("decomp-test", config);
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);
        team.add_member("rev", TeamRole::Reviewer);

        let mut runner = TeamRunner::new(team);
        // Lead returns two tasks as JSON.
        runner
            .bind_session(
                "lead",
                MockExecutor::new(r#"{"tasks": ["Task A", "Task B"]}"#),
            )
            .unwrap();
        // Worker always succeeds.
        runner
            .bind_session("w1", MockExecutor::new("done"))
            .unwrap();
        // Reviewer always approves.
        runner
            .bind_session("rev", MockExecutor::new("APPROVED: looks good"))
            .unwrap();

        let result = runner.run_until_done("Build the feature").await.unwrap();

        assert_eq!(result.done_tasks.len(), 2);
        assert!(result.rejected_tasks.is_empty());
    }

    #[tokio::test]
    async fn test_team_runner_worker_execution() {
        let config = TeamConfig {
            poll_interval_ms: 1,
            max_rounds: 3,
            ..TeamConfig::default()
        };
        let mut team = AgentTeam::new("worker-exec-test", config);
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);

        let mut runner = TeamRunner::new(team);
        runner
            .bind_session(
                "lead",
                MockExecutor::new(r#"{"tasks": ["Write unit tests"]}"#),
            )
            .unwrap();
        runner
            .bind_session("w1", MockExecutor::new("Added 3 tests"))
            .unwrap();

        // No reviewer bound → tasks end up InReview (not Done), which is fine for this test.
        let board = runner.task_board();
        let _ = runner.run_until_done("Test the module").await;

        // The task must have been claimed and completed (InReview or Done).
        let tasks = board.by_status(TaskStatus::InReview);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].result.as_deref(), Some("Added 3 tests"));
    }

    #[tokio::test]
    async fn test_team_runner_reviewer_approval() {
        let config = TeamConfig {
            poll_interval_ms: 1,
            max_rounds: 5,
            ..TeamConfig::default()
        };
        let mut team = AgentTeam::new("reviewer-test", config);
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);
        team.add_member("rev", TeamRole::Reviewer);

        let mut runner = TeamRunner::new(team);
        runner
            .bind_session(
                "lead",
                MockExecutor::new(r#"{"tasks": ["Implement feature X"]}"#),
            )
            .unwrap();
        runner
            .bind_session("w1", MockExecutor::new("Feature X implemented"))
            .unwrap();
        runner
            .bind_session("rev", MockExecutor::new("APPROVED: complete"))
            .unwrap();

        let result = runner.run_until_done("Ship feature X").await.unwrap();

        assert_eq!(result.done_tasks.len(), 1);
        assert_eq!(
            result.done_tasks[0].result.as_deref(),
            Some("Feature X implemented")
        );
    }

    #[tokio::test]
    async fn test_team_runner_rejection_and_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Reviewer approves on the second attempt.
        struct ConditionalReviewer {
            calls: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl AgentExecutor for ConditionalReviewer {
            async fn execute(&self, _prompt: &str) -> crate::error::Result<String> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok("REJECTED: needs improvement".to_string())
                } else {
                    Ok("APPROVED: now correct".to_string())
                }
            }
        }

        let config = TeamConfig {
            poll_interval_ms: 1,
            max_rounds: 10,
            ..TeamConfig::default()
        };
        let mut team = AgentTeam::new("retry-test", config);
        team.add_member("lead", TeamRole::Lead);
        team.add_member("w1", TeamRole::Worker);
        team.add_member("rev", TeamRole::Reviewer);

        let mut runner = TeamRunner::new(team);
        runner
            .bind_session("lead", MockExecutor::new(r#"{"tasks": ["Do the thing"]}"#))
            .unwrap();
        runner
            .bind_session("w1", MockExecutor::new("attempt result"))
            .unwrap();
        runner
            .bind_session(
                "rev",
                Arc::new(ConditionalReviewer {
                    calls: AtomicUsize::new(0),
                }),
            )
            .unwrap();

        let result = runner.run_until_done("Complete the thing").await.unwrap();

        // After retry, the task is eventually approved.
        assert_eq!(result.done_tasks.len(), 1);
        assert!(result.rejected_tasks.is_empty());
    }
}
