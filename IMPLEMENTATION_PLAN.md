# A3S Code 默认实现计划

## 目标

为 A3S Code 提供开箱即用的默认实现，通过实现现有的 trait 接口来支持：
1. **SecurityProvider** - 默认安全提供者（taint tracking, output sanitization, injection detection）
2. **ContextProvider** - 默认上下文提供者（基于文件系统的简单 RAG）
3. **ConfirmationProvider** - 已有 `ConfirmationManager` 实现，需要确认是否完整

## 现有 Trait 接口

### 1. SecurityProvider (core/src/security/mod.rs)
```rust
pub trait SecurityProvider: Send + Sync {
    fn taint_input(&self, text: &str) {}
    fn sanitize_output(&self, text: &str) -> String { text.to_string() }
    fn wipe(&self) {}
    fn register_hooks(&self, hook_engine: &HookEngine) {}
    fn teardown(&self, hook_engine: &HookEngine) {}
}
```

**当前状态：** 只有 `NoOpSecurityProvider`（空实现）

**需要实现：** `DefaultSecurityProvider`
- Taint tracking: 检测和跟踪敏感数据（SSN, API keys, emails, phone numbers）
- Output sanitization: 自动 redact 敏感数据
- Injection detection: 检测 prompt injection 攻击
- Hook integration: 通过 PreToolUse/PostToolUse hooks 集成

### 2. ContextProvider (core/src/context.rs)
```rust
#[async_trait]
pub trait ContextProvider: Send + Sync {
    async fn query(&self, query: ContextQuery) -> Result<ContextResult>;
    async fn on_turn_complete(&self, messages: &[Message]) -> Result<()> { Ok(()) }
}
```

**当前状态：** 有 `MemoryContextProvider`（连接 Memory 系统）

**需要实现：** `FileSystemContextProvider`
- 基于文件系统的简单 RAG
- 支持 glob 模式搜索文件
- 基于关键词的简单相关性评分
- 可选：使用 BM25 算法提升搜索质量

### 3. ConfirmationProvider (core/src/hitl.rs)
```rust
#[async_trait]
pub trait ConfirmationProvider: Send + Sync {
    async fn requires_confirmation(&self, tool_name: &str) -> bool;
    async fn request_confirmation(...) -> oneshot::Receiver<ConfirmationResponse>;
    async fn confirm(&self, tool_id: &str, approved: bool, reason: Option<String>) -> Result<bool, String>;
    async fn policy(&self) -> ConfirmationPolicy;
    async fn set_policy(&self, policy: ConfirmationPolicy);
}
```

**当前状态：** 已有 `ConfirmationManager` 实现（需要验证）

## 实现计划

### Phase 1: DefaultSecurityProvider

**文件：** `core/src/security/default.rs`

**功能：**
1. **Taint Tracking**
   - 使用 regex 检测敏感数据模式
   - 支持的模式：
     - SSN: `\d{3}-\d{2}-\d{4}`
     - Email: `[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}`
     - Phone: `\+?\d{1,3}?[-.\s]?\(?\d{1,4}\)?[-.\s]?\d{1,4}[-.\s]?\d{1,9}`
     - API Keys: `(sk|pk)[-_][a-zA-Z0-9]{20,}`
     - Credit Card: `\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}`
   - 存储在 `Arc<RwLock<HashSet<String>>>` 中

2. **Output Sanitization**
   - 替换检测到的敏感数据为 `[REDACTED]`
   - 保留数据类型提示：`[REDACTED:EMAIL]`, `[REDACTED:SSN]`

3. **Injection Detection**
   - 检测常见 prompt injection 模式：
     - "Ignore previous instructions"
     - "You are now in developer mode"
     - "Disregard all prior context"
   - 返回警告但不阻止（通过 hook 发出 SecurityWarning 事件）

4. **Hook Integration**
   - PreToolUse: 检查工具参数中的敏感数据
   - PostToolUse: Sanitize 工具输出
   - GenerateStart: Injection detection
   - GenerateEnd: Sanitize LLM 输出

