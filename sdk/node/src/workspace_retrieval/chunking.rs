use super::*;
use napi::bindgen_prelude::{ClassInstance, Either3};

type NodeChunkingRegistry =
    std::collections::HashMap<String, Weak<NodeWorkspaceChunkingConfiguration>>;

pub(super) struct NodeWorkspaceChunkingConfiguration {
    strategy: a3s_code_core::WorkspaceChunkingStrategy,
}

fn chunking_registry() -> &'static Mutex<NodeChunkingRegistry> {
    static REGISTRY: OnceLock<Mutex<NodeChunkingRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Explicit compatibility line chunking for a session-owned workspace catalog.
///
/// Omitting a chunking strategy also preserves this default.
#[napi]
#[derive(Default)]
pub struct LineWorkspaceChunkingStrategy {}

#[napi]
impl LineWorkspaceChunkingStrategy {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {}
    }

    /// Distinguishes this nominal strategy from primitive structural values.
    #[napi(getter)]
    pub fn uses_line_boundaries(&self) -> bool {
        true
    }
}

/// Fixed UTF-8-safe byte windows with bounded overlap.
#[napi]
pub struct FixedWindowWorkspaceChunkingStrategy {
    options: a3s_code_core::FixedWindowChunkingOptions,
}

#[napi]
impl FixedWindowWorkspaceChunkingStrategy {
    #[napi(
        constructor,
        ts_args_type = "targetBytes: number, overlapBytes?: number | null"
    )]
    pub fn new(target_bytes: f64, overlap_bytes: Option<f64>) -> napi::Result<Self> {
        let target_bytes = js_optional_usize(
            Some(target_bytes),
            "workspaceRetrieval.chunkingStrategy.targetBytes",
            0,
        )?;
        let overlap_bytes = js_optional_usize(
            overlap_bytes,
            "workspaceRetrieval.chunkingStrategy.overlapBytes",
            0,
        )?;
        let options = a3s_code_core::FixedWindowChunkingOptions::new(target_bytes, overlap_bytes)
            .map_err(chunking_error)?;
        validate_strategy(a3s_code_core::WorkspaceChunkingStrategy::FixedWindow(
            options,
        ))?;
        Ok(Self { options })
    }

    #[napi(getter)]
    pub fn target_bytes(&self) -> f64 {
        self.options.target_bytes as f64
    }

    #[napi(getter)]
    pub fn overlap_bytes(&self) -> f64 {
        self.options.overlap_bytes as f64
    }
}

/// Recursive separator-aware byte windows with bounded overlap.
#[napi]
pub struct RecursiveWorkspaceChunkingStrategy {
    options: a3s_code_core::RecursiveChunkingOptions,
}

#[napi]
impl RecursiveWorkspaceChunkingStrategy {
    #[napi(
        constructor,
        ts_args_type = "targetBytes: number, overlapBytes?: number | null, separators?: Array<string> | null"
    )]
    pub fn new(
        target_bytes: f64,
        overlap_bytes: Option<f64>,
        separators: Option<Vec<String>>,
    ) -> napi::Result<Self> {
        let target_bytes = js_optional_usize(
            Some(target_bytes),
            "workspaceRetrieval.chunkingStrategy.targetBytes",
            0,
        )?;
        let overlap_bytes = js_optional_usize(
            overlap_bytes,
            "workspaceRetrieval.chunkingStrategy.overlapBytes",
            0,
        )?;
        let mut options = a3s_code_core::RecursiveChunkingOptions::new(target_bytes, overlap_bytes)
            .map_err(chunking_error)?;
        if let Some(separators) = separators {
            options = options
                .with_separators(separators)
                .map_err(chunking_error)?;
        }
        validate_strategy(a3s_code_core::WorkspaceChunkingStrategy::Recursive(
            options.clone(),
        ))?;
        Ok(Self { options })
    }

    #[napi(getter)]
    pub fn target_bytes(&self) -> f64 {
        self.options.target_bytes as f64
    }

    #[napi(getter)]
    pub fn overlap_bytes(&self) -> f64 {
        self.options.overlap_bytes as f64
    }

    #[napi(getter)]
    pub fn separators(&self) -> Vec<String> {
        self.options
            .separators()
            .iter()
            .map(ToString::to_string)
            .collect()
    }
}

pub(super) type WorkspaceChunkingStrategyInput = Either3<
    ClassInstance<LineWorkspaceChunkingStrategy>,
    ClassInstance<FixedWindowWorkspaceChunkingStrategy>,
    ClassInstance<RecursiveWorkspaceChunkingStrategy>,
>;

