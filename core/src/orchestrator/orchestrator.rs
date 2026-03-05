//! Agent Orchestrator 核心实现

use crate::error::Result;
use crate::orchestrator::{
    ControlSignal, OrchestratorConfig, OrchestratorEvent, SubAgentConfig, SubAgentHandle,
    SubAgentState,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Agent Orchestrator - 主子智能体协调器
///
/// 基于事件总线实现统一的监控和控制机制。
/// 默认使用内存事件通讯，支持用户自定义 NATS provider。
pub struct AgentOrchestrator {
    /// 配置
    config: OrchestratorConfig,

    /// 事件广播通道
    event_tx: broadcast::Sender<OrchestratorEvent>,

    /// SubAgent 注册表
    subagents: Arc<RwLock<HashMap<String, SubAgentHandle>>>,

    /// 下一个 SubAgent ID
    next_id: Arc<RwLock<u64>>,
}

impl AgentOrchestrator {
    /// 创建新的 orchestrator（使用内存事件通讯）
    ///
    /// 这是默认的创建方式，适用于单进程场景。
    pub fn new_memory() -> Self {
        Self::new(OrchestratorConfig::default())
    }

    /// 使用自定义配置创建 orchestrator
    pub fn new(config: OrchestratorConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_buffer_size);

        Self {
            config,
            event_tx,
            subagents: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    /// 订阅所有 SubAgent 事件
    ///
    /// 返回一个接收器，可以接收所有 SubAgent 的事件。
    pub fn subscribe_all(&self) -> broadcast::Receiver<OrchestratorEvent> {
        self.event_tx.subscribe()
    }

    /// 订阅特定 SubAgent 的事件
    ///
    /// 返回一个过滤后的接收器，只接收指定 SubAgent 的事件。
    pub fn subscribe_subagent(&self, id: &str) -> SubAgentEventStream {
        let rx = self.event_tx.subscribe();
        SubAgentEventStream {
            rx,
            filter_id: id.to_string(),
        }
    }

    /// 启动新的 SubAgent
    ///
    /// 返回 SubAgent 句柄，可用于控制和查询状态。
    pub async fn spawn_subagent(&self, config: SubAgentConfig) -> Result<SubAgentHandle> {
        // 检查并发限制
        {
            let subagents = self.subagents.read().await;
            let active_count = subagents
                .values()
                .filter(|h| !h.state().is_terminal())
                .count();

            if active_count >= self.config.max_concurrent_subagents {
                return Err(anyhow::anyhow!(
                    "Maximum concurrent subagents ({}) reached",
                    self.config.max_concurrent_subagents
                )
                .into());
            }
        }

        // 生成 SubAgent ID
        let id = {
            let mut next_id = self.next_id.write().await;
            let id = format!("subagent-{}", *next_id);
            *next_id += 1;
            id
        };

        // 创建控制通道
        let (control_tx, control_rx) = tokio::sync::mpsc::channel(self.config.control_buffer_size);

        // 创建状态
        let state = Arc::new(RwLock::new(SubAgentState::Initializing));

        // 发布启动事件
        let _ = self.event_tx.send(OrchestratorEvent::SubAgentStarted {
            id: id.clone(),
            agent_type: config.agent_type.clone(),
            description: config.description.clone(),
            parent_id: config.parent_id.clone(),
            config: config.clone(),
        });

        // 创建 SubAgentWrapper 并启动执行
        let wrapper = crate::orchestrator::wrapper::SubAgentWrapper::new(
            id.clone(),
            config,
            self.event_tx.clone(),
            control_rx,
            state.clone(),
        );

        let task_handle = tokio::spawn(async move { wrapper.execute().await });

        // 创建句柄
        let handle = SubAgentHandle::new(id.clone(), control_tx, state.clone(), task_handle);

        // 注册到 orchestrator
        self.subagents.write().await.insert(id.clone(), handle.clone());

        Ok(handle)
    }

    /// 发送控制信号到 SubAgent
    pub async fn send_control(&self, id: &str, signal: ControlSignal) -> Result<()> {
        let subagents = self.subagents.read().await;
        let handle = subagents
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("SubAgent '{}' not found", id))?;

        handle.send_control(signal.clone()).await?;

        // 发布控制信号接收事件
        let _ = self.event_tx.send(OrchestratorEvent::ControlSignalReceived {
            id: id.to_string(),
            signal,
        });

        Ok(())
    }

    /// 暂停 SubAgent
    pub async fn pause_subagent(&self, id: &str) -> Result<()> {
        self.send_control(id, ControlSignal::Pause).await
    }

    /// 恢复 SubAgent
    pub async fn resume_subagent(&self, id: &str) -> Result<()> {
        self.send_control(id, ControlSignal::Resume).await
    }

    /// 取消 SubAgent
    pub async fn cancel_subagent(&self, id: &str) -> Result<()> {
        self.send_control(id, ControlSignal::Cancel).await
    }

    /// 调整 SubAgent 参数
    pub async fn adjust_subagent_params(
        &self,
        id: &str,
        max_steps: Option<usize>,
        timeout_ms: Option<u64>,
    ) -> Result<()> {
        self.send_control(
            id,
            ControlSignal::AdjustParams {
                max_steps,
                timeout_ms,
            },
        )
        .await
    }

    /// 获取 SubAgent 状态
    pub async fn get_subagent_state(&self, id: &str) -> Option<SubAgentState> {
        let subagents = self.subagents.read().await;
        subagents.get(id).map(|h| h.state())
    }

    /// 获取所有 SubAgent 的状态
    pub async fn get_all_states(&self) -> HashMap<String, SubAgentState> {
        let subagents = self.subagents.read().await;
        subagents
            .iter()
            .map(|(id, handle)| (id.clone(), handle.state()))
            .collect()
    }

    /// 获取活跃的 SubAgent 数量
    pub async fn active_count(&self) -> usize {
        let subagents = self.subagents.read().await;
        subagents
            .values()
            .filter(|h| !h.state().is_terminal())
            .count()
    }

    /// 等待所有 SubAgent 完成
    pub async fn wait_all(&self) -> Result<()> {
        loop {
            let active = self.active_count().await;
            if active == 0 {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// 发布事件（内部使用）
    pub(crate) fn publish_event(&self, event: OrchestratorEvent) {
        let _ = self.event_tx.send(event);
    }

    /// 获取事件发送器（用于 SubAgentWrapper）
    pub(crate) fn event_sender(&self) -> broadcast::Sender<OrchestratorEvent> {
        self.event_tx.clone()
    }
}

impl std::fmt::Debug for AgentOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOrchestrator")
            .field("event_buffer_size", &self.config.event_buffer_size)
            .field("max_concurrent_subagents", &self.config.max_concurrent_subagents)
            .finish()
    }
}

/// SubAgent 事件流（过滤特定 SubAgent 的事件）
pub struct SubAgentEventStream {
    rx: broadcast::Receiver<OrchestratorEvent>,
    filter_id: String,
}

impl SubAgentEventStream {
    /// 接收下一个事件
    pub async fn recv(&mut self) -> Option<OrchestratorEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if let Some(id) = event.subagent_id() {
                        if id == self.filter_id {
                            return Some(event);
                        }
                    }
                }
                Err(_) => return None,
            }
        }
    }
}
