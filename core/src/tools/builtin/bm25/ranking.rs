use std::collections::{HashMap, HashSet};

pub(super) const K1: f64 = 1.2;
pub(super) const B: f64 = 0.75;

#[derive(Debug, Clone)]
pub(super) struct Bm25Document {
    pub(super) term_frequencies: HashMap<String, u32>,
    pub(super) length: usize,
}

impl Bm25Document {
    pub(super) fn from_text(text: &str) -> Self {
        let tokens = tokenize(text);
        let mut term_frequencies = HashMap::new();
        for token in &tokens {
            *term_frequencies.entry(token.clone()).or_insert(0) += 1;
        }
        Self {
            term_frequencies,
            length: tokens.len(),
        }
    }
}

pub(super) fn query_terms(query: &str, limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    tokenize(query)
        .into_iter()
        .filter(|term| seen.insert(term.clone()))
        .take(limit)
        .collect()
}

pub(super) fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut previous_cjk = None;

    for ch in text.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut tokens);
            tokens.push(ch.to_string());
            if let Some(previous) = previous_cjk {
                tokens.push(format!("{previous}{ch}"));
            }
            previous_cjk = Some(ch);
        } else {
            previous_cjk = None;
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
            } else {
                flush_word(&mut word, &mut tokens);
            }
        }
    }
    flush_word(&mut word, &mut tokens);
    tokens
}

pub(super) fn score_documents(query_terms: &[String], documents: &[Bm25Document]) -> Vec<f64> {
    let mut scores = vec![0.0; documents.len()];
    if query_terms.is_empty() || documents.is_empty() {
        return scores;
    }

    let document_count = documents.len() as f64;
    let average_document_length = documents
        .iter()
        .map(|document| document.length)
        .sum::<usize>() as f64
        / document_count;
    let average_document_length = average_document_length.max(1.0);
    let mut seen = HashSet::new();

    for term in query_terms {
        if !seen.insert(term.as_str()) {
            continue;
        }
        let document_frequency = documents
            .iter()
            .filter(|document| document.term_frequencies.contains_key(term))
            .count() as f64;
        if document_frequency == 0.0 {
            continue;
        }
        let inverse_document_frequency =
            (1.0 + (document_count - document_frequency + 0.5) / (document_frequency + 0.5)).ln();

        for (document, score) in documents.iter().zip(&mut scores) {
            let term_frequency = document
                .term_frequencies
                .get(term)
                .copied()
                .unwrap_or_default() as f64;
            if term_frequency == 0.0 {
                continue;
            }
            let length_ratio = document.length as f64 / average_document_length;
            let denominator = term_frequency + K1 * (1.0 - B + B * length_ratio);
            *score += inverse_document_frequency
                * (term_frequency * (K1 + 1.0) / denominator.max(f64::EPSILON));
        }
    }
    scores
}

fn flush_word(word: &mut String, tokens: &mut Vec<String>) {
    if word.is_empty() {
        return;
    }
    if !word.chars().any(char::is_alphanumeric) {
        word.clear();
        return;
    }

    let mut variants = Vec::new();
    variants.push(word.to_lowercase());
    for segment in word.split('_').filter(|segment| !segment.is_empty()) {
        variants.push(segment.to_lowercase());
        variants.extend(split_identifier(segment));
    }

    let mut seen = HashSet::new();
    tokens.extend(
        variants
            .into_iter()
            .filter(|variant| !variant.is_empty() && seen.insert(variant.clone())),
    );
    word.clear();
}

fn split_identifier(identifier: &str) -> Vec<String> {
    let chars = identifier.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut parts = Vec::new();
    let mut start = 0usize;
    for index in 1..chars.len() {
        let previous = chars[index - 1];
        let current = chars[index];
        let next = chars.get(index + 1).copied();
        let at_case_boundary = previous.is_lowercase() && current.is_uppercase();
        let at_acronym_boundary = previous.is_uppercase()
            && current.is_uppercase()
            && next.is_some_and(char::is_lowercase);
        let at_numeric_boundary = previous.is_numeric() != current.is_numeric()
            && (previous.is_alphanumeric() && current.is_alphanumeric());
        if at_case_boundary || at_acronym_boundary || at_numeric_boundary {
            parts.push(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .to_lowercase(),
            );
            start = index;
        }
    }
    parts.push(chars[start..].iter().collect::<String>().to_lowercase());
    parts
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            | 0x3040..=0x30ff
            | 0xac00..=0xd7af
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_preserves_identifiers_and_adds_code_subterms() {
        let tokens = tokenize("getUserProfile user_profile HTTPServer2");

        for expected in [
            "getuserprofile",
            "get",
            "user",
            "profile",
            "user_profile",
            "httpserver2",
            "http",
            "server",
            "2",
        ] {
            assert!(
                tokens.iter().any(|token| token == expected),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn tokenizer_adds_cjk_characters_and_bigrams() {
        let tokens = tokenize("用户检索");

        for expected in ["用", "户", "检", "索", "用户", "户检", "检索"] {
            assert!(
                tokens.iter().any(|token| token == expected),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn query_terms_are_unique_and_bounded() {
        assert_eq!(query_terms("alpha alpha beta gamma", 2), ["alpha", "beta"]);
    }

    #[test]
    fn bm25_prefers_documents_covering_more_query_terms() {
        let documents = [
            Bm25Document::from_text("cache cache cache"),
            Bm25Document::from_text("cache invalidation policy"),
        ];
        let scores = score_documents(&query_terms("cache invalidation", 16), &documents);

        assert!(scores[1] > scores[0], "scores: {scores:?}");
    }

    #[test]
    fn bm25_applies_document_length_normalization() {
        let documents = [
            Bm25Document::from_text("needle compact"),
            Bm25Document::from_text(
                "needle filler filler filler filler filler filler filler filler filler",
            ),
        ];
        let scores = score_documents(&query_terms("needle", 16), &documents);

        assert!(scores[0] > scores[1], "scores: {scores:?}");
    }
}
