use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use roxmltree::Document;
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use std::process::Command;
use tar::Archive;
use zip::ZipArchive;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

use super::email::{parse_eml_string, parse_mbox_string, strip_emlx_wrapper};

const MAX_NESTED_ARCHIVE_DEPTH: usize = 3;

#[derive(Debug, Default, Clone)]
pub(super) struct ContainerMetadata {
    pub title: Option<String>,
    pub subject: Option<String>,
    pub creator: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
}

impl ContainerMetadata {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.subject.is_none()
            && self.creator.is_none()
            && self.description.is_none()
            && self.language.is_none()
    }

    pub(super) fn as_block_content(&self) -> String {
        let mut lines = Vec::new();
        if let Some(title) = &self.title {
            lines.push(format!("title={title}"));
        }
        if let Some(subject) = &self.subject {
            lines.push(format!("subject={subject}"));
        }
        if let Some(creator) = &self.creator {
            lines.push(format!("creator={creator}"));
        }
        if let Some(description) = &self.description {
            lines.push(format!("description={description}"));
        }
        if let Some(language) = &self.language {
            lines.push(format!("language={language}"));
        }
        lines.join("\n")
    }
}

pub(super) fn parse_epub(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let mut inferred_title = read_epub_title(&mut zip, &names);

    for name in names {
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")) {
            continue;
        }

        let content = read_zip_entry(&mut zip, &name)?;
        if inferred_title.is_none() {
            inferred_title = super::extract_markup_title(&content);
        }
        let section_doc = super::parse_markup_string(&content, true).unwrap_or_else(|| {
            super::fallback_text_blocks(&super::render_html_to_text(&content).unwrap_or_default())
        });
        if section_doc.is_empty() {
            continue;
        }

        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some(name.clone()),
                format!("source: {}", name),
            )
            .with_source(name.clone()),
        );
        for (idx, block) in section_doc.into_iter().enumerate() {
            let label = block
                .label
                .as_ref()
                .map(|label| format!("{}: {}", name, label))
                .or_else(|| Some(name.clone()));
            doc.push(
                DocumentBlock::new(block.kind, label, block.content)
                    .with_source(name.clone())
                    .with_ordinal(idx + 1),
            );
        }
    }

    if inferred_title.is_some() {
        doc.title = inferred_title;
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_zip(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut entries = Vec::new();
    for name in names {
        let lower = name.to_ascii_lowercase();
        if !is_archive_entry_candidate(&lower) {
            continue;
        }
        let bytes = match read_zip_entry_bytes(&mut zip, &name) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        entries.push((name, bytes));
    }

    parse_archive_entries(path, "zip", zip.len(), entries)
}

pub(super) fn parse_iwork_package(path: &Path, package_type: &str) -> Result<ParsedDocument> {
    let mut doc = parse_zip(path)?;
    doc.blocks.insert(
        0,
        DocumentBlock::new(
            DocumentBlockKind::Metadata,
            Some("iwork"),
            format!("format={package_type}"),
        )
        .with_source(package_type)
        .with_ordinal(0),
    );
    Ok(doc)
}

pub(super) fn parse_tar(path: &Path) -> Result<ParsedDocument> {
    let file = File::open(path)
        .with_context(|| format!("failed to open tar archive {}", path.display()))?;
    parse_tar_reader(file, path)
}

pub(super) fn parse_tgz(path: &Path) -> Result<ParsedDocument> {
    let file = File::open(path)
        .with_context(|| format!("failed to open gzip archive {}", path.display()))?;
    let decoder = GzDecoder::new(file);
    parse_tar_reader(decoder, path)
}

pub(super) fn parse_gzip(path: &Path) -> Result<ParsedDocument> {
    let file = File::open(path)
        .with_context(|| format!("failed to open gzip archive {}", path.display()))?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to decompress gzip archive {}", path.display()))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("archive.gz");
    let inner_name = file_name
        .strip_suffix(".gz")
        .unwrap_or(file_name)
        .to_string();

    if inner_name.to_ascii_lowercase().ends_with(".tar") {
        return parse_tar_bytes(&inner_name, bytes);
    }

    parse_archive_entries(path, "gzip", 1, vec![(inner_name, bytes)])
}

pub(super) fn parse_7z(path: &Path) -> Result<ParsedDocument> {
    let Some(binary) = find_7z_binary() else {
        anyhow::bail!(
            "failed to parse 7z archive {}: no 7z/7zz/7za binary found on PATH",
            path.display()
        );
    };

    let entries = list_7z_entries(&binary, path)?;
    let entry_count = entries.len();
    let mut extracted = Vec::new();
    for name in entries {
        let lower = name.to_ascii_lowercase();
        if !is_archive_entry_candidate(&lower) {
            continue;
        }
        let bytes = extract_7z_entry(&binary, path, &name)?;
        extracted.push((name, bytes));
    }

    parse_archive_entries(path, "7z", entry_count, extracted)
}

pub(super) fn read_docx_core_metadata(
    zip: &mut ZipArchive<File>,
) -> Result<Option<ContainerMetadata>> {
    let content = match read_zip_entry(zip, "docProps/core.xml") {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };
    parse_container_metadata_xml(&content)
}

pub(super) fn read_odf_metadata(zip: &mut ZipArchive<File>) -> Result<Option<ContainerMetadata>> {
    let content = match read_zip_entry(zip, "meta.xml") {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };
    parse_container_metadata_xml(&content)
}

pub(super) fn open_zip(path: &Path) -> Result<ZipArchive<File>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open zip container {}", path.display()))?;
    ZipArchive::new(file)
        .with_context(|| format!("failed to read zip container {}", path.display()))
}

