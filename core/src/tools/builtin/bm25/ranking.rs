pub(super) use crate::workspace::retrieval::lexical::{query_terms, tokenize, VecLexicalIndex};

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
    fn vec_fts_prefers_documents_covering_more_query_terms() {
        let index = VecLexicalIndex::build([
            ("short", "cache cache cache"),
            ("broad", "cache invalidation policy"),
        ])
        .expect("Vec FTS index must build");
        let hits = index
            .search(&query_terms("cache invalidation", 16), 2)
            .expect("Vec FTS query must succeed");

        assert_eq!(hits[0].0, 1, "hits: {hits:?}");
        assert!(hits[0].1 > hits[1].1, "hits: {hits:?}");
    }

    #[test]
    fn vec_fts_applies_document_length_normalization() {
        let index = VecLexicalIndex::build([
            ("compact", "needle compact"),
            (
                "long",
                "needle filler filler filler filler filler filler filler filler filler",
            ),
        ])
        .expect("Vec FTS index must build");
        let hits = index
            .search(&query_terms("needle", 16), 2)
            .expect("Vec FTS query must succeed");

        assert_eq!(hits[0].0, 0, "hits: {hits:?}");
        assert!(hits[0].1 > hits[1].1, "hits: {hits:?}");
    }
}
