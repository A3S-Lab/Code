//! Host-only ACL declaration. Parsing does not qualify or admit execution.

use a3s_acl::{Block, Token, Value};
use a3s_code_core::content_digest::{digest_bytes, validate_digest};
use a3s_code_core::evaluation::EvidenceLimitsV1;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const MANIFEST_SCHEMA: &str = "a3s.code.behavior-acceptance-manifest.v1";
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_FILES: u64 = 512;
pub const MAX_PATH_BYTES: u64 = 4096;
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub const CASE_COUNT: usize = 16;
const MAX_EXACT_ACL_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("acceptance manifest refuses {0}")]
    Manifest(String),
    #[error("{set} refuses token `{token}`")]
    ClosedSet { set: &'static str, token: String },
    #[error("acceptance digest is not canonical sha256")]
    Digest,
    #[error("budget_not_admitted: {0}")]
    BudgetNotAdmitted(&'static str),
    #[error("acceptance report refuses {0}")]
    Report(String),
    #[error("acceptance manifest I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Canonical digest data, validated on both construction and deserialization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DigestV1(String);

impl DigestV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        validate_digest(&value).map_err(|_| ContractError::Digest)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for DigestV1 {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<DigestV1> for String {
    fn from(value: DigestV1) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AcceptanceManifestV1 {
    pub schema: String,
    pub manifest_version: u64,
    pub manifest_digest: DigestV1,
    pub task_id: String,
    pub cluster_id: String,
    pub variant: String,
    pub public_prompt_digest: DigestV1,
    pub base_byte_digest: DigestV1,
    pub case_ids: Vec<String>,
    pub candidate: CandidateDeclarationV1,
    pub oracle: OracleDeclarationV1,
    pub limits: AcceptanceLimitsV1,
    pub environment: EnvironmentDeclarationV1,
    pub live: Option<LiveDeclarationV1>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateDeclarationV1 {
    pub allowed_paths: Vec<String>,
    pub entrypoint: String,
    pub max_files: u64,
    pub max_path_bytes: u64,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OracleCaseV1 {
    pub case_id: String,
    pub input_digest: DigestV1,
    pub expected_output_digest: DigestV1,
}

#[derive(Debug, Clone, Serialize)]
pub struct OracleDeclarationV1 {
    pub material_root: PathBuf,
    pub verifier_digest: DigestV1,
    pub golden_digest: DigestV1,
    pub bug_digest: DigestV1,
    pub mutant_digest: DigestV1,
    pub case_count: u64,
    pub cases: Vec<OracleCaseV1>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcceptanceLimitsV1 {
    pub case_count: u64,
    pub per_case_deadline_ms: u64,
    pub aggregate_deadline_ms: u64,
    pub request_bytes: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub aggregate_output_bytes: u64,
    pub host_artifact_bytes: u64,
    pub receipt_capture_bytes: u64,
    pub payload_bytes: u64,
    pub evidence: EvidenceLimitsV1,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentDeclarationV1 {
    pub os: String,
    pub arch: String,
    pub sandbox_version: String,
    pub sandbox_revision: String,
    pub backend: String,
    pub environment_digest: DigestV1,
    pub backend_digest: DigestV1,
    pub policy_digest: DigestV1,
    pub capability_digest: DigestV1,
    pub qualification_digest: DigestV1,
    pub interpreter: PathBuf,
    pub interpreter_digest: DigestV1,
    pub interpreter_version: String,
    pub environment_allowlist: Vec<String>,
    pub maker_root: PathBuf,
    pub candidate_root: PathBuf,
    pub scratch_root: PathBuf,
    pub evidence_root: PathBuf,
    pub result_root: PathBuf,
    pub other_attempt_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveDeclarationV1 {
    pub provider_model_route: String,
    pub provider_revision: Option<String>,
    pub model_revision: Option<String>,
    pub retry_policy_digest: DigestV1,
    pub fallback_policy_digest: DigestV1,
    pub admission_budget_digest: DigestV1,
    pub token_bound: u64,
    pub money_bound_microusd: u64,
    pub first_attempt_id: String,
    pub remediation_attempt_id: String,
    pub usage_uncertainty_policy: super::report::UsageUncertaintyPolicyV1,
}

impl AcceptanceManifestV1 {
    /// One explicitly supplied host file. No discovery, overlays or env calls.
    pub fn load_host(path: &Path) -> Result<Self, ContractError> {
        if path.extension().and_then(|value| value.to_str()) != Some("acl") {
            return Err(invalid("the manifest must have the .acl extension"));
        }
        let bytes = read_manifest_bytes(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid("non-UTF-8 ACL"))?;
        Self::parse_host(text)
    }

    pub fn parse_host(source: &str) -> Result<Self, ContractError> {
        if source.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("manifest exceeds 1 MiB"));
        }
        reject_lossy_tokens(source)?;
        let document = a3s_acl::parse_acl(source).map_err(|error| invalid(error.to_string()))?;
        let mut sections = std::collections::BTreeMap::new();
        for block in &document.blocks {
            if ![
                "acceptance",
                "candidate",
                "oracle",
                "limits",
                "environment",
                "live",
            ]
            .contains(&block.name.as_str())
                || !block.labels.is_empty()
                || sections.insert(block.name.as_str(), block).is_some()
            {
                return Err(invalid(format!(
                    "unknown, labeled or duplicate block {}",
                    block.name
                )));
            }
        }
        let required = |name| {
            sections
                .get(name)
                .copied()
                .ok_or_else(|| invalid(format!("missing {name} block")))
        };
        let acceptance = required("acceptance")?;
        fields(
            acceptance,
            &[
                "schema",
                "manifest_version",
                "task_id",
                "cluster_id",
                "variant",
                "public_prompt_digest",
                "base_byte_digest",
                "case_ids",
            ],
            false,
        )?;
        if string(acceptance, "schema")? != MANIFEST_SCHEMA
            || integer(acceptance, "manifest_version", 1)? != 1
        {
            return Err(invalid("unsupported manifest schema/version"));
        }
        let case_ids = strings(acceptance, "case_ids", true)?;
        if case_ids.len() != CASE_COUNT {
            return Err(invalid(
                "case_ids must contain exactly 16 unique ordered IDs",
            ));
        }
        let candidate = parse_candidate(required("candidate")?)?;
        let oracle = parse_oracle(required("oracle")?, &case_ids)?;
        let limits = parse_limits(required("limits")?)?;
        let environment = parse_environment(required("environment")?)?;
        let mut roots = vec![
            &oracle.material_root,
            &environment.maker_root,
            &environment.candidate_root,
            &environment.scratch_root,
            &environment.evidence_root,
            &environment.result_root,
        ];
        roots.extend(environment.other_attempt_roots.iter());
        for (index, root) in roots.iter().enumerate() {
            if roots[..index]
                .iter()
                .any(|other| root.starts_with(other) || other.starts_with(root))
            {
                return Err(invalid("separate host roots must not overlap"));
            }
        }
        let live = sections
            .get("live")
            .map(|block| parse_live(block))
            .transpose()?;
        Ok(Self {
            schema: MANIFEST_SCHEMA.into(),
            manifest_version: 1,
            manifest_digest: DigestV1::new(digest_bytes(
                "a3s.code.behavior-acceptance-manifest.bytes.v1",
                source.as_bytes(),
            ))?,
            task_id: string(acceptance, "task_id")?,
            cluster_id: string(acceptance, "cluster_id")?,
            variant: string(acceptance, "variant")?,
            public_prompt_digest: digest(acceptance, "public_prompt_digest")?,
            base_byte_digest: digest(acceptance, "base_byte_digest")?,
            case_ids,
            candidate,
            oracle,
            limits,
            environment,
            live,
        })
    }
}

fn parse_candidate(block: &Block) -> Result<CandidateDeclarationV1, ContractError> {
    fields(
        block,
        &[
            "allowed_paths",
            "entrypoint",
            "max_files",
            "max_path_bytes",
            "max_file_bytes",
            "max_total_bytes",
        ],
        false,
    )?;
    let allowed_paths = strings(block, "allowed_paths", true)?;
    let result = CandidateDeclarationV1 {
        allowed_paths,
        entrypoint: string(block, "entrypoint")?,
        max_files: integer(block, "max_files", MAX_FILES)?,
        max_path_bytes: integer(block, "max_path_bytes", MAX_PATH_BYTES)?,
        max_file_bytes: integer(block, "max_file_bytes", MAX_FILE_BYTES)?,
        max_total_bytes: integer(block, "max_total_bytes", MAX_TOTAL_BYTES)?,
    };
    if result.allowed_paths.len() as u64 > result.max_files
        || !result.allowed_paths.contains(&result.entrypoint)
        || result.max_file_bytes > result.max_total_bytes
    {
        return Err(invalid(
            "candidate allowlist, entrypoint or byte limits disagree",
        ));
    }
    for path in &result.allowed_paths {
        validate_relative_path(path, result.max_path_bytes)?;
    }
    Ok(result)
}

fn parse_oracle(block: &Block, case_ids: &[String]) -> Result<OracleDeclarationV1, ContractError> {
    fields(
        block,
        &[
            "material_root",
            "verifier_digest",
            "golden_digest",
            "bug_digest",
            "mutant_digest",
            "case_count",
        ],
        true,
    )?;
    let mut cases = Vec::new();
    for case in &block.blocks {
        if case.name != "case" || case.labels.len() != 1 {
            return Err(invalid("oracle permits only singly labeled case blocks"));
        }
        fields(case, &["input_digest", "expected_output_digest"], false)?;
        validate_id(&case.labels[0])?;
        cases.push(OracleCaseV1 {
            case_id: case.labels[0].clone(),
            input_digest: digest(case, "input_digest")?,
            expected_output_digest: digest(case, "expected_output_digest")?,
        });
    }
    if integer(block, "case_count", CASE_COUNT as u64)? != CASE_COUNT as u64
        || cases.iter().map(|case| &case.case_id).ne(case_ids.iter())
    {
        return Err(invalid("oracle cases must exactly match ordered case_ids"));
    }
    Ok(OracleDeclarationV1 {
        material_root: absolute_path(block, "material_root")?,
        verifier_digest: digest(block, "verifier_digest")?,
        golden_digest: digest(block, "golden_digest")?,
        bug_digest: digest(block, "bug_digest")?,
        mutant_digest: digest(block, "mutant_digest")?,
        case_count: CASE_COUNT as u64,
        cases,
    })
}

fn parse_limits(block: &Block) -> Result<AcceptanceLimitsV1, ContractError> {
    fields(
        block,
        &[
            "case_count",
            "per_case_deadline_ms",
            "aggregate_deadline_ms",
            "request_bytes",
            "stdout_bytes",
            "stderr_bytes",
            "aggregate_output_bytes",
            "host_artifact_bytes",
            "receipt_capture_bytes",
            "payload_bytes",
            "evidence_max_events",
            "evidence_max_event_bytes",
            "evidence_max_artifacts",
            "evidence_max_artifact_bytes",
            "evidence_max_prompt_bytes",
            "evidence_max_result_bytes",
        ],
        false,
    )?;
    use a3s_code_core::evaluation::*;
    let evidence = EvidenceLimitsV1 {
        max_events: integer(block, "evidence_max_events", EVIDENCE_MAX_EVENTS as u64)? as usize,
        max_event_bytes: integer(
            block,
            "evidence_max_event_bytes",
            EVIDENCE_MAX_EVENT_BYTES as u64,
        )? as usize,
        max_artifacts: integer(
            block,
            "evidence_max_artifacts",
            EVIDENCE_MAX_ARTIFACTS as u64,
        )? as usize,
        max_artifact_bytes: integer(
            block,
            "evidence_max_artifact_bytes",
            EVIDENCE_MAX_ARTIFACT_BYTES as u64,
        )? as usize,
        max_prompt_bytes: integer(
            block,
            "evidence_max_prompt_bytes",
            EVIDENCE_MAX_PROMPT_BYTES as u64,
        )? as usize,
        max_result_bytes: integer(
            block,
            "evidence_max_result_bytes",
            EVIDENCE_MAX_RESULT_BYTES as u64,
        )? as usize,
    };
    evidence
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let result = AcceptanceLimitsV1 {
        case_count: integer(block, "case_count", CASE_COUNT as u64)?,
        per_case_deadline_ms: integer(block, "per_case_deadline_ms", 2_000)?,
        aggregate_deadline_ms: integer(block, "aggregate_deadline_ms", 120_000)?,
        request_bytes: integer(block, "request_bytes", 16 * 1024)?,
        stdout_bytes: integer(block, "stdout_bytes", 64 * 1024)?,
        stderr_bytes: integer(block, "stderr_bytes", 64 * 1024)?,
        aggregate_output_bytes: integer(block, "aggregate_output_bytes", MAX_EXACT_ACL_INTEGER)?,
        host_artifact_bytes: integer(block, "host_artifact_bytes", MAX_EXACT_ACL_INTEGER)?,
        receipt_capture_bytes: integer(block, "receipt_capture_bytes", MAX_FILE_BYTES)?,
        payload_bytes: integer(block, "payload_bytes", 256 * 1024)?,
        evidence,
    };
    if result.case_count != CASE_COUNT as u64
        || result.aggregate_deadline_ms < result.per_case_deadline_ms
    {
        return Err(invalid(
            "limits require exactly 16 cases and a coherent deadline",
        ));
    }
    Ok(result)
}

fn parse_environment(block: &Block) -> Result<EnvironmentDeclarationV1, ContractError> {
    fields(
        block,
        &[
            "os",
            "arch",
            "sandbox_version",
            "sandbox_revision",
            "backend",
            "environment_digest",
            "backend_digest",
            "policy_digest",
            "capability_digest",
            "qualification_digest",
            "interpreter",
            "interpreter_digest",
            "interpreter_version",
            "environment_allowlist",
            "maker_root",
            "candidate_root",
            "scratch_root",
            "evidence_root",
            "result_root",
            "other_attempt_roots",
        ],
        false,
    )?;
    let sandbox_version = string(block, "sandbox_version")?;
    let sandbox_revision = string(block, "sandbox_revision")?;
    if sandbox_version != "0.2.1" || sandbox_revision != "2b8687892bbb4a5e3d6e74fca42949a3f186d236"
    {
        return Err(invalid(
            "resolved sandbox identity differs from the v1 contract",
        ));
    }
    let other_attempt_roots = strings(block, "other_attempt_roots", false)?
        .into_iter()
        .map(|value| validate_absolute_path(&value))
        .collect::<Result<_, _>>()?;
    Ok(EnvironmentDeclarationV1 {
        os: string(block, "os")?,
        arch: string(block, "arch")?,
        sandbox_version,
        sandbox_revision,
        backend: string(block, "backend")?,
        environment_digest: digest(block, "environment_digest")?,
        backend_digest: digest(block, "backend_digest")?,
        policy_digest: digest(block, "policy_digest")?,
        capability_digest: digest(block, "capability_digest")?,
        qualification_digest: digest(block, "qualification_digest")?,
        interpreter: absolute_path(block, "interpreter")?,
        interpreter_digest: digest(block, "interpreter_digest")?,
        interpreter_version: string(block, "interpreter_version")?,
        environment_allowlist: strings(block, "environment_allowlist", false)?,
        maker_root: absolute_path(block, "maker_root")?,
        candidate_root: absolute_path(block, "candidate_root")?,
        scratch_root: absolute_path(block, "scratch_root")?,
        evidence_root: absolute_path(block, "evidence_root")?,
        result_root: absolute_path(block, "result_root")?,
        other_attempt_roots,
    })
}

fn parse_live(block: &Block) -> Result<LiveDeclarationV1, ContractError> {
    fields(
        block,
        &[
            "provider_model_route",
            "provider_revision",
            "model_revision",
            "retry_policy_digest",
            "fallback_policy_digest",
            "admission_budget_digest",
            "token_bound",
            "money_bound_microusd",
            "first_attempt_id",
            "remediation_attempt_id",
            "usage_uncertainty_policy",
        ],
        false,
    )?;
    if !block.attributes.contains_key("token_bound")
        || !block.attributes.contains_key("money_bound_microusd")
        || !block.attributes.contains_key("admission_budget_digest")
    {
        return Err(ContractError::BudgetNotAdmitted(
            "live bounds and budget identity are required",
        ));
    }
    let token_bound = integer(block, "token_bound", MAX_EXACT_ACL_INTEGER).map_err(|_| {
        ContractError::BudgetNotAdmitted("token_bound must be a positive exact integer")
    })?;
    let money_bound_microusd = match block.attributes.get("money_bound_microusd") {
        Some(Value::String(value)) => canonical_money(value)?,
        _ => {
            return Err(ContractError::BudgetNotAdmitted(
                "money_bound_microusd must be a positive canonical decimal string",
            ))
        }
    };
    let first_attempt_id = string(block, "first_attempt_id")?;
    let remediation_attempt_id = string(block, "remediation_attempt_id")?;
    if first_attempt_id == remediation_attempt_id {
        return Err(invalid("logical attempt IDs must differ"));
    }
    Ok(LiveDeclarationV1 {
        provider_model_route: string(block, "provider_model_route")?,
        provider_revision: optional_identity(block, "provider_revision")?,
        model_revision: optional_identity(block, "model_revision")?,
        retry_policy_digest: digest(block, "retry_policy_digest")?,
        fallback_policy_digest: digest(block, "fallback_policy_digest")?,
        admission_budget_digest: digest(block, "admission_budget_digest")?,
        token_bound,
        money_bound_microusd,
        first_attempt_id,
        remediation_attempt_id,
        usage_uncertainty_policy: super::report::UsageUncertaintyPolicyV1::parse(&string(
            block,
            "usage_uncertainty_policy",
        )?)?,
    })
}

fn canonical_money(value: &str) -> Result<u64, ContractError> {
    let amount = value
        .parse::<u64>()
        .ok()
        .filter(|amount| *amount > 0 && amount.to_string() == value);
    amount.ok_or(ContractError::BudgetNotAdmitted(
        "money_bound_microusd must be a positive canonical decimal u64",
    ))
}

fn invalid(value: impl Into<String>) -> ContractError {
    ContractError::Manifest(value.into())
}
fn fields(block: &Block, allowed: &[&str], children: bool) -> Result<(), ContractError> {
    if (!children && !block.blocks.is_empty())
        || block
            .attributes
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
    {
        return Err(invalid(format!(
            "unknown fields or nested blocks in {}",
            block.name
        )));
    }
    Ok(())
}
fn attr<'a>(block: &'a Block, key: &str) -> Result<&'a Value, ContractError> {
    block
        .attributes
        .get(key)
        .ok_or_else(|| invalid(format!("missing {}.{key}", block.name)))
}
fn string(block: &Block, key: &str) -> Result<String, ContractError> {
    let Value::String(value) = attr(block, key)? else {
        return Err(invalid(format!(
            "{}.{key} requires a literal string",
            block.name
        )));
    };
    validate_id(value)?;
    Ok(value.clone())
}
pub fn validate_id(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES as usize
        || value.contains(['\0', '\r', '\n'])
    {
        return Err(invalid("empty, oversized or control-bearing exact string"));
    }
    Ok(())
}
fn optional_identity(block: &Block, key: &str) -> Result<Option<String>, ContractError> {
    match attr(block, key)? {
        Value::Null => Ok(None),
        Value::String(_) => string(block, key).map(Some),
        _ => Err(invalid(format!(
            "{key} requires a literal string or explicit null"
        ))),
    }
}
fn digest(block: &Block, key: &str) -> Result<DigestV1, ContractError> {
    DigestV1::new(string(block, key)?)
}
fn integer(block: &Block, key: &str, ceiling: u64) -> Result<u64, ContractError> {
    match attr(block, key)? {
        Value::Number(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value > 0.0
                && *value <= ceiling as f64 =>
        {
            Ok(*value as u64)
        }
        _ => Err(invalid(format!(
            "{}.{key} requires an integer in 1..={ceiling}",
            block.name
        ))),
    }
}
fn strings(block: &Block, key: &str, nonempty: bool) -> Result<Vec<String>, ContractError> {
    let Value::List(values) = attr(block, key)? else {
        return Err(invalid(format!("{key} requires a literal string list")));
    };
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for value in values {
        let Value::String(value) = value else {
            return Err(invalid(format!("{key} contains a non-string")));
        };
        validate_id(value)?;
        if !seen.insert(value) {
            return Err(invalid(format!("duplicate {key} entry")));
        }
        result.push(value.clone());
    }
    if nonempty && result.is_empty() {
        return Err(invalid(format!("{key} must not be empty")));
    }
    Ok(result)
}
pub fn validate_relative_path(value: &str, ceiling: u64) -> Result<(), ContractError> {
    validate_id(value)?;
    if value.len() as u64 > ceiling
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(value)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid(
            "candidate path is not an exact normal relative path",
        ));
    }
    Ok(())
}
fn validate_absolute_path(value: &str) -> Result<PathBuf, ContractError> {
    validate_id(value)?;
    let path = Path::new(value);
    if !path.is_absolute()
        || value
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid(
            "host root/interpreter requires an exact absolute path",
        ));
    }
    Ok(path.to_path_buf())
}
fn absolute_path(block: &Block, key: &str) -> Result<PathBuf, ContractError> {
    validate_absolute_path(&string(block, key)?)
}

/// ACL 0.3.0's AST overwrites attributes and represents numbers as f64.
/// Its own lexer spans allow fail-closed checks without another ACL grammar.
fn reject_lossy_tokens(source: &str) -> Result<(), ContractError> {
    let tokens = a3s_acl::Lexer::new(source).tokenize();
    let mut scopes = vec![BTreeSet::new()];
    for (index, token) in tokens.iter().enumerate() {
        match &token.token {
            Token::LeftBrace => scopes.push(BTreeSet::new()),
            Token::RightBrace => {
                if scopes.len() > 1 {
                    scopes.pop();
                }
            }
            Token::Ident(key) | Token::String(key)
                if tokens.get(index + 1).is_some_and(|next| {
                    matches!(next.token, Token::Equal | Token::Colon | Token::PlusEqual)
                }) =>
            {
                let scope = scopes
                    .last_mut()
                    .ok_or_else(|| invalid("invalid ACL scope"))?;
                if !scope.insert(key.clone()) {
                    return Err(invalid(format!("duplicate attribute {key}")));
                }
            }
            Token::Number(_) => {
                let raw = source
                    .get(token.span.start.offset..token.span.end.offset)
                    .ok_or_else(|| invalid("invalid ACL numeric span"))?;
                let integer = raw.parse::<u64>().map_err(|_| {
                    invalid("numeric ACL limits require canonical positive integer literals")
                })?;
                if integer > MAX_EXACT_ACL_INTEGER || integer.to_string() != raw {
                    return Err(invalid(
                        "ACL numeric literal is not an exact canonical bounded integer",
                    ));
                }
            }
            Token::PlusEqual => return Err(invalid("manifest attributes cannot be appended")),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_manifest_bytes(path: &Path) -> Result<Vec<u8>, ContractError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let components = absolute
        .components()
        .filter(|component| !matches!(component, Component::RootDir))
        .collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid(
            "manifest path must have normal absolute components",
        ));
    }
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open("/")?;
    for (index, component) in components.iter().enumerate() {
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| invalid("NUL in manifest path"))?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // fstatat initializes the complete stat buffer on success; the owned
        // directory and NUL-terminated component remain live for this call.
        let status = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // The successful call above initialized this buffer.
        let before = unsafe { metadata.assume_init() };
        let last = index + 1 == components.len();
        let kind = before.st_mode & libc::S_IFMT;
        if (last
            && (kind != libc::S_IFREG
                || before.st_nlink != 1
                || before.st_size < 0
                || before.st_size as u64 > MAX_MANIFEST_BYTES as u64))
            || (!last && kind != libc::S_IFDIR)
        {
            return Err(invalid(
                "manifest path contains a link, nonregular file or oversized file",
            ));
        }
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC
            | if last { 0 } else { libc::O_DIRECTORY };
        // The component cannot redirect through a symlink; O_NONBLOCK also
        // prevents a concurrent regular-file-to-FIFO swap from blocking.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // A successful openat yields one uniquely owned descriptor.
        let file = unsafe { std::fs::File::from_raw_fd(descriptor) };
        let opened = file.metadata()?;
        if opened.dev() != before.st_dev as u64
            || u128::from(opened.ino()) != u128::from(before.st_ino)
        {
            return Err(invalid("manifest component changed before safe open"));
        }
        if last {
            if !opened.file_type().is_file()
                || opened.nlink() != 1
                || opened.len() > MAX_MANIFEST_BYTES as u64
            {
                return Err(invalid("manifest changed before regular-file read"));
            }
            let mut bytes = Vec::new();
            (&file)
                .take(MAX_MANIFEST_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_MANIFEST_BYTES || file.metadata()?.len() != opened.len() {
                return Err(invalid("manifest changed or exceeded 1 MiB while reading"));
            }
            return Ok(bytes);
        }
        directory = file;
    }
    Err(invalid("manifest file is absent"))
}
#[cfg(not(unix))]
fn read_manifest_bytes(_: &Path) -> Result<Vec<u8>, ContractError> {
    Err(invalid(
        "safe manifest open is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests;
