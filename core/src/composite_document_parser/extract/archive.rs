use anyhow::Result;
use std::path::Path;

use crate::document_parser::ParsedDocument;

pub(super) fn parse_archive_document(path: &Path) -> Result<Option<ParsedDocument>> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return super::parse_tgz(path).map(Some);
    }
    if lower.ends_with(".7z") {
        return super::parse_7z(path).map(Some);
    }
    if lower.ends_with(".tar") {
        return super::parse_tar(path).map(Some);
    }
    if lower.ends_with(".gz") {
        return super::parse_gzip(path).map(Some);
    }
    Ok(None)
}
