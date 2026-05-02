//! Advanced SubAgent control-plane implementation.

use crate::error::Result;
use crate::orchestrator::{
    ControlSignal, OrchestratorConfig, OrchestratorEvent, SubAgentActivity, SubAgentConfig,
    SubAgentHandle, SubAgentInfo, SubAgentState,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Advanced SubAgent control plane.
///
/// This API is for explicit SubAgent lifecycle control: spawn, pause, resume,
/// cancel, inspect, and subscribe to events. Routine model-visible delegation
/// should use `task` / `parallel_task`.
pub struct AgentOrchestrator {
    /// 配置
    config: OrchestratorConfig,

    /// Agent used to execute SubAgents.
    agent: Option<Arc<crate::Agent>>,

    /// 事件广播通道
    event_tx: broadcast::Sender<OrchestratorEvent>,

    /// SubAgent 注册表
    subagents: Arc<RwLock<HashMap<String, SubAgentHandle>>>,

    /// 下一个 SubAgent ID
    next_id: Arc<RwLock<u64>>,
}

impl AgentOrchestrator {
    /// Create a memory-backed control plane.
    ///
    /// This is useful for inspecting an empty control plane in tests. Spawning
    /// SubAgents requires [`Self::from_agent`].
    pub fn new_memory() -> Self {
        Self::new(OrchestratorConfig::default())
    }

    /// Create a memory-backed control plane with custom config.
    pub fn new(config: OrchestratorConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_buffer_size);

        Self {
            config,
            agent: None,
            event_tx,
            subagents: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    /// Create an orchestrator backed by a real Agent for LLM execution.
    ///
    /// SubAgents spawned by this orchestrator will run the actual agent
    /// definition (permissions, system prompt, model, max_steps) loaded from
    /// the agent's configuration and any extra `agent_dirs` provided in
    /// `SubAgentConfig`.
    pub fn from_agent(agent: Arc<crate::Agent>) -> Self {
        Self::from_agent_with_config(agent, OrchestratorConfig::default())
    }

    /// Create an orchestrator backed by a real Agent with custom config.
    pub fn from_agent_with_config(agent: Arc<crate::Agent>, config: OrchestratorConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_buffer_size);

        Self {
            config,
            agent: Some(agent),
            event_tx,
            subagents: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        }
    }

    /// Subscribe to all SubAgent events.
    ///
    /// 返回一个接收器，可以接收所有 SubAgent 的事件。
    pub fn subscribe_all(&self) -> broadcast::Receiver<OrchestratorEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe to events for a specific SubAgent.
    ///
    /// 返回一个过滤后的接收器，只接收指定 SubAgent 的事件。
    pub fn subscribe_subagent(&self, id: &str) -> SubAgentEventStream {
        let rx = self.event_tx.subscribe();
        SubAgentEventStream {
            history: VecDeque::new(),
            rx,
            filter_id: id.to_string(),
        }
    }

    /// Spawn a new SubAgent.
    ///
    /// 返回 SubAgent 句柄，可用于控制和查询状态。
    pub async fn spawn_subagent(&self, config: SubAgentConfig) -> Result<SubAgentHandle> {
        let agent = self.agent.clone().ok_or_else(|| {
            anyhow::anyhow!("SubAgent execution requires AgentOrchestrator::from_agent")
        })?;

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
        let (subagent_event_tx, _) = broadcast::channel(self.config.event_buffer_size);

        // 创建状态
        let state = Arc::new(RwLock::new(SubAgentState::Initializing));

        // 创建活动跟踪
        let activity = Arc::new(RwLock::new(SubAgentActivity::Idle));
        let event_history = Arc::new(RwLock::new(VecDeque::with_capacity(
            self.config.event_buffer_size,
        )));

        // 发布启动事件
        let started_event = OrchestratorEvent::SubAgentStarted {
            id: id.clone(),
            agent_type: config.agent_type.clone(),
            description: config.description.clone(),
            parent_id: config.parent_id.clone(),
            config: config.clone(),
        };
        let _ = self.event_tx.send(started_event.clone());
        let _ = subagent_event_tx.send(started_event.clone());
        event_history.write().await.push_back(started_event);

        // 创建 SubAgentWrapper 并启动执行
        let wrapper = crate::orchestrator::wrapper::SubAgentWrapper::new(
            id.clone(),
            config.clone(),
            agent,
            self.event_tx.clone(),
            subagent_event_tx.clone(),
            Arc::clone(&event_history),
            control_rx,
            state.clone(),
            activity.clone(),
        );

        let task_handle = tokio::spawn(async move { wrapper.execute().await });

        // 创建句柄
        let handle = SubAgentHandle::new(crate::orchestrator::handle::SubAgentHandleParts {
            id: id.clone(),
            config,
            control_tx,
            subagent_event_tx,
            event_history,
            state: state.clone(),
            activity: activity.clone(),
            task_handle,
        });

        // 注册到 orchestrator
        self.subagents
            .write()
            .await
            .insert(id.clone(), handle.clone());

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
        let _ = self
            .event_tx
            .send(OrchestratorEvent::ControlSignalReceived {
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

    /// 获取所有 SubAgent 的信息列表
    pub async fn list_subagents(&self) -> Vec<SubAgentInfo> {
        let subagents = self.subagents.read().await;
        let mut infos = Vec::new();

        for (id, handle) in subagents.iter() {
            let state = handle.state_async().await;
            let activity = handle.activity().await;
            let config = handle.config();

            infos.push(SubAgentInfo {
                id: id.clone(),
                agent_type: config.agent_type.clone(),
                description: config.description.clone(),
                state: format!("{:?}", state),
                parent_id: config.parent_id.clone(),
                created_at: handle.created_at(),
                updated_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
                current_activity: Some(activity),
            });
        }

        infos
    }

    /// 获取特定 SubAgent 的详细信息
    pub async fn get_subagent_info(&self, id: &str) -> Option<SubAgentInfo> {
        let subagents = self.subagents.read().await;
        let handle = subagents.get(id)?;

        let state = handle.state_async().await;
        let activity = handle.activity().await;
        let config = handle.config();

        Some(SubAgentInfo {
            id: id.to_string(),
            agent_type: config.agent_type.clone(),
            description: config.description.clone(),
            state: format!("{:?}", state),
            parent_id: config.parent_id.clone(),
            created_at: handle.created_at(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            current_activity: Some(activity),
        })
    }

    /// 获取所有活跃 SubAgent 的当前活动
    pub async fn get_active_activities(&self) -> HashMap<String, SubAgentActivity> {
        let subagents = self.subagents.read().await;
        let mut activities = HashMap::new();

        for (id, handle) in subagents.iter() {
            if !handle.state().is_terminal() {
                let activity = handle.activity().await;
                activities.insert(id.clone(), activity);
            }
        }

        activities
    }

    /// 获取 SubAgent 句柄（用于直接控制）
    pub async fn get_handle(&self, id: &str) -> Option<SubAgentHandle> {
        let subagents = self.subagents.read().await;
        subagents.get(id).cloned()
    }
}

impl std::fmt::Debug for AgentOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOrchestrator")
            .field("event_buffer_size", &self.config.event_buffer_size)
            .field(
                "max_concurrent_subagents",
                &self.config.max_concurrent_subagents,
            )
            .finish()
    }
}

/// SubAgent 事件流（过滤特定 SubAgent 的事件）
pub struct SubAgentEventStream {
    pub(crate) history: VecDeque<OrchestratorEvent>,
    pub(crate) rx: broadcast::Receiver<OrchestratorEvent>,
    pub(crate) filter_id: String,
}

impl SubAgentEventStream {
    /// 接收下一个事件
    pub async fn recv(&mut self) -> Option<OrchestratorEvent> {
        if let Some(event) = self.history.pop_front() {
            return Some(event);
        }

        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if let Some(id) = event.subagent_id() {
                        if id == self.filter_id {
                            return Some(event);
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}
