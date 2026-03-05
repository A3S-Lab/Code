# Orchestrator Monitoring API 快速参考

## 设置

```bash
# 设置 Kimi API Key
export MOONSHOT_API_KEY=your_api_key_here
```

## Python API

### 创建和启动

```python
from a3s_code import Orchestrator, SubAgentConfig

# 创建 Orchestrator
orch = Orchestrator.create()

# 配置 SubAgent
config = SubAgentConfig(
    agent_type="explore",
    description="Find Python files",
    prompt="Use glob to find all Python files",
    permissive=True,
    max_steps=5
)

# 启动 SubAgent
handle = orch.spawn_subagent(config)
```

### 监控 API

```python
# 获取所有 SubAgent 信息（包含当前活动）
subagents = orch.list_subagents()
for info in subagents:
    print(f"{info.id}: {info.state}")
    if info.current_activity:
        print(f"  Activity: {info.current_activity.activity_type}")

# 获取特定 SubAgent 信息
info = orch.get_subagent_info(subagent_id)

# 获取所有活跃 SubAgent 的当前活动
activities = orch.get_active_activities()
for subagent_id, activity in activities:
    print(f"{subagent_id}: {activity.activity_type}")

# 获取所有 SubAgent 状态
states = orch.get_all_states()

# 获取活跃数量
count = orch.active_count()
```

### 控制 API

```python
# 暂停
orch.pause_subagent(subagent_id)

# 恢复
orch.resume_subagent(subagent_id)

# 取消
orch.cancel_subagent(subagent_id)

# 等待所有完成
orch.wait_all()
```

## TypeScript/Node.js API

### 创建和启动

```typescript
import { Orchestrator, SubAgentConfig } from '@a3s-lab/code';

// 创建 Orchestrator
const orch = Orchestrator.create();

// 配置 SubAgent
const config: SubAgentConfig = {
  agentType: 'explore',
  description: 'Find TypeScript files',
  prompt: 'Use glob to find all TypeScript files',
  permissive: true,
  maxSteps: 5
};

// 启动 SubAgent
const handle = orch.spawnSubagent(config);
```

### 监控 API

```typescript
// 获取所有 SubAgent 信息（包含当前活动）
const subagents = orch.listSubagents();
for (const info of subagents) {
  console.log(`${info.id}: ${info.state}`);
  if (info.currentActivity) {
    console.log(`  Activity: ${info.currentActivity.activityType}`);
  }
}

// 获取特定 SubAgent 信息
const info = orch.getSubagentInfo(subagentId);

// 获取所有活跃 SubAgent 的当前活动
const activities = orch.getActiveActivities();
for (const entry of activities) {
  console.log(`${entry.id}: ${entry.activity.activityType}`);
}

// 获取所有 SubAgent 状态
const states = orch.getAllStates();

// 获取活跃数量
const count = orch.activeCount();
```

### 控制 API

```typescript
// 暂停
orch.pauseSubagent(subagentId);

// 恢复
orch.resumeSubagent(subagentId);

// 取消
orch.cancelSubagent(subagentId);

// 等待所有完成
orch.waitAll();
```

## SubAgentInfo 结构

```typescript
interface SubAgentInfo {
  id: string;              // SubAgent ID
  agentType: string;       // 类型 (explore, analyze, etc.)
  description: string;     // 任务描述
  state: string;           // 当前状态
  parentId?: string;       // 父 SubAgent ID
  createdAt: number;       // 创建时间戳
  updatedAt: number;       // 更新时间戳
  currentActivity?: {      // 当前活动
    activityType: string;  // idle | calling_tool | requesting_llm | waiting_for_control
    data?: string;         // JSON 数据
  };
}
```

## 活动类型

| 类型 | 说明 | 数据示例 |
|------|------|---------|
| `idle` | 空闲 | `null` |
| `calling_tool` | 调用工具 | `{"tool_name": "glob", "args": {...}}` |
| `requesting_llm` | 请求 LLM | `{"message_count": 3}` |
| `waiting_for_control` | 等待控制 | `{"reason": "Paused by orchestrator"}` |

## 运行示例

### Python

```bash
# 快速开始
cd sdk/python/examples
pip install -r requirements.txt
python quickstart_monitoring.py

# 完整示例
python orchestrator_monitoring_kimi.py
```

### TypeScript

```bash
# 快速开始
cd sdk/node/examples
npm install
npm run quickstart

# 完整示例
npm run full
```

## 常见模式

### 实时监控循环

```python
# Python
import asyncio

for i in range(10):
    subagents = orch.list_subagents()
    print(f"[{i}] Active: {orch.active_count()}")
    for info in subagents:
        activity = info.current_activity.activity_type if info.current_activity else "idle"
        print(f"  {info.id}: {info.state} | {activity}")
    await asyncio.sleep(1)
```

```typescript
// TypeScript
for (let i = 0; i < 10; i++) {
  const subagents = orch.listSubagents();
  console.log(`[${i}] Active: ${orch.activeCount()}`);
  for (const info of subagents) {
    const activity = info.currentActivity?.activityType || 'idle';
    console.log(`  ${info.id}: ${info.state} | ${activity}`);
  }
  await sleep(1000);
}
```

### 条件控制

```python
# Python - 自动取消空闲太久的 SubAgent
for info in orch.list_subagents():
    if info.current_activity and info.current_activity.activity_type == "idle":
        # 空闲超过阈值，取消
        orch.cancel_subagent(info.id)
```

```typescript
// TypeScript - 自动取消空闲太久的 SubAgent
for (const info of orch.listSubagents()) {
  if (info.currentActivity?.activityType === 'idle') {
    // 空闲超过阈值，取消
    orch.cancelSubagent(info.id);
  }
}
```

## 相关文档

- [完整示例文档](./ORCHESTRATOR_MONITORING.md)
- [架构设计](../../../docs/architecture/agent-team-architecture.md)
- [Python SDK](../python/README.md)
- [Node.js SDK](../node/README.md)
