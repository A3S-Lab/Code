use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{CapabilitySetError, Sha256Digest};

pub const UI_DOCUMENT_SCHEMA: &str = "a3s.code.ui-document.v1";
pub const UI_BINDING_SCHEMA: &str = "a3s.code.ui-binding.v1";
pub const MAX_UI_ASSET_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_UI_ASSETS_PER_KIND: usize = 16;
pub const MAX_UI_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

const MAX_UI_PUBLIC_NAME_BYTES: usize = 256;
const MAX_UI_TITLE_BYTES: usize = 256;
const MAX_UI_DESCRIPTION_BYTES: usize = 1_024;
const MAX_UI_ICON_BYTES: usize = 64;
const UI_DIGEST_PREFIX: &[u8] = b"a3s-code-ui\0";

/// Closed static asset roles accepted by the renderer-neutral UI contract.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiAssetKind {
    Html,
    Style,
    Script,
}

impl UiAssetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Style => "style",
            Self::Script => "script",
        }
    }

    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Html => "text/html",
            Self::Style => "text/css",
            Self::Script => "text/javascript",
        }
    }
}

impl fmt::Display for UiAssetKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A bounded, path-free UTF-8 asset with a content-derived identity.
#[derive(Clone, Eq, PartialEq)]
pub struct UiAsset {
    kind: UiAssetKind,
    content: Arc<str>,
    digest: Sha256Digest,
}

impl UiAsset {
    /// Copy bounded UTF-8 content into an immutable asset and derive its digest.
    pub fn new(kind: UiAssetKind, content: impl AsRef<str>) -> Result<Self, UiBindingError> {
        Self::build(kind, content, None)
    }

    /// Construct an asset only when the supplied bytes match reviewed evidence.
    pub fn new_verified(
        kind: UiAssetKind,
        content: impl AsRef<str>,
        expected_digest: Sha256Digest,
    ) -> Result<Self, UiBindingError> {
        Self::build(kind, content, Some(expected_digest))
    }

    fn build(
        kind: UiAssetKind,
        content: impl AsRef<str>,
        expected_digest: Option<Sha256Digest>,
    ) -> Result<Self, UiBindingError> {
        let content = content.as_ref();
        if content.is_empty() {
            return Err(UiBindingError::EmptyAsset { kind });
        }
        if content.len() > MAX_UI_ASSET_BYTES {
            return Err(UiBindingError::AssetTooLarge {
                kind,
                max: MAX_UI_ASSET_BYTES,
            });
        }
        let digest = digest_bytes(content.as_bytes())?;
        if let Some(expected) = expected_digest {
            if expected != digest {
                return Err(UiBindingError::AssetDigestMismatch {
                    kind,
                    expected: expected.to_string(),
                    actual: digest.to_string(),
                });
            }
        }
        Ok(Self {
            kind,
            content: Arc::from(content),
            digest,
        })
    }

    pub const fn kind(&self) -> UiAssetKind {
        self.kind
    }

    pub const fn media_type(&self) -> &'static str {
        self.kind.media_type()
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub fn len(&self) -> usize {
        self.content.len()
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

impl fmt::Debug for UiAsset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiAsset")
            .field("kind", &self.kind)
            .field("bytes", &self.len())
            .field("digest", &self.digest)
            .finish()
    }
}

/// Exact path-free document bytes selected for one UI generation.
#[derive(Clone, Eq, PartialEq)]
pub struct UiDocument {
    entry: UiAsset,
    styles: Vec<UiAsset>,
    scripts: Vec<UiAsset>,
    total_bytes: usize,
    digest: Sha256Digest,
}

