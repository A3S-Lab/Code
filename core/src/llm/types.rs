//! Public types for the LLM module

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::io::Read;

/// Typed trust label for tool-result content at the value boundary (KRN-5).
///
/// Only [`ToolResultTrustV1::Trusted`] content may occupy an instruction-adjacent
/// position. Non-trusted content is model-visible data. Content that crossed an
/// external boundary ([`ToolResultTrustV1::External`]) requires redaction review
/// before prompt use; middleware fail-closes when that review bit is unset.
/// Workspace-produced data stays loadable without that gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultTrustV1 {
    /// Produced or cryptographically verified by the host runtime itself.
    Trusted,
    /// Produced inside the governed workspace boundary by local tools.
    #[default]
    WorkspaceData,
    /// Crossed an external boundary (web, MCP, download).
    External,
}

impl ToolResultTrustV1 {
    /// Whether this content may occupy an instruction-adjacent position.
    pub const fn may_instruct(self) -> bool {
        matches!(self, Self::Trusted)
    }

    /// Whether redaction/egress review must run before prompt use.
    ///
    /// Matches the model-middleware trust gate: only
    /// [`ToolResultTrustV1::External`] fails closed without
    /// `redaction_reviewed`.
    pub const fn requires_redaction_review(self) -> bool {
        matches!(self, Self::External)
    }
}

/// Maximum bytes loaded by [`Attachment::from_file`].
pub const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;

/// A string wrapper that redacts its value in Debug and Display output.
/// Prevents API keys from leaking into logs and error messages.
#[derive(Clone, Default)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Access the secret value (use sparingly — only for HTTP headers)
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<&String> for SecretString {
    fn from(s: &String) -> Self {
        Self(s.clone())
    }
}

/// Tool definition for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

/// Image attachment for multi-modal messages.
///
/// Supports JPEG, PNG, GIF, and WebP. Data is stored as raw bytes and
/// base64-encoded when serialized for LLM APIs.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Raw image bytes
    pub data: Vec<u8>,
    /// MIME type (e.g., `"image/jpeg"`, `"image/png"`)
    pub media_type: String,
}

impl Attachment {
    /// Create an attachment from raw bytes and media type.
    pub fn new(data: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self {
            data,
            media_type: media_type.into(),
        }
    }

    /// Create a JPEG attachment.
    pub fn jpeg(data: Vec<u8>) -> Self {
        Self::new(data, "image/jpeg")
    }

    /// Create a PNG attachment.
    pub fn png(data: Vec<u8>) -> Self {
        Self::new(data, "image/png")
    }

    /// Create a GIF attachment.
    pub fn gif(data: Vec<u8>) -> Self {
        Self::new(data, "image/gif")
    }

    /// Create a WebP attachment.
    pub fn webp(data: Vec<u8>) -> Self {
        Self::new(data, "image/webp")
    }

    /// Read an image file and auto-detect media type from extension.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let file = std::fs::File::open(path)?;
        if file.metadata()?.len() > MAX_ATTACHMENT_BYTES as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("attachment exceeds the {MAX_ATTACHMENT_BYTES}-byte limit"),
            ));
        }
        // Use the opened handle and a +1 sentinel so concurrent file growth
        // cannot bypass the bound after the metadata check.
        let mut data = Vec::new();
        file.take(MAX_ATTACHMENT_BYTES as u64 + 1)
            .read_to_end(&mut data)?;
        if data.len() > MAX_ATTACHMENT_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("attachment exceeds the {MAX_ATTACHMENT_BYTES}-byte limit"),
            ));
        }
        let media_type = match path.extension().and_then(|e| e.to_str()) {
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("png") => "image/png",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => "application/octet-stream",
        };
        Ok(Self::new(data, media_type))
    }

    /// Return the base64-encoded data.
    pub fn base64_data(&self) -> String {
        BASE64_STANDARD.encode(&self.data)
    }

    /// Convert to a `ContentBlock::Image`.
    pub fn to_content_block(&self) -> ContentBlock {
        ContentBlock::Image {
            source: ImageSource {
                source_type: "base64".to_string(),
                media_type: self.media_type.clone(),
                data: self.base64_data(),
            },
        }
    }
}

/// Image source for the `ContentBlock::Image` variant.
///
/// Matches the Anthropic API format:
/// `{"type": "base64", "media_type": "image/jpeg", "data": "..."}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// Content within a tool result — either text or an image.
///
/// Anthropic's tool_result content supports an array of text/image blocks.
/// This enum models that for multi-modal tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