pub(super) fn read_zip_entry(zip: &mut ZipArchive<File>, name: &str) -> Result<String> {
    let bytes = read_zip_entry_bytes(zip, name)?;
    decode_text_bytes(&bytes).with_context(|| format!("failed to decode zip entry as text: {name}"))
}

pub(super) fn read_zip_entry_bytes<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut file = zip
        .by_name(name)
        .with_context(|| format!("zip entry not found: {name}"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .with_context(|| format!("failed to read zip entry: {name}"))?;
    Ok(buf)
}

fn parse_zip_entry_blocks(name: &str, bytes: &[u8], depth: usize) -> Vec<DocumentBlock> {
    if depth < MAX_NESTED_ARCHIVE_DEPTH {
        if let Some(document) = parse_nested_archive_entry(name, bytes, depth + 1) {
            return flatten_archive_entry_document(name, document);
        }
    }

    if let Some(document) = super::parse_archive_office_entry(name, bytes) {
        return flatten_archive_entry_document(name, document);
    }

    let Ok(content) = decode_text_bytes(bytes) else {
        return Vec::new();
    };

    if let Some(document) = parse_structured_archive_entry(name, &content) {
        return flatten_archive_entry_document(name, document);
    }

    let ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" => super::parse_delimited_blocks(&content, ','),
        "tsv" => super::parse_delimited_blocks(&content, '\t'),
        "jsonl" | "ndjson" => super::parse_json_lines_blocks(&content),
        "json" => super::parse_json_document_blocks(&content),
        "yaml" | "yml" => super::parse_yaml_document_blocks(&content),
        "toml" => super::parse_toml_document_blocks(&content),
        "html" | "htm" | "xhtml" => {
            super::parse_markup_string(&content, true).unwrap_or_else(|| {
                super::fallback_text_blocks(
                    &super::render_html_to_text(&content).unwrap_or_default(),
                )
            })
        }
        "xml" | "svg" | "opml" | "fb2" | "docbook" | "dbk" | "jats" | "nxml" | "tei" | "dita"
        | "ditamap" | "fodt" | "fods" | "fodp" | "plist" => {
            super::parse_markup_string(&content, false).unwrap_or_else(|| {
                super::fallback_text_blocks(&super::extract_xml_text(&content).unwrap_or_default())
            })
        }
        _ => super::fallback_text_blocks(&content),
    }
}

