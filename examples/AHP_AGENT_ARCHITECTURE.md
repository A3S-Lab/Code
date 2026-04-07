# AHP 智能体监控架构

## 架构概述

这是一个**智能体监控智能体**的架构，两个智能体都使用 A3S Code 构建：

```
┌─────────────────────────────────────────────────────────────┐
│                      业务智能体                              │
│                  (A3S Code Agent)                           │
│                                                             │
│  - 执行实际业务任务                                          │
│  - 调用工具（Bash, Read, Write, etc.）                      │
│  - 配置 AHP transport 指向 AHP Server                       │
└─────────────────────────────────────────────────────────────┘
                          │
                          │ AHP 协议 (JSON-RPC 2.0)
                          │ stdio/HTTP
                          ↓
┌─────────────────────────────────────────────────────────────┐
│                   AHP Server 智能体                          │
│                  (A3S Code Agent)                           │
│                                                             │
│  - 接收 pre_action/post_action 事件                         │
│  - 使用 LLM 分析安全风险                                     │
│  - 做出 allow/block/modify 决策                             │
│  - 返回决策给业务智能体                                      │
└─────────────────────────────────────────────────────────────┘
```

## 工作流程

### Pre-Action（执行前）

```
1. 业务智能体准备调用工具
   ↓
2. 发送 pre_action 事件到 AHP Server
   {
     "tool_name": "Bash",
     "arguments": {"command": "rm -rf /"},
     "context": {...}
   }
   ↓
3. AHP Server 智能体使用 LLM 分析
   - 理解操作意图
   - 识别安全风险
   - 评估潜在影响
   ↓
4. AHP Server 返回决策
   {
     "action": "block",
     "reason": "检测到危险的系统删除操作",
     "severity": "critical"
   }
   ↓
5. 业务智能体根据决策执行或阻止
```

### Post-Action（执行后）

```
1. 业务智能体执行工具并获得输出
   ↓
2. 发送 post_action 事件到 AHP Server
   {
     "tool_name": "Read",
     "output": "API_KEY=sk-123...",
     "context": {...}
   }
   ↓
3. AHP Server 智能体使用 LLM 分析
   - 检测敏感信息（PII）
   - 识别提示词注入
   - 发现恶意载荷
   ↓
4. AHP Server 返回决策
   {
     "action": "modify",
     "modified_output": "API_KEY=[REDACTED]",
     "redactions": [...]
   }
   ↓
5. 业务智能体使用净化后的输出
```

## 核心优势

### 1. 智能体监控智能体

- **两个都是 A3S Code Agent**：都具有 LLM 推理能力
- **上下文感知**：AHP Server 理解操作的上下文和意图
- **自适应学习**：从历史决策中学习和改进

### 2. 超越规则匹配

传统方法：
```python
if "rm -rf" in command:
    return "block"
```

智能体方法：
```python
# AHP Server 智能体使用 LLM 分析
decision = await ahp_agent.analyze(
    tool="Bash",
    command="清理临时文件: rm -rf /tmp/myapp/*",
    context={"user": "admin", "workspace": "/tmp"}
)
# 结果: "allow" - 因为理解了意图和范围
```

### 3. 自然语言解释

```json
{
  "action": "block",
  "reason": "该命令尝试递归删除根目录，这会导致系统完全损坏。建议使用更具体的路径，如 'rm -rf /tmp/specific_dir'",
  "severity": "critical",
  "suggestions": [
    "指定明确的目录路径",
    "使用 --interactive 标志",
    "先用 ls 确认目标"
  ]
}
```

## 文件说明

### 1. `ahp_server_agent.py`

**AHP Server 智能体** - 安全监控智能体

- 使用 `Agent.create()` 创建智能体
- 使用 `session.send()` 与 LLM 交互进行分析
- 实现 AHP 2.0 协议服务器
- 提供 pre_action 和 post_action 监控

关键代码：
```python
# 创建智能体
self.agent = Agent.create(self.config_path)
self.session = self.agent.session(workspace, permissive=True)

# 使用 LLM 分析
result = self.session.send(security_analysis_prompt)
decision = self.extract_json_decision(result.text)
```

### 2. `business_agent_with_ahp.py`

