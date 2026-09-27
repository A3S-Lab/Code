//! [`GrpcRetryPolicy`] — classifies a `tonic::Code` into a [`Disposition`], the
//! gRPC analogue of [`crate::RetryPolicy`]. Behind the `grpc` feature.

use crate::retry_policy::Disposition;
use std::error::Error;
use tonic::Code;

/// Maps a gRPC [`Code`] to a [`Disposition`].
pub struct GrpcRetryPolicy {
    retryable: &'static [Code],
}

impl GrpcRetryPolicy {
    /// Retry only transient connection errors (`Unavailable`, `Unknown`);
    /// excluding `Internal`/`DeadlineExceeded` avoids amplifying a sick peer.
    pub const DEFAULT: Self = Self::new(&[Code::Unavailable, Code::Unknown]);

    /// Permissive preset: also retry `Internal` and `DeadlineExceeded`.
    pub const PERMISSIVE: Self = Self::new(&[
        Code::Unavailable,
        Code::Unknown,
        Code::Internal,
        Code::DeadlineExceeded,
    ]);

    /// Construct from an explicit retryable-code set.
    pub const fn new(retryable: &'static [Code]) -> Self {
        Self { retryable }
    }

    /// Classify `code`. Returns `None` for `Code::Ok` (success, not an error).
    pub fn classify(&self, code: Code) -> Option<Disposition> {
        match code {
            Code::Ok => None,
            c if self.is_retryable(c) => Some(Disposition::Retryable),
            _ => Some(Disposition::Terminal),
        }
    }

    /// `true` iff `code` is in the retryable set.
    pub fn is_retryable(&self, code: Code) -> bool {
        self.retryable.contains(&code)
    }

    /// [`Self::is_retryable`] on the code, or a `status` the client synthesized
    /// from a connection failure (GOAWAY, connection closed, channel timeout),
    /// which carries the error as source where a server-sent status never does.
    /// `ResourceExhausted` (ENHANCE_YOUR_CALM) and `PermissionDenied`
    /// (INADEQUATE_SECURITY) are peer verdicts and stay terminal.
    pub fn is_retryable_status(&self, status: &tonic::Status) -> bool {
        self.is_retryable(status.code())
            || (status.source().is_some()
                && matches!(
                    status.code(),
                    Code::Internal | Code::Cancelled | Code::Unknown | Code::Unavailable
                ))
    }
}

impl Default for GrpcRetryPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_retries_transient_codes() {
        for c in [Code::Unavailable, Code::Unknown] {
            assert!(GrpcRetryPolicy::DEFAULT.is_retryable(c));
            assert_eq!(
                GrpcRetryPolicy::DEFAULT.classify(c),
                Some(Disposition::Retryable)
            );
        }
    }

    #[test]
    fn default_excludes_internal_and_deadline_exceeded() {
        for c in [Code::Internal, Code::DeadlineExceeded] {
            assert!(!GrpcRetryPolicy::DEFAULT.is_retryable(c));
            assert_eq!(
                GrpcRetryPolicy::DEFAULT.classify(c),
                Some(Disposition::Terminal)
            );
        }
    }

    #[test]
    fn default_terminal_for_permanent_codes() {
        for c in [
            Code::NotFound,
            Code::PermissionDenied,
            Code::InvalidArgument,
            Code::AlreadyExists,
            Code::Unauthenticated,
        ] {
            assert_eq!(
                GrpcRetryPolicy::DEFAULT.classify(c),
                Some(Disposition::Terminal)
            );
        }
    }

    #[test]
    fn ok_classifies_as_none() {
        assert_eq!(GrpcRetryPolicy::DEFAULT.classify(Code::Ok), None);
    }

    #[test]
    fn permissive_also_retries_internal_and_deadline() {
        for c in [
            Code::Unavailable,
            Code::Unknown,
            Code::Internal,
            Code::DeadlineExceeded,
        ] {
            assert!(GrpcRetryPolicy::PERMISSIVE.is_retryable(c));
        }
    }

    #[test]
    fn custom_set_is_respected() {
        let policy = GrpcRetryPolicy::new(&[Code::ResourceExhausted]);
        assert!(policy.is_retryable(Code::ResourceExhausted));
        assert!(!policy.is_retryable(Code::Unavailable));
    }

    /// The shape a tonic client synthesizes from a connection failure: the
    /// code tonic picked plus the `tonic::transport::Error` as source. An
    /// invalid URI is the one such error constructible without a runtime.
    fn synthesized(mut status: tonic::Status) -> tonic::Status {
        let transport_error = tonic::transport::Endpoint::from_shared("not a uri")
            .expect_err("an invalid URI is rejected");
        status.set_source(std::sync::Arc::new(transport_error));
        status
    }

    #[test]
    fn client_synthesized_connection_failures_are_retryable() {
        use tonic::Status;
        let policy = GrpcRetryPolicy::DEFAULT;
        // GOAWAY / connection closed arrive as Internal, the channel timeout as Cancelled.
        assert!(policy.is_retryable_status(&synthesized(Status::internal("h2 protocol error"))));
        assert!(policy.is_retryable_status(&synthesized(Status::cancelled("Timeout expired"))));
        // The same codes sent by the server carry no source and stay terminal.
        assert!(!policy.is_retryable_status(&Status::internal("h2 protocol error")));
        assert!(!policy.is_retryable_status(&Status::cancelled("Timeout expired")));
        // Peer verdicts (ENHANCE_YOUR_CALM, INADEQUATE_SECURITY) are not connection failures.
        assert!(!policy.is_retryable_status(&synthesized(Status::resource_exhausted("calm"))));
        assert!(!policy.is_retryable_status(&synthesized(Status::permission_denied("tls"))));
    }
}