fn parse_structured_archive_entry(name: &str, content: &str) -> Option<ParsedDocument> {
    let path = Path::new(name);
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "eml" => parse_eml_string(path, content).ok(),
        "emlx" => parse_eml_string(path, strip_emlx_wrapper(content)).ok(),
        "mbox" => parse_mbox_string(path, content).ok(),
        "ics" | "ical" | "ifb" => super::parse_ics_string(
            path,
            content,
            super::normalize_text,
            super::tagged_fields_to_text,
        )
        .ok(),
        "vcf" | "vcard" => super::parse_vcf_string(
            path,
            content,
            super::normalize_text,
            super::tagged_fields_to_text,
        )
        .ok(),
        "ris" => super::parse_ris_string(path, content, super::normalize_text).ok(),
        "enw" => super::parse_enw_string(path, content, super::normalize_text).ok(),
        "nbib" => super::parse_nbib_string(path, content, super::normalize_text).ok(),
        "bib" | "bibtex" => super::parse_bib_string(path, content, super::normalize_text).ok(),
        "csl" => super::parse_csl_string(path, content, super::normalize_text).ok(),
        _ => None,
    }
}

fn flatten_archive_entry_document(name: &str, document: ParsedDocument) -> Vec<DocumentBlock> {
    let mut blocks = Vec::new();

    if let Some(title) = document
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    {
        blocks.push(
            DocumentBlock::new(
                DocumentBlockKind::Heading,
                Some(format!("{name}: title")),
                title.trim(),
            )
            .with_source(name.to_string())
            .with_ordinal(0),
        );
    }

    for (idx, block) in document.blocks.into_iter().enumerate() {
        let label = block
            .label
            .map(|label| format!("{name}: {label}"))
            .or_else(|| Some(name.to_string()));

        let mut new_block = DocumentBlock::new(block.kind, label, block.content)
            .with_source(name.to_string())
            .with_ordinal(idx + 1);

        for (key, value) in block.attributes {
            new_block = new_block.with_attribute(key, value);
        }
        if let Some(payload) = block.structured_payload {
            new_block = new_block.with_structured_payload(payload);
        }
        if let Some(metadata) = block.metadata {
            new_block = new_block.with_metadata(metadata);
        }

        if let Some(location) = block.location {
            if let Some(page) = location.page {
                new_block = new_block.with_page(page);
            }
            if location.continued_from_previous_page {
                new_block = new_block.with_continued_from_previous_page(true);
            }
            if location.continued_to_next_page {
                new_block = new_block.with_continued_to_next_page(true);
            }
        }

        blocks.push(new_block);
    }

    blocks
}

fn parse_tar_reader<R: Read>(reader: R, path: &Path) -> Result<ParsedDocument> {
    parse_tar_reader_named(reader, path, path, 0)
}

fn parse_tar_reader_named<R: Read>(
    reader: R,
    title_path: &Path,
    display_path: &Path,
    depth: usize,
) -> Result<ParsedDocument> {
    let mut archive = Archive::new(reader);
    let mut entries = Vec::new();
    let mut entry_count = 0usize;
    for entry in archive
        .entries()
        .with_context(|| format!("failed to read tar entries from {}", display_path.display()))?
    {
        entry_count += 1;
        let mut entry = entry
            .with_context(|| format!("failed to read tar entry from {}", display_path.display()))?;
        let Ok(entry_path) = entry.path() else {
            continue;
        };
        let name = entry_path.to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if !is_archive_entry_candidate(&lower) {
            continue;
        }

        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        entries.push((name, bytes));
    }

    parse_archive_entries_with_depth(title_path, "tar", entry_count, entries, depth)
}

fn parse_archive_entries(
    path: &Path,
    source: &str,
    entry_count: usize,
    entries: Vec<(String, Vec<u8>)>,
) -> Result<ParsedDocument> {
    parse_archive_entries_with_depth(path, source, entry_count, entries, 0)
}

fn archive_metadata_content(kind: &str, entry_count: usize) -> String {
    format!("type={kind}\nentries={entry_count}")
}

