# AHP Safety Skills

这个目录包含两个核心的 safety-skills，用于 AHP Server 智能体的安全分析。

## Skills 概述

### 1. Dangerous Operation Detector（危险操作检测器）

**文件**: `dangerous_operation_detector.py`

**功能**:
- 检测危险命令模式（rm -rf, dd, mkfs, fork bomb 等）
- 识别敏感路径访问（/etc/passwd, ~/.ssh, credentials 等）
- 检测 SSRF 风险（访问私有 IP、localhost）
- 识别命令注入（; | && ` $()）
- 检测路径遍历（../）
- 评估操作风险等级（low/medium/high/critical）

**使用方法**:
```python
from skills import detect_dangerous_operation

result = detect_dangerous_operation(
    tool_name="bash",
    arguments={"command": "rm -rf /"}
)

print(f"危险: {result['is_dangerous']}")
print(f"风险等级: {result['risk_level']}")
for threat in result['threats']:
    print(f"  - {threat['description']}")
```

**返回格式**:
```json
{
    "is_dangerous": true,
    "risk_level": "critical",
    "threats": [
        {
            "type": "dangerous_command",
            "description": "递归删除根目录",
            "pattern": "rm -rf /",
            "risk_level": "critical"
        }
    ],
    "recommendations": [
        "避免使用危险的系统命令",
        "使用明确的路径和参数"
    ],
    "metadata": {
        "tool_name": "bash",
        "threat_count": 1
    }
}
```

### 2. Untrusted Output Sanitizer（不可信输出净化器）

**文件**: `untrusted_output_sanitizer.py`

**功能**:
- 检测提示词注入（ignore previous instructions, system prompt override）
- 识别和脱敏 PII（API keys, passwords, emails, SSN, credit cards）
- 检测恶意载荷（XSS, JavaScript protocol, eval/exec）
- 识别社会工程攻击（urgent action, verify account）
- 限制输出大小（默认 100KB）
- 净化和修改输出

**使用方法**:
```python
from skills import sanitize_untrusted_output

result = sanitize_untrusted_output(
    tool_name="read",
    output="API_KEY=sk-1234567890abcdef\nEmail: user@example.com"
)

print(f"安全: {result['is_safe']}")
print(f"风险等级: {result['risk_level']}")
print(f"净化后: {result['sanitized_output']}")
```

**返回格式**:
```json
{
    "is_safe": false,
    "risk_level": "critical",
    "threats": [
        {
            "type": "pii_detected",
            "description": "检测到 API Key",
            "location": "位置 9-41",
            "risk_level": "critical"
        }
    ],
    "sanitized_output": "API_KEY=[REDACTED_API_KEY]\nEmail: [REDACTED_EMAIL]",
    "redactions": [
        {
            "type": "API Key",
            "original": "sk-1234567890abcdef",
            "replacement": "[REDACTED_API_KEY]",
            "risk_level": "critical"
        }
    ],
    "recommendations": [
        "已自动脱敏敏感信息",
        "确认脱敏后的输出不会泄露隐私"
    ]
}
```

## 在 AHP Server 中使用

AHP Server 智能体会自动使用这些 skills 来辅助安全分析：

### 工作流程

```
1. 业务智能体发送 pre_action 事件
   ↓
2. AHP Server 调用 Dangerous Operation Detector
   ↓
3. 如果检测到 critical 威胁 → 直接阻止（快速路径）
   ↓
4. 否则，将 Skill 结果传递给 LLM 进行深度分析
   ↓
5. LLM 结合 Skill 分析和上下文理解做出最终决策
   ↓
