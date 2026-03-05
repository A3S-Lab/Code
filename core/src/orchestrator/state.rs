//! SubAgent 状态定义

use serde::{Deserialize, Serialize};

/// SubAgent 状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubAgentState {
    /// 初始化中
    Initializing,

    /// 运行中
    Running,

    /// 已暂停
    Paused,

    /// 已完成
    Completed { success: bool, output: String },

    /// 已取消
    Cancelled,

    /// 错误
    Error { message: String },
}

impl SubAgentState {
    /// 是否为终止状态
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SubAgentState::Completed { .. }
                | SubAgentState::Cancelled
                | SubAgentState::Error { .. }
        )
    }

    /// 是否为运行状态
    pub fn is_running(&self) -> bool {
        matches!(self, SubAgentState::Running)
    }

    /// 是否为暂停状态
    pub fn is_paused(&self) -> bool {
        matches!(self, SubAgentState::Paused)
    }
}

impl std::fmt::Display for SubAgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubAgentState::Initializing => write!(f, "initializing"),
            SubAgentState::Running => write!(f, "running"),
            SubAgentState::Paused => write!(f, "paused"),
            SubAgentState::Completed { success, .. } => {
                write!(f, "completed(success={})", success)
            }
            SubAgentState::Cancelled => write!(f, "cancelled"),
            SubAgentState::Error { .. } => write!(f, "error"),
        }
    }
}
