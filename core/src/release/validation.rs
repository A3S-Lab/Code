use super::types::{
    AgentReleaseArtifact, AgentReleaseCacheMode, AgentReleaseCapability, AgentReleaseEntrypoint,
    AgentReleaseHealth, AgentReleasePersistentDataMode, AgentReleaseProvenance,
    AgentReleaseSecretRequirement, AgentReleaseSecretTarget, AgentReleaseStorage,
    AgentReleaseWorkspaceMode,
};
use super::{
    AgentReleaseError, AgentReleaseField, AGENT_RELEASE_ENTRYPOINT_ARGS_V1,
    AGENT_RELEASE_ENTRYPOINT_COMMAND_V1, AGENT_RELEASE_OCI_MEDIA_TYPE, MAX_CAPABILITY_LEVEL,
    MAX_ENTRYPOINT_ARGS, MAX_SHUTDOWN_GRACE_SECONDS,
};
use a3s_acl::{Block, Value};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

const MAX_COMMAND_BYTES: usize = 1_024;
const MAX_ENTRYPOINT_ARGUMENT_BYTES: usize = 4_096;
const MAX_HEALTH_PATH_BYTES: usize = 256;
const MAX_NAME_BYTES: usize = 128;
const MAX_PROVENANCE_URI_BYTES: usize = 2_048;
const MAX_SECRET_DESTINATION_BYTES: usize = 256;
const SECRET_FILE_PREFIX: &str = "/run/secrets/";

pub(crate) fn parse_artifact(block: &Block) -> Result<AgentReleaseArtifact, AgentReleaseError> {
    let digest = required_string(block, "digest", AgentReleaseField::ArtifactDigest)?;
    validate_digest(&digest, AgentReleaseField::ArtifactDigest)?;
    let media_type = required_string(block, "media_type", AgentReleaseField::ArtifactMediaType)?;
    if media_type != AGENT_RELEASE_OCI_MEDIA_TYPE {
        return Err(AgentReleaseError::InvalidField(
            AgentReleaseField::ArtifactMediaType,
        ));
    }
    Ok(AgentReleaseArtifact { digest, media_type })
}

pub(crate) fn parse_entrypoint(block: &Block) -> Result<AgentReleaseEntrypoint, AgentReleaseError> {
    let command = required_string(block, "command", AgentReleaseField::EntrypointCommand)?;
    if command.len() > MAX_COMMAND_BYTES
        || !command.starts_with('/')
        || command
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AgentReleaseError::InvalidField(
            AgentReleaseField::EntrypointCommand,
        ));
    }
    if command != AGENT_RELEASE_ENTRYPOINT_COMMAND_V1 {
        return Err(AgentReleaseError::InvalidField(
            AgentReleaseField::EntrypointCommand,
        ));
    }

    let args = match block.attributes.get("args") {
        Some(Value::List(values)) if values.len() <= MAX_ENTRYPOINT_ARGS => values
            .iter()
            .map(|value| match value {
                Value::String(value)
                    if value.len() <= MAX_ENTRYPOINT_ARGUMENT_BYTES
                        && !value.chars().any(char::is_control) =>
                {
                    Ok(value.clone())
                }
                _ => Err(AgentReleaseError::InvalidField(
                    AgentReleaseField::EntrypointArgument,
                )),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(AgentReleaseError::InvalidField(
                AgentReleaseField::EntrypointArgument,
            ))
        }
    };
    if !args
        .iter()
        .map(String::as_str)
        .eq(AGENT_RELEASE_ENTRYPOINT_ARGS_V1)
    {
        return Err(AgentReleaseError::InvalidField(
            AgentReleaseField::EntrypointArgument,
        ));
    }
    Ok(AgentReleaseEntrypoint { command, args })
}

pub(crate) fn parse_health(block: &Block) -> Result<AgentReleaseHealth, AgentReleaseError> {
    let transport = required_string(block, "transport", AgentReleaseField::HealthTransport)?;
    if transport != "http" {
        return Err(AgentReleaseError::InvalidField(
            AgentReleaseField::HealthTransport,
        ));
    }
    let port = required_integer(
        block,
        "port",
        AgentReleaseField::HealthPort,
        u16::MAX as u32,
    )? as u16;
    let readiness_path =
        required_string(block, "readiness_path", AgentReleaseField::ReadinessPath)?;
    validate_health_path(&readiness_path, AgentReleaseField::ReadinessPath)?;
    let liveness_path = required_string(block, "liveness_path", AgentReleaseField::LivenessPath)?;
    validate_health_path(&liveness_path, AgentReleaseField::LivenessPath)?;
    if readiness_path == liveness_path {
        return Err(AgentReleaseError::InvalidField(
            AgentReleaseField::LivenessPath,
        ));
    }
    let shutdown_grace_seconds = required_integer(
        block,
        "shutdown_grace_seconds",
        AgentReleaseField::ShutdownGraceSeconds,
        MAX_SHUTDOWN_GRACE_SECONDS,
    )?;
    Ok(AgentReleaseHealth {
        transport,
        port,
        readiness_path,
        liveness_path,
        shutdown_grace_seconds,
    })
}

