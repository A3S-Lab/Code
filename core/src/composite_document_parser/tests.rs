use super::*;
use crate::document_parser::DocumentBlock;
use crate::document_parser::DocumentBlockKind;
use crate::document_parser::DocumentParser;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::{Mutex, OnceLock};
use tar::{Builder, Header};
use tempfile::TempDir;
use zip::write::FileOptions;

use super::normalize_paged::{
    best_open_column_alignment, infer_open_columns_from_rows, normalize_paged_text_boundaries,
    page_boundary_carry, page_boundary_multi_column_carry,
};
use super::ocr::should_attempt_pdf_ocr;
use super::ocr::{build_ocr_metadata_block, maybe_run_pdf_ocr, OcrResult, PdfOcrMode};
use super::text::{
    infer_reference_column_gaps, split_aligned_columns_with_gaps, split_paged_text, AlignedTextRow,
};

struct MockOcrProvider {
    text: Option<String>,
}

impl DocumentOcrProvider for MockOcrProvider {
    fn name(&self) -> &str {
        "mock-ocr"
    }

    fn capabilities(&self) -> DocumentOcrCapabilities {
        let mut capabilities = DocumentOcrCapabilities::new(["pdf"]);
        capabilities.model = Some("kimi-vision".to_string());
        capabilities
    }

    fn ocr_pdf(
        &self,
        _path: &Path,
        _config: &crate::config::DocumentOcrConfig,
    ) -> Result<Option<String>> {
        Ok(self.text.clone())
    }
}

struct RequestOnlyOcrProvider {
    text: Option<String>,
}

impl DocumentOcrProvider for RequestOnlyOcrProvider {
    fn name(&self) -> &str {
        "request-only-ocr"
    }

    fn capabilities(&self) -> DocumentOcrCapabilities {
        DocumentOcrCapabilities::new(["pdf", "image", "docx", "pptx", "xlsx", "odf"])
    }

    fn ocr_document_result(
        &self,
        request: &DocumentOcrRequest<'_>,
    ) -> Result<Option<DocumentOcrOutput>> {
        if matches!(
            request.format,
            DocumentOcrFormat::Pdf
                | DocumentOcrFormat::Image
                | DocumentOcrFormat::Docx
                | DocumentOcrFormat::Pptx
                | DocumentOcrFormat::Xlsx
                | DocumentOcrFormat::Odf
        ) {
            Ok(self.text.clone().map(|text| DocumentOcrOutput {
                text: text.clone(),
                pages: vec![DocumentOcrPageResult {
                    page: Some(1),
                    text,
                    language: Some("en".to_string()),
                    confidence_score_percent: Some(87),
                }],
                language: Some("en".to_string()),
                confidence_score_percent: Some(87),
                model: Some("request-vision".to_string()),
            }))
        } else {
            Ok(None)
        }
    }
}

struct ImageOnlyOcrProvider;

impl DocumentOcrProvider for ImageOnlyOcrProvider {
    fn name(&self) -> &str {
        "image-only-ocr"
    }

    fn capabilities(&self) -> DocumentOcrCapabilities {
        DocumentOcrCapabilities::new(["image"])
    }
}

struct CountingRequestOcrProvider {
    calls: Arc<Mutex<usize>>,
    text: String,
}

impl DocumentOcrProvider for CountingRequestOcrProvider {
    fn name(&self) -> &str {
        "counting-request-ocr"
    }

    fn capabilities(&self) -> DocumentOcrCapabilities {
        DocumentOcrCapabilities::new(["image"])
    }

    fn ocr_document_result(
        &self,
        request: &DocumentOcrRequest<'_>,
    ) -> Result<Option<DocumentOcrOutput>> {
        if request.format != DocumentOcrFormat::Image {
            return Ok(None);
        }
        *self.calls.lock().unwrap() += 1;
        Ok(Some(DocumentOcrOutput {
            text: self.text.clone(),
            pages: vec![DocumentOcrPageResult {
                page: Some(1),
                text: self.text.clone(),
                language: Some("en".to_string()),
                confidence_score_percent: Some(91),
            }],
            language: Some("en".to_string()),
            confidence_score_percent: Some(91),
            model: Some("counting-model".to_string()),
        }))
    }
}

fn write_file(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn write_zip(dir: &TempDir, name: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let file = File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default();

    for (entry, content) in entries {
        zip.start_file(*entry, options).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }

    zip.finish().unwrap();
    path
}

fn make_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = FileOptions::default();

    for (entry, content) in entries {
        zip.start_file(*entry, options).unwrap();
        zip.write_all(content).unwrap();
    }

    zip.finish().unwrap().into_inner()
}

fn write_tar(dir: &TempDir, name: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let file = File::create(&path).unwrap();
    let mut tar = Builder::new(file);

    for (entry, content) in entries {
        let bytes = content.as_bytes();
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, *entry, bytes).unwrap();
    }

    tar.finish().unwrap();
    path
}

fn make_tar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = Builder::new(Vec::new());

    for (entry, content) in entries {
        let mut header = Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, *entry, *content).unwrap();
    }

    tar.into_inner().unwrap()
}

fn write_zip_bytes(dir: &TempDir, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, make_zip_bytes(entries)).unwrap();
    path
}

fn write_gzip(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let file = File::create(&path).unwrap();
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder.write_all(content).unwrap();
    encoder.finish().unwrap();
    path
}

fn utf16le_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

fn simple_pdf_bytes(text: &str) -> Vec<u8> {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)");

    let objects = vec![
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
        "2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n".to_string(),
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n".to_string(),
        format!(
            "4 0 obj\n<< /Length {} >>\nstream\nBT\n/F1 18 Tf\n36 96 Td\n({escaped}) Tj\nET\nendstream\nendobj\n",
            format!("BT\n/F1 18 Tf\n36 96 Td\n({escaped}) Tj\nET\n").len()
        ),
        "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
            .to_string(),
    ];

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];

    for object in &objects {
        offsets.push(pdf.len());
        pdf.extend_from_slice(object.as_bytes());
    }

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );
    pdf
}

fn ocr_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(unix)]
fn write_executable(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn parses_html() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "sample.html",
            "<html><head><title>Welcome Page</title><meta name=\"description\" content=\"Landing page intro\" /></head><body><h1>Hello</h1><p>World</p><table><tr><th>Name</th><th>Score</th></tr><tr><td>Alice</td><td>42</td></tr></table><ul><li>Fast</li><li>Reliable</li></ul><dl><dt>Parser</dt><dd>CompositeDocumentParser</dd></dl></body></html>",
        );
    let doc = parse_html_document(&path).unwrap();
    assert_eq!(doc.title.as_deref(), Some("Welcome Page"));
    assert!(doc.blocks.iter().enumerate().all(|(idx, block)| {
        block.location.as_ref().and_then(|loc| loc.ordinal) == Some(idx + 1)
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("description")
            && block.content.contains("Landing page intro")
    }));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Heading));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.content.contains("Name\tScore")
            && block.content.contains("Alice\t42")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Section
            && block.label.as_deref() == Some("list")
            && block.content.contains("- Fast")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Section
            && block.label.as_deref() == Some("definitions")
            && block.content.contains("Parser: CompositeDocumentParser")
    }));
    assert!(doc.to_text().contains("Hello"));
    assert!(doc.to_text().contains("World"));
}

#[test]
fn parses_html_semantic_labels_and_image_alt() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "semantic.html",
            "<html><head><title>Semantic Page</title></head><body><section><h2>Overview</h2><p>Parser improvements are summarized here.</p></section><table><caption>Quarterly Results</caption><tr><th>Name</th><th>Score</th></tr><tr><td>Alice</td><td>42</td></tr></table><img src=\"charts/summary.png\" alt=\"Bar chart of parser recall\" /></body></html>",
        );
    let doc = parse_html_document(&path).unwrap();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Section
            && block.label.as_deref() == Some("Overview")
            && block.content.contains("Parser improvements")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("Quarterly Results")
            && block.content.contains("Name\tScore")
            && block.structured_payload.as_deref().is_some_and(|payload| {
                payload.contains("\"rows\":[[\"Name\",\"Score\"],[\"Alice\",\"42\"]]")
            })
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("image-alt: charts/summary.png")
            && block.content.contains("Bar chart of parser recall")
    }));
}

#[test]
fn parses_docx_like_zip() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "sample.docx",
        &[
            (
                "docProps/core.xml",
                r#"<cp:coreProperties xmlns:cp="urn:test" xmlns:dc="urn:test-dc"><dc:title>Quarterly Report</dc:title><dc:creator>A3S</dc:creator><dc:subject>Status</dc:subject></cp:coreProperties>"#,
            ),
            (
                "word/document.xml",
                r#"<w:document xmlns:w="urn:test"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t>World</w:t></w:r></w:p></w:body></w:document>"#,
            ),
        ],
    );
    let doc = parse_docx(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    assert_eq!(doc.title.as_deref(), Some("Quarterly Report"));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("core-properties")
            && block.content.contains("creator=A3S")
    }));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("document")));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Heading));
    assert!(doc.to_text().contains("Hello"));
    assert!(doc.to_text().contains("World"));
}

#[test]
fn parses_legacy_doc_compound_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.doc");
    let mut compound = cfb::create(&path).unwrap();
    {
        let mut stream = compound.create_stream("/WordDocument").unwrap();
        stream
            .write_all(&utf16le_bytes(
                "Legacy Project Plan\nThis binary doc body should be extracted.",
            ))
            .unwrap();
    }
    compound.flush().unwrap();
    drop(compound);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(text.contains("Legacy Project Plan"));
    assert!(text.contains("This binary doc body should be extracted."));
}

#[test]
fn parses_xlsx_shared_strings_and_inline_cells() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "sample.xlsx",
        &[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Summary" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<sst xmlns="urn:test"><si><t>Name</t></si><si><t>Alice</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="urn:test"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>Score</t></is></c><c r="C1" t="b"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="C2"><f>SUM(A2:B2)</f></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>Bob</t></is></c><c r="B3"><v>42</v></c><c r="C3" t="b"><v>0</v></c></row></sheetData></worksheet>"#,
            ),
        ],
    );
    let doc = parse_xlsx(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    let text = doc.to_text();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("Summary: worksheet")
            && block.content.contains("rows=3")
            && block.content.contains("columns=3")
    }));
    assert!(doc
            .blocks
            .iter()
            .any(|block| {
                block.label.as_deref() == Some("Summary")
                    && block.attributes.get("row_count").map(String::as_str) == Some("3")
                    && block.attributes.get("column_count").map(String::as_str) == Some("3")
                    && block
                        .structured_payload
                        .as_deref()
                        .is_some_and(|payload| payload.contains("\"rows\":[[\"Name\",\"Score\",\"TRUE\"],[\"Alice\",\"\",\"=SUM(A2:B2)\"],[\"Bob\",\"42\",\"FALSE\"]]"))
            }));
    assert!(text.contains("Name"));
    assert!(text.contains("Score"));
    assert!(text.contains("TRUE"));
    assert!(text.contains("Alice"));
    assert!(text.contains("Alice\t\t=SUM(A2:B2)"));
    assert!(text.contains("Bob\t42\tFALSE"));
    assert!(text.contains("42"));
}

