use std::sync::Arc;

use a3s_tool_runtime::Tool;

use crate::computer::local::LocalFs;
use crate::implementations::grok_build::read_file::{
    READ_FILE_MAX_BYTES, READ_FILE_MAX_TOKENS, ReadFileInput, ReadFileOutput, ReadFileParams,
    ReadFileTool,
};
use crate::notification::types::ToolNotificationHandle;
use crate::types::context::TruncationConfig;
use crate::types::resources::{
    Cwd, FileSystem, NotificationHandle, Params, Resources, TruncationCfg,
};
use crate::types::source_summary::{
    CapApplicability, CapDisposition, ReadLimitKind, ReadReason, ReadRole, SourceSummarySlot,
    ToolOutputLimit, ToolSourceResult,
};
use crate::types::template_renderer::TemplateRenderer;
use crate::types::tool::ToolKind;
use crate::types::tool_metadata::test_ctx;

fn resources(cwd: &std::path::Path) -> Resources {
    let mut resources = Resources::new();
    resources.insert(Cwd(cwd.to_path_buf()));
    resources.insert(FileSystem(Arc::new(LocalFs)));
    resources.insert(NotificationHandle(ToolNotificationHandle::noop()));
    resources.insert(TemplateRenderer::new(
        [(ToolKind::Search, "Grep".to_owned())].into(),
        Default::default(),
    ));
    resources
}

fn input(path: &std::path::Path) -> ReadFileInput {
    ReadFileInput {
        path: path.to_string_lossy().into_owned(),
        offset: None,
        limit: None,
        pages: None,
        format: None,
    }
}

#[tokio::test]
async fn missing_skill_keeps_the_role_and_does_not_reject_tokens() {
    let tmp = tempfile::TempDir::new().unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&tmp.path().join("SKILL.md")))
        .await
        .unwrap();
    assert!(matches!(output, ReadFileOutput::FileNotFound(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.role, ReadRole::SkillEntry);
    assert_ne!(read.tokens.disposition, CapDisposition::Rejected);
    assert!(read.tokens.observed.is_none());
    assert_eq!(read.limit_kind(), ReadLimitKind::Unknown);
    assert_ne!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert!(!format!("{read:?}").contains("SKILL.md"));
}

#[tokio::test]
async fn token_refusal_is_rejected_without_a_path_in_the_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("secret-read-dir");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("huge.txt");
    std::fs::write(
        &path,
        "Q".repeat(READ_FILE_MAX_BYTES + a3s_token_estimation::BYTES_PER_TOKEN as usize),
    )
    .unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileTooLarge(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.limit_kind(), ReadLimitKind::Tokens);
    assert_eq!(read.tokens.disposition, CapDisposition::Rejected);
    assert_eq!(read.tokens.configured, Some(READ_FILE_MAX_TOKENS as i64));
    assert!(read.tokens.observed.is_none());
    assert!(read.returned_lines.is_none());
    assert!(read.returned_bytes.is_none());
    assert_eq!(
        read.source_bytes,
        Some((READ_FILE_MAX_BYTES + a3s_token_estimation::BYTES_PER_TOKEN as usize) as i64)
    );
    assert!(!format!("{read:?}").contains("secret-read-dir"));
}

#[tokio::test]
async fn line_and_byte_caps_record_multiple() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("window.txt");
    let body = "line-1-xxxxxxxx\nline-2-xxxxxxxx\nline-3-xxxxxxxx\nline-4-xxxxxxxx\n";
    std::fs::write(&path, body).unwrap();
    let mut resources = resources(tmp.path());
    resources.insert(Params(ReadFileParams {
        max_output_bytes: Some(15),
        ..Default::default()
    }));
    resources.insert(TruncationCfg(TruncationConfig {
        max_lines_read: Some(2),
        ..Default::default()
    }));
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources.into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    let ReadFileOutput::FileContent(content) = output else {
        panic!("expected file content");
    };
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.limit_kind(), ReadLimitKind::Multiple);
    assert_eq!(read.lines.disposition, CapDisposition::Truncated);
    assert_eq!(read.formatted_bytes.disposition, CapDisposition::Truncated);
    assert_eq!(read.tokens.disposition, CapDisposition::WithinLimit);
    assert!(read.tokens.observed.is_none());
    // Post-budget raw lines, not the pre-budget window and not the marker line.
    assert_eq!(read.returned_lines, Some(1));
    assert_ne!(
        read.returned_lines,
        Some(content.content.lines().count() as i64)
    );
    assert_eq!(read.returned_bytes, Some(content.content.len() as i64));
    assert_eq!(read.source_bytes, Some(body.len() as i64));
}

