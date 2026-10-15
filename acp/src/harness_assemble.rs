//! Studio passes one admitted assemble on `_meta.a3sHarness.components`.
//! Any other list is refused. An absent field keeps the default stock list.

use a3s_code_core::{BuiltinHostHarnessRegistry, HarnessComposeOptions, SessionOptions};
use serde_json::Value;
use std::sync::Arc;

const DEFAULT: &[&str] = &["system", "tools", "budget", "compact", "infer"];
const LEAN: &[&str] = &["system", "tools", "budget", "infer"];
const HOST: &[&str] = &[
    "system",
    "tools",
    "host:intent_stamp",
    "budget",
    "compact",
    "infer",
];

pub fn components_from_meta(
    meta: Option<&serde_json::Map<String, Value>>,
) -> Result<Vec<String>, String> {
    let Some(harness) = meta.and_then(|meta| meta.get("a3sHarness")) else {
        return Ok(owned(DEFAULT));
    };
    let Some(items) = harness.get("components").and_then(Value::as_array) else {
        return Err("a3sHarness.components must be a list".to_owned());
    };
    let names: Vec<String> = items
        .iter()
        .map(|item| item.as_str().unwrap_or("").to_owned())
        .collect();
    if same(&names, DEFAULT) || same(&names, LEAN) || same(&names, HOST) {
        return Ok(names);
    }
    Err("a3sHarness.components is not an admitted assemble".to_owned())
}

pub fn apply(options: SessionOptions, components: &[String]) -> Result<SessionOptions, String> {
    if components.is_empty() {
        return Ok(options);
    }
    let compose = HarnessComposeOptions::compose(components.to_vec(), None, None, Vec::new())
        .map_err(|error| error.to_string())?;
    let mut options = options.with_harness(compose);
    if components.iter().any(|name| name.starts_with("host:")) {
        options = options.with_host_harness_registry(Arc::new(BuiltinHostHarnessRegistry));
    }
    Ok(options)
}

fn same(names: &[String], recipe: &[&str]) -> bool {
    names.len() == recipe.len()
        && names
            .iter()
            .zip(recipe)
            .all(|(name, expected)| name == expected)
}

fn owned(recipe: &[&str]) -> Vec<String> {
    recipe.iter().map(|name| (*name).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_meta_is_the_default_list() {
        let names = components_from_meta(None).expect("default");
        assert_eq!(names, owned(DEFAULT));
    }

    #[test]
    fn host_list_installs_the_builtin_registry() {
        let meta = json!({ "a3sHarness": { "components": HOST } });
        let names = components_from_meta(meta.as_object()).expect("host");
        let options = apply(SessionOptions::new(), &names).expect("apply");
        let harness = options.harness.expect("harness");
        assert_eq!(harness.components, names);
        assert!(options.host_harness_registry.is_some());
    }

    #[test]
    fn a_fourth_list_is_refused() {
        let meta = json!({ "a3sHarness": { "components": ["system", "infer"] } });
        assert!(components_from_meta(meta.as_object()).is_err());
    }
}
