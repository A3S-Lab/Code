//! Release qualification for context assembly and the default ephemeral memory path.
//!
//! Corpus construction is excluded from query latency. The measured profile
//! includes ranking, deduplication, prompt rendering, and the Code-owned
//! `AgentMemory` recall call over the pinned `a3s-memory` in-process store.

use a3s_code_core::context::{
    ContextAssembler, ContextBudget, ContextItem, ContextResult, ContextSourcePolicy, ContextType,
};
use a3s_code_core::memory::AgentMemory;
use a3s_memory::{InMemoryStore, MemoryItem, MemoryStore, MemoryType};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CONTEXT_INPUT_ITEMS: usize = 25_000;
const CONTEXT_UNIQUE_ITEMS: usize = 20_000;
const CONTEXT_PROVIDERS: usize = 10;
const CONTEXT_SELECTED_ITEMS: usize = 64;
const CONTEXT_TOKEN_BUDGET: usize = 16_384;
const MEMORY_CORPUS_ITEMS: usize = 2_500;
const MEMORY_RESULT_LIMIT: usize = 20;
const WARMUP_SAMPLES: usize = 3;
const MEASURED_SAMPLES: usize = 20;
const CONTEXT_P95_BUDGET_MS: f64 = 500.0;
const MEMORY_P95_BUDGET_MS: f64 = 250.0;
const ACTIVE_RSS_DELTA_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const RETAINED_RSS_DELTA_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize)]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        anyhow::bail!("context and memory qualification must run with --release");
    }

    let rss_before = resident_set_bytes();
    let context_setup_started = Instant::now();
    let (context_results, context_input_bytes) = context_corpus();
    let context_setup_elapsed = context_setup_started.elapsed();
    let assembler = ContextAssembler::new(ContextBudget {
        max_items: CONTEXT_SELECTED_ITEMS,
        max_tokens: CONTEXT_TOKEN_BUDGET,
    })
    .with_source_policy(ContextSourcePolicy {
        max_items_per_source: None,
        max_tokens_per_source: None,
    });

    let baseline = assemble_and_validate(&assembler, &context_results)?;
    for _ in 0..WARMUP_SAMPLES {
        let observed = assemble_and_validate(&assembler, &context_results)?;
        ensure_same_assembly(&baseline, &observed)?;
    }
    let mut context_samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        let observed = assemble_and_validate(&assembler, &context_results)?;
        context_samples.push(started.elapsed());
        ensure_same_assembly(&baseline, &observed)?;
    }
    let context_latency = latency(context_samples)?;

    let memory_setup_started = Instant::now();
    let store = Arc::new(InMemoryStore::new());
    for index in 0..MEMORY_CORPUS_ITEMS {
        let marker = if index == 42 {
            " unique_recall_marker_0042"
        } else {
            ""
        };
        let mut item = MemoryItem::new(format!(
            "Deterministic memory corpus entry {index:04}.{marker} It records a bounded agent observation."
        ))
        .with_type(MemoryType::Semantic)
        .with_importance(0.5)
        .with_tag(format!("partition-{}", index % 32));
        item.id = format!("memory-{index:04}");
        item.timestamp = DateTime::<Utc>::UNIX_EPOCH;
        store.store(item).await?;
    }
    let memory_setup_elapsed = memory_setup_started.elapsed();
    if store.count().await? != MEMORY_CORPUS_ITEMS {
        anyhow::bail!("memory corpus did not retain the expected item count");
    }
    let memory = AgentMemory::new(store.clone());
    for _ in 0..WARMUP_SAMPLES {
        validate_recall(
            memory
                .recall_similar("unique_recall_marker_0042", MEMORY_RESULT_LIMIT)
                .await?,
        )?;
    }
    let mut memory_samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        let matches = memory
            .recall_similar("unique_recall_marker_0042", MEMORY_RESULT_LIMIT)
            .await?;
        memory_samples.push(started.elapsed());
        validate_recall(matches)?;
    }
    let memory_latency = latency(memory_samples)?;
    let rss_active = resident_set_bytes();

    let context_passed = context_latency.p95_ms <= CONTEXT_P95_BUDGET_MS;
    let memory_passed = memory_latency.p95_ms <= MEMORY_P95_BUDGET_MS;
    let active_rss_delta = rss_delta(rss_before, rss_active);

    drop(memory);
    drop(store);
    drop(context_results);
    drop(assembler);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rss_after_drop = resident_set_bytes();
    let retained_rss_delta = rss_delta(rss_before, rss_after_drop);
    let rss_passed = rss_within_budget(active_rss_delta, ACTIVE_RSS_DELTA_BUDGET_BYTES)
        && rss_within_budget(retained_rss_delta, RETAINED_RSS_DELTA_BUDGET_BYTES);
    let passed = context_passed && memory_passed && rss_passed;

    let report = json!({
        "schemaVersion": 1,
        "profile": "context-memory-corpus-v1",
        "build": "release",
        "machine": machine_metadata(),
        "parameters": {
            "warmupSamples": WARMUP_SAMPLES,
            "measuredSamples": MEASURED_SAMPLES,
        },
        "contextAssembly": {
            "inputItems": CONTEXT_INPUT_ITEMS,
            "uniqueItems": CONTEXT_UNIQUE_ITEMS,
            "providers": CONTEXT_PROVIDERS,
            "inputBytes": context_input_bytes,
            "selectedItems": baseline.ids.len(),
            "selectedTokens": baseline.tokens,
            "renderedBytes": baseline.rendered_bytes,
            "setupObservedMs": duration_ms(context_setup_elapsed),
            "setupIncludedInLatency": false,
            "p50Ms": context_latency.p50_ms,
            "p95Ms": context_latency.p95_ms,
            "maxMs": context_latency.max_ms,
            "budgetP95Ms": CONTEXT_P95_BUDGET_MS,
            "passed": context_passed,
        },
        "memoryRecall": {
            "backend": "a3s-memory/InMemoryStore",
            "codeBoundary": "AgentMemory::recall_similar",
            "corpusItems": MEMORY_CORPUS_ITEMS,
            "resultLimit": MEMORY_RESULT_LIMIT,
            "setupObservedMs": duration_ms(memory_setup_elapsed),
            "setupIncludedInLatency": false,
            "p50Ms": memory_latency.p50_ms,
            "p95Ms": memory_latency.p95_ms,
            "maxMs": memory_latency.max_ms,
            "budgetP95Ms": MEMORY_P95_BUDGET_MS,
            "providerNetworkIncluded": false,
            "passed": memory_passed,
        },
        "resources": {
            "rssBeforeBytes": rss_before,
            "rssActiveBytes": rss_active,
            "rssAfterDropBytes": rss_after_drop,
            "activeRssDeltaBytes": active_rss_delta,
            "activeRssDeltaBudgetBytes": ACTIVE_RSS_DELTA_BUDGET_BYTES,
            "retainedRssDeltaBytes": retained_rss_delta,
            "retainedRssDeltaBudgetBytes": RETAINED_RSS_DELTA_BUDGET_BYTES,
            "linuxRssRequired": cfg!(target_os = "linux"),
            "passed": rss_passed,
        },
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        anyhow::bail!(
            "context/memory qualification failed: assembly p95 {:.3} ms, recall p95 {:.3} ms",
            context_latency.p95_ms,
            memory_latency.p95_ms,
        );
    }
    Ok(())
}

