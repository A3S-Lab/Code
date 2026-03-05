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

    /// 是否启用宽松模式（绕过 HITL）
    #[serde(default)]
    pub permissive: bool,

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
}

impl SubAgentConfig {
    /// 创建新的 SubAgent 配置
    pub fn new(agent_type: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            agent_type: agent_type.into(),
            description: String::new(),
            prompt: prompt.into(),
            permissive: false,
            max_steps: None,
            timeout_ms: None,
            parent_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// 设置描述
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// 设置宽松模式
    pub fn with_permissive(mut self, permissive: bool) -> Self {
        self.permissive = permissive;
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
}