#[test]
fn parses_xlsb_heuristic_sheet_strings() {
    let dir = TempDir::new().unwrap();
    let path = write_zip_bytes(
        &dir,
        "sample.xlsb",
        &[
            (
                "xl/workbook.bin",
                &utf16le_bytes("Workbook\0Summary") as &[u8],
            ),
            (
                "xl/_rels/workbook.bin.rels",
                br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.bin" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#
                    as &[u8],
            ),
            (
                "xl/sharedStrings.bin",
                &utf16le_bytes("Alice\0Score\0Budget") as &[u8],
            ),
            (
                "xl/worksheets/sheet1.bin",
                &utf16le_bytes("Name\nAlice\nBob\nScore\n42\n99"),
            ),
        ],
    );

    let doc = parse_xlsb(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    let text = doc.to_text();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("Summary: worksheet")
            && block
                .content
                .contains("extraction=heuristic-string-recovery")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("Summary")
            && block.attributes.get("extraction").map(String::as_str)
                == Some("heuristic-string-recovery")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.attributes.get("column_count").map(String::as_str) == Some("2")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("shared strings")
            && block.attributes.get("extraction").map(String::as_str)
                == Some("shared-strings-recovery")
            && block.structured_payload.is_some()
    }));
    assert!(text.contains("Name"));
    assert!(text.contains("Alice"));
    assert!(text.contains("Bob"));
    assert!(text.contains("42"));
    assert!(text.contains("99"));
    assert!(text.contains("Budget"));
    assert!(text.contains("Name\tScore"));
    assert!(text.contains("Alice\t42"));
    assert!(text.contains("Bob\t99"));
}

#[test]
fn xlsb_column_major_values_are_transposed_into_rows() {
    let values = vec![
        "Name".to_string(),
        "Alice".to_string(),
        "Bob".to_string(),
        "Score".to_string(),
        "42".to_string(),
        "99".to_string(),
    ];

    let rows = super::office::infer_xlsb_column_major_rows(&values).unwrap();

    assert_eq!(
        rows,
        vec![
            vec!["Name".to_string(), "Score".to_string()],
            vec!["Alice".to_string(), "42".to_string()],
            vec!["Bob".to_string(), "99".to_string()],
        ]
    );
}

#[test]
fn xlsb_row_major_values_are_grouped_into_rows() {
    let values = vec![
        "Name".to_string(),
        "Score".to_string(),
        "Alice".to_string(),
        "42".to_string(),
        "Bob".to_string(),
        "99".to_string(),
    ];

    let rows = super::office::infer_xlsb_row_major_rows(&values).unwrap();

    assert_eq!(
        rows,
        vec![
            vec!["Name".to_string(), "Score".to_string()],
            vec!["Alice".to_string(), "42".to_string()],
            vec!["Bob".to_string(), "99".to_string()],
        ]
    );
}

#[test]
fn parses_xlsb_row_major_sheet_strings() {
    let dir = TempDir::new().unwrap();
    let path = write_zip_bytes(
        &dir,
        "row-major.xlsb",
        &[
            (
                "xl/workbook.bin",
                &utf16le_bytes("Workbook\0Metrics") as &[u8],
            ),
            (
                "xl/_rels/workbook.bin.rels",
                br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.bin" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#
                    as &[u8],
            ),
            (
                "xl/worksheets/sheet1.bin",
                &utf16le_bytes("Name\nScore\nAlice\n42\nBob\n99"),
            ),
        ],
    );

    let doc = parse_xlsb(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    let text = doc.to_text();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("Metrics")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.attributes.get("column_count").map(String::as_str) == Some("2")
            && block.structured_payload.is_some()
    }));
    assert!(text.contains("Name\tScore"));
    assert!(text.contains("Alice\t42"));
    assert!(text.contains("Bob\t99"));
}

#[test]
fn parses_xlsb_embedded_tsv_block() {
    let dir = TempDir::new().unwrap();
    let path = write_zip_bytes(
        &dir,
        "embedded-table.xlsb",
        &[
            (
                "xl/workbook.bin",
                &utf16le_bytes("Workbook\0SheetA") as &[u8],
            ),
            (
                "xl/_rels/workbook.bin.rels",
                br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.bin" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#
                    as &[u8],
            ),
            (
                "xl/worksheets/sheet1.bin",
                &utf16le_bytes("Name\tScore\nAlice\t42\nBob\t99"),
            ),
        ],
    );

    let doc = parse_xlsb(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    let text = doc.to_text();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("SheetA")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.attributes.get("column_count").map(String::as_str) == Some("2")
            && block.structured_payload.is_some()
    }));
    assert!(text.contains("Name\tScore"));
    assert!(text.contains("Alice\t42"));
    assert!(text.contains("Bob\t99"));
}

#[test]
fn parses_xlsb_fragmented_tsv_rows() {
    let dir = TempDir::new().unwrap();
    let path = write_zip_bytes(
        &dir,
        "fragmented-rows.xlsb",
        &[
            (
                "xl/workbook.bin",
                &utf16le_bytes("Workbook\0SheetB") as &[u8],
            ),
            (
                "xl/_rels/workbook.bin.rels",
                br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.bin" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#
                    as &[u8],
            ),
            (
                "xl/worksheets/sheet1.bin",
                &utf16le_bytes("Name\tScore\0Alice\t42\0Bob\t99"),
            ),
        ],
    );

    let doc = parse_xlsb(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    let text = doc.to_text();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("SheetB")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.attributes.get("column_count").map(String::as_str) == Some("2")
            && block.structured_payload.is_some()
    }));
    assert!(text.contains("Name\tScore"));
    assert!(text.contains("Alice\t42"));
    assert!(text.contains("Bob\t99"));
}

#[test]
fn parses_xlsb_mixed_text_and_table_segments() {
    let dir = TempDir::new().unwrap();
    let path = write_zip_bytes(
        &dir,
        "mixed-segments.xlsb",
        &[
            (
                "xl/workbook.bin",
                &utf16le_bytes("Workbook\0SheetC") as &[u8],
            ),
            (
                "xl/_rels/workbook.bin.rels",
                br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.bin" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>"#
                    as &[u8],
            ),
            (
                "xl/worksheets/sheet1.bin",
                &utf16le_bytes(
                    "Quarterly summary\nRevenue table follows\0Name\tScore\0Alice\t42\0Bob\t99\0Notes\nFinal review pending",
                ),
            ),
        ],
    );

    let doc = parse_xlsb(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    let text = doc.to_text();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("SheetC: worksheet")
            && block.attributes.get("text_block_count").map(String::as_str) == Some("2")
            && block.attributes.get("segment_count").map(String::as_str) == Some("3")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("SheetC")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.attributes.get("column_count").map(String::as_str) == Some("2")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.attributes.get("extraction").map(String::as_str) == Some("text-segmentation")
            && block.content.contains("Revenue table follows")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.attributes.get("extraction").map(String::as_str) == Some("text-segmentation")
            && block.content.contains("Final review pending")
    }));
    assert!(text.contains("Name\tScore"));
    assert!(text.contains("Alice\t42"));
    assert!(text.contains("Bob\t99"));
}

#[test]
fn parses_hwpx_section_xml_content() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "sample.hwpx",
        &[
            ("Contents/content.hpf", r#"<hpf><title>Guide</title></hpf>"#),
            (
                "Contents/section0.xml",
                r#"<root><title>Intro</title><p>Hello HWPX</p><p>Second paragraph</p></root>"#,
            ),
        ],
    );

    let doc = parse_hwpx(&path).unwrap();
    let text = doc.to_text();

    assert_eq!(doc.blocks[0].label.as_deref(), Some("hwpx"));
    assert!(text.contains("Intro"));
    assert!(text.contains("Hello HWPX"));
    assert!(text.contains("Second paragraph"));
}

#[test]
fn parses_hwp_prvtext_stream() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sample.hwp");
    let mut compound = cfb::create(&path).unwrap();
    {
        let mut stream = compound.create_stream("/PrvText").unwrap();
        stream
            .write_all(&utf16le_bytes("HWP Title\nHello HWP body text"))
            .unwrap();
    }
    compound.flush().unwrap();
    drop(compound);

    let doc = parse_hwp(&path).unwrap();
    let text = doc.to_text();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("hwp"));
    assert!(text.contains("HWP Title"));
    assert!(text.contains("Hello HWP body text"));
}

#[test]
fn parses_legacy_xls_compound_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("scores.xls");
    let mut compound = cfb::create(&path).unwrap();
    {
        let mut stream = compound.create_stream("/Workbook").unwrap();
        stream
            .write_all(b"Name\tScore\nAlice\t42\nBob\t99\n")
            .unwrap();
    }
    compound.flush().unwrap();
    drop(compound);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.content.contains("Bob\t99")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.structured_payload.as_deref().is_some_and(|payload| {
                payload
                    .contains("\"rows\":[[\"Name\",\"Score\"],[\"Alice\",\"42\"],[\"Bob\",\"99\"]]")
            })
    }));
}

#[test]
fn xlsx_cell_reference_to_index_maps_columns() {
    assert_eq!(super::office::xlsx_cell_reference_to_index("A1"), Some(0));
    assert_eq!(super::office::xlsx_cell_reference_to_index("B7"), Some(1));
    assert_eq!(super::office::xlsx_cell_reference_to_index("Z9"), Some(25));
    assert_eq!(super::office::xlsx_cell_reference_to_index("AA3"), Some(26));
    assert_eq!(
        super::office::xlsx_cell_reference_to_index("AB12"),
        Some(27)
    );
    assert_eq!(super::office::xlsx_cell_reference_to_index(""), None);
}

#[test]
fn parses_pptx_slides() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "slides.pptx",
        &[(
            "ppt/slides/slide1.xml",
            r#"<p:sld xmlns:p="urn:test" xmlns:a="urn:test-a"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Quarterly Review</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
        )],
    );
    let doc = parse_pptx(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("slide 1")));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Heading));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.location.as_ref().and_then(|loc| loc.page) == Some(1)));
    assert!(doc.to_text().contains("Quarterly Review"));
}

#[test]
fn parses_legacy_ppt_compound_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("review.ppt");
    let mut compound = cfb::create(&path).unwrap();
    {
        let mut stream = compound.create_stream("/PowerPoint Document").unwrap();
        stream
            .write_all(&utf16le_bytes(
                "Quarterly Review\nRoadmap highlights\nRevenue is up.",
            ))
            .unwrap();
    }
    compound.flush().unwrap();
    drop(compound);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(text.contains("Quarterly Review"));
    assert!(text.contains("Revenue is up."));
}

#[test]
fn parses_odf_content() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "document.odt",
        &[
            (
                "meta.xml",
                r#"<office:document-meta xmlns:office="urn:test" xmlns:dc="urn:test-dc" xmlns:meta="urn:test-meta"><office:meta><dc:title>ODF Handbook</dc:title><meta:initial-creator>Roy</meta:initial-creator></office:meta></office:document-meta>"#,
            ),
            (
                "content.xml",
                r#"<office:document-content xmlns:office="urn:test" xmlns:text="urn:test-text"><office:body><office:text><text:p>Hello ODF</text:p><text:p>Second line</text:p></office:text></office:body></office:document-content>"#,
            ),
        ],
    );
    let doc = parse_odf(&path, &crate::config::DocumentParserConfig::default(), None).unwrap();
    assert_eq!(doc.title.as_deref(), Some("ODF Handbook"));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("document-metadata")
            && block.content.contains("creator=Roy")
    }));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Paragraph));
    assert!(doc.to_text().contains("Hello ODF"));
    assert!(doc.to_text().contains("Second line"));
}

