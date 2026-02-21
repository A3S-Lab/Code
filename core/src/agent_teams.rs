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
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            max_tasks: 50,
            channel_buffer: 128,
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

    /// Claim the next open task for a member. Returns the task if available.
    pub fn claim(&self, member_id: &str) -> Option<TeamTask> {
        let mut tasks = self.tasks.write().unwrap();
        let task = tasks.iter_mut().find(|t| t.status == TaskStatus::Open)?;
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
}
