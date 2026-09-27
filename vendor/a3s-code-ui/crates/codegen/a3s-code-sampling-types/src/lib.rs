//! Pure data types for the A3S sampling / chat-completion API layer.
//!
//! API-agnostic conversation, chat-completion request/response, streaming, and error types used across the A3S agent stack.
//! It contains no I/O: no HTTP clients, no file system access.
//! Downstream crates like `a3s-chat-state` can depend on it without pulling in the full `a3s-code-shell`.

#![deny(clippy::indexing_slicing)]

pub mod conversation;
pub mod doom_loop;
pub mod error;
pub mod messages;
pub mod provider_error;
pub mod serde_helpers;
pub mod tool_overrides;
pub mod types;

pub use self::conversation::*;
pub use self::doom_loop::{
    DEFAULT_EXACT_REPETITION_MIN_TOKENS, DOOM_LOOP_CHECK_EVENT_TYPE, DOOM_LOOP_CHECK_HEADER,
    DoomLoopPeek, DoomLoopRecoveryPolicy, DoomLoopSignal, DoomLoopSignalKind,
    EXACT_REPETITION_CHECK_HEADER, is_check_event, peek_doom_loop,
};
pub use self::error::{
    ApiErrorCode, EmptyReason, EmptyResponseContext, INVALID_IMAGE_ERROR_CODE,
    ResponseModelMetadata, Result, SamplingError, SentCredential, is_context_length_error,
    is_retryable_api_status, is_size_overflow_error_code, parse_error_code, status_user_message,
    user_facing_api_error_message,
};
pub use self::tool_overrides::{
    ClearableField, MAX_WEB_SEARCH_DOMAINS, SearchDateBound, SearchDateBoundError, ToolOverrides,
    ToolOverridesUpdate, WebSearchOptions, WebSearchOptionsError, XSearchOptions,
};
pub use self::types::*;

pub use async_openai::types::responses as rs;
