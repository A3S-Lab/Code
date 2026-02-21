//! # News Radar — Multi-Channel News Aggregation & Analysis Agent
//!
//! Uses A3S Code + WebMCP to:
//! 1. Fetch news from multiple channels (web search, RSS via MCP, site scraping)
//! 2. Deduplicate raw content
//! 3. LLM-powered extraction: structured news items with impact assessment
//! 4. Stream a daily briefing report
//!
//! ```bash
//! cargo run --example news_radar
//! cargo run --example news_radar -- --topics ai,dev
//! ```

use a3s_code_core::{Agent, AgentEvent, SessionOptions, SessionQueueConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

fn find_config() -> PathBuf {
    if let Ok(p) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().expect("no home dir");
    let candidates = [
        home.join(".a3s/config.hcl"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.a3s/config.hcl"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .expect("Config not found. Set A3S_CONFIG or create ~/.a3s/config.hcl")
}

// ─── Channel Definitions ────────────────────────────────────────────

struct NewsChannel {
    name: &'static str,
    search_queries: Vec<&'static str>,
    rss_feeds: Vec<&'static str>,
    sites: Vec<&'static str>,
}

fn default_channels() -> Vec<NewsChannel> {
    vec![
        NewsChannel {
            name: "tech",
            search_queries: vec![
                "latest technology news today",
                "AI artificial intelligence breakthroughs",
                "open source software releases",
            ],
            rss_feeds: vec![
                "https://news.ycombinator.com/rss",
                "https://www.reddit.com/r/technology/.rss",
            ],
            sites: vec!["https://news.ycombinator.com"],
        },
        NewsChannel {
            name: "ai",
            search_queries: vec![
                "AI research papers this week",
                "large language model news today",
            ],
            rss_feeds: vec!["https://arxiv.org/rss/cs.AI"],
            sites: vec!["https://huggingface.co/blog"],
        },
        NewsChannel {
            name: "dev",
            search_queries: vec![
                "software development news today",
                "Rust programming language updates",
            ],
            rss_feeds: vec!["https://dev.to/feed"],
            sites: vec!["https://github.com/trending"],
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawItem {
    channel: String,
    topic: String,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NewsItem {
    title: String,
    summary: String,
    source: String,
    topic: String,
    impact: String,
    entities: Vec<String>,
}

fn content_hash(text: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let trimmed = text.trim().to_lowercase();
    let prefix = &trimmed[..trimmed.len().min(500)];
    let mut hasher = DefaultHasher::new();
    prefix.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

// ─── Phase 1: Multi-Channel Fetch ───────────────────────────────────

async fn fetch_via_search(
    session: &a3s_code_core::AgentSession,
    queries: &[&str],
    topic: &str,
) -> Vec<RawItem> {
    let mut results = Vec::new();

    for query in queries {
        let q = query.to_string();
        let t = topic.to_string();
        println!("  🔍 search: {q}");
        let r = session
            .tool(
                "web_search",
                serde_json::json!({"query": q, "limit": 10, "timeout": 20, "format": "text"}),
            )
            .await;
        match r {
            Ok(out) if out.exit_code == 0 && !out.output.is_empty() => {
                results.push(RawItem {
                    channel: "search".into(),
                    topic: t,
                    content: out.output,
                });
            }
            Ok(_) => {}
            Err(e) => eprintln!("  ⚠ search failed: {q} — {e}"),
        }
    }
    results
}

async fn fetch_via_mcp(
    session: &a3s_code_core::AgentSession,
    urls: &[&str],
    channel_type: &str,
    topic: &str,
) -> Vec<RawItem> {
    let mut results = Vec::new();
    let icon = if channel_type == "rss" {
        "📡"
    } else {
        "🌐"
    };

    for url in urls {
        println!("  {icon} {channel_type}: {url}");
        let r = session
            .tool(
                "mcp__fetch__fetch",
                serde_json::json!({"url": url, "max_length": 50000, "raw": false}),
            )
            .await;
        match r {
            Ok(out) if out.exit_code == 0 && !out.output.is_empty() => {
                results.push(RawItem {
                    channel: channel_type.into(),
                    topic: topic.into(),
                    content: out.output,
                });
            }
            Ok(_) => {}
            Err(e) => eprintln!("  ⚠ {channel_type} failed: {url} — {e}"),
        }
    }
    results
}

async fn fetch_all_channels(
    session: &a3s_code_core::AgentSession,
    channels: &[NewsChannel],
) -> Vec<RawItem> {
    let mut all = Vec::new();

    for ch in channels {
        println!("\n📰 Channel: {}", ch.name);
        let search = fetch_via_search(session, &ch.search_queries, ch.name).await;
        let rss = fetch_via_mcp(session, &ch.rss_feeds, "rss", ch.name).await;
        let scrape = fetch_via_mcp(session, &ch.sites, "scrape", ch.name).await;
        all.extend(search);
        all.extend(rss);
        all.extend(scrape);
    }

    all
}

// ─── Phase 2: Deduplicate ───────────────────────────────────────────

fn deduplicate(items: Vec<RawItem>) -> Vec<RawItem> {
    let before = items.len();
    let mut seen = HashSet::new();
    let unique: Vec<_> = items
        .into_iter()
        .filter(|item| seen.insert(content_hash(&item.content)))
        .collect();
    println!("\n🔄 Dedup: {before} → {} unique items", unique.len());
    unique
}

// ─── Phase 3: LLM Extraction ───────────────────────────────────────

async fn extract_news_items(
    session: &a3s_code_core::AgentSession,
    raw_items: &[RawItem],
) -> Vec<NewsItem> {
    println!("\n🧠 Extracting news items via LLM...");
    let mut all_items = Vec::new();
    let chunk_size = 5;

    for (batch_idx, chunk) in raw_items.chunks(chunk_size).enumerate() {
        let combined: String = chunk
            .iter()
            .map(|r| {
                let truncated = &r.content[..r.content.len().min(3000)];
                format!("[{}:{}] {}", r.channel, r.topic, truncated)
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        let prompt = format!(
            "You are a news analyst. From the raw content below, extract structured news items.\n\n\
             For each distinct news story, output a JSON object with:\n\
             - \"title\": concise headline (< 80 chars)\n\
             - \"summary\": 2-3 sentence summary\n\
             - \"source\": original source name or URL\n\
             - \"topic\": the topic category\n\
             - \"impact\": \"high\" | \"medium\" | \"low\"\n\
             - \"entities\": list of key people, companies, or technologies mentioned\n\n\
             Return a JSON array of objects only. Skip ads and boilerplate.\n\n\
             Raw content:\n{combined}"
        );

        let result = session.send(&prompt, None).await;
        match result {
            Ok(r) => {
                let text = r.text.trim().to_string();
                // Strip markdown code fences
                let json_text = if text.starts_with("```") {
                    text.lines()
                        .skip(1)
                        .take_while(|l| !l.starts_with("```"))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    text
                };
                match serde_json::from_str::<Vec<NewsItem>>(&json_text) {
                    Ok(items) => {
                        println!(
                            "  ✓ Extracted {} items from batch {}",
                            items.len(),
                            batch_idx + 1
                        );
                        all_items.extend(items);
                    }
                    Err(e) => {
                        eprintln!("  ⚠ Parse error in batch {}: {e}", batch_idx + 1);
                    }
                }
            }
            Err(e) => eprintln!("  ⚠ LLM error in batch {}: {e}", batch_idx + 1),
        }
    }

    println!("  📊 Total extracted: {} news items", all_items.len());
    all_items
}

// ─── Phase 4: Generate Report (Streaming) ───────────────────────────

async fn generate_report(
    session: &a3s_code_core::AgentSession,
    items: &[NewsItem],
    output_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
    let items_json = serde_json::to_string_pretty(items)?;
    let source_count = items
        .iter()
        .map(|i| i.source.as_str())
        .collect::<HashSet<_>>()
        .len();

    let prompt = format!(
        "You are a senior news analyst producing a daily intelligence briefing.\n\n\
         Date: {date}\n\
         News items (JSON): {items_json}\n\n\
         Generate a structured markdown report with:\n\n\
         # 📡 News Radar — Daily Briefing ({date})\n\n\
         ## 🔥 Top Stories\n(3-5 highest impact stories with detailed analysis)\n\n\
         ## 📊 By Topic\n### Tech\n### AI & ML\n### Development\n\
         (Group stories by topic, 2-3 sentences each)\n\n\
         ## 🏢 Key Entities\n(Table of most mentioned companies/people/technologies)\n\n\
         ## 📈 Trend Analysis\n(What patterns emerge? What's gaining momentum?)\n\n\
         ## ⚡ Action Items\n(What should a tech professional pay attention to?)\n\n\
         ---\n*Generated by News Radar Agent at {timestamp}*\n\
         *Sources: {source_count} channels, {} stories analyzed*\n\n\
         Write in clear, professional language. Prioritize signal over noise.",
        items.len()
    );

    println!("\n📝 Generating report...\n");
    println!("{}", "=".repeat(60));

    let (mut rx, _handle) = session.stream(&prompt, None).await?;
    let mut report_text = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta { text } => {
                print!("{text}");
                report_text.push_str(&text);
            }
            AgentEvent::ToolStart { name, .. } => {
                println!("\n  🔧 {name}...");
            }
            AgentEvent::End { usage, .. } => {
                println!("\n{}", "=".repeat(60));
                println!("✓ Report generated ({} tokens)", usage.total_tokens);
                break;
            }
            AgentEvent::Error { message } => {
                eprintln!("\n❌ Error: {message}");
                break;
            }
            _ => {}
        }
    }

    // Save report
    std::fs::create_dir_all(output_dir)?;
    let report_path = output_dir.join(format!("news-radar-{date}.md"));
    std::fs::write(&report_path, &report_text)?;
    println!("\n💾 Saved to {}", report_path.display());

    Ok(())
}

// ─── Main ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let topics: Option<Vec<&str>> = args
        .iter()
        .position(|a| a == "--topics")
        .and_then(|i| args.get(i + 1))
        .map(|t| t.split(',').collect());

    let channels = default_channels();
    let channels: Vec<_> = match &topics {
        Some(ts) => channels
            .into_iter()
            .filter(|c| ts.contains(&c.name))
            .collect(),
        None => channels,
    };

    let config = find_config();
    let agent = Agent::new(config.to_str().unwrap()).await?;

    let queue_config = SessionQueueConfig::default()
        .with_lane_features()
        .with_timeout(60_000);

    let opts = SessionOptions::new()
        .with_queue_config(queue_config)
        .with_auto_compact(true)
        .with_auto_compact_threshold(0.7)
        .with_parse_retries(3)
        .with_tool_timeout(30_000)
        .with_circuit_breaker(5)
        .with_permissive_policy();

    let session = agent.session(".", Some(opts))?;

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "📡 News Radar — {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
    );
    println!(
        "   Topics: {}",
        channels
            .iter()
            .map(|c| c.name)
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Phase 1: Fetch
    println!("\n── Phase 1: Multi-Channel Fetch ──");
    let raw_items = fetch_all_channels(&session, &channels).await;
    println!("\n✓ Fetched {} raw items", raw_items.len());

    if raw_items.is_empty() {
        println!("⚠ No items fetched. Check network and config.");
        return Ok(());
    }

    // Phase 2: Deduplicate
    println!("\n── Phase 2: Deduplicate ──");
    let unique_items = deduplicate(raw_items);

    // Phase 3: LLM extraction
    println!("\n── Phase 3: LLM Analysis ──");
    let news_items = extract_news_items(&session, &unique_items).await;

    if news_items.is_empty() {
        println!("⚠ No news items extracted.");
        return Ok(());
    }

    // Phase 4: Generate report
    println!("\n── Phase 4: Generate Report ──");
    let output_dir = std::path::PathBuf::from("./reports");
    generate_report(&session, &news_items, &output_dir).await?;

    // Queue stats
    let stats = session.queue_stats().await;
    println!(
        "\n📊 Queue stats: pending={}, active={}",
        stats.total_pending, stats.total_active
    );

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✓ News Radar cycle complete");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(())
}
