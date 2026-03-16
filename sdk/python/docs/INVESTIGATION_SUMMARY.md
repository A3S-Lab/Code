# SubAgentConfig skill_dirs 问题分析总结

## 问题报告

用户报告：SubAgentConfig 的 `skill_dirs` 参数在 v1.4.4 中仍然无效，sub-agent 找不到 skill。

## 调查结果

### ✅ 已修复：Python SDK 属性访问

**问题**：`hasattr(cfg, 'skill_dirs')` 返回 `False`

**修复**：为 `PySubAgentConfig` 添加了 12 个字段的 getter/setter

**验证**：所有测试通过 ✅

### ✅ 已验证：Rust 层参数传递正确

**代码路径**：
1. `wrapper.rs:123-125` - 从 `SubAgentConfig` 读取 `skill_dirs`
2. `wrapper.rs:124` - 传递给 `SessionOptions`
3. `agent_api.rs:1084` - 加载 skill 文件

**结论**：Rust 层代码完全正确，参数传递无问题。

### ❌ 真正的问题：Skill 文件格式错误

用户提供的 skill 文件 frontmatter：

```yaml
name: scoring-video-adapter
description:"视频评分适配器      # ❌ 引号未闭合
kind:tool                         # ❌ 无效值（应为 instruction 或 persona）
allowed-tools: "..."
```

**问题分析**：

1. **YAML 解析失败**：引号未闭合导致 YAML 解析错误
2. **无效的 kind 值**：`tool` 不是有效值（只能是 `instruction` 或 `persona`）
3. **静默失败**：解析失败时只记录 warning，不会抛出错误

**代码证据**（`registry.rs:143-150`）：

```rust
match Skill::from_file(&path) {
    Ok(skill) => {
        // 注册成功
    },
    Err(e) => {
        tracing::warn!("Failed to parse skill file {}: {}", path.display(), e);
        // 静默跳过，不注册
    }
}
```

## 解决方案

### 1. 修复 Skill 文件格式

**正确的格式**：

```yaml
---
name: scoring-video-adapter
description: "视频评分适配器"
kind: instruction
allowed-tools: "mcp_video-processor_(*), mcp_longvt__(*), Bash(*), Read(*), Write(*)"
---
# Skill Content

Your instructions here...
```

### 2. 使用诊断工具

```bash
python3 diagnose_skill_dirs.py
```

输入 skill 文件路径，工具会检查：
- Frontmatter 结构
- 必需字段
- 引号闭合
- 无效字段
- 文件扩展名

### 3. 启用 Debug 日志

```python
import logging
logging.basicConfig(level=logging.DEBUG)
```

查找日志消息：
- `"Loaded skill 'xxx' from ..."`（成功）
- `"Failed to parse skill file ..."`（失败）

## 提供的工具和文档

1. **diagnose_skill_dirs.py** - 诊断工具，检查 skill 文件格式
2. **SKILL_FORMAT_ISSUE.md** - Skill 格式问题详细分析
3. **TROUBLESHOOTING_GUIDE.md** - 完整排查指南
4. **example_skill_correct.md** - 正确的 skill 文件示例
5. **TEST_RESULTS.md** - SDK 修复测试结果
6. **QUICK_REFERENCE.md** - 快速参考指南

## 结论

**skill_dirs 参数本身没有问题**，问题在于：

1. ✅ Python SDK 属性访问 - 已修复并验证
2. ✅ Rust 层参数传递 - 代码正确
3. ❌ **Skill 文件格式** - 用户的 skill 文件有格式错误

**用户需要做的**：

1. 修复 skill 文件的 frontmatter 格式
2. 使用诊断工具验证
3. 启用 debug 日志查看加载过程

**如果修复后仍有问题**，请提供：
- Debug 日志完整输出
- Skill 文件完整内容（包括 frontmatter 和 body）
- 使用的完整 Python 代码
