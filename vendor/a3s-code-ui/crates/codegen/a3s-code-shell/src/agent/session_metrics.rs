//! Session lifecycle event structs.
//!
//! The structs moved to `a3s-code-telemetry` in the telemetry crate split.
//! This module re-exports them so the existing import path in shell keeps working.

pub(crate) use a3s_code_telemetry::session_metrics::{
    DoomLoopDetected, DoomLoopRecovery, LongReasoningReminderTurn, SessionContextSnapshot,
    SessionStartKind, SessionStarted, TraceUploadAttempted, TraceUploadFailed, TraceUploadSkipped,
    TraceUploadSucceeded, Turn, TurnCompletedLifecycle,
};