#[test]
fn parses_epub_html_entries() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
            &dir,
            "book.epub",
            &[
                (
                    "META-INF/container.xml",
                    r#"<container xmlns="urn:test"><rootfiles><rootfile full-path="OPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                (
                    "OPS/package.opf",
                    r#"<package xmlns:dc="urn:test-dc"><metadata><dc:title>Package Title</dc:title><dc:creator>Writer</dc:creator></metadata></package>"#,
                ),
                (
                    "OPS/ch1.xhtml",
                    "<html><head><title>Book Title</title></head><body><p>Chapter One</p></body></html>",
                ),
            ],
        );
    let doc = parse_epub(&path).unwrap();
    assert_eq!(doc.title.as_deref(), Some("Package Title"));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Paragraph));
    assert!(doc.to_text().contains("Chapter One"));
}

#[test]
fn parses_pages_package_via_zip_assets() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "deck.pages",
        &[
            (
                "Metadata/DocumentIdentifier.plist",
                r#"<plist><dict><key>Title</key><string>Quarterly Review</string></dict></plist>"#,
            ),
            (
                "Preview/index.html",
                r#"<html><body><section><h1>Quarterly Review</h1><p>Revenue grew 20 percent.</p></section></body></html>"#,
            ),
        ],
    );
    let doc = parse_iwork_package(&path, "pages").unwrap();
    let text = doc.to_text();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("iwork"));
    assert!(doc.blocks[0].content.contains("format=pages"));
    assert!(text.contains("Quarterly Review"));
    assert!(text.contains("Revenue grew 20 percent."));
}

#[test]
fn parses_pages_package_via_preview_pdf() {
    let dir = TempDir::new().unwrap();
    let path = write_zip_bytes(
        &dir,
        "deck.pages",
        &[
            (
                "Metadata/DocumentIdentifier.plist",
                br#"<plist><dict><key>Title</key><string>Quarterly Review</string></dict></plist>"#,
            ),
            (
                "QuickLook/Preview.pdf",
                &simple_pdf_bytes("Preview PDF body for iWork package"),
            ),
        ],
    );

    let doc = parse_iwork_package(&path, "pages").unwrap();
    let text = doc.to_text();

    assert!(text.contains("Quarterly Review"));
    assert!(text.contains("Preview PDF body for iWork package"));
}

#[test]
fn parses_generic_zip_text_entries() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "bundle.zip",
        &[
            ("docs/readme.txt", "Bundle overview"),
            (
                "docs/spec.xml",
                "<root><title>Spec Title</title><section><p>Zip XML text</p></section></root>",
            ),
            ("bin/data.bin", "\u{0000}\u{0001}\u{0002}"),
        ],
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("archive")
            && block.content.contains("entries=3")
    }));
    assert!(text.contains("docs/readme.txt"));
    assert!(text.contains("Bundle overview"));
    assert!(text.contains("Zip XML text"));
}

#[test]
fn parses_tar_text_entries() {
    let dir = TempDir::new().unwrap();
    let path = write_tar(
        &dir,
        "bundle.tar",
        &[
            ("docs/readme.txt", "Bundle overview"),
            (
                "docs/spec.xml",
                "<root><title>Spec Title</title><section><p>Tar XML text</p></section></root>",
            ),
            ("bin/data.bin", "\u{0000}\u{0001}\u{0002}"),
        ],
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("archive")
            && block.content.contains("type=tar")
            && block.content.contains("entries=3")
    }));
    assert!(text.contains("docs/readme.txt"));
    assert!(text.contains("Bundle overview"));
    assert!(text.contains("Tar XML text"));
}

#[test]
fn parses_tgz_text_entries() {
    let dir = TempDir::new().unwrap();
    let tar_path = write_tar(
        &dir,
        "bundle.tar",
        &[
            ("notes/info.txt", "Compressed overview"),
            ("notes/data.json", "{ \"title\": \"payload\" }"),
        ],
    );
    let tar_bytes = std::fs::read(tar_path).unwrap();
    let path = write_gzip(&dir, "bundle.tar.gz", &tar_bytes);
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("archive")
            && block.content.contains("type=tar")
            && block.content.contains("entries=2")
    }));
    assert!(text.contains("notes/info.txt"));
    assert!(text.contains("Compressed overview"));
    assert!(text.contains("notes/data.json: root.title"));
    assert!(text.contains("payload"));
}

#[test]
fn parses_gzip_text_entry() {
    let dir = TempDir::new().unwrap();
    let path = write_gzip(
        &dir,
        "notes.txt.gz",
        b"Release notes\n\nCompositeDocumentParser gzip support",
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("archive")
            && block.content.contains("type=gzip")
            && block.content.contains("entries=1")
    }));
    assert!(text.contains("notes.txt"));
    assert!(text.contains("CompositeDocumentParser gzip support"));
}

#[test]
fn parses_markdown_document() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "notes.md",
        "# CompositeDocumentParser\n\n- archive support\n- msg support\n",
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("notes.md"));
    let text = doc.to_text();
    assert!(text.contains("# CompositeDocumentParser"));
    assert!(text.contains("archive support"));
    assert!(text.contains("msg support"));
}

#[test]
fn parses_csv_document_as_table() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "report.csv", "name,score\nalice,42\nbob,99\n");
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("table")
            && block.content.contains("name\tscore")
            && block.content.contains("alice\t42")
            && block.attributes.get("delimiter").map(String::as_str) == Some("csv")
            && block.structured_payload.as_deref().is_some_and(|payload| {
                payload
                    .contains("\"rows\":[[\"name\",\"score\"],[\"alice\",\"42\"],[\"bob\",\"99\"]]")
            })
    }));
}

#[test]
fn parses_jsonl_document_as_records() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "events.jsonl",
        "{\"event\":\"parse\",\"ok\":true}\n{\"event\":\"search\",\"ok\":false}\n",
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Code
            && block.label.as_deref() == Some("record 1")
            && block.content.contains("\"event\": \"parse\"")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Code
            && block.label.as_deref() == Some("record 2")
            && block.content.contains("\"ok\": false")
    }));
}

#[test]
fn parses_json_document_as_structured_blocks() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "config.json",
        r#"{"service":{"name":"a3s","ports":[3000,3001]},"enabled":true}"#,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("structure")
            && block.content.contains("format=json")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Section
            && block.label.as_deref() == Some("root.service")
            && block.content.contains("fields=name, ports")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("root.service.name")
            && block.content == "a3s"
    }));
}

#[test]
fn parses_yaml_document_as_structured_blocks() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "config.yaml",
        "service:\n  name: a3s\n  ports:\n    - 3000\nenabled: true\n",
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("structure")
            && block.content.contains("format=yaml")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.label.as_deref() == Some("root.service.ports") && block.content.contains("items=1")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("root.enabled")
            && block.content == "true"
    }));
}

#[test]
fn parses_toml_document_as_structured_blocks() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "config.toml",
        "[service]\nname = \"a3s\"\nports = [3000, 3001]\n[agentic_parse]\nenabled = true\n",
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("structure")
            && block.content.contains("format=toml")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Section
            && block.label.as_deref() == Some("root.service")
            && block.content.contains("fields=name, ports")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("root.agentic_parse.enabled")
            && block.content == "true"
    }));
}

#[test]
fn parses_json_inside_archive_as_structured_blocks() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "bundle.zip",
        &[("config/settings.json", r#"{"search":{"enabled":true}}"#)],
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("config/settings.json: structure")
            && block.content.contains("format=json")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("config/settings.json: root.search.enabled")
            && block.content == "true"
    }));
}

#[test]
fn parses_csv_inside_archive_as_table() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "dataset.zip",
        &[("tables/data.csv", "name,score\nalice,42\nbob,99\n")],
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Table
            && block.label.as_deref() == Some("tables/data.csv: table")
            && block.attributes.get("row_count").map(String::as_str) == Some("3")
            && block.structured_payload.is_some()
            && block.content.contains("bob\t99")
    }));
}

#[test]
fn parses_eml_inside_archive_as_mail_blocks() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
            &dir,
            "mail-bundle.zip",
            &[(
                "inbox/welcome.eml",
                "Subject: Archived Message\nFrom: Roy <roy@example.com>\nTo: Team <team@example.com>\n\nArchive-aware parser body.\n",
            )],
        );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(text.contains("Archived Message"));
    assert!(text.contains("From: Roy <roy@example.com>"));
    assert!(text.contains("Archive-aware parser body."));
}

#[test]
fn parses_ics_inside_archive_as_structured_metadata() {
    let dir = TempDir::new().unwrap();
    let path = write_tar(
            &dir,
            "calendar-bundle.tar",
            &[(
                "events/roadmap.ics",
                "BEGIN:VCALENDAR\nVERSION:2.0\nX-WR-CALNAME:Release Calendar\nBEGIN:VEVENT\nUID:42\nSUMMARY:Archive Planning\nDTSTART:20260327T090000Z\nDESCRIPTION:Structured data inside archive.\nEND:VEVENT\nEND:VCALENDAR\n",
            )],
        );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(text.contains("Release Calendar"));
    assert!(text.contains("Archive Planning"));
    assert!(text.contains("Structured data inside archive."));
}

#[test]
fn parses_docx_inside_archive_with_office_parser() {
    let dir = TempDir::new().unwrap();
    let inner_docx = make_zip_bytes(&[
            (
                "docProps/core.xml",
                br#"<cp:coreProperties xmlns:cp="urn:test" xmlns:dc="urn:test-dc"><dc:title>Archived Report</dc:title></cp:coreProperties>"#
                    as &[u8],
            ),
            (
                "word/document.xml",
                br#"<w:document xmlns:w="urn:test"><w:body><w:p><w:r><w:t>Archive embedded docx body.</w:t></w:r></w:p></w:body></w:document>"#
                    as &[u8],
            ),
        ]);
    let path = write_zip_bytes(&dir, "bundle.zip", &[("docs/report.docx", &inner_docx)]);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert!(text.contains("Archived Report"));
    assert!(text.contains("Archive embedded docx body."));
}

#[test]
fn parses_pdf_inside_archive_with_pdf_parser() {
    let dir = TempDir::new().unwrap();
    let inner_pdf = simple_pdf_bytes("Archive embedded PDF body.");
    let path = write_zip_bytes(&dir, "bundle.zip", &[("docs/preview.pdf", &inner_pdf)]);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();

    assert!(text.contains("Archive embedded PDF body."));
}

#[test]
fn parses_legacy_doc_inside_archive_with_office_parser() {
    let dir = TempDir::new().unwrap();
    let inner_doc_path = dir.path().join("inner.doc");
    let mut compound = cfb::create(&inner_doc_path).unwrap();
    {
        let mut stream = compound.create_stream("/WordDocument").unwrap();
        stream
            .write_all(&utf16le_bytes(
                "Archived Legacy Plan\nCompound file text from tar.",
            ))
            .unwrap();
    }
    compound.flush().unwrap();
    drop(compound);
    let inner_doc = std::fs::read(&inner_doc_path).unwrap();

    let tar_path = dir.path().join("legacy-docs.tar");
    let file = File::create(&tar_path).unwrap();
    let mut tar = Builder::new(file);
    let mut header = Header::new_gnu();
    header.set_size(inner_doc.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "legacy/plan.doc", inner_doc.as_slice())
        .unwrap();
    tar.finish().unwrap();

    let doc = CompositeDocumentParser::default()
        .parse_document(&tar_path)
        .unwrap();
    let text = doc.to_text();
    assert!(text.contains("Archived Legacy Plan"));
    assert!(text.contains("Compound file text from tar."));
}

