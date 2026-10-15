use crate::error::{CodeError, Result};
use crate::goal_control::GoalContinuationPolicyV1;
use a3s_acl::{Document, Lexer, Token, Value};
use std::collections::HashSet;
use std::num::NonZeroU64;

fn invalid(message: impl Into<String>) -> CodeError {
    CodeError::Config(format!("goal_control: {}", message.into()))
}

pub(super) fn parse_layer(
    source: &str,
    document: &Document,
) -> Result<Option<GoalContinuationPolicyV1>> {
    let mut declarations = document
        .blocks
        .iter()
        .filter(|block| block.name == "goal_control");
    let Some(block) = declarations.next() else {
        return Ok(None);
    };
    if declarations.next().is_some() {
        return Err(invalid("duplicate declaration in one ACL layer"));
    }
    require_closed_block(source)?;
    if !block.labels.is_empty() || !block.blocks.is_empty() {
        return Err(invalid("requires an unlabeled block without nested blocks"));
    }
    let fields = &block.attributes;
    let Some(Value::Bool(automatic_enabled)) = fields.get("automatic_enabled") else {
        return Err(invalid("automatic_enabled requires an explicit boolean"));
    };
    let positive = |key: &str| -> Result<NonZeroU64> {
        if let Some(Value::String(text)) = fields.get(key) {
            if let Ok(value) = text.parse::<NonZeroU64>() {
                if value.to_string() == *text {
                    return Ok(value);
                }
            }
        }
        Err(invalid(format!(
            "{key} requires a canonical positive u64 decimal string"
        )))
    };
    let policy = GoalContinuationPolicyV1 {
        automatic_enabled: *automatic_enabled,
        max_slices: positive("max_slices")?,
        max_no_progress_slices: positive("max_no_progress_slices")?,
        deadline_unix_ms: positive("deadline_unix_ms")?,
    };
    policy
        .check_bounds()
        .map_err(|error| invalid(error.to_string()))?;
    Ok(Some(policy))
}

fn require_closed_block(source: &str) -> Result<()> {
    // The ACL AST erases duplicate attributes and bare identifiers' literal types.
    let mut tokens = Lexer::new(source)
        .tokenize()
        .into_iter()
        .map(|token| token.token)
        .filter(|token| !matches!(token, Token::Comment | Token::Newline));
    let mut depth = 0usize;
    let mut found = false;
    while let Some(token) = tokens.next() {
        match token {
            Token::Ident(name) if depth == 0 && name == "goal_control" => {
                if found || tokens.next() != Some(Token::LeftBrace) {
                    return Err(invalid("requires one unlabeled goal_control { ... } block"));
                }
                found = true;
                let mut fields = HashSet::new();
                loop {
                    let key = match tokens.next() {
                        Some(Token::RightBrace) => break,
                        Some(Token::Ident(key)) => key,
                        _ => {
                            return Err(invalid(
                                "requires literal field assignments and a closing brace",
                            ))
                        }
                    };
                    if !matches!(
                        key.as_str(),
                        "automatic_enabled"
                            | "max_slices"
                            | "max_no_progress_slices"
                            | "deadline_unix_ms"
                    ) {
                        return Err(invalid(
                            "field is not in the closed set automatic_enabled, max_slices, max_no_progress_slices, deadline_unix_ms",
                        ));
                    }
                    if !fields.insert(key.clone()) {
                        return Err(invalid(format!("duplicate {key}")));
                    }
                    if tokens.next() != Some(Token::Equal) {
                        return Err(invalid("requires field = literal assignments"));
                    }
                    let literal = tokens.next();
                    let valid = if key == "automatic_enabled" {
                        matches!(literal, Some(Token::True | Token::False))
                    } else {
                        matches!(literal, Some(Token::String(_)))
                    };
                    if !valid {
                        return Err(invalid(format!("{key} requires its declared literal type")));
                    }
                }
            }
            Token::LeftBrace | Token::LeftBracket | Token::LeftParen => depth += 1,
            Token::RightBrace | Token::RightBracket | Token::RightParen => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    if found {
        Ok(())
    } else {
        Err(invalid("top-level declaration is unavailable"))
    }
}

#[cfg(test)]
mod tests;
