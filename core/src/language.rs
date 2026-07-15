//! Shared programming-language identification.

use std::path::Path;

/// Stateless catalog for the language identifiers used across the crate.
pub(crate) struct LanguageCatalog;

impl LanguageCatalog {
    /// Return the language identifier associated with a path extension.
    ///
    /// Extension matching is intentionally case-sensitive to preserve the
    /// workspace manifest's existing classification behavior.
    pub(crate) fn id_for_path(path: &Path) -> Option<&'static str> {
        match path.extension().and_then(|ext| ext.to_str())? {
            "rs" => Some("rust"),
            "toml" => Some("toml"),
            "hcl" => Some("hcl"),
            "js" | "mjs" | "cjs" => Some("javascript"),
            "jsx" => Some("javascript-react"),
            "ts" | "mts" | "cts" => Some("typescript"),
            "tsx" => Some("typescript-react"),
            "json" => Some("json"),
            "md" | "mdx" => Some("markdown"),
            "py" => Some("python"),
            "go" => Some("go"),
            "java" => Some("java"),
            "kt" | "kts" => Some("kotlin"),
            "swift" => Some("swift"),
            "c" | "h" => Some("c"),
            "cc" | "cpp" | "cxx" | "hpp" => Some("cpp"),
            "cs" => Some("csharp"),
            "rb" => Some("ruby"),
            "php" => Some("php"),
            "sh" | "bash" | "zsh" => Some("shell"),
            "yml" | "yaml" => Some("yaml"),
            "html" | "htm" => Some("html"),
            "css" => Some("css"),
            "scss" | "sass" => Some("scss"),
            "sql" => Some("sql"),
            "xml" => Some("xml"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguageCatalog;
    use std::path::Path;

    #[test]
    fn preserves_all_manifest_language_identifiers_and_aliases() {
        let cases = [
            ("main.rs", "rust"),
            ("Cargo.toml", "toml"),
            ("agent.hcl", "hcl"),
            ("index.js", "javascript"),
            ("index.mjs", "javascript"),
            ("index.cjs", "javascript"),
            ("view.jsx", "javascript-react"),
            ("index.ts", "typescript"),
            ("index.mts", "typescript"),
            ("index.cts", "typescript"),
            ("view.tsx", "typescript-react"),
            ("data.json", "json"),
            ("README.md", "markdown"),
            ("guide.mdx", "markdown"),
            ("main.py", "python"),
            ("main.go", "go"),
            ("Main.java", "java"),
            ("Main.kt", "kotlin"),
            ("build.kts", "kotlin"),
            ("main.swift", "swift"),
            ("main.c", "c"),
            ("main.h", "c"),
            ("main.cc", "cpp"),
            ("main.cpp", "cpp"),
            ("main.cxx", "cpp"),
            ("main.hpp", "cpp"),
            ("Main.cs", "csharp"),
            ("main.rb", "ruby"),
            ("index.php", "php"),
            ("run.sh", "shell"),
            ("run.bash", "shell"),
            ("run.zsh", "shell"),
            ("config.yml", "yaml"),
            ("config.yaml", "yaml"),
            ("index.html", "html"),
            ("index.htm", "html"),
            ("style.css", "css"),
            ("style.scss", "scss"),
            ("style.sass", "scss"),
            ("schema.sql", "sql"),
            ("document.xml", "xml"),
        ];

        for (path, expected) in cases {
            assert_eq!(
                LanguageCatalog::id_for_path(Path::new(path)),
                Some(expected),
                "unexpected language for {path}"
            );
        }
    }

    #[test]
    fn keeps_extension_matching_case_sensitive() {
        assert_eq!(LanguageCatalog::id_for_path(Path::new("main.RS")), None);
        assert_eq!(LanguageCatalog::id_for_path(Path::new("README")), None);
        assert_eq!(LanguageCatalog::id_for_path(Path::new("archive.zip")), None);
    }
}
