# AgentTeam 主子智能体通讯机制设计

## 需求分析

用户需要：
1. 主智能体能实时监控所有子智能体的行为、规划和执行
2. 主智能体能调整子智能体的行为（暂停、恢复、修改参数等）
3. 统一的事件流架构，整合 AgentTeam 和 SubAgent

## 当前架构问题

### 1. AgentTeam 架构
- **通讯方式**: mpsc 消息传递 + 共享任务板
- **问题**:
  - 只有任务级别的状态更新，无法监控内部执行
  - 完全自治，无法中断或调整
  - 与 SubAgent 事件流不集成

### 2. SubAgent (task tool) 架构
- **通讯方式**: broadcast 事件流
- **问题**:
  - 单向监控，无法控制
  - 只支持单个 SubAgent，不支持团队协作

## 设计方案

### 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                      Main Agent                              │
│  ┌────────────────────────────────────────────────────┐     │
│  │  AgentOrchestrator                                 │     │
│  │  - 统一事件总线 (broadcast::Sender)                │     │
│  │  - 控制信号通道 (mpsc::Sender per SubAgent)        │     │
│  │  - 状态管理 (SubAgent registry)                    │     │
│  └────────────────────────────────────────────────────┘     │
│         ↓ events                    ↑ control signals       │
└─────────┼───────────────────────────┼─────────────────────┘
          │                           │
    ┌─────┴───────────────────────────┴─────┐
    │     Bidirectional Communication       │
    └─────┬───────────────────────────┬─────┘
          │                           │
┌─────────┼───────────────────────────┼─────────────────────┐
│         ↓ control                   ↑ events               │
│  ┌────────────────────────────────────────────────────┐   │
│  │  SubAgent Wrapper                                  │   │
│  │  - 接收控制信号 (mpsc::Receiver)                   │   │
│  │  - 发送事件 (broadcast::Sender)                    │   │
│  │  - 包装 AgentLoop 执行                             │   │
│  └────────────────────────────────────────────────────┘   │
│                    SubAgent Instance                       │
└────────────────────────────────────────────────────────────┘
```

### 核心组件

#### 1. AgentOrchestrator（主智能体协调器）

```rust
pub struct AgentOrchestrator {
    /// 统一事件总线 - 所有 SubAgent 事件汇聚到这里
    event_bus: broadcast::Sender<OrchestratorEvent>,

    /// SubAgent 注册表
    subagents: RwLock<HashMap<String, SubAgentHandle>>,

    /// 全局配置
    config: OrchestratorConfig,
}

pub struct SubAgentHandle {
    /// SubAgent ID
    id: String,

    /// 控制信号发送器
    control_tx: mpsc::Sender<ControlSignal>,

    /// SubAgent 状态
    state: Arc<RwLock<SubAgentState>>,

    /// 任务句柄
    task_handle: tokio::task::JoinHandle<Result<String>>,
}

pub enum ControlSignal {
    /// 暂停执行
    Pause,

    /// 恢复执行
    Resume,

    /// 取消执行
    Cancel,

    /// 调整参数
    AdjustParams {
        max_steps: Option<usize>,
        timeout_ms: Option<u64>,
    },

    /// 注入新指令
    InjectPrompt(String),
}

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
    Error(String),
}
```

#### 2. OrchestratorEvent（统一事件类型）

```rust
pub enum OrchestratorEvent {
    /// SubAgent 生命周期事件
    SubAgentStarted {
        id: String,
        agent_type: String,
        description: String,
    },

    SubAgentCompleted {
        id: String,
        success: bool,
        output: String,
    },

    SubAgentStateChanged {
        id: String,
        old_state: SubAgentState,
        new_state: SubAgentState,
    },

    /// SubAgent 内部事件（来自 AgentLoop）
    SubAgentInternalEvent {
        id: String,
        event: AgentEvent,  // 复用现有的 AgentEvent
    },

    /// 规划事件
    PlanningStarted {
        id: String,
        goal: String,
    },

    PlanningCompleted {
        id: String,
        plan: ExecutionPlan,
    },

    /// 工具执行事件
    ToolExecutionStarted {
        id: String,
        tool_name: String,
        args: serde_json::Value,
    },

    ToolExecutionCompleted {
        id: String,
        tool_name: String,
        result: String,
        exit_code: i32,
    },

    /// 控制信号响应
    ControlSignalReceived {
        id: String,
        signal: ControlSignal,
    },

    ControlSignalApplied {
        id: String,
        signal: ControlSignal,
        success: bool,
    },
}
```

#### 3. SubAgentWrapper（子智能体包装器）

```rust
pub struct SubAgentWrapper {
    /// SubAgent ID
    id: String,

    /// 控制信号接收器
    control_rx: mpsc::Receiver<ControlSignal>,

    /// 事件发送器
    event_tx: broadcast::Sender<OrchestratorEvent>,

    /// 状态
    state: Arc<RwLock<SubAgentState>>,

    /// 底层 AgentLoop
    agent_loop: AgentLoop,

    /// 执行上下文
    context: SubAgentContext,
}

impl SubAgentWrapper {
    /// 执行 SubAgent，同时监听控制信号
    pub async fn execute(&mut self, prompt: &str) -> Result<String> {
        // 发送启动事件
        self.send_event(OrchestratorEvent::SubAgentStarted { ... });

        // 更新状态
        self.update_state(SubAgentState::Running);

        // 创建可中断的执行任务
        let result = self.execute_with_control(prompt).await;

        // 发送完成事件
        self.send_event(OrchestratorEvent::SubAgentCompleted { ... });

        result
    }

