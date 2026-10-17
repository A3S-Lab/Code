//! One shell-command classifier for every permission surface.
//!
//! Permission rules, the interactive guardrail, the verifier turn, and host
//! approval learning all read the same [`ShellAnalysis`]. The analysis comes
//! from a quote-aware parse of the command, never from substring matches on
//! the raw text, and it is recursive: command and process substitutions,
//! subshells, groups, function bodies, `sh -c` / `eval` / `env -S` strings,
//! and heredocs fed to a shell are classified as commands of their own.
//!
//! The classifier can only make a decision more conservative. [`ShellRisk`]
//! has no "allow" member: callers combine it with their own decision and may
//! only move toward Ask or Deny. A construct the parser cannot model exactly
//! is recorded as an [`OpaqueConstruct`], and an opaque command never matches
//! an allow rule and is never read-only.

mod lexer;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use lexer::{Connector, RawSegment, RedirectKind, Word};

/// Nested scripts deeper than this are not classified; the command is opaque.
const MAX_DEPTH: usize = 4;
/// Longer commands are opaque rather than lexed.
const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// The closed set of shell risk classes, ordered from least to most severe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShellRisk {
    /// Every command is on the read-only allowlist.
    ReadOnly,
    /// Ordinary side effects: builds, tests, edits, network fetches.
    #[default]
    Mutating,
    /// Syntax the parser cannot model exactly.
    Opaque,
    /// A network fetch piped into a shell or interpreter.
    RemoteCode,
    /// `sudo`, `doas`, `su`, `pkexec`, `run0`.
    Escalating,
    /// Forced recursive deletion of a broad or absolute target, a function
    /// definition, or a write to a device node.
    Destructive,
    /// Wiping the root, a home, or a top-level system directory; formatting
    /// or overwriting a disk; killing every process; powering off.
    Catastrophic,
}

impl ShellRisk {
    const ALL: [Self; 7] = [
        Self::ReadOnly,
        Self::Mutating,
        Self::Opaque,
        Self::RemoteCode,
        Self::Escalating,
        Self::Destructive,
        Self::Catastrophic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Mutating => "mutating",
            Self::Opaque => "opaque",
            Self::RemoteCode => "remote_code",
            Self::Escalating => "escalating",
            Self::Destructive => "destructive",
            Self::Catastrophic => "catastrophic",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|risk| risk.as_str() == value)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "shell risk must be one of the closed set [read_only, mutating, opaque, \
                     remote_code, escalating, destructive, catastrophic]; got {value:?}"
                )
            })
    }

    /// Denied by the interactive guardrail in every approval mode.
    pub const fn is_hard_denial(self) -> bool {
        matches!(
            self,
            Self::RemoteCode | Self::Escalating | Self::Destructive | Self::Catastrophic
        )
    }
}

/// The closed set of constructs the parser records instead of modelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpaqueConstruct {
    CommandSubstitution,
    ProcessSubstitution,
    ArithmeticExpansion,
    AnsiCQuote,
    Subshell,
    Group,
    FunctionDefinition,
    ControlFlow,
    Heredoc,
    Eval,
    DynamicProgram,
    UnterminatedQuote,
    UnbalancedNesting,
    NestingTooDeep,
    Oversized,
    Empty,
}

impl OpaqueConstruct {
    const ALL: [Self; 16] = [
        Self::CommandSubstitution,
        Self::ProcessSubstitution,
        Self::ArithmeticExpansion,
        Self::AnsiCQuote,
        Self::Subshell,
        Self::Group,
        Self::FunctionDefinition,
        Self::ControlFlow,
        Self::Heredoc,
        Self::Eval,
        Self::DynamicProgram,
        Self::UnterminatedQuote,
        Self::UnbalancedNesting,
        Self::NestingTooDeep,
        Self::Oversized,
        Self::Empty,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommandSubstitution => "command_substitution",
            Self::ProcessSubstitution => "process_substitution",
            Self::ArithmeticExpansion => "arithmetic_expansion",
            Self::AnsiCQuote => "ansi_c_quote",
            Self::Subshell => "subshell",
            Self::Group => "group",
            Self::FunctionDefinition => "function_definition",
            Self::ControlFlow => "control_flow",
            Self::Heredoc => "heredoc",
            Self::Eval => "eval",
            Self::DynamicProgram => "dynamic_program",
            Self::UnterminatedQuote => "unterminated_quote",
            Self::UnbalancedNesting => "unbalanced_nesting",
            Self::NestingTooDeep => "nesting_too_deep",
            Self::Oversized => "oversized",
            Self::Empty => "empty",
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|construct| construct.as_str() == value)
            .ok_or_else(|| {
                let names: Vec<&str> = Self::ALL.iter().map(|c| c.as_str()).collect();
                anyhow::anyhow!(
                    "opaque shell construct must be one of the closed set [{}]; got {value:?}",
                    names.join(", ")
                )
            })
    }
}

