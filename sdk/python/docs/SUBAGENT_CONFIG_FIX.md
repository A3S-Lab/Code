# SubAgentConfig 类型暴露修复

## 问题描述

在 a3s_code Python SDK 1.4.3 和 1.4.4 版本中，`SubAgentConfig` 类接受 `skill_dirs` 等参数，但这些参数在 Python 侧无法访问：

```python
cfg = SubAgentConfig(
    agent_type="my-agent",
    prompt="test",
    skill_dirs=["/tmp/skills"]
)

# ❌ 失败：AttributeError
print(cfg.skill_dirs)

# ❌ 返回 False
hasattr(cfg, 'skill_dirs')
```

## 根本原因

`PySubAgentConfig` 的 Rust 实现只有构造函数和 `__repr__`，没有为任何字段添加 getter/setter：

```rust
#[pyclass(name = "SubAgentConfig")]
struct PySubAgentConfig {
    inner: RustSubAgentConfig,  // 所有数据存在这里
}

#[pymethods]
impl PySubAgentConfig {
    #[new]
    fn new(...) -> Self { ... }  // ✓ 有构造函数

    fn __repr__(&self) -> String { ... }  // ✓ 有 repr

    // ❌ 缺少所有 getter/setter
}
```

虽然数据正确存储在 `inner` 中，但 Python 侧完全无法访问。

## 修复方案

为 `PySubAgentConfig` 添加所有字段的 getter/setter（参考 `PySessionOptions` 的实现模式）：

### 修改文件

`crates/code/sdk/python/src/lib.rs` 第 4162 行之前（`}` 之前）添加：

```rust
// Getters and setters for all fields

#[getter]
fn get_agent_type(&self) -> String {
    self.inner.agent_type.clone()
}

#[setter]
fn set_agent_type(&mut self, value: String) {
    self.inner.agent_type = value;
}

#[getter]
fn get_description(&self) -> String {
    self.inner.description.clone()
}

#[setter]
fn set_description(&mut self, value: String) {
    self.inner.description = value;
}

#[getter]
fn get_prompt(&self) -> String {
    self.inner.prompt.clone()
}

#[setter]
fn set_prompt(&mut self, value: String) {
    self.inner.prompt = value;
}

#[getter]
fn get_permissive(&self) -> bool {
    self.inner.permissive
}

#[setter]
fn set_permissive(&mut self, value: bool) {
    self.inner.permissive = value;
}

#[getter]
fn get_permissive_deny(&self) -> Vec<String> {
    self.inner.permissive_deny.clone()
}

#[setter]
fn set_permissive_deny(&mut self, value: Vec<String>) {
    self.inner.permissive_deny = value;
}

#[getter]
fn get_max_steps(&self) -> Option<usize> {
    self.inner.max_steps
}

#[setter]
fn set_max_steps(&mut self, value: Option<usize>) {
    self.inner.max_steps = value;
}

#[getter]
fn get_timeout_ms(&self) -> Option<u64> {
    self.inner.timeout_ms
}

#[setter]
fn set_timeout_ms(&mut self, value: Option<u64>) {
    self.inner.timeout_ms = value;
}

#[getter]
fn get_parent_id(&self) -> Option<String> {
    self.inner.parent_id.clone()
}

#[setter]
fn set_parent_id(&mut self, value: Option<String>) {
    self.inner.parent_id = value;
}

#[getter]
fn get_workspace(&self) -> String {
    self.inner.workspace.clone()
}

#[setter]
fn set_workspace(&mut self, value: String) {
    self.inner.workspace = value;
}

#[getter]
fn get_agent_dirs(&self) -> Vec<String> {
    self.inner.agent_dirs.clone()
}

#[setter]
fn set_agent_dirs(&mut self, value: Vec<String>) {
    self.inner.agent_dirs = value;
}

#[getter]
fn get_skill_dirs(&self) -> Vec<String> {
    self.inner.skill_dirs.clone()
}

#[setter]
fn set_skill_dirs(&mut self, value: Vec<String>) {
    self.inner.skill_dirs = value;
}

#[getter]
fn get_lane_config(&self) -> Option<PySessionQueueConfig> {
    self.inner.lane_config.as_ref().map(|lc| PySessionQueueConfig {
        inner: lc.clone(),
    })
}

#[setter]
fn set_lane_config(&mut self, value: Option<PySessionQueueConfig>) {
    self.inner.lane_config = value.map(|v| v.inner);
}
```

## 验证修复

### 1. 重新构建 Python SDK

```bash
cd crates/code/sdk/python
pip install -e .
```

### 2. 运行测试脚本

```bash
python test_subagent_config.py
```

预期输出：

```
Testing attribute access...
✓ agent_type: my-sub-agent
✓ prompt: Call Skill('scoring-video-adapter')
✓ workspace: /path/to/project
✓ permissive: True
✓ skill_dirs: ['/path/to/project/skills']

Testing hasattr...
✓ All attributes accessible via hasattr

Testing setter...
✓ skill_dirs updated to: ['/new/path']

✅ All tests passed! SubAgentConfig attributes are now accessible.
```

### 3. 验证原始问题已修复

```python
from a3s_code import SubAgentConfig

cfg = SubAgentConfig(
    agent_type="my-sub-agent",
    prompt="Call Skill('scoring-video-adapter')",
    skill_dirs=["/tmp/skills"]
)

# ✅ 现在可以访问了
print(cfg.skill_dirs)  # ['/tmp/skills']

# ✅ hasattr 返回 True
assert hasattr(cfg, 'skill_dirs')  # True

# ✅ 可以修改
cfg.skill_dirs = ["/new/path"]
print(cfg.skill_dirs)  # ['/new/path']
```

## 影响范围

- **修复的类**：`PySubAgentConfig`
- **新增功能**：12 个字段的 getter/setter（agent_type, description, prompt, permissive, permissive_deny, max_steps, timeout_ms, parent_id, workspace, agent_dirs, skill_dirs, lane_config）
- **向后兼容性**：✅ 完全兼容，只是新增了属性访问能力
- **运行时行为**：✅ 无变化，之前 `skill_dirs` 就已经正确传递给 Rust 层

## 注意事项

1. **这不是 Rust 层的 bug**：Rust 层代码一直正确处理 `skill_dirs`，问题只在 Python 绑定层
2. **运行时行为未改变**：修复前 sub-agent 就能正确加载 skills，只是 Python 侧无法验证参数
3. **类型检查改进**：修复后 IDE 和类型检查工具能正确识别 `SubAgentConfig` 的属性

## 相关文件

- 修改：`crates/code/sdk/python/src/lib.rs` (第 4162 行前)
- 测试：`crates/code/sdk/python/test_subagent_config.py`
- 文档：`crates/code/sdk/python/SUBAGENT_CONFIG_FIX.md` (本文件)
