# AgentOrchestrator 基于 a3s-event 的实现方案

## 为什么选择 a3s-event

1. **统一的事件基础设施** - A3S 生态系统的标准事件系统
2. **Provider 可插拔** - 支持内存、NATS、自定义 provider
3. **分布式支持** - 通过 NATS 支持跨进程/跨机器的 agent 协作
4. **持久化** - JetStream 提供事件持久化和回放
5. **成熟的 API** - EventBus, Subscription, DLQ 等完整功能

## 架构设计

### 核心组件

```rust
use a3s_event::{EventBus, Event, MemoryProvider};

/// Agent Orchestrator - 基于 a3s-event 的主子智能体协调器
pub struct AgentOrchestrator {
    /// 事件总线
    event_bus: Arc<EventBus<Box<dyn EventProvider>>>,

    /// SubAgent 注册表
    subagents: Arc<RwLock<HashMap<String, SubAgentHandle>>>,

    /// 配置
    config: OrchestratorConfig,
}

/// SubAgent 句柄
pub struct SubAgentHandle {
    /// SubAgent ID
    id: String,

    /// 控制主题 (用于发送控制信号)
    control_subject: String,

    /// 状态
    state: Arc<RwLock<SubAgentState>>,

    /// 任务句柄
    task_handle: tokio::task::JoinHandle<Result<String>>,
}
```

### 事件主题设计

使用 a3s-event 的主题命名约定：`<domain>.<entity>.<action>`

```
agent.orchestrator.started          # Orchestrator 启动
agent.orchestrator.stopped          # Orchestrator 停止

agent.subagent.{id}.started         # SubAgent 启动
agent.subagent.{id}.completed       # SubAgent 完成
agent.subagent.{id}.failed          # SubAgent 失败
agent.subagent.{id}.state_changed   # SubAgent 状态变更

agent.subagent.{id}.tool.started    # 工具执行开始
agent.subagent.{id}.tool.completed  # 工具执行完成
agent.subagent.{id}.tool.failed     # 工具执行失败

agent.subagent.{id}.llm.request     # LLM 请求
agent.subagent.{id}.llm.response    # LLM 响应
agent.subagent.{id}.llm.delta       # LLM 流式输出

agent.subagent.{id}.planning.started   # 规划开始
agent.subagent.{id}.planning.completed # 规划完成

agent.subagent.{id}.control         # 控制信号 (主智能体 → 子智能体)
agent.subagent.{id}.control.ack     # 控制信号确认 (子智能体 → 主智能体)
```

### 事件 Payload 结构

```rust
/// SubAgent 启动事件
#[derive(Serialize, Deserialize)]
pub struct SubAgentStartedPayload {
    pub id: String,
    pub agent_type: String,
    pub description: String,
    pub parent_id: Option<String>,
    pub config: SubAgentConfig,
}

/// SubAgent 完成事件
#[derive(Serialize, Deserialize)]
pub struct SubAgentCompletedPayload {
    pub id: String,
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
    pub token_usage: TokenUsage,
}

/// 工具执行事件
#[derive(Serialize, Deserialize)]
pub struct ToolExecutionPayload {
    pub id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub result: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
}

/// 控制信号 Payload
#[derive(Serialize, Deserialize)]
pub struct ControlSignalPayload {
    pub signal_type: ControlSignalType,
    pub params: serde_json::Value,
    pub timestamp: i64,
}

#[derive(Serialize, Deserialize)]
pub enum ControlSignalType {
    Pause,
    Resume,
    Cancel,
    AdjustParams,
    InjectPrompt,
}
```

## 实现代码

### 1. AgentOrchestrator 实现