pub(super) fn bind_workspace_chunking_strategy(
    input: &WorkspaceChunkingStrategyInput,
) -> napi::Result<(String, Arc<NodeWorkspaceChunkingConfiguration>)> {
    let strategy = match input {
        Either3::A(_) => a3s_code_core::WorkspaceChunkingStrategy::Lines,
        Either3::B(value) => a3s_code_core::WorkspaceChunkingStrategy::FixedWindow(value.options),
        Either3::C(value) => {
            a3s_code_core::WorkspaceChunkingStrategy::Recursive(value.options.clone())
        }
    };
    validate_strategy(strategy.clone())?;
    let configuration = Arc::new(NodeWorkspaceChunkingConfiguration { strategy });
    let instance_id = a3s_code_core::host_env::HostEnv::system().next_id();
    let mut registry = chunking_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    registry.retain(|_, configuration| configuration.strong_count() > 0);
    registry.insert(instance_id.clone(), Arc::downgrade(&configuration));
    drop(registry);
    Ok((instance_id, configuration))
}

pub(super) fn resolve_workspace_chunking_strategy(
    instance_id: &str,
) -> napi::Result<Option<a3s_code_core::WorkspaceChunkingStrategy>> {
    if instance_id.is_empty() {
        return Ok(None);
    }
    let weak = {
        let mut registry = chunking_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.retain(|_, configuration| configuration.strong_count() > 0);
        registry.get(instance_id).cloned()
    };
    weak.and_then(|configuration| configuration.upgrade())
        .map(|configuration| configuration.strategy.clone())
        .map(Some)
        .ok_or_else(|| {
            napi::Error::from_reason(
                "WorkspaceRetrievalOptions chunking strategy identity is invalid or expired; pass the original instance",
            )
        })
}

pub(super) fn unregister_workspace_chunking_strategy(
    instance_id: &str,
    configuration: &Arc<NodeWorkspaceChunkingConfiguration>,
) {
    if instance_id.is_empty() {
        return;
    }
    let own_configuration = Arc::downgrade(configuration);
    let mut registry = chunking_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if registry
        .get(instance_id)
        .is_some_and(|registered| registered.ptr_eq(&own_configuration))
    {
        registry.remove(instance_id);
    }
}

fn validate_strategy(
    strategy: a3s_code_core::WorkspaceChunkingStrategy,
) -> napi::Result<a3s_code_core::WorkspaceChunkingStrategy> {
    strategy
        .validate_for(a3s_code_core::ChunkingConfig::default())
        .map_err(chunking_error)?;
    Ok(strategy)
}

fn chunking_error(error: a3s_code_core::WorkspaceChunkingError) -> napi::Error {
    napi::Error::from_reason(format!("workspaceRetrieval.chunkingStrategy: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_core::{ChunkCatalogLimits, WorkspaceChunkCatalog, WorkspacePath};
    use serde::Deserialize;

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
            "../../../../core/tests/fixtures/workspace-chunking-sdk-v1.json"
        ))
        .expect("workspace chunking SDK fixture")
    }

    #[test]
    fn typed_objects_match_the_shared_core_ranges() {
        let fixture = fixture();
        assert_eq!(fixture.schema, "a3s.workspace-chunking-sdk.fixture.v1");
        for case in fixture.cases {
            let strategy = match case.name.as_str() {
                "line" => a3s_code_core::WorkspaceChunkingStrategy::Lines,
                "fixed_window" => a3s_code_core::WorkspaceChunkingStrategy::FixedWindow(
                    FixedWindowWorkspaceChunkingStrategy::new(
                        case.target_bytes.expect("fixed target") as f64,
                        case.overlap_bytes.map(|value| value as f64),
                    )
                    .expect("fixed strategy")
                    .options,
                ),
                "recursive" => a3s_code_core::WorkspaceChunkingStrategy::Recursive(
                    RecursiveWorkspaceChunkingStrategy::new(
                        case.target_bytes.expect("recursive target") as f64,
                        case.overlap_bytes.map(|value| value as f64),
                        case.separators,
                    )
                    .expect("recursive strategy")
                    .options,
                ),
                name => panic!("unknown fixture strategy {name}"),
            };
            let catalog = WorkspaceChunkCatalog::new_with_strategy(
                strategy,
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
    fn shared_invalid_windows_fail_before_binding() {
        for invalid in fixture().invalid_windows {
            assert!(
                FixedWindowWorkspaceChunkingStrategy::new(
                    invalid.target_bytes as f64,
                    Some(invalid.overlap_bytes as f64),
                )
                .is_err(),
                "{}",
                invalid.name
            );
        }
    }

    #[test]
    fn recursive_separator_validation_is_core_owned() {
        for separators in [
            Vec::<String>::new(),
            vec!["\n".to_owned(), "\n".to_owned()],
            vec!["\0".to_owned()],
        ] {
            assert!(
                RecursiveWorkspaceChunkingStrategy::new(64.0, Some(0.0), Some(separators),)
                    .is_err()
            );
        }
    }
}
