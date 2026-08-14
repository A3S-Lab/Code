use super::super::{
    ChunkCatalogLimits, ChunkingConfig, CustomWorkspaceChunkingStrategy,
    FixedWindowChunkingOptions, RecursiveChunkingOptions, WorkspaceChunkCatalog,
    WorkspaceChunkRange, WorkspaceChunkingError, WorkspaceChunkingInput, WorkspaceChunkingStrategy,
    WorkspaceIndexError,
};
use crate::workspace::WorkspacePath;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct SdkChunkingFixture {
    schema: String,
    cases: Vec<SdkChunkingCase>,
    invalid_windows: Vec<SdkInvalidWindow>,
}

#[derive(Deserialize)]
struct SdkChunkingCase {
    name: String,
    content: String,
    target_bytes: Option<usize>,
    overlap_bytes: Option<usize>,
    separators: Option<Vec<String>>,
    ranges: Vec<SdkChunkRange>,
}

#[derive(Deserialize)]
struct SdkChunkRange {
    start: usize,
    end: usize,
}

#[derive(Deserialize)]
struct SdkInvalidWindow {
    name: String,
    target_bytes: usize,
    overlap_bytes: usize,
}

fn sdk_chunking_fixture() -> SdkChunkingFixture {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/workspace-chunking-sdk-v1.json"
    ))
    .expect("workspace chunking SDK fixture")
}

fn config(max_bytes: usize) -> ChunkingConfig {
    ChunkingConfig {
        max_lines: 80,
        max_bytes,
        max_chunks_per_file: 32,
    }
}

#[test]
fn shared_sdk_fixture_locks_core_ranges_and_invalid_windows() {
    let fixture = sdk_chunking_fixture();
    assert_eq!(fixture.schema, "a3s.workspace-chunking-sdk.fixture.v1");

    for case in fixture.cases {
        let strategy = match case.name.as_str() {
            "line" => WorkspaceChunkingStrategy::Lines,
            "fixed_window" => WorkspaceChunkingStrategy::FixedWindow(
                FixedWindowChunkingOptions::new(
                    case.target_bytes.expect("fixed target"),
                    case.overlap_bytes.expect("fixed overlap"),
                )
                .expect("fixed strategy"),
            ),
            "recursive" => WorkspaceChunkingStrategy::Recursive(
                RecursiveChunkingOptions::new(
                    case.target_bytes.expect("recursive target"),
                    case.overlap_bytes.expect("recursive overlap"),
                )
                .expect("recursive strategy")
                .with_separators(case.separators.expect("recursive separators"))
                .expect("recursive separators"),
            ),
            name => panic!("unknown fixture strategy {name}"),
        };
        let catalog = WorkspaceChunkCatalog::new_with_strategy(
            strategy,
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
        )
        .expect("fixture catalog");
        let path = WorkspacePath::from_normalized("fixture.txt");
        let snapshot = catalog
            .replace_file(&path, None, 1, &case.content)
            .expect("fixture chunks");
        let actual = snapshot
            .chunks()
            .iter()
            .map(|chunk| (chunk.start_byte, chunk.end_byte))
            .collect::<Vec<_>>();
        let expected = case
            .ranges
            .iter()
            .map(|range| (range.start, range.end))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{}", case.name);
    }

    for invalid in fixture.invalid_windows {
        let rejected = FixedWindowChunkingOptions::new(invalid.target_bytes, invalid.overlap_bytes)
            .map(WorkspaceChunkingStrategy::FixedWindow)
            .and_then(|strategy| strategy.validate_for(ChunkingConfig::default()))
            .is_err();
        assert!(rejected, "{}", invalid.name);
    }
}