```rust
impl AgentOrchestrator {
    /// 创建新的 orchestrator
    pub fn new(provider: Box<dyn EventProvider>) -> Self {
        let event_bus = Arc::new(EventBus::new(provider));

        Self {
            event_bus,
            subagents: Arc::new(RwLock::new(HashMap::new())),
            config: OrchestratorConfig::default(),
        }
    }

    /// 订阅所有 SubAgent 事件
    pub async fn subscribe_all(&self) -> Result<impl Stream<Item = ReceivedEvent>> {
        // 订阅所有 agent.subagent.* 主题
        self.event_bus.subscribe(
            "agent.subagent.*",
            SubscribeOptions::default()
        ).await
    }

    /// 订阅特定 SubAgent 的事件
    pub async fn subscribe_subagent(&self, id: &str) -> Result<impl Stream<Item = ReceivedEvent>> {
        let subject = format!("agent.subagent.{}.>", id);
        self.event_bus.subscribe(&subject, SubscribeOptions::default()).await
    }

    /// 启动 SubAgent
    pub async fn spawn_subagent(&self, config: SubAgentConfig) -> Result<SubAgentHandle> {
        let id = format!("subagent-{}", uuid::Uuid::new_v4());

        // 发布启动事件
        self.event_bus.publish(
            "agent",
            &format!("subagent.{}.started", id),
            &format!("SubAgent {} started", id),
            "orchestrator",
            serde_json::to_value(SubAgentStartedPayload {
                id: id.clone(),
                agent_type: config.agent_type.clone(),
                description: config.description.clone(),
                parent_id: None,
                config: config.clone(),
            })?,
        ).await?;

        // 创建 SubAgent wrapper
        let wrapper = SubAgentWrapper::new(
            id.clone(),
            config,
            Arc::clone(&self.event_bus),
        );

        // 启动执行任务
        let task_handle = tokio::spawn(async move {
            wrapper.execute().await
        });

        // 创建句柄
        let handle = SubAgentHandle {
            id: id.clone(),
            control_subject: format!("agent.subagent.{}.control", id),
            state: Arc::new(RwLock::new(SubAgentState::Running)),
            task_handle,
        };

        // 注册到 orchestrator
        self.subagents.write().unwrap().insert(id.clone(), handle.clone());

        Ok(handle)
    }

    /// 发送控制信号到 SubAgent
    pub async fn send_control(&self, id: &str, signal: ControlSignal) -> Result<()> {
        let subject = format!("agent.subagent.{}.control", id);

        self.event_bus.publish(
            "agent",
            &subject,
            &format!("Control signal: {:?}", signal),
            "orchestrator",
            serde_json::to_value(ControlSignalPayload {
                signal_type: signal.into(),
                params: serde_json::json!({}),
                timestamp: chrono::Utc::now().timestamp(),
            })?,
        ).await?;

        Ok(())
    }

    /// 获取所有 SubAgent 的状态
    pub fn get_all_states(&self) -> HashMap<String, SubAgentState> {
        self.subagents
            .read()
            .unwrap()
            .iter()
            .map(|(id, handle)| {
                (id.clone(), handle.state.read().unwrap().clone())
            })
            .collect()
    }
}
```

### 2. SubAgentWrapper 实现

```rust
pub struct SubAgentWrapper {
    id: String,
    config: SubAgentConfig,
    event_bus: Arc<EventBus<Box<dyn EventProvider>>>,
    agent_loop: AgentLoop,
    state: Arc<RwLock<SubAgentState>>,
}

impl SubAgentWrapper {
    pub fn new(
        id: String,
        config: SubAgentConfig,
        event_bus: Arc<EventBus<Box<dyn EventProvider>>>,
    ) -> Self {
        // 创建 AgentLoop
        let agent_loop = AgentLoop::new(/* ... */);

        Self {
            id,
            config,
            event_bus,
            agent_loop,
            state: Arc::new(RwLock::new(SubAgentState::Initializing)),
        }
    }

    pub async fn execute(&self) -> Result<String> {
        // 订阅控制信号
        let control_subject = format!("agent.subagent.{}.control", self.id);
        let mut control_stream = self.event_bus
            .subscribe(&control_subject, SubscribeOptions::default())
            .await?;

        // 更新状态
        self.update_state(SubAgentState::Running).await?;

        // 创建事件转发器 - 将 AgentLoop 事件转发到 EventBus
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);
        let event_bus = Arc::clone(&self.event_bus);
        let id = self.id.clone();

        tokio::spawn(async move {
            while let Some(agent_event) = event_rx.recv().await {
                // 将 AgentEvent 转换为 a3s-event Event 并发布
                let _ = Self::forward_agent_event(&event_bus, &id, agent_event).await;
            }
        });

        // 执行 AgentLoop，同时监听控制信号
        let result = tokio::select! {
            // AgentLoop 执行
            result = self.agent_loop.execute(&[], &self.config.prompt, Some(event_tx)) => {
                result
            }

            // 控制信号处理
            Some(control_event) = control_stream.next() => {
                self.handle_control_event(control_event).await
            }
        };

        // 发布完成事件
        self.publish_completed(result.is_ok(), &result).await?;

        result.map(|r| r.text)
    }

    async fn forward_agent_event(
        event_bus: &EventBus<Box<dyn EventProvider>>,
        id: &str,
        agent_event: AgentEvent,
    ) -> Result<()> {
        let (subject_suffix, payload) = match agent_event {
            AgentEvent::ToolStart { id: tool_id, name } => {
                ("tool.started", serde_json::json!({
                    "tool_id": tool_id,
                    "tool_name": name,
                }))
            }
            AgentEvent::ToolEnd { id: tool_id, name, output, exit_code, .. } => {
                ("tool.completed", serde_json::json!({
                    "tool_id": tool_id,
                    "tool_name": name,
                    "output": output,
                    "exit_code": exit_code,
                }))
            }
            AgentEvent::TextDelta { text } => {
                ("llm.delta", serde_json::json!({
                    "text": text,
                }))
            }
            // ... 其他事件类型
            _ => return Ok(()),
        };

        let subject = format!("subagent.{}.{}", id, subject_suffix);
        event_bus.publish(
            "agent",
            &subject,
            "",
            id,
            payload,
        ).await?;

        Ok(())
    }

    async fn handle_control_event(&self, event: ReceivedEvent) -> Result<AgentResult> {
        let payload: ControlSignalPayload = serde_json::from_value(event.event.payload)?;

        match payload.signal_type {
            ControlSignalType::Pause => {
                self.update_state(SubAgentState::Paused).await?;
                // 等待 Resume 信号
                self.wait_for_resume().await
            }
            ControlSignalType::Cancel => {
                self.update_state(SubAgentState::Cancelled).await?;
                Err(anyhow::anyhow!("Cancelled by orchestrator").into())
            }
            // ... 其他控制信号
            _ => Ok(AgentResult::default()),
        }
    }

    async fn update_state(&self, new_state: SubAgentState) -> Result<()> {
        let old_state = {
            let mut state = self.state.write().unwrap();
            let old = state.clone();
            *state = new_state.clone();
            old
        };

        // 发布状态变更事件
        self.event_bus.publish(
            "agent",
            &format!("subagent.{}.state_changed", self.id),
            &format!("State: {:?} -> {:?}", old_state, new_state),
            &self.id,
            serde_json::json!({
                "old_state": old_state,
                "new_state": new_state,
            }),
        ).await?;

        Ok(())
    }
}
```

