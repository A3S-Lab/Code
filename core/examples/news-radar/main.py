#!/usr/bin/env python3
"""
News Radar — Multi-Channel News Aggregation & Analysis Agent

Uses A3S Code + WebMCP to:
1. Fetch news from multiple channels (RSS, web search, direct scraping)
2. Deduplicate and cluster related stories
3. LLM-powered analysis: summarize, extract key entities, assess impact
4. Generate a structured daily briefing report

Usage:
    # One-shot run
    python main.py

    # With custom topics
    python main.py --topics "AI,Rust,Cloud Native"

    # Scheduled mode (runs daily at 08:00)
    python main.py --schedule

    # Custom config path
    python main.py --config /path/to/agent.hcl
"""

import asyncio
import argparse
import json
import hashlib
import re
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path


# ─── News Channel Definitions ───────────────────────────────────────

DEFAULT_CHANNELS = {
    "tech": {
        "search_queries": [
            "latest technology news today",
            "AI artificial intelligence breakthroughs",
            "open source software releases",
        ],
        "rss_feeds": [
            "https://news.ycombinator.com/rss",
            "https://www.reddit.com/r/technology/.rss",
            "https://feeds.arstechnica.com/arstechnica/technology-lab",
        ],
        "sites": [
            "https://news.ycombinator.com",
            "https://www.theverge.com/tech",
        ],
    },
    "ai": {
        "search_queries": [
            "AI research papers this week",
            "large language model news today",
            "machine learning industry updates",
        ],
        "rss_feeds": [
            "https://arxiv.org/rss/cs.AI",
            "https://www.reddit.com/r/MachineLearning/.rss",
        ],
        "sites": [
            "https://huggingface.co/blog",
        ],
    },
    "dev": {
        "search_queries": [
            "software development news today",
            "Rust programming language updates",
            "cloud native kubernetes news",
        ],
        "rss_feeds": [
            "https://dev.to/feed",
            "https://www.reddit.com/r/programming/.rss",
        ],
        "sites": [
            "https://github.com/trending",
        ],
    },
}


# ─── Utility Functions ───────────────────────────────────────────────

def find_config():
    """Locate agent.hcl config file."""
    candidates = [
        Path(__file__).parent / "agent.hcl",
        Path.home() / ".a3s" / "config.hcl",
    ]
    env_path = __import__("os").environ.get("A3S_CONFIG")
    if env_path:
        candidates.insert(0, Path(env_path))
    for p in candidates:
        if p.exists():
            return str(p)
    raise FileNotFoundError(
        "Config not found. Set A3S_CONFIG or create ~/.a3s/config.hcl"
    )


def content_hash(text: str) -> str:
    """Generate a short hash for deduplication."""
    return hashlib.sha256(text.strip().lower().encode()).hexdigest()[:12]


def now_str() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


def today_str() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%d")


# ─── Phase 1: Multi-Channel Fetch ───────────────────────────────────

async def fetch_via_search(session, queries: list[str]) -> list[dict]:
    """Use built-in web_search tool for search engine results."""
    results = []

    async def search_one(query):
        print(f"  🔍 search: {query}")
        try:
            r = await asyncio.to_thread(
                session.tool,
                "web_search",
                {"query": query, "limit": 10, "timeout": 20, "format": "text"},
            )
            if r.exit_code == 0 and r.output:
                return {"channel": "search", "query": query, "content": r.output}
        except Exception as e:
            print(f"  ⚠ search failed: {query} — {e}")
        return None

    raw = await asyncio.gather(
        *[search_one(q) for q in queries], return_exceptions=True
    )
    for r in raw:
        if isinstance(r, dict):
            results.append(r)
    return results