fn is_archive_entry_candidate(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") {
        return true;
    }

    matches!(
        Path::new(&lower)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some(
            "txt"
                | "md"
                | "markdown"
                | "html"
                | "htm"
                | "xhtml"
                | "xml"
                | "svg"
                | "opml"
                | "fb2"
                | "docbook"
                | "dbk"
                | "jats"
                | "nxml"
                | "tei"
                | "dita"
                | "ditamap"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "csv"
                | "tsv"
                | "jsonl"
                | "ndjson"
                | "log"
                | "plist"
                | "eml"
                | "emlx"
                | "mbox"
                | "ics"
                | "ical"
                | "ifb"
                | "vcf"
                | "vcard"
                | "ris"
                | "enw"
                | "nbib"
                | "bib"
                | "bibtex"
                | "csl"
                | "fods"
                | "fodt"
                | "fodp"
                | "doc"
                | "dot"
                | "docx"
                | "docm"
                | "dotx"
                | "dotm"
                | "xls"
                | "xlt"
                | "xlsx"
                | "xlsm"
                | "xlsb"
                | "xltx"
                | "xltm"
                | "xlam"
                | "ppt"
                | "pps"
                | "pptx"
                | "pptm"
                | "ppsx"
                | "potx"
                | "potm"
                | "hwp"
                | "hwpx"
                | "odt"
                | "ods"
                | "odp"
                | "mdx"
                | "rst"
                | "org"
                | "adoc"
                | "tex"
                | "latex"
                | "typ"
                | "typst"
                | "pdf"
                | "zip"
                | "7z"
                | "tar"
                | "gz"
                | "tgz"
        )
    )
}

fn parse_nested_archive_entry(name: &str, bytes: &[u8], depth: usize) -> Option<ParsedDocument> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        return parse_tgz_bytes(name, bytes.to_vec(), depth).ok();
    }
    if lower.ends_with(".7z") {
        return parse_7z_bytes(name, bytes.to_vec(), depth).ok();
    }
    if lower.ends_with(".tar") {
        return parse_tar_bytes_with_depth(name, bytes.to_vec(), depth).ok();
    }
    if lower.ends_with(".gz") {
        return parse_gzip_bytes(name, bytes.to_vec(), depth).ok();
    }
    if lower.ends_with(".zip") {
        return parse_zip_bytes(name, bytes.to_vec(), depth).ok();
    }
    None
}

fn parse_zip_bytes(name: &str, bytes: Vec<u8>, depth: usize) -> Result<ParsedDocument> {
    let cursor = Cursor::new(bytes);
    let mut zip = ZipArchive::new(cursor)
        .with_context(|| format!("failed to read nested zip container {name}"))?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut entries = Vec::new();
    for entry_name in names {
        let lower = entry_name.to_ascii_lowercase();
        if !is_archive_entry_candidate(&lower) {
            continue;
        }
        let entry_bytes = match read_zip_entry_bytes(&mut zip, &entry_name) {
            Ok(entry_bytes) => entry_bytes,
            Err(_) => continue,
        };
        entries.push((entry_name, entry_bytes));
    }

    parse_archive_entries_with_depth(Path::new(name), "zip", zip.len(), entries, depth)
}

fn parse_7z_bytes(name: &str, bytes: Vec<u8>, depth: usize) -> Result<ParsedDocument> {
    let tempdir = tempfile::tempdir().context("failed to create tempdir for nested 7z archive")?;
    let path = tempdir.path().join(
        Path::new(name)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("archive.7z"),
    );
    std::fs::write(&path, bytes)
        .with_context(|| format!("failed to stage nested 7z archive {name}"))?;

    let Some(binary) = find_7z_binary() else {
        anyhow::bail!("no 7z/7zz/7za binary found on PATH");
    };

    let entries = list_7z_entries(&binary, &path)?;
    let entry_count = entries.len();
    let mut extracted = Vec::new();
    for entry_name in entries {
        let lower = entry_name.to_ascii_lowercase();
        if !is_archive_entry_candidate(&lower) {
            continue;
        }
        let bytes = extract_7z_entry(&binary, &path, &entry_name)?;
        extracted.push((entry_name, bytes));
    }

    parse_archive_entries_with_depth(Path::new(name), "7z", entry_count, extracted, depth)
}

fn parse_tar_bytes(name: &str, bytes: Vec<u8>) -> Result<ParsedDocument> {
    parse_tar_bytes_with_depth(name, bytes, 1)
}