#[tokio::test]
async fn image_size_error_is_not_succeeded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("oversized.jpg");
    std::fs::write(&path, jpeg_over_decode_ceiling()).unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::ImageSizeError(_)));
    let summary = slot.snapshot();
    assert_eq!(summary.result, ToolSourceResult::Failed(None));
    let read = summary.read().expect("read profile");
    assert_ne!(read.tokens.applicability, CapApplicability::NotApplicable);
    assert_ne!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.limit_kind(), ReadLimitKind::Unknown);
}

#[tokio::test]
async fn embedded_image_stays_succeeded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("icon.png");
    let bytes = crate::implementations::read_file::metadata::png_with_svg_bytes_in_idat();
    std::fs::write(&path, &bytes).unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::ImageContent(_)));
    let summary = slot.snapshot();
    assert_eq!(summary.result, ToolSourceResult::Succeeded);
    let read = summary.read().expect("read profile");
    assert_eq!(read.tokens.applicability, CapApplicability::NotApplicable);
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.source_bytes, Some(bytes.len() as i64));
}

#[tokio::test]
async fn ordinary_missing_file_is_not_not_limited() {
    let tmp = tempfile::TempDir::new().unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&tmp.path().join("missing.txt")))
        .await
        .unwrap();
    assert!(matches!(output, ReadFileOutput::FileNotFound(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.role, ReadRole::Ordinary);
    assert_eq!(read.lines.applicability, CapApplicability::Unknown);
    assert_eq!(read.lines.disposition, CapDisposition::Unobserved);
    assert_eq!(read.tokens.applicability, CapApplicability::Unknown);
    assert_eq!(read.tokens.disposition, CapDisposition::Unobserved);
    assert_eq!(
        read.formatted_bytes.applicability,
        CapApplicability::NotApplicable
    );
    assert_eq!(
        summary.result,
        ToolSourceResult::Failed(Some(ReadReason::NotFound))
    );
    assert_eq!(read.limit_kind(), ReadLimitKind::Unknown);
    assert_eq!(summary.output_limit, ToolOutputLimit::Unobserved);
}

#[tokio::test]
async fn ordinary_directory_is_not_not_limited() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir(tmp.path().join("subdir")).unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&tmp.path().join("subdir")))
        .await
        .unwrap();
    assert!(matches!(output, ReadFileOutput::IsADirectory(_)));
    let summary = slot.snapshot();
    assert_eq!(
        summary.result,
        ToolSourceResult::Failed(Some(ReadReason::Directory))
    );
    assert_ne!(summary.output_limit, ToolOutputLimit::NotLimited);
    let read = summary.read().expect("read profile");
    assert_eq!(read.role, ReadRole::Ordinary);
    assert_eq!(read.limit_kind(), ReadLimitKind::Unknown);
}

#[tokio::test]
async fn empty_file_stays_not_limited() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("empty.txt");
    std::fs::write(&path, "").unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(summary.result, ToolSourceResult::Empty);
    assert_eq!(read.lines.observed, Some(0));
    assert_eq!(read.lines.disposition, CapDisposition::WithinLimit);
    assert_eq!(read.tokens.disposition, CapDisposition::Unobserved);
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.source_bytes, Some(0));
}

#[tokio::test]
async fn empty_skill_file_is_not_limited() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("SKILL.md");
    std::fs::write(&path, "").unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.role, ReadRole::SkillEntry);
    assert_eq!(summary.result, ToolSourceResult::Empty);
    assert_eq!(read.tokens.applicability, CapApplicability::Applies);
    assert_eq!(read.tokens.disposition, CapDisposition::WithinLimit);
    assert_eq!(read.tokens.observed, Some(0));
    assert_eq!(read.tokens.configured, Some(READ_FILE_MAX_TOKENS as i64));
    assert_eq!(
        read.formatted_bytes.applicability,
        CapApplicability::NotApplicable
    );
    assert_eq!(read.formatted_bytes.disposition, CapDisposition::Unobserved);
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.source_bytes, Some(0));
}