pub(crate) fn parse_storage(block: &Block) -> Result<AgentReleaseStorage, AgentReleaseError> {
    let workspace =
        match required_string(block, "workspace", AgentReleaseField::WorkspaceMode)?.as_str() {
            "read_only" => AgentReleaseWorkspaceMode::ReadOnly,
            "ephemeral" => AgentReleaseWorkspaceMode::Ephemeral,
            _ => {
                return Err(AgentReleaseError::InvalidField(
                    AgentReleaseField::WorkspaceMode,
                ))
            }
        };
    let cache = match required_string(block, "cache", AgentReleaseField::CacheMode)?.as_str() {
        "none" => AgentReleaseCacheMode::None,
        "ephemeral" => AgentReleaseCacheMode::Ephemeral,
        _ => {
            return Err(AgentReleaseError::InvalidField(
                AgentReleaseField::CacheMode,
            ))
        }
    };
    let persistent_data = match required_string(
        block,
        "persistent_data",
        AgentReleaseField::PersistentDataMode,
    )?
    .as_str()
    {
        "none" => AgentReleasePersistentDataMode::None,
        "external" => AgentReleasePersistentDataMode::External,
        _ => {
            return Err(AgentReleaseError::InvalidField(
                AgentReleaseField::PersistentDataMode,
            ))
        }
    };
    Ok(AgentReleaseStorage {
        workspace,
        cache,
        persistent_data,
    })
}

pub(crate) fn parse_capability(block: &Block) -> Result<AgentReleaseCapability, AgentReleaseError> {
    let name = block
        .labels
        .first()
        .cloned()
        .ok_or(AgentReleaseError::InvalidField(
            AgentReleaseField::CapabilityName,
        ))?;
    let level = required_integer(
        block,
        "level",
        AgentReleaseField::CapabilityLevel,
        MAX_CAPABILITY_LEVEL,
    )?;
    AgentReleaseCapability::new(name, level)
}

pub(crate) fn parse_secret(
    block: &Block,
) -> Result<AgentReleaseSecretRequirement, AgentReleaseError> {
    let name = block
        .labels
        .first()
        .cloned()
        .ok_or(AgentReleaseError::InvalidField(
            AgentReleaseField::SecretName,
        ))?;
    validate_dotted_name(&name, AgentReleaseField::SecretName)?;
    let target = match required_string(block, "target", AgentReleaseField::SecretTarget)?.as_str() {
        "environment" => AgentReleaseSecretTarget::Environment,
        "file" => AgentReleaseSecretTarget::File,
        _ => {
            return Err(AgentReleaseError::InvalidField(
                AgentReleaseField::SecretTarget,
            ))
        }
    };
    let destination = required_string(block, "destination", AgentReleaseField::SecretDestination)?;
    match target {
        AgentReleaseSecretTarget::Environment => validate_environment_name(&destination)?,
        AgentReleaseSecretTarget::File => validate_secret_path(&destination)?,
    }
    Ok(AgentReleaseSecretRequirement {
        name,
        target,
        destination,
    })
}

pub(crate) fn parse_provenance(block: &Block) -> Result<AgentReleaseProvenance, AgentReleaseError> {
    let kind = block
        .labels
        .first()
        .cloned()
        .ok_or(AgentReleaseError::InvalidField(
            AgentReleaseField::ProvenanceKind,
        ))?;
    let uri = required_string(block, "uri", AgentReleaseField::ProvenanceUri)?;
    let digest = required_string(block, "digest", AgentReleaseField::ProvenanceDigest)?;
    AgentReleaseProvenance::new(kind, uri, digest)
}

pub(crate) fn unique_capabilities(
    capabilities: impl IntoIterator<Item = AgentReleaseCapability>,
) -> Result<Vec<AgentReleaseCapability>, AgentReleaseError> {
    let mut unique = BTreeMap::new();
    for capability in capabilities {
        if unique.insert(capability.name.clone(), capability).is_some() {
            return Err(AgentReleaseError::DuplicateCapability);
        }
    }
    Ok(unique.into_values().collect())
}

pub(crate) fn unique_provenance(
    provenance: impl IntoIterator<Item = AgentReleaseProvenance>,
) -> Result<Vec<AgentReleaseProvenance>, AgentReleaseError> {
    let mut unique = BTreeMap::new();
    for reference in provenance {
        if unique.insert(reference.kind.clone(), reference).is_some() {
            return Err(AgentReleaseError::DuplicateProvenance);
        }
    }
    Ok(unique.into_values().collect())
}

