# Agent Orchestrator - 实现完整指南

## 当前状态

AgentOrchestrator 的核心协调功能已完全实现并测试通过：

- ✅ 事件驱动架构（基于 a3s-event）
- ✅ 实时监控（SubAgent 生命周期、状态变更、进度）
- ✅ 动态控制（暂停、恢复、取消、参数调整、提示词注入）
- ✅ Python SDK 绑定
- ✅ 单元测试（7 个测试全部通过）
- ✅ 集成测试

## 占位符执行 vs 真实执行

当前 `SubAgentWrapper` 使用占位符执行（placeholder）来演示协调功能。要集成真实的 LLM 执行，需要以下步骤：

### 方案 1：通过 Agent API 集成（推荐）

最简单的方法是让 Orchestrator 协调多个 `Agent` 实例：

```rust
// 在应用层面协调
let agent = Agent::new("agent.hcl").await?;
let orch = AgentOrchestrator::new_memory();

// 为每个 SubAgent 创建独立的 session
let session1 = agent.session("/workspace1", None)?;
let session2 = agent.session("/workspace2", None)?;

// 使用 tokio::spawn 并发执行
let handle1 = tokio::spawn(async move {
    session1.send("Task 1", None).await
});

let handle2 = tokio::spawn(async move {
    session2.send("Task 2", None).await
});

// 监控和控制通过 Orchestrator 事件
```

**优点：**
- 简单，不需要修改 Orchestrator 内部
- 利用现有的 Agent API
- 每个 SubAgent 有独立的 session 和 workspace

**缺点：**
- 需要在应用层面手动协调
- 控制信号需要通过 session API 传递

### 方案 2：在 SubAgentWrapper 中集成 AgentLoop

修改 `SubAgentWrapper` 来创建和执行真实的 `AgentLoop`：

```rust
// 在 orchestrator.rs 中添加依赖
pub struct AgentOrchestrator {
    // ... 现有字段
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    workspace: PathBuf,
}

// 在 wrapper.rs 中使用
impl SubAgentWrapper {
    async fn execute_real(&mut self) -> Result<String> {
        // 创建 ToolContext
        let tool_context = ToolContext {
            workspace: self.workspace.clone(),
            session_id: Some(self.id.clone()),
        };

        // 创建 AgentConfig
        let mut agent_config = AgentConfig::default();
        agent_config.max_tool_rounds = self.config.max_steps.unwrap_or(50);
        // ... 其他配置

        // 创建 AgentLoop
        let agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            tool_context,
            agent_config,
        );

        // 执行
        let result = agent_loop.execute(&[], &self.config.prompt, None).await?;
        Ok(result.text)
    }
}
```

**优点：**
- 完全集成，SubAgent 可以真正执行 LLM 调用和工具
- 控制信号可以直接影响 AgentLoop 执行

**缺点：**
- 需要传递大量依赖（LlmClient, ToolExecutor, workspace 等）
- Orchestrator API 变得复杂
- 需要处理 AgentLoop 的所有配置选项

### 方案 3：混合方案

在 Orchestrator 中添加可选的 Agent 工厂：

```rust
pub struct OrchestratorConfig {
    // ... 现有字段
    agent_factory: Option<Arc<dyn AgentFactory>>,
}

#[async_trait]
pub trait AgentFactory: Send + Sync {
    async fn create_agent(&self, config: &SubAgentConfig) -> Result<Box<dyn AgentExecutor>>;
}

#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn execute(&mut self, prompt: &str) -> Result<String>;
    async fn pause(&mut self) -> Result<()>;
    async fn resume(&mut self) -> Result<()>;
    async fn cancel(&mut self) -> Result<()>;
}
```

**优点：**
- 灵活，用户可以提供自己的 Agent 实现
- Orchestrator 保持简单
- 支持占位符和真实执行

**缺点：**
- 需要额外的抽象层
- 用户需要实现 AgentFactory 和 AgentExecutor

## 推荐实现路径

对于大多数用例，**推荐方案 1**：

1. 使用 Orchestrator 进行事件监控和状态管理
2. 在应用层面使用 Agent API 创建和执行 sessions
3. 通过 Orchestrator 事件总线协调多个 Agent

示例代码：

```python
from a3s_code import Agent, Orchestrator, SubAgentConfig

# 创建主 Agent
agent = Agent.create("agent.hcl")

# 创建 Orchestrator 用于监控
orch = Orchestrator.create()

# 创建 SubAgent 配置
config = SubAgentConfig(
    agent_type="general",
    prompt="Analyze code",
    permissive=True,
    max_steps=10
)

# 生成 SubAgent（占位符）用于监控
handle = orch.spawn_subagent(config)

# 在单独的线程/任务中执行真实的 Agent
import threading

def run_agent():
    session = agent.session("/workspace", None)
    result = session.send(config.prompt, None)
    print(f"Result: {result.text}")

thread = threading.Thread(target=run_agent)
thread.start()

# 监控 SubAgent 状态
while not handle.state().startswith("Completed"):
    time.sleep(0.5)
    print(f"State: {handle.state()}")

thread.join()
```

## 测试

当前提供的测试：

1. **单元测试** (`core/src/orchestrator/tests.rs`)
   - 7 个测试覆盖核心功能
   - 使用占位符执行

2. **Python 绑定测试** (`sdk/python/examples/orchestrator_test.py`)
   - 验证 Python SDK 功能
   - 测试所有 API

3. **集成测试** (`sdk/python/examples/orchestrator_integration.py`)
   - 演示多 SubAgent 协调
   - 展示暂停/恢复/取消控制

4. **Kimi 模型测试** (`sdk/python/examples/orchestrator_kimi.py`)
   - 准备好的真实 LLM 测试脚本
   - 需要 MOONSHOT_API_KEY 环境变量

## 下一步

要使用真实的 Kimi 模型测试：

```bash
# 设置 API key
export MOONSHOT_API_KEY="your-key-here"

# 运行测试（当前使用占位符）
cd sdk/python
python examples/orchestrator_kimi.py

# 要使用真实 LLM，需要实现方案 1、2 或 3
```

## 总结

AgentOrchestrator 的核心协调功能已完全实现。要集成真实的 LLM 执行：

- **快速原型**：使用方案 1（应用层协调）
- **完全集成**：使用方案 2（修改 SubAgentWrapper）
- **最大灵活性**：使用方案 3（AgentFactory 抽象）

当前的占位符实现足以演示和测试协调功能，并为真实集成提供了清晰的接口。
