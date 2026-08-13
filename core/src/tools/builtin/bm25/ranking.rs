pub(super) use crate::workspace::retrieval::lexical::{
    query_terms, score_documents, tokenize, Bm25Document, B, K1,
};

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
