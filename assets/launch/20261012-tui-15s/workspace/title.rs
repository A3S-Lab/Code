pub fn title(input: &str) -> &str {
    let value = input.trim();
    if value.is_empty() { "Untitled" } else { value }
}