#[test]
fn parses_nested_zip_entries_recursively() {
    let dir = TempDir::new().unwrap();
    let inner_zip = make_zip_bytes(&[(
        "config/settings.json",
        br#"{"search":{"enabled":true,"provider":"nested"}}"# as &[u8],
    )]);
    let path = write_zip_bytes(&dir, "outer.zip", &[("nested/inner.zip", &inner_zip)]);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();

    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref() == Some("nested/inner.zip: nested/inner.zip: archive")
            && block.content.contains("type=zip")
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::Metadata
            && block.label.as_deref()
                == Some(
                    "nested/inner.zip: nested/inner.zip: config/settings.json: root.search.enabled",
                )
            && block.content == "true"
    }));
}

#[test]
fn parses_nested_tar_gz_entries_recursively() {
    let dir = TempDir::new().unwrap();
    let inner_tar = make_tar_bytes(&[(
            "events/release.ics",
            b"BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:99\nSUMMARY:Nested Launch\nDESCRIPTION:Inner tgz payload.\nEND:VEVENT\nEND:VCALENDAR\n"
                as &[u8],
        )]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&inner_tar).unwrap();
    let inner_tgz = encoder.finish().unwrap();
    let path = write_zip_bytes(
        &dir,
        "nested.tgz.zip",
        &[("archives/releases.tgz", &inner_tgz)],
    );

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();

    assert!(text.contains("Nested Launch"));
    assert!(text.contains("Inner tgz payload."));
}

#[test]
fn parses_plain_eml() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "mail.eml",
            "Subject: Hello\nFrom: alice@example.com\nTo: bob@example.com\nContent-Type: text/plain; charset=utf-8\n\nThis is a plain email body.\n",
        );
    let doc = parse_eml(&path).unwrap();
    let text = doc.to_text();
    assert_eq!(doc.title.as_deref(), Some("Hello"));
    assert!(text.contains("Subject: Hello"));
    assert!(text.contains("alice@example.com"));
    assert!(text.contains("This is a plain email body."));
    assert!(doc.blocks.iter().any(|block| {
        block.kind == DocumentBlockKind::EmailHeader
            && block
                .structured_payload
                .as_deref()
                .is_some_and(|payload| payload.contains("\"subject\":\"Hello\""))
    }));
}

#[test]
fn parses_emlx_wrapper() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "mail.emlx",
        concat!(
            "126\n",
            "Subject: Apple Mail\n",
            "From: alice@example.com\n",
            "To: bob@example.com\n",
            "Content-Type: text/plain; charset=utf-8\n",
            "\n",
            "Wrapped emlx body.\n",
            "<?xml version=\"1.0\"?><plist><dict/></plist>\n"
        ),
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert_eq!(doc.title.as_deref(), Some("Apple Mail"));
    assert!(text.contains("alice@example.com"));
    assert!(text.contains("Wrapped emlx body."));
}

#[test]
fn parses_mbox_messages() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "inbox.mbox",
        concat!(
            "From sender@example.com Fri Mar 27 10:00:00 2026\n",
            "Subject: First Mail\n",
            "From: alice@example.com\n",
            "To: bob@example.com\n",
            "Content-Type: text/plain; charset=utf-8\n",
            "\n",
            "Message one body.\n",
            "From sender2@example.com Fri Mar 27 11:00:00 2026\n",
            "Subject: Second Mail\n",
            "From: carol@example.com\n",
            "To: dave@example.com\n",
            "Content-Type: text/plain; charset=utf-8\n",
            "\n",
            "Message two body.\n"
        ),
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert_eq!(doc.title.as_deref(), Some("First Mail"));
    assert!(text.contains("message 1: headers"));
    assert!(text.contains("Message one body."));
    assert!(text.contains("message 2: body"));
    assert!(text.contains("Message two body."));
}

#[test]
fn parses_msg_via_best_effort_string_recovery() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mail.msg");
    let mut compound = cfb::create(&path).unwrap();
    msg::write_msg_utf16_stream(&mut compound, "/__substg1.0_0037001F", "Quarterly Sync");
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__substg1.0_007D001F",
        "From: alice@example.com\nTo: bob@example.com\nSubject: Quarterly Sync",
    );
    msg::write_msg_utf16_stream(&mut compound, "/__substg1.0_0C1A001F", "Alice");
    msg::write_msg_utf16_stream(&mut compound, "/__substg1.0_0C1F001F", "alice@example.com");
    msg::write_msg_time_stream(&mut compound, "/__substg1.0_00390040", 1_774_605_600);
    msg::write_msg_utf16_stream(
            &mut compound,
            "/__substg1.0_1000001F",
            "This is the recovered Outlook body with enough words to be treated as meaningful content by CompositeDocumentParser.",
        );
    compound
        .create_storage("/__recip_version1.0_#00000000")
        .unwrap();
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__recip_version1.0_#00000000/__substg1.0_3001001F",
        "Bob",
    );
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__recip_version1.0_#00000000/__substg1.0_39FE001F",
        "bob@example.com",
    );
    {
        let mut stream = compound
            .create_stream("/__recip_version1.0_#00000000/__substg1.0_0C150003")
            .unwrap();
        use std::io::Write as _;
        stream.write_all(&1u32.to_le_bytes()).unwrap();
    }
    compound
        .create_storage("/__recip_version1.0_#00000001")
        .unwrap();
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__recip_version1.0_#00000001/__substg1.0_3001001F",
        "Carol",
    );
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__recip_version1.0_#00000001/__substg1.0_39FE001F",
        "carol@example.com",
    );
    {
        let mut stream = compound
            .create_stream("/__recip_version1.0_#00000001/__substg1.0_0C150003")
            .unwrap();
        use std::io::Write as _;
        stream.write_all(&2u32.to_le_bytes()).unwrap();
    }
    compound
        .create_storage("/__attach_version1.0_#00000000")
        .unwrap();
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__attach_version1.0_#00000000/__substg1.0_3703001F",
        ".pdf",
    );
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__attach_version1.0_#00000000/__substg1.0_370E001F",
        "application/pdf",
    );
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__attach_version1.0_#00000000/__substg1.0_3712001F",
        "<report@cid>",
    );
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__attach_version1.0_#00000000/__substg1.0_3707001F",
        "report.pdf",
    );
    {
        let mut stream = compound
            .create_stream("/__attach_version1.0_#00000000/__substg1.0_37050003")
            .unwrap();
        use std::io::Write as _;
        stream.write_all(&1u32.to_le_bytes()).unwrap();
    }
    {
        let mut stream = compound
            .create_stream("/__attach_version1.0_#00000000/__substg1.0_0E200003")
            .unwrap();
        use std::io::Write as _;
        stream.write_all(&4096u32.to_le_bytes()).unwrap();
    }
    compound.flush().unwrap();
    drop(compound);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert_eq!(doc.title.as_deref(), Some("Quarterly Sync"));
    assert!(text.contains("From: Alice <alice@example.com>"));
    assert!(text.contains("To: Bob <bob@example.com>"));
    assert!(text.contains("Cc: Carol <carol@example.com>"));
    assert!(text.contains("Date: 2026-03-27 10:00:00 UTC"));
    assert!(text.contains("attachments"));
    assert!(text.contains("name=report.pdf"));
    assert!(text.contains("mime=application/pdf"));
    assert!(text.contains("content_id=report@cid"));
    assert!(text.contains("size=4096"));
    assert!(text.contains("method=by_value"));
    assert!(text.contains("alice@example.com"));
    assert!(text.contains("bob@example.com"));
    assert!(text.contains("recovered Outlook body"));
    assert!(doc.blocks.iter().any(|block| {
        block.label.as_deref() == Some("attachments")
            && block.attributes.get("attachment_count").map(String::as_str) == Some("1")
            && block
                .structured_payload
                .as_deref()
                .is_some_and(|payload| payload.contains("report.pdf"))
    }));
}

#[test]
fn parses_msg_html_body_fallback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mail-html.msg");
    let mut compound = cfb::create(&path).unwrap();
    msg::write_msg_utf16_stream(&mut compound, "/__substg1.0_0037001F", "HTML Body");
    msg::write_msg_utf16_stream(
        &mut compound,
        "/__substg1.0_1013001F",
        "<html><body><h1>Status</h1><p>Hello <b>HTML</b> body.</p></body></html>",
    );
    compound.flush().unwrap();
    drop(compound);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert_eq!(doc.title.as_deref(), Some("HTML Body"));
    assert!(text.contains("Status"));
    assert!(text.contains("Hello HTML body."));
}

#[test]
fn parses_msg_rtf_body_fallback() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("mail-rtf.msg");
    let mut compound = cfb::create(&path).unwrap();
    msg::write_msg_utf16_stream(&mut compound, "/__substg1.0_0037001F", "RTF Body");
    {
        let mut stream = compound.create_stream("/__substg1.0_10090102").unwrap();
        use std::io::Write as _;
        stream
            .write_all(br"{\rtf1\ansi Hello \b RTF\b0  body.\par Second line.}")
            .unwrap();
    }
    compound.flush().unwrap();
    drop(compound);

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    let text = doc.to_text();
    assert_eq!(doc.title.as_deref(), Some("RTF Body"));
    assert!(text.contains("RTF"));
    assert!(text.contains("Second line."));
}

#[test]
fn parsed_text_document_sets_block_locations() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "notes.rtf", "{\\rtf1\\ansi Hello \\par World}");
    let doc = parsed_text_document(
        &path,
        parse_rtf(&path).unwrap(),
        DocumentBlockKind::Paragraph,
    )
    .unwrap();
    assert!(doc.blocks.iter().enumerate().all(|(idx, block)| {
        block.location.as_ref().and_then(|loc| loc.ordinal) == Some(idx + 1)
    }));
}

#[test]
fn parsed_paged_text_document_sets_page_locations() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "scan.pdf", "placeholder");
    let doc = parsed_paged_text_document(
        &path,
        "Cover Page\n\nIntro\u{000c}Second Page\n\nMore text".to_string(),
        DocumentBlockKind::Paragraph,
    )
    .unwrap();

    assert!(doc.blocks.iter().any(|block| {
        block.content.contains("Cover Page")
            && block.location.as_ref().and_then(|loc| loc.page) == Some(1)
    }));
    assert!(doc.blocks.iter().any(|block| {
        block.content.contains("Second Page")
            && block.location.as_ref().and_then(|loc| loc.page) == Some(2)
    }));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("page 1: heading")));
}

#[test]
fn split_paged_text_ignores_empty_form_feed_sections() {
    let pages = split_paged_text("Page 1\u{000c}\u{000c}Page 2");
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0], "Page 1");
    assert_eq!(pages[1], "Page 2");
}

#[test]
fn normalize_paged_text_boundaries_joins_hyphenated_page_breaks() {
    let pages = normalize_paged_text_boundaries(vec![
        "Page-aware loca-".to_string(),
        "tors improve review speed.\nSecond line.".to_string(),
    ]);

    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0], "Page-aware locators improve review speed.");
    assert_eq!(pages[1], "Second line.");
}

