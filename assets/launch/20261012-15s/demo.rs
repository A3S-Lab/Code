fn title(input: &str) -> &str {
    let value = input.trim();
    if value.is_empty() { "Untitled" } else { value }
}

#[test]
fn empty_input() { assert_eq!(title(""), "Untitled"); }
#[test]
fn whitespace_input() { assert_eq!(title("  \n  "), "Untitled"); }
#[test]
fn normal_input() { assert_eq!(title("  A3S Code  "), "A3S Code"); }
