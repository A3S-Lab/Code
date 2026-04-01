use anyhow::Result;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DocumentProbe {
    pub original_ext: String,
    pub detected_ext: String,
}

impl DocumentProbe {
    pub fn new(original_ext: String, detected_ext: String) -> Self {
        Self {
            original_ext,
            detected_ext,
        }
    }
}

pub(super) fn probe_document(path: &Path) -> Result<DocumentProbe> {
    let original_ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    let detected_ext =
        detect_from_contents(path, &original_ext)?.unwrap_or_else(|| original_ext.clone());
    Ok(DocumentProbe::new(original_ext, detected_ext))
}

fn detect_from_contents(path: &Path, original_ext: &str) -> Result<Option<String>> {
    if looks_like_zip(path)? {
        if matches!(original_ext, "pages" | "numbers" | "key") {
            return Ok(Some(original_ext.to_string()));
        }
        return detect_zip_kind(path);
    }

    if original_ext.is_empty()
        || original_ext == "xml"
        || original_ext == "html"
        || original_ext == "htm"
        || original_ext == "xhtml"
        || original_ext == "svg"
    {
        if let Some(kind) = detect_markup_kind(path)? {
            return Ok(Some(kind));
        }
    }

    Ok(None)
}

fn looks_like_zip(path: &Path) -> Result<bool> {
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    let read = file.read(&mut magic)?;
    Ok(read >= 4 && magic == [0x50, 0x4B, 0x03, 0x04])
}

fn detect_zip_kind(path: &Path) -> Result<Option<String>> {
    let mut zip = match super::open_zip(path) {
        Ok(zip) => zip,
        Err(_) => return Ok(None),
    };
    let names = zip
        .file_names()
        .map(|name| name.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if names.iter().any(|name| name == "word/document.xml") {
        return Ok(Some("docx".to_string()));
    }
    if names
        .iter()
        .any(|name| name == "contents/content.hpf" || name.starts_with("contents/section"))
    {
        return Ok(Some("hwpx".to_string()));
    }
    if names.iter().any(|name| name == "xl/workbook.bin") {
        return Ok(Some("xlsb".to_string()));
    }
    if names.iter().any(|name| name == "xl/workbook.xml") {
        return Ok(Some("xlsx".to_string()));
    }
    if names.iter().any(|name| name == "ppt/presentation.xml") {
        return Ok(Some("pptx".to_string()));
    }
    if let Ok(mimetype) = super::read_zip_entry(&mut zip, "mimetype") {
        let mimetype = mimetype.trim();
        if mimetype == "application/epub+zip" {
            return Ok(Some("epub".to_string()));
        }
        if mimetype == "application/vnd.oasis.opendocument.text" {
            return Ok(Some("odt".to_string()));
        }
        if mimetype == "application/vnd.oasis.opendocument.spreadsheet" {
            return Ok(Some("ods".to_string()));
        }
        if mimetype == "application/vnd.oasis.opendocument.presentation" {
            return Ok(Some("odp".to_string()));
        }
    }

    Ok(Some("zip".to_string()))
}

fn detect_markup_kind(path: &Path) -> Result<Option<String>> {
    let bytes = std::fs::read(path)?;
    let sample = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]).to_ascii_lowercase();
    let trimmed = sample.trim_start();
    if !(trimmed.starts_with('<')
        || trimmed.starts_with("<?xml")
        || trimmed.starts_with("<!doctype html"))
    {
        return Ok(None);
    }

    if trimmed.contains("<!doctype html") || trimmed.contains("<html") {
        if trimmed.contains("www.w3.org/1999/xhtml") {
            return Ok(Some("xhtml".to_string()));
        }
        return Ok(Some("html".to_string()));
    }
    if trimmed.contains("<svg") {
        return Ok(Some("svg".to_string()));
    }
    if trimmed.contains("application/vnd.oasis.opendocument.text")
        || trimmed.contains("office:mimetype=\"application/vnd.oasis.opendocument.text\"")
    {
        return Ok(Some("fodt".to_string()));
    }
    if trimmed.contains("application/vnd.oasis.opendocument.spreadsheet")
        || trimmed.contains("office:mimetype=\"application/vnd.oasis.opendocument.spreadsheet\"")
    {
        return Ok(Some("fods".to_string()));
    }
    if trimmed.contains("application/vnd.oasis.opendocument.presentation")
        || trimmed.contains("office:mimetype=\"application/vnd.oasis.opendocument.presentation\"")
    {
        return Ok(Some("fodp".to_string()));
    }
    if trimmed.contains("<?xml") || trimmed.contains('<') {
        return Ok(Some("xml".to_string()));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::FileOptions;

    fn write_file(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn probe_detects_docx_from_zip_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("upload.bin");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"<w:document/>").unwrap();
        zip.finish().unwrap();

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "bin");
        assert_eq!(probe.detected_ext, "docx");
    }

    #[test]
    fn probe_detects_svg_from_xml_payload() {
        let dir = TempDir::new().unwrap();
        let path = write_file(
            &dir,
            "upload",
            br#"<?xml version="1.0"?><svg xmlns="urn:test"><text>Hello</text></svg>"#,
        );

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "");
        assert_eq!(probe.detected_ext, "svg");
    }

    #[test]
    fn probe_detects_xhtml_from_markup_payload() {
        let dir = TempDir::new().unwrap();
        let path = write_file(
            &dir,
            "page.xml",
            br#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body>Hi</body></html>"#,
        );

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "xml");
        assert_eq!(probe.detected_ext, "xhtml");
    }

    #[test]
    fn probe_detects_odt_from_zip_mimetype() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("payload");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();
        zip.start_file("mimetype", options).unwrap();
        zip.write_all(b"application/vnd.oasis.opendocument.text")
            .unwrap();
        zip.start_file("content.xml", options).unwrap();
        zip.write_all(br#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"/>"#)
            .unwrap();
        zip.finish().unwrap();

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "");
        assert_eq!(probe.detected_ext, "odt");
    }

    #[test]
    fn probe_detects_xlsb_from_zip_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("payload.bin");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();
        zip.start_file("xl/workbook.bin", options).unwrap();
        zip.write_all(b"workbook").unwrap();
        zip.start_file("xl/worksheets/sheet1.bin", options).unwrap();
        zip.write_all(b"sheet").unwrap();
        zip.finish().unwrap();

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "bin");
        assert_eq!(probe.detected_ext, "xlsb");
    }

    #[test]
    fn probe_detects_hwpx_from_zip_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("payload.data");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();
        zip.start_file("Contents/content.hpf", options).unwrap();
        zip.write_all(b"<hpf/>").unwrap();
        zip.start_file("Contents/section0.xml", options).unwrap();
        zip.write_all(b"<root><p>Hello HWPX</p></root>").unwrap();
        zip.finish().unwrap();

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "data");
        assert_eq!(probe.detected_ext, "hwpx");
    }

    #[test]
    fn probe_preserves_iwork_extension_for_zip_packages() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deck.pages");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();
        zip.start_file("preview.txt", options).unwrap();
        zip.write_all(b"Preview").unwrap();
        zip.finish().unwrap();

        let probe = probe_document(&path).unwrap();
        assert_eq!(probe.original_ext, "pages");
        assert_eq!(probe.detected_ext, "pages");
    }
}