#[tokio::test]
async fn empty_instruction_file_is_not_limited() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("AGENTS.md");
    std::fs::write(&path, "").unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.role, ReadRole::Instruction);
    assert_eq!(summary.result, ToolSourceResult::Empty);
    assert_eq!(read.tokens.applicability, CapApplicability::Applies);
    assert_eq!(read.tokens.disposition, CapDisposition::WithinLimit);
    assert_eq!(read.tokens.observed, Some(0));
    assert_eq!(read.lines.disposition, CapDisposition::WithinLimit);
    assert_eq!(read.lines.observed, Some(0));
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.source_bytes, Some(0));
}

#[tokio::test]
async fn empty_skill_with_a_byte_budget_evaluates_that_cap() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("SKILL.md");
    std::fs::write(&path, "").unwrap();
    let mut resources = resources(tmp.path());
    resources.insert(Params(ReadFileParams {
        max_output_bytes: Some(64),
        ..Default::default()
    }));
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources.into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(summary.result, ToolSourceResult::Empty);
    assert_eq!(
        read.formatted_bytes.applicability,
        CapApplicability::Applies
    );
    assert_eq!(
        read.formatted_bytes.disposition,
        CapDisposition::WithinLimit
    );
    assert_eq!(read.formatted_bytes.observed, Some(0));
    assert_eq!(read.formatted_bytes.configured, Some(64));
    assert_eq!(read.tokens.disposition, CapDisposition::WithinLimit);
    assert_eq!(read.tokens.observed, Some(0));
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
}

#[tokio::test]
async fn ordinary_read_records_source_bytes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("note.txt");
    std::fs::write(&path, "hi\n").unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(read.role, ReadRole::Ordinary);
    assert_eq!(summary.result, ToolSourceResult::Succeeded);
    assert_eq!(read.tokens.disposition, CapDisposition::WithinLimit);
    assert!(read.tokens.observed.is_none());
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.source_bytes, Some(3));
    assert!(read.returned_lines.is_some());
    assert!(read.returned_bytes.is_some());
}

#[tokio::test]
async fn whole_read_token_fallback_is_a_token_truncation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let filler = "Q".repeat(READ_FILE_MAX_BYTES + a3s_token_estimation::BYTES_PER_TOKEN as usize);
    let mut body = "alpha\nbeta\n".to_owned();
    body.push_str(&filler);
    let skill = tmp.path().join("SKILL.md");
    let agents = tmp.path().join("AGENTS.md");
    std::fs::write(&skill, &body).unwrap();
    std::fs::write(&agents, &body).unwrap();
    for (path, role) in [
        (&skill, ReadRole::SkillEntry),
        (&agents, ReadRole::Instruction),
    ] {
        let slot = SourceSummarySlot::new();
        let mut ctx = test_ctx(resources(tmp.path()).into_shared());
        ctx.insert(slot.clone());
        let mut request = input(path);
        request.limit = Some(2);
        let output = Tool::run(&ReadFileTool, ctx, request).await.unwrap();
        assert!(matches!(output, ReadFileOutput::FileContent(_)));
        let summary = slot.snapshot();
        let read = summary.read().expect("read profile");
        assert_eq!(read.role, role);
        assert_eq!(summary.result, ToolSourceResult::Succeeded);
        assert_eq!(read.tokens.applicability, CapApplicability::Applies);
        assert_eq!(read.tokens.disposition, CapDisposition::Truncated);
        assert_eq!(read.tokens.configured, Some(READ_FILE_MAX_TOKENS as i64));
        assert!(read.tokens.observed.is_none());
        assert_eq!(read.lines.disposition, CapDisposition::WithinLimit);
        assert_eq!(read.limit_kind(), ReadLimitKind::Tokens);
        assert_eq!(summary.output_limit, ToolOutputLimit::Limited);
        assert_eq!(read.returned_lines, Some(2));
        assert!(read.returned_bytes.is_some());
        assert_ne!(read.returned_bytes, read.source_bytes);
        assert_eq!(read.source_bytes, Some(body.len() as i64));
    }
}