/// One simple command, at any nesting depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    depth: usize,
    literal: Vec<String>,
    effective: Vec<String>,
    plain_words: bool,
    wrapped: bool,
    has_assignments: bool,
    redirects: bool,
    writes_file: bool,
    piped_input: bool,
    risk: ShellRisk,
}

impl ShellCommand {
    /// Nesting depth: 0 for a top-level command.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The words as written after quote removal, joined by single spaces.
    pub fn literal_text(&self) -> String {
        self.literal.join(" ")
    }

    /// The command after reserved words, assignments, and wrappers such as
    /// `env`, `sudo`, `nice`, `timeout` and `xargs` are removed, with the
    /// program reduced to its basename.
    pub fn effective_text(&self) -> String {
        self.effective.join(" ")
    }

    pub fn argv(&self) -> &[String] {
        &self.effective
    }

    pub fn program(&self) -> Option<&str> {
        self.effective.first().map(String::as_str)
    }

    pub fn risk(&self) -> ShellRisk {
        self.risk
    }

    /// Every word is a literal: no quoting, expansion, or glob.
    pub fn has_plain_words(&self) -> bool {
        self.plain_words
    }

    /// Ran through a wrapper (`env`, `nice`, `sudo`, ...).
    pub fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    pub fn has_assignments(&self) -> bool {
        self.has_assignments
    }

    pub fn has_redirects(&self) -> bool {
        self.redirects
    }

    /// Redirects output into a file (not a descriptor or `/dev/null`).
    pub fn writes_file(&self) -> bool {
        self.writes_file
    }

    /// Reads the previous command's output through a pipe.
    pub fn is_piped(&self) -> bool {
        self.piped_input
    }
}

/// The classification of a whole command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAnalysis {
    commands: Vec<ShellCommand>,
    opaque: BTreeSet<OpaqueConstruct>,
    only_pipes: bool,
    risk: ShellRisk,
}

impl ShellAnalysis {
    /// The most severe risk of any command or construct in the line.
    pub fn risk(&self) -> ShellRisk {
        self.risk
    }

    /// Every simple command, top-level and nested, in source order.
    pub fn commands(&self) -> &[ShellCommand] {
        &self.commands
    }

    pub fn opaque_constructs(&self) -> impl Iterator<Item = OpaqueConstruct> + '_ {
        self.opaque.iter().copied()
    }

    pub fn is_opaque(&self) -> bool {
        !self.opaque.is_empty()
    }

    /// Top-level commands are joined only by pipes (no `;`, `&&`, `||`, `&`).
    pub fn has_only_pipes(&self) -> bool {
        self.only_pipes
    }

    /// Exactly one top-level command, no nesting, no opaque construct, no
    /// wrapper, assignment, redirect, quoting, or expansion.
    pub fn is_single_plain_command(&self) -> bool {
        matches!(self.commands.as_slice(), [command]
            if command.depth == 0
                && command.plain_words
                && !command.wrapped
                && !command.has_assignments
                && !command.redirects)
            && self.opaque.is_empty()
    }

    /// Whether the line can write files: a file redirect, a file-mutating
    /// program, a mutating Git subcommand, a hard denial, or a construct the
    /// parser could not see into.
    pub fn writes_files(&self) -> bool {
        self.risk >= ShellRisk::RemoteCode
            || self.opaque.contains(&OpaqueConstruct::Eval)
            || self.opaque.contains(&OpaqueConstruct::DynamicProgram)
            || self.opaque.contains(&OpaqueConstruct::UnterminatedQuote)
            || self.opaque.contains(&OpaqueConstruct::NestingTooDeep)
            || self.opaque.contains(&OpaqueConstruct::Oversized)
            || self
                .commands
                .iter()
                .any(|command| command.writes_file || program_mutates_files(&command.effective))
    }
}