async def fetch_via_mcp_rss(session, feeds: list[str]) -> list[dict]:
    """Use MCP fetch server to grab RSS feeds."""
    results = []

    async def fetch_one(url):
        print(f"  📡 rss: {url}")
        try:
            r = await asyncio.to_thread(
                session.tool,
                "mcp__fetch__fetch",
                {"url": url, "max_length": 50000, "raw": False},
            )
            if r.exit_code == 0 and r.output:
                return {"channel": "rss", "url": url, "content": r.output}
        except Exception as e:
            print(f"  ⚠ rss failed: {url} — {e}")
        return None

    raw = await asyncio.gather(
        *[fetch_one(u) for u in feeds], return_exceptions=True
    )
    for r in raw:
        if isinstance(r, dict):
            results.append(r)
    return results


async def fetch_via_mcp_scrape(session, sites: list[str]) -> list[dict]:
    """Use MCP fetch server to scrape web pages."""
    results = []

    async def scrape_one(url):
        print(f"  🌐 scrape: {url}")
        try:
            r = await asyncio.to_thread(
                session.tool,
                "mcp__fetch__fetch",
                {"url": url, "max_length": 80000, "raw": False},
            )
            if r.exit_code == 0 and r.output:
                return {"channel": "scrape", "url": url, "content": r.output}
        except Exception as e:
            print(f"  ⚠ scrape failed: {url} — {e}")
        return None

    raw = await asyncio.gather(
        *[scrape_one(u) for u in sites], return_exceptions=True
    )
    for r in raw:
        if isinstance(r, dict):
            results.append(r)
    return results


async def fetch_all_channels(session, channels: dict) -> list[dict]:
    """Fetch from all channels in parallel."""
    all_results = []

    for name, cfg in channels.items():
        print(f"\n📰 Channel: {name}")
        search_results, rss_results, scrape_results = await asyncio.gather(
            fetch_via_search(session, cfg.get("search_queries", [])),
            fetch_via_mcp_rss(session, cfg.get("rss_feeds", [])),
            fetch_via_mcp_scrape(session, cfg.get("sites", [])),
        )
        for r in search_results + rss_results + scrape_results:
            r["topic"] = name
        all_results.extend(search_results + rss_results + scrape_results)

    return all_results


# ─── Phase 2: Deduplicate & Cluster ─────────────────────────────────

def deduplicate(raw_items: list[dict]) -> list[dict]:
    """Remove near-duplicate content by hash."""
    seen = set()
    unique = []
    for item in raw_items:
        h = content_hash(item.get("content", "")[:500])
        if h not in seen:
            seen.add(h)
            unique.append(item)
    print(f"\n🔄 Dedup: {len(raw_items)} → {len(unique)} unique items")
    return unique


# ─── Phase 3: LLM Analysis ──────────────────────────────────────────

EXTRACT_PROMPT = """\
You are a news analyst. From the raw content below, extract structured news items.

For each distinct news story, output a JSON object with:
- "title": concise headline (< 80 chars)
- "summary": 2-3 sentence summary
- "source": original source name or URL
- "topic": the topic category
- "impact": "high" | "medium" | "low"
- "entities": list of key people, companies, or technologies mentioned

Return a JSON array of objects. Only include genuinely newsworthy items.
Skip ads, navigation text, and boilerplate.

Raw content:
{content}
"""

async def extract_news_items(session, raw_items: list[dict]) -> list[dict]:
    """Use LLM to extract structured news from raw content."""
    print("\n🧠 Extracting news items via LLM...")

    # Batch raw items into chunks to avoid context overflow
    chunk_size = 5
    all_items = []

    for i in range(0, len(raw_items), chunk_size):
        chunk = raw_items[i : i + chunk_size]
        combined = "\n\n---\n\n".join(
            f"[{r['channel']}:{r['topic']}] {r['content'][:3000]}" for r in chunk
        )

        result = await asyncio.to_thread(
            session.send,
            EXTRACT_PROMPT.format(content=combined),
        )

        try:
            # Try to parse JSON from the response
            text = result.text.strip()
            # Handle markdown code blocks
            if text.startswith("```"):
                text = re.sub(r"^```\w*\n?", "", text)
                text = re.sub(r"\n?```$", "", text)
            items = json.loads(text)
            if isinstance(items, list):
                all_items.extend(items)
                print(f"  ✓ Extracted {len(items)} items from batch {i // chunk_size + 1}")
        except (json.JSONDecodeError, ValueError) as e:
            print(f"  ⚠ Parse error in batch {i // chunk_size + 1}: {e}")

    print(f"  📊 Total extracted: {len(all_items)} news items")
    return all_items


