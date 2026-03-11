# Test config using kimi-k2.5
# Set KIMI_API_KEY and KIMI_BASE_URL environment variables before running.
default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  base_url = env("KIMI_BASE_URL")
  api_key  = env("KIMI_API_KEY")

  models {
    id   = "kimi-k2.5"
    name = "KIMI K2.5"
  }
}
