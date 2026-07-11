//! Incremental repair of partial JSON values produced by streaming models.

use serde_json::Value;

// ---------------------------------------------------------------------------
// Partial JSON parsing (for streaming)
// ---------------------------------------------------------------------------

/// Attempt to parse a potentially incomplete JSON string into the most complete
/// valid partial object possible.
///
/// Strategy: try parsing as-is first. If that fails, run a single-pass JSON
/// scanner that trims incomplete trailing tokens and closes open containers.
/// This mirrors the practical behavior of Vercel AI SDK's partial JSON parser
/// while keeping A3S's final schema validation strict.
#[cfg(test)]
pub(super) fn try_parse_partial_json(text: &str) -> Option<Value> {
    parse_partial_json(text).map(|parsed| parsed.value)
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PartialJsonValue {
    pub(super) value: Value,
    pub(super) repaired: bool,
}

pub(super) fn parse_partial_json(text: &str) -> Option<PartialJsonValue> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Fast path: already valid
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if v.is_object() || v.is_array() {
            return Some(PartialJsonValue {
                value: v,
                repaired: false,
            });
        }
    }

    let repaired = fix_partial_json(trimmed);
    if repaired.trim().is_empty() || repaired == trimmed {
        return None;
    }

    serde_json::from_str::<Value>(&repaired)
        .ok()
        .filter(|v| v.is_object() || v.is_array())
        .map(|value| PartialJsonValue {
            value,
            repaired: true,
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartialJsonState {
    Root,
    Finish,
    InsideString,
    InsideStringEscape,
    InsideStringUnicodeEscape,
    InsideLiteral,
    InsideNumber,
    InsideObjectStart,
    InsideObjectKey,
    InsideObjectAfterKey,
    InsideObjectBeforeValue,
    InsideObjectAfterValue,
    InsideObjectAfterComma,
    InsideArrayStart,
    InsideArrayAfterValue,
    InsideArrayAfterComma,
}

fn is_json_hex_digit(ch: char) -> bool {
    ch.is_ascii_hexdigit()
}

fn process_partial_value_start(
    ch: char,
    end: usize,
    swap_state: PartialJsonState,
    stack: &mut Vec<PartialJsonState>,
    last_valid_end: &mut usize,
    literal_start: &mut Option<usize>,
    start: usize,
) {
    match ch {
        '"' => {
            *last_valid_end = end;
            stack.pop();
            stack.push(swap_state);
            stack.push(PartialJsonState::InsideString);
        }
        'f' | 't' | 'n' => {
            *last_valid_end = end;
            *literal_start = Some(start);
            stack.pop();
            stack.push(swap_state);
            stack.push(PartialJsonState::InsideLiteral);
        }
        '-' => {
            stack.pop();
            stack.push(swap_state);
            stack.push(PartialJsonState::InsideNumber);
        }
        '0'..='9' => {
            *last_valid_end = end;
            stack.pop();
            stack.push(swap_state);
            stack.push(PartialJsonState::InsideNumber);
        }
        '{' => {
            *last_valid_end = end;
            stack.pop();
            stack.push(swap_state);
            stack.push(PartialJsonState::InsideObjectStart);
        }
        '[' => {
            *last_valid_end = end;
            stack.pop();
            stack.push(swap_state);
            stack.push(PartialJsonState::InsideArrayStart);
        }
        _ => {}
    }
}

fn process_after_partial_object_value(
    ch: char,
    end: usize,
    stack: &mut Vec<PartialJsonState>,
    last_valid_end: &mut usize,
) {
    match ch {
        ',' => {
            stack.pop();
            stack.push(PartialJsonState::InsideObjectAfterComma);
        }
        '}' => {
            *last_valid_end = end;
            stack.pop();
        }
        _ => {}
    }
}

fn process_after_partial_array_value(
    ch: char,
    end: usize,
    stack: &mut Vec<PartialJsonState>,
    last_valid_end: &mut usize,
) {
    match ch {
        ',' => {
            stack.pop();
            stack.push(PartialJsonState::InsideArrayAfterComma);
        }
        ']' => {
            *last_valid_end = end;
            stack.pop();
        }
        _ => {}
    }
}

fn fix_partial_json(input: &str) -> String {
    use PartialJsonState::*;

    let mut stack = vec![Root];
    let mut last_valid_end = 0usize;
    let mut literal_start: Option<usize> = None;
    let mut unicode_escape_digits = 0usize;

    for (start, ch) in input.char_indices() {
        let end = start + ch.len_utf8();
        let current_state = *stack.last().unwrap_or(&Finish);

        match current_state {
            Root => {
                process_partial_value_start(
                    ch,
                    end,
                    Finish,
                    &mut stack,
                    &mut last_valid_end,
                    &mut literal_start,
                    start,
                );
            }
            InsideObjectStart => match ch {
                '"' => {
                    stack.pop();
                    stack.push(InsideObjectKey);
                }
                '}' => {
                    last_valid_end = end;
                    stack.pop();
                }
                _ => {}
            },
            InsideObjectAfterComma => {
                if ch == '"' {
                    stack.pop();
                    stack.push(InsideObjectKey);
                }
            }
            InsideObjectKey => {
                if ch == '"' {
                    stack.pop();
                    stack.push(InsideObjectAfterKey);
                }
            }
            InsideObjectAfterKey => {
                if ch == ':' {
                    stack.pop();
                    stack.push(InsideObjectBeforeValue);
                }
            }
            InsideObjectBeforeValue => {
                process_partial_value_start(
                    ch,
                    end,
                    InsideObjectAfterValue,
                    &mut stack,
                    &mut last_valid_end,
                    &mut literal_start,
                    start,
                );
            }
            InsideObjectAfterValue => {
                process_after_partial_object_value(ch, end, &mut stack, &mut last_valid_end);
            }
            InsideString => match ch {
                '"' => {
                    stack.pop();
                    last_valid_end = end;
                }
                '\\' => {
                    stack.push(InsideStringEscape);
                }
                _ => {
                    last_valid_end = end;
                }
            },
            InsideArrayStart => match ch {
                ']' => {
                    last_valid_end = end;
                    stack.pop();
                }
                _ => {
                    last_valid_end = end;
                    process_partial_value_start(
                        ch,
                        end,
                        InsideArrayAfterValue,
                        &mut stack,
                        &mut last_valid_end,
                        &mut literal_start,
                        start,
                    );
                }
            },
            InsideArrayAfterValue => match ch {
                ',' => {
                    stack.pop();
                    stack.push(InsideArrayAfterComma);
                }
                ']' => {
                    last_valid_end = end;
                    stack.pop();
                }
                _ => {
                    last_valid_end = end;
                }
            },
            InsideArrayAfterComma => {
                process_partial_value_start(
                    ch,
                    end,
                    InsideArrayAfterValue,
                    &mut stack,
                    &mut last_valid_end,
                    &mut literal_start,
                    start,
                );
            }
            InsideStringEscape => {
                stack.pop();
                if ch == 'u' {
                    unicode_escape_digits = 0;
                    stack.push(InsideStringUnicodeEscape);
                } else {
                    last_valid_end = end;
                }
            }
            InsideStringUnicodeEscape => {
                if is_json_hex_digit(ch) {
                    unicode_escape_digits += 1;
                    if unicode_escape_digits == 4 {
                        stack.pop();
                        last_valid_end = end;
                    }
                }
            }
            InsideNumber => match ch {
                '0'..='9' => {
                    last_valid_end = end;
                }
                'e' | 'E' | '-' | '.' => {}
                ',' => {
                    stack.pop();
                    if stack.last() == Some(&InsideArrayAfterValue) {
                        process_after_partial_array_value(ch, end, &mut stack, &mut last_valid_end);
                    }
                    if stack.last() == Some(&InsideObjectAfterValue) {
                        process_after_partial_object_value(
                            ch,
                            end,
                            &mut stack,
                            &mut last_valid_end,
                        );
                    }
                }
                '}' => {
                    stack.pop();
                    if stack.last() == Some(&InsideObjectAfterValue) {
                        process_after_partial_object_value(
                            ch,
                            end,
                            &mut stack,
                            &mut last_valid_end,
                        );
                    }
                }
                ']' => {
                    stack.pop();
                    if stack.last() == Some(&InsideArrayAfterValue) {
                        process_after_partial_array_value(ch, end, &mut stack, &mut last_valid_end);
                    }
                }
                _ => {
                    stack.pop();
                }
            },
            InsideLiteral => {
                let partial_literal = literal_start
                    .and_then(|s| input.get(s..end))
                    .unwrap_or_default();
                if !("false".starts_with(partial_literal)
                    || "true".starts_with(partial_literal)
                    || "null".starts_with(partial_literal))
                {
                    stack.pop();
                    if stack.last() == Some(&InsideObjectAfterValue) {
                        process_after_partial_object_value(
                            ch,
                            end,
                            &mut stack,
                            &mut last_valid_end,
                        );
                    } else if stack.last() == Some(&InsideArrayAfterValue) {
                        process_after_partial_array_value(ch, end, &mut stack, &mut last_valid_end);
                    }
                } else {
                    last_valid_end = end;
                }
            }
            Finish => {}
        }
    }

    let mut result = input[..last_valid_end].to_string();
    for state in stack.iter().rev() {
        match state {
            InsideString => result.push('"'),
            InsideObjectKey
            | InsideObjectAfterKey
            | InsideObjectAfterComma
            | InsideObjectStart
            | InsideObjectBeforeValue
            | InsideObjectAfterValue => result.push('}'),
            InsideArrayStart | InsideArrayAfterComma | InsideArrayAfterValue => result.push(']'),
            InsideLiteral => {
                let partial_literal = literal_start
                    .and_then(|s| input.get(s..input.len()))
                    .unwrap_or_default();
                if "true".starts_with(partial_literal) {
                    result.push_str(&"true"[partial_literal.len()..]);
                } else if "false".starts_with(partial_literal) {
                    result.push_str(&"false"[partial_literal.len()..]);
                } else if "null".starts_with(partial_literal) {
                    result.push_str(&"null"[partial_literal.len()..]);
                }
            }
            _ => {}
        }
    }

    result
}