#[test]
fn normalize_paged_text_boundaries_joins_sentence_continuations() {
    let pages = normalize_paged_text_boundaries(vec![
        "Authentication flow continues".to_string(),
        "through session validation.\nNext section starts Here.".to_string(),
    ]);

    assert_eq!(pages.len(), 2);
    assert_eq!(
        pages[0],
        "Authentication flow continues through session validation."
    );
    assert_eq!(pages[1], "Next section starts Here.");
}

#[test]
fn normalize_paged_text_pages_marks_cross_page_continuations() {
    let pages = normalize_paged_text_pages(vec![
        "Authentication flow continues".to_string(),
        "through session validation.\nNext section starts Here.".to_string(),
    ]);

    assert_eq!(pages.len(), 2);
    assert!(pages[0].continued_to_next_page);
    assert!(pages[1].continued_from_previous_page);
    assert_eq!(
        pages[0].text,
        "Authentication flow continues through session validation."
    );
    assert_eq!(pages[1].text, "Next section starts Here.");
}

#[test]
fn normalize_paged_text_boundaries_joins_multi_line_sentence_continuations() {
    let pages = normalize_paged_text_boundaries(vec![
        "Authentication flow continues".to_string(),
        "through session validation\nacross services.\nNext section starts Here.".to_string(),
    ]);

    assert_eq!(pages.len(), 2);
    assert_eq!(
        pages[0],
        "Authentication flow continues through session validation across services."
    );
    assert_eq!(pages[1], "Next section starts Here.");
}

#[test]
fn normalize_paged_text_boundaries_carries_multi_column_rows_across_pages() {
    let pages = normalize_paged_text_boundaries(vec![
        "Auth flow keeps page labels         Parser metadata stays typed".to_string(),
        "through long reviews.               across OCR boundaries.\nNext page intro starts here."
            .to_string(),
    ]);

    assert_eq!(pages.len(), 2);
    assert!(pages[0].contains("Auth flow keeps page labels"));
    assert!(pages[0].contains("through long reviews."));
    assert!(pages[0].contains("across OCR boundaries."));
    assert_eq!(pages[1], "Next page intro starts here.");
}

#[test]
fn page_boundary_multi_column_carry_prefers_open_column_alignment_for_ragged_rows() {
    let current = "Authentication review remains stable today.      OCR metadata stays deterministic across releases.          Search locators continue through cited passages";
    let next = "Fresh page introductions can start cleanly.          with fewer reviewer jumps.";

    assert_eq!(page_boundary_multi_column_carry(current, next), Some(1));
}

#[test]
fn normalize_paged_text_boundaries_carries_ragged_multi_column_rows_across_pages() {
    let pages = normalize_paged_text_boundaries(vec![
            "Authentication review remains stable today.      OCR metadata stays deterministic across releases.          Search locators continue through cited passages".to_string(),
            "Fresh page introductions can start cleanly.          with fewer reviewer jumps.\nStandalone next-page summary begins here.".to_string(),
        ]);

    assert_eq!(pages.len(), 2);
    assert!(pages[0].contains("Search locators continue through cited passages"));
    assert!(pages[0].contains("with fewer reviewer jumps."));
    assert_eq!(pages[1], "Standalone next-page summary begins here.");
}

#[test]
fn parsed_paged_text_document_marks_boundary_blocks_as_continued() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "scan.pdf", "placeholder");
    let doc = parsed_paged_text_document(
        &path,
        "Authentication flow continues\u{000c}through session validation.\n\nNext heading"
            .to_string(),
        DocumentBlockKind::Paragraph,
    )
    .unwrap();

    let first = &doc.blocks[0];
    let second = &doc.blocks[1];
    assert!(first
        .location
        .as_ref()
        .is_some_and(|loc| loc.continued_to_next_page));
    assert!(second
        .location
        .as_ref()
        .is_some_and(|loc| loc.continued_from_previous_page));
}

#[test]
fn infer_open_columns_from_rows_keeps_columns_missing_from_last_row_open() {
    let rows = vec![
            split_aligned_columns_with_gaps(
                "Auth flow keeps page labels     Parser metadata stays typed                     Search ranking prefers labeled",
            ),
            split_aligned_columns_with_gaps(
                "Navigation remains stable.        across OCR boundaries",
            ),
        ];
    let reference_gaps = infer_reference_column_gaps(&rows, 3);
    let open = infer_open_columns_from_rows(&rows, 3, reference_gaps.as_deref());

    assert!(open.contains(&1));
    assert!(open.contains(&2));
    assert!(!open.contains(&0));
}

#[test]
fn best_open_column_alignment_prefers_hitting_open_columns_over_gap_score() {
    let row = AlignedTextRow {
        cells: vec![
            "Fresh page introductions can start cleanly.".to_string(),
            "with fewer reviewer jumps.".to_string(),
        ],
        gaps: vec![10],
    };

    let aligned = best_open_column_alignment(&row, 3, &[6, 10], &[2]).unwrap();
    assert!(aligned[2].as_deref() == Some("with fewer reviewer jumps."));
    assert!(aligned[0].is_none() || aligned[1].is_none());
}

#[test]
fn page_boundary_carry_skips_multi_column_lines() {
    let carry = page_boundary_carry(
        "Auth flow keeps page labels         Parser metadata stays typed",
        "through long reviews.               across OCR boundaries.",
    );

    assert!(carry.is_none());
}

#[test]
fn paged_text_blocks_promote_aligned_text_to_table() {
    let blocks = paged_text_blocks(
        "Name    Score    Status\nAlice   42       Active\nBob     99       Paused",
        DocumentBlockKind::Paragraph,
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Table);
    assert_eq!(blocks[0].label.as_deref(), Some("table"));
    assert!(blocks[0].content.contains("Name\tScore\tStatus"));
    assert!(blocks[0].content.contains("Bob\t99\tPaused"));
    assert_eq!(
        blocks[0].attributes.get("row_count").map(String::as_str),
        Some("3")
    );
    assert!(blocks[0]
            .structured_payload
            .as_deref()
            .is_some_and(|payload| payload.contains("\"rows\":[[\"Name\",\"Score\",\"Status\"],[\"Alice\",\"42\",\"Active\"],[\"Bob\",\"99\",\"Paused\"]]")));
}

#[test]
fn paged_text_blocks_promote_markdown_pipe_table() {
    let blocks = paged_text_blocks(
            "| Name | Score | Status |\n| ---- | ----: | ------ |\n| Alice | 42 | Active |\n| Bob | 99 | Paused |",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Table);
    assert_eq!(blocks[0].label.as_deref(), Some("table"));
    assert!(blocks[0].content.contains("Name\tScore\tStatus"));
    assert!(blocks[0].content.contains("Alice\t42\tActive"));
    assert!(!blocks[0].content.contains("----"));
}

#[test]
fn paged_text_blocks_promote_ocr_pipe_table() {
    let blocks = paged_text_blocks(
        "Name ¦ Score ¦ Status\nAlice ¦ 42 ¦ Active\nBob ¦ 99 ¦ Paused",
        DocumentBlockKind::Paragraph,
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Table);
    assert!(blocks[0].content.contains("Bob\t99\tPaused"));
}

