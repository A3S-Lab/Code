//! Persistent "don't ask again" approvals for interactive sessions.
//!
//! Only an explicit `allow-always` answer learns. The learned unit is a
//! command rule — the program plus the subcommand that decides its risk
//! (`git status`, `cargo test`, `npm run lint`), never a bare first token —
//! taken from a closed allowlist. A later `bash` call skips the ask only when
//! it parses to the same rule.
//!
//! Safety contract:
//! - a command carrying shell control syntax (`; & | $ \` < > ( ) { } \`,
//!   newline, control characters) is never learnable and never matches, so
//!   a learned rule cannot be chained into another command;
//! - per-program argument refusals keep a learned rule from reaching
//!   argument-level execution (`rg --pre`, `git grep -O`, `cargo --config`);
//! - rules live in the user state root (`<state root>/code/approvals.acl`),
//!   keyed by the canonical workspace path. A repository cannot ship or
//!   pre-seed approvals; the old `<workspace>/.a3s/approvals.acl` is
//!   ignored;
//! - the per-workspace list is capped and oldest-first evicted, and the
//!   document is A3S ACL parsed with `a3s-acl`.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const APPROVALS_FILE: &str = "approvals.acl";
const LOCK_FILE: &str = "approvals.lock";
const MAX_RULES_PER_WORKSPACE: usize = 64;
const MAX_COMMAND_LEN: usize = 4096;
const MAX_TOKEN_LEN: usize = 64;
const MAX_STORE_BYTES: u64 = 1024 * 1024;

/// Shell syntax that can chain, substitute, redirect or group commands.
const SHELL_CONTROL: &[char] = &[
    ';', '&', '|', '$', '`', '<', '>', '(', ')', '{', '}', '\\', '\n', '\r',
];

/// How a program's rule is formed.
enum Shape {
    /// The program alone; any (checked) arguments.
    Bare,
    /// The program plus one of these subcommands.
    Subcommand(&'static [&'static str]),
    /// The program plus a named target (`just test`, `make lint`).
    Target,
    /// The program plus `test`, or `run <script>`.
    PackageScript,
}

/// The closed set of learnable programs. Anything else always asks.
const LEARNABLE: &[(&str, Shape)] = &[
    ("ls", Shape::Bare),
    ("pwd", Shape::Bare),
    ("cat", Shape::Bare),
    ("head", Shape::Bare),
    ("tail", Shape::Bare),
    ("wc", Shape::Bare),
    ("grep", Shape::Bare),
    ("rg", Shape::Bare),
    ("which", Shape::Bare),
    ("file", Shape::Bare),
    ("stat", Shape::Bare),
    ("du", Shape::Bare),
    ("df", Shape::Bare),
    ("tree", Shape::Bare),
    ("diff", Shape::Bare),
    ("pytest", Shape::Bare),
    (
        "git",
        Shape::Subcommand(&[
            "status",
            "diff",
            "log",
            "show",
            "rev-parse",
            "ls-files",
            "blame",
            "grep",
            "shortlog",
            "describe",
        ]),
    ),
    (
        "cargo",
        Shape::Subcommand(&[
            "build", "check", "test", "clippy", "fmt", "doc", "tree", "metadata", "bench",
        ]),
    ),
    (
        "go",
        Shape::Subcommand(&["build", "test", "vet", "fmt", "list"]),
    ),
    ("npm", Shape::PackageScript),
    ("pnpm", Shape::PackageScript),
    ("yarn", Shape::PackageScript),
    ("bun", Shape::PackageScript),
    ("just", Shape::Target),
    ("make", Shape::Target),
];

/// Arguments that turn a learnable program into an arbitrary executor or
/// writer outside the user's approval.
fn refuses_argument(program: &str, argument: &str) -> bool {
    let refused: &[&str] = match program {
        "rg" => &["--pre"],
        "git" => &[
            "--output",
            "--ext-diff",
            "-O",
            "--open-files-in-pager",
            "--textconv",
        ],
        "cargo" => &["--config", "-Z"],
        "make" => &["-f", "--file", "--makefile", "-C", "--directory", "--eval"],
        "just" => &["-f", "--justfile", "-d", "--working-directory"],
        _ => &[],
    };
    refused.iter().any(|prefix| argument.starts_with(prefix))
}

