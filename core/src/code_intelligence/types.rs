use crate::workspace::WorkspacePath;
use std::fmt;

/// Extensible identifier for a programming language.
///
/// Values use the language identifier understood by the active semantic
/// runtime. This is a newtype rather than an enum so adding language support
/// does not require changing the public contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LanguageId(String);

impl LanguageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for LanguageId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for LanguageId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for LanguageId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for LanguageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Zero-based position in a text document.
///
/// `character` is measured in UTF-16 code units, not Unicode scalar values,
/// grapheme clusters, or UTF-8 bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodePosition {
    pub line: u32,
    pub character: u32,
}

impl CodePosition {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// Half-open range in a text document using zero-based UTF-16 positions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CodeRange {
    pub start: CodePosition,
    pub end: CodePosition,
}

impl CodeRange {
    pub const fn new(start: CodePosition, end: CodePosition) -> Self {
        Self { start, end }
    }
}

/// Workspace-relative location of a semantic result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodeLocation {
    pub path: WorkspacePath,
    pub range: CodeRange,
}

/// Monotonic revision assigned to a saved document by the runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRevision(u64);

impl DocumentRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for DocumentRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Saved-document evidence associated with a query result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSnapshot {
    pub revision: DocumentRevision,
    /// Opaque hash of the saved content observed by the runtime.
    pub content_hash: String,
    /// Whether the saved document changed before the query completed.
    pub stale: bool,
}

/// Bounded result returned by a semantic query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeQueryResult<T> {
    pub items: Vec<T>,
    pub truncated: bool,
    /// Monotonic workspace revision observed by the query.
    pub workspace_revision: u64,
    /// Present for a query anchored to one saved document.
    pub document: Option<DocumentSnapshot>,
}

/// Normalized kind of a code symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodeSymbolKind {
    File,
    Module,
    Namespace,
    Package,
    Class,
    Method,
    Property,
    Field,
    Constructor,
    Enum,
    Interface,
    Function,
    Variable,
    Constant,
    String,
    Number,
    Boolean,
    Array,
    Object,
    Key,
    Null,
    EnumMember,
    Struct,
    Event,
    Operator,
    TypeParameter,
    Unknown,
}

/// Hierarchical symbol returned for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: CodeSymbolKind,
    /// Full syntactic range of the symbol.
    pub range: CodeRange,
    /// Range callers should select when navigating to the symbol.
    pub selection_range: CodeRange,
    pub children: Vec<DocumentSymbol>,
}

/// Symbol returned by a workspace-wide search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolInformation {
    pub name: String,
    pub kind: CodeSymbolKind,
    pub location: CodeLocation,
    pub container_name: Option<String>,
}

/// Supported semantic navigation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavigationKind {
    Definition,
    Declaration,
    References,
    Implementations,
}

/// Severity of a code diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CodeDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// Diagnostic associated with a saved workspace document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDiagnostic {
    pub location: CodeLocation,
    pub severity: Option<CodeDiagnosticSeverity>,
    /// String-normalized diagnostic code when the runtime supplies one.
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

/// Read-only semantic operations currently available from a runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeIntelligenceCapabilities {
    pub document_symbols: bool,
    pub workspace_symbols: bool,
    pub definition: bool,
    pub declaration: bool,
    pub references: bool,
    pub implementations: bool,
    pub diagnostics: bool,
}

/// Lifecycle state of a workspace or language runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeIntelligenceState {
    Starting,
    Ready,
    Degraded,
    Unavailable,
}

/// Status of one configured language runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeIntelligenceLanguageStatus {
    pub language: LanguageId,
    pub state: CodeIntelligenceState,
    pub capabilities: CodeIntelligenceCapabilities,
    pub message: Option<String>,
}

/// Current aggregate status of workspace code intelligence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeIntelligenceStatus {
    pub state: CodeIntelligenceState,
    /// Union of operations available from ready or degraded language runtimes.
    pub capabilities: CodeIntelligenceCapabilities,
    pub languages: Vec<CodeIntelligenceLanguageStatus>,
    pub message: Option<String>,
}

impl Default for CodeIntelligenceStatus {
    fn default() -> Self {
        Self {
            state: CodeIntelligenceState::Unavailable,
            capabilities: CodeIntelligenceCapabilities::default(),
            languages: Vec::new(),
            message: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_value_types_are_send_and_sync() {
        assert_send_sync::<LanguageId>();
        assert_send_sync::<CodePosition>();
        assert_send_sync::<CodeRange>();
        assert_send_sync::<CodeLocation>();
        assert_send_sync::<DocumentRevision>();
        assert_send_sync::<DocumentSnapshot>();
        assert_send_sync::<CodeQueryResult<DocumentSymbol>>();
        assert_send_sync::<SymbolInformation>();
        assert_send_sync::<CodeDiagnostic>();
        assert_send_sync::<CodeIntelligenceStatus>();
    }

    #[test]
    fn position_character_counts_utf16_code_units() {
        let prefix = "a\u{1f980}\u{4e2d}";
        let position = CodePosition::new(0, prefix.encode_utf16().count() as u32);

        assert_eq!(position.character, 4);
    }

    #[test]
    fn language_id_is_open_ended() {
        let language = LanguageId::from("example-language");

        assert_eq!(language.as_str(), "example-language");
        assert_eq!(language.to_string(), "example-language");
    }

    #[test]
    fn default_status_exposes_no_capabilities() {
        let status = CodeIntelligenceStatus::default();

        assert_eq!(status.state, CodeIntelligenceState::Unavailable);
        assert_eq!(status.capabilities, CodeIntelligenceCapabilities::default());
        assert!(status.languages.is_empty());
    }
}
