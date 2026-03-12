# BTW 功能测试指南

## 为什么使用 env() 函数？

使用 HCL 的 `env()` 函数是管理敏感配置的最佳实践：

### ✅ 优点

1. **安全性**: API key 不会出现在代码或配置文件中
2. **灵活性**: 可以轻松切换不同环境（开发/测试/生产）
3. **标准化**: 符合 12-factor app 原则
4. **版本控制友好**: 配置文件可以安全地提交到 git

### ❌ 不推荐的做法

```hcl
# ❌ 硬编码 API key（不安全）
providers {
  api_key = "sk-abc123..."
}
```

### ✅ 推荐的做法

```hcl
# ✅ 使用环境变量（安全）
providers {
  api_key = env("KIMI_API_KEY")
}
```

## 测试文件说明

### 1. `test_agent.hcl` - 配置文件

使用 `env()` 函数从环境变量读取敏感信息：

```hcl
default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("KIMI_API_KEY")      # 从环境变量读取
  base_url = env("KIMI_BASE_URL")     # 从环境变量读取

  models {
    id = "kimi-k2.5"
    # ...
  }
}
```

### 2. `test_btw_env.ts` - 测试脚本

使用配置文件路径，不在代码中处理敏感信息：

```typescript
// ✅ 直接使用配置文件
const agent = await Agent.create('./test_agent.hcl');

// ❌ 不要在代码中动态生成包含 API key 的配置
const config = `api_key = "${process.env.API_KEY}"`;
```

### 3. `setup_test_env.sh` - 环境设置脚本

从现有配置读取并设置环境变量（仅用于测试）。

## 运行测试

### 方法 1: 使用 setup 脚本（推荐）

```bash
# 设置环境变量并运行测试
source setup_test_env.sh && npx ts-node test_btw_env.ts
```

### 方法 2: 手动设置环境变量

```bash
# 设置环境变量
export KIMI_API_KEY="sk-your-key-here"
export KIMI_BASE_URL="https://api.example.com/v1"

# 运行测试
npx ts-node test_btw_env.ts
```

### 方法 3: 使用 .env 文件（生产环境推荐）

创建 `.env` 文件（不要提交到 git）：

```bash
KIMI_API_KEY=sk-your-key-here
KIMI_BASE_URL=https://api.example.com/v1
```

使用 dotenv 加载：

```bash
npm install dotenv
node -r dotenv/config test_btw_env.js
```

## 测试输出

测试会自动脱敏显示敏感信息：

```
Environment configured:
  KIMI_BASE_URL: http://<REDACTED>/v1
  KIMI_API_KEY: sk-ZaH1Ynk...g5cT
```

## 最佳实践总结

| 场景 | 推荐做法 | 原因 |
|------|---------|------|
| 配置文件 | 使用 `env("VAR_NAME")` | 安全，可提交到 git |
| 本地开发 | `.env` 文件 + gitignore | 方便，不会泄露 |
| CI/CD | 环境变量或 secrets | 安全，易于管理 |
| 生产环境 | 环境变量或密钥管理服务 | 最安全 |

## 文件清单

- ✅ `test_agent.hcl` - 使用 env() 的配置文件
- ✅ `test_btw_env.ts` - 使用配置文件的测试脚本
- ✅ `setup_test_env.sh` - 环境设置脚本
- ❌ `test_btw.ts` - 旧版本（不推荐，动态生成配置）

## .gitignore 建议

```gitignore
# 敏感配置
.env
.env.local
*.key
*_secret.hcl

# 测试配置可以提交（使用 env() 函数）
test_agent.hcl
```

---

**关键原则**: 永远不要在代码或配置文件中硬编码敏感信息，始终使用环境变量。