    async fn execute_with_control(&mut self, prompt: &str) -> Result<String> {
        // 包装 AgentLoop 执行，同时监听控制信号
        tokio::select! {
            // AgentLoop 执行
            result = self.agent_loop.execute(...) => {
                result
            }

            // 控制信号处理
            signal = self.control_rx.recv() => {
                self.handle_control_signal(signal).await
            }
        }
    }

    async fn handle_control_signal(&mut self, signal: ControlSignal) -> Result<String> {
        match signal {
            ControlSignal::Pause => {
                self.update_state(SubAgentState::Paused);
                // 等待 Resume 信号
                self.wait_for_resume().await
            }

            ControlSignal::Cancel => {
                self.update_state(SubAgentState::Cancelled);
                Err(anyhow::anyhow!("Cancelled by orchestrator"))
            }

            ControlSignal::AdjustParams { max_steps, timeout_ms } => {
                // 动态调整参数
                self.adjust_params(max_steps, timeout_ms);
                Ok(String::new())
            }

            ControlSignal::InjectPrompt(new_prompt) => {
                // 注入新指令
                self.agent_loop.execute(&new_prompt, ...).await
            }

            _ => Ok(String::new())
        }
    }
}
```

### 使用示例

#### 1. 创建 Orchestrator 并监控事件

```rust
// 创建 orchestrator
let orchestrator = AgentOrchestrator::new(config);

// 订阅事件流
let mut event_stream = orchestrator.subscribe();

// 在后台监控所有事件
tokio::spawn(async move {
    while let Ok(event) = event_stream.recv().await {
        match event {
            OrchestratorEvent::SubAgentInternalEvent { id, event } => {
                println!("SubAgent {}: {:?}", id, event);
            }
            OrchestratorEvent::ToolExecutionStarted { id, tool_name, .. } => {
                println!("SubAgent {} executing tool: {}", id, tool_name);
            }
            _ => {}
        }
    }
});
```

#### 2. 启动 SubAgent 并控制

```rust
// 启动 SubAgent
let handle = orchestrator.spawn_subagent(SubAgentConfig {
    agent_type: "general",
    description: "Analyze code",
    prompt: "Use glob to find Python files",
    permissive: true,
    max_steps: 10,
}).await?;

// 实时监控状态
let state = handle.state().await;
println!("SubAgent state: {:?}", state);

// 动态调整参数
handle.send_control(ControlSignal::AdjustParams {
    max_steps: Some(20),
    timeout_ms: Some(60000),
}).await?;

// 暂停执行
handle.send_control(ControlSignal::Pause).await?;

// 恢复执行
handle.send_control(ControlSignal::Resume).await?;

// 取消执行
handle.send_control(ControlSignal::Cancel).await?;
```

#### 3. 集成到 AgentTeam

```rust
// 创建 AgentTeam with Orchestrator
let mut team = AgentTeam::with_orchestrator(
    "refactor-team",
    config,
    orchestrator.clone()
);

// 所有 team member 的执行都会通过 orchestrator
team.add_member("lead", TeamRole::Lead);
team.add_member("worker-1", TeamRole::Worker);

// 主智能体可以监控所有 team member 的行为
let mut event_stream = orchestrator.subscribe();
tokio::spawn(async move {
    while let Ok(event) = event_stream.recv().await {
        // 处理事件
    }
});

// 主智能体可以控制任何 team member
let worker_handle = team.get_member_handle("worker-1")?;
worker_handle.send_control(ControlSignal::Pause).await?;
```

## 实现优先级

### Phase 1: 核心基础设施（必须）
1. ✅ AgentOrchestrator 基础结构
2. ✅ OrchestratorEvent 事件类型
3. ✅ SubAgentWrapper 包装器
4. ✅ 控制信号机制

### Phase 2: 事件流集成（必须）
1. ✅ 将 SubAgent (task tool) 事件转发到 Orchestrator
2. ✅ 将 AgentTeam 事件转发到 Orchestrator
3. ✅ 统一事件格式

### Phase 3: 高级控制（可选）
1. ⏳ 动态参数调整
2. ⏳ 指令注入
3. ⏳ 执行暂停/恢复
4. ⏳ 智能调度策略

### Phase 4: SDK 暴露（必须）
1. ⏳ Python SDK API
2. ⏳ Node.js SDK API
3. ⏳ 文档和示例

## 技术挑战

1. **可中断的 AgentLoop 执行**
   - 当前 AgentLoop 是同步执行的，需要改造为可中断
   - 使用 tokio::select! 或 CancellationToken

2. **状态一致性**
   - 多个 SubAgent 并发执行时的状态同步
   - 使用 Arc<RwLock<>> 保证线程安全

3. **事件顺序保证**
   - broadcast channel 可能丢失事件
   - 考虑使用 mpsc + 事件序列号

4. **性能开销**
   - 每个事件都要经过 orchestrator
   - 考虑事件过滤和批处理

## 下一步行动

1. 实现 AgentOrchestrator 核心结构
2. 实现 SubAgentWrapper 包装器
3. 修改 task.rs 集成 orchestrator
4. 修改 agent_teams.rs 集成 orchestrator
5. 添加 Python/Node SDK API
6. 编写集成测试和文档