/// Classify a shell command line.
pub fn analyze(command: &str) -> ShellAnalysis {
    let mut acc = Accumulator::default();
    if command.len() > MAX_SOURCE_BYTES {
        acc.opaque.insert(OpaqueConstruct::Oversized);
    } else {
        acc.only_pipes = analyze_into(command, 0, &mut acc);
    }
    if acc.commands.is_empty() {
        acc.opaque.insert(OpaqueConstruct::Empty);
    }
    let mut risk = acc
        .commands
        .iter()
        .map(|command| command.risk)
        .max()
        .unwrap_or(ShellRisk::Opaque);
    if !acc.opaque.is_empty() {
        risk = risk.max(ShellRisk::Opaque);
    }
    if acc.opaque.contains(&OpaqueConstruct::FunctionDefinition) {
        risk = risk.max(ShellRisk::Destructive);
    }
    ShellAnalysis {
        commands: acc.commands,
        opaque: acc.opaque,
        only_pipes: acc.only_pipes,
        risk,
    }
}

#[derive(Default)]
struct Accumulator {
    commands: Vec<ShellCommand>,
    opaque: BTreeSet<OpaqueConstruct>,
    only_pipes: bool,
}

/// Returns whether the top-level connectors were all pipes.
fn analyze_into(source: &str, depth: usize, acc: &mut Accumulator) -> bool {
    if depth > MAX_DEPTH {
        acc.opaque.insert(OpaqueConstruct::NestingTooDeep);
        return false;
    }
    let lexed = lexer::lex(source);
    acc.opaque.extend(lexed.opaque.iter().copied());
    let only_pipes = lexed
        .segments
        .iter()
        .all(|segment| matches!(segment.connector, Connector::Start | Connector::Pipe));
    let mut pipeline_fetches = false;
    for segment in &lexed.segments {
        if segment.connector != Connector::Pipe {
            pipeline_fetches = false;
        }
        let flow = classify_segment(segment, depth, pipeline_fetches, acc);
        pipeline_fetches |= flow.fetches;
    }
    for nested in &lexed.nested {
        analyze_into(nested, depth + 1, acc);
    }
    only_pipes
}

#[derive(Default)]
struct Flow {
    fetches: bool,
    executes_stdin: bool,
}

fn classify_segment(
    segment: &RawSegment,
    depth: usize,
    pipeline_fetches: bool,
    acc: &mut Accumulator,
) -> Flow {
    let words = &segment.words;
    let mut index = 0;
    while words
        .get(index)
        .is_some_and(|word| !word.quoted && lexer::is_reserved_prefix(&word.text))
    {
        if !matches!(words[index].text.as_str(), "!" | "time") {
            acc.opaque.insert(OpaqueConstruct::ControlFlow);
        }
        index += 1;
    }
    if let Some(word) = words.get(index).filter(|word| !word.quoted) {
        match word.text.as_str() {
            "for" | "case" | "select" | "[[" | "coproc" => {
                acc.opaque.insert(OpaqueConstruct::ControlFlow);
                index = words.len();
            }
            "function" => {
                acc.opaque.insert(OpaqueConstruct::FunctionDefinition);
                index = words.len();
            }
            "fi" | "done" | "esac" | "}" | "]]" | "in" => index = words.len(),
            _ => {}
        }
    }
    let assignments_start = index;
    while words.get(index).is_some_and(|word| word.assignment) {
        index += 1;
    }
    let has_assignments = index > assignments_start;

    let mut command = Classified {
        risk: ShellRisk::Mutating,
        ..Classified::default()
    };
    let mut flow = Flow::default();
    let program_start = unwrap_wrappers(words, index, depth, &mut command, acc);
    let effective = &words[program_start.min(words.len())..];
    if let Some((program_word, args)) = effective.split_first() {
        if program_word.dynamic {
            acc.opaque.insert(OpaqueConstruct::DynamicProgram);
            command.risk = ShellRisk::Opaque;
        } else {
            classify_program(
                basename(&program_word.text),
                program_word,
                args,
                depth,
                &mut command,
                &mut flow,
                acc,
            );
        }
    } else if command.escalated {
        command.risk = ShellRisk::Escalating;
    } else if command.lookup {
        command.risk = ShellRisk::ReadOnly;
    }
    if command.escalated {
        command.risk = command.risk.max(ShellRisk::Escalating);
    }
    if flow.executes_stdin && pipeline_fetches && segment.connector == Connector::Pipe {
        command.risk = command.risk.max(ShellRisk::RemoteCode);
    }

    let mut writes_file = command.writes_file;
    for redirect in &segment.redirects {
        match redirect.kind {
            RedirectKind::Output | RedirectKind::Append | RedirectKind::ReadWrite => {
                let target = &redirect.target.text;
                if redirect.target.dynamic || !is_harmless_device(target) {
                    writes_file = true;
                }
                if is_block_device(target) {
                    command.risk = ShellRisk::Catastrophic;
                } else if target.starts_with("/dev/") && !is_harmless_device(target) {
                    command.risk = command.risk.max(ShellRisk::Destructive);
                }
            }
            RedirectKind::Heredoc | RedirectKind::HereString if flow.executes_stdin => {
                if let Some(body) = &redirect.body {
                    analyze_into(body, depth + 1, acc);
                }
            }
            _ => {}
        }
    }

    if words.is_empty() && segment.redirects.is_empty() {
        return flow;
    }
    // `LD_PRELOAD=x cat` runs code and `cat a > b` writes: neither is read-only.
    if writes_file || has_assignments {
        command.risk = command.risk.max(ShellRisk::Mutating);
    }
    let mut effective_text: Vec<String> = effective.iter().map(|word| word.text.clone()).collect();
    if let Some(program) = effective_text.first_mut() {
        *program = basename(program).to_string();
    }
    let plain_words = words
        .iter()
        .all(|word| !word.quoted && !word.dynamic && !word.glob);
    acc.commands.push(ShellCommand {
        depth,
        literal: words.iter().map(|word| word.text.clone()).collect(),
        effective: effective_text,
        plain_words,
        wrapped: command.wrapped,
        has_assignments,
        redirects: !segment.redirects.is_empty(),
        writes_file,
        piped_input: segment.connector == Connector::Pipe,
        risk: command.risk,
    });
    flow
}

