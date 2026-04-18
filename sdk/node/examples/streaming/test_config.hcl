default_model = "openai/kimi-k2.5"

providers = [
  {
    name = "openai"
    api_key = "sk-ZaH1YnkiGmcBt8qxKWfsBV5w9aInp4QuDUeq1HEIOAzEg5cT"
    base_url = "http://35.220.164.252:3888/v1/"
    models = [
      {
        id = "kimi-k2.5"
        name = "Kimi"
        family = ""
        apiKey = null
        baseUrl = null
        attachment = false
        reasoning = false
        toolCall = true
        temperature = true
        releaseDate = null
        modalities = {
          input = ["text"]
          output = ["text"]
        }
        cost = {
          input = 0
          output = 0
          cacheRead = 0
          cacheWrite = 0
        }
        limit = {
          context = 128000
          output = 4096
        }
      }
    ]
  }
]

storage_backend = "file"
sessions_dir = "/tmp/a3s-sessions"
skill_dirs = []
agent_dirs = []
max_tool_rounds = 25