impl UiDocument {
    pub fn new(
        entry: UiAsset,
        styles: impl IntoIterator<Item = UiAsset>,
        scripts: impl IntoIterator<Item = UiAsset>,
    ) -> Result<Self, UiBindingError> {
        ensure_asset_kind("entry", &entry, UiAssetKind::Html)?;
        let styles = styles.into_iter().collect::<Vec<_>>();
        let scripts = scripts.into_iter().collect::<Vec<_>>();
        if styles.len() > MAX_UI_ASSETS_PER_KIND {
            return Err(UiBindingError::AssetCountExceeded {
                kind: UiAssetKind::Style,
                max: MAX_UI_ASSETS_PER_KIND,
            });
        }
        if scripts.len() > MAX_UI_ASSETS_PER_KIND {
            return Err(UiBindingError::AssetCountExceeded {
                kind: UiAssetKind::Script,
                max: MAX_UI_ASSETS_PER_KIND,
            });
        }
        for style in &styles {
            ensure_asset_kind("styles", style, UiAssetKind::Style)?;
        }
        for script in &scripts {
            ensure_asset_kind("scripts", script, UiAssetKind::Script)?;
        }
        let total_bytes = styles
            .iter()
            .chain(&scripts)
            .try_fold(entry.len(), |total, asset| total.checked_add(asset.len()))
            .ok_or(UiBindingError::DocumentTooLarge {
                max: MAX_UI_DOCUMENT_BYTES,
            })?;
        if total_bytes > MAX_UI_DOCUMENT_BYTES {
            return Err(UiBindingError::DocumentTooLarge {
                max: MAX_UI_DOCUMENT_BYTES,
            });
        }
        let digest = document_digest(&entry, &styles, &scripts)?;
        Ok(Self {
            entry,
            styles,
            scripts,
            total_bytes,
            digest,
        })
    }

    pub fn entry(&self) -> &UiAsset {
        &self.entry
    }

    pub fn styles(&self) -> &[UiAsset] {
        &self.styles
    }

    pub fn scripts(&self) -> &[UiAsset] {
        &self.scripts
    }

    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

impl fmt::Debug for UiDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiDocument")
            .field("total_bytes", &self.total_bytes)
            .field("styles", &self.styles.len())
            .field("scripts", &self.scripts.len())
            .field("digest", &self.digest)
            .finish_non_exhaustive()
    }
}

/// Renderer-neutral metadata and exact static document selected by a host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiBindingSpec {
    pub public_name: String,
    pub title: String,
    pub description: String,
    pub icon: String,
    pub order: i32,
    pub document: UiDocument,
}

/// Immutable UI value accepted by the Code capability projection kernel.
///
/// The binding deliberately contains no filesystem path, URL, renderer,
/// credential, state store, or ambient backend authority. Its descriptor owns
/// exact Tool, Skill, MCP, and Flow dependency edges; the embedding host owns
/// sandbox, origin, CSP, navigation, state, and message-protocol policy.
#[derive(Clone, Eq, PartialEq)]
pub struct UiBinding {
    public_name: Box<str>,
    title: Box<str>,
    description: Box<str>,
    icon: Box<str>,
    order: i32,
    document: UiDocument,
    surface_digest: Sha256Digest,
}

impl UiBinding {
    pub fn new(spec: UiBindingSpec) -> Result<Self, UiBindingError> {
        validate_required_text("public_name", &spec.public_name, MAX_UI_PUBLIC_NAME_BYTES)?;
        validate_required_text("title", &spec.title, MAX_UI_TITLE_BYTES)?;
        validate_optional_text("description", &spec.description, MAX_UI_DESCRIPTION_BYTES)?;
        validate_icon(&spec.icon)?;
        let surface_digest = binding_digest(&spec)?;
        Ok(Self {
            public_name: spec.public_name.into_boxed_str(),
            title: spec.title.into_boxed_str(),
            description: spec.description.into_boxed_str(),
            icon: spec.icon.into_boxed_str(),
            order: spec.order,
            document: spec.document,
            surface_digest,
        })
    }

