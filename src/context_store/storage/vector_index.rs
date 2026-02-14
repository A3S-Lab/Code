//! In-memory vector index for cosine similarity search

use super::super::config::VectorIndexConfig;

pub struct VectorIndex {
    dimension: usize,
    vectors: Vec<(String, Vec<f32>)>,
    _config: VectorIndexConfig,
}

impl VectorIndex {
    pub fn new(dimension: usize, config: &VectorIndexConfig) -> Self {
        Self { dimension, vectors: Vec::new(), _config: config.clone() }
    }

    pub fn insert(&mut self, id: String, vector: Vec<f32>) {
        if vector.len() != self.dimension { return; }
        self.vectors.retain(|(k, _)| k != &id);
        self.vectors.push((id, vector));
    }

    pub fn remove(&mut self, id: &str) {
        self.vectors.retain(|(k, _)| k != id);
    }

    pub fn search(&self, query: &[f32], limit: usize, threshold: f32) -> Vec<(String, f32)> {
        if query.len() != self.dimension { return Vec::new(); }
        let mut results: Vec<(String, f32)> = self.vectors.iter()
            .map(|(id, vec)| (id.clone(), cosine_similarity(query, vec)))
            .filter(|(_, score)| *score >= threshold)
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        results
    }

    pub fn len(&self) -> usize { self.vectors.len() }
    pub fn is_empty(&self) -> bool { self.vectors.is_empty() }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::config::VectorIndexConfig;

    fn default_config() -> VectorIndexConfig { VectorIndexConfig::default() }

    #[test]
    fn test_insert_and_search() {
        let mut index = VectorIndex::new(3, &default_config());
        index.insert("a".to_string(), vec![1.0, 0.0, 0.0]);
        index.insert("b".to_string(), vec![0.0, 1.0, 0.0]);
        let results = index.search(&[1.0, 0.0, 0.0], 10, 0.0);
        assert_eq!(results[0].0, "a");
        assert!((results[0].1 - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_remove() {
        let mut index = VectorIndex::new(3, &default_config());
        index.insert("a".to_string(), vec![1.0, 0.0, 0.0]);
        assert_eq!(index.len(), 1);
        index.remove("a");
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn test_threshold_filter() {
        let mut index = VectorIndex::new(3, &default_config());
        index.insert("a".to_string(), vec![1.0, 0.0, 0.0]);
        index.insert("b".to_string(), vec![0.0, 1.0, 0.0]);
        let results = index.search(&[1.0, 0.0, 0.0], 10, 0.9);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }
}
