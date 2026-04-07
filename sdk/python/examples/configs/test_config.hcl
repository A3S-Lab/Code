# Test configuration for a3s-code Python SDK
# Uses environment variables for credentials

default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("OPENAI_API_KEY")
  base_url = env("OPENAI_BASE_URL")

  models {
    id        = "kimi-k2.5"
    name      = "Kimi K2.5"
    tool_call = true
  }
}