fn parse_tar_bytes_with_depth(name: &str, bytes: Vec<u8>, depth: usize) -> Result<ParsedDocument> {
    parse_tar_reader_named(Cursor::new(bytes), Path::new(name), Path::new(name), depth)
}

fn parse_tgz_bytes(name: &str, bytes: Vec<u8>, depth: usize) -> Result<ParsedDocument> {
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut tar_bytes = Vec::new();
    decoder
        .read_to_end(&mut tar_bytes)
        .with_context(|| format!("failed to decompress nested gzip archive {name}"))?;
    parse_tar_reader_named(
        Cursor::new(tar_bytes),
        Path::new(name),
        Path::new(name),
        depth,
    )
}

fn parse_gzip_bytes(name: &str, bytes: Vec<u8>, depth: usize) -> Result<ParsedDocument> {
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut inner_bytes = Vec::new();
    decoder
        .read_to_end(&mut inner_bytes)
        .with_context(|| format!("failed to decompress nested gzip archive {name}"))?;

    let file_name = Path::new(name)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or(name);
    let inner_name = file_name
        .strip_suffix(".gz")
        .unwrap_or(file_name)
        .to_string();

    if inner_name.to_ascii_lowercase().ends_with(".tar") {
        return parse_tar_bytes(&inner_name, inner_bytes);
    }

    parse_archive_entries_with_depth(
        Path::new(name),
        "gzip",
        1,
        vec![(inner_name, inner_bytes)],
        depth,
    )
}

fn parse_archive_entries_with_depth(
    path: &Path,
    source: &str,
    entry_count: usize,
    mut entries: Vec<(String, Vec<u8>)>,
    depth: usize,
) -> Result<ParsedDocument> {
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    doc.push(
        DocumentBlock::new(
            DocumentBlockKind::Metadata,
            Some("archive"),
            archive_metadata_content(source, entry_count),
        )
        .with_source(source)
        .with_ordinal(0),
    );

    let mut ordinal = 1usize;
    for (name, bytes) in entries {
        let blocks = parse_zip_entry_blocks(&name, &bytes, depth);
        if blocks.is_empty() {
            continue;
        }

        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some(format!("{name}: source")),
                format!("source: {name}"),
            )
            .with_source(name.clone())
            .with_ordinal(ordinal),
        );
        ordinal += 1;

        for block in blocks {
            let label = block
                .label
                .map(|label| format!("{name}: {label}"))
                .or_else(|| Some(name.clone()));
            let mut new_block = DocumentBlock::new(block.kind, label, block.content)
                .with_source(name.clone())
                .with_ordinal(ordinal);
            for (key, value) in block.attributes {
                new_block = new_block.with_attribute(key, value);
            }
            if let Some(payload) = block.structured_payload {
                new_block = new_block.with_structured_payload(payload);
            }
            if let Some(metadata) = block.metadata {
                new_block = new_block.with_metadata(metadata);
            }
            if let Some(location) = block.location {
                if let Some(page) = location.page {
                    new_block = new_block.with_page(page);
                }
                if location.continued_from_previous_page {
                    new_block = new_block.with_continued_from_previous_page(true);
                }
                if location.continued_to_next_page {
                    new_block = new_block.with_continued_to_next_page(true);
                }
            }
            doc.push(new_block);
            ordinal += 1;
        }
    }

    super::ensure_document(doc, path)
}

fn decode_text_bytes(bytes: &[u8]) -> Result<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_string());
    }

    if bytes.len() >= 2 && bytes.len() % 2 == 0 {
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let text = String::from_utf16(&utf16).context("failed to decode utf16 archive entry")?;
        return Ok(text.trim_start_matches('\u{feff}').to_string());
    }

    anyhow::bail!("unsupported archive text encoding")
}

fn find_7z_binary() -> Option<&'static str> {
    ["7zz", "7z", "7za"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--help")
            .output()
            .map(|output| {
                output.status.success() || !output.stdout.is_empty() || !output.stderr.is_empty()
            })
            .unwrap_or(false)
    })
}

