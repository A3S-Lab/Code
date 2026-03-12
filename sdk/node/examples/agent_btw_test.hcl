# A3S Code Test Configuration
# Uses environment variables for sensitive data

default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("KIMI_API_KEY")
  base_url = env("KIMI_BASE_URL")

  models {
    id        = "kimi-k2.5"
    name      = "KIMI K2.5"
    family    = "kimi"
    tool_call = true

    limit {
      context = 256000
      output  = 8192
    }
  }
}

storage_backend = "memory"
max_tool_rounds = 20
