use crate::document_consume::{self};
use crate::document_service_types::{document_block_kind_label, ResolvedParseRequest};
use crate::llm::{LlmClient, Message};
use crate::tools::{ToolContext, ToolOutput};
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

pub(crate) async fn execute_parse_request(
    llm: Arc<dyn LlmClient>,
    ctx: &ToolContext,
    path: &Path,
    query: Option<&str>,
    request: &ResolvedParseRequest,
) -> Result<ToolOutput> {
    if !path.exists() {
        return Ok(document_consume::build_parse_file_not_found_output(path));
    }

    let prepared = match document_consume::prepare_parse_document_from_path(
        path,
        ctx.document_parsers.as_deref(),
        ctx.document_pipeline.as_deref(),
        request.strategy.label(),
        request.max_chars,
        query,
        document_block_kind_label,
    )? {
        Some(prepared) => prepared,
        None => return Ok(document_consume::build_parse_unreadable_output(path)),
    };

    let llm_answer = if let Some(q) = query {
        let llm_request =
            document_consume::build_parse_llm_request_for_prepared(&prepared, q, request.max_chars);

        let messages = vec![Message::user(&llm_request.user_prompt)];
        match llm
            .complete(&messages, Some(&llm_request.system_prompt), &[])
            .await
        {
            Ok(resp) => Some(resp.text()),
            Err(e) => Some(document_consume::build_parse_llm_failure_message(&e)),
        }
    } else {
        None
    };

    Ok(document_consume::build_parse_tool_output_from_prepared(
        &prepared,
        ctx.document_parser_config.as_ref(),
        query.is_some(),
        request.max_chars,
        llm_answer.as_deref(),
    ))
}