fn list_7z_entries(binary: &str, path: &Path) -> Result<Vec<String>> {
    let output = Command::new(binary)
        .arg("l")
        .arg("-slt")
        .arg(path)
        .output()
        .with_context(|| format!("failed to spawn {binary} to list {}", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{binary} failed to list {}: {}",
            path.display(),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut started_entries = false;
    let mut current_path: Option<String> = None;
    let mut current_is_dir = false;
    let mut entries = Vec::new();

    let flush_entry = |entries: &mut Vec<String>,
                       current_path: &mut Option<String>,
                       current_is_dir: &mut bool| {
        if let Some(path) = current_path.take() {
            if !*current_is_dir && !path.trim().is_empty() {
                entries.push(path);
            }
        }
        *current_is_dir = false;
    };

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed == "----------" {
            started_entries = true;
            flush_entry(&mut entries, &mut current_path, &mut current_is_dir);
            continue;
        }
        if !started_entries {
            continue;
        }
        if trimmed.is_empty() {
            flush_entry(&mut entries, &mut current_path, &mut current_is_dir);
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Path = ") {
            current_path = Some(value.to_string());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Folder = ") {
            current_is_dir = value.eq_ignore_ascii_case("+");
        }
    }
    flush_entry(&mut entries, &mut current_path, &mut current_is_dir);

    Ok(entries)
}

fn extract_7z_entry(binary: &str, archive: &Path, entry_name: &str) -> Result<Vec<u8>> {
    let output = Command::new(binary)
        .arg("x")
        .arg("-so")
        .arg(archive)
        .arg(entry_name)
        .output()
        .with_context(|| {
            format!(
                "failed to spawn {binary} to extract {entry_name} from {}",
                archive.display()
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{binary} failed to extract {entry_name} from {}: {}",
            archive.display(),
            stderr.trim()
        );
    }
    Ok(output.stdout)
}

fn read_epub_title(zip: &mut ZipArchive<File>, names: &[String]) -> Option<String> {
    if let Ok(container_xml) = read_zip_entry(zip, "META-INF/container.xml") {
        if let Some(opf_path) = extract_epub_rootfile_path(&container_xml) {
            if let Ok(opf_xml) = read_zip_entry(zip, &opf_path) {
                if let Ok(metadata) = parse_container_metadata_xml(&opf_xml) {
                    if let Some(title) = metadata.and_then(|metadata| metadata.title) {
                        return Some(title);
                    }
                }
            }
        }
    }

    for name in names {
        if !name.to_ascii_lowercase().ends_with(".opf") {
            continue;
        }
        if let Ok(opf_xml) = read_zip_entry(zip, name) {
            if let Ok(metadata) = parse_container_metadata_xml(&opf_xml) {
                if let Some(title) = metadata.and_then(|metadata| metadata.title) {
                    return Some(title);
                }
            }
        }
    }

    None
}

fn extract_epub_rootfile_path(container_xml: &str) -> Option<String> {
    let doc = Document::parse(container_xml).ok()?;
    doc.descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "rootfile")
        .and_then(|node| {
            node.attribute("full-path")
                .or_else(|| super::attribute_by_local_name(node, "full-path"))
        })
        .map(str::to_string)
}

fn parse_container_metadata_xml(xml: &str) -> Result<Option<ContainerMetadata>> {
    let doc = Document::parse(xml).context("failed to parse container metadata xml")?;
    let metadata = ContainerMetadata {
        title: first_tag_text(&doc, &["title"]),
        subject: first_tag_text(&doc, &["subject"]),
        creator: first_tag_text(&doc, &["creator", "initial-creator"]),
        description: first_tag_text(&doc, &["description"]),
        language: first_tag_text(&doc, &["language"]),
    };

    if metadata.is_empty() {
        Ok(None)
    } else {
        Ok(Some(metadata))
    }
}

fn first_tag_text(doc: &Document<'_>, tag_names: &[&str]) -> Option<String> {
    doc.descendants()
        .find(|node| {
            node.is_element() && tag_names.iter().any(|name| node.tag_name().name() == *name)
        })
        .map(super::collect_node_text)
        .filter(|text| !text.trim().is_empty())
}
