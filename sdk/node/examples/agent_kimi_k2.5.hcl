# A3S Code Configuration for Kimi K2.5 Model
# API key is read from the KIMI_API_KEY environment variable.

default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("KIMI_API_KEY")
  base_url = env("KIMI_BASE_URL")

  models {
    id   = "kimi-k2.5"
    name = "KIMI K2.5"
  }
}

storage_backend = "memory"
max_tool_rounds = 20
