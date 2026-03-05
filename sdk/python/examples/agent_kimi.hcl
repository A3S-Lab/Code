# A3S Code Configuration for Kimi Model

# Default model
default_model = "moonshot/moonshot-v1-8k"

# Moonshot (Kimi) Provider Configuration
providers {
  name    = "moonshot"
  api_key = env("MOONSHOT_API_KEY")
  base_url = "https://api.moonshot.cn/v1"

  models {
    id   = "moonshot-v1-8k"
    name = "Kimi (Moonshot v1 8k)"
  }

  models {
    id   = "moonshot-v1-32k"
    name = "Kimi (Moonshot v1 32k)"
  }

  models {
    id   = "moonshot-v1-128k"
    name = "Kimi (Moonshot v1 128k)"
  }
}

# Storage backend
storage_backend = "memory"

# Maximum tool execution rounds per turn
max_tool_rounds = 20