#[derive(Debug)]
struct AssemblyObservation {
    ids: Vec<String>,
    tokens: usize,
    rendered_bytes: usize,
}

fn context_corpus() -> (Vec<ContextResult>, usize) {
    let items_per_provider = CONTEXT_INPUT_ITEMS / CONTEXT_PROVIDERS;
    let mut results = Vec::with_capacity(CONTEXT_PROVIDERS);
    let mut input_bytes = 0usize;
    for provider_index in 0..CONTEXT_PROVIDERS {
        let mut result = ContextResult::new(format!("provider-{provider_index:02}"));
        for item_index in 0..items_per_provider {
            let ordinal = provider_index * items_per_provider + item_index;
            let logical = ordinal % CONTEXT_UNIQUE_ITEMS;
            let content = format!(
                "Context evidence {logical:05} from provider {provider_index:02}; bounded ranking and deterministic deduplication fixture."
            );
            input_bytes = input_bytes.saturating_add(content.len());
            let relevance = ((logical * 17 + provider_index * 31) % 1_000) as f32 / 1_000.0;
            result.add_item(
                ContextItem::new(
                    format!("context-{provider_index:02}-{item_index:04}"),
                    ContextType::Resource,
                    content,
                )
                .with_token_count(32)
                .with_relevance(relevance)
                .with_priority(((logical * 7) % 100) as f32 / 100.0)
                .with_trust(((logical * 11) % 100) as f32 / 100.0)
                .with_freshness(((logical * 13) % 100) as f32 / 100.0)
                .with_source(format!("fixture://evidence/{logical:05}"))
                .with_provenance(format!("provider-{provider_index:02}")),
            );
        }
        results.push(result);
    }
    (results, input_bytes)
}

