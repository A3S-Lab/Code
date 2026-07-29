const PDF_CONTENT_TYPE: &str = "application/pdf";
const PDF_MAGIC: &[u8] = b"%PDF-";

pub(super) fn response_is_pdf(content_type: &str, bytes: &[u8]) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(PDF_CONTENT_TYPE))
        || bytes.starts_with(PDF_MAGIC)
}

/// Extract PDF text away from Tokio's async worker threads.
pub(super) async fn extract_text(bytes: Vec<u8>) -> Result<String, String> {
    let text = tokio::task::spawn_blocking(move || extract_text_with_lopdf(&bytes))
        .await
        .map_err(|error| format!("PDF text extraction worker failed: {error}"))??;
    if text.trim().is_empty() {
        return Err(
            "PDF contains no extractable text; it may be image-only or scanned".to_string(),
        );
    }
    Ok(text)
}

fn extract_text_with_lopdf(bytes: &[u8]) -> Result<String, String> {
    let document = lopdf::Document::load_mem(bytes)
        .map_err(|error| format!("Could not parse or extract text from PDF: {error}"))?;
    let page_numbers = document.get_pages().keys().copied().collect::<Vec<_>>();
    let mut text = String::new();
    let mut extraction_errors = Vec::new();
    for chunk in document.extract_text_chunks(&page_numbers) {
        match chunk {
            Ok(chunk) => {
                text.push_str(&chunk);
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            Err(error) => extraction_errors.push(error.to_string()),
        }
    }
    if text.trim().is_empty() && !extraction_errors.is_empty() {
        return Err(format!(
            "Could not parse or extract text from PDF: {}",
            extraction_errors.join("; ")
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::types::{Tool, ToolContext};
    use lopdf::{
        content::{Content, Operation},
        dictionary, Document, Object, Stream,
    };

    fn pdf_document(text: Option<&str>) -> Vec<u8> {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = document.add_object(dictionary! {
            "Font" => dictionary! {
                "F1" => font_id,
            },
        });
        let mut operations = Vec::new();
        if let Some(text) = text {
            operations.extend([
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
            ]);
        }
        let content = Content { operations };
        let content_id = document.add_object(Stream::new(
            dictionary! {},
            content.encode().expect("test PDF content must encode"),
        ));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document
            .save_to(&mut bytes)
            .expect("test PDF must serialize");
        bytes
    }

    #[test]
    fn detects_pdf_from_content_type_or_magic() {
        assert!(response_is_pdf(
            "Application/PDF; charset=binary",
            b"not a PDF payload"
        ));
        assert!(response_is_pdf("application/octet-stream", b"%PDF-1.7\n"));
        assert!(!response_is_pdf("text/html", b"<html></html>"));
        assert!(!response_is_pdf("application/json", b"{\"pdf\":true}"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extracts_text_from_pdf_without_external_io() {
        let text = extract_text(pdf_document(Some("STORM research evidence")))
            .await
            .expect("generated PDF text must extract");

        assert!(text.contains("STORM research evidence"), "{text:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejects_pdf_without_extractable_text() {
        let error = extract_text(pdf_document(None)).await.unwrap_err();

        assert!(error.contains("no extractable text"), "{error}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reports_malformed_pdf_as_parse_failure() {
        let error = extract_text(b"%PDF-1.7\nmalformed".to_vec())
            .await
            .unwrap_err();

        assert!(
            error.contains("Could not parse or extract text from PDF"),
            "{error}"
        );
    }

    #[tokio::test]
    #[ignore = "requires external network"]
    async fn extracts_real_storm_arxiv_pdf() {
        let result = super::super::WebFetchTool
            .execute(
                &serde_json::json!({
                    "url": "https://arxiv.org/pdf/2402.14207",
                    "format": "text",
                    "timeout": 30
                }),
                &ToolContext::new(std::env::temp_dir()),
            )
            .await
            .expect("web_fetch must execute");
        assert!(result.success, "{}", result.content);
        assert_eq!(
            result
                .metadata
                .as_ref()
                .map(|value| &value["document_kind"]),
            Some(&serde_json::json!("pdf"))
        );
        assert_eq!(
            result.metadata.as_ref().map(|value| &value["content_type"]),
            Some(&serde_json::json!("application/pdf"))
        );
        assert!(
            result.content.contains("STORM"),
            "extracted text omitted the title"
        );
        assert!(
            result.content.contains("Wikipedia-like Articles"),
            "extracted text omitted the paper subject"
        );
        assert!(
            !result.content.contains("Taylor Hawkins"),
            "extracted PDF text included unrelated page content"
        );
    }

    #[tokio::test]
    #[ignore = "requires external network"]
    async fn extracts_type3_font_researcherbench_pdf_without_panicking() {
        let result = super::super::WebFetchTool
            .execute(
                &serde_json::json!({
                    "url": "https://arxiv.org/pdf/2507.16280",
                    "format": "text",
                    "timeout": 30
                }),
                &ToolContext::new(std::env::temp_dir()),
            )
            .await
            .expect("web_fetch must execute");

        assert!(result.success, "{}", result.content);
        assert!(
            result.content.contains("ResearcherBench"),
            "extracted text omitted the paper title"
        );
    }
}