    pub fn public_name(&self) -> &str {
        &self.public_name
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn icon(&self) -> &str {
        &self.icon
    }

    pub const fn order(&self) -> i32 {
        self.order
    }

    pub fn document(&self) -> &UiDocument {
        &self.document
    }

    pub fn surface_digest(&self) -> &Sha256Digest {
        &self.surface_digest
    }
}

impl fmt::Debug for UiBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UiBinding")
            .field("public_name", &self.public_name)
            .field("title", &self.title)
            .field("icon", &self.icon)
            .field("order", &self.order)
            .field("document", &self.document)
            .field("surface_digest", &self.surface_digest)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum UiBindingError {
    #[error("UI field '{field}' is invalid: {reason}")]
    InvalidText {
        field: &'static str,
        reason: &'static str,
    },
    #[error("UI field '{field}' exceeds its byte bound of {max}")]
    TextTooLarge { field: &'static str, max: usize },
    #[error("UI icon must be a bounded lowercase icon identifier")]
    InvalidIcon,
    #[error("UI {kind} asset is empty")]
    EmptyAsset { kind: UiAssetKind },
    #[error("UI {kind} asset exceeds its byte bound of {max}")]
    AssetTooLarge { kind: UiAssetKind, max: usize },
    #[error(
        "UI {kind} asset digest does not match reviewed evidence (expected {expected}, found {actual})"
    )]
    AssetDigestMismatch {
        kind: UiAssetKind,
        expected: String,
        actual: String,
    },
    #[error("UI document field '{field}' requires {expected}, found {actual}")]
    AssetKindMismatch {
        field: &'static str,
        expected: UiAssetKind,
        actual: UiAssetKind,
    },
    #[error("UI document contains more than {max} {kind} assets")]
    AssetCountExceeded { kind: UiAssetKind, max: usize },
    #[error("UI document exceeds its aggregate byte bound of {max}")]
    DocumentTooLarge { max: usize },
    #[error("UI digest construction violated the canonical SHA-256 invariant")]
    DigestInvariant,
}

fn ensure_asset_kind(
    field: &'static str,
    asset: &UiAsset,
    expected: UiAssetKind,
) -> Result<(), UiBindingError> {
    if asset.kind() == expected {
        return Ok(());
    }
    Err(UiBindingError::AssetKindMismatch {
        field,
        expected,
        actual: asset.kind(),
    })
}

fn validate_required_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), UiBindingError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(UiBindingError::InvalidText {
            field,
            reason: "it is empty, padded, or contains control characters",
        });
    }
    if value.len() > max {
        return Err(UiBindingError::TextTooLarge { field, max });
    }
    Ok(())
}

fn validate_optional_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), UiBindingError> {
    if value.len() > max {
        return Err(UiBindingError::TextTooLarge { field, max });
    }
    if !value.is_empty() && (value.trim() != value || value.chars().any(char::is_control)) {
        return Err(UiBindingError::InvalidText {
            field,
            reason: "it is padded or contains control characters",
        });
    }
    Ok(())
}

fn validate_icon(value: &str) -> Result<(), UiBindingError> {
    let valid = !value.is_empty()
        && value.len() <= MAX_UI_ICON_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric);
    if valid {
        Ok(())
    } else {
        Err(UiBindingError::InvalidIcon)
    }
}

fn document_digest(
    entry: &UiAsset,
    styles: &[UiAsset],
    scripts: &[UiAsset],
) -> Result<Sha256Digest, UiBindingError> {
    let mut digest = UiDigest::new(UI_DOCUMENT_SCHEMA);
    digest.asset(entry);
    digest.count(styles.len());
    for style in styles {
        digest.asset(style);
    }
    digest.count(scripts.len());
    for script in scripts {
        digest.asset(script);
    }
    digest.finish()
}

fn binding_digest(spec: &UiBindingSpec) -> Result<Sha256Digest, UiBindingError> {
    let mut digest = UiDigest::new(UI_BINDING_SCHEMA);
    digest.field(spec.public_name.as_bytes());
    digest.field(spec.title.as_bytes());
    digest.field(spec.description.as_bytes());
    digest.field(spec.icon.as_bytes());
    digest.field(&spec.order.to_be_bytes());
    digest.field(spec.document.digest().as_str().as_bytes());
    digest.finish()
}

