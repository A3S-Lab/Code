# 添加 Skill Kind: tool 支持 - 完整总结

## 问题背景

用户的 skill 文件使用了 `kind: tool`，但这不是有效的 skill kind 值，导致 skill 解析失败。

原有的有效值只有：
- `instruction` (默认)
- `persona`

## 解决方案

为 skill 系统添加 `tool` 作为第三种有效的 kind 类型。

## 修改内容

### 1. Core 库修改

#### `crates/code/core/src/skills/mod.rs`

**添加 `Tool` 枚举值**：

```rust
pub enum SkillKind {
    #[default]
    Instruction,
    Persona,
    Tool,  // 新增
}
```

**更新文档**：

```rust
//! ## Skill Kinds
//!
//! - `instruction` (default): Injected into system prompt when matched
//! - `persona`: Session-level system prompt (bound at session creation)
//! - `tool`: Tool-like skill with specialized functionality (treated like instruction)
```

#### `crates/code/core/src/skills/registry.rs`

**更新 `to_system_prompt()` 方法**：

```rust
.filter(|s| {
    // 包含 Instruction 和 Tool 类型
    s.kind == SkillKind::Instruction || s.kind == SkillKind::Tool
})
```

**更新 `match_skills()` 方法**：

```rust
.filter(|s| {
    // 包含 Instruction 和 Tool 类型
    s.kind == SkillKind::Instruction || s.kind == SkillKind::Tool
})
```

### 2. Python SDK 修改

#### `crates/code/sdk/python/src/lib.rs`

**添加 `Tool` 类型的 Python 绑定**：

```rust
kind: match s.kind {
    RustSkillKind::Instruction => "instruction".to_string(),
    RustSkillKind::Persona => "persona".to_string(),
    RustSkillKind::Tool => "tool".to_string(),  // 新增
},
```

### 3. 测试修复

#### `crates/code/core/tests/test_subagent_permissions.rs`

**添加缺失的 `skill_dirs` 字段**：

```rust
let config = SubAgentConfig {
    // ... 其他字段
    skill_dirs: vec![],  // 新增
    // ...
};
```

## 测试结果

### 单元测试

```bash
$ cargo test --lib
test result: ok. 1477 passed; 0 failed; 3 ignored
```

✅ 所有单元测试通过

### Python SDK 测试

```bash
$ python3 test_tool_kind.py
✅ SUCCESS!
The 'tool' kind is now supported for skills!
```

✅ Tool kind 功能正常

## 使用示例

### 正确的 Skill 文件格式

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
```

## Skill Kind 行为对比

| Kind | 何时注入 | 用途 | 行为 |
|------|---------|------|------|
| `instruction` | 匹配时 | 通用指令 | 注入到系统提示 |
| `persona` | 会话创建时 | 角色扮演 | 会话级系统提示 |
| `tool` | 匹配时 | 工具功能 | 注入到系统提示（与 instruction 相同） |

**注意**：`tool` 和 `instruction` 的行为完全相同，区别只是语义分类。

## 文件清单

### 修改的文件

1. `crates/code/core/src/skills/mod.rs` - 添加 `Tool` 枚举值和文档
2. `crates/code/core/src/skills/registry.rs` - 更新过滤逻辑
3. `crates/code/sdk/python/src/lib.rs` - 添加 Python 绑定
4. `crates/code/core/tests/test_subagent_permissions.rs` - 修复测试

### 新增的文件

1. `test_tool_kind.py` - Tool kind 功能测试
2. `example_tool_skill.md` - Tool skill 示例
3. `TOOL_KIND_SUPPORT.md` - 功能文档
4. `TOOL_KIND_SUMMARY.md` - 本文件

## 版本信息

- **Core 版本**: a3s-code-core v1.4.4
- **Python SDK 版本**: a3s-code v1.4.4
- **新增功能**: `kind: tool` 支持

## 向后兼容性

✅ **完全向后兼容**

- 现有的 `instruction` 和 `persona` skill 不受影响
- 只是新增了第三种有效的 kind 值
- 不会破坏任何现有代码

## 总结

✅ **`kind: tool` 现在是有效的 skill 类型！**

用户的原始问题已完全解决：

1. ✅ Python SDK 属性访问 - 已修复
2. ✅ Rust 层参数传递 - 已验证正确
3. ✅ Skill 文件格式 - 添加了 `tool` kind 支持
4. ✅ 所有测试通过

用户现在只需要修复 skill 文件中的引号问题：

```yaml
---
name: scoring-video-adapter
description: "视频评分适配器"  # ✓ 引号闭合
kind: tool                     # ✓ 现在是有效值
allowed-tools: "mcp_video-processor_(*), mcp_longvt__(*), Bash(*), Read(*), Write(*)"
---
```

然后 skill 就能正常工作了！
