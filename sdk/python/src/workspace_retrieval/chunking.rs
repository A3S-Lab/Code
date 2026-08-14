use super::*;

/// Explicit compatibility line chunking for a session-owned workspace catalog.
#[pyclass(name = "LineWorkspaceChunkingStrategy")]
#[derive(Clone, Default)]
pub(crate) struct PyLineWorkspaceChunkingStrategy {}

#[pymethods]
impl PyLineWorkspaceChunkingStrategy {
    #[new]
    fn new() -> Self {
        Self {}
    }

    fn __repr__(&self) -> &'static str {
        "LineWorkspaceChunkingStrategy()"
    }
}

/// Fixed UTF-8-safe byte windows with bounded overlap.
#[pyclass(name = "FixedWindowWorkspaceChunkingStrategy")]
#[derive(Clone)]
pub(crate) struct PyFixedWindowWorkspaceChunkingStrategy {
    options: a3s_code_core::FixedWindowChunkingOptions,
}

#[pymethods]
impl PyFixedWindowWorkspaceChunkingStrategy {
    #[new]
    #[pyo3(signature = (target_bytes, overlap_bytes=0))]
    fn new(target_bytes: usize, overlap_bytes: usize) -> PyResult<Self> {
        let options = a3s_code_core::FixedWindowChunkingOptions::new(target_bytes, overlap_bytes)
            .map_err(chunking_error)?;
        validate_strategy(a3s_code_core::WorkspaceChunkingStrategy::FixedWindow(
            options,
        ))?;
        Ok(Self { options })
    }

    #[getter]
    fn target_bytes(&self) -> usize {
        self.options.target_bytes
    }

    #[getter]
    fn overlap_bytes(&self) -> usize {
        self.options.overlap_bytes
    }

    fn __repr__(&self) -> String {
        format!(
            "FixedWindowWorkspaceChunkingStrategy(target_bytes={}, overlap_bytes={})",
            self.options.target_bytes, self.options.overlap_bytes
        )
    }
}

/// Recursive separator-aware byte windows with bounded overlap.
#[pyclass(name = "RecursiveWorkspaceChunkingStrategy")]
#[derive(Clone)]
pub(crate) struct PyRecursiveWorkspaceChunkingStrategy {
    options: a3s_code_core::RecursiveChunkingOptions,
}

#[pymethods]
impl PyRecursiveWorkspaceChunkingStrategy {
    #[new]
    #[pyo3(signature = (target_bytes, overlap_bytes=0, separators=None))]
    fn new(
        target_bytes: usize,
        overlap_bytes: usize,
        separators: Option<Vec<String>>,
    ) -> PyResult<Self> {
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

    #[getter]
    fn target_bytes(&self) -> usize {
        self.options.target_bytes
    }

    #[getter]
    fn overlap_bytes(&self) -> usize {
        self.options.overlap_bytes
    }

    #[getter]
    fn separators(&self) -> Vec<String> {
        self.options
            .separators()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "RecursiveWorkspaceChunkingStrategy(target_bytes={}, overlap_bytes={}, separators={:?})",
            self.options.target_bytes,
            self.options.overlap_bytes,
            self.separators(),
        )
    }
}

pub(super) fn python_chunking_strategy_to_core(
    py: Python<'_>,
    value: Option<PyObject>,
) -> PyResult<Option<a3s_code_core::WorkspaceChunkingStrategy>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.bind(py);
    let strategy = if value
        .extract::<PyRef<'_, PyLineWorkspaceChunkingStrategy>>()
        .is_ok()
    {
        a3s_code_core::WorkspaceChunkingStrategy::Lines
    } else if let Ok(options) = value.extract::<PyRef<'_, PyFixedWindowWorkspaceChunkingStrategy>>()
    {
        a3s_code_core::WorkspaceChunkingStrategy::FixedWindow(options.options)
    } else if let Ok(options) = value.extract::<PyRef<'_, PyRecursiveWorkspaceChunkingStrategy>>() {
        a3s_code_core::WorkspaceChunkingStrategy::Recursive(options.options.clone())
    } else {
        return Err(PyTypeError::new_err(
            "chunking_strategy must be a LineWorkspaceChunkingStrategy, FixedWindowWorkspaceChunkingStrategy, or RecursiveWorkspaceChunkingStrategy",
        ));
    };
    validate_strategy(strategy.clone())?;
    Ok(Some(strategy))
}

fn validate_strategy(
    strategy: a3s_code_core::WorkspaceChunkingStrategy,
) -> PyResult<a3s_code_core::WorkspaceChunkingStrategy> {
    strategy
        .validate_for(a3s_code_core::ChunkingConfig::default())
        .map_err(chunking_error)?;
    Ok(strategy)
}

fn chunking_error(error: a3s_code_core::WorkspaceChunkingError) -> PyErr {
    PyValueError::new_err(format!("workspace_retrieval.chunking_strategy: {error}"))
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
                    PyFixedWindowWorkspaceChunkingStrategy::new(
                        case.target_bytes.expect("fixed target"),
                        case.overlap_bytes.expect("fixed overlap"),
                    )
                    .expect("fixed strategy")
                    .options,
                ),
                "recursive" => a3s_code_core::WorkspaceChunkingStrategy::Recursive(
                    PyRecursiveWorkspaceChunkingStrategy::new(
                        case.target_bytes.expect("recursive target"),
                        case.overlap_bytes.expect("recursive overlap"),
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
    fn shared_invalid_windows_fail_before_session_conversion() {
        for invalid in fixture().invalid_windows {
            assert!(
                PyFixedWindowWorkspaceChunkingStrategy::new(
                    invalid.target_bytes,
                    invalid.overlap_bytes,
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
            assert!(PyRecursiveWorkspaceChunkingStrategy::new(64, 0, Some(separators),).is_err());
        }
    }
}