fn digest_bytes(value: &[u8]) -> Result<Sha256Digest, UiBindingError> {
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(value))).map_err(map_digest_error)
}

fn map_digest_error(_error: CapabilitySetError) -> UiBindingError {
    UiBindingError::DigestInvariant
}

struct UiDigest(Sha256);

impl UiDigest {
    fn new(domain: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(UI_DIGEST_PREFIX);
        hash_field(&mut hasher, domain.as_bytes());
        Self(hasher)
    }

    fn field(&mut self, value: &[u8]) {
        hash_field(&mut self.0, value);
    }

    fn count(&mut self, value: usize) {
        self.field(&(value as u64).to_be_bytes());
    }

    fn asset(&mut self, asset: &UiAsset) {
        self.field(asset.kind().as_str().as_bytes());
        self.field(asset.digest().as_str().as_bytes());
    }

    fn finish(self) -> Result<Sha256Digest, UiBindingError> {
        Sha256Digest::new(format!("sha256:{:x}", self.0.finalize())).map_err(map_digest_error)
    }
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_content_and_surface_digests_have_a_stable_golden_identity() {
        let document = UiDocument::new(
            UiAsset::new(UiAssetKind::Html, "<!doctype html><main>exact</main>").unwrap(),
            [UiAsset::new(UiAssetKind::Style, "main { display: grid; }").unwrap()],
            [UiAsset::new(UiAssetKind::Script, "globalThis.ready = true;").unwrap()],
        )
        .unwrap();
        assert_eq!(
            document.entry().digest().as_str(),
            "sha256:8a9058b3f3c8402c616024c84410de04b23f7a9280552c3b1c05b9fd386fe763"
        );
        assert_eq!(
            document.digest().as_str(),
            "sha256:4286fdb205409b4cd204cefdd7e1e8fe0d45a4a269193ff656a976a2353a0114"
        );
        let binding = UiBinding::new(UiBindingSpec {
            public_name: "panel".to_owned(),
            title: "Evidence".to_owned(),
            description: "Exact.".to_owned(),
            icon: "panel-top".to_owned(),
            order: 20,
            document,
        })
        .unwrap();
        assert_eq!(
            binding.surface_digest().as_str(),
            "sha256:c78cab3d09b64058f352bb84a10f749707b2cd84d1acfb780e77bee6fe3cae27"
        );
    }

    #[test]
    fn ui_assets_and_documents_enforce_role_and_memory_bounds() {
        assert!(matches!(
            UiAsset::new(UiAssetKind::Script, ""),
            Err(UiBindingError::EmptyAsset {
                kind: UiAssetKind::Script
            })
        ));
        assert!(matches!(
            UiAsset::new(UiAssetKind::Style, "x".repeat(MAX_UI_ASSET_BYTES + 1)),
            Err(UiBindingError::AssetTooLarge {
                kind: UiAssetKind::Style,
                max: MAX_UI_ASSET_BYTES
            })
        ));
        let wrong_entry = UiAsset::new(UiAssetKind::Script, "globalThis.ready = true;").unwrap();
        assert!(matches!(
            UiDocument::new(wrong_entry, [], []),
            Err(UiBindingError::AssetKindMismatch {
                field: "entry",
                expected: UiAssetKind::Html,
                actual: UiAssetKind::Script
            })
        ));
    }

    #[test]
    fn ui_debug_output_never_embeds_executable_content() {
        let secret_marker = "DO_NOT_EMBED_UI_SOURCE_IN_DEBUG";
        let asset = UiAsset::new(UiAssetKind::Script, secret_marker).unwrap();
        assert!(!format!("{asset:?}").contains(secret_marker));
    }
}