#[test]
fn paged_text_blocks_reflow_two_column_prose() {
    let blocks = paged_text_blocks(
            "Authentication flow starts after      The parser now emits page-aware\nlogin and continues through             locators for every section and\nsession validation across services.     match so search review is easier.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Paragraph);
    assert_eq!(blocks[1].kind, DocumentBlockKind::Paragraph);
    assert!(blocks[0]
        .content
        .contains("Authentication flow starts after login"));
    assert!(blocks[0]
        .content
        .contains("session validation across services."));
    assert!(blocks[1]
        .content
        .contains("The parser now emits page-aware locators"));
    assert!(blocks[1].content.contains("search review is easier."));
}

#[test]
fn paged_text_blocks_keep_two_column_numeric_data_as_table() {
    let blocks = paged_text_blocks(
        "Name    Score\nAlice   42\nBob     99",
        DocumentBlockKind::Paragraph,
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Table);
    assert!(blocks[0].content.contains("Alice\t42"));
}

#[test]
fn paged_text_blocks_reflow_two_column_heading_and_body() {
    let blocks = paged_text_blocks(
            "1. Overview                         2. Results\nParser changes improve locator         Search metadata now includes\nquality for documents.                 richer block context.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Heading);
    assert_eq!(blocks[1].kind, DocumentBlockKind::Section);
    assert_eq!(blocks[2].kind, DocumentBlockKind::Heading);
    assert_eq!(blocks[3].kind, DocumentBlockKind::Section);
    assert_eq!(blocks[1].label.as_deref(), Some("1. Overview"));
    assert_eq!(blocks[3].label.as_deref(), Some("2. Results"));
}

#[test]
fn paged_text_blocks_reflow_two_column_preserves_paragraph_breaks() {
    let blocks = paged_text_blocks(
            "Authentication flow starts after    Parser metadata now tracks OCR\nlogin and session checks.              provider usage across files.\nNew requests reuse the same token.     Search locators point at page labels.\n\n",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Paragraph);
    assert_eq!(blocks[1].kind, DocumentBlockKind::Paragraph);
    assert_eq!(blocks[2].kind, DocumentBlockKind::Paragraph);
    assert_eq!(blocks[3].kind, DocumentBlockKind::Paragraph);
    assert!(blocks[0]
        .content
        .contains("Authentication flow starts after login"));
    assert!(blocks[1]
        .content
        .contains("New requests reuse the same token."));
    assert!(blocks[2]
        .content
        .contains("Parser metadata now tracks OCR backend"));
    assert!(blocks[3]
        .content
        .contains("Search locators point at page labels."));
}

#[test]
fn paged_text_blocks_reflow_three_column_prose() {
    let blocks = paged_text_blocks(
            "Auth flow now keeps page labels.    Parser OCR fallback is deterministic.    Search ranking prefers labeled blocks.\nSessions resume with fewer misses.     PDFs expose richer section boundaries.   Reviewers can jump to cited passages.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 6);
    assert!(blocks[0]
        .content
        .contains("Auth flow now keeps page labels."));
    assert!(blocks[1]
        .content
        .contains("Sessions resume with fewer misses."));
    assert!(blocks[2]
        .content
        .contains("Parser OCR fallback is deterministic."));
    assert!(blocks[3]
        .content
        .contains("PDFs expose richer section boundaries."));
    assert!(blocks[4]
        .content
        .contains("Search ranking prefers labeled blocks."));
    assert!(blocks[5]
        .content
        .contains("Reviewers can jump to cited passages."));
}

#[test]
fn paged_text_blocks_reflow_three_column_prose_with_missing_tail_cell() {
    let blocks = paged_text_blocks(
            "Auth flow now keeps page labels.    Parser OCR fallback is deterministic.    Search ranking prefers labeled blocks.\nSessions resume with fewer misses.     PDFs expose richer section boundaries.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 5);
    assert!(blocks[0]
        .content
        .contains("Auth flow now keeps page labels."));
    assert!(blocks
        .iter()
        .any(|block| block.content.contains("Sessions resume with fewer misses.")));
    assert!(blocks.iter().any(|block| block
        .content
        .contains("Parser OCR fallback is deterministic.")));
    assert!(blocks.iter().any(|block| block
        .content
        .contains("PDFs expose richer section boundaries.")));
    assert!(blocks.iter().any(|block| block
        .content
        .contains("Search ranking prefers labeled blocks.")));
}

#[test]
fn paged_text_blocks_reflow_three_column_prose_with_missing_middle_cell() {
    let blocks = paged_text_blocks(
            "Auth flow now keeps page labels.    Parser OCR fallback is deterministic.    Search ranking prefers labeled blocks.\nSessions resume with fewer misses.                                             Reviewers can jump to cited passages.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 5);
    assert!(blocks
        .iter()
        .any(|block| block.content.contains("Auth flow now keeps page labels.")));
    assert!(blocks
        .iter()
        .any(|block| block.content.contains("Sessions resume with fewer misses.")));
    assert!(blocks.iter().any(|block| block
        .content
        .contains("Parser OCR fallback is deterministic.")));
    assert!(blocks.iter().any(|block| block
        .content
        .contains("Search ranking prefers labeled blocks.")));
    assert!(blocks.iter().any(|block| block
        .content
        .contains("Reviewers can jump to cited passages.")));
}

#[test]
fn paged_text_blocks_reflow_dehyphenates_column_word_breaks() {
    let blocks = paged_text_blocks(
            "Page-aware loca-                    OCR meta-\ntors improve review speed across      data remains visible in parser\nreview workflows and audits.          runtime traces for operators.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 2);
    assert!(blocks[0]
        .content
        .contains("Page-aware locators improve review speed across review workflows and audits."));
    assert!(blocks[1]
        .content
        .contains("OCR metadata remains visible in parser runtime traces for operators."));
    assert!(!blocks[0].content.contains("loca- tors"));
    assert!(!blocks[1].content.contains("meta- data"));
}

#[test]
fn paged_text_blocks_keep_three_column_numeric_data_as_table() {
    let blocks = paged_text_blocks(
        "Name    Score    Rank\nAlice   42       1\nBob     35       2",
        DocumentBlockKind::Paragraph,
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Table);
    assert!(blocks[0].content.contains("Alice\t42\t1"));
}

#[test]
fn paged_text_blocks_preserve_paragraphs_when_not_tabular() {
    let blocks = paged_text_blocks(
        "This is a normal paragraph with extra spacing.\nAnother sentence follows.",
        DocumentBlockKind::Paragraph,
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Paragraph);
}

#[test]
fn paged_text_blocks_split_heading_from_following_body() {
    let blocks = paged_text_blocks(
            "1. Overview\nThis section explains the parser changes.\nIt includes page-aware extraction.",
            DocumentBlockKind::Paragraph,
        );

    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Heading);
    assert_eq!(blocks[0].content, "1. Overview");
    assert_eq!(blocks[1].kind, DocumentBlockKind::Section);
    assert_eq!(blocks[1].label.as_deref(), Some("1. Overview"));
    assert!(blocks[1]
        .content
        .contains("This section explains the parser changes."));
}

#[test]
fn paged_text_blocks_keep_short_headings_as_single_heading_block() {
    let blocks = paged_text_blocks("Appendix A", DocumentBlockKind::Paragraph);

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, DocumentBlockKind::Heading);
    assert_eq!(blocks[0].content, "Appendix A");
}

#[test]
fn label_paged_block_prefixes_existing_section_labels() {
    let block = label_paged_block(
        DocumentBlock::new(DocumentBlockKind::Section, Some("1. Overview"), "Body text"),
        2,
        1,
    );

    assert_eq!(block.label.as_deref(), Some("page 2: 1. Overview"));
}

#[test]
fn parses_xml_document_into_structured_blocks() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "sample.xml",
            "<root><title>Spec</title><section><p>Intro text</p><p>More text</p></section><table><tr><td>A</td><td>B</td></tr></table></root>",
        );
    let doc = parse_xml_document(&path).unwrap();
    assert_eq!(doc.title.as_deref(), Some("Spec"));
    assert!(doc.blocks.iter().enumerate().all(|(idx, block)| {
        block.location.as_ref().and_then(|loc| loc.ordinal) == Some(idx + 1)
    }));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Paragraph));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Table && block.content.contains("A\tB")));
    assert!(doc.to_text().contains("Intro text"));
}

#[test]
fn composite_document_parser_supports_xml_family_extensions() {
    let parser = CompositeDocumentParser::default();
    assert!(parser.supported_extensions().contains(&"svg"));
    assert!(parser.supported_extensions().contains(&"opml"));
    assert!(parser.supported_extensions().contains(&"fb2"));
    assert!(parser.supported_extensions().contains(&"docbook"));
    assert!(parser.supported_extensions().contains(&"dbk"));
    assert!(parser.supported_extensions().contains(&"jats"));
    assert!(parser.supported_extensions().contains(&"tei"));
    assert!(parser.supported_extensions().contains(&"nxml"));
    assert!(parser.supported_extensions().contains(&"dita"));
    assert!(parser.supported_extensions().contains(&"ditamap"));
}

#[test]
fn parses_svg_via_xml_path() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "diagram.svg",
        r#"<svg xmlns="urn:test"><title>Architecture</title><text>Agentic Parse</text><text>CompositeDocumentParser</text></svg>"#,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("Architecture"));
    assert!(doc.to_text().contains("Agentic Parse"));
    assert!(doc.to_text().contains("CompositeDocumentParser"));
}

#[test]
fn parses_docx_without_matching_extension_via_probe() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "upload.bin",
        &[
            (
                "docProps/core.xml",
                r#"<cp:coreProperties xmlns:cp="urn:test" xmlns:dc="urn:test-dc"><dc:title>Detected Report</dc:title></cp:coreProperties>"#,
            ),
            (
                "word/document.xml",
                r#"<w:document xmlns:w="urn:test"><w:body><w:p><w:r><w:t>Hello Detector</w:t></w:r></w:p></w:body></w:document>"#,
            ),
        ],
    );

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("Detected Report"));
    assert!(doc.to_text().contains("Hello Detector"));
}

#[test]
fn parses_svg_without_extension_via_probe() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "diagram",
        r#"<svg xmlns="urn:test"><title>Detected SVG</title><text>Probe Path</text></svg>"#,
    );

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("Detected SVG"));
    assert!(doc.to_text().contains("Probe Path"));
}

#[test]
fn parses_flat_odf_without_extension_via_probe() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "document",
        r#"<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" office:mimetype="application/vnd.oasis.opendocument.text"><office:body><office:text><text:p xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">Detected FODT body</text:p></office:text></office:body></office:document>"#,
    );

    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc.to_text().contains("Detected FODT body"));
}

#[test]
fn parses_opml_via_xml_path() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "feeds.opml",
        r#"<opml version="2.0"><head><title>Reading List</title></head><body><outline text="A3S"/><outline text="Kreuzberg"/></body></opml>"#,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("Reading List"));
    assert!(doc.to_text().contains("A3S"));
}

#[test]
fn parses_docbook_via_xml_path() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "guide.docbook",
        r#"<article><title>DocBook Guide</title><section><title>Intro</title><para>Hello DocBook</para></section></article>"#,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("DocBook Guide"));
    assert!(doc.to_text().contains("Hello DocBook"));
}

#[test]
fn parses_tei_via_xml_path() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "paper.tei",
        r#"<TEI><teiHeader><fileDesc><titleStmt><title>TEI Paper</title></titleStmt></fileDesc></teiHeader><text><body><p>Hello TEI</p></body></text></TEI>"#,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("TEI Paper"));
    assert!(doc.to_text().contains("Hello TEI"));
}

#[test]
fn parses_ipynb_notebook() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "analysis.ipynb",
        r##"{
              "metadata": {
                "kernelspec": { "display_name": "Python 3" },
                "language_info": { "name": "python" }
              },
              "cells": [
                { "cell_type": "markdown", "source": ["# Title\n", "Notebook intro"] },
                { "cell_type": "code", "source": ["print('hello')\n"] }
              ]
            }"##,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("notebook")
            && block.content.contains("kernel=Python 3")
            && block
                .structured_payload
                .as_deref()
                .is_some_and(|payload| payload.contains("\"kernel\":\"Python 3\""))));
    assert!(doc.blocks.iter().any(|block| {
        block.label.as_deref() == Some("markdown cell 1")
            && block.attributes.get("cell_type").map(String::as_str) == Some("markdown")
    }));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.kind == DocumentBlockKind::Code
            && block.content.contains("print('hello')")
            && block.attributes.get("cell_type").map(String::as_str) == Some("code")));
}

#[test]
fn parses_ris_citation() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "refs.ris",
            "TY  - JOUR\nTI  - Agentic Parsing\nAU  - Lin, Roy\nPY  - 2026\nAB  - Structured parsing.\nER  -\n",
        );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("Agentic Parsing"));
    assert!(doc.to_text().contains("AU=Lin, Roy"));
    assert!(doc.to_text().contains("AB=Structured parsing."));
    assert!(doc.blocks.iter().any(|block| {
        block.attributes.get("citation_format").map(String::as_str) == Some("ris")
            && block
                .structured_payload
                .as_deref()
                .is_some_and(|payload| payload.contains("\"TI\":\"Agentic Parsing\""))
    }));
}

#[test]
fn parses_endnote_enw_citation() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "refs.enw",
            "%0 Journal Article\n%T CompositeDocumentParser ENW\n%A Roy Lin\n%D 2026\n%J A3S Journal\n%X Structured citation parsing.\n",
        );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("CompositeDocumentParser ENW"));
    assert!(doc.to_text().contains("%A=Roy Lin"));
    assert!(doc.to_text().contains("%J=A3S Journal"));
}

#[test]
fn parses_nbib_citation() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "pubmed.nbib",
            "PMID- 42\nTI  - CompositeDocumentParser Alignment\nFAU - Roy Lin\nAB  - Citation parsing.\n",
        );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(
        doc.title.as_deref(),
        Some("CompositeDocumentParser Alignment")
    );
    assert!(doc.to_text().contains("PMID=42"));
    assert!(doc.to_text().contains("FAU=Roy Lin"));
}

#[test]
fn parses_bibtex_citation() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
            &dir,
            "refs.bib",
            "@article{a3s2026,\n  title = {CompositeDocumentParser Alignment},\n  author = {Roy Lin},\n  year = {2026},\n  journal = {A3S Journal}\n}\n",
        );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(
        doc.title.as_deref(),
        Some("CompositeDocumentParser Alignment")
    );
    assert!(doc.to_text().contains("author=Roy Lin"));
    assert!(doc.to_text().contains("journal=A3S Journal"));
    assert!(doc.blocks.iter().any(|block| {
        block.attributes.get("citation_format").map(String::as_str) == Some("bib")
            && block.structured_payload.as_deref().is_some_and(|payload| {
                payload.contains("\"title\":\"CompositeDocumentParser Alignment\"")
            })
    }));
}