pub(crate) fn unique_secrets(
    secrets: impl IntoIterator<Item = AgentReleaseSecretRequirement>,
) -> Result<Vec<AgentReleaseSecretRequirement>, AgentReleaseError> {
    let mut names = BTreeMap::new();
    let mut destinations = BTreeSet::new();
    for secret in secrets {
        let destination = (secret.target.as_str(), secret.destination.clone());
        if names.insert(secret.name.clone(), secret).is_some() || !destinations.insert(destination)
        {
            return Err(AgentReleaseError::DuplicateSecret);
        }
    }
    Ok(names.into_values().collect())
}

pub(crate) fn required_block<'a>(
    blocks: &'a [Block],
    name: &str,
    field: AgentReleaseField,
) -> Result<&'a Block, AgentReleaseError> {
    blocks
        .iter()
        .find(|block| block.name == name)
        .ok_or(AgentReleaseError::InvalidField(field))
}

pub(crate) fn validate_dotted_name(
    name: &str,
    field: AgentReleaseField,
) -> Result<(), AgentReleaseError> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name.split('.').all(|segment| {
            let mut bytes = segment.bytes();
            bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
                && bytes
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(field))
    }
}

pub(crate) fn validate_protocol(protocol: &str) -> Result<(), AgentReleaseError> {
    let valid = protocol
        .strip_prefix("a3s.code.agent.v")
        .and_then(|value| value.parse::<u32>().ok().map(|version| (value, version)))
        .is_some_and(|(source, version)| {
            version > 0 && version <= MAX_CAPABILITY_LEVEL && source == version.to_string()
        });
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(AgentReleaseField::Protocol))
    }
}

pub(crate) fn required_string(
    block: &Block,
    name: &str,
    field: AgentReleaseField,
) -> Result<String, AgentReleaseError> {
    match block.attributes.get(name) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(AgentReleaseError::InvalidField(field)),
    }
}

fn required_integer(
    block: &Block,
    name: &str,
    field: AgentReleaseField,
    max: u32,
) -> Result<u32, AgentReleaseError> {
    match block.attributes.get(name) {
        Some(Value::Number(value))
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 1.0
                && *value <= max as f64 =>
        {
            Ok(*value as u32)
        }
        _ => Err(AgentReleaseError::InvalidField(field)),
    }
}

pub(crate) fn validate_digest(
    digest: &str,
    field: AgentReleaseField,
) -> Result<(), AgentReleaseError> {
    let valid = digest.strip_prefix("sha256:").is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    });
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(field))
    }
}

fn validate_health_path(path: &str, field: AgentReleaseField) -> Result<(), AgentReleaseError> {
    let valid = path.strip_prefix('/').is_some_and(|relative| {
        !relative.is_empty()
            && relative.split('/').all(|segment| {
                !segment.is_empty()
                    && segment != "."
                    && segment != ".."
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                    })
            })
    }) && path.len() <= MAX_HEALTH_PATH_BYTES;
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(field))
    }
}

fn validate_environment_name(name: &str) -> Result<(), AgentReleaseError> {
    let mut bytes = name.bytes();
    let valid = name.len() <= MAX_NAME_BYTES
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(
            AgentReleaseField::SecretDestination,
        ))
    }
}

fn validate_secret_path(path: &str) -> Result<(), AgentReleaseError> {
    let valid = path
        .strip_prefix(SECRET_FILE_PREFIX)
        .is_some_and(|relative| {
            !relative.is_empty()
                && relative.split('/').all(|segment| {
                    !segment.is_empty()
                        && segment != "."
                        && segment != ".."
                        && segment.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')
                        })
                })
        })
        && path.len() <= MAX_SECRET_DESTINATION_BYTES;
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(
            AgentReleaseField::SecretDestination,
        ))
    }
}

pub(crate) fn validate_provenance_uri(uri: &str) -> Result<(), AgentReleaseError> {
    let parsed = Url::parse(uri)
        .map_err(|_| AgentReleaseError::InvalidField(AgentReleaseField::ProvenanceUri))?;
    let scheme_valid = match parsed.scheme() {
        "https" => parsed.host_str().is_some(),
        "urn" => !parsed.path().is_empty(),
        _ => false,
    };
    let valid = uri.len() <= MAX_PROVENANCE_URI_BYTES
        && uri.is_ascii()
        && !uri
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        && scheme_valid
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    if valid {
        Ok(())
    } else {
        Err(AgentReleaseError::InvalidField(
            AgentReleaseField::ProvenanceUri,
        ))
    }
}