6. 返回决策给业务智能体
```

### 双层防护

**第一层：Safety Skills（快速、确定性）**
- 基于规则的模式匹配
- 毫秒级响应
- 捕获已知威胁
- 对 critical 威胁直接阻止

**第二层：LLM 分析（智能、上下文感知）**
- 理解操作意图
- 考虑上下文和历史
- 识别复杂攻击
- 提供自然语言解释

### 配置

在 `ahp_server_agent.py` 中，Skills 会自动导入：

```python
from skills import detect_dangerous_operation, sanitize_untrusted_output
```

如果 Skills 不可用，AHP Server 会回退到纯 LLM 分析模式。

## 独立测试

### 测试危险操作检测器

```bash
cd /path/to/examples/skills
python3 dangerous_operation_detector.py
```

输出示例：
```
======================================================================
Dangerous Operation Detector - 测试
======================================================================

测试: 危险删除
工具: bash
参数: {'command': 'rm -rf /'}

结果:
  危险: True
  风险等级: critical
  威胁数量: 1
  威胁:
    - 递归删除根目录 (风险: critical)
  建议:
    - 避免使用危险的系统命令，使用更安全的替代方案
    - 如果必须执行，请使用明确的路径和参数
```

### 测试输出净化器

```bash
cd /path/to/examples/skills
python3 untrusted_output_sanitizer.py
```

输出示例：
```
======================================================================
Untrusted Output Sanitizer - 测试
======================================================================

测试: 包含 API Key
工具: read
原始输出: API_KEY=sk-1234567890abcdefghijklmnopqrstuvwxyz
DATABASE_URL=postgres://localhost

结果:
  安全: False
  风险等级: critical
  威胁数量: 2
  脱敏数量: 2
  威胁:
    - 检测到 API Key (风险: critical)
    - 检测到 Database Connection (风险: critical)
  脱敏:
    - API Key: sk-1234567890abcdef... → [REDACTED_API_KEY]
    - Database Connection: postgres://localhost → [REDACTED_DB_CONNECTION]
  净化后输出: API_KEY=[REDACTED_API_KEY]
DATABASE_URL=[REDACTED_DB_CONNECTION]
```

## 扩展 Skills

### 添加新的危险模式

编辑 `dangerous_operation_detector.py`:

```python
DANGEROUS_PATTERNS = [
    # ... 现有模式 ...
    (r"your_pattern", "描述", "risk_level"),
]
```

### 添加新的 PII 模式

编辑 `untrusted_output_sanitizer.py`:

```python
PII_PATTERNS = [
    # ... 现有模式 ...
    (r"your_pattern", "[REDACTED_TYPE]", "类型", "risk_level"),
]
```

### 创建新的 Skill

1. 在 `skills/` 目录创建新文件
2. 实现检测/净化逻辑
3. 在 `__init__.py` 中导出
4. 在 `ahp_server_agent.py` 中集成

## 性能

### Dangerous Operation Detector
- 平均响应时间: < 1ms
- 内存占用: < 1MB
- 适合实时检测

### Untrusted Output Sanitizer
- 平均响应时间: < 5ms（取决于输出大小）
- 内存占用: < 2MB
- 支持最大 100KB 输出

## 最佳实践

1. **快速路径优化**: 对 critical 威胁直接阻止，无需 LLM 分析
2. **结合使用**: Skills 提供快速检测，LLM 提供深度分析
3. **定期更新**: 根据新威胁更新模式库
4. **监控误报**: 记录 Skill 和 LLM 决策的差异
5. **性能优先**: 在高负载场景下，可以只使用 Skills

## 故障处理

如果 Skills 不可用：
- AHP Server 会自动回退到纯 LLM 分析
- 日志会显示警告信息
- 功能不受影响，但响应时间会增加

确保 Skills 可用：
```bash
# 方法 1: 添加到 PYTHONPATH
export PYTHONPATH=/path/to/examples:$PYTHONPATH

# 方法 2: 安装为包
cd /path/to/examples
pip install -e .
```

## 总结

这两个 Safety Skills 为 AHP Server 提供了：
- ✅ 快速的威胁检测（毫秒级）
- ✅ 确定性的规则匹配
- ✅ 自动的 PII 脱敏
- ✅ 与 LLM 分析的完美结合
- ✅ 可扩展的模式库

它们是 AHP 安全监控架构的重要组成部分！