**配置：**
```rust
pub struct DefaultSecurityConfig {
    pub enable_taint_tracking: bool,
    pub enable_output_sanitization: bool,
    pub enable_injection_detection: bool,
    pub custom_patterns: Vec<(String, String)>, // (name, regex)
}
```

### Phase 2: FileSystemContextProvider

**文件：** `core/src/context/fs_provider.rs`

**功能：**
1. **File Indexing**
   - 扫描指定目录
   - 支持 glob 模式过滤（如 `**/*.rs`, `**/*.md`）
   - 提取文件内容和元数据

2. **Simple Search**
   - 基于关键词匹配
   - 计算简单相关性分数（关键词出现次数 / 文档长度）
   - 返回 top-k 结果

3. **Optional: BM25 Search**
   - 使用 `tantivy` 或 `sonic` crate
   - 更好的相关性排序
   - 支持 fuzzy search

**配置：**
```rust
pub struct FileSystemContextConfig {
    pub root_path: PathBuf,
    pub include_patterns: Vec<String>, // ["**/*.rs", "**/*.md"]
    pub exclude_patterns: Vec<String>, // ["**/target/**", "**/node_modules/**"]
    pub max_file_size: usize,          // 1MB default
    pub use_bm25: bool,                // false = simple keyword, true = BM25
}
```

### Phase 3: 验证 ConfirmationManager

**任务：**
1. 检查 `ConfirmationManager` 是否完整实现了 `ConfirmationProvider` trait
2. 如果缺失，补充实现
3. 添加测试

### Phase 4: 集成和文档

**任务：**
1. 在 `SessionOptions` 中添加便捷方法：
   ```rust
   impl SessionOptions {
       pub fn with_default_security(mut self) -> Self {
           self.security_provider = Some(Arc::new(DefaultSecurityProvider::new()));
           self
       }

       pub fn with_fs_context(mut self, root_path: impl Into<PathBuf>) -> Self {
           let config = FileSystemContextConfig::new(root_path);
           self.context_provider = Some(Arc::new(FileSystemContextProvider::new(config)));
           self
       }
   }
   ```

2. 更新文档：
   - README.md: 添加默认实现示例
   - docs/security.mdx: 添加 DefaultSecurityProvider 使用指南
   - docs/context.mdx: 添加 FileSystemContextProvider 使用指南

3. 添加示例：
   - `core/examples/default_security.rs`
   - `core/examples/fs_context.rs`

## 依赖项

需要添加的 crates（可选）：
- `regex` - 用于敏感数据检测（已有）
- `tantivy` 或 `sonic` - 用于 BM25 搜索（可选）
- `walkdir` - 用于文件系统遍历（可能已有）

## 测试计划

### DefaultSecurityProvider
- 测试 taint tracking 检测各种敏感数据
- 测试 output sanitization 正确 redact
- 测试 injection detection 识别攻击模式
- 测试 hook integration 正确触发

### FileSystemContextProvider
- 测试文件索引和过滤
- 测试关键词搜索和相关性评分
- 测试 glob 模式匹配
- 测试大文件处理

### Integration Tests
- 测试 DefaultSecurityProvider + ConfirmationManager 组合
- 测试 FileSystemContextProvider + MemoryContextProvider 组合
- 测试完整的 session 流程

## 时间估算

- Phase 1 (DefaultSecurityProvider): 4-6 小时
- Phase 2 (FileSystemContextProvider): 3-4 小时
- Phase 3 (验证 ConfirmationManager): 1-2 小时
- Phase 4 (集成和文档): 2-3 小时
- 测试: 2-3 小时

**总计：** 12-18 小时

## 优先级

1. **High**: DefaultSecurityProvider（安全是核心需求）
2. **Medium**: FileSystemContextProvider（RAG 是常见需求）
3. **Low**: BM25 优化（可以后续添加）

## 下一步

1. 确认实现计划
2. 开始实现 Phase 1: DefaultSecurityProvider
3. 编写测试
4. 更新文档
