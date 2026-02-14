//! Pathway - URI-like addressing for context nodes

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pathway {
    raw: String,
    namespace: String,
    path: String,
}

impl Pathway {
    pub fn parse(s: &str) -> super::error::Result<Self> {
        let s = s.trim();
        let stripped = s.strip_prefix("a3s://").unwrap_or(s);
        let (namespace, path) = match stripped.split_once('/') {
            Some((ns, p)) => (ns.to_string(), p.to_string()),
            None => (stripped.to_string(), String::new()),
        };
        if namespace.is_empty() {
            return Err(super::error::A3SError::Pathway("Empty namespace".to_string()));
        }
        Ok(Self { raw: format!("a3s://{}/{}", namespace, path), namespace, path })
    }

    pub fn new(namespace: &str, path: &str) -> Self {
        Self {
            raw: format!("a3s://{}/{}", namespace, path),
            namespace: namespace.to_string(),
            path: path.to_string(),
        }
    }

    pub fn namespace(&self) -> &str { &self.namespace }
    pub fn path(&self) -> &str { &self.path }
    pub fn as_str(&self) -> &str { &self.raw }

    pub fn join(&self, segment: &str) -> Self {
        let new_path = if self.path.is_empty() {
            segment.to_string()
        } else {
            format!("{}/{}", self.path, segment)
        };
        Self::new(&self.namespace, &new_path)
    }

    pub fn parent(&self) -> Option<Self> {
        if self.path.is_empty() { return None; }
        match self.path.rsplit_once('/') {
            Some((parent, _)) => Some(Self::new(&self.namespace, parent)),
            None => Some(Self::new(&self.namespace, "")),
        }
    }
}

impl std::fmt::Display for Pathway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_uri() {
        let p = Pathway::parse("a3s://docs/readme.md").unwrap();
        assert_eq!(p.namespace(), "docs");
        assert_eq!(p.path(), "readme.md");
    }

    #[test]
    fn test_parse_without_prefix() {
        let p = Pathway::parse("docs/readme.md").unwrap();
        assert_eq!(p.namespace(), "docs");
        assert_eq!(p.path(), "readme.md");
    }

    #[test]
    fn test_parse_namespace_only() {
        let p = Pathway::parse("a3s://docs").unwrap();
        assert_eq!(p.namespace(), "docs");
        assert_eq!(p.path(), "");
    }

    #[test]
    fn test_parse_empty_fails() {
        assert!(Pathway::parse("a3s://").is_err());
    }

    #[test]
    fn test_join() {
        let p = Pathway::new("docs", "src");
        let child = p.join("main.rs");
        assert_eq!(child.path(), "src/main.rs");
    }

    #[test]
    fn test_parent() {
        let p = Pathway::new("docs", "src/main.rs");
        let parent = p.parent().unwrap();
        assert_eq!(parent.path(), "src");
        let root = parent.parent().unwrap();
        assert_eq!(root.path(), "");
        assert!(root.parent().is_none());
    }

    #[test]
    fn test_display() {
        let p = Pathway::new("docs", "readme.md");
        assert_eq!(p.to_string(), "a3s://docs/readme.md");
    }
}
