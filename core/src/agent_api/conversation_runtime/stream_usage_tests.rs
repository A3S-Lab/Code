#![cfg(test)]

use super::*;

#[tokio::test]
async fn public_stream_end_preserves_mapped_usage_including_cache_tokens() {
    let result = AgentResult {
        text: "source analysis complete".into(),
        messages: Vec::new(),
        usage: crate::llm::TokenUsage {
            prompt_tokens: 701,
            completion_tokens: 43,
            total_tokens: 744,
            cache_read_tokens: Some(512),
            cache_write_tokens: Some(128),
        },
        tool_calls_count: 0,
        verification_reports: Vec::new(),
        completion: crate::harness_loop::CompletionTerminal::Narrative,
        forced_stop: None,
        run_admission: "ordinary".into(),
    };
    let mut view = a3s_effect::CodingView::empty();
    view.assistant = Some(result.text.clone());
    let settled = Ok(a3s_effect::Settlement {
        view,
        log: Vec::new(),
        steps: 0,
        cut: a3s_effect::cut_log(&[]),
        trace: Vec::new(),
    });
    for streamed_text in [false, true] {
        let (tx, mut rx) = mpsc::channel(4);
        emit_settled(&tx, &settled, Some(&result), streamed_text).await;
        drop(tx);
        let mut ends = 0;
        let mut deltas = 0;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::TextDelta { text } => {
                    assert_eq!(text, result.text);
                    deltas += 1;
                }
                AgentEvent::End { text, usage, .. } => {
                    assert_eq!(text, result.text);
                    assert_eq!(
                        serde_json::to_value(usage).unwrap(),
                        serde_json::to_value(&result.usage).unwrap()
                    );
                    ends += 1;
                }
                other => panic!("unexpected public settlement event: {other:?}"),
            }
        }
        assert_eq!(ends, 1);
        assert_eq!(deltas, usize::from(!streamed_text));
    }
}