# ─── Phase 4: Generate Report ────────────────────────────────────────

REPORT_PROMPT = """\
You are a senior news analyst producing a daily intelligence briefing.

Date: {date}
News items (JSON): {items}

Generate a structured markdown report with:

# 📡 News Radar — Daily Briefing ({date})

## 🔥 Top Stories
(3-5 highest impact stories with detailed analysis)

## 📊 By Topic
### Tech
### AI & ML
### Development
(Group stories by topic, 2-3 sentences each)

## 🏢 Key Entities
(Table of most mentioned companies/people/technologies with context)

## 📈 Trend Analysis
(What patterns emerge? What's gaining momentum?)

## ⚡ Action Items
(What should a tech professional pay attention to?)

---
*Generated by News Radar Agent at {timestamp}*
*Sources: {source_count} channels, {item_count} stories analyzed*

Write in clear, professional language. Use inline links where available.
Prioritize signal over noise.
"""

async def generate_report(session, items: list[dict], output_dir: Path):
    """Stream the final report via LLM."""
    date = today_str()
    timestamp = now_str()

    prompt = REPORT_PROMPT.format(
        date=date,
        items=json.dumps(items, ensure_ascii=False, indent=2)[:60000],
        timestamp=timestamp,
        source_count=len(set(i.get("source", "unknown") for i in items)),
        item_count=len(items),
    )

    print(f"\n📝 Generating report...\n")
    print("=" * 60)

    report_text = []

    for event in session.stream(prompt):
        t = event.event_type if hasattr(event, "event_type") else event.get("type", "")
        if t == "text_delta":
            text = event.text if hasattr(event, "text") else event.get("text", "")
            print(text, end="", flush=True)
            report_text.append(text)
        elif t == "tool_start":
            name = event.tool_name if hasattr(event, "tool_name") else event.get("name", "")
            print(f"\n  🔧 {name}...", flush=True)
        elif t == "end":
            tokens = (
                event.total_tokens
                if hasattr(event, "total_tokens")
                else event.get("usage", {}).get("total_tokens", "?")
            )
            print(f"\n{'=' * 60}")
            print(f"✓ Report generated ({tokens} tokens)")
            break
        elif t == "error":
            err = event.error if hasattr(event, "error") else event.get("message", "")
            print(f"\n❌ Error: {err}")
            break

    # Save report
    full_report = "".join(report_text)
    output_dir.mkdir(parents=True, exist_ok=True)
    report_path = output_dir / f"news-radar-{date}.md"
    report_path.write_text(full_report, encoding="utf-8")
    print(f"\n💾 Saved to {report_path}")

    return full_report


# ─── Phase 5: Scheduler ─────────────────────────────────────────────