#[test]
fn parses_csl_json_citation() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "refs.csl",
        r#"[
              {
                "id": "a3s-2026",
                "type": "article-journal",
                "title": "CompositeDocumentParser Alignment",
                "author": [
                  { "given": "Roy", "family": "Lin" },
                  { "literal": "A3S Lab" }
                ],
                "container-title": "A3S Journal",
                "issued": { "date-parts": [[2026, 3, 27]] },
                "abstract": "Structured metadata extraction",
                "keyword": ["parser", "agentic"],
                "DOI": "10.1234/a3s.2026.1"
              }
            ]"#,
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(
        doc.title.as_deref(),
        Some("CompositeDocumentParser Alignment")
    );
    let text = doc.to_text();
    assert!(text.contains("author=Roy Lin; A3S Lab"));
    assert!(text.contains("issued=2026-3-27"));
    assert!(text.contains("DOI=10.1234/a3s.2026.1"));
    assert!(doc.blocks.iter().any(|block| {
        block.attributes.get("citation_format").map(String::as_str) == Some("csl")
            && block.structured_payload.as_deref().is_some_and(|payload| {
                payload.contains("\"title\":\"CompositeDocumentParser Alignment\"")
            })
    }));
}

#[test]
fn parses_icalendar_event() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "roadmap.ics",
        concat!(
            "BEGIN:VCALENDAR\n",
            "VERSION:2.0\n",
            "PRODID:-//A3S//Code//EN\n",
            "X-WR-CALNAME:A3S Roadmap\n",
            "BEGIN:VEVENT\n",
            "UID:event-1\n",
            "SUMMARY:CompositeDocumentParser Sync\n",
            "DTSTART:20260327T090000Z\n",
            "DTEND:20260327T100000Z\n",
            "LOCATION:Shanghai\n",
            "DESCRIPTION:Align with Kreuzberg\\nShip improvements\n",
            "ATTENDEE:roy@example.com\n",
            "END:VEVENT\n",
            "END:VCALENDAR\n"
        ),
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("A3S Roadmap"));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("calendar")
            && block.content.contains("PRODID=-//A3S//Code//EN")));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("vevent 1")
            && block
                .content
                .contains("SUMMARY=CompositeDocumentParser Sync")
            && block.content.contains("LOCATION=Shanghai")
            && block
                .content
                .contains("DESCRIPTION=Align with Kreuzberg\nShip improvements")));
}

#[test]
fn parses_vcard_contact() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "roy.vcf",
        concat!(
            "BEGIN:VCARD\n",
            "VERSION:3.0\n",
            "FN:Roy Lin\n",
            "ORG:A3S Lab\n",
            "TITLE:Founder\n",
            "EMAIL:roy@example.com\n",
            "TEL:+86-12345678\n",
            "ADR:Shanghai;Pudong;Zhangjiang;;;\n",
            "NOTE:Agentic\\nParser\n",
            "END:VCARD\n"
        ),
    );
    let doc = CompositeDocumentParser::default()
        .parse_document(&path)
        .unwrap();
    assert_eq!(doc.title.as_deref(), Some("Roy Lin"));
    assert!(doc
        .blocks
        .iter()
        .any(|block| block.label.as_deref() == Some("contact 1")
            && block.content.contains("ORG=A3S Lab")
            && block.content.contains("EMAIL=roy@example.com")
            && block.content.contains("ADR=Shanghai, Pudong, Zhangjiang")
            && block.content.contains("NOTE=Agentic\nParser")));
}

#[test]
fn composite_document_parser_supports_calendar_and_vcard_extensions() {
    let parser = CompositeDocumentParser::default();
    assert!(parser.supported_extensions().contains(&"ics"));
    assert!(parser.supported_extensions().contains(&"ical"));
    assert!(parser.supported_extensions().contains(&"ifb"));
    assert!(parser.supported_extensions().contains(&"vcf"));
    assert!(parser.supported_extensions().contains(&"vcard"));
}

#[test]
fn composite_document_parser_supports_additional_citation_and_presentation_extensions() {
    let parser = CompositeDocumentParser::default();
    assert!(parser.supported_extensions().contains(&"txt"));
    assert!(parser.supported_extensions().contains(&"md"));
    assert!(parser.supported_extensions().contains(&"json"));
    assert!(parser.supported_extensions().contains(&"yaml"));
    assert!(parser.supported_extensions().contains(&"yml"));
    assert!(parser.supported_extensions().contains(&"toml"));
    assert!(parser.supported_extensions().contains(&"jsonl"));
    assert!(parser.supported_extensions().contains(&"ndjson"));
    assert!(parser.supported_extensions().contains(&"csv"));
    assert!(parser.supported_extensions().contains(&"enw"));
    assert!(parser.supported_extensions().contains(&"csl"));
    assert!(parser.supported_extensions().contains(&"pptm"));
    assert!(parser.supported_extensions().contains(&"ppsx"));
    assert!(parser.supported_extensions().contains(&"potx"));
    assert!(parser.supported_extensions().contains(&"potm"));
    assert!(parser.supported_extensions().contains(&"docm"));
    assert!(parser.supported_extensions().contains(&"dotx"));
    assert!(parser.supported_extensions().contains(&"dotm"));
    assert!(parser.supported_extensions().contains(&"xltx"));
    assert!(parser.supported_extensions().contains(&"xltm"));
    assert!(parser.supported_extensions().contains(&"xlam"));
    assert!(parser.supported_extensions().contains(&"fodt"));
    assert!(parser.supported_extensions().contains(&"fods"));
    assert!(parser.supported_extensions().contains(&"fodp"));
    assert!(parser.supported_extensions().contains(&"zip"));
    assert!(parser.supported_extensions().contains(&"tar"));
    assert!(parser.supported_extensions().contains(&"gz"));
    assert!(parser.supported_extensions().contains(&"emlx"));
    assert!(parser.supported_extensions().contains(&"mbox"));
    assert!(parser.supported_extensions().contains(&"msg"));
}

#[test]
fn parses_multipart_eml_with_html_and_quoted_printable() {
    let dir = TempDir::new().unwrap();
    let path = write_file(
        &dir,
        "multipart.eml",
        concat!(
            "Subject: Multipart Test\n",
            "From: sender@example.com\n",
            "To: receiver@example.com\n",
            "Content-Type: multipart/alternative; boundary=\"abc123\"\n",
            "\n",
            "--abc123\n",
            "Content-Type: text/plain; charset=utf-8\n",
            "Content-Transfer-Encoding: quoted-printable\n",
            "\n",
            "Hello=20World=21\n",
            "--abc123\n",
            "Content-Type: text/html; charset=utf-8\n",
            "\n",
            "<html><body><p>Ignored HTML fallback</p></body></html>\n",
            "--abc123--\n"
        ),
    );
    let text = parse_eml(&path).unwrap().to_text();
    assert!(text.contains("Subject: Multipart Test"));
    assert!(text.contains("Hello World!"));
}

#[test]
fn strips_rtf_control_words() {
    let text = strip_rtf(r"{\rtf1\ansi Hello \par World}");
    assert!(text.contains("Hello"));
    assert!(text.contains("World"));
}

#[test]
fn pdf_ocr_heuristic_detects_weak_text() {
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    assert!(should_attempt_pdf_ocr("", &config));
    assert!(should_attempt_pdf_ocr("%%% ---", &config));
    assert!(!should_attempt_pdf_ocr(
            "This is a reasonably healthy PDF text extraction with enough words and letters to avoid OCR fallback across multiple paragraphs and sections of the document body.",
            &config
        ));
}

#[test]
fn pdf_ocr_fallback_uses_provider_when_text_is_weak() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.pdf", "not-a-real-pdf");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = MockOcrProvider {
        text: Some("OCR recovered text".to_string()),
    };

    let result = maybe_run_pdf_ocr(&path, String::new(), &config, Some(&provider)).unwrap();
    assert_eq!(result.text, "OCR recovered text");
    assert_eq!(result.mode, PdfOcrMode::Used);
    assert_eq!(result.provider_name.as_deref(), Some("mock-ocr"));
}

#[test]
fn pdf_ocr_fallback_preserves_extracted_text_without_provider() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.pdf", "not-a-real-pdf");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = maybe_run_pdf_ocr(&path, "weak".to_string(), &config, None).unwrap();
    assert_eq!(result.text, "weak");
    assert_eq!(result.mode, PdfOcrMode::Fallback);
}

#[test]
fn composite_document_parser_can_hold_ocr_provider() {
    let parser = CompositeDocumentParser::with_config_and_ocr(
        crate::config::DocumentParserConfig::default(),
        Arc::new(MockOcrProvider { text: None }),
    );
    assert!(parser.ocr_provider().is_some());
    let capabilities = parser.ocr_provider_capabilities().unwrap();
    assert_eq!(capabilities.formats, vec!["pdf".to_string()]);
    assert_eq!(capabilities.model.as_deref(), Some("kimi-vision"));
}

#[test]
fn ocr_capabilities_supports_format_case_insensitively() {
    let caps = DocumentOcrCapabilities::new(["PDF", "Image"]);
    assert!(caps.supports_format(DocumentOcrFormat::Pdf));
    assert!(caps.supports_format(DocumentOcrFormat::Image));
    assert!(!caps.supports_format(DocumentOcrFormat::Docx));
}

#[test]
fn composite_document_parser_reports_its_own_name_for_unsupported_extensions() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.bin", "raw");
    let parser = CompositeDocumentParser::default();
    let err = parser.parse_document(&path).unwrap_err().to_string();
    assert!(err.contains("composite document parser"));
}

#[test]
fn pdf_ocr_metadata_block_is_emitted_when_ocr_is_used() {
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: Some("moonshot/kimi-vl".to_string()),
            prompt: Some("Read the scanned PDF accurately".to_string()),
            max_images: 4,
            dpi: 180,
        }),
        ..Default::default()
    };
    let metadata = build_ocr_metadata_block(
            &config,
            DocumentOcrFormat::Pdf,
            &OcrResult {
                text: "recovered".to_string(),
                mode: PdfOcrMode::Used,
                provider_name: Some("mock-ocr".to_string()),
                pages: vec![
                    DocumentOcrPageResult {
                        page: Some(1),
                        text: "p1".to_string(),
                        language: Some("en".to_string()),
                        confidence_score_percent: Some(93),
                    },
                    DocumentOcrPageResult {
                        page: Some(2),
                        text: "p2".to_string(),
                        language: Some("en".to_string()),
                        confidence_score_percent: Some(93),
                    },
                ],
                page_count: Some(2),
                language: Some("en".to_string()),
                confidence_score_percent: Some(93),
                model: Some("kimi-vision".to_string()),
                structured_payload: Some(
                    "{\"text\":\"recovered\",\"pages\":[{\"page\":1,\"text\":\"p1\"},{\"page\":2,\"text\":\"p2\"}],\"language\":\"en\",\"confidence_score_percent\":93,\"model\":\"kimi-vision\"}".to_string(),
                ),
            },
        )
        .unwrap();

    assert_eq!(metadata.kind, DocumentBlockKind::Metadata);
    assert_eq!(metadata.label.as_deref(), Some("ocr"));
    assert!(metadata.content.contains("format=pdf"));
    assert!(metadata.content.contains("provider=mock-ocr"));
    assert!(metadata.content.contains("model=kimi-vision"));
    assert!(metadata.content.contains("prompt=set"));
    assert!(metadata.content.contains("dpi=180"));
    assert!(metadata.content.contains("page_count=2"));
    assert!(metadata.content.contains("language=en"));
    assert!(metadata.content.contains("confidence_score_percent=93"));
    assert!(metadata
        .structured_payload
        .as_deref()
        .is_some_and(|payload: &str| payload.contains("\"pages\"")));
}

