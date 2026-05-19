//! Typed error surface for the workspace subsystem.
//!
//! Until this module landed, every backend method returned
//! `anyhow::Result<T>` and callers that wanted to react to specific
//! failure kinds had to downcast — fragile, opaque to docs, and
//! non-exhaustive. [`WorkspaceError`] gives the trait surface a typed
//! enum with `#[non_exhaustive]` so callers can `match` known
//! variants while still leaving room for future ones without breaking
//! compatibility.
//!
//! # Migration shape
//!
//! The migration ships in two commits:
//!
//! 1. **7.3.a (this commit):** introduce [`WorkspaceError`] and
//!    [`WorkspaceResult`] alongside the existing `anyhow::Result`
//!    surface. Add `From` conversions in both directions. *No trait
//!    signature changes* — purely additive infrastructure. Existing
//!    callers and backends remain on `anyhow::Result`; the new types
//!    are immediately usable but not yet required.
//!
//! 2. **7.3.b (next commit):** flip every trait method and helper to
//!    return `WorkspaceResult<T>`, update every backend implementation,
//!    every tool, and the SDK transparent paths. That commit is the
//!    breaking change that motivates the v3.0.0 version bump.
//!
//! Splitting it this way lets the type definitions land independently
//! (and be reviewed in isolation) without breaking any existing
//! callsite.
//!
//! # Bridge between `anyhow::Error` and `WorkspaceError`
//!
//! In addition to the auto-generated `From<anyhow::Error>` impl
//! provided by `#[from]` on the `Backend` variant, this module supplies
//! [`WorkspaceError::from_anyhow`] which **preserves the typed variant**
//! when an `anyhow::Error` was originally constructed from a known
//! conflict struct (`WorkspaceVersionConflict`, `RemoteGitConflict`).
//! The plain `Into::into` path drops the type information into the
//! `Backend(_)` variant because `anyhow::Error` erases the source type
//! at the value level.

use super::{RemoteGitConflict, WorkspaceVersionConflict};
use std::time::Duration;