fn assemble_and_validate(
    assembler: &ContextAssembler,
    results: &[ContextResult],
) -> anyhow::Result<AssemblyObservation> {
    let assembly = assembler.assemble(results);
    if assembly.items.len() != CONTEXT_SELECTED_ITEMS
        || assembly.total_tokens > CONTEXT_TOKEN_BUDGET
        || !assembly.truncated
    {
        anyhow::bail!(
            "context assembly violated its budget: items={}, tokens={}, truncated={}",
            assembly.items.len(),
            assembly.total_tokens,
            assembly.truncated,
        );
    }
    let rendered = assembly.to_xml();
    if rendered.is_empty() {
        anyhow::bail!("context assembly rendered an empty prompt projection");
    }
    Ok(AssemblyObservation {
        ids: assembly.items.into_iter().map(|item| item.id).collect(),
        tokens: assembly.total_tokens,
        rendered_bytes: rendered.len(),
    })
}

fn ensure_same_assembly(
    expected: &AssemblyObservation,
    observed: &AssemblyObservation,
) -> anyhow::Result<()> {
    if observed.ids != expected.ids
        || observed.tokens != expected.tokens
        || observed.rendered_bytes != expected.rendered_bytes
    {
        anyhow::bail!("context assembly changed order or resource counts between samples");
    }
    Ok(())
}

fn validate_recall(items: Vec<MemoryItem>) -> anyhow::Result<()> {
    if items.is_empty() || items.len() > MEMORY_RESULT_LIMIT {
        anyhow::bail!("memory recall returned an invalid result count");
    }
    if items[0].id != "memory-0042" {
        anyhow::bail!("memory recall did not rank the deterministic target first");
    }
    Ok(())
}

fn latency(mut samples: Vec<Duration>) -> anyhow::Result<Latency> {
    if samples.is_empty() {
        anyhow::bail!("benchmark has no measured samples");
    }
    samples.sort_unstable();
    Ok(Latency {
        p50_ms: percentile_ms(&samples, 50),
        p95_ms: percentile_ms(&samples, 95),
        max_ms: duration_ms(*samples.last().expect("samples are not empty")),
    })
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    let rank = (samples.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(samples.len() - 1);
    duration_ms(samples[rank])
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn rss_delta(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    Some(after?.saturating_sub(before?))
}

fn rss_within_budget(value: Option<u64>, budget: u64) -> bool {
    match value {
        Some(value) => value <= budget,
        None => !cfg!(target_os = "linux"),
    }
}

fn resident_set_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

fn machine_metadata() -> serde_json::Value {
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logicalCpus": std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
        "processor": processor_name(),
    })
}

fn processor_name() -> Option<String> {
    std::env::var("PROCESSOR_IDENTIFIER").ok().or_else(|| {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        cpuinfo.lines().find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|value| value.split_once(':'))
                .map(|(_, value)| value.trim().to_owned())
        })
    })
}
