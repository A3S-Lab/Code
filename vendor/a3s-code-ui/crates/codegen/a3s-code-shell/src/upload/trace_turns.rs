//! Request, report and skip reasons for the `a3s trace` per-turn gap-fill, plus the entry point
//! that resolves to the real upload or to a stub that reports the feature as unavailable.
use crate::session::repo_changes::TraceExportConfig;
use serde::Serialize;
use std::path::Path;
/// Stamped into `metadata.json.client_source` so every consumer can tell these turns from live captures.
pub const TRACE_RECONSTRUCTED_CLIENT_SOURCE: &str = "grok-trace-reconstructed";
pub struct TraceTurnsRequest<'a> {
    pub session_id: &'a str,
    pub session_dir: &'a Path,
    pub upload_config: &'a TraceExportConfig,
    /// Uploaded moments ago by the caller; a probe chunk is trusted only if it reports this present.
    pub canary_object_path: &'a str,
    /// The bundle bytes already uploaded; reused as the final turn's session-state archive when ≤ 50 MiB.
    pub bundle_archive: &'a [u8],
    pub client_version: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceTurnsSkipReason {
    /// Not available in this build.
    BuildUnavailable,
    /// Direct GCS and S3 destinations have no existence probe, so never-overwrite cannot be guaranteed.
    UnsupportedUploadMethod,
    /// No prompt boundaries in the chat history.
    NoTurns,
    /// Rebuilding turns from the session directory did not complete.
    ReconstructionFailed,
    /// The probe failed, was unauthorized, or did not report the canary.
    ExistsProbeUnavailable,
    /// The trace-turn counter is absent or names more turns than can be probed.
    TurnNumberingUnknown,
    /// The trace-turn counter disagrees with the prompt indexes and at least one live turn folder exists.
    TurnNumberingMisaligned,
}
impl TraceTurnsSkipReason {
    /// User-facing sentence; no hostnames, buckets, or URLs.
    pub fn user_message(self) -> &'static str {
        match self {
            TraceTurnsSkipReason::BuildUnavailable => "not available in this build",
            TraceTurnsSkipReason::UnsupportedUploadMethod => {
                "only proxy uploads can check for existing turn artifacts"
            }
            TraceTurnsSkipReason::NoTurns => "no turns found in the session history",
            TraceTurnsSkipReason::ReconstructionFailed => {
                "could not rebuild turns from the session directory"
            }
            TraceTurnsSkipReason::ExistsProbeUnavailable => {
                "could not confirm which turn artifacts already exist, so nothing was written"
            }
            TraceTurnsSkipReason::TurnNumberingUnknown => {
                "could not determine the session's turn numbering"
            }
            TraceTurnsSkipReason::TurnNumberingMisaligned => {
                "turn numbering does not match the prompt history and live turn artifacts exist"
            }
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TraceTurnsReport {
    pub turns_total: usize,
    pub turns_reconstructed: usize,
    /// The newest turn was left out because a live agent may still be writing it.
    pub turns_in_flight: usize,
    pub artifacts_uploaded: usize,
    pub artifacts_skipped_existing: usize,
    pub artifacts_skipped_oversize: usize,
    pub artifacts_failed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<TraceTurnsSkipReason>,
}
impl TraceTurnsReport {
    #[must_use]
    pub fn skipped(reason: TraceTurnsSkipReason) -> Self {
        TraceTurnsReport {
            skipped_reason: Some(reason),
            ..TraceTurnsReport::default()
        }
    }
}
pub async fn upload_trace_turns(_request: TraceTurnsRequest<'_>) -> TraceTurnsReport {
    TraceTurnsReport::skipped(TraceTurnsSkipReason::BuildUnavailable)
}