/// Error type returned by every [`WorkspaceFileSystem`](super::WorkspaceFileSystem)
/// and friend trait method.
///
/// `#[non_exhaustive]` so adding a new variant in a future release is a
/// minor change — existing `match` callers compile, they just hit the
/// catch-all arm for unknown variants.
///
/// The variants intentionally split into three categories:
///
/// * **Structured failures** the trait surface knows how to describe
///   (`NotFound`, `InvalidArgument`, `Timeout`, `Unsupported`). New
///   variants in this category should also be structured.
/// * **Typed conflicts** with their own payload structs that already
///   ship as part of the public API
///   (`VersionConflict(WorkspaceVersionConflict)`,
///   `RemoteGitConflict(RemoteGitConflict)`).
/// * **`Backend(anyhow::Error)`** — the escape hatch. Any failure not
///   covered above wraps an `anyhow::Error`. Backends should prefer the
///   typed variants where they apply; `Backend` is for genuinely
///   opaque or backend-specific failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceError {
    /// A read / list against a path that does not exist on the backend.
    ///
    /// Backends that distinguish "doesn't exist" from "exists but
    /// access denied" should still emit this for the former; the
    /// latter belongs in `Backend(_)` with the backend's native auth
    /// error wrapped.
    #[error("path not found: {path}")]
    NotFound {
        /// Path that triggered the failure, in workspace-relative form
        /// where possible. May include backend-specific qualifiers
        /// (`s3://bucket/key`) when that aids debugging.
        path: String,
    },

    /// Compare-and-swap write rejected because the in-storage version
    /// no longer matches what the caller observed at read time. Carries
    /// the existing public [`WorkspaceVersionConflict`] struct verbatim.
    #[error(transparent)]
    VersionConflict(#[from] WorkspaceVersionConflict),

    /// A remote git server returned 409 / 422 with a typed conflict
    /// code (e.g. `BRANCH_EXISTS`, `WORKING_TREE_DIRTY`,
    /// `NOTHING_TO_STASH`). Carries the existing public
    /// [`RemoteGitConflict`] struct verbatim.
    #[error(transparent)]
    RemoteGitConflict(#[from] RemoteGitConflict),

    /// Caller passed an argument the backend cannot honour
    /// (empty version on a CAS write, malformed pattern on a search,
    /// path with parent-traversal, ...). Backends should prefer this
    /// over `Backend(_)` for caller-fault errors so the model can
    /// reason about retry strategy.
    #[error("invalid argument: {message}")]
    InvalidArgument {
        /// Human-readable description; safe to surface to the model.
        message: String,
    },

    /// The operation's outer timeout (see
    /// [`WorkspaceServices::operation_timeout`](super::WorkspaceServices::operation_timeout))
    /// fired before the backend responded.
    #[error("workspace operation '{op}' timed out after {duration:?}")]
    Timeout {
        /// Human-readable operation name, e.g. `read_text` or `s3.get_object`.
        op: String,
        /// Configured timeout that expired.
        duration: Duration,
    },

    /// The backend explicitly does not support this operation.
    ///
    /// Used by adapters that wrap a partial trait surface (e.g. the
    /// remote git backend rejecting worktree operations even though
    /// `WorkspaceGit` is implemented).
    #[error("not supported by this backend: {0}")]
    Unsupported(String),

    /// Catch-all wrapping a lower-level error that does not map to one
    /// of the typed variants above. This is the bridge between the
    /// existing `anyhow::Result` world and the typed surface — when a
    /// backend throws a generic I/O / HTTP / SDK error it ends up here.
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl WorkspaceError {
    /// Convert an `anyhow::Error` to a `WorkspaceError`, **preserving
    /// the typed variant** when the original cause was one of the
    /// known conflict structs.
    ///
    /// `Into::into` (auto-derived from the `#[from] anyhow::Error`
    /// variant) drops every `anyhow::Error` into the `Backend` arm
    /// because at the value level `anyhow::Error` has type-erased its
    /// source. Use this helper instead when migrating code paths that
    /// today emit `anyhow::Error::new(WorkspaceVersionConflict { .. })`
    /// or `anyhow::Error::new(RemoteGitConflict { .. })` — the typed
    /// variant survives the round-trip.
    ///
    /// ```ignore
    /// // Old code:
    /// fn legacy() -> anyhow::Result<()> { ... }
    /// // New caller:
    /// let typed = WorkspaceError::from_anyhow(legacy().unwrap_err());
    /// match typed {
    ///     WorkspaceError::VersionConflict(v) => retry(v),
    ///     other => return Err(other),
    /// }
    /// ```
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        if let Some(conflict) = err.downcast_ref::<WorkspaceVersionConflict>() {
            return Self::VersionConflict(conflict.clone());
        }
        if let Some(conflict) = err.downcast_ref::<RemoteGitConflict>() {
            return Self::RemoteGitConflict(conflict.clone());
        }
        Self::Backend(err)
    }
}

/// Result alias used throughout the workspace trait surface in v3.0+.
///
/// In v2.x this co-exists with [`anyhow::Result`] (the legacy return
/// type of every trait method); in v3.0 the trait surface will return
/// `WorkspaceResult<T>` directly. See the module docs for the
/// two-commit migration plan.
pub type WorkspaceResult<T> = std::result::Result<T, WorkspaceError>;

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn anyhow_with_version_conflict_round_trips_through_from_anyhow() {
        let conflict = WorkspaceVersionConflict {
            path: "doc.md".to_string(),
            expected: "etag-1".to_string(),
            actual: Some("etag-2".to_string()),
        };
        let err: anyhow::Error = anyhow::Error::new(conflict.clone());

        let typed = WorkspaceError::from_anyhow(err);
        match typed {
            WorkspaceError::VersionConflict(v) => {
                assert_eq!(v.path, "doc.md");
                assert_eq!(v.expected, "etag-1");
                assert_eq!(v.actual.as_deref(), Some("etag-2"));
            }
            other => panic!("expected VersionConflict, got {other:?}"),
        }
    }

    #[test]
    fn anyhow_with_remote_git_conflict_round_trips_through_from_anyhow() {
        let conflict = RemoteGitConflict {
            code: "BRANCH_EXISTS".to_string(),
            message: "branch 'feat/x' already exists".to_string(),
        };
        let err: anyhow::Error = anyhow::Error::new(conflict);

        let typed = WorkspaceError::from_anyhow(err);
        match typed {
            WorkspaceError::RemoteGitConflict(c) => {
                assert_eq!(c.code, "BRANCH_EXISTS");
                assert!(c.message.contains("feat/x"));
            }
            other => panic!("expected RemoteGitConflict, got {other:?}"),
        }
    }

    #[test]
    fn anyhow_without_known_type_falls_into_backend_variant() {
        let err: anyhow::Error = anyhow!("some I/O thing exploded");
        let typed = WorkspaceError::from_anyhow(err);
        match typed {
            WorkspaceError::Backend(e) => {
                assert!(e.to_string().contains("I/O thing exploded"));
            }
            other => panic!("expected Backend, got {other:?}"),
        }
    }

    #[test]
    fn workspace_error_converts_back_to_anyhow_via_blanket_impl() {
        // anyhow's blanket `From<E: Error + Send + Sync + 'static>` impl
        // means `?` on a `WorkspaceResult` inside an `anyhow::Result`
        // function lifts cleanly. This is the only thing keeping
        // existing `anyhow::Result`-returning callers compatible during
        // the Phase 7.3.b migration.
        fn produce() -> WorkspaceResult<()> {
            Err(WorkspaceError::NotFound {
                path: "missing.txt".into(),
            })
        }
        fn consumes_anyhow() -> anyhow::Result<()> {
            produce()?;
            Ok(())
        }
        let err = consumes_anyhow().unwrap_err();
        assert!(err.to_string().contains("missing.txt"));
        // The original typed value is still recoverable via downcast.
        assert!(err.downcast_ref::<WorkspaceError>().is_some());
    }

    #[test]
    fn version_conflict_struct_converts_via_from() {
        // The auto-derived `#[from] WorkspaceVersionConflict` impl lets
        // backends build a `WorkspaceError` directly from the existing
        // conflict struct without going through anyhow first.
        let conflict = WorkspaceVersionConflict {
            path: "x.txt".into(),
            expected: "v1".into(),
            actual: None,
        };
        let err: WorkspaceError = conflict.into();
        matches!(err, WorkspaceError::VersionConflict(_))
            .then_some(())
            .expect("From<WorkspaceVersionConflict> must produce VersionConflict variant");
    }

    #[test]
    fn invalid_argument_variant_carries_message_in_display() {
        let err = WorkspaceError::InvalidArgument {
            message: "expected_version must not be empty".into(),
        };
        let s = err.to_string();
        assert!(s.contains("invalid argument"), "got: {s}");
        assert!(s.contains("expected_version"), "got: {s}");
    }

    #[test]
    fn timeout_variant_carries_op_and_duration_in_display() {
        let err = WorkspaceError::Timeout {
            op: "read_text".into(),
            duration: Duration::from_secs(30),
        };
        let s = err.to_string();
        assert!(s.contains("read_text"), "got: {s}");
        assert!(s.contains("30"), "got: {s}");
    }

    #[test]
    fn unsupported_variant_names_the_operation() {
        let err = WorkspaceError::Unsupported("worktree on remote git".into());
        let s = err.to_string();
        assert!(s.contains("not supported"), "got: {s}");
        assert!(s.contains("worktree"), "got: {s}");
    }
}
