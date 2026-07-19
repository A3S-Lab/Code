use super::{BrowserBackend, CodeConfig, StorageBackend};
use crate::mcp::McpTransportConfig;
use crate::queue::{SessionLane, TaskHandlerMode};

#[test]
fn acl_loader_covers_every_code_config_section() {
    let config = CodeConfig::from_acl(
        r#"
default_model = "openai/gpt-test"
storage_backend = "custom"
sessions_dir = "./sessions"
memory_dir = "./memory"
storage_url = "sqlite://state.db"
skill_dirs = ["./skills", "./shared-skills"]
agent_dirs = ["./agents"]
max_tool_rounds = 42
max_parallel_tasks = 6
auto_parallel = false
thinking_budget = 12000
llm_api_timeout_ms = 45000
os = "https://os.example.test"

auto_delegation {
  enabled = true
  auto_parallel = false
  allow_manual_delegation = false
  min_confidence = 0.81
  max_tasks = 7
}

providers "openai" {
  api_key = "provider-secret"
  base_url = "https://llm.example.test/v1"
  session_id_header = "x-session-id"
  headers = { Authorization = "Bearer provider-secret", X_Tenant = "a3s" }

  models "gpt-test" {
    name = "GPT Test"
    family = "gpt"
    api_key = "model-secret"
    base_url = "https://model.example.test/v1"
    session_id_header = "x-model-session"
    headers = { X_Model = "test" }
    attachment = true
    reasoning = true
    tool_call = false
    temperature = false
    release_date = "2026-07-01"
    modalities = { input = ["text", "image"], output = ["text"] }
    cost = { input = 1.25, output = 4.5, cache_read = 0.2, cache_write = 0.4 }
    limit = { context = 128000, output = 8192 }
  }
}

memory {
  max_short_term = 120
  max_working = 16
  prune_interval_secs = 1800
  llm_extraction = false
  llm_extraction_max_items = 9
  llm_extraction_max_input_chars = 12000
  relevance {
    decay_days = 45
    importance_weight = 0.8
    recency_weight = 0.2
  }
  prune_policy {
    max_age_days = 120
    min_importance_to_keep = 0.6
    max_items = 5000
  }
}

queue {
  control_max_concurrency = 3
  query_max_concurrency = 9
  execute_max_concurrency = 5
  generate_max_concurrency = 2
  enable_dlq = true
  dlq_max_size = 250
  enable_metrics = true
  enable_alerts = true
  default_timeout_ms = 65000
  storage_path = "./queue"
  pressure_threshold = 30
  lane_handlers {
    query {
      mode = "external"
      timeout_ms = 20000
    }
  }
  lane_timeouts = { query = 30000, execute = 120000 }
  retry_policy {
    strategy = "exponential"
    max_retries = 5
    initial_delay_ms = 250
  }
  rate_limit {
    limit_type = "per_minute"
    max_operations = 600
  }
  priority_boost {
    strategy = "aggressive"
    deadline_ms = 10000
  }
}

search {
  timeout = 18
  health {
    max_failures = 4
    suspend_seconds = 90
  }
  headless {
    backend = "lightpanda"
    max_tabs = 6
    browser_path = "/opt/lightpanda"
    launch_args = ["--disable-gpu"]
    proxy_url = "http://127.0.0.1:7890"
  }
  engine {
    duckduckgo {
      enabled = true
      weight = 1.3
      timeout = 12
    }
  }
}

document_parser {
  enabled = true
  max_file_size_mb = 80
  cache {
    enabled = true
    directory = "./document-cache"
  }
}

mcp_servers "filesystem" {
  transport = "stdio"
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-filesystem"]
  enabled = true
  env = { MCP_TOKEN = "mcp-secret" }
  tool_timeout_secs = 75
  oauth {
    auth_url = "https://auth.example.test"
    token_url = "https://token.example.test"
    client_id = "client"
    client_secret = "oauth-secret"
    scopes = ["tools.read"]
    redirect_uri = "http://127.0.0.1/callback"
    access_token = "access-secret"
  }
}
"#,
    )
    .expect("full ACL config should load");

    assert_eq!(config.default_model.as_deref(), Some("openai/gpt-test"));
    assert_eq!(config.storage_backend, StorageBackend::Custom);
    assert_eq!(config.sessions_dir.unwrap().to_string_lossy(), "./sessions");
    assert_eq!(config.memory_dir.unwrap().to_string_lossy(), "./memory");
    assert_eq!(config.storage_url.as_deref(), Some("sqlite://state.db"));
    assert_eq!(config.skill_dirs.len(), 2);
    assert_eq!(config.agent_dirs.len(), 1);
    assert_eq!(config.max_tool_rounds, Some(42));
    assert_eq!(config.max_parallel_tasks, Some(6));
    assert_eq!(config.auto_parallel, Some(false));
    assert_eq!(config.thinking_budget, Some(12000));
    assert_eq!(config.llm_api_timeout_ms, Some(45000));
    assert_eq!(
        config.os.as_ref().map(|value| value.address.as_str()),
        Some("https://os.example.test")
    );
    assert!(config.auto_delegation.enabled);
    assert!(!config.auto_delegation.auto_parallel);
    assert!(!config.auto_delegation.allow_manual_delegation);
    assert!((config.auto_delegation.min_confidence - 0.81).abs() < f32::EPSILON);
    assert_eq!(config.auto_delegation.max_tasks, 7);

    let provider = &config.providers[0];
    assert_eq!(
        provider.headers.get("Authorization").map(String::as_str),
        Some("Bearer provider-secret")
    );
    let model = &provider.models[0];
    assert_eq!(model.modalities.input, ["text", "image"]);
    assert_eq!(model.modalities.output, ["text"]);
    assert_eq!(model.limit.context, 128000);
    assert_eq!(model.limit.output, 8192);
    assert_eq!(model.cost.cache_read, 0.2);
    assert!(!model.tool_call);
    assert!(!model.temperature);

    let memory = config.memory.expect("memory config");
    assert_eq!(memory.max_short_term, 120);
    assert_eq!(memory.max_working, 16);
    assert_eq!(memory.relevance.decay_days, 45.0);
    assert_eq!(
        memory.prune_policy.as_ref().map(|policy| policy.max_items),
        Some(5000)
    );
    assert!(!memory.llm_extraction);

    let queue = config.queue.expect("queue config");
    assert_eq!(queue.query_max_concurrency, 9);
    assert!(queue.enable_dlq);
    assert_eq!(
        queue.lane_timeouts.get(&SessionLane::Execute),
        Some(&120000)
    );
    assert_eq!(
        queue
            .lane_handlers
            .get(&SessionLane::Query)
            .map(|handler| handler.mode),
        Some(TaskHandlerMode::External)
    );
    assert_eq!(
        queue.retry_policy.as_ref().map(|policy| policy.max_retries),
        Some(5)
    );

    let search = config.search.expect("search config");
    assert_eq!(search.timeout, 18);
    assert_eq!(
        search.headless.as_ref().map(|headless| headless.backend),
        Some(BrowserBackend::Lightpanda)
    );
    assert_eq!(
        search
            .engines
            .get("duckduckgo")
            .and_then(|engine| engine.timeout),
        Some(12)
    );

    let parser = config.document_parser.expect("document parser config");
    assert_eq!(parser.max_file_size_mb, 80);
    assert_eq!(
        parser
            .cache
            .as_ref()
            .and_then(|cache| cache.directory.as_ref())
            .map(|path| path.to_string_lossy()),
        Some("./document-cache".into())
    );

    let mcp = &config.mcp_servers[0];
    assert_eq!(mcp.name, "filesystem");
    assert_eq!(mcp.tool_timeout_secs, 75);
    assert_eq!(
        mcp.env.get("MCP_TOKEN").map(String::as_str),
        Some("mcp-secret")
    );
    assert!(
        matches!(mcp.transport, McpTransportConfig::Stdio { ref command, .. } if command == "npx")
    );
    assert_eq!(
        mcp.oauth
            .as_ref()
            .and_then(|oauth| oauth.client_secret.as_deref()),
        Some("oauth-secret")
    );
}