#[derive(Default)]
struct Classified {
    risk: ShellRisk,
    escalated: bool,
    wrapped: bool,
    from_xargs: bool,
    lookup: bool,
    writes_file: bool,
}

/// Skip wrapper programs and their options, returning the program index.
fn unwrap_wrappers(
    words: &[Word],
    mut index: usize,
    depth: usize,
    command: &mut Classified,
    acc: &mut Accumulator,
) -> usize {
    for _ in 0..8 {
        let Some(word) = words.get(index) else {
            return index;
        };
        if word.dynamic {
            return index;
        }
        let next = index + 1;
        index = match basename(&word.text) {
            "sudo" => skip_options(
                words,
                next,
                &["-u", "-g", "-h", "-p", "-C", "-U", "-r", "-t", "-T", "-D"],
            ),
            "doas" => skip_options(words, next, &["-u", "-C"]),
            "pkexec" => skip_options(words, next, &["--user"]),
            "run0" => skip_options(words, next, &["-u", "--user", "-D", "--chdir"]),
            "env" => skip_env(words, next, depth, acc),
            "nohup" | "builtin" | "unbuffer" | "chronic" => next,
            "exec" => skip_options(words, next, &["-a"]),
            "command" => {
                if words
                    .get(next)
                    .is_some_and(|w| matches!(w.text.as_str(), "-v" | "-V"))
                {
                    command.lookup = true;
                    return words.len();
                }
                skip_options(words, next, &[])
            }
            "nice" => skip_options(words, next, &["-n", "--adjustment"]),
            "ionice" => skip_options(words, next, &["-c", "-n", "-p", "-t", "-P", "-u"]),
            "time" => {
                if words[next.min(words.len())..]
                    .iter()
                    .take_while(|w| w.text.starts_with('-'))
                    .any(|w| {
                        matches!(w.text.as_str(), "-o" | "--output")
                            || w.text.starts_with("--output=")
                    })
                {
                    command.writes_file = true;
                }
                skip_options(words, next, &["-f", "-o", "--format", "--output"])
            }
            "timeout" => {
                let after = skip_options(words, next, &["-s", "-k", "--signal", "--kill-after"]);
                after + 1
            }
            "stdbuf" => skip_options(words, next, &["-i", "-o", "-e"]),
            "caffeinate" => skip_options(words, next, &["-w", "-t"]),
            "watch" => skip_options(
                words,
                next,
                &["-n", "--interval", "-q", "--equexit", "--differences"],
            ),
            "xargs" => {
                command.from_xargs = true;
                skip_options(
                    words,
                    next,
                    &[
                        "-a",
                        "-d",
                        "-E",
                        "-e",
                        "-I",
                        "-i",
                        "-L",
                        "-l",
                        "-n",
                        "-P",
                        "-s",
                        "--arg-file",
                        "--delimiter",
                        "--max-args",
                        "--max-procs",
                        "--max-lines",
                        "--replace",
                    ],
                )
            }
            _ => return index,
        };
        match basename(&word.text) {
            "sudo" | "doas" | "pkexec" | "run0" => command.escalated = true,
            _ => {}
        }
        command.wrapped = true;
    }
    acc.opaque.insert(OpaqueConstruct::NestingTooDeep);
    words.len()
}

