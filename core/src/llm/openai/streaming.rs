use super::*;

impl OpenAiClient {
    /// Execute a fully-built streaming chat-completions request (sets `stream`).
    pub(super) async fn send_streaming(
        &self,
        mut request: serde_json::Value,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        {
            request["stream"] = serde_json::json!(true);
            request["stream_options"] = serde_json::json!({ "include_usage": true });
            let request_started_at = Instant::now();
            let url = format!("{}{}", self.base_url, self.chat_completions_path);
            let request_headers = self.request_headers();

            let streaming_resp = crate::retry::with_retry(&self.retry_config, |_attempt| {
                let http = &self.http;
                let url = &url;
                let request_headers = request_headers.clone();
                let request = &request;
                let cancel_token = cancel_token.clone();
                async move {
                    let headers = request_headers
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str()))
                        .collect::<Vec<_>>();
                    // Wrap in tokio::select! so cancellation aborts the HTTP request mid-flight
                    let resp = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            return AttemptOutcome::Fatal(anyhow::anyhow!("HTTP request cancelled"));
                        }
                        result = http.post_streaming(url, headers, request, cancel_token.clone()) => {
                            match result {
                                Ok(r) => r,
                                Err(e) => {
                                    // Transient network error (timeout, reset,
                                    // mid-flight drop — common on throttled
                                    // endpoints): retry with backoff like 429/5xx
                                    // instead of failing the turn. GLM and other
                                    // OpenAI-compatible endpoints hit this most.
                                    return if crate::retry::is_transient_error(&e) {
                                        AttemptOutcome::Retryable {
                                            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
                                            body: format!("network error: {e}"),
                                            retry_after: None,
                                        }
                                    } else {
                                        AttemptOutcome::Fatal(anyhow::anyhow!(
                                            "HTTP request failed: {}",
                                            e
                                        ))
                                    };
                                }
                            }
                        }
                    };
                    let status = reqwest::StatusCode::from_u16(resp.status)
                        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                    if status.is_success() {
                        AttemptOutcome::Success(resp)
                    } else {
                        let retry_after = resp
                            .retry_after
                            .as_deref()
                            .and_then(|v| RetryConfig::parse_retry_after(Some(v)));
                        if self.retry_config.is_retryable_status(status) {
                            AttemptOutcome::Retryable {
                                status,
                                body: resp.error_body,
                                retry_after,
                            }
                        } else {
                            AttemptOutcome::Fatal(anyhow::anyhow!(
                                "OpenAI API error at {} ({}): {}",
                                url,
                                status,
                                resp.error_body
                            ))
                        }
                    }
                }
            })
            .await?;

            let (tx, rx) = mpsc::channel(100);

            let mut stream = streaming_resp.byte_stream;
            let provider_name = self.provider_name.clone();
            let request_model = self.model.clone();
            let request_url = url.clone();
            let stream_cancellation = cancel_token.clone();
            tokio::spawn(async move {
                let mut buffer = String::new();
                let mut content_blocks: Vec<ContentBlock> = Vec::new();
                let mut text_content = String::new();
                let mut reasoning_content_accum = String::new();
                let mut tool_calls: std::collections::BTreeMap<usize, (String, String, String)> =
                    std::collections::BTreeMap::new();
                let mut started_tool_calls = std::collections::HashSet::new();
                let mut usage = TokenUsage::default();
                let mut finish_reason = None;
                let mut token_logprobs: Vec<TokenLogProb> = Vec::new();
                let mut response_id = None;
                let mut response_model = None;
                let mut response_object = None;
                let mut first_token_ms = None;
                let mut parsed_any_event = false;
                let mut stream_failed = false;

                loop {
                    let chunk_result = tokio::select! {
                        biased;
                        _ = stream_cancellation.cancelled() => return,
                        _ = tx.closed() => return,
                        chunk = stream.next() => match chunk {
                            Some(chunk) => chunk,
                            None => break,
                        },
                    };
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!("Stream error: {}", e);
                            stream_failed = true;
                            break;
                        }
                    };

                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(event_end) = buffer.find("\n\n") {
                        let event_data: String = buffer.drain(..event_end).collect();
                        buffer.drain(..2);

                        for line in event_data.lines() {
                            if let Some(data) = crate::sse::data_field_value(line) {
                                if data == "[DONE]" {
                                    if !text_content.is_empty() {
                                        content_blocks.push(ContentBlock::Text {
                                            text: text_content.clone(),
                                        });
                                    }
                                    for (id, name, args) in tool_calls.values() {
                                        content_blocks.push(ContentBlock::ToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: Self::parse_tool_arguments(name, args),
                                        });
                                    }
                                    tool_calls.clear();
                                    crate::telemetry::record_llm_usage(
                                        usage.prompt_tokens,
                                        usage.completion_tokens,
                                        usage.total_tokens,
                                        finish_reason.as_deref(),
                                    );
                                    let response = LlmResponse {
                                        message: Message {
                                            role: "assistant".to_string(),
                                            content: std::mem::take(&mut content_blocks),
                                            reasoning_content: if reasoning_content_accum.is_empty()
                                            {
                                                None
                                            } else {
                                                Some(std::mem::take(&mut reasoning_content_accum))
                                            },
                                        },
                                        usage: usage.clone(),
                                        stop_reason: std::mem::take(&mut finish_reason),
                                        token_logprobs: std::mem::take(&mut token_logprobs),
                                        meta: Some(LlmResponseMeta {
                                            provider: Some(provider_name.clone()),
                                            request_model: Some(request_model.clone()),
                                            request_url: Some(request_url.clone()),
                                            response_id: response_id.clone(),
                                            response_model: response_model.clone(),
                                            response_object: response_object.clone(),
                                            first_token_ms,
                                            duration_ms: Some(
                                                request_started_at.elapsed().as_millis() as u64,
                                            ),
                                        }),
                                    };
                                    let _ = tx.send(StreamEvent::Done(response)).await;
                                    return;
                                }

                                if let Ok(event) = serde_json::from_str::<OpenAiStreamChunk>(data) {
                                    parsed_any_event = true;
                                    if response_id.is_none() {
                                        response_id = event.id.clone();
                                    }
                                    if response_model.is_none() {
                                        response_model = event.model.clone();
                                    }
                                    if response_object.is_none() {
                                        response_object = event.object.clone();
                                    }
                                    if let Some(u) = event.usage {
                                        usage.prompt_tokens = u.prompt_tokens;
                                        usage.completion_tokens = u.completion_tokens;
                                        usage.total_tokens = u.total_tokens;
                                        // MiniMax: fall back to total_characters when total_tokens is 0.
                                        if usage.total_tokens == 0 {
                                            usage.total_tokens = u.total_characters.unwrap_or(0);
                                        }
                                        usage.cache_read_tokens = u
                                            .prompt_tokens_details
                                            .as_ref()
                                            .and_then(|d| d.cached_tokens);
                                    }

                                    if let Some(choice) = event.choices.into_iter().next() {
                                        if let Some(logprobs) = choice.logprobs.as_ref() {
                                            token_logprobs.extend(
                                                openai_logprobs_to_token_logprobs(logprobs),
                                            );
                                        }
                                        if let Some(reason) = choice.finish_reason {
                                            finish_reason = Some(reason);
                                        }

                                        if let Some(message) = choice.message {
                                            // If text_content already has content (from delta processing),
                                            // skip message.content to avoid sending duplicate full content
                                            let skip_content = !text_content.is_empty();
                                            if let Some(reasoning) = message.reasoning_content {
                                                // Reasoning is its OWN channel — accumulate into
                                                // reasoning_content, NEVER text_content. Merging it into
                                                // text_content made the assistant's `content` carry the
                                                // chain-of-thought, so a reasoning-only turn looked like a
                                                // finished text answer (response.text() non-empty) and the
                                                // agent loop terminated before the model emitted its tool
                                                // call → workers never reach generate_object → asset-diagnose
                                                // "未返回结构化输出". It also tripped `skip_content`, dropping
                                                // the model's real content.
                                                if first_token_ms.is_none() {
                                                    first_token_ms = Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    );
                                                }
                                                if let Some(delta) = Self::merge_stream_text(
                                                    &mut reasoning_content_accum,
                                                    &reasoning,
                                                ) {
                                                    let _ = tx
                                                        .send(StreamEvent::ReasoningDelta(delta))
                                                        .await;
                                                }
                                            }
                                            if !skip_content {
                                                if let Some(content) = message
                                                    .content
                                                    .filter(|value| !value.is_empty())
                                                {
                                                    if first_token_ms.is_none() {
                                                        first_token_ms = Some(
                                                            request_started_at.elapsed().as_millis()
                                                                as u64,
                                                        );
                                                    }
                                                    if let Some(delta) = Self::merge_stream_text(
                                                        &mut text_content,
                                                        &content,
                                                    ) {
                                                        let _ = tx
                                                            .send(StreamEvent::TextDelta(delta))
                                                            .await;
                                                    }
                                                }
                                            }
                                            if let Some(tcs) = message.tool_calls {
                                                for (index, tc) in tcs.into_iter().enumerate() {
                                                    tool_calls.insert(
                                                        index,
                                                        (
                                                            tc.id,
                                                            tc.function.name,
                                                            tc.function.arguments,
                                                        ),
                                                    );
                                                }
                                            }
                                        } else if let Some(delta) = choice.delta {
                                            if let Some(ref rc) = delta.reasoning_content {
                                                // Reasoning stays in reasoning_content, never text_content
                                                // (see the message-branch note above).
                                                if first_token_ms.is_none() {
                                                    first_token_ms = Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    );
                                                }
                                                if let Some(delta) = Self::merge_stream_text(
                                                    &mut reasoning_content_accum,
                                                    rc,
                                                ) {
                                                    let _ = tx
                                                        .send(StreamEvent::ReasoningDelta(delta))
                                                        .await;
                                                }
                                            }

                                            if let Some(content) = delta.content {
                                                if first_token_ms.is_none() {
                                                    first_token_ms = Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    );
                                                }
                                                if let Some(delta) = Self::merge_stream_text(
                                                    &mut text_content,
                                                    &content,
                                                ) {
                                                    let _ = tx
                                                        .send(StreamEvent::TextDelta(delta))
                                                        .await;
                                                }
                                            }

                                            if let Some(tcs) = delta.tool_calls {
                                                for tc in tcs {
                                                    let index = tc.index;
                                                    let was_started =
                                                        started_tool_calls.contains(&index);
                                                    let mut new_arguments = None;
                                                    let entry = tool_calls
                                                        .entry(index)
                                                        .or_insert_with(|| {
                                                            (
                                                                String::new(),
                                                                String::new(),
                                                                String::new(),
                                                            )
                                                        });

                                                    if let Some(id) = tc.id {
                                                        entry.0 = id;
                                                    }
                                                    if let Some(func) = tc.function {
                                                        if let Some(name) = func.name {
                                                            entry.1 = name;
                                                        }
                                                        if let Some(args) = func.arguments {
                                                            entry.2.push_str(&args);
                                                            new_arguments = Some(args);
                                                        }
                                                    }

                                                    let ready =
                                                        !entry.0.is_empty() && !entry.1.is_empty();
                                                    let start =
                                                        (!was_started && ready).then(|| {
                                                            (
                                                                entry.0.clone(),
                                                                entry.1.clone(),
                                                                entry.2.clone(),
                                                            )
                                                        });
                                                    let delta = if was_started && ready {
                                                        new_arguments.map(|arguments| {
                                                            (entry.0.clone(), arguments)
                                                        })
                                                    } else {
                                                        None
                                                    };

                                                    if let Some((id, name, buffered_arguments)) =
                                                        start
                                                    {
                                                        started_tool_calls.insert(index);
                                                        if first_token_ms.is_none() {
                                                            first_token_ms = Some(
                                                                request_started_at
                                                                    .elapsed()
                                                                    .as_millis()
                                                                    as u64,
                                                            );
                                                        }
                                                        let _ = tx
                                                            .send(StreamEvent::ToolUseStart {
                                                                id: id.clone(),
                                                                name,
                                                            })
                                                            .await;
                                                        if !buffered_arguments.is_empty() {
                                                            let _ = tx
                                                                .send(
                                                                    StreamEvent::ToolUseInputDelta {
                                                                        id: Some(id),
                                                                        delta: buffered_arguments,
                                                                    },
                                                                )
                                                                .await;
                                                        }
                                                    } else if let Some((id, arguments)) = delta {
                                                        let _ = tx
                                                            .send(StreamEvent::ToolUseInputDelta {
                                                                id: Some(id),
                                                                delta: arguments,
                                                            })
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                let trailing = buffer.trim();
                if !trailing.is_empty() {
                    if let Ok(event) = serde_json::from_str::<OpenAiStreamChunk>(trailing) {
                        parsed_any_event = true;
                        if response_id.is_none() {
                            response_id = event.id.clone();
                        }
                        if response_model.is_none() {
                            response_model = event.model.clone();
                        }
                        if response_object.is_none() {
                            response_object = event.object.clone();
                        }
                        if let Some(u) = event.usage {
                            usage.prompt_tokens = u.prompt_tokens;
                            usage.completion_tokens = u.completion_tokens;
                            usage.total_tokens = u.total_tokens;
                            usage.cache_read_tokens = u
                                .prompt_tokens_details
                                .as_ref()
                                .and_then(|d| d.cached_tokens);
                        }
                        if let Some(choice) = event.choices.into_iter().next() {
                            if let Some(logprobs) = choice.logprobs.as_ref() {
                                token_logprobs.extend(openai_logprobs_to_token_logprobs(logprobs));
                            }
                            if let Some(reason) = choice.finish_reason {
                                finish_reason = Some(reason);
                            }
                            // If text_content already has content (from delta processing),
                            // skip message.content to avoid sending duplicate full content
                            let skip_content = !text_content.is_empty();
                            if let Some(message) = choice.message {
                                if let Some(reasoning) = message.reasoning_content {
                                    // Reasoning → reasoning_content only, never text_content
                                    // (see the note in complete_streaming).
                                    if first_token_ms.is_none() {
                                        first_token_ms =
                                            Some(request_started_at.elapsed().as_millis() as u64);
                                    }
                                    if let Some(delta) = Self::merge_stream_text(
                                        &mut reasoning_content_accum,
                                        &reasoning,
                                    ) {
                                        let _ = tx.send(StreamEvent::ReasoningDelta(delta)).await;
                                    }
                                }
                                if !skip_content {
                                    if let Some(content) =
                                        message.content.filter(|value| !value.is_empty())
                                    {
                                        if first_token_ms.is_none() {
                                            first_token_ms = Some(
                                                request_started_at.elapsed().as_millis() as u64,
                                            );
                                        }
                                        if let Some(delta) =
                                            Self::merge_stream_text(&mut text_content, &content)
                                        {
                                            let _ = tx.send(StreamEvent::TextDelta(delta)).await;
                                        }
                                    }
                                }
                                if let Some(tcs) = message.tool_calls {
                                    for (index, tc) in tcs.into_iter().enumerate() {
                                        tool_calls.insert(
                                            index,
                                            (tc.id, tc.function.name, tc.function.arguments),
                                        );
                                    }
                                }
                            } else if let Some(delta) = choice.delta {
                                if let Some(ref rc) = delta.reasoning_content {
                                    // Reasoning → reasoning_content only, never text_content
                                    // (see the note in complete_streaming).
                                    if first_token_ms.is_none() {
                                        first_token_ms =
                                            Some(request_started_at.elapsed().as_millis() as u64);
                                    }
                                    if let Some(delta) =
                                        Self::merge_stream_text(&mut reasoning_content_accum, rc)
                                    {
                                        let _ = tx.send(StreamEvent::ReasoningDelta(delta)).await;
                                    }
                                }
                                if let Some(content) = delta.content {
                                    if first_token_ms.is_none() {
                                        first_token_ms =
                                            Some(request_started_at.elapsed().as_millis() as u64);
                                    }
                                    if let Some(delta) =
                                        Self::merge_stream_text(&mut text_content, &content)
                                    {
                                        let _ = tx.send(StreamEvent::TextDelta(delta)).await;
                                    }
                                }
                            }
                        }
                    } else if let Ok(response) = serde_json::from_str::<OpenAiResponse>(trailing) {
                        parsed_any_event = true;
                        response_id = response.id.clone();
                        response_model = response.model.clone();
                        response_object = response.object.clone();
                        usage.prompt_tokens = response.usage.prompt_tokens;
                        usage.completion_tokens = response.usage.completion_tokens;
                        usage.total_tokens = response.usage.total_tokens;
                        // MiniMax: fall back to total_characters when total_tokens is 0.
                        if usage.total_tokens == 0 {
                            usage.total_tokens = response.usage.total_characters.unwrap_or(0);
                        }
                        usage.cache_read_tokens = response
                            .usage
                            .prompt_tokens_details
                            .as_ref()
                            .and_then(|d| d.cached_tokens);

                        if let Some(choice) = response.choices.into_iter().next() {
                            if let Some(logprobs) = choice.logprobs.as_ref() {
                                token_logprobs.extend(openai_logprobs_to_token_logprobs(logprobs));
                            }
                            finish_reason = choice.finish_reason;
                            if let Some(text) =
                                choice.message.content.filter(|text| !text.is_empty())
                            {
                                if first_token_ms.is_none() {
                                    first_token_ms =
                                        Some(request_started_at.elapsed().as_millis() as u64);
                                }
                                let _ = Self::merge_stream_text(&mut text_content, &text);
                            }
                            if let Some(reasoning) = choice.message.reasoning_content {
                                reasoning_content_accum.push_str(&reasoning);
                            }
                            if let Some(final_tool_calls) = choice.message.tool_calls {
                                for tc in final_tool_calls {
                                    tool_calls.insert(
                                        tool_calls.len(),
                                        (tc.id, tc.function.name, tc.function.arguments),
                                    );
                                }
                            }
                        }
                    }
                }

                if finish_reason.is_none() {
                    if stream_failed {
                        tracing::warn!(
                            provider = %provider_name,
                            model = %request_model,
                            "OpenAI-compatible stream failed before terminal evidence; closing without Done"
                        );
                    } else if parsed_any_event {
                        tracing::warn!(
                            provider = %provider_name,
                            model = %request_model,
                            "OpenAI-compatible stream reached EOF before terminal evidence; closing without Done"
                        );
                    } else {
                        tracing::warn!(
                            provider = %provider_name,
                            model = %request_model,
                            trailing = %trailing.chars().take(400).collect::<String>(),
                            "OpenAI-compatible stream ended without any parseable events"
                        );
                    }
                    return;
                }

                if parsed_any_event
                    || !text_content.is_empty()
                    || !tool_calls.is_empty()
                    || !content_blocks.is_empty()
                {
                    tracing::warn!(
                        provider = %provider_name,
                        model = %request_model,
                        "OpenAI-compatible stream ended without [DONE] after finish reason; finalizing buffered response"
                    );
                    if !text_content.is_empty() {
                        content_blocks.push(ContentBlock::Text {
                            text: text_content.clone(),
                        });
                    }
                    for (id, name, args) in tool_calls.values() {
                        content_blocks.push(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: Self::parse_tool_arguments(name, args),
                        });
                    }
                    tool_calls.clear();
                    crate::telemetry::record_llm_usage(
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        usage.total_tokens,
                        finish_reason.as_deref(),
                    );
                    let response = LlmResponse {
                        message: Message {
                            role: "assistant".to_string(),
                            content: std::mem::take(&mut content_blocks),
                            reasoning_content: if reasoning_content_accum.is_empty() {
                                None
                            } else {
                                Some(std::mem::take(&mut reasoning_content_accum))
                            },
                        },
                        usage: usage.clone(),
                        stop_reason: std::mem::take(&mut finish_reason),
                        token_logprobs: std::mem::take(&mut token_logprobs),
                        meta: Some(LlmResponseMeta {
                            provider: Some(provider_name.clone()),
                            request_model: Some(request_model.clone()),
                            request_url: Some(request_url.clone()),
                            response_id: response_id.clone(),
                            response_model: response_model.clone(),
                            response_object: response_object.clone(),
                            first_token_ms,
                            duration_ms: Some(request_started_at.elapsed().as_millis() as u64),
                        }),
                    };
                    let _ = tx.send(StreamEvent::Done(response)).await;
                }
            });

            Ok(rx)
        }
    }
}

pub(super) fn openai_logprobs_to_token_logprobs(
    logprobs: &OpenAiChoiceLogprobs,
) -> Vec<TokenLogProb> {
    logprobs
        .content
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|item| TokenLogProb {
                    token: item.token.clone(),
                    logprob: item.logprob,
                    bytes: item.bytes.clone(),
                    top_logprobs: item
                        .top_logprobs
                        .iter()
                        .map(|top| TopTokenLogProb {
                            token: top.token.clone(),
                            logprob: top.logprob,
                            bytes: top.bytes.clone(),
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default()
}
