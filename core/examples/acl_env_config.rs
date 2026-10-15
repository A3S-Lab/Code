//! Release-only ACL env injection. Credentials never cross stdout or argv.

use a3s_acl::{Block, Document, Value};
use a3s_code_core::config::CodeConfig;
use anyhow::{anyhow, bail, ensure, Context, Result};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, ExitCode},
};

#[path = "acl_env_config/tests.rs"]
#[cfg(test)]
mod tests;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

enum Mode {
    Check,
    Run,
}

impl Mode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "check" => Ok(Self::Check),
            "run" => Ok(Self::Run),
            _ => bail!("ACL helper mode must be check or run"),
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Run => "run",
        }
    }
}

struct Prepared {
    document: Document,
    api_key: Option<String>,
    base_url: Option<String>,
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn env_attribute(name: &str) -> Value {
    Value::Call("env".to_owned(), vec![Value::String(name.to_owned())])
}

fn rewrite_credentials(block: &mut Block) {
    for name in ["apiKey", "api_key", "baseUrl", "base_url"] {
        block.attributes.remove(name);
    }
    block
        .attributes
        .insert("api_key".to_owned(), env_attribute("A3S_OPENAI_API_KEY"));
    block
        .attributes
        .insert("base_url".to_owned(), env_attribute("A3S_OPENAI_BASE_URL"));
}

fn prepare(content: &str, env: impl Fn(&str) -> Option<String>) -> Result<Prepared> {
    // Do not include parser diagnostics: malformed input can contain secrets.
    let config = CodeConfig::from_acl(content).map_err(|_| anyhow!("invalid ACL configuration"))?;
    let (provider, model) = config
        .default_model_config()
        .ok_or_else(|| anyhow!("configured default provider/model is missing"))?;
    let mut document = a3s_acl::parse(content).map_err(|_| anyhow!("invalid ACL configuration"))?;
    // This is an environment-variable spelling, never an entity classifier.
    let provider_env: String = provider
        .name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let api_key = nonempty(env(&format!("A3S_{provider_env}_API_KEY")))
        .or_else(|| nonempty(provider.get_api_key(model).map(str::to_owned)))
        .or_else(|| nonempty(env("A3S_OPENAI_API_KEY")))
        .or_else(|| nonempty(env("MINIMAX_API_KEY")));
    let base_url = nonempty(env(&format!("A3S_{provider_env}_BASE_URL")))
        .or_else(|| nonempty(provider.get_base_url(model).map(str::to_owned)))
        .or_else(|| nonempty(env("A3S_OPENAI_BASE_URL")))
        .or_else(|| nonempty(env("MINIMAX_BASE_URL")));
    let mut matches = document
        .blocks
        .iter_mut()
        .filter(|block| block.name == "providers" && block.labels.first() == Some(&provider.name));
    let block = matches
        .next()
        .ok_or_else(|| anyhow!("default provider ACL block is missing"))?;
    ensure!(
        matches.next().is_none(),
        "default provider ACL block is ambiguous"
    );
    rewrite_credentials(block);
    // Model overrides take precedence in Core and must use the injected env too.
    for child in &mut block.blocks {
        if child.name == "models" && child.labels.first() == Some(&model.id) {
            rewrite_credentials(child);
        }
    }
    Ok(Prepared {
        document,
        api_key,
        base_url,
    })
}

fn read_config(path: &Path) -> Result<String> {
    ensure!(
        path.extension().is_some_and(|extension| extension == "acl"),
        "configuration must use .acl"
    );
    let before = fs::symlink_metadata(path).context("cannot inspect ACL file")?;
    ensure!(before.is_file(), "ACL source must be a regular file");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("cannot open ACL file")?;
    let opened = file.metadata().context("cannot inspect opened ACL file")?;
    ensure!(opened.is_file(), "opened ACL source must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == opened.dev() && before.ino() == opened.ino(),
            "ACL source changed during open"
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_CONFIG_BYTES,
        "ACL source exceeds 1 MiB"
    );
    String::from_utf8(bytes).map_err(|_| anyhow!("ACL source must be UTF-8"))
}

fn run() -> Result<i32> {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .ok_or_else(|| anyhow!("ACL helper mode is required"))?;
    let mode = Mode::parse(mode.to_str().unwrap_or_default())?;
    let path = args.next().ok_or_else(|| anyhow!("ACL file is required"))?;
    let prepared = prepare(&read_config(Path::new(&path))?, |name| {
        std::env::var(name).ok()
    })?;
    match mode {
        Mode::Check => {
            ensure!(args.next().is_none(), "check accepts only an ACL file");
            println!(
                "{}",
                u8::from(prepared.api_key.is_some() && prepared.base_url.is_some())
            );
            Ok(0)
        }
        Mode::Run => {
            let command: OsString = args
                .next()
                .ok_or_else(|| anyhow!("run requires a child command"))?;
            let mut temporary = tempfile::Builder::new()
                .prefix("a3s-code-env-")
                .suffix(".acl")
                .tempfile()?;
            // NamedTempFile is exclusively created with mode 0600 on Unix and
            // is retained until the child exits; errors and signals fail closed.
            temporary.write_all(a3s_acl::generate(&prepared.document).as_bytes())?;
            temporary.flush()?;
            let mut child = Command::new(command);
            child
                .args(args)
                .env("A3S_CONFIG_FILE", temporary.path())
                .env("A3S_CODE_ACL_ENV_PREPARED", "1");
            for (name, value) in [
                ("A3S_OPENAI_API_KEY", prepared.api_key),
                ("A3S_OPENAI_BASE_URL", prepared.base_url),
            ] {
                if let Some(value) = value {
                    child.env(name, value);
                } else {
                    child.env_remove(name);
                }
            }
            let status = child
                .status()
                .with_context(|| format!("{} child could not start", mode.as_str()))?;
            Ok(status.code().unwrap_or(1))
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("ACL env preparation failed: {error}");
            ExitCode::FAILURE
        }
    }
}
