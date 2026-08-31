use a3s_code_core::release::{
    agent_harness_compatibility_v1, AgentReleaseManifest, AgentReleaseProvenance,
};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: publish_agent_release_fixture <template> <output> <artifact-digest> <source-uri> <source-digest> <builder-uri> <builder-digest>";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let template_path = required_path(&mut arguments)?;
    let output_path = required_path(&mut arguments)?;
    let artifact_digest = required_utf8(&mut arguments)?;
    let source_uri = required_utf8(&mut arguments)?;
    let source_digest = required_utf8(&mut arguments)?;
    let builder_uri = required_utf8(&mut arguments)?;
    let builder_digest = required_utf8(&mut arguments)?;
    if arguments.next().is_some() {
        return Err(USAGE.into());
    }

    let template = AgentReleaseManifest::from_file(&template_path)?;
    let manifest = template.bind_publication(
        artifact_digest,
        [
            AgentReleaseProvenance::new("source", source_uri, source_digest)?,
            AgentReleaseProvenance::new("builder", builder_uri, builder_digest)?,
        ],
    )?;
    manifest.verify_compatibility(&agent_harness_compatibility_v1())?;
    write_new(&output_path, manifest.canonical_acl().as_bytes())?;

    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": "a3s.code.agent-release-publication.v1",
            "artifactDigest": manifest.artifact().digest(),
            "manifestIdentity": manifest.identity(),
            "protocol": manifest.protocol(),
            "health": {
                "port": manifest.health().port(),
                "readinessPath": manifest.health().readiness_path(),
                "livenessPath": manifest.health().liveness_path(),
                "shutdownGraceSeconds": manifest.health().shutdown_grace_seconds(),
            },
            "provenanceKinds": manifest
                .provenance()
                .iter()
                .map(|reference| reference.kind())
                .collect::<Vec<_>>(),
        }))?
    );
    Ok(())
}

fn required_path(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| USAGE.into())
}

fn required_utf8(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<String, Box<dyn std::error::Error>> {
    arguments
        .next()
        .ok_or_else(|| Box::<dyn std::error::Error>::from(USAGE))?
        .into_string()
        .map_err(|_| Box::<dyn std::error::Error>::from("digest and URI arguments must be UTF-8"))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = output.write_all(bytes).and_then(|()| output.sync_all()) {
        drop(output);
        let _ = std::fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}