#[test]
fn extract_document_runtime_metadata_parses_ocr_block() {
    let doc = ParsedDocument {
            title: Some("scan.pdf".to_string()),
            blocks: vec![
                DocumentBlock::new(
                    DocumentBlockKind::Metadata,
                    Some("ocr"),
                    "mode=ocr\nformat=pdf\nprovider=mock-ocr\nmodel=moonshot/kimi-vl\nprompt=set\nmax_images=4\ndpi=180\npage_count=2\nlanguage=en\nconfidence_score_percent=93",
                )
                .with_structured_payload(
                    "{\"text\":\"Recovered text\",\"pages\":[{\"page\":1,\"text\":\"p1\"},{\"page\":2,\"text\":\"p2\"}],\"language\":\"en\",\"confidence_score_percent\":93,\"model\":\"moonshot/kimi-vl\"}",
                ),
                DocumentBlock::new(
                    DocumentBlockKind::Paragraph,
                    Some("body"),
                    "Recovered text",
                ),
            ],
            metadata: None,
        };

    let metadata = extract_document_runtime_metadata(&doc).unwrap();
    let ocr = metadata.ocr.unwrap();
    assert!(ocr.used);
    assert_eq!(ocr.format.as_deref(), Some("pdf"));
    assert_eq!(ocr.provider.as_deref(), Some("mock-ocr"));
    assert_eq!(ocr.model.as_deref(), Some("moonshot/kimi-vl"));
    assert_eq!(ocr.max_images, Some(4));
    assert_eq!(ocr.dpi, Some(180));
    assert_eq!(ocr.page_count, Some(2));
    assert_eq!(ocr.language.as_deref(), Some("en"));
    assert_eq!(ocr.confidence_score_percent, Some(93));
}

#[test]
fn pdf_ocr_can_use_unified_request_entrypoint() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.pdf", "not-a-real-pdf");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = RequestOnlyOcrProvider {
        text: Some("Unified OCR text".to_string()),
    };

    let result = maybe_run_pdf_ocr(&path, String::new(), &config, Some(&provider)).unwrap();
    assert_eq!(result.text, "Unified OCR text");
    assert_eq!(result.mode, PdfOcrMode::Used);
    assert_eq!(result.provider_name.as_deref(), Some("request-only-ocr"));
    assert_eq!(result.pages.len(), 1);
    assert_eq!(result.page_count, Some(1));
    assert_eq!(result.language.as_deref(), Some("en"));
    assert_eq!(result.confidence_score_percent, Some(87));
    assert_eq!(result.model.as_deref(), Some("request-vision"));
    assert!(result
        .structured_payload
        .as_deref()
        .is_some_and(|payload: &str| payload.contains("\"page\":1")));
}

#[test]
fn pdf_ocr_skips_provider_without_pdf_capability() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.pdf", "not-a-real-pdf");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result =
        maybe_run_pdf_ocr(&path, String::new(), &config, Some(&ImageOnlyOcrProvider)).unwrap();
    assert_eq!(result.text, "");
    assert_eq!(result.mode, PdfOcrMode::Fallback);
    assert_eq!(result.provider_name.as_deref(), Some("image-only-ocr"));
}

#[test]
fn image_ocr_uses_provider_when_enabled() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.png", "not-a-real-image");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: Some("moonshot/kimi-vl".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = RequestOnlyOcrProvider {
        text: Some("Detected image text".to_string()),
    };

    let doc = parse_image_document(&path, &config, Some(&provider)).unwrap();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr"));
    assert!(doc.blocks[0].content.contains("format=image"));
    assert!(doc.to_text().contains("Detected image text"));
    assert!(doc.blocks.iter().any(|block| {
        block.content.contains("Detected image text")
            && block.attributes.get("ocr_page").map(String::as_str) == Some("1")
            && block.attributes.get("ocr_language").map(String::as_str) == Some("en")
            && block
                .attributes
                .get("ocr_confidence_score_percent")
                .map(String::as_str)
                == Some("87")
    }));
}

#[test]
fn image_ocr_uses_cache_after_first_request() {
    let dir = TempDir::new().unwrap();
    let cache_dir = dir.path().join("ocr-cache");
    let path = write_file(&dir, "cached.png", "not-a-real-image");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        cache: Some(crate::config::DocumentCacheConfig {
            enabled: true,
            directory: Some(cache_dir),
        }),
    };
    let calls = Arc::new(Mutex::new(0usize));
    let provider = CountingRequestOcrProvider {
        calls: Arc::clone(&calls),
        text: "Cached OCR text".to_string(),
    };

    let first = parse_image_document(&path, &config, Some(&provider)).unwrap();
    let second = parse_image_document(&path, &config, Some(&provider)).unwrap();

    assert!(first.to_text().contains("Cached OCR text"));
    assert!(second.to_text().contains("Cached OCR text"));
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[test]
fn image_ocr_requires_enabled_ocr_config() {
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.png", "not-a-real-image");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: None,
        ..Default::default()
    };

    let err = parse_image_document(&path, &config, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no extractable text found"));
}

#[test]
fn image_ocr_requires_provider() {
    let _guard = ocr_env_lock().lock().unwrap();
    std::env::remove_var("A3S_DOCUMENT_OCR_TESSERACT_BIN");
    std::env::remove_var("A3S_DOCUMENT_OCR_PDFTOPPM_BIN");
    let dir = TempDir::new().unwrap();
    let path = write_file(&dir, "sample.png", "not-a-real-image");
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    match parse_image_document(&path, &config, None) {
        Ok(doc) => assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr")),
        Err(err) => assert!(err.to_string().contains(
            "image context extraction requires a configured OCR backend or local tesseract"
        )),
    }
}

#[cfg(unix)]
#[test]
fn image_ocr_can_use_builtin_tesseract_provider() {
    let _guard = ocr_env_lock().lock().unwrap();
    let dir = TempDir::new().unwrap();
    let image = write_file(&dir, "sample.png", "not-a-real-image");
    let tesseract = write_executable(&dir, "tesseract", "#!/bin/sh\necho 'Built-in OCR text'\n");

    std::env::set_var("A3S_DOCUMENT_OCR_TESSERACT_BIN", &tesseract);
    std::env::remove_var("A3S_DOCUMENT_OCR_PDFTOPPM_BIN");

    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 10,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: None,
            prompt: None,
            max_images: 2,
            dpi: 150,
        }),
        ..Default::default()
    };

    let doc = parse_image_document(&image, &config, None).unwrap();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr"));
    assert!(doc.blocks[0].content.contains("provider=builtin-tesseract"));
    assert!(doc.to_text().contains("Built-in OCR text"));

    std::env::remove_var("A3S_DOCUMENT_OCR_TESSERACT_BIN");
}

#[cfg(unix)]
#[test]
fn pdf_ocr_can_use_builtin_tesseract_and_pdftoppm_provider() {
    let _guard = ocr_env_lock().lock().unwrap();
    let dir = TempDir::new().unwrap();
    let pdf = write_file(&dir, "scan.pdf", "%PDF-1.4 fake");
    let tesseract = write_executable(&dir, "tesseract", "#!/bin/sh\nbasename \"$1\" .png\n");
    let pdftoppm = write_executable(
            &dir,
            "pdftoppm",
            "#!/bin/sh\nprefix=\"${9}\"\nprintf 'fake' > \"${prefix}-1.png\"\nprintf 'fake' > \"${prefix}-2.png\"\n",
        );

    std::env::set_var("A3S_DOCUMENT_OCR_TESSERACT_BIN", &tesseract);
    std::env::set_var("A3S_DOCUMENT_OCR_PDFTOPPM_BIN", &pdftoppm);

    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 10,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: None,
            prompt: None,
            max_images: 2,
            dpi: 144,
        }),
        ..Default::default()
    };

    let result = maybe_run_pdf_ocr(&pdf, String::new(), &config, None).unwrap();
    assert_eq!(result.mode, PdfOcrMode::Used);
    assert_eq!(result.provider_name.as_deref(), Some("builtin-tesseract"));
    assert!(result.text.contains("page-1"));
    assert!(result.text.contains("page-2"));

    std::env::remove_var("A3S_DOCUMENT_OCR_TESSERACT_BIN");
    std::env::remove_var("A3S_DOCUMENT_OCR_PDFTOPPM_BIN");
}

#[test]
fn docx_ocr_fallback_uses_provider_when_no_text_is_extractable() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "scan.docx",
        &[(
            "word/document.xml",
            r#"<w:document xmlns:w="urn:test"><w:body></w:body></w:document>"#,
        )],
    );
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: Some("moonshot/kimi-vl".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = RequestOnlyOcrProvider {
        text: Some("Recovered DOCX OCR text".to_string()),
    };

    let doc = parse_docx(&path, &config, Some(&provider)).unwrap();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr"));
    assert!(doc.blocks[0].content.contains("format=docx"));
    assert!(doc.to_text().contains("Recovered DOCX OCR text"));
}

#[test]
fn pptx_ocr_fallback_uses_provider_when_no_text_is_extractable() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "scan.pptx",
        &[(
            "ppt/slides/slide1.xml",
            r#"<p:sld xmlns:p="urn:test" xmlns:a="urn:test-a"><p:cSld><p:spTree></p:spTree></p:cSld></p:sld>"#,
        )],
    );
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: Some("moonshot/kimi-vl".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = RequestOnlyOcrProvider {
        text: Some("Recovered PPTX OCR text".to_string()),
    };

    let doc = parse_pptx(&path, &config, Some(&provider)).unwrap();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr"));
    assert!(doc.blocks[0].content.contains("format=pptx"));
    assert!(doc.to_text().contains("Recovered PPTX OCR text"));
}

#[test]
fn xlsx_ocr_fallback_uses_provider_when_no_text_is_extractable() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "scan.xlsx",
        &[(
            "xl/worksheets/sheet1.xml",
            r#"<worksheet xmlns="urn:test"><sheetData></sheetData></worksheet>"#,
        )],
    );
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: Some("moonshot/kimi-vl".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = RequestOnlyOcrProvider {
        text: Some("Recovered XLSX OCR text".to_string()),
    };

    let doc = parse_xlsx(&path, &config, Some(&provider)).unwrap();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr"));
    assert!(doc.blocks[0].content.contains("format=xlsx"));
    assert!(doc.to_text().contains("Recovered XLSX OCR text"));
}

#[test]
fn odf_ocr_fallback_uses_provider_when_no_text_is_extractable() {
    let dir = TempDir::new().unwrap();
    let path = write_zip(
        &dir,
        "scan.odt",
        &[(
            "content.xml",
            r#"<office:document-content xmlns:office="urn:test" xmlns:text="urn:test-text"><office:body><office:text></office:text></office:body></office:document-content>"#,
        )],
    );
    let config = crate::config::DocumentParserConfig {
        enabled: true,
        max_file_size_mb: 50,
        ocr: Some(crate::config::DocumentOcrConfig {
            enabled: true,
            model: Some("moonshot/kimi-vl".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let provider = RequestOnlyOcrProvider {
        text: Some("Recovered ODF OCR text".to_string()),
    };

    let doc = parse_odf(&path, &config, Some(&provider)).unwrap();
    assert_eq!(doc.blocks[0].label.as_deref(), Some("ocr"));
    assert!(doc.blocks[0].content.contains("format=odf"));
    assert!(doc.to_text().contains("Recovered ODF OCR text"));
}