/// Message content types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: ToolResultContentField,
        is_error: Option<bool>,
        /// Prompt-boundary trust label. Absent legacy payloads default to
        /// [`ToolResultTrustV1::WorkspaceData`]. Provider adapters must not
        /// forward this field on the wire.
        #[serde(default, skip_serializing_if = "tool_result_trust_is_default")]
        trust: ToolResultTrustV1,
        /// Set after governed sanitization (or for host-authored Trusted
        /// results). External results require this before model admission.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redaction_reviewed: bool,
    },
}

fn tool_result_trust_is_default(trust: &ToolResultTrustV1) -> bool {
    *trust == ToolResultTrustV1::WorkspaceData
}

/// The `content` field of a `ToolResult` block.
///
/// Anthropic accepts either a plain string or an array of content blocks
/// (text + image). We use an untagged enum so that plain-string tool results
/// (the common case) serialize as `"content": "..."` and multi-modal results
/// serialize as `"content": [{"type":"text","text":"..."},{"type":"image",...}]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContentField {
    /// Plain text content (backward-compatible default).
    Text(String),
    /// Array of text and/or image blocks (multi-modal tool output).
    Blocks(Vec<ToolResultContent>),
}

impl ToolResultContentField {
    /// Extract the text content as a string reference.
    ///
    /// For `Text`, returns the inner string. For `Blocks`, concatenates
    /// all text blocks.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| {
                    if let ToolResultContent::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl From<String> for ToolResultContentField {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for ToolResultContentField {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl PartialEq<&str> for ToolResultContentField {
    fn eq(&self, other: &&str) -> bool {
        match self {
            Self::Text(s) => s == *other,
            _ => false,
        }
    }
}

impl PartialEq<str> for ToolResultContentField {
    fn eq(&self, other: &str) -> bool {
        match self {
            Self::Text(s) => s == other,
            _ => false,
        }
    }
}

/// Whether a message belongs in the product transcript or only on the model wire.
///
/// Product transcript shows what a human authored (and ordinary assistant/tool
/// chrome). Wire-only messages are runtime steering for the model — plan
/// kickoffs, continuations, compaction summaries, repair prompts — and must not
/// appear as user bubbles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptVisibility {
    /// Included in the product-facing transcript.
    #[default]
    Product,
    /// Model-wire only; omitted from product transcript export.
    Wire,
}

impl TranscriptVisibility {
    pub const fn is_product(self) -> bool {
        matches!(self, Self::Product)
    }

    pub const fn is_wire(self) -> bool {
        matches!(self, Self::Wire)
    }

    /// Serde `skip_serializing_if` callback (receives `&Self`).
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn is_product_ref(visibility: &Self) -> bool {
        visibility.is_product()
    }
}

/// Message in conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
    /// Reasoning/thinking content from models like kimi-k2.5, DeepSeek-R1.
    /// Stored so it can be sent back in conversation history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Optional product-facing text when `content` carries a richer model-wire
    /// prompt (composed context, planner chrome). LLM providers still read
    /// `content`; Desktop/export uses this when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_text: Option<String>,
    /// Defaults to [`TranscriptVisibility::Product`] for backward-compatible
    /// snapshots that predate this field.
    #[serde(default, skip_serializing_if = "TranscriptVisibility::is_product_ref")]
    pub transcript_visibility: TranscriptVisibility,
}

impl Message {
    fn plain(role: impl Into<String>, text: &str) -> Self {
        Self {
            role: role.into(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: TranscriptVisibility::Product,
        }
    }

    /// Product-visible user message (human-authored or ordinary user turn).
    pub fn user(text: &str) -> Self {
        Self::plain("user", text)
    }

    /// Model-wire user steering that must not appear as a product user bubble.
    pub fn user_wire(text: &str) -> Self {
        let mut message = Self::plain("user", text);
        message.transcript_visibility = TranscriptVisibility::Wire;
        message
    }

    /// Persist model-wire `content` while keeping a separate product transcript
    /// string for UI export (composed / planner prompts).
    pub fn user_for_model_with_transcript(model_text: &str, transcript_text: &str) -> Self {
        let model = model_text.trim();
        let transcript = transcript_text.trim();
        let mut message = Self::plain("user", model);
        if !transcript.is_empty() && transcript != model {
            message.transcript_text = Some(transcript.to_string());
        }
        message
    }

    /// A plain-text assistant message. Used e.g. to mark a host-cancelled turn
    /// in committed history so role alternation stays valid for the next turn.
    pub fn assistant(text: &str) -> Self {
        Self::plain("assistant", text)
    }

    /// Model-wire assistant output that must not appear in the product transcript
    /// (e.g. Findings address replies — only Findings status is product-visible).
    pub fn assistant_wire(text: &str) -> Self {
        let mut message = Self::plain("assistant", text);
        message.transcript_visibility = TranscriptVisibility::Wire;
        message
    }

    /// Create a user message with text and image attachments.
    pub fn user_with_attachments(text: &str, attachments: &[Attachment]) -> Self {
        let mut content: Vec<ContentBlock> =
            attachments.iter().map(|a| a.to_content_block()).collect();
        content.push(ContentBlock::Text {
            text: text.to_string(),
        });
        Self {
            role: "user".to_string(),
            content,
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: TranscriptVisibility::Product,
        }
    }

    pub fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self::tool_result_with_trust(
            tool_use_id,
            content,
            is_error,
            ToolResultTrustV1::WorkspaceData,
            false,
        )
    }