#[test]
fn fixed_window_strategy_is_utf8_safe_and_applies_bounded_overlap() {
    let strategy =
        WorkspaceChunkingStrategy::FixedWindow(FixedWindowChunkingOptions::new(4, 1).unwrap());
    let catalog = WorkspaceChunkCatalog::new_with_strategy(
        strategy,
        config(8),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("notes.txt"),
            None,
            1,
            "abcdefghij",
        )
        .unwrap();

    let chunks = catalog.snapshot().unwrap().chunks().to_vec();
    let ranges = chunks
        .iter()
        .map(|chunk| (chunk.start_byte, chunk.end_byte, chunk.text.as_ref()))
        .collect::<Vec<_>>();
    assert_eq!(
        ranges,
        vec![(0, 4, "abcd"), (3, 7, "defg"), (6, 10, "ghij")]
    );
    assert_eq!(catalog.snapshot().unwrap().text_bytes(), 12);

    let unicode = WorkspaceChunkCatalog::new_with_strategy(
        WorkspaceChunkingStrategy::FixedWindow(FixedWindowChunkingOptions::new(5, 1).unwrap()),
        config(8),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    unicode
        .replace_file(
            &WorkspacePath::from_normalized("unicode.txt"),
            None,
            1,
            "ab工作区cd",
        )
        .unwrap();
    for chunk in unicode.snapshot().unwrap().chunks().iter() {
        assert!("ab工作区cd".is_char_boundary(chunk.start_byte));
        assert!("ab工作区cd".is_char_boundary(chunk.end_byte));
        assert!(chunk.text.len() <= 5);
    }

    let multiline = WorkspaceChunkCatalog::new_with_strategy(
        WorkspaceChunkingStrategy::FixedWindow(FixedWindowChunkingOptions::new(5, 2).unwrap()),
        config(8),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    multiline
        .replace_file(
            &WorkspacePath::from_normalized("lines.txt"),
            None,
            1,
            "aa\nbb\ncc\n",
        )
        .unwrap();
    let line_ranges = multiline
        .snapshot()
        .unwrap()
        .chunks()
        .iter()
        .map(|chunk| (chunk.start_line, chunk.end_line))
        .collect::<Vec<_>>();
    assert_eq!(line_ranges, vec![(1, 2), (2, 3), (3, 3)]);
}

#[test]
fn recursive_strategy_prefers_structural_separators_and_allows_custom_order() {
    let strategy = WorkspaceChunkingStrategy::Recursive(
        RecursiveChunkingOptions::new(20, 0)
            .unwrap()
            .with_separators(["\n\n", "\n", " "])
            .unwrap(),
    );
    let catalog = WorkspaceChunkCatalog::new_with_strategy(
        strategy,
        config(32),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("guide.md"),
            Some("markdown"),
            1,
            "alpha beta\n\nsecond paragraph\n\nthird",
        )
        .unwrap();

    let texts = catalog
        .snapshot()
        .unwrap()
        .chunks()
        .iter()
        .map(|chunk| chunk.text.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        vec!["alpha beta\n\n", "second paragraph\n\n", "third"]
    );
}

struct ValidCustomStrategy;

impl CustomWorkspaceChunkingStrategy for ValidCustomStrategy {
    fn split(
        &self,
        input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        let split = input.content.find("--").unwrap() + 2;
        Ok(vec![
            WorkspaceChunkRange::new(0, split),
            WorkspaceChunkRange::new(split, input.content.len()),
        ])
    }
}

#[test]
fn host_custom_strategy_can_supply_ranges_but_code_owns_chunk_identity_and_lines() {
    let catalog = WorkspaceChunkCatalog::new_with_strategy(
        WorkspaceChunkingStrategy::custom(Arc::new(ValidCustomStrategy)),
        config(64),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("custom.txt"),
            None,
            9,
            "first\n--second\n",
        )
        .unwrap();

    let chunks = catalog.snapshot().unwrap().chunks().to_vec();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].text.as_ref(), "first\n--");
    assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 2));
    assert_eq!((chunks[1].start_line, chunks[1].end_line), (2, 2));
    assert!(chunks
        .iter()
        .all(|chunk| chunk.id.as_str().starts_with("sha256:")));
}

struct GapStrategy;

impl CustomWorkspaceChunkingStrategy for GapStrategy {
    fn split(
        &self,
        input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        Ok(vec![
            WorkspaceChunkRange::new(0, 2),
            WorkspaceChunkRange::new(3, input.content.len()),
        ])
    }
}

