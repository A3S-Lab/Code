use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use super::{
    DocumentOcrCapabilities, DocumentOcrFormat, DocumentOcrOutput, DocumentOcrPageResult,
    DocumentOcrProvider, DocumentOcrRequest,
};

const TESSERACT_ENV: &str = "A3S_DOCUMENT_OCR_TESSERACT_BIN";
const PDFTOPPM_ENV: &str = "A3S_DOCUMENT_OCR_PDFTOPPM_BIN";

pub(super) struct BuiltinOcrProvider {
    tesseract_bin: PathBuf,
    pdftoppm_bin: Option<PathBuf>,
}

impl BuiltinOcrProvider {
    pub(super) fn discover() -> Option<Self> {
        let tesseract_bin = resolve_command_override_or_path(TESSERACT_ENV, "tesseract")?;
        let pdftoppm_bin = resolve_command_override_or_path(PDFTOPPM_ENV, "pdftoppm");
        Some(Self {
            tesseract_bin,
            pdftoppm_bin,
        })
    }

    fn extract_image(&self, path: &Path, dpi: u32) -> Result<Option<String>> {
        let output = Command::new(&self.tesseract_bin)
            .arg(path)
            .arg("stdout")
            .arg("--dpi")
            .arg(dpi.to_string())
            .output()
            .with_context(|| format!("failed to run tesseract on {}", path.display()))?;

        if !output.status.success() {
            anyhow::bail!(
                "tesseract exited with status {} while OCRing {}",
                output.status,
                path.display()
            );
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!text.is_empty()).then_some(text))
    }

    fn extract_image_result(
        &self,
        path: &Path,
        dpi: u32,
        page: Option<usize>,
    ) -> Result<Option<DocumentOcrPageResult>> {
        self.extract_image(path, dpi).map(|text| {
            text.map(|text| DocumentOcrPageResult {
                page,
                text,
                language: None,
                confidence_score_percent: None,
            })
        })
    }

    fn extract_pdf_result(
        &self,
        request: &DocumentOcrRequest<'_>,
    ) -> Result<Option<DocumentOcrOutput>> {
        let Some(pdftoppm_bin) = &self.pdftoppm_bin else {
            return Ok(None);
        };

        let tempdir = std::env::temp_dir().join(format!("a3s-document-ocr-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tempdir).context("failed to create temp dir for OCR pages")?;
        let prefix = tempdir.join("page");
        let status = Command::new(pdftoppm_bin)
            .arg("-r")
            .arg(request.config.dpi.to_string())
            .arg("-f")
            .arg("1")
            .arg("-l")
            .arg(request.config.max_images.to_string())
            .arg("-png")
            .arg(request.path)
            .arg(&prefix)
            .status()
            .with_context(|| format!("failed to run pdftoppm on {}", request.path.display()))?;

        if !status.success() {
            anyhow::bail!(
                "pdftoppm exited with status {} while rasterizing {}",
                status,
                request.path.display()
            );
        }

        let mut pages = std::fs::read_dir(&tempdir)
            .context("failed to read rasterized OCR pages")?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("png"))
            .collect::<Vec<_>>();
        pages.sort();

        let mut page_results = Vec::new();
        for (idx, page) in pages
            .into_iter()
            .take(request.config.max_images)
            .enumerate()
        {
            if let Some(result) =
                self.extract_image_result(&page, request.config.dpi, Some(idx + 1))?
            {
                page_results.push(result);
            }
        }

        let text = page_results
            .iter()
            .map(|page| page.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let _ = std::fs::remove_dir_all(&tempdir);
        Ok((!text.trim().is_empty()).then_some(DocumentOcrOutput {
            text,
            pages: page_results,
            language: None,
            confidence_score_percent: None,
            model: Some("tesseract".to_string()),
        }))
    }
}

impl DocumentOcrProvider for BuiltinOcrProvider {
    fn name(&self) -> &str {
        "builtin-tesseract"
    }

    fn capabilities(&self) -> DocumentOcrCapabilities {
        let mut caps = if self.pdftoppm_bin.is_some() {
            DocumentOcrCapabilities::new(["image", "pdf"])
        } else {
            DocumentOcrCapabilities::new(["image"])
        };
        caps.model = Some("tesseract".to_string());
        caps.prompt_configurable = false;
        caps.page_level_results = true;
        caps
    }

    fn ocr_document_result(
        &self,
        request: &DocumentOcrRequest<'_>,
    ) -> Result<Option<DocumentOcrOutput>> {
        match request.format {
            DocumentOcrFormat::Image => Ok(self
                .extract_image_result(request.path, request.config.dpi, Some(1))?
                .map(|page| DocumentOcrOutput {
                    text: page.text.clone(),
                    pages: vec![page],
                    language: None,
                    confidence_score_percent: None,
                    model: Some("tesseract".to_string()),
                })),
            DocumentOcrFormat::Pdf => self.extract_pdf_result(request),
            _ => Ok(None),
        }
    }
}

fn resolve_command_override_or_path(env_key: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(env_key)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| which_in_path(fallback))
}

fn which_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