### 3. 使用示例

```rust
use a3s_event::provider::memory::MemoryProvider;

#[tokio::main]
async fn main() -> Result<()> {
    // 创建 orchestrator (使用内存 provider)
    let orchestrator = AgentOrchestrator::new(
        Box::new(MemoryProvider::default())
    );

    // 订阅所有事件
    let mut event_stream = orchestrator.subscribe_all().await?;

    // 在后台监控所有事件
    tokio::spawn(async move {
        while let Some(event) = event_stream.next().await {
            println!("Event: {} - {}", event.event.subject, event.event.description);

            // 根据事件类型处理
            if event.event.subject.contains("tool.started") {
                let payload: ToolExecutionPayload =
                    serde_json::from_value(event.event.payload)?;
                println!("  Tool: {}", payload.tool_name);
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    // 启动 SubAgent
    let handle = orchestrator.spawn_subagent(SubAgentConfig {
        agent_type: "general".to_string(),
        description: "Analyze code".to_string(),
        prompt: "Use glob to find Python files".to_string(),
        permissive: true,
        max_steps: 10,
    }).await?;

    // 等待一段时间后暂停
    tokio::time::sleep(Duration::from_secs(5)).await;
    orchestrator.send_control(&handle.id, ControlSignal::Pause).await?;

    // 恢复执行
    tokio::time::sleep(Duration::from_secs(2)).await;
    orchestrator.send_control(&handle.id, ControlSignal::Resume).await?;

    // 等待完成
    let result = handle.task_handle.await??;
    println!("Result: {}", result);

    Ok(())
}
```

### 4. 分布式部署（使用 NATS）

```rust
use a3s_event::provider::nats::{NatsProvider, NatsConfig};

#[tokio::main]
async fn main() -> Result<()> {
    // 创建 NATS provider
    let nats_config = NatsConfig {
        url: "nats://localhost:4222".to_string(),
        ..Default::default()
    };
    let provider = NatsProvider::connect(nats_config).await?;

    // 创建 orchestrator (使用 NATS provider)
    let orchestrator = AgentOrchestrator::new(Box::new(provider));

    // 现在可以跨进程/跨机器监控和控制 SubAgent
    // 主智能体在机器 A，子智能体在机器 B，通过 NATS 通讯

    Ok(())
}
```

## 优势

1. **统一事件系统** - 与 A3S 生态系统无缝集成
2. **可扩展性** - 支持分布式部署（NATS）
3. **持久化** - JetStream 提供事件持久化和回放
4. **解耦** - 主子智能体通过事件通讯，松耦合
5. **可观测性** - 所有事件都可以被监控、记录、分析
6. **灵活性** - 可以轻松切换 provider（内存 → NATS → 自定义）

## 下一步

1. 实现 `AgentOrchestrator` 核心结构
2. 实现 `SubAgentWrapper` 包装器
3. 修改 `task.rs` 集成 orchestrator
4. 修改 `agent_teams.rs` 集成 orchestrator
5. 添加 Python/Node SDK API
6. 编写集成测试和文档
