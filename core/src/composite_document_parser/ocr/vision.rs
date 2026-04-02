//! Vision API OCR provider.
//!
//! Supports OpenAI-compatible vision APIs for document OCR fallback.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Serialize;
use uuid::Uuid;

use super::{
    DocumentOcrCapabilities, DocumentOcrFormat, DocumentOcrOutput, DocumentOcrPageResult,
    DocumentOcrProvider, DocumentOcrRequest,
};

const DEFAULT_VISION_BASE_URL: &str = "https://api.openai.com/v1";

pub(super) struct VisionOcrProvider {
    model: String,
    base_url: String,
    api_key: String,
    pdftoppm_bin: Option<std::path::PathBuf>,
}

impl VisionOcrProvider {
    pub(super) fn new(
        model: String,
        base_url: Option<String>,
        api_key: String,
    ) -> Option<Self> {
        // Discover pdftoppm if available
        let pdftoppm_bin = std::env::var_os("A3S_DOCUMENT_OCR_PDFTOPPM_BIN")
            .map(std::path::PathBuf::from)
            .or_else(|| which_in_path("pdftoppm"));

        Some(Self {
            model,
            base_url: base_url.unwrap_or_else(|| DEFAULT_VISION_BASE_URL.to_string()),
            api_key,
            pdftoppm_bin,
        })
    }

    fn render_pdf_to_images(&self, path: &Path, dpi: u32, max_images: usize) -> Result<Vec<Vec<u8>>> {
        let Some(pdftoppm_bin) = &self.pdftoppm_bin else {
            anyhow::bail!("pdftoppm not found for PDF rendering");
        };

        let tempdir = std::env::temp_dir().join(format!("a3s-vision-ocr-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tempdir).context("failed to create temp dir for PDF pages")?;
        let prefix = tempdir.join("page");

        let status = std::process::Command::new(pdftoppm_bin)
            .arg("-r")
            .arg(dpi.to_string())
            .arg("-f")
            .arg("1")
            .arg("-l")
            .arg(max_images.to_string())
            .arg("-png")
            .arg(path)
            .arg(&prefix)
            .status()
            .with_context(|| format!("failed to run pdftoppm on {}", path.display()))?;

        if !status.success() {
            let _ = std::fs::remove_dir_all(&tempdir);
            anyhow::bail!("pdftoppm exited with status {} while rasterizing {}", status, path.display());
        }

        let mut pages = std::fs::read_dir(&tempdir)
            .context("failed to read rasterized PDF pages")?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("png"))
            .collect::<Vec<_>>();
        pages.sort();

        let mut images = Vec::new();
        for page in pages {
            let data = std::fs::read(&page).context("failed to read PNG page")?;
            images.push(data);
        }

        let _ = std::fs::remove_dir_all(&tempdir);
        Ok(images)
    }

    fn call_vision_api(&self, images: &[Vec<u8>], prompt: &str) -> Result<String> {
        // Build messages with images
        let mut contents = Vec::new();
        for (idx, image_data) in images.iter().enumerate() {
            let base64_image = base64::engine::general_purpose::STANDARD.encode(image_data);
            contents.push(serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:image/png;base64,{}", base64_image),
                    "detail": "low"
                }
            }));
            // Only process first page if there are multiple (for performance)
            if idx == 0 && images.len() > 1 {
                break;
            }
        }

        let messages = serde_json::json!([
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": prompt
                    },
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&images[0])),
                            "detail": "low"
                        }
                    }
                ]
            }
        ]);

        let client = reqwest::blocking::Client::new();
        let response = client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": self.model,
                "messages": messages,
                "max_tokens": 4096
            }))
            .send()
            .context("Vision API request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            anyhow::bail!("Vision API returned error {}: {}", status, body);
        }

        #[derive(Serialize, serde::Deserialize)]
        struct VisionResponse {
            choices: Vec<Choice>,
        }
        #[derive(Serialize, serde::Deserialize)]
        struct Choice {
            message: Message,
        }
        #[derive(Serialize, serde::Deserialize)]
        struct Message {
            content: String,
        }

        let vr: VisionResponse = response.json().context("Failed to parse Vision API response")?;
        let text = vr
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(text)
    }
}

impl DocumentOcrProvider for VisionOcrProvider {
    fn name(&self) -> &str {
        "vision-api"
    }

    fn capabilities(&self) -> DocumentOcrCapabilities {
        let mut caps = DocumentOcrCapabilities::new(["image", "pdf"]);
        caps.model = Some(self.model.clone());
        caps.prompt_configurable = true;
        caps.page_level_results = false;
        caps
    }

    fn ocr_document_result(&self, request: &DocumentOcrRequest<'_>) -> Result<Option<DocumentOcrOutput>> {
        let images = match request.format {
            DocumentOcrFormat::Image => {
                let data = std::fs::read(request.path).context("Failed to read image file")?;
                vec![data]
            }
            DocumentOcrFormat::Pdf => {
                if self.pdftoppm_bin.is_none() {
                    return Ok(None);
                }
                self.render_pdf_to_images(request.path, request.config.dpi, request.config.max_images)?
            }
            _ => return Ok(None),
        };

        if images.is_empty() {
            return Ok(None);
        }

        let prompt = request.config.prompt.as_deref()
            .unwrap_or("Extract all text from this document. Preserve the structure and formatting. Return only the extracted text, nothing else.");

        let text = self.call_vision_api(&images, prompt)?;

        Ok(Some(DocumentOcrOutput {
            text: text.clone(),
            pages: vec![DocumentOcrPageResult {
                page: Some(1),
                text,
                language: None,
                confidence_score_percent: None,
            }],
            language: None,
            confidence_score_percent: None,
            model: Some(self.model.clone()),
        }))
    }
}

fn which_in_path(binary: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
