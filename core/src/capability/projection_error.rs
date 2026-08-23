use thiserror::Error;

use super::CapabilityKind;

const MAX_ADAPTER_ERROR_BYTES: usize = 1_024;

/// Bounded failure returned by a surface-owned capability projection adapter.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct CapabilityAdapterError {
    message: Box<str>,
}

impl CapabilityAdapterError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let message = if message.is_empty() {
            "capability adapter preparation failed".to_owned()
        } else {
            truncate_utf8(message, MAX_ADAPTER_ERROR_BYTES)
        };
        Self {
            message: message.into_boxed_str(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Failure that leaves the currently published projection unchanged.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CapabilityProjectionError {
    #[error("Capability projection is missing runtime value '{capability}'")]
    MissingValue { capability: String },
    #[error("Capability projection contains unknown runtime value '{capability}'")]
    UnexpectedValue { capability: String },
    #[error("Capability projection repeats runtime value '{capability}'")]
    DuplicateValue { capability: String },
    #[error("Capability kind '{kind}' has no supported A3S Code runtime projection")]
    UnsupportedKind { kind: CapabilityKind },
    #[error(
        "Capability '{capability}' descriptor kind '{descriptor_kind}' does not match runtime value kind '{value_kind}'"
    )]
    KindMismatch {
        capability: String,
        descriptor_kind: CapabilityKind,
        value_kind: CapabilityKind,
    },
    #[error(
        "Capability '{capability}' publishes name '{expected}', but its runtime value publishes '{actual}'"
    )]
    PublicNameMismatch {
        capability: String,
        expected: String,
        actual: String,
    },
    #[error("Capability transaction stages unknown target value '{capability}'")]
    UnknownStagedCapability { capability: String },
    #[error("Capability transaction stages value '{capability}' more than once")]
    DuplicateStagedCapability { capability: String },
    #[error(
        "Capability transaction target generation must be {expected}, but the target set is {actual}"
    )]
    TargetGenerationMismatch { expected: u64, actual: u64 },
    #[error("Capability catalog generation is exhausted")]
    GenerationExhausted,
    #[error("Capability adapter for '{capability}' failed to prepare: {message}")]
    PrepareFailed { capability: String, message: String },
    #[error("Capability transaction preparation was cancelled")]
    Cancelled,
    #[error("Capability transaction exceeds its effect bound of {max}")]
    EffectBoundExceeded { max: usize },
    #[error(
        "Capability commit lost its catalog compare-and-swap race (expected generation {expected_generation} digest {expected_digest}, found generation {actual_generation} digest {actual_digest})"
    )]
    CommitConflict {
        expected_generation: u64,
        expected_digest: String,
        actual_generation: u64,
        actual_digest: String,
    },
    #[error("Capability transaction entered an invalid internal typestate")]
    InvalidTransactionState,
}

fn truncate_utf8(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut boundary = max;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}
