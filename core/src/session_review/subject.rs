//! Typed review subjects — what is being verified, not which UI skin is used.

use super::finding::SessionReviewAnchorV1;
use super::{validate_id, SessionReviewError};
use serde::{Deserialize, Serialize};

/// Bound review target. Scenarios choose a subject; Core does not invent rubrics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
pub enum ReviewSubjectV1 {
    /// Assistant turn / bubble in the session transcript (sticky reply review).
    Transcript {
        #[serde(flatten)]
        anchor: SessionReviewAnchorV1,
    },
    /// Host / Use escape hatch for custom scenarios (bounded ids only).
    Opaque { kind: String, uri: String },
}

impl ReviewSubjectV1 {
    pub fn transcript(anchor: SessionReviewAnchorV1) -> Self {
        Self::Transcript { anchor }
    }

    pub fn opaque(
        kind: impl Into<String>,
        uri: impl Into<String>,
    ) -> Result<Self, SessionReviewError> {
        let subject = Self::Opaque {
            kind: kind.into(),
            uri: uri.into(),
        };
        subject.validate()?;
        Ok(subject)
    }

    pub fn validate(&self) -> Result<(), SessionReviewError> {
        match self {
            Self::Transcript { anchor } => anchor.validate(),
            Self::Opaque { kind, uri } => {
                validate_id("subject.kind", kind)?;
                validate_id("subject.uri", uri)?;
                Ok(())
            }
        }
    }

    pub fn as_transcript_anchor(&self) -> Option<&SessionReviewAnchorV1> {
        match self {
            Self::Transcript { anchor } => Some(anchor),
            Self::Opaque { .. } => None,
        }
    }

    pub const fn supports_strong_annotation(&self) -> bool {
        match self {
            Self::Transcript { anchor } => anchor.supports_strong_annotation(),
            Self::Opaque { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_rejects_newlines_in_kind() {
        assert!(ReviewSubjectV1::opaque("bad\nkind", "uri:1").is_err());
    }

    #[test]
    fn transcript_round_trips() {
        let subject = ReviewSubjectV1::transcript(
            SessionReviewAnchorV1::new("turn-1")
                .unwrap()
                .with_span(0, 4)
                .unwrap(),
        );
        let bytes = serde_json::to_vec(&subject).unwrap();
        let decoded: ReviewSubjectV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, subject);
        assert!(decoded.supports_strong_annotation());
    }
}
