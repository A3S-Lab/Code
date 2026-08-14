use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BridgeWorkspaceChunkingStrategy {
    line: Option<BridgeLineWorkspaceChunkingStrategy>,
    fixed_window: Option<BridgeFixedWindowWorkspaceChunkingStrategy>,
    recursive: Option<BridgeRecursiveWorkspaceChunkingStrategy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeLineWorkspaceChunkingStrategy {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeFixedWindowWorkspaceChunkingStrategy {
    target_bytes: usize,
    overlap_bytes: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeRecursiveWorkspaceChunkingStrategy {
    target_bytes: usize,
    overlap_bytes: usize,
    separators: Option<Vec<String>>,
}

impl BridgeWorkspaceChunkingStrategy {
    pub(super) fn into_core(
        self,
    ) -> Result<a3s_code_core::WorkspaceChunkingStrategy, BridgeFailure> {
        let selected = usize::from(self.line.is_some())
            + usize::from(self.fixed_window.is_some())
            + usize::from(self.recursive.is_some());
        if selected != 1 {
            return Err(invalid_retrieval(
                "chunking_strategy must contain exactly one typed strategy block",
            ));
        }
        let strategy = if self.line.is_some() {
            a3s_code_core::WorkspaceChunkingStrategy::Lines
        } else if let Some(options) = self.fixed_window {
            a3s_code_core::WorkspaceChunkingStrategy::FixedWindow(
                a3s_code_core::FixedWindowChunkingOptions::new(
                    options.target_bytes,
                    options.overlap_bytes,
                )
                .map_err(chunking_error)?,
            )
        } else if let Some(options) = self.recursive {
            let mut recursive = a3s_code_core::RecursiveChunkingOptions::new(
                options.target_bytes,
                options.overlap_bytes,
            )
            .map_err(chunking_error)?;
            if let Some(separators) = options.separators {
                recursive = recursive
                    .with_separators(separators)
                    .map_err(chunking_error)?;
            }
            a3s_code_core::WorkspaceChunkingStrategy::Recursive(recursive)
        } else {
            unreachable!("exactly one strategy was checked")
        };
        strategy
            .validate_for(a3s_code_core::ChunkingConfig::default())
            .map_err(chunking_error)?;
        Ok(strategy)
    }
}

fn chunking_error(error: a3s_code_core::WorkspaceChunkingError) -> BridgeFailure {
    invalid_retrieval(format!("chunking_strategy: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_core::{ChunkCatalogLimits, WorkspaceChunkCatalog, WorkspacePath};

    #[derive(Deserialize)]
    struct Fixture {
        schema: String,
        cases: Vec<FixtureCase>,
        invalid_windows: Vec<InvalidWindow>,
    }

    #[derive(Deserialize)]
    struct FixtureCase {
        name: String,
        content: String,
        target_bytes: Option<usize>,
        overlap_bytes: Option<usize>,
        separators: Option<Vec<String>>,
        ranges: Vec<FixtureRange>,
    }

    #[derive(Deserialize)]
    struct FixtureRange {
        start: usize,
        end: usize,
    }

    #[derive(Deserialize)]
    struct InvalidWindow {
        name: String,
        target_bytes: usize,
        overlap_bytes: usize,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../../../../core/tests/fixtures/workspace-chunking-sdk-v1.json"
        ))
        .expect("workspace chunking SDK fixture")
    }

    #[test]
    fn typed_wire_matches_the_shared_core_ranges() {
        let fixture = fixture();
        assert_eq!(fixture.schema, "a3s.workspace-chunking-sdk.fixture.v1");
        for case in fixture.cases {
            let bridge = match case.name.as_str() {
                "line" => BridgeWorkspaceChunkingStrategy {
                    line: Some(BridgeLineWorkspaceChunkingStrategy {}),
                    ..BridgeWorkspaceChunkingStrategy::default()
                },
                "fixed_window" => BridgeWorkspaceChunkingStrategy {
                    fixed_window: Some(BridgeFixedWindowWorkspaceChunkingStrategy {
                        target_bytes: case.target_bytes.expect("fixed target"),
                        overlap_bytes: case.overlap_bytes.expect("fixed overlap"),
                    }),
                    ..BridgeWorkspaceChunkingStrategy::default()
                },
                "recursive" => BridgeWorkspaceChunkingStrategy {
                    recursive: Some(BridgeRecursiveWorkspaceChunkingStrategy {
                        target_bytes: case.target_bytes.expect("recursive target"),
                        overlap_bytes: case.overlap_bytes.expect("recursive overlap"),
                        separators: case.separators,
                    }),
                    ..BridgeWorkspaceChunkingStrategy::default()
                },
                name => panic!("unknown fixture strategy {name}"),
            };
            let catalog = WorkspaceChunkCatalog::new_with_strategy(
                bridge.into_core().expect("bridge strategy"),
                a3s_code_core::ChunkingConfig::default(),
                ChunkCatalogLimits::default(),
            )
            .expect("fixture catalog");
            let snapshot = catalog
                .replace_file(
                    &WorkspacePath::from_normalized("fixture.txt"),
                    None,
                    1,
                    &case.content,
                )
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
    }

    #[test]
    fn shared_invalid_windows_fail_bridge_conversion() {
        for invalid in fixture().invalid_windows {
            let bridge = BridgeWorkspaceChunkingStrategy {
                fixed_window: Some(BridgeFixedWindowWorkspaceChunkingStrategy {
                    target_bytes: invalid.target_bytes,
                    overlap_bytes: invalid.overlap_bytes,
                }),
                ..BridgeWorkspaceChunkingStrategy::default()
            };
            assert!(bridge.into_core().is_err(), "{}", invalid.name);
        }
    }

    #[test]
    fn primitive_and_ambiguous_selectors_are_rejected() {
        for field in ["kind", "strategy", "mode"] {
            let mut object = serde_json::Map::new();
            object.insert(
                field.to_owned(),
                serde_json::Value::String("recursive".to_owned()),
            );
            assert!(serde_json::from_value::<BridgeWorkspaceChunkingStrategy>(
                serde_json::Value::Object(object)
            )
            .is_err());
        }
        assert!(BridgeWorkspaceChunkingStrategy::default()
            .into_core()
            .is_err());
        assert!(BridgeWorkspaceChunkingStrategy {
            line: Some(BridgeLineWorkspaceChunkingStrategy {}),
            fixed_window: Some(BridgeFixedWindowWorkspaceChunkingStrategy {
                target_bytes: 64,
                overlap_bytes: 0,
            }),
            recursive: None,
        }
        .into_core()
        .is_err());
    }
}
