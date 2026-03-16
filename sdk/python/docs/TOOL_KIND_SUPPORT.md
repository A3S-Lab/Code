# Skill Kind: tool 支持已添加

## 更新内容

### 1. 新增 `SkillKind::Tool` 枚举值

**文件**: `crates/code/core/src/skills/mod.rs`

```rust
pub enum SkillKind {
    Instruction,  // 指令型（默认）
    Persona,      // 人格型
    Tool,         // 工具型（新增）
}
```

### 2. 更新 Skill Registry 处理逻辑

**文件**: `crates/code/core/src/skills/registry.rs`

- `to_system_prompt()` - 现在包含 `Tool` 类型的 skill
- `match_skills()` - 现在匹配 `Tool` 类型的 skill

```rust
.filter(|s| {
    // 包含 Instruction 和 Tool 类型
    s.kind == SkillKind::Instruction || s.kind == SkillKind::Tool
})
```

### 3. 更新 Python SDK 绑定

**文件**: `crates/code/sdk/python/src/lib.rs`

```rust
kind: match s.kind {
    RustSkillKind::Instruction => "instruction".to_string(),
    RustSkillKind::Persona => "persona".to_string(),
    RustSkillKind::Tool => "tool".to_string(),  // 新增
},
```

## 行为说明

### Tool Kind 的行为

`kind: tool` 的 skill 会：

1. ✅ 被加载到 skill registry
2. ✅ 出现在系统提示的 skill 目录中
3. ✅ 可以通过 `Call Skill('name')` 调用
4. ✅ 支持 `allowed-tools` 权限控制
5. ✅ 与 `instruction` 类型行为相同（都会被注入到系统提示）

### 与其他 Kind 的区别

| Kind | 注入时机 | 用途 | 示例 |
|------|---------|------|------|
| `instruction` | 匹配时注入 | 通用指令 | 代码审查、文档生成 |
| `persona` | 会话创建时 | 角色扮演 | 专家助手、特定风格 |
| `tool` | 匹配时注入 | 工具功能 | 视频处理、数据分析 |

**实际上**：`tool` 和 `instruction` 的行为完全相同，区别只是语义上的分类。

## 使用示例

### 正确的 Tool Skill 格式

```markdown
---
name: scoring-video-adapter
description: "视频评分适配器"
kind: tool
allowed-tools: "mcp_video-processor_(*), mcp_longvt__(*), Bash(*), Read(*), Write(*)"
---
# Scoring Video Adapter

Your tool skill instructions here...
```

### Python 代码

```python
from a3s_code import Agent, Orchestrator, SubAgentConfig

agent = Agent.create("config.hcl")
orchestrator = Orchestrator.create(agent=agent)

handle = orchestrator.spawn_subagent(SubAgentConfig(
    agent_type="test",
    prompt="Call Skill('scoring-video-adapter')",
    workspace="/your/workspace",
    permissive=True,
    skill_dirs=["/absolute/path/to/skills"],
))

result = handle.wait()
print(result)
```

## 测试验证

运行测试脚本：

```bash
cd crates/code/sdk/python
python3 test_tool_kind.py
```

预期输出：

```
✅ SUCCESS!
The 'tool' kind is now supported for skills!
```

## 版本信息

- **Core 版本**: a3s-code-core v1.4.4
- **Python SDK 版本**: a3s-code v1.4.4
- **新增功能**: `kind: tool` 支持

## 文件清单

1. **修改的文件**:
   - `crates/code/core/src/skills/mod.rs` - 添加 `Tool` 枚举值
   - `crates/code/core/src/skills/registry.rs` - 更新过滤逻辑
   - `crates/code/sdk/python/src/lib.rs` - 添加 Python 绑定

2. **测试文件**:
   - `test_tool_kind.py` - Tool kind 功能测试

3. **示例文件**:
   - `example_tool_skill.md` - 正确的 tool skill 示例

## 总结

✅ **`kind: tool` 现在是有效的 skill 类型！**

你的原始 skill 文件现在只需要修复引号问题即可正常工作：

```yaml
---
name: scoring-video-adapter
description: "视频评分适配器"  # ✓ 引号闭合
kind: tool                     # ✓ 现在是有效值
allowed-tools: "mcp_video-processor_(*), mcp_longvt__(*), Bash(*), Read(*), Write(*)"
---
```