fn skip_options(words: &[Word], mut index: usize, takes_value: &[&str]) -> usize {
    while let Some(word) = words.get(index) {
        let text = word.text.as_str();
        if text == "--" {
            return index + 1;
        }
        if text.len() > 1 && text.starts_with('-') {
            index += if takes_value.contains(&text) { 2 } else { 1 };
            continue;
        }
        break;
    }
    index
}

fn skip_env(words: &[Word], mut index: usize, depth: usize, acc: &mut Accumulator) -> usize {
    while let Some(word) = words.get(index) {
        let text = word.text.as_str();
        if text == "--" {
            index += 1;
            break;
        }
        let split = if matches!(text, "-S" | "--split-string") {
            index += 1;
            words.get(index)
        } else if let Some(rest) = text.strip_prefix("--split-string=") {
            analyze_nested_text(rest, word.dynamic, depth, acc);
            return words.len();
        } else {
            None
        };
        if let Some(script) = split {
            analyze_nested_text(&script.text, script.dynamic, depth, acc);
            return words.len();
        }
        if matches!(text, "-u" | "--unset" | "-C" | "--chdir" | "-P") {
            index += 2;
        } else if text.len() > 1 && text.starts_with('-') || word.assignment {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn analyze_nested_text(text: &str, dynamic: bool, depth: usize, acc: &mut Accumulator) {
    if dynamic {
        acc.opaque.insert(OpaqueConstruct::DynamicProgram);
    }
    analyze_into(text, depth + 1, acc);
}

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "mksh", "ash", "fish", "busybox",
];
const FETCHERS: &[&str] = &["curl", "wget", "fetch", "aria2c", "http", "https", "xh"];
/// Top-level directories whose recursive removal destroys the host.
const SYSTEM_DIRS: &[&str] = &[
    "Applications",
    "Library",
    "System",
    "Users",
    "Volumes",
    "bin",
    "boot",
    "cores",
    "dev",
    "etc",
    "home",
    "lib",
    "lib32",
    "lib64",
    "opt",
    "private",
    "proc",
    "root",
    "run",
    "sbin",
    "srv",
    "sys",
    "usr",
    "var",
];

fn is_interpreter(program: &str) -> bool {
    matches!(
        program,
        "perl" | "ruby" | "node" | "php" | "lua" | "deno" | "Rscript" | "tclsh"
    ) || program.starts_with("python")
}

fn classify_program(
    program: &str,
    program_word: &Word,
    args: &[Word],
    depth: usize,
    command: &mut Classified,
    flow: &mut Flow,
    acc: &mut Accumulator,
) {
    let texts: Vec<&str> = args.iter().map(|word| word.text.as_str()).collect();
    command.risk = match program {
        _ if SHELLS.contains(&program) => {
            let args = if program == "busybox" {
                match args.split_first() {
                    Some((applet, rest)) if SHELLS.contains(&applet.text.as_str()) => rest,
                    _ => return,
                }
            } else {
                args
            };
            classify_shell_invocation(args, depth, flow, acc);
            ShellRisk::Mutating
        }
        _ if is_interpreter(program) => {
            let inline = texts
                .iter()
                .any(|t| matches!(*t, "-c" | "-e" | "-E" | "-r" | "--eval"));
            let positional = texts.iter().any(|t| !t.starts_with('-'));
            flow.executes_stdin = !inline && (!positional || texts.contains(&"-"));
            ShellRisk::Mutating
        }
        "eval" => {
            acc.opaque.insert(OpaqueConstruct::Eval);
            let dynamic = args.iter().any(|word| word.dynamic);
            analyze_nested_text(&texts.join(" "), dynamic, depth, acc);
            ShellRisk::Mutating
        }
        "su" => {
            command.escalated = true;
            if let Some(position) = texts.iter().position(|t| matches!(*t, "-c" | "--command")) {
                if let Some(script) = args.get(position + 1) {
                    analyze_nested_text(&script.text, script.dynamic, depth, acc);
                }
            }
            ShellRisk::Escalating
        }
        _ if FETCHERS.contains(&program) => {
            flow.fetches = true;
            ShellRisk::Mutating
        }
        "rm" => rm_risk(args, command.from_xargs),
        "chmod" | "chown" | "chgrp" => {
            let recursive = texts
                .iter()
                .any(|t| *t == "--recursive" || short_flag(t, 'R'));
            if recursive
                && args
                    .iter()
                    .any(|w| !w.text.starts_with('-') && is_root_target(&w.text))
            {
                ShellRisk::Catastrophic
            } else {
                ShellRisk::Mutating
            }
        }
        _ if program == "mkfs"
            || program.starts_with("mkfs.")
            || program.starts_with("newfs")
            || matches!(program, "mke2fs" | "wipefs") =>
        {
            ShellRisk::Catastrophic
        }
        "dd" => texts
            .iter()
            .filter_map(|t| t.strip_prefix("of="))
            .map(|target| {
                if is_block_device(target) {
                    ShellRisk::Catastrophic
                } else if target.starts_with("/dev/") {
                    ShellRisk::Destructive
                } else {
                    ShellRisk::Mutating
                }
            })
            .max()
            .unwrap_or(ShellRisk::Mutating),
        "diskutil" => {
            let erases = texts.iter().any(|t| {
                let lower = t.to_ascii_lowercase();
                lower.starts_with("erase")
                    || matches!(
                        lower.as_str(),
                        "zerodisk" | "randomdisk" | "secureerase" | "partitiondisk" | "reformat"
                    )
            });
            if erases {
                ShellRisk::Catastrophic
            } else {
                ShellRisk::Mutating
            }
        }
        "kill" if kills_every_process(&texts) => ShellRisk::Catastrophic,
        "shutdown" | "reboot" | "halt" | "poweroff" => ShellRisk::Catastrophic,
        "init" | "telinit" if texts.iter().any(|t| matches!(*t, "0" | "6")) => {
            ShellRisk::Catastrophic
        }
        "systemctl"
            if texts
                .iter()
                .any(|t| matches!(*t, "poweroff" | "reboot" | "halt" | "kexec")) =>
        {
            ShellRisk::Catastrophic
        }
        _ if !command.wrapped
            && program == program_word.text
            && read_only_program(program, &texts) =>
        {
            ShellRisk::ReadOnly
        }
        _ => ShellRisk::Mutating,
    };
}

fn classify_shell_invocation(args: &[Word], depth: usize, flow: &mut Flow, acc: &mut Accumulator) {
    let mut inline = false;
    let mut reads_stdin = false;
    let mut index = 0;
    while let Some(word) = args.get(index) {
        let text = word.text.as_str();
        if text == "--" || text == "-" {
            index += 1;
            break;
        }
        if matches!(text, "-o" | "+o" | "--rcfile" | "--init-file") {
            index += 2;
            continue;
        }
        if text.len() > 1 && (text.starts_with('-') || text.starts_with('+')) {
            if !text.starts_with("--") {
                inline |= text.contains('c');
                reads_stdin |= text.contains('s');
            } else if text == "--command" {
                inline = true;
            }
            index += 1;
            continue;
        }
        break;
    }
    match args.get(index) {
        Some(script) if inline => analyze_nested_text(&script.text, script.dynamic, depth, acc),
        None if inline => {
            acc.opaque.insert(OpaqueConstruct::UnbalancedNesting);
        }
        Some(_) if !reads_stdin => {}
        _ => flow.executes_stdin = true,
    }
}

fn rm_risk(args: &[Word], from_xargs: bool) -> ShellRisk {
    let mut recursive = false;
    let mut force = false;
    let mut targets = Vec::new();
    let mut options_done = false;
    for word in args {
        let text = word.text.as_str();
        if !options_done && text == "--" {
            options_done = true;
        } else if !options_done && text.starts_with("--") {
            recursive |= text == "--recursive";
            force |= text == "--force";
        } else if !options_done && text.len() > 1 && text.starts_with('-') && !word.quoted {
            recursive |= short_flag(text, 'r') || short_flag(text, 'R');
            force |= short_flag(text, 'f');
        } else {
            targets.push(word);
        }
    }
    if recursive && targets.iter().any(|word| is_root_target(&word.text)) {
        return ShellRisk::Catastrophic;
    }
    let broad = targets.iter().any(|word| {
        let text = word.text.as_str();
        word.dynamic
            || text.starts_with('/')
            || text.starts_with('~')
            || text.starts_with('*')
            || matches!(text, "." | ".." | "./" | "../")
    });
    if recursive && force && (broad || from_xargs) {
        ShellRisk::Destructive
    } else {
        ShellRisk::Mutating
    }
}

fn short_flag(text: &str, flag: char) -> bool {
    text.starts_with('-') && !text.starts_with("--") && text[1..].contains(flag)
}

/// The root, a home directory, or a top-level system directory, after
/// lexical `.`/`..`/`*` normalization.
fn is_root_target(text: &str) -> bool {
    let (home, rest) = if let Some(rest) = strip_home(text) {
        (true, rest)
    } else if text.starts_with('/') {
        (false, text)
    } else {
        return false;
    };
    let mut components: Vec<&str> = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." | "*" => {}
            ".." => {
                if components.pop().is_none() && home {
                    return true;
                }
            }
            other => components.push(other),
        }
    }
    match components.as_slice() {
        [] => true,
        [single] if !home => SYSTEM_DIRS.contains(single),
        _ => false,
    }
}

