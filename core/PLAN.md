# 精简优化计划

## 用户要求

1. **删除** 动态工具扩展（tools/dynamic, skill_discovery, skill_catalog, skill_loader）
2. **删除** context-store 模块
3. **删除** cron 集成（已 feature-gated，直接移除）
4. **安全模块** — 只保留接口（trait + config），删除具体实现
5. **记忆模块** — 只保留接口（trait + types），删除具体实现

## 步骤

### Step 1: 删除动态工具扩展

**删除文件:**
- `src/tools/dynamic/binary.rs`
- `src/tools/dynamic/http.rs`
- `src/tools/dynamic/script.rs`
- `src/tools/skill_discovery.rs`
- `src/tools/skill_catalog.rs`
- `src/tools/skill_loader.rs`

**保留:** `read_process_output` 和 `substitute_template_args` 从 `dynamic/mod.rs` 移到 `tools/process.rs`（bash.rs 依赖 `read_process_output`）

**修改:**
- `tools/mod.rs` — 移除 dynamic/skill_discovery/skill_catalog/skill_loader 模块声明和 re-exports，移除 ToolExecutor 中 skill discovery 工具注册
- `tools/builtin/bash.rs` — 改为 `use crate::tools::process::read_process_output`
- `tools/types.rs` — 检查 ToolBackend enum 是否需要精简（移除 Binary/Http/Script variants）
- `lib.rs` — 移除相关 re-exports
- `agent.rs` — 移除 `use crate::tools::skill::Skill` 和 AgentConfig 中的 `skill_tool_filters`

### Step 2: 删除 context-store 模块

**删除目录:** `src/context_store/` 整个目录

**修改:**
- `lib.rs` — 移除 `#[cfg(feature = "context-store")] pub mod context_store;`
- `Cargo.toml` — 移除 `context-store = ["walkdir"]` feature 和 `walkdir` 依赖
- `session/mod.rs` — 移除 context-store 相关的 cfg 分支和 A3SContextClient/A3SContextProvider 引用

### Step 3: 删除 cron 集成

**修改:**
- `Cargo.toml` — 移除 `cron` feature（如果还在的话，之前已 feature-gated）
- 检查是否有残留引用

### Step 4: 安全模块 — 只保留接口

**保留:**
- `security/config.rs` — SecurityConfig, FeatureToggles, RedactionStrategy 等配置类型
- `security/mod.rs` — SecurityGuard 结构体改为 trait `SecurityProvider`

**删除具体实现:**
- `security/audit.rs` — 删除
- `security/classifier.rs` — 删除
- `security/injection.rs` — 删除
- `security/interceptor.rs` — 删除
- `security/sanitizer.rs` — 删除
- `security/taint.rs` — 删除

**新增 trait:**
```rust
pub trait SecurityProvider: Send + Sync {
    fn taint_input(&self, text: &str);
    fn sanitize_output(&self, text: &str) -> String;
    fn wipe(&self);
}
```

**修改:**
- `session/mod.rs` — 改用 `Arc<dyn SecurityProvider>` 替代 `Arc<SecurityGuard>`

### Step 5: 记忆模块 — 只保留接口

**保留:**
- `MemoryConfig`, `RelevanceConfig` — 配置类型
- `MemoryItem`, `MemoryType` — 数据类型
- `MemoryStore` trait — 存储接口
- `MemoryStats` — 统计类型
- `MemoryContextProvider` — 上下文提供者（依赖 ContextProvider trait）

**删除具体实现:**
- `InMemoryStore` — 删除
- `FileStore` — 删除
- `AgentMemory` 中的具体逻辑 — 简化或删除，让外部实现
- 所有 helper 函数（search_memories, sort_by_relevance 等）

**修改:**
- `session/mod.rs` — 改用 `Arc<dyn MemoryStore>` 替代 `Arc<RwLock<AgentMemory>>`
- `config.rs` — 保留 MemoryConfig 引用

## 执行顺序

1 → 2 → 3 → 4 → 5，每步完成后跑测试确认编译通过
