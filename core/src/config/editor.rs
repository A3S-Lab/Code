use super::acl_render::{render_labeled_section, render_single_section};
use super::CodeConfig;
use crate::error::{CodeError, Result};

/// Logical portions of `config.acl` that can be rewritten independently.
///
/// The editor replaces only the requested top-level entries. Unrelated blocks,
/// unknown configuration, comments outside a replaced block, and raw
/// expressions such as `env("TOKEN")` remain intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigSection {
    DefaultModel,
    Providers,
    ModelRuntime,
    Execution,
    Storage,
    Memory,
    Queue,
    Search,
    Os,
    DocumentParser,
    McpServers,
}

/// Rewrite selected sections of an ACL document from a typed configuration.
pub fn rewrite_acl_sections(
    source: &str,
    config: &CodeConfig,
    sections: &[ConfigSection],
) -> Result<String> {
    let document = a3s_acl::parse_acl(source)
        .map_err(|error| CodeError::Config(format!("Failed to parse ACL: {error}")))?;
    let original = CodeConfig::from_acl(source)?;
    let mut output = source.to_string();

    for section in deduplicated_sections(sections) {
        match section {
            ConfigSection::Providers | ConfigSection::McpServers => {
                let rendered = render_labeled_section(section, config, &original, &document);
                let names = section_names(section);
                output = replace_labeled_entries(&output, names, rendered);
            }
            _ => {
                for entry in render_single_section(section, config, &original, &document) {
                    output = replace_single_entry(&output, entry.names, entry.text);
                }
            }
        }
    }

    CodeConfig::from_acl(&output).map_err(|error| {
        CodeError::Config(format!("Generated ACL did not pass validation: {error}"))
    })?;
    Ok(normalize_final_newline(output))
}

fn deduplicated_sections(sections: &[ConfigSection]) -> Vec<ConfigSection> {
    let mut result = Vec::new();
    for section in sections {
        if !result.contains(section) {
            result.push(*section);
        }
    }
    result
}

fn section_names(section: ConfigSection) -> &'static [&'static str] {
    match section {
        ConfigSection::Providers => &["providers"],
        ConfigSection::McpServers => &["mcp_servers", "mcpServers", "mcp_server"],
        _ => &[],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TopLevelEntry {
    name: String,
    label: Option<String>,
    start: usize,
    end: usize,
}

fn replace_single_entry(source: &str, names: &[&str], rendered: Option<String>) -> String {
    let entries = scan_top_level_entries(source);
    let matching = entries
        .iter()
        .filter(|entry| names.contains(&entry.name.as_str()))
        .collect::<Vec<_>>();

    if matching.is_empty() {
        return rendered
            .map(|value| append_entry(source, &value))
            .unwrap_or_else(|| source.to_string());
    }

    let mut edits = Vec::new();
    for (index, entry) in matching.into_iter().enumerate() {
        let replacement = if index == 0 {
            rendered.clone().unwrap_or_default()
        } else {
            String::new()
        };
        edits.push((entry.start, entry.end, replacement));
    }
    apply_edits(source, edits)
}

fn replace_labeled_entries(
    source: &str,
    names: &[&str],
    rendered: Vec<(String, String)>,
) -> String {
    let entries = scan_top_level_entries(source);
    let matching = entries
        .iter()
        .filter(|entry| names.contains(&entry.name.as_str()))
        .collect::<Vec<_>>();
    let mut remaining = rendered;
    let mut edits = Vec::new();

    for entry in matching {
        let replacement = entry.label.as_ref().and_then(|label| {
            remaining
                .iter()
                .position(|(candidate, _)| candidate == label)
                .map(|index| remaining.remove(index).1)
        });
        edits.push((entry.start, entry.end, replacement.unwrap_or_default()));
    }

    let mut output = apply_edits(source, edits);
    for (_, entry) in remaining {
        output = append_entry(&output, &entry);
    }
    output
}

fn append_entry(source: &str, entry: &str) -> String {
    let mut output = source.trim_end().to_string();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(entry.trim());
    output.push('\n');
    output
}

fn apply_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut output = source.to_string();
    for (start, end, replacement) in edits {
        let replacement = if replacement.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n", replacement.trim_end())
        };
        output.replace_range(start..end, &replacement);
    }
    output
}

fn normalize_final_newline(mut source: String) -> String {
    while source.ends_with("\n\n\n") {
        source.pop();
    }
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source
}

