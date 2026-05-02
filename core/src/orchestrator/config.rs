//! Orchestrator configuration

use serde::{Deserialize, Serialize};

/// Orchestrator 配置
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// 事件通道缓冲区大小
    pub event_buffer_size: usize,

    /// 控制信号通道缓冲区大小
    pub control_buffer_size: usize,

    /// SubAgent 最大并发数
    pub max_concurrent_subagents: usize,

    /// 事件主题前缀
    pub subject_prefix: String,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            event_buffer_size: 1000,
            control_buffer_size: 100,
            max_concurrent_subagents: 50,
            subject_prefix: "agent".to_string(),
        }
    }
}

/// SubAgent 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    /// Agent 类型 (general, explore, plan, etc.)
    pub agent_type: String,

    /// 任务描述
    pub description: String,

    /// 执行提示词
    pub prompt: String,

    /// 最大执行步数
    pub max_steps: Option<usize>,

    /// 执行超时（毫秒）
    pub timeout_ms: Option<u64>,

    /// 父 SubAgent ID（用于嵌套）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// 自定义元数据
    #[serde(default)]
    pub metadata: serde_json::Value,

    /// Workspace directory for the SubAgent (defaults to ".")
    #[serde(default = "default_workspace")]
    pub workspace: String,

    /// Extra directories to scan for agent definition files
    #[serde(default)]
    pub agent_dirs: Vec<String>,

    /// Extra directories to scan for skill definition files
    #[serde(default)]
    pub skill_dirs: Vec<String>,
}

/// SubAgent 信息（元数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentInfo {
    /// SubAgent ID
    pub id: String,

    /// Agent 类型
    pub agent_type: String,

    /// 任务描述
    pub description: String,

    /// 当前状态
    pub state: String,

    /// 父 SubAgent ID
    pub parent_id: Option<String>,

    /// 创建时间（Unix 时间戳，毫秒）
    pub created_at: u64,

    /// 最后更新时间（Unix 时间戳，毫秒）
    pub updated_at: u64,

    /// 当前活动
    pub current_activity: Option<SubAgentActivity>,
}

/// SubAgent 当前活动
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubAgentActivity {
    /// 空闲
    Idle,

    /// 正在调用工具
    CallingTool {
        tool_name: String,
        args: serde_json::Value,
    },

    /// 正在请求 LLM
    RequestingLlm { message_count: usize },

    /// 正在等待控制信号
    WaitingForControl { reason: String },
}

impl SubAgentConfig {
    /// 创建新的 SubAgent 配置
    pub fn new(agent_type: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            agent_type: agent_type.into(),
            description: String::new(),
            prompt: prompt.into(),
            max_steps: None,
            timeout_ms: None,
            parent_id: None,
            metadata: serde_json::Value::Null,
            workspace: default_workspace(),
            agent_dirs: Vec::new(),
            skill_dirs: Vec::new(),
        }
    }

    /// 设置描述
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// 设置最大步数
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = Some(max_steps);
        self
    }

    /// 设置超时
    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// 设置父 ID
    pub fn with_parent_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// 设置元数据
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set the workspace directory for this SubAgent
    pub fn with_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = workspace.into();
        self
    }

    /// Add extra directories to scan for agent definition files
    pub fn with_agent_dirs(mut self, dirs: Vec<String>) -> Self {
        self.agent_dirs = dirs;
        self
    }

    /// Add extra directories to scan for skill definition files
    pub fn with_skill_dirs(mut self, dirs: Vec<String>) -> Self {
        self.skill_dirs = dirs;
        self
    }
}

fn default_workspace() -> String {
    ".".to_string()
}