fn strip_home(text: &str) -> Option<&str> {
    for prefix in ["$HOME", "${HOME}"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with('/') {
                return Some(rest);
            }
        }
    }
    let rest = text.strip_prefix('~')?;
    Some(rest.find('/').map_or("", |slash| &rest[slash..]))
}

fn kills_every_process(args: &[&str]) -> bool {
    let mut index = 0;
    match args.first() {
        Some(&("-s" | "-n")) => index = 2,
        Some(first) if first.starts_with('-') && args.len() > 1 && *first != "--" => index = 1,
        _ => {}
    }
    if args.get(index) == Some(&"--") {
        index += 1;
    }
    args[index.min(args.len())..].contains(&"-1")
}

fn is_harmless_device(path: &str) -> bool {
    matches!(
        path,
        "/dev/null"
            | "/dev/stdout"
            | "/dev/stderr"
            | "/dev/tty"
            | "/dev/zero"
            | "/dev/random"
            | "/dev/urandom"
    ) || path.starts_with("/dev/fd/")
}

fn is_block_device(path: &str) -> bool {
    path.strip_prefix("/dev/").is_some_and(|device| {
        [
            "sd", "hd", "vd", "xvd", "nvme", "disk", "rdisk", "mmcblk", "md", "dm-", "mapper/",
            "loop",
        ]
        .iter()
        .any(|prefix| device.starts_with(prefix))
    })
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

fn program_mutates_files(argv: &[String]) -> bool {
    let Some((program, args)) = argv.split_first() else {
        return false;
    };
    match program.as_str() {
        "rm" | "rmdir" | "unlink" | "mv" | "cp" | "tee" | "dd" | "truncate" | "shred" | "ln"
        | "install" | "chmod" | "chown" | "chgrp" | "touch" | "mkdir" | "rsync" | "patch" => true,
        "sed" | "perl" => args
            .iter()
            .any(|arg| arg.starts_with("-i") || arg.starts_with("--in-place")),
        "git" => git_subcommand(args).is_some_and(|sub| {
            matches!(
                sub,
                "add"
                    | "am"
                    | "apply"
                    | "checkout"
                    | "cherry-pick"
                    | "clean"
                    | "commit"
                    | "merge"
                    | "mv"
                    | "pull"
                    | "push"
                    | "rebase"
                    | "reset"
                    | "restore"
                    | "revert"
                    | "rm"
                    | "stash"
                    | "switch"
                    | "tag"
            )
        }),
        _ => false,
    }
}

fn git_subcommand(args: &[String]) -> Option<&str> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => index += 2,
            value if value.starts_with('-') => index += 1,
            value => return Some(value),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Read-only allowlist. A program is read-only only when invoked by its bare
// name, without a wrapper, and with no option that executes or writes.
// ---------------------------------------------------------------------------

pub(super) fn read_only_program(program: &str, args: &[&str]) -> bool {
    let all = |check: &dyn Fn(&str) -> bool| args.iter().all(|value| check(value));
    match program {
        "pwd" | "cat" | "head" | "tail" | "wc" | "stat" | "file" | "cut" | "tr" | "whoami"
        | "df" | "uname" => all(&|value| !option_executes_or_writes(value)),
        "ls" => all(&|value| {
            !option_executes_or_writes(value)
                && !short_option_contains(value, 'L')
                && !short_option_contains(value, 'R')
                && !matches!(value, "--dereference" | "--recursive")
        }),
        "rg" => all(&|value| {
            !option_executes_or_writes(value)
                && !matches!(value, "--pre" | "--hostname-bin" | "-L" | "--follow")
                && !value.starts_with("--pre=")
                && !value.starts_with("--hostname-bin=")
        }),
        "grep" => all(&|value| {
            !option_executes_or_writes(value)
                && !short_option_contains(value, 'f')
                && !matches!(
                    value,
                    "-R" | "-r"
                        | "--recursive"
                        | "--dereference-recursive"
                        | "--include"
                        | "--exclude-from"
                        | "--file"
                )
                && !value.starts_with("--exclude-from=")
                && !value.starts_with("--file=")
        }),
        "du" => all(&|value| {
            !short_option_contains(value, 'L')
                && value != "--dereference"
                && !option_executes_or_writes(value)
        }),
        "date" => all(&|value| {
            !matches!(value, "-s" | "--set")
                && !short_option_contains(value, 's')
                && !value.starts_with("--set=")
                && !option_executes_or_writes(value)
        }),
        "sort" => all(&|value| {
            !matches!(
                value,
                "-o" | "-T" | "--output" | "--temporary-directory" | "--compress-program"
            ) && !short_option_contains(value, 'o')
                && !short_option_contains(value, 'T')
                && !value.starts_with("--output=")
                && !value.starts_with("--temporary-directory=")
                && !value.starts_with("--compress-program=")
                && !option_executes_or_writes(value)
        }),
        // uniq writes when a second positional operand is present.
        "uniq" => args.iter().filter(|value| !value.starts_with('-')).count() <= 1,
        // Builtins can carry surprising option semantics, so options ask.
        "printf" | "echo" => all(&|value| !value.starts_with('-')),
        "find" => !args.iter().any(|value| {
            matches!(
                *value,
                "-L" | "-H"
                    | "-delete"
                    | "-exec"
                    | "-execdir"
                    | "-ok"
                    | "-okdir"
                    | "-fprint"
                    | "-fprint0"
                    | "-fprintf"
                    | "-fls"
                    | "-follow"
                    | "-lname"
            )
        }),
        // Sed scripts can write (`w`) or execute (`e`) without an option.
        "sed" => false,
        "git" => read_only_git(args),
        _ => false,
    }
}

fn short_option_contains(value: &str, flag: char) -> bool {
    value.starts_with('-') && !value.starts_with("--") && value[1..].contains(flag)
}

fn option_executes_or_writes(value: &str) -> bool {
    matches!(
        value,
        "--output" | "--exec" | "--command" | "--config" | "--files-from"
    ) || [
        "--output=",
        "--exec=",
        "--command=",
        "--config=",
        "--files-from=",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
}

pub(super) fn read_only_git(args: &[&str]) -> bool {
    let mut index = 0;
    while let Some(value) = args.get(index) {
        match *value {
            "--no-pager" | "-P" | "--no-optional-locks" => index += 1,
            // `-C` moves the filesystem boundary; other global options
            // (config, exec-path) stay behind confirmation.
            value if value.starts_with('-') => return false,
            _ => break,
        }
    }
    let Some(subcommand) = args.get(index).copied() else {
        return false;
    };
    let rest = &args[index + 1..];
    if rest.iter().any(|value| {
        matches!(
            *value,
            "--ext-diff" | "--textconv" | "--exec-path" | "--config-env"
        ) || value.starts_with("--exec-path=")
            || value.starts_with("--config-env=")
    }) {
        return false;
    }
    match subcommand {
        "status" | "diff" | "log" | "show" | "blame" | "grep" | "ls-files" | "rev-parse" => {
            !rest.iter().any(|value| {
                option_executes_or_writes(value)
                    || matches!(*value, "--paginate" | "-p" | "--ext-diff" | "--textconv")
                    || value.starts_with("--format=") && value.contains("%(rest)")
            })
        }
        "remote" => rest
            .first()
            .is_none_or(|value| matches!(*value, "-v" | "show")),
        "branch" => rest.iter().all(|value| {
            matches!(
                *value,
                "--all" | "-a" | "--list" | "--show-current" | "--verbose" | "-v" | "-vv"
            )
        }),
        _ => false,
    }
}
