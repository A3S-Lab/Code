//! Value-only attribution for one tool invocation.
//!
//! Holds no emitter, session owner, or live span. A direct-user or system call
//! leaves the model absent unless the caller supplies a model from a known invocation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationSource {
    Model,
    UserDirect,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallOrigin {
    invocation_id: String,
    session_id: Option<String>,
    turn: Option<i64>,
    requested_model: Option<String>,
    tool_id: String,
    tool_version: Option<String>,
    source: InvocationSource,
}

impl ToolCallOrigin {
    pub fn model_call(
        invocation_id: impl Into<String>,
        session_id: impl Into<String>,
        turn: Option<i64>,
        requested_model: Option<String>,
        tool_id: impl Into<String>,
        tool_version: Option<String>,
    ) -> Self {
        Self::new(
            InvocationSource::Model,
            invocation_id,
            Some(session_id.into()),
            turn,
            requested_model,
            tool_id,
            tool_version,
        )
    }

    /// `requested_model` is set only when this call really came from a known model invocation.
    pub fn direct(
        source: InvocationSource,
        invocation_id: impl Into<String>,
        session_id: Option<String>,
        turn: Option<i64>,
        requested_model: Option<String>,
        tool_id: impl Into<String>,
        tool_version: Option<String>,
    ) -> Self {
        debug_assert!(
            !matches!(source, InvocationSource::Model),
            "model calls use model_call"
        );
        Self::new(
            source,
            invocation_id,
            session_id,
            turn,
            requested_model,
            tool_id,
            tool_version,
        )
    }

    pub fn with_known_model(mut self, model: impl Into<String>) -> Self {
        let model = model.into();
        if !model.is_empty() {
            self.requested_model = Some(model);
        }
        self
    }

    pub fn source(&self) -> InvocationSource {
        self.source
    }

    pub fn requested_model(&self) -> Option<&str> {
        self.requested_model.as_deref()
    }

    fn new(
        source: InvocationSource,
        invocation_id: impl Into<String>,
        session_id: Option<String>,
        turn: Option<i64>,
        requested_model: Option<String>,
        tool_id: impl Into<String>,
        tool_version: Option<String>,
    ) -> Self {
        Self {
            invocation_id: invocation_id.into(),
            session_id: session_id.filter(|id| !id.is_empty()),
            turn,
            requested_model: requested_model.filter(|model| !model.is_empty()),
            tool_id: tool_id.into(),
            tool_version: tool_version.filter(|version| !version.is_empty()),
            source,
        }
    }
}
