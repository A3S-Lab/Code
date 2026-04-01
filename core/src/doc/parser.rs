use anyhow::Result;
use std::path::Path;

use super::{ExtractedDocument, ParsedDocument};

pub trait DocumentParser: Send + Sync {
    fn name(&self) -> &str;

    fn signature(&self) -> String {
        self.name().to_string()
    }

    fn supported_extensions(&self) -> &[&str];

    fn parse(&self, path: &Path) -> Result<String>;

    fn parse_extracted(&self, path: &Path) -> Result<ExtractedDocument> {
        Ok(ExtractedDocument::new(ParsedDocument::from_text(
            self.parse(path)?,
        )))
    }

    fn can_parse(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                self.supported_extensions()
                    .iter()
                    .any(|supported| supported.eq_ignore_ascii_case(ext))
            })
            .unwrap_or(false)
    }

    fn max_file_size(&self) -> u64 {
        10 * 1024 * 1024
    }
}
