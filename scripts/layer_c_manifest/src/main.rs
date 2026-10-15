//! Validate the Layer C ACL contract before emitting its complete suite list.
use a3s_acl::{parse, Block, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_SUITES: usize = 256;

#[derive(Clone, Copy)]
enum Track {
    Mandatory,
    Measurement,
}

impl Track {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "mandatory" => Ok(Self::Mandatory),
            "measurement" => Ok(Self::Measurement),
            _ => Err(format!("unknown Layer C track: {raw}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Mandatory => "mandatory",
            Self::Measurement => "measurement",
        }
    }
}

struct Suite {
    name: String,
    package: String,
    track: Track,
    features: Vec<String>,
    minimum_tests: usize,
}

fn token(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 128
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn feature_token(raw: &str) -> bool {
    if token(raw) {
        return true;
    }
    // ACP's live targets may select features on their Core dependency. Keep
    // the dependency scope explicit; whitespace, extra slashes and arbitrary
    // dependency names remain refused before emitting any suite contract.
    raw.len() <= 128
        && raw
            .split_once('/')
            .is_some_and(|(package, feature)| package == "a3s-code-core" && token(feature))
}

fn string_attr<'a>(block: &'a Block, name: &str) -> Result<&'a str, String> {
    match block.attributes.get(name) {
        Some(Value::String(raw)) => Ok(raw),
        _ => Err(format!(
            "suite {} requires string attribute {name}",
            block.labels.join("/")
        )),
    }
}

fn suites(input: &str) -> Result<Vec<Suite>, String> {
    let document = parse(input).map_err(|error| error.to_string())?;
    if document.blocks.is_empty() || document.blocks.len() > MAX_SUITES {
        return Err(format!("Layer C requires 1..={MAX_SUITES} suite blocks"));
    }
    let mut result = Vec::new();
    let mut names = HashSet::new();
    for block in document.blocks {
        if block.name != "suite" || block.labels.len() != 1 || !block.blocks.is_empty() {
            return Err(
                "Layer C accepts only suite blocks with one label and no nested blocks".into(),
            );
        }
        let name = block.labels[0].clone();
        if !token(&name) || !names.insert(name.clone()) {
            return Err(format!("invalid or duplicate Layer C suite name: {name}"));
        }
        for key in block.attributes.keys() {
            if !["package", "track", "features", "minimum_tests"].contains(&key.as_str()) {
                return Err(format!("suite {name} has unknown attribute {key}"));
            }
        }
        let package = string_attr(&block, "package")?.to_owned();
        if !["a3s-code-core", "a3s-code-acp"].contains(&package.as_str()) {
            return Err(format!("unknown Layer C package: {package}"));
        }
        let track = Track::parse(string_attr(&block, "track")?)?;
        let features = match block.attributes.get("features") {
            Some(Value::List(values)) => {
                let mut features = Vec::new();
                let mut unique = HashSet::new();
                for value in values {
                    let Value::String(feature) = value else {
                        return Err(format!("suite {name} features must be strings"));
                    };
                    if !feature_token(feature) || !unique.insert(feature.clone()) {
                        return Err(format!(
                            "suite {name} has invalid or duplicate feature {feature}"
                        ));
                    }
                    features.push(feature.clone());
                }
                features
            }
            None => Vec::new(),
            _ => return Err(format!("suite {name} features must be a list")),
        };
        let minimum_tests = match block.attributes.get("minimum_tests") {
            Some(Value::Number(number))
                if number.is_finite()
                    && *number >= 1.0
                    && *number <= 100_000.0
                    && number.fract() == 0.0 =>
            {
                *number as usize
            }
            _ => {
                return Err(format!(
                    "suite {name} requires positive integer minimum_tests"
                ))
            }
        };
        result.push(Suite {
            name,
            package,
            track,
            features,
            minimum_tests,
        });
    }
    if !result
        .iter()
        .any(|suite| matches!(suite.track, Track::Mandatory))
    {
        return Err("Layer C requires at least one mandatory suite".into());
    }
    Ok(result)
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: a3s-code-layer-c-manifest <manifest.acl>")?;
    if args.next().is_some() {
        return Err("usage: a3s-code-layer-c-manifest <manifest.acl>".into());
    }
    let path = Path::new(&path);
    if path.extension().is_none_or(|extension| extension != "acl") {
        return Err("Layer C manifest must have the .acl extension".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES {
        return Err("Layer C manifest must be a regular file no larger than 1 MiB".into());
    }
    let mut input = String::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut input)
        .map_err(|error| error.to_string())?;
    if input.len() as u64 > MAX_BYTES {
        return Err("Layer C manifest exceeds 1 MiB".into());
    }
    // Emit only after validating the entire contract: partial output must never
    // shrink the expected-receipt set after a later malformed suite.
    let suites = suites(&input)?;
    let mut output = io::BufWriter::new(io::stdout().lock());
    for suite in suites {
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}",
            suite.name,
            suite.package,
            suite.track.as_str(),
            suite.features.join(","),
            suite.minimum_tests
        )
        .map_err(|error| error.to_string())?;
    }
    output.flush().map_err(|error| error.to_string())?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Layer C manifest refused: {error}");
        std::process::exit(1);
    }
}
