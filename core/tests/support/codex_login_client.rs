use std::path::Path;

pub use a3s_code_core::llm::CodexLoginClient;

pub fn default_codex_model() -> String {
    std::env::var("A3S_CODEX_MODEL")
        .ok()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| {
            codex_models_from_cache()
                .into_iter()
                .next()
                .unwrap_or_else(|| "gpt-5.5".to_string())
        })
}

fn codex_models_from_cache() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let path = Path::new(&home).join(".codex/models_cache.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };

    let Some(models) = cache.get("models").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut listed: Vec<(i64, String)> = models
        .iter()
        .filter_map(|model| {
            if model.get("visibility").and_then(serde_json::Value::as_str) != Some("list") {
                return None;
            }
            let slug = model
                .get("slug")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            let priority = model
                .get("priority")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(999);
            Some((priority, slug))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    listed.into_iter().map(|(_, slug)| slug).collect()
}
