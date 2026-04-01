use crate::document_parser::DocumentBlockLocation;

pub(crate) fn format_block_location(location: &DocumentBlockLocation) -> String {
    let mut parts = Vec::new();
    if let Some(source) = &location.source {
        if !source.trim().is_empty() {
            parts.push(format!("source={}", source.trim()));
        }
    }
    if let Some(page) = location.page {
        parts.push(format!("page={page}"));
    }
    if let Some(ordinal) = location.ordinal {
        parts.push(format!("ordinal={ordinal}"));
    }
    if location.continued_from_previous_page {
        parts.push("continued_from_previous_page=true".to_string());
    }
    if location.continued_to_next_page {
        parts.push("continued_to_next_page=true".to_string());
    }
    parts.join(", ")
}

pub(crate) fn derive_match_locator(search_lines: &[String], line_idx: usize) -> Option<String> {
    let mut page = None;
    let mut label = None;

    for idx in (0..=line_idx).rev().take(6) {
        let line = search_lines[idx].trim();
        if page.is_none() {
            page = parse_page_locator(line);
        }
        if label.is_none() {
            label = parse_structural_label(line);
        }
        if page.is_some() && label.is_some() {
            break;
        }
    }

    match (page, label) {
        (Some(page), Some(label)) => Some(format!("{page} | {label}")),
        (Some(page), None) => Some(page),
        (None, Some(label)) => Some(label),
        (None, None) => None,
    }
}

pub(crate) fn derive_locator_from_location_and_label(
    location: Option<&DocumentBlockLocation>,
    label: Option<&str>,
) -> Option<String> {
    let page = location
        .and_then(|location| location.page)
        .map(|page| format!("page {page}"));
    let label = label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToOwned::to_owned);

    match (page, label) {
        (Some(page), Some(label)) => Some(format!("{page} | {label}")),
        (Some(page), None) => Some(page),
        (None, Some(label)) => Some(label),
        (None, None) => None,
    }
}

fn parse_page_locator(line: &str) -> Option<String> {
    let payload = line.strip_prefix("[loc] ")?.trim();
    payload.split(',').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("page=")
            .map(|page| format!("page {page}"))
    })
}

fn parse_structural_label(line: &str) -> Option<String> {
    let payload = line.strip_prefix('[')?;
    let (_, value) = payload.split_once("] ")?;
    let value = value.trim();
    (!value.is_empty()).then_some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_block_location_includes_continuation_flags() {
        let location = DocumentBlockLocation {
            source: Some("report.pdf".to_string()),
            page: Some(2),
            ordinal: Some(4),
            continued_from_previous_page: true,
            continued_to_next_page: true,
        };

        assert_eq!(
            format_block_location(&location),
            "source=report.pdf, page=2, ordinal=4, continued_from_previous_page=true, continued_to_next_page=true"
        );
    }

    #[test]
    fn derive_match_locator_prefers_page_and_label_markers() {
        let lines = vec![
            "# report.pdf".to_string(),
            "[loc] source=scan.pdf, page=2, ordinal=4".to_string(),
            "[section] page 2: 1. Overview".to_string(),
            "The parser now emits structured search labels.".to_string(),
        ];

        assert_eq!(
            derive_match_locator(&lines, 3).as_deref(),
            Some("page 2 | page 2: 1. Overview")
        );
    }

    #[test]
    fn derive_locator_from_location_and_label_combines_structured_inputs() {
        let location = DocumentBlockLocation {
            source: Some("report.pdf".to_string()),
            page: Some(3),
            ordinal: Some(5),
            continued_from_previous_page: false,
            continued_to_next_page: false,
        };

        assert_eq!(
            derive_locator_from_location_and_label(Some(&location), Some("Results")).as_deref(),
            Some("page 3 | Results")
        );
    }
}
