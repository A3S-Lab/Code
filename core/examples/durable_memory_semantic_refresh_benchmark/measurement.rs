use a3s_code_core::memory::{SemanticRefreshRunMetrics, SemanticRefreshRunOutcome};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Latency {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunWork {
    pub sequence: u64,
    pub outcome: SemanticRefreshRunOutcome,
    pub elapsed_ms: u64,
    pub source_change_token_requests: u64,
    pub source_change_token_observations: u64,
    pub source_snapshot_requests: u64,
    pub source_snapshot_node_reads: u64,
    pub source_snapshot_bytes: u64,
    pub embedding_cache_hits: u64,
    pub embedding_inputs: u64,
    pub embedding_input_bytes: u64,
    pub provider_requests: u64,
    pub provider_inputs: u64,
    pub provider_input_bytes: u64,
    pub publication_attempts: u64,
    pub publication_records: u64,
}

impl From<&SemanticRefreshRunMetrics> for RunWork {
    fn from(run: &SemanticRefreshRunMetrics) -> Self {
        Self {
            sequence: run.sequence(),
            outcome: run.outcome(),
            elapsed_ms: run.elapsed_ms(),
            source_change_token_requests: run.source_change_token_requests(),
            source_change_token_observations: run.source_change_token_observations(),
            source_snapshot_requests: run.source_snapshot_requests(),
            source_snapshot_node_reads: run.source_snapshot_node_reads(),
            source_snapshot_bytes: run.source_snapshot_bytes(),
            embedding_cache_hits: run.embedding_cache_hits(),
            embedding_inputs: run.embedding_inputs(),
            embedding_input_bytes: run.embedding_input_bytes(),
            provider_requests: run.provider_requests(),
            provider_inputs: run.provider_inputs(),
            provider_input_bytes: run.provider_input_bytes(),
            publication_attempts: run.publication_attempts(),
            publication_records: run.publication_records(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "rule", content = "bytes")]
pub enum ExpectedBytes {
    Zero,
    Positive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedWork {
    pub sequence: u64,
    pub outcome: SemanticRefreshRunOutcome,
    pub source_change_token_requests: u64,
    pub source_snapshot_requests: u64,
    pub source_snapshot_node_reads: u64,
    pub source_snapshot_bytes: ExpectedBytes,
    pub embedding_cache_hits: u64,
    pub embedding_inputs: u64,
    pub embedding_input_bytes: u64,
    pub provider_requests: u64,
    pub provider_inputs: u64,
    pub provider_input_bytes: u64,
    pub publication_attempts: u64,
    pub publication_records: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseEvidence {
    pub name: &'static str,
    pub observed: RunWork,
    pub expected: ExpectedWork,
    pub violations: Vec<String>,
    pub passed: bool,
}

impl PhaseEvidence {
    pub fn evaluate(
        name: &'static str,
        run: &SemanticRefreshRunMetrics,
        expected: ExpectedWork,
    ) -> Self {
        let observed = RunWork::from(run);
        Self::evaluate_observed(name, observed, expected)
    }

    fn evaluate_observed(name: &'static str, observed: RunWork, expected: ExpectedWork) -> Self {
        let mut violations = Vec::new();
        compare(
            &mut violations,
            "sequence",
            observed.sequence,
            expected.sequence,
        );
        if observed.outcome != expected.outcome {
            violations.push(format!(
                "outcome was {:?}, expected {:?}",
                observed.outcome, expected.outcome
            ));
        }
        compare(
            &mut violations,
            "source change-token requests",
            observed.source_change_token_requests,
            expected.source_change_token_requests,
        );
        compare(
            &mut violations,
            "source change-token observations",
            observed.source_change_token_observations,
            expected.source_change_token_requests,
        );
        compare(
            &mut violations,
            "source snapshot requests",
            observed.source_snapshot_requests,
            expected.source_snapshot_requests,
        );
        compare(
            &mut violations,
            "source snapshot node reads",
            observed.source_snapshot_node_reads,
            expected.source_snapshot_node_reads,
        );
        match expected.source_snapshot_bytes {
            ExpectedBytes::Zero if observed.source_snapshot_bytes != 0 => violations.push(format!(
                "source snapshot bytes were {}, expected zero",
                observed.source_snapshot_bytes
            )),
            ExpectedBytes::Positive if observed.source_snapshot_bytes == 0 => {
                violations.push("source snapshot bytes were zero, expected a positive value".into())
            }
            ExpectedBytes::Zero | ExpectedBytes::Positive => {}
        }
        for (label, actual, wanted) in [
            (
                "embedding cache hits",
                observed.embedding_cache_hits,
                expected.embedding_cache_hits,
            ),
            (
                "embedding inputs",
                observed.embedding_inputs,
                expected.embedding_inputs,
            ),
            (
                "embedding input bytes",
                observed.embedding_input_bytes,
                expected.embedding_input_bytes,
            ),
            (
                "provider requests",
                observed.provider_requests,
                expected.provider_requests,
            ),
            (
                "provider inputs",
                observed.provider_inputs,
                expected.provider_inputs,
            ),
            (
                "provider input bytes",
                observed.provider_input_bytes,
                expected.provider_input_bytes,
            ),
            (
                "publication attempts",
                observed.publication_attempts,
                expected.publication_attempts,
            ),
            (
                "publication records",
                observed.publication_records,
                expected.publication_records,
            ),
        ] {
            compare(&mut violations, label, actual, wanted);
        }
        Self {
            name,
            observed,
            expected,
            passed: violations.is_empty(),
            violations,
        }
    }
}

fn compare(violations: &mut Vec<String>, label: &str, actual: u64, expected: u64) {
    if actual != expected {
        violations.push(format!("{label} was {actual}, expected {expected}"));
    }
}

pub fn latency(mut samples: Vec<Duration>) -> Result<Latency> {
    if samples.is_empty() {
        bail!("benchmark has no measured samples");
    }
    samples.sort_unstable();
    let maximum = samples
        .last()
        .copied()
        .context("sorted benchmark samples unexpectedly became empty")?;
    Ok(Latency {
        p50_ms: percentile_ms(&samples, 50),
        p95_ms: percentile_ms(&samples, 95),
        max_ms: duration_ms(maximum),
    })
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    let rank = (samples.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(samples.len() - 1);
    duration_ms(samples[rank])
}

pub fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

pub fn rss_delta(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    Some(after?.saturating_sub(before?))
}

pub fn rss_within_budget(value: Option<u64>, budget: u64) -> bool {
    match value {
        Some(value) => value <= budget,
        None => !cfg!(target_os = "linux"),
    }
}

pub fn max_rss(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

pub fn resident_set_bytes() -> Option<u64> {
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

pub fn machine_metadata() -> serde_json::Value {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_gate_rejects_work_amplification_and_snapshot_rule_drift() {
        let expected = ExpectedWork {
            sequence: 1,
            outcome: SemanticRefreshRunOutcome::Published,
            source_change_token_requests: 3,
            source_snapshot_requests: 1,
            source_snapshot_node_reads: 10,
            source_snapshot_bytes: ExpectedBytes::Positive,
            embedding_cache_hits: 9,
            embedding_inputs: 1,
            embedding_input_bytes: 12,
            provider_requests: 1,
            provider_inputs: 1,
            provider_input_bytes: 12,
            publication_attempts: 1,
            publication_records: 10,
        };
        let observed = RunWork {
            sequence: 1,
            outcome: SemanticRefreshRunOutcome::Published,
            elapsed_ms: 7,
            source_change_token_requests: 3,
            source_change_token_observations: 3,
            source_snapshot_requests: 1,
            source_snapshot_node_reads: 10,
            source_snapshot_bytes: 128,
            embedding_cache_hits: 9,
            embedding_inputs: 1,
            embedding_input_bytes: 12,
            provider_requests: 1,
            provider_inputs: 1,
            provider_input_bytes: 12,
            publication_attempts: 1,
            publication_records: 10,
        };
        assert!(PhaseEvidence::evaluate_observed("valid", observed, expected).passed);

        let mut drifted = observed;
        drifted.source_snapshot_bytes = 0;
        drifted.provider_inputs = 2;
        let rejected = PhaseEvidence::evaluate_observed("drifted", drifted, expected);
        assert!(!rejected.passed);
        assert_eq!(rejected.violations.len(), 2);
    }

    #[test]
    fn latency_uses_nearest_rank_percentiles() {
        let values = (1..=20).map(Duration::from_millis).collect();
        let observed = latency(values).unwrap();
        assert_eq!(observed.p50_ms, 10.0);
        assert_eq!(observed.p95_ms, 19.0);
        assert_eq!(observed.max_ms, 20.0);
    }
}