    /// Host-authored tool result (guards, control-plane errors).
    pub fn tool_result_trusted(tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self::tool_result_with_trust(
            tool_use_id,
            content,
            is_error,
            ToolResultTrustV1::Trusted,
            true,
        )
    }

    pub fn tool_result_with_trust(
        tool_use_id: &str,
        content: &str,
        is_error: bool,
        trust: ToolResultTrustV1,
        redaction_reviewed: bool,
    ) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: ToolResultContentField::Text(content.to_string()),
                is_error: Some(is_error),
                trust,
                redaction_reviewed,
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: TranscriptVisibility::Product,
        }
    }

    /// Create a tool result message with multi-modal content (text + images).
    pub fn tool_result_with_images(
        tool_use_id: &str,
        text: &str,
        images: &[Attachment],
        is_error: bool,
    ) -> Self {
        Self::tool_result_with_images_and_trust(
            tool_use_id,
            text,
            images,
            is_error,
            ToolResultTrustV1::WorkspaceData,
            false,
        )
    }

    pub fn tool_result_with_images_and_trust(
        tool_use_id: &str,
        text: &str,
        images: &[Attachment],
        is_error: bool,
        trust: ToolResultTrustV1,
        redaction_reviewed: bool,
    ) -> Self {
        let mut blocks: Vec<ToolResultContent> = vec![ToolResultContent::Text {
            text: text.to_string(),
        }];
        for img in images {
            blocks.push(ToolResultContent::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: img.media_type.clone(),
                    data: img.base64_data(),
                },
            });
        }
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: ToolResultContentField::Blocks(blocks),
                is_error: Some(is_error),
                trust,
                redaction_reviewed,
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: TranscriptVisibility::Product,
        }
    }

    pub fn is_product_transcript(&self) -> bool {
        self.transcript_visibility.is_product()
    }

    /// Text the product UI should show for this message.
    pub fn transcript_display_text(&self) -> String {
        if let Some(text) = self
            .transcript_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return text.to_string();
        }
        self.text()
    }

    /// Extract text content from message
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Extract tool calls from message
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    Some(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        args: input.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

/// LLM response
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmResponseMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_object: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub message: Message,
    pub usage: TokenUsage,
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub token_logprobs: Vec<TokenLogProb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<LlmResponseMeta>,
}

impl LlmResponse {
    /// Get text content
    pub fn text(&self) -> String {
        self.message.text()
    }

    /// Get tool calls
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.message.tool_calls()
    }
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub cache_read_tokens: Option<usize>,
    pub cache_write_tokens: Option<usize>,
}

impl TokenUsage {
    pub(crate) fn accumulate(&mut self, usage: &Self) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(usage.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(usage.total_tokens);
        self.cache_read_tokens =
            accumulate_optional_usage(self.cache_read_tokens, usage.cache_read_tokens);
        self.cache_write_tokens =
            accumulate_optional_usage(self.cache_write_tokens, usage.cache_write_tokens);
    }
}

fn accumulate_optional_usage(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (None, None) => None,
        (left, right) => Some(
            left.unwrap_or_default()
                .saturating_add(right.unwrap_or_default()),
        ),
    }
}

/// Token-level log probability emitted by an OpenAI-compatible backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLogProb {
    pub token: String,
    pub logprob: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_logprobs: Vec<TopTokenLogProb>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTokenLogProb {
    pub token: String,
    pub logprob: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
}

/// Tool call from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

/// Streaming event from LLM
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta
    TextDelta(String),
    /// Reasoning/thinking delta (for models like kimi, deepseek that use reasoning_content)
    ReasoningDelta(String),
    /// Tool use started (id, name)
    ToolUseStart { id: String, name: String },
    /// Tool use input delta. `id` is present when the provider exposes the
    /// authoritative call id for interleaved parallel tool arguments.
    ToolUseInputDelta { id: Option<String>, delta: String },
    /// Response complete
    Done(LlmResponse),
}
