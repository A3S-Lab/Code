//! Report-pack assembly: distributions, DM-PROD1 rows, manifest, hygiene scan.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Bounded latency or cost distribution retained for a qualification row.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Distribution {
    pub count: usize,
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

impl Distribution {
    pub fn from_samples(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted: Vec<f64> = samples.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return Self::default();
        }
        sorted.sort_by(f64::total_cmp);
        let sum: f64 = sorted.iter().sum();
        Self {
            count: sorted.len(),
            min: sorted[0],
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            max: sorted[sorted.len() - 1],
            mean: sum / sorted.len() as f64,
        }
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let rank = (quantile * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// One DM-PROD1 table row with its verdict and supporting evidence.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dimension {
    pub id: &'static str,
    pub dimension: &'static str,
    pub pass_criteria: &'static str,
    pub passed: bool,
    /// Why a satisfied row is still narrower than the manual's intent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caveat: Option<String>,
    pub evidence: Value,
}

impl Dimension {
    pub fn new(
        id: &'static str,
        dimension: &'static str,
        pass_criteria: &'static str,
        passed: bool,
        evidence: Value,
    ) -> Self {
        Self {
            id,
            dimension,
            pass_criteria,
            passed,
            caveat: None,
            evidence,
        }
    }

    pub fn with_caveat(mut self, caveat: impl Into<String>) -> Self {
        self.caveat = Some(caveat.into());
        self
    }
}

/// Result of writing and scanning one report pack.
#[derive(Debug)]
pub struct Pack {
    pub directory: PathBuf,
    pub report_path: PathBuf,
    pub manifest_path: PathBuf,
    pub hygiene_path: PathBuf,
    pub hygiene_ok: bool,
    pub hygiene_findings: Vec<String>,
}

/// Literal strings that must never appear anywhere in the pack.
pub struct HygienePolicy {
    /// Credential values and credential-shaped markers.
    pub secrets: Vec<String>,
    /// Corpus and query plaintext.
    pub plaintext: Vec<String>,
}

impl HygienePolicy {
    /// Scan a value that is about to be serialized into the pack.
    ///
    /// This lets the report carry its own hygiene verdict; `write_pack` still
    /// rescans everything on disk, so the marker file remains authoritative.
    pub fn scan_value(&self, label: &str, value: &Value) -> Result<Vec<String>> {
        let body = serde_json::to_string(value)?;
        Ok(self.findings(label, &body))
    }

    fn findings(&self, label: &str, body: &str) -> Vec<String> {
        let mut findings = Vec::new();
        for secret in &self.secrets {
            if !secret.is_empty() && body.contains(secret.as_str()) {
                findings.push(format!("{label}: credential marker present"));
            }
        }
        for plaintext in &self.plaintext {
            if !plaintext.is_empty() && body.contains(plaintext.as_str()) {
                findings.push(format!("{label}: prompt plaintext present"));
            }
        }
        findings
    }
}

/// Write `report`, then derive the manifest and hygiene verdict from the bytes
/// actually on disk rather than from the in-memory value.
pub async fn write_pack(
    directory: PathBuf,
    report: &Value,
    policy: &HygienePolicy,
) -> Result<Pack> {
    tokio::fs::create_dir_all(&directory)
        .await
        .with_context(|| format!("could not create {}", directory.display()))?;
    let report_path = directory.join("report.json");
    let body = serde_json::to_vec_pretty(report)?;
    tokio::fs::write(&report_path, &body).await?;

    let mut entries = Vec::new();
    let mut findings = Vec::new();
    for path in sorted_files(&directory).await? {
        let bytes = tokio::fs::read(&path).await?;
        let name = file_name(&path);
        if let Ok(text) = std::str::from_utf8(&bytes) {
            findings.extend(policy.findings(&name, text));
        }
        entries.push(json!({
            "file": name,
            "bytes": bytes.len(),
            "sha256": format!("sha256:{:x}", Sha256::digest(&bytes)),
        }));
    }

    let hygiene_ok = findings.is_empty();
    let hygiene_path = directory.join(if hygiene_ok {
        "HYGIENE_OK"
    } else {
        "HYGIENE_FAIL"
    });
    let hygiene_body = if hygiene_ok {
        "DM-PROD1 secret hygiene: no provider credential or prompt plaintext found in this pack.\n"
            .to_string()
    } else {
        format!("DM-PROD1 secret hygiene FAILED.\n{}\n", findings.join("\n"))
    };
    tokio::fs::write(&hygiene_path, hygiene_body).await?;

    let manifest_path = directory.join("MANIFEST.json");
    let manifest = json!({
        "schemaVersion": 1,
        "profile": "a3s.code.dm-prod1-host.v1",
        "hygiene": if hygiene_ok { "HYGIENE_OK" } else { "HYGIENE_FAIL" },
        "files": entries,
    });
    tokio::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?).await?;

    Ok(Pack {
        directory,
        report_path,
        manifest_path,
        hygiene_path,
        hygiene_ok,
        hygiene_findings: findings,
    })
}

async fn sorted_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut reader = tokio::fs::read_dir(directory).await?;
    let mut files = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        if entry.file_type().await?.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}