#[tokio::test]
async fn skill_token_fallback_with_a_line_clip_is_multiple() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("SKILL.md");
    let filler = "Q".repeat(READ_FILE_MAX_BYTES + a3s_token_estimation::BYTES_PER_TOKEN as usize);
    let mut body = "alpha\nbeta\ngamma\n".to_owned();
    body.push_str(&filler);
    std::fs::write(&path, &body).unwrap();
    let mut resources = resources(tmp.path());
    resources.insert(TruncationCfg(TruncationConfig {
        max_lines_read: Some(2),
        ..Default::default()
    }));
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources.into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(summary.result, ToolSourceResult::Succeeded);
    assert_eq!(read.tokens.disposition, CapDisposition::Truncated);
    assert!(read.tokens.observed.is_none());
    assert_eq!(read.lines.disposition, CapDisposition::Truncated);
    assert_eq!(read.limit_kind(), ReadLimitKind::Multiple);
    assert_eq!(read.returned_lines, Some(2));
    assert_eq!(read.source_bytes, Some(body.len() as i64));
}

#[tokio::test]
async fn line_clipped_token_refusal_stays_multiple() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("SKILL.md");
    let huge_line = format!(
        "{}\n",
        "Q".repeat(READ_FILE_MAX_BYTES + a3s_token_estimation::BYTES_PER_TOKEN as usize)
    );
    let body = huge_line.repeat(2);
    std::fs::write(&path, &body).unwrap();
    let mut resources = resources(tmp.path());
    resources.insert(TruncationCfg(TruncationConfig {
        max_lines_read: Some(1),
        ..Default::default()
    }));
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources.into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileTooLarge(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(
        summary.result,
        ToolSourceResult::Failed(Some(ReadReason::TokenLimit))
    );
    assert_eq!(read.tokens.disposition, CapDisposition::Rejected);
    assert_eq!(read.tokens.applicability, CapApplicability::Applies);
    assert_eq!(read.tokens.configured, Some(READ_FILE_MAX_TOKENS as i64));
    assert!(read.tokens.observed.is_none());
    assert_eq!(read.lines.disposition, CapDisposition::Truncated);
    assert_eq!(read.limit_kind(), ReadLimitKind::Multiple);
    assert!(read.returned_lines.is_none());
    assert!(read.returned_bytes.is_none());
    assert_eq!(read.source_bytes, Some(body.len() as i64));
}

#[tokio::test]
async fn small_skill_whole_read_stays_within_token_limit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("SKILL.md");
    let body = "hello\n";
    std::fs::write(&path, body).unwrap();
    let slot = SourceSummarySlot::new();
    let mut ctx = test_ctx(resources(tmp.path()).into_shared());
    ctx.insert(slot.clone());
    let output = Tool::run(&ReadFileTool, ctx, input(&path)).await.unwrap();
    assert!(matches!(output, ReadFileOutput::FileContent(_)));
    let summary = slot.snapshot();
    let read = summary.read().expect("read profile");
    assert_eq!(summary.result, ToolSourceResult::Succeeded);
    assert_eq!(read.role, ReadRole::SkillEntry);
    assert_eq!(read.tokens.disposition, CapDisposition::WithinLimit);
    assert!(read.tokens.observed.is_none());
    assert_eq!(read.lines.disposition, CapDisposition::Exempt);
    assert_eq!(read.limit_kind(), ReadLimitKind::None);
    assert_eq!(summary.output_limit, ToolOutputLimit::NotLimited);
    assert_eq!(read.source_bytes, Some(body.len() as i64));
}

/// Declared dimensions are above the decode ceiling. The SOI marker stays a JPEG.
fn jpeg_over_decode_ceiling() -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{DynamicImage, ImageBuffer, Rgb};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(64, 64, Rgb([7, 8, 9]));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&DynamicImage::ImageRgb8(img))
        .unwrap();
    let sof = jpeg
        .windows(2)
        .position(|w| w == [0xFF, 0xC0])
        .expect("baseline SOF0 present");
    let Some(dims) = jpeg.get_mut(sof + 5..sof + 9) else {
        panic!("SOF0 dimensions missing at {sof}");
    };
    dims.copy_from_slice(&[0x40, 0x00, 0x40, 0x00]);
    jpeg
}