fn is_plain_name(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN_LEN
        && !token.starts_with(['.', '-'])
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

/// Split a command that carries no shell control syntax. Only quoting is
/// left to honour; an unterminated quote is not a command we learn.
fn split_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => current.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

/// The rule one command would learn or match, or `None` when it must
/// always ask.
pub fn learnable_rule(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty()
        || command.len() > MAX_COMMAND_LEN
        || command
            .chars()
            .any(|c| SHELL_CONTROL.contains(&c) || c.is_control())
    {
        return None;
    }
    let words = split_words(command)?;
    let (program, rest) = words.split_first()?;
    if !is_plain_name(program) {
        return None;
    }
    let (_, shape) = LEARNABLE.iter().find(|(name, _)| *name == program)?;
    if rest
        .iter()
        .any(|argument| refuses_argument(program, argument))
    {
        return None;
    }
    let rule_words = match shape {
        Shape::Bare => vec![program.as_str()],
        Shape::Subcommand(allowed) => {
            let subcommand = rest.first()?;
            if !allowed.contains(&subcommand.as_str()) {
                return None;
            }
            vec![program.as_str(), subcommand.as_str()]
        }
        Shape::Target => {
            let target = rest.first()?;
            if !is_plain_name(target) {
                return None;
            }
            vec![program.as_str(), target.as_str()]
        }
        Shape::PackageScript => match rest.first().map(String::as_str) {
            Some("test") => vec![program.as_str(), "test"],
            Some("run") => {
                let script = rest.get(1)?;
                if !is_plain_name(script) {
                    return None;
                }
                vec![program.as_str(), "run", script.as_str()]
            }
            _ => return None,
        },
    };
    Some(rule_words.join(" "))
}

/// `<state root>/code/approvals.acl`.
pub fn approvals_store_path(workspace: &Path) -> anyhow::Result<PathBuf> {
    Ok(a3s_code_core::config::code_state_dir(workspace)?.join(APPROVALS_FILE))
}

/// The repository-controlled location earlier releases read. It is never
/// consulted now.
fn legacy_workspace_store(workspace: &Path) -> PathBuf {
    workspace.join(".a3s").join(APPROVALS_FILE)
}

fn workspace_key(workspace: &Path) -> String {
    std::fs::canonicalize(workspace)
        .unwrap_or_else(|_| workspace.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

type Store = BTreeMap<String, Vec<String>>;

fn render_store(store: &Store) -> String {
    let mut document = String::new();
    for (workspace, rules) in store {
        let rules = rules
            .iter()
            .map(|rule| format!("\"{rule}\""))
            .collect::<Vec<_>>()
            .join(", ");
        document.push_str(&format!(
            "workspace \"{workspace}\" {{\n  rules = [{rules}]\n}}\n"
        ));
    }
    document
}

fn parse_store(text: &str) -> anyhow::Result<Store> {
    let document = a3s_acl::parse_acl(text)
        .map_err(|error| anyhow::anyhow!("approvals store is not valid ACL: {error}"))?;
    let mut store = Store::new();
    for block in &document.blocks {
        if block.name != "workspace" {
            anyhow::bail!("approvals store has an unknown block `{}`", block.name);
        }
        let [workspace] = block.labels.as_slice() else {
            anyhow::bail!("approvals store workspace block needs exactly one label");
        };
        let Some(a3s_acl::ast::Value::List(items)) = block.attributes.get("rules") else {
            anyhow::bail!("approvals store workspace block needs a `rules` list");
        };
        let rules = items
            .iter()
            .map(|item| match item {
                a3s_acl::ast::Value::String(rule)
                    if learnable_rule(rule).as_ref() == Some(rule) =>
                {
                    Ok(rule.clone())
                }
                _ => Err(anyhow::anyhow!(
                    "approvals store holds a rule that is not learnable"
                )),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        store.insert(workspace.clone(), rules);
    }
    Ok(store)
}

fn read_store(path: &Path) -> anyhow::Result<Store> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Store::new()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() {
        anyhow::bail!("approvals store {} is not a regular file", path.display());
    }
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(MAX_STORE_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_STORE_BYTES {
        anyhow::bail!("approvals store {} is too large", path.display());
    }
    parse_store(&text)
}

fn warn_legacy_store_once(workspace: &Path) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let legacy = legacy_workspace_store(workspace);
    if legacy.exists() {
        WARNED.call_once(|| {
            tracing::warn!(
                path = %legacy.display(),
                "ignoring repository-controlled approvals; learned approvals now live in the user state root"
            );
        });
    }
}

/// The rules learned for one workspace. An unreadable store learns nothing.
pub fn load_rules(workspace: &Path) -> Vec<String> {
    match approvals_store_path(workspace) {
        Ok(store) => load_rules_in(&store, workspace),
        Err(error) => {
            tracing::warn!(%error, "learned approvals are unavailable; every command asks");
            Vec::new()
        }
    }
}

fn load_rules_in(store: &Path, workspace: &Path) -> Vec<String> {
    match read_store(store) {
        Ok(mut store) => store.remove(&workspace_key(workspace)).unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, "learned approvals are unavailable; every command asks");
            Vec::new()
        }
    }
}

/// Whether a tool call matches a rule the user chose "don't ask again" for.
pub fn is_previously_approved(workspace: &Path, tool_name: &str, args: &serde_json::Value) -> bool {
    if tool_name != "bash" {
        return false;
    }
    warn_legacy_store_once(workspace);
    command_rule(args).is_some_and(|rule| load_rules(workspace).contains(&rule))
}

fn command_rule(args: &serde_json::Value) -> Option<String> {
    args.get("command")
        .and_then(serde_json::Value::as_str)
        .and_then(learnable_rule)
}

/// Persist the rule of one command the user answered `allow-always` for.
/// Returns the learned rule; `Ok(None)` when it is not learnable or already
/// known.
pub fn record_always_allowed_bash(
    workspace: &Path,
    command: &str,
) -> anyhow::Result<Option<String>> {
    record_in(&approvals_store_path(workspace)?, workspace, command)
}

fn record_in(path: &Path, workspace: &Path, command: &str) -> anyhow::Result<Option<String>> {
    let Some(rule) = learnable_rule(command) else {
        return Ok(None);
    };
    let Some(parent) = path.parent() else {
        anyhow::bail!("approvals store {} has no parent", path.display());
    };
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(LOCK_FILE))?;
    lock.lock()?;
    let mut store = read_store(path)?;
    let rules = store.entry(workspace_key(workspace)).or_default();
    if rules.contains(&rule) {
        return Ok(None);
    }
    rules.push(rule.clone());
    let overflow = rules.len().saturating_sub(MAX_RULES_PER_WORKSPACE);
    rules.drain(..overflow);
    let document = render_store(&store);
    // A workspace path a3s-acl cannot round-trip is refused instead of
    // written as a store that would later fail to load.
    if parse_store(&document)? != store {
        anyhow::bail!("approvals store would not round-trip for this workspace path");
    }
    write_private(path, document.as_bytes())?;
    Ok(Some(rule))
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("acl.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
