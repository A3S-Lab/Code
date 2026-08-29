use super::render_env_block;

#[test]
fn env_block_contains_grounding_facts() {
    let block = render_env_block(std::path::Path::new("/tmp/demo-ws"));
    assert!(block.starts_with("<env>"), "block: {block}");
    assert!(block.trim_end().ends_with("</env>"));
    assert!(block.contains("Working directory: /tmp/demo-ws"));
    assert!(block.contains("Platform:"));
    assert!(block.contains(std::env::consts::OS));
    assert!(block.contains("Today's date:"));
}

#[test]
fn env_block_date_is_iso_yyyy_mm_dd() {
    let block = render_env_block(std::path::Path::new("/tmp"));
    let line = block
        .lines()
        .find(|l| l.starts_with("Today's date:"))
        .expect("date line present");
    let date = line.trim_start_matches("Today's date:").trim();
    assert_eq!(date.len(), 10, "date not YYYY-MM-DD: {date}");
    assert_eq!(date.matches('-').count(), 2, "date not YYYY-MM-DD: {date}");
    assert!(date.chars().all(|c| c.is_ascii_digit() || c == '-'));
}
