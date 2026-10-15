#![cfg(test)]

use super::*;

#[test]
fn typed_host_budget_denial_is_terminal_even_with_context() {
    for wrapped in [false, true] {
        let denial = crate::error::CodeError::BudgetExhausted {
            resource: "original host tokens".into(),
            reason: "host allocation exhausted".into(),
        };
        let expected = denial.to_string();
        let error = anyhow::Error::new(denial);
        let error = if wrapped {
            error.context("original model child")
        } else {
            error
        };
        assert!(matches!(model_error(error), ActorError::Defect(message) if message == expected));
    }
}

#[test]
fn identical_untyped_budget_words_remain_retryable_provider_errors() {
    let text = crate::error::CodeError::BudgetExhausted {
        resource: "original host tokens".into(),
        reason: "host allocation exhausted".into(),
    }
    .to_string();
    assert!(matches!(
        model_error(anyhow::anyhow!(text.clone())),
        ActorError::Handler { key, message } if key == "infer" && message == text
    ));
    assert!(matches!(
        model_error(crate::llm::NonRetryableLlmError::new("original provider quota").into()),
        ActorError::Defect(message) if message == "original provider quota"
    ));
}