async def run_once(config_path: str, channels: dict, output_dir: Path):
    """Execute one full news radar cycle."""
    from a3s_code import Agent, SessionQueueConfig

    print(f"\n{'━' * 60}")
    print(f"📡 News Radar — {now_str()}")
    print(f"{'━' * 60}")

    # Create agent and session
    agent = Agent.create(config_path)

    qc = SessionQueueConfig()
    qc.set_query_concurrency(10)
    qc.set_execute_concurrency(4)
    qc.enable_metrics()
    qc.enable_dlq()

    session = agent.session(".", queue_config=qc, permissive=True)

    # Phase 1: Fetch
    print("\n── Phase 1: Multi-Channel Fetch ──")
    raw_items = await fetch_all_channels(session, channels)
    print(f"\n✓ Fetched {len(raw_items)} raw items")

    if not raw_items:
        print("⚠ No items fetched. Check network and config.")
        return

    # Phase 2: Deduplicate
    print("\n── Phase 2: Deduplicate ──")
    unique_items = deduplicate(raw_items)

    # Phase 3: LLM extraction
    print("\n── Phase 3: LLM Analysis ──")
    news_items = await extract_news_items(session, unique_items)

    if not news_items:
        print("⚠ No news items extracted.")
        return

    # Phase 4: Generate report
    print("\n── Phase 4: Generate Report ──")
    await generate_report(session, news_items, output_dir)

    # Stats
    if hasattr(session, "has_queue") and session.has_queue():
        try:
            stats = session.queue_stats()
            print(f"\n📊 Queue stats: pending={stats.get('total_pending', 0)}, "
                  f"failed={stats.get('total_failed', 0)}")
        except Exception:
            pass

    print(f"\n{'━' * 60}")
    print(f"✓ News Radar cycle complete — {now_str()}")
    print(f"{'━' * 60}\n")


async def schedule_loop(config_path: str, channels: dict, output_dir: Path,
                        hour: int = 8, minute: int = 0):
    """Run the radar at a fixed time daily."""
    print(f"⏰ Scheduled mode: will run daily at {hour:02d}:{minute:02d} UTC")
    print("   Press Ctrl+C to stop.\n")

    while True:
        now = datetime.now(timezone.utc)
        target = now.replace(hour=hour, minute=minute, second=0, microsecond=0)
        if target <= now:
            # Already past today's target, schedule for tomorrow
            from datetime import timedelta
            target += timedelta(days=1)

        wait_seconds = (target - now).total_seconds()
        hours_left = wait_seconds / 3600
        print(f"⏳ Next run in {hours_left:.1f} hours ({target.strftime('%Y-%m-%d %H:%M UTC')})")

        await asyncio.sleep(wait_seconds)

        try:
            await run_once(config_path, channels, output_dir)
        except Exception as e:
            print(f"❌ Cycle failed: {e}")
            import traceback
            traceback.print_exc()


# ─── CLI Entry Point ─────────────────────────────────────────────────

def parse_args():
    parser = argparse.ArgumentParser(
        description="News Radar — Multi-Channel News Aggregation & Analysis Agent"
    )
    parser.add_argument(
        "--config", type=str, default=None,
        help="Path to agent.hcl config file",
    )
    parser.add_argument(
        "--topics", type=str, default=None,
        help="Comma-separated topic list (default: tech,ai,dev)",
    )
    parser.add_argument(
        "--schedule", action="store_true",
        help="Run in scheduled mode (daily at 08:00 UTC)",
    )
    parser.add_argument(
        "--hour", type=int, default=8,
        help="Schedule hour (UTC, default: 8)",
    )
    parser.add_argument(
        "--output", type=str, default="./reports",
        help="Output directory for reports (default: ./reports)",
    )
    return parser.parse_args()


def build_channels(topics: str | None) -> dict:
    """Filter channels by topic list."""
    if topics is None:
        return DEFAULT_CHANNELS
    requested = [t.strip().lower() for t in topics.split(",")]
    filtered = {k: v for k, v in DEFAULT_CHANNELS.items() if k in requested}
    if not filtered:
        print(f"⚠ No matching topics for: {topics}")
        print(f"  Available: {', '.join(DEFAULT_CHANNELS.keys())}")
        sys.exit(1)
    return filtered


async def main():
    args = parse_args()

    config_path = args.config or find_config()
    channels = build_channels(args.topics)
    output_dir = Path(args.output)

    print(f"📡 News Radar Agent")
    print(f"   Config:  {config_path}")
    print(f"   Topics:  {', '.join(channels.keys())}")
    print(f"   Output:  {output_dir}")

    if args.schedule:
        await schedule_loop(config_path, channels, output_dir, hour=args.hour)
    else:
        await run_once(config_path, channels, output_dir)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n\n👋 News Radar stopped.")
