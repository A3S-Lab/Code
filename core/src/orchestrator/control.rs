//! 控制信号定义

use serde::{Deserialize, Serialize};

/// 控制信号 - 主智能体发送给子智能体的指令
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlSignal {
    /// 暂停执行
    Pause,

    /// 恢复执行
    Resume,

    /// 取消执行
    Cancel,

    /// 调整参数
    AdjustParams {
        #[serde(skip_serializing_if = "Option::is_none")]
        max_steps: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },

    /// 注入新提示词
    InjectPrompt {
        prompt: String,
    },
}

impl std::fmt::Display for ControlSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlSignal::Pause => write!(f, "pause"),
            ControlSignal::Resume => write!(f, "resume"),
            ControlSignal::Cancel => write!(f, "cancel"),
            ControlSignal::AdjustParams { .. } => write!(f, "adjust_params"),
            ControlSignal::InjectPrompt { .. } => write!(f, "inject_prompt"),
        }
    }
}
