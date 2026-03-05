# Orchestrator Monitoring Examples with Kimi

这些示例展示了如何使用真实的 Kimi 模型测试 Orchestrator 的实时监控功能。

## 功能演示

1. **创建 Orchestrator** - 使用内存事件通信
2. **启动多个 SubAgent** - 不同类型的任务（explore, analyze, document）
3. **实时监控** - 查看所有 SubAgent 的状态和当前活动
4. **动态控制** - 暂停、恢复、取消 SubAgent
5. **查询信息** - 获取特定 SubAgent 的详细信息
6. **活动跟踪** - 监控所有活跃 SubAgent 的当前活动

## 前置要求

### 1. 获取 Kimi API Key

访问 [Moonshot AI](https://platform.moonshot.cn/) 注册并获取 API Key。

### 2. 设置环境变量

```bash
export MOONSHOT_API_KEY=your_api_key_here
```

## Python 示例

### 安装依赖

```bash
pip install a3s-code
```

### 运行示例

```bash
cd sdk/python/examples
python orchestrator_monitoring_kimi.py
```

### 示例输出

```
=== Orchestrator Real-time Monitoring with Kimi ===

✓ Creating Orchestrator with memory-based event communication

Spawning 3 SubAgents...
  ✓ Spawned: subagent-1 (explore)
  ✓ Spawned: subagent-2 (analyze)
  ✓ Spawned: subagent-3 (document)

✓ Total active SubAgents: 3

=== Real-time Monitoring (5 snapshots) ===

--- Snapshot #1 ---
Time: 14:30:15
Active SubAgents: 3/3

SubAgent: subagent-1
  Type: explore
  Description: 探索 Python 代码库
  State: Running
  Created: 1709625015123
  Current Activity: calling_tool
    Data: {"tool_name": "glob", "args": {"pattern": "**/*.py"}}

SubAgent: subagent-2
  Type: analyze
  Description: 分析代码质量
  State: Running
  Created: 1709625015234
  Current Activity: requesting_llm
    Data: {"message_count": 3}

SubAgent: subagent-3
  Type: document
  Description: 生成文档
  State: Running
  Created: 1709625015345
  Current Activity: idle

...
```

## TypeScript/Node.js 示例

### 安装依赖

```bash
npm install @a3s-lab/code tsx
```

### 运行示例

```bash
cd sdk/node/examples
npx tsx orchestrator_monitoring_kimi.ts
```

### 示例输出

```
=== Orchestrator Real-time Monitoring with Kimi ===

✓ Creating Orchestrator with memory-based event communication

Spawning 3 SubAgents...
  ✓ Spawned: subagent-1 (explore)
  ✓ Spawned: subagent-2 (analyze)
  ✓ Spawned: subagent-3 (document)

✓ Total active SubAgents: 3

=== Real-time Monitoring (5 snapshots) ===

--- Snapshot #1 ---
Time: 2:30:15 PM
Active SubAgents: 3/3

SubAgent: subagent-1
  Type: explore
  Description: 探索 TypeScript 代码库
  State: Running
  Created: 1709625015123
  Current Activity: calling_tool
    Data: {"tool_name": "glob", "args": {"pattern": "**/*.ts"}}

...
```

## 监控的活动类型

示例会展示 4 种 SubAgent 活动类型：

1. **idle** - 空闲状态
   ```json
   {
     "activity_type": "idle",
     "data": null
   }
   ```

2. **calling_tool** - 正在调用工具
   ```json
   {
     "activity_type": "calling_tool",
     "data": "{\"tool_name\": \"glob\", \"args\": {\"pattern\": \"**/*.py\"}}"
   }
   ```

3. **requesting_llm** - 正在请求 LLM
   ```json
   {
     "activity_type": "requesting_llm",
     "data": "{\"message_count\": 3}"
   }
   ```

4. **waiting_for_control** - 等待控制信号（如暂停状态）
   ```json
   {
     "activity_type": "waiting_for_control",
     "data": "{\"reason\": \"Paused by orchestrator\"}"
   }
   ```

## 控制操作演示

示例会演示以下控制操作：

```python
# Python
orchestrator.pause_subagent(subagent_id)   # 暂停
orchestrator.resume_subagent(subagent_id)  # 恢复
orchestrator.cancel_subagent(subagent_id)  # 取消
orchestrator.wait_all()                     # 等待所有完成
```

```typescript
// TypeScript
orchestrator.pauseSubagent(subagentId);   // 暂停
orchestrator.resumeSubagent(subagentId);  // 恢复
orchestrator.cancelSubagent(subagentId);  // 取消
orchestrator.waitAll();                    // 等待所有完成
```

## 查询 API 演示

示例会演示以下查询 API：

```python
# Python
# 获取所有 SubAgent 信息
subagents = orchestrator.list_subagents()

# 获取特定 SubAgent 信息
info = orchestrator.get_subagent_info(subagent_id)

# 获取所有活跃 SubAgent 的当前活动
activities = orchestrator.get_active_activities()

# 获取所有 SubAgent 状态
states = orchestrator.get_all_states()

# 获取活跃数量
count = orchestrator.active_count()
```

```typescript
// TypeScript
// 获取所有 SubAgent 信息
const subagents = orchestrator.listSubagents();

// 获取特定 SubAgent 信息
const info = orchestrator.getSubagentInfo(subagentId);

// 获取所有活跃 SubAgent 的当前活动
const activities = orchestrator.getActiveActivities();

// 获取所有 SubAgent 状态
const states = orchestrator.getAllStates();

// 获取活跃数量
const count = orchestrator.activeCount();
```

## 注意事项

1. **API Key 安全**: 不要将 API Key 硬编码在代码中，使用环境变量
2. **速率限制**: Kimi API 有速率限制，注意控制并发数量
3. **错误处理**: 生产环境中应添加完善的错误处理
4. **资源清理**: 确保所有 SubAgent 正确完成或取消

## 故障排查

### 问题：MOONSHOT_API_KEY not set

**解决方案**:
```bash
export MOONSHOT_API_KEY=your_api_key_here
```

### 问题：API 调用失败

**可能原因**:
- API Key 无效或过期
- 网络连接问题
- 超过速率限制

**解决方案**:
- 检查 API Key 是否正确
- 检查网络连接
- 减少并发 SubAgent 数量

### 问题：SubAgent 长时间卡在某个状态

**解决方案**:
```python
# 使用 cancel 取消卡住的 SubAgent
orchestrator.cancel_subagent(subagent_id)
```

## 扩展示例

### 自定义监控间隔

```python
# Python - 每 500ms 监控一次
for snapshot in range(1, 11):
    subagents = orchestrator.list_subagents()
    # ... 处理监控数据
    await asyncio.sleep(0.5)
```

```typescript
// TypeScript - 每 500ms 监控一次
for (let snapshot = 1; snapshot <= 10; snapshot++) {
  const subagents = orchestrator.listSubagents();
  // ... 处理监控数据
  await sleep(500);
}
```

### 条件控制

```python
# Python - 当进度慢时暂停并调整
for info in orchestrator.list_subagents():
    if info.current_activity and info.current_activity.activity_type == "idle":
        # 空闲太久，可能有问题
        orchestrator.cancel_subagent(info.id)
```

```typescript
// TypeScript - 当进度慢时暂停并调整
for (const info of orchestrator.listSubagents()) {
  if (info.currentActivity?.activityType === 'idle') {
    // 空闲太久，可能有问题
    orchestrator.cancelSubagent(info.id);
  }
}
```

## 相关文档

- [Orchestrator 架构设计](../../../docs/architecture/agent-team-architecture.md)
- [Python SDK 文档](../README.md)
- [Node.js SDK 文档](../../node/README.md)
- [Kimi API 文档](https://platform.moonshot.cn/docs)