fn scan_top_level_entries(source: &str) -> Vec<TopLevelEntry> {
    let bytes = source.as_bytes();
    let mut entries = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        index = skip_space_and_comments(bytes, index);
        if index >= bytes.len() {
            break;
        }
        if !is_ident_start(bytes[index]) {
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < bytes.len() && is_ident_part(bytes[index]) {
            index += 1;
        }
        let name = source[start..index].to_string();
        let header_start = index;
        let mut cursor = index;
        let mut braces = 0usize;
        let mut brackets = 0usize;
        let mut parens = 0usize;
        let mut quote = None;
        let mut escaped = false;
        let mut saw_delimiter = false;

        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if let Some(active_quote) = quote {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == active_quote {
                    quote = None;
                }
                cursor += 1;
                continue;
            }
            match byte {
                b'"' | b'\'' => {
                    quote = Some(byte);
                    cursor += 1;
                }
                b'#' => cursor = skip_line(bytes, cursor),
                b'/' if bytes.get(cursor + 1) == Some(&b'/') => cursor = skip_line(bytes, cursor),
                b'{' => {
                    saw_delimiter = true;
                    braces += 1;
                    cursor += 1;
                }
                b'}' => {
                    braces = braces.saturating_sub(1);
                    cursor += 1;
                }
                b'[' => {
                    saw_delimiter = true;
                    brackets += 1;
                    cursor += 1;
                }
                b']' => {
                    brackets = brackets.saturating_sub(1);
                    cursor += 1;
                }
                b'(' => {
                    saw_delimiter = true;
                    parens += 1;
                    cursor += 1;
                }
                b')' => {
                    parens = parens.saturating_sub(1);
                    cursor += 1;
                }
                b'=' | b':' => {
                    saw_delimiter = true;
                    cursor += 1;
                }
                b'\n' if saw_delimiter && braces == 0 && brackets == 0 && parens == 0 => {
                    cursor += 1;
                    break;
                }
                _ => cursor += 1,
            }
        }

        let header_end = source[header_start..cursor]
            .find('{')
            .or_else(|| source[header_start..cursor].find('='))
            .map(|offset| header_start + offset)
            .unwrap_or(cursor);
        entries.push(TopLevelEntry {
            name,
            label: parse_first_quoted(&source[header_start..header_end]),
            start,
            end: cursor,
        });
        index = cursor.max(start + 1);
    }

    entries
}

fn skip_space_and_comments(bytes: &[u8], mut index: usize) -> usize {
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'#' {
            index = skip_line(bytes, index);
            continue;
        }
        if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'/') {
            index = skip_line(bytes, index);
            continue;
        }
        return index;
    }
}

