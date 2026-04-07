# Test config for a3s-code SDK
# This is the format expected by Agent.create()

default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("OPENAI_API_KEY")
  base_url = env("OPENAI_BASE_URL")

  models {
    id        = "kimi-k2.5"
    name      = "Kimi"
    tool_call = true
  }
}