struct InvalidUtf8Strategy;

impl CustomWorkspaceChunkingStrategy for InvalidUtf8Strategy {
    fn split(
        &self,
        input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        Ok(vec![
            WorkspaceChunkRange::new(0, 1),
            WorkspaceChunkRange::new(1, input.content.len()),
        ])
    }
}

struct FailedStrategy;

impl CustomWorkspaceChunkingStrategy for FailedStrategy {
    fn split(
        &self,
        _input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        Err(WorkspaceChunkingError::StrategyFailed)
    }
}

struct PanickingStrategy;

impl CustomWorkspaceChunkingStrategy for PanickingStrategy {
    fn split(
        &self,
        _input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        panic!("host chunker panic must not escape")
    }
}

#[test]
fn hostile_custom_ranges_cannot_create_gaps_or_break_utf8_boundaries() {
    let gap = WorkspaceChunkCatalog::new_with_strategy(
        WorkspaceChunkingStrategy::custom(Arc::new(GapStrategy)),
        config(64),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    let error = gap
        .replace_file(
            &WorkspacePath::from_normalized("gap.txt"),
            None,
            1,
            "abcdef",
        )
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceIndexError::InvalidChunkRanges { .. }
    ));

    let invalid_utf8 = WorkspaceChunkCatalog::new_with_strategy(
        WorkspaceChunkingStrategy::custom(Arc::new(InvalidUtf8Strategy)),
        config(64),
        ChunkCatalogLimits::default(),
    )
    .unwrap();
    let error = invalid_utf8
        .replace_file(
            &WorkspacePath::from_normalized("utf8.txt"),
            None,
            1,
            "工作区",
        )
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceIndexError::InvalidChunkRanges { .. }
    ));
}

#[test]
fn custom_strategy_failures_and_panics_are_redacted() {
    for strategy in [
        WorkspaceChunkingStrategy::custom(Arc::new(FailedStrategy)),
        WorkspaceChunkingStrategy::custom(Arc::new(PanickingStrategy)),
    ] {
        let catalog = WorkspaceChunkCatalog::new_with_strategy(
            strategy,
            config(64),
            ChunkCatalogLimits::default(),
        )
        .unwrap();
        let error = catalog
            .replace_file(
                &WorkspacePath::from_normalized("failure.txt"),
                None,
                1,
                "sensitive source text",
            )
            .unwrap_err();
        assert!(matches!(
            &error,
            WorkspaceIndexError::ChunkingStrategyFailed { path }
                if path == "failure.txt"
        ));
        assert!(!error.to_string().contains("sensitive source text"));
    }
}

#[test]
fn built_in_options_are_bounded_by_catalog_chunk_limits() {
    let strategy =
        WorkspaceChunkingStrategy::FixedWindow(FixedWindowChunkingOptions::new(65, 8).unwrap());
    let error = WorkspaceChunkCatalog::new_with_strategy(
        strategy,
        config(64),
        ChunkCatalogLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(error, WorkspaceIndexError::InvalidConfig(_)));

    assert!(RecursiveChunkingOptions::new(64, 64).is_err());
    assert!(RecursiveChunkingOptions::new(64, 8)
        .unwrap()
        .with_separators(["", "\n"])
        .is_err());

    let bypassed_constructor = WorkspaceChunkingStrategy::FixedWindow(FixedWindowChunkingOptions {
        target_bytes: 16,
        overlap_bytes: 16,
    });
    assert!(WorkspaceChunkCatalog::new_with_strategy(
        bypassed_constructor,
        config(64),
        ChunkCatalogLimits::default(),
    )
    .is_err());
}

#[test]
fn public_chunking_strategy_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<WorkspaceChunkRange>();
    assert_send_sync::<WorkspaceChunkingInput<'static>>();
    assert_send_sync::<WorkspaceChunkingStrategy>();
    assert_send_sync::<FixedWindowChunkingOptions>();
    assert_send_sync::<RecursiveChunkingOptions>();
}