**业务智能体** - 受监控的业务智能体

- 使用 `Agent.create()` 创建业务智能体
- 配置 `ahp_transport` 指向 AHP Server
- 所有操作自动受 AHP 监控

关键代码：
```python
# 创建业务智能体
agent = Agent.create(config_path)

# 配置 AHP 监控
opts = SessionOptions()
opts.ahp_transport = {
    "type": "stdio",
    "program": "python3",
    "args": ["ahp_server_agent.py"]
}

# 创建受监控的会话
session = agent.session(workspace, opts)

# 所有操作都会被 AHP Server 监控
result = session.send("执行某个任务")
```

## 使用方法

### 1. 配置 LLM

创建 `~/.a3s/config.hcl`:

```hcl
default_model = "moonshot/moonshot-v1-8k"

providers {
  name    = "moonshot"
  api_key = env("MOONSHOT_API_KEY")

  models {
    id   = "moonshot-v1-8k"
    name = "Moonshot v1 8k"
  }
}
```

或设置环境变量：
```bash
export MOONSHOT_API_KEY=your_api_key
```

### 2. 运行演示

```bash
# 运行完整演示
python3 business_agent_with_ahp.py
```

演示包括：
- ✓ 安全操作（会被允许）
- ✗ 危险操作（会被阻止）
- 🔒 输出净化（PII 脱敏）
- 🛡️ 提示词注入检测

### 3. 单独运行 AHP Server

```bash
# 作为独立服务运行
python3 ahp_server_agent.py

# 然后从其他智能体连接
```

## 对比：规则 vs 智能体

| 特性 | 基于规则 | 基于智能体 |
|------|---------|-----------|
| 实现 | 正则表达式 | A3S Code Agent + LLM |
| 检测能力 | 已知模式 | 上下文理解 |
| 误报率 | 高 | 低 |
| 适应性 | 静态 | 自适应学习 |
| 解释性 | 无 | 自然语言解释 |
| 成本 | 无 | LLM API 调用 |
| 延迟 | <1ms | 100-500ms |

## 适用场景

### 使用智能体监控（推荐）

- 需要理解操作意图和上下文
- 需要处理复杂的安全场景
- 需要自然语言解释决策
- 可以接受 LLM API 成本和延迟

### 使用规则监控

- 严格的延迟要求（<10ms）
- 无 LLM API 访问
- 简单的模式匹配足够
- 生产环境的第一道防线

### 混合方案

```python
# 第一层：快速规则检查
if matches_dangerous_pattern(command):
    return "block"

# 第二层：智能体深度分析
decision = await ahp_agent.analyze(command, context)
return decision
```

## 扩展

### 1. 添加自定义安全策略

修改 `ahp_server_agent.py` 中的提示词：

```python
CUSTOM_POLICY = """
额外的安全策略：
1. 禁止访问 /data/sensitive/ 目录
2. 所有数据库操作需要审计日志
3. 网络请求必须使用 HTTPS
"""

PRE_ACTION_PROMPT = SECURITY_ANALYSIS_SYSTEM_PROMPT + CUSTOM_POLICY + ...
```

### 2. 集成外部安全服务

```python
async def analyze_pre_action(self, tool_name, arguments, context):
    # 先用智能体分析
    llm_decision = await self.llm_analyze(...)

    # 再调用外部服务
    external_decision = await self.call_security_api(...)

    # 综合决策
    return self.merge_decisions(llm_decision, external_decision)
```

### 3. 部署为 HTTP 服务

```python
from fastapi import FastAPI

app = FastAPI()
ahp_server = AHPServerAgent()

@app.post("/ahp")
async def handle_ahp_request(request: dict):
    return await ahp_server.handle_request(request)
```

## 总结

这个架构实现了**智能体监控智能体**的模式：

1. ✅ **两个都是 A3S Code Agent**：都具有 LLM 能力
2. ✅ **通过 AHP 协议通信**：标准化的监控接口
3. ✅ **智能安全分析**：超越简单规则匹配
4. ✅ **自然语言解释**：可理解的决策理由
5. ✅ **自适应学习**：从历史中改进

这是真正的**智能体驾驭智能体**的实现！
