# News Radar Agent — Multi-Channel News Aggregation & Analysis
# Uses WebMCP for web scraping + built-in web_search for search engines

default_model = "openai/kimi-k2.5"

providers {
  name     = "anthropic"
  api_key  = env("ANTHROPIC_API_KEY")
  base_url = env("ANTHROPIC_BASE_URL")

  models {
    id          = "claude-sonnet-4-20250514"
    name        = "Claude Sonnet 4"
    family      = "claude-sonnet"
    tool_call   = true
    temperature = true

    limit {
      context = 200000
      output  = 64000
    }
  }
}

providers {
  name = "openai"

  models {
    id          = "kimi-k2.5"
    name        = "KIMI K2.5"
    family      = "kimi"
    api_key     = env("OPENAI_API_KEY")
    base_url    = env("OPENAI_BASE_URL")
    tool_call   = true
    temperature = true

    limit {
      context = 128000
      output  = 4096
    }
  }
}

# Lane queue — parallel news fetching
queue {
  query_max_concurrency   = 10
  execute_max_concurrency = 4
  enable_metrics          = true
  enable_dlq              = true

  retry_policy {
    strategy         = "exponential"
    max_retries      = 3
    initial_delay_ms = 500
  }
}

# Built-in web search
search {
  timeout = 30

  engine {
    google     { enabled = true; weight = 1.5 }
    bing       { enabled = true; weight = 1.0 }
    duckduckgo { enabled = true; weight = 1.2 }
  }
}

# MCP Servers — Web scraping via Fetch and Puppeteer
mcp_servers {
  name      = "fetch"
  transport = "stdio"
  command   = "npx"
  args      = ["-y", "@modelcontextprotocol/server-fetch"]
  enabled   = true
}

mcp_servers {
  name      = "puppeteer"
  transport = "stdio"
  command   = "npx"
  args      = ["-y", "@anthropic/mcp-server-puppeteer"]
  enabled   = true
}