fn skip_line(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn parse_first_quoted(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let start = bytes
        .iter()
        .position(|byte| *byte == b'"' || *byte == b'\'')?;
    let quote = bytes[start];
    let mut result = String::new();
    let mut escaped = false;
    for byte in &bytes[start + 1..] {
        if escaped {
            result.push(*byte as char);
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == quote {
            return Some(result);
        } else {
            result.push(*byte as char);
        }
    }
    None
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::{rewrite_acl_sections, scan_top_level_entries, ConfigSection};
    use crate::config::CodeConfig;

    const COMPLETE_ACL: &str = r#"# keep this document comment
default_model = "openai/gpt-test"
thinking_budget = 12000
llm_api_timeout_ms = 45000
storage_backend = "custom"
sessions_dir = "./sessions"
memory_dir = "./memory"
storage_url = env("A3S_EDITOR_STORAGE_URL")
skill_dirs = ["./skills"]
agent_dirs = ["./agents"]
max_tool_rounds = 42
max_parallel_tasks = 6
auto_parallel = false
os = "https://os.example.test"

auto_delegation {
  enabled = true
  auto_parallel = false
  allow_manual_delegation = false
  min_confidence = 0.81
  max_tasks = 7
}

providers "openai" {
  api_key = env("A3S_EDITOR_PROVIDER_KEY")
  base_url = "https://llm.example.test/v1"
  headers = { Authorization = env("A3S_EDITOR_PROVIDER_TOKEN"), X_Tenant = "a3s" }

  models "gpt-test" {
    name = "GPT Test"
    family = "gpt"
    api_key = env("A3S_EDITOR_MODEL_KEY")
    attachment = true
    reasoning = true
    tool_call = true
    temperature = false
    modalities = { input = ["text", "image"], output = ["text"] }
    cost = { input = 1.25, output = 4.5, cache_read = 0.2, cache_write = 0.4 }
    limit = { context = 128000, output = 8192 }
  }
}

memory {
  max_short_term = 120
  max_working = 16
  prune_interval_secs = 1800
  llm_extraction = true
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
    launch_args = ["--disable-gpu"]
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
  ocr {
    enabled = true
    model = "openai/gpt-test"
    max_images = 12
    dpi = 192
    provider = "vision"
    api_key = env("A3S_EDITOR_OCR_KEY")
  }
}

mcp_servers "remote" {
  transport = "streamable-http"
  url = "https://mcp.example.test"
  headers = { Authorization = env("A3S_EDITOR_MCP_HEADER") }
  enabled = true
  env = { MCP_TOKEN = env("A3S_EDITOR_MCP_TOKEN") }
  tool_timeout_secs = 75
  oauth {
    auth_url = "https://auth.example.test"
    token_url = "https://token.example.test"
    client_id = "client"
    client_secret = env("A3S_EDITOR_OAUTH_SECRET")
    scopes = ["tools.read"]
    redirect_uri = "http://127.0.0.1/callback"
    access_token = env("A3S_EDITOR_OAUTH_TOKEN")
  }
}

# unknown top-level configuration must survive every settings save
future_feature "keep-me" {
  enabled = true
  nested {
    value = "untouched"
  }
}
"#;

    #[test]
    fn scanner_keeps_comments_and_finds_labeled_blocks() {
        let source = r#"# before
default_model = "openai/a"

providers "openai" {
  api_key = env("OPENAI_API_KEY")
  models "a" { name = "A" }
}

unknown { nested { enabled = true } }
"#;
        let entries = scan_top_level_entries(source);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "default_model");
        assert_eq!(entries[1].label.as_deref(), Some("openai"));
        assert_eq!(entries[2].name, "unknown");
        assert!(source[..entries[0].start].contains("# before"));
    }

    #[test]
    fn every_config_section_round_trips_without_losing_unmanaged_content_or_env_calls() {
        let mut config = CodeConfig::from_acl(COMPLETE_ACL).expect("complete config");
        config.default_model = Some("openai/gpt-test".to_string());
        config.thinking_budget = Some(16000);
        config.max_tool_rounds = Some(64);
        config.auto_delegation.max_tasks = 9;
        config.memory.as_mut().expect("memory").max_working = 24;
        config.queue.as_mut().expect("queue").query_max_concurrency = 11;
        config.search.as_mut().expect("search").timeout = 22;
        config
            .document_parser
            .as_mut()
            .expect("document parser")
            .max_file_size_mb = 96;

        let output = rewrite_acl_sections(
            COMPLETE_ACL,
            &config,
            &[
                ConfigSection::DefaultModel,
                ConfigSection::Providers,
                ConfigSection::ModelRuntime,
                ConfigSection::Execution,
                ConfigSection::Storage,
                ConfigSection::Memory,
                ConfigSection::Queue,
                ConfigSection::Search,
                ConfigSection::Os,
                ConfigSection::DocumentParser,
                ConfigSection::McpServers,
            ],
        )
        .expect("rewrite every section");

        assert!(output.contains("# keep this document comment"));
        assert!(
            output.contains("# unknown top-level configuration must survive every settings save")
        );
        assert!(output.contains("future_feature \"keep-me\""));
        assert!(output.contains("value = \"untouched\""));
        for expression in [
            "env(\"A3S_EDITOR_STORAGE_URL\")",
            "env(\"A3S_EDITOR_PROVIDER_KEY\")",
            "env(\"A3S_EDITOR_PROVIDER_TOKEN\")",
            "env(\"A3S_EDITOR_MODEL_KEY\")",
            "env(\"A3S_EDITOR_OCR_KEY\")",
            "env(\"A3S_EDITOR_MCP_HEADER\")",
            "env(\"A3S_EDITOR_MCP_TOKEN\")",
            "env(\"A3S_EDITOR_OAUTH_SECRET\")",
            "env(\"A3S_EDITOR_OAUTH_TOKEN\")",
        ] {
            assert!(
                output.contains(expression),
                "missing {expression}:\n{output}"
            );
        }

        let round_trip = CodeConfig::from_acl(&output).expect("rewritten ACL parses");
        assert_eq!(round_trip.thinking_budget, Some(16000));
        assert_eq!(round_trip.max_tool_rounds, Some(64));
        assert_eq!(round_trip.auto_delegation.max_tasks, 9);
        assert_eq!(round_trip.memory.expect("memory").max_working, 24);
        assert_eq!(round_trip.queue.expect("queue").query_max_concurrency, 11);
        assert_eq!(round_trip.search.expect("search").timeout, 22);
        assert_eq!(
            round_trip
                .document_parser
                .expect("document parser")
                .max_file_size_mb,
            96
        );
    }

    #[test]
    fn labeled_sections_remove_missing_entries_and_append_new_entries() {
        let mut config = CodeConfig::from_acl(COMPLETE_ACL).expect("complete config");
        let mut provider = config.providers[0].clone();
        provider.name = "replacement".to_string();
        provider.models[0].id = "replacement-model".to_string();
        provider.models[0].name = "Replacement Model".to_string();
        provider.api_key = Some("replacement-key".to_string());
        config.providers = vec![provider];

        let mut server = config.mcp_servers[0].clone();
        server.name = "replacement-mcp".to_string();
        server.env.insert("VISIBLE".to_string(), "yes".to_string());
        config.mcp_servers = vec![server];

        let output = rewrite_acl_sections(
            COMPLETE_ACL,
            &config,
            &[ConfigSection::Providers, ConfigSection::McpServers],
        )
        .expect("rewrite labeled sections");

        assert!(!output.contains("providers \"openai\""));
        assert!(!output.contains("mcp_servers \"remote\""));
        assert!(output.contains("providers \"replacement\""));
        assert!(output.contains("models \"replacement-model\""));
        assert!(output.contains("mcp_servers \"replacement-mcp\""));

        let round_trip = CodeConfig::from_acl(&output).expect("rewritten ACL parses");
        assert_eq!(round_trip.providers.len(), 1);
        assert_eq!(round_trip.providers[0].name, "replacement");
        assert_eq!(round_trip.mcp_servers.len(), 1);
        assert_eq!(round_trip.mcp_servers[0].name, "replacement-mcp");
    }
}
