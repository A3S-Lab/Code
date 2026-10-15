//! Value-typed options newly exposed by the release bindings.

use super::*;

#[pymethods]
impl PySessionOptions {
    #[getter]
    fn watch_skill_dirs(&self) -> Vec<String> {
        self.watch_skill_dirs.clone()
    }
    #[setter]
    fn set_watch_skill_dirs(&mut self, value: Vec<String>) {
        self.watch_skill_dirs = value;
    }
    #[getter]
    fn projected_skill_files(&self) -> Vec<String> {
        self.projected_skill_files.clone()
    }
    #[setter]
    fn set_projected_skill_files(&mut self, value: Vec<String>) {
        self.projected_skill_files = value;
    }
    #[getter]
    fn sandbox_network(&self) -> Option<String> {
        self.sandbox_network.clone()
    }
    #[setter]
    fn set_sandbox_network(&mut self, value: Option<String>) -> PyResult<()> {
        if let Some(mode) = value.as_deref() {
            a3s_code_core::sandbox::SandboxNetworkMode::parse(mode)
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        }
        self.sandbox_network = value;
        Ok(())
    }
    #[getter]
    fn sandbox_network_allow(&self) -> Vec<String> {
        self.sandbox_network_allow.clone()
    }
    #[setter]
    fn set_sandbox_network_allow(&mut self, value: Vec<String>) {
        self.sandbox_network_allow = value;
    }
    #[getter]
    fn code_intelligence(&self) -> bool {
        self.code_intelligence
    }
    #[setter]
    fn set_code_intelligence(&mut self, value: bool) {
        self.code_intelligence = value;
    }
    #[getter]
    fn path_injection_budget(&self) -> Option<usize> {
        self.path_injection_budget
    }
    #[setter]
    fn set_path_injection_budget(&mut self, value: Option<usize>) {
        self.path_injection_budget = value;
    }
    #[getter]
    fn verification_contracts(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.verification_contracts)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_string_to_py(py, &json)
    }
    #[setter]
    fn set_verification_contracts(&mut self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.verification_contracts = serde_json::from_str(&py_any_to_json(value)?)
            .map_err(|error| PyValueError::new_err(format!("verification_contracts: {error}")))?;
        Ok(())
    }
    #[getter]
    fn require_assistant_final_response(&self) -> Option<bool> {
        self.require_assistant_final_response
    }
    #[setter]
    fn set_require_assistant_final_response(&mut self, value: Option<bool>) {
        self.require_assistant_final_response = value;
    }
    /// ``{"id", "revision", "instructions"}`` or ``None``.
    #[getter]
    fn work_policy(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.work_policy)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        json_string_to_py(py, &json)
    }
    #[setter]
    fn set_work_policy(&mut self, value: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Policy {
            id: String,
            revision: u32,
            instructions: String,
        }
        self.work_policy = value
            .map(|value| {
                let policy: Policy = serde_json::from_str(&py_any_to_json(value)?)
                    .map_err(|error| PyValueError::new_err(format!("work_policy: {error}")))?;
                a3s_code_core::WorkPolicyV1::new(policy.id, policy.revision, policy.instructions)
                    .map_err(|error| PyValueError::new_err(format!("work_policy: {error}")))
            })
            .transpose()?;
        Ok(())
    }
}
