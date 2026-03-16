# SubAgentConfig skill_dirs 问题完整排查指南

## 问题现象

Sub-agent 报错：`Skill 'xxx' not found`，即使：
- ✓ Skill 文件存在
- ✓ 路径正确（绝对路径）
- ✓ skill_dirs 参数已传入

## 根本原因分析

经过代码审查，发现问题**不在 skill_dirs 参数传递**，而在：

### 1. Skill 文件格式错误（最常见）

用户报告的 skill 文件 frontmatter：

```yaml
name: scoring-video-adapter
description:"视频评分适配器      # ❌ 引号未闭合
kind:tool                         # ❌ 无效值（应为 instruction 或 persona）
allowed-tools: "..."
```

**问题**：
- YAML 解析失败 → Skill 加载失败 → 不会注册到 registry
- 错误会被静默跳过，只有 warning 日志

**修复**：

```yaml
name: scoring-video-adapter
description: "视频评分适配器"    # ✓ 引号闭合
kind: instruction                # ✓ 有效值
allowed-tools: "..."
```

### 2. 文件扩展名必须是 .md

```rust
// From registry.rs:138
if path.extension().and_then(|s| s.to_str()) != Some("md") {
    continue;  // 跳过非 .md 文件
}
```

### 3. Frontmatter 必须用 --- 包围

```markdown
---
name: skill-name
description: "..."
---
# Skill Content
```

## 排查步骤

### Step 1: 验证 Skill 文件格式

运行诊断脚本：

```bash
cd crates/code/sdk/python
python3 diagnose_skill_dirs.py
```

输入你的 skill 文件路径，脚本会检查：
- ✓ Frontmatter 结构
- ✓ 必需字段（name）
- ✓ 引号闭合
- ✓ 无效字段
- ✓ 文件扩展名

### Step 2: 启用 Debug 日志

```python
import logging
logging.basicConfig(
    level=logging.DEBUG,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)

# 然后运行你的代码
```

查找这些日志消息：

**成功加载**：
```
DEBUG - Loaded skill 'scoring-video-adapter' from /path/to/skills/scoring-video-adapter.md
```

**解析失败**：
```
WARN - Failed to parse skill file /path/to/skills/scoring-video-adapter.md: ...
```

**验证失败**：
```
WARN - Skill validation failed for /path/to/skills/scoring-video-adapter.md: ...
```

### Step 3: 验证路径

```python
import os

skill_dir = "/your/skill/dir"
skill_file = os.path.join(skill_dir, "scoring-video-adapter.md")

print(f"Directory exists: {os.path.isdir(skill_dir)}")
print(f"File exists: {os.path.isfile(skill_file)}")
print(f"Absolute path: {os.path.abspath(skill_dir)}")
```

### Step 4: 手动测试 Skill 解析

创建测试脚本：

```python
# test_skill_parsing.py
import yaml

skill_file = "/path/to/your/skill.md"

with open(skill_file, 'r') as f:
    content = f.read()

# 检查 frontmatter
parts = content.split("---")
if len(parts) < 3:
    print("❌ Invalid frontmatter structure")
    exit(1)

frontmatter = parts[1].strip()
print("Frontmatter:")
print(frontmatter)
print()

# 尝试解析 YAML
try:
    data = yaml.safe_load(frontmatter)
    print("✓ YAML parsing successful")
    print(f"  name: {data.get('name')}")
    print(f"  description: {data.get('description')}")
    print(f"  kind: {data.get('kind')}")
except yaml.YAMLError as e:
    print(f"❌ YAML parsing failed: {e}")
    exit(1)

# 检查必需字段
if not data.get('name'):
    print("❌ Missing required field: name")
    exit(1)

# 检查 kind 值
kind = data.get('kind', 'instruction')
if kind not in ['instruction', 'persona']:
    print(f"❌ Invalid kind value: {kind}")
    print("   Valid values: instruction, persona")
    exit(1)

print("\n✅ Skill file format is valid!")
```

## 常见错误和修复

### 错误 1: 引号未闭合

```yaml
# ❌ 错误
description:"视频评分适配器

# ✓ 正确
description: "视频评分适配器"
```

### 错误 2: 无效的 kind 值

```yaml
# ❌ 错误
kind: tool

# ✓ 正确
kind: instruction
```

### 错误 3: 字段名拼写错误

```yaml
# ❌ 错误
allowed_tools: "..."    # 下划线

# ✓ 正确
allowed-tools: "..."    # 连字符
```

### 错误 4: 缺少 frontmatter 分隔符

```markdown
❌ 错误：
name: my-skill
description: "..."

# Content

✓ 正确：
---
name: my-skill
description: "..."
---
# Content
```

### 错误 5: 相对路径

```python
# ❌ 可能有问题
skill_dirs=["./skills"]

# ✓ 推荐使用绝对路径
import os
skill_dirs=[os.path.abspath("./skills")]
```

## 验证修复

修复 skill 文件后，运行完整测试：

```python
from a3s_code import Agent, Orchestrator, SubAgentConfig
import logging
import os

# 启用 debug 日志
logging.basicConfig(level=logging.DEBUG)

# 使用绝对路径
skill_dir = os.path.abspath("/path/to/skills")
workspace = os.path.abspath("/path/to/workspace")

print(f"Skill directory: {skill_dir}")
print(f"Workspace: {workspace}")

# 创建 agent 和 orchestrator
agent = Agent.create("config.hcl")
orchestrator = Orchestrator.create(agent=agent)

# 创建 sub-agent config
config = SubAgentConfig(
    agent_type="test-agent",
    prompt="Call Skill('scoring-video-adapter')",
    workspace=workspace,
    permissive=True,
    skill_dirs=[skill_dir],
)

print(f"\nSubAgentConfig:")
print(f"  skill_dirs: {config.skill_dirs}")
print(f"  workspace: {config.workspace}")

# 启动 sub-agent
print("\nSpawning sub-agent...")
handle = orchestrator.spawn_subagent(config)

# 等待结果
print("Waiting for result...")
result = handle.wait()

print(f"\nResult: {result}")
```

## 预期输出

**成功时**：
```
DEBUG - Loaded skill 'scoring-video-adapter' from /path/to/skills/scoring-video-adapter.md
DEBUG - Skill registry now has 6 skills (5 built-in + 1 custom)
...
Result: <skill execution output>
```

**失败时**：
```
WARN - Failed to parse skill file /path/to/skills/scoring-video-adapter.md: ...
...
Error: Skill 'scoring-video-adapter' not found
```

## 总结

**skill_dirs 参数传递是正常的**（我们已经验证了 Rust 层代码），问题在于：

1. ✅ Python SDK 属性访问 - 已修复
2. ✅ Rust 层参数传递 - 代码正确
3. ❌ **Skill 文件格式** - 这是最可能的问题

**下一步**：
1. 使用 `diagnose_skill_dirs.py` 检查 skill 文件
2. 修复 frontmatter 格式错误
3. 启用 debug 日志查看加载过程
4. 使用绝对路径

如果修复后仍然有问题，请提供：
- Debug 日志输出
- Skill 文件完整内容
- 使用的完整代码
