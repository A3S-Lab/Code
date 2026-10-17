//! Quote-aware tokenizer for the shell subset the classifier models.
//!
//! The lexer never executes or expands anything. Constructs it cannot model
//! exactly are recorded as [`OpaqueConstruct`]s; the text of every nested
//! script it can locate (substitutions, subshells, groups, function bodies,
//! heredoc bodies) is returned so the analyzer can classify it as well.

use std::collections::BTreeSet;

use super::OpaqueConstruct;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Word {
    /// Text after quote removal. Expansions keep their source spelling.
    pub(super) text: String,
    pub(super) quoted: bool,
    /// Contains a parameter, command, arithmetic, or ANSI-C expansion.
    pub(super) dynamic: bool,
    /// Contains an unquoted glob or brace-expansion character.
    pub(super) glob: bool,
    /// Starts with an unquoted `NAME=` / `NAME+=` assignment prefix.
    pub(super) assignment: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Connector {
    Start,
    Sequence,
    And,
    Or,
    Pipe,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RedirectKind {
    Input,
    Output,
    Append,
    ReadWrite,
    DupOutput,
    DupInput,
    HereString,
    Heredoc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Redirect {
    pub(super) kind: RedirectKind,
    pub(super) target: Word,
    /// Heredoc body, or here-string text.
    pub(super) body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawSegment {
    pub(super) connector: Connector,
    pub(super) words: Vec<Word>,
    pub(super) redirects: Vec<Redirect>,
}

#[derive(Debug, Default)]
pub(super) struct Lexed {
    pub(super) segments: Vec<RawSegment>,
    pub(super) nested: Vec<String>,
    pub(super) opaque: BTreeSet<OpaqueConstruct>,
}

struct PendingHeredoc {
    segment: usize,
    redirect: usize,
    delimiter: String,
    strip_tabs: bool,
    expands: bool,
}

pub(super) fn lex(source: &str) -> Lexed {
    let mut lexer = Lexer {
        chars: source.chars().collect(),
        index: 0,
        out: Lexed::default(),
        current: RawSegment {
            connector: Connector::Start,
            words: Vec::new(),
            redirects: Vec::new(),
        },
        pending: Vec::new(),
    };
    lexer.run();
    lexer.out
}

struct Lexer {
    chars: Vec<char>,
    index: usize,
    out: Lexed,
    current: RawSegment,
    pending: Vec<PendingHeredoc>,
}

impl Lexer {
    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.index + offset).copied()
    }

    fn flag(&mut self, construct: OpaqueConstruct) {
        self.out.opaque.insert(construct);
    }

    fn at_command_start(&self) -> bool {
        self.current.redirects.is_empty()
            && self
                .current
                .words
                .iter()
                .all(|word| !word.quoted && is_reserved_prefix(&word.text))
    }

    /// `function NAME` with the body still to come.
    fn after_function_keyword(&self) -> bool {
        matches!(self.current.words.as_slice(), [keyword, _] if keyword.text == "function")
    }

    /// A single plain word can name a function (`f`, `:`, `do-it`).
    fn names_function(&self) -> bool {
        match self.current.words.as_slice() {
            [name] => !name.quoted && !name.assignment && !name.dynamic && !name.text.is_empty(),
            [_, _] => self.after_function_keyword(),
            _ => false,
        }
    }

    fn function_body(&mut self) {
        self.flag(OpaqueConstruct::FunctionDefinition);
        self.current.words.clear();
        self.skip_blank();
        match self.peek(0) {
            Some('{') => {
                self.index += 1;
                self.read_balanced_into_nested('{', '}');
            }
            Some('(') => {
                self.index += 1;
                self.read_balanced_into_nested('(', ')');
            }
            _ => self.flag(OpaqueConstruct::UnbalancedNesting),
        }
    }

    fn finish_segment(&mut self, next: Connector) {
        let segment = std::mem::replace(
            &mut self.current,
            RawSegment {
                connector: next,
                words: Vec::new(),
                redirects: Vec::new(),
            },
        );
        if !segment.words.is_empty() || !segment.redirects.is_empty() {
            self.out.segments.push(segment);
        } else if !matches!(segment.connector, Connector::Start | Connector::Sequence) {
            // `a && && b` and friends are syntax errors; keep the connector.
            self.current.connector = segment.connector;
        }
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' => self.index += 1,
                '\r' => self.index += 1,
                '\n' => {
                    self.index += 1;
                    self.finish_segment(Connector::Sequence);
                    self.read_pending_heredocs();
                }
                '#' if self.at_word_boundary() => self.skip_comment(),
                '\\' if self.peek(1) == Some('\n') => self.index += 2,
                ';' => {
                    if self.peek(1) == Some(';') {
                        self.flag(OpaqueConstruct::ControlFlow);
                        self.index += 2;
                    } else {
                        self.index += 1;
                    }
                    self.finish_segment(Connector::Sequence);
                }
                '&' => match self.peek(1) {
                    Some('&') => {
                        self.index += 2;
                        self.finish_segment(Connector::And);
                    }
                    Some('>') => {
                        self.index += 2;
                        let kind = if self.peek(0) == Some('>') {
                            self.index += 1;
                            RedirectKind::Append
                        } else {
                            RedirectKind::Output
                        };
                        self.push_redirect(kind);
                    }
                    _ => {
                        self.index += 1;
                        self.finish_segment(Connector::Background);
                    }
                },
                '|' => match self.peek(1) {
                    Some('|') => {
                        self.index += 2;
                        self.finish_segment(Connector::Or);
                    }
                    Some('&') => {
                        self.index += 2;
                        self.finish_segment(Connector::Pipe);
                    }
                    _ => {
                        self.index += 1;
                        self.finish_segment(Connector::Pipe);
                    }
                },
                '<' | '>' if self.peek(1) == Some('(') => self.process_substitution(),
                '<' | '>' => self.redirect(),
                '0'..='9' if self.fd_redirect_ahead() => {
                    while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                        self.index += 1;
                    }
                    self.redirect();
                }
                '(' => self.open_paren(),
                ')' => {
                    self.flag(OpaqueConstruct::UnbalancedNesting);
                    self.index += 1;
                    self.finish_segment(Connector::Sequence);
                }
                '{' if self.peek(1).is_none_or(char::is_whitespace)
                    && self.after_function_keyword() =>
                {
                    self.function_body();
                }
                '{' if self.at_command_start() && self.peek(1).is_none_or(char::is_whitespace) => {
                    self.index += 1;
                    self.flag(OpaqueConstruct::Group);
                    self.read_balanced_into_nested('{', '}');
                }
                '}' if self.at_command_start() => {
                    self.flag(OpaqueConstruct::UnbalancedNesting);
                    self.index += 1;
                }
                _ => {
                    let word = self.read_word();
                    self.current.words.push(word);
                }
            }
        }
        self.finish_segment(Connector::Sequence);
        if !self.pending.is_empty() {
            self.flag(OpaqueConstruct::UnterminatedQuote);
            self.pending.clear();
        }
    }

    fn at_word_boundary(&self) -> bool {
        self.index == 0
            || self
                .chars
                .get(self.index - 1)
                .is_some_and(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')'))
    }

    fn skip_comment(&mut self) {
        while let Some(c) = self.peek(0) {
            if c == '\n' {
                break;
            }
            self.index += 1;
        }
    }

    fn fd_redirect_ahead(&self) -> bool {
        if !self.at_word_boundary() {
            return false;
        }
        let mut offset = 0;
        while self.peek(offset).is_some_and(|c| c.is_ascii_digit()) {
            offset += 1;
        }
        matches!(self.peek(offset), Some('<' | '>')) && self.peek(offset + 1) != Some('(')
    }

    fn open_paren(&mut self) {
        if self.names_function() && self.next_non_space_after(1) == Some(')') {
            self.index += 1;
            while self.peek(0).is_some_and(|c| c != ')') {
                self.index += 1;
            }
            self.index += 1;
            self.function_body();
            return;
        }
        if self.at_command_start() {
            self.index += 1;
            if self.peek(0) == Some('(') {
                self.flag(OpaqueConstruct::ArithmeticExpansion);
                self.index += 1;
                self.read_balanced_text('(', ')');
                self.skip_char(')');
                self.skip_char(')');
            } else {
                self.flag(OpaqueConstruct::Subshell);
                self.read_balanced_into_nested('(', ')');
            }
            return;
        }
        self.flag(OpaqueConstruct::UnbalancedNesting);
        self.index += 1;
    }

    fn next_non_space_after(&self, mut offset: usize) -> Option<char> {
        while self.peek(offset).is_some_and(|c| c == ' ' || c == '\t') {
            offset += 1;
        }
        self.peek(offset)
    }

    fn skip_blank(&mut self) {
        while self.peek(0).is_some_and(char::is_whitespace) {
            self.index += 1;
        }
    }

    fn skip_char(&mut self, expected: char) {
        if self.peek(0) == Some(expected) {
            self.index += 1;
        } else {
            self.flag(OpaqueConstruct::UnbalancedNesting);
        }
    }

    fn process_substitution(&mut self) {
        self.index += 2;
        self.flag(OpaqueConstruct::ProcessSubstitution);
        let inner = self.read_balanced_into_nested('(', ')');
        self.current.words.push(Word {
            text: format!("<({inner})"),
            dynamic: true,
            ..Word::default()
        });
    }

    fn redirect(&mut self) {
        let c = self.peek(0);
        self.index += 1;
        let kind = match (c, self.peek(0)) {
            (Some('>'), Some('>')) => {
                self.index += 1;
                RedirectKind::Append
            }
            (Some('>'), Some('|')) => {
                self.index += 1;
                RedirectKind::Output
            }
            (Some('>'), Some('&')) => {
                self.index += 1;
                RedirectKind::DupOutput
            }
            (Some('>'), _) => RedirectKind::Output,
            (Some('<'), Some('<')) => {
                self.index += 1;
                if self.peek(0) == Some('<') {
                    self.index += 1;
                    RedirectKind::HereString
                } else {
                    RedirectKind::Heredoc
                }
            }
            (Some('<'), Some('>')) => {
                self.index += 1;
                RedirectKind::ReadWrite
            }
            (Some('<'), Some('&')) => {
                self.index += 1;
                RedirectKind::DupInput
            }
            _ => RedirectKind::Input,
        };
        if kind == RedirectKind::Heredoc {
            self.heredoc();
            return;
        }
        self.push_redirect(kind);
    }

    fn push_redirect(&mut self, kind: RedirectKind) {
        self.skip_inline_blank();
        if self
            .peek(0)
            .is_none_or(|c| is_operator_start(c) || c == '\n')
        {
            self.flag(OpaqueConstruct::UnbalancedNesting);
            return;
        }
        let target = self.read_word();
        let kind = match kind {
            RedirectKind::DupOutput
                if !(target.text == "-" || target.text.chars().all(|c| c.is_ascii_digit())) =>
            {
                RedirectKind::Output
            }
            other => other,
        };
        let body = (kind == RedirectKind::HereString).then(|| target.text.clone());
        self.current.redirects.push(Redirect { kind, target, body });
    }

    fn skip_inline_blank(&mut self) {
        while self.peek(0).is_some_and(|c| c == ' ' || c == '\t') {
            self.index += 1;
        }
    }

    fn heredoc(&mut self) {
        let strip_tabs = if self.peek(0) == Some('-') {
            self.index += 1;
            true
        } else {
            false
        };
        self.skip_inline_blank();
        let delimiter = self.read_word();
        self.flag(OpaqueConstruct::Heredoc);
        self.current.redirects.push(Redirect {
            kind: RedirectKind::Heredoc,
            target: delimiter.clone(),
            body: None,
        });
        self.pending.push(PendingHeredoc {
            segment: self.out.segments.len(),
            redirect: self.current.redirects.len() - 1,
            delimiter: delimiter.text,
            strip_tabs,
            expands: !delimiter.quoted,
        });
    }

    fn read_pending_heredocs(&mut self) {
        for pending in std::mem::take(&mut self.pending) {
            let mut body = String::new();
            let mut terminated = false;
            while self.index < self.chars.len() {
                let start = self.index;
                while self.peek(0).is_some_and(|c| c != '\n') {
                    self.index += 1;
                }
                let line: String = self.chars[start..self.index].iter().collect();
                if self.peek(0) == Some('\n') {
                    self.index += 1;
                }
                let compared = if pending.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if compared == pending.delimiter {
                    terminated = true;
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if !terminated {
                self.flag(OpaqueConstruct::UnterminatedQuote);
            }
            if pending.expands {
                self.scan_expansions(&body);
            }
            if let Some(redirect) = self
                .out
                .segments
                .get_mut(pending.segment)
                .and_then(|segment| segment.redirects.get_mut(pending.redirect))
            {
                redirect.body = Some(body);
            }
        }
    }

    /// Find command substitutions inside text that the shell expands but does
    /// not execute as a script (an unquoted heredoc body).
    fn scan_expansions(&mut self, text: &str) {
        let inner = lex_double_quoted(text);
        if !inner.nested.is_empty() {
            self.flag(OpaqueConstruct::CommandSubstitution);
        }
        self.out.nested.extend(inner.nested);
    }

    /// Read until the matching close character, honouring quotes and
    /// escapes, and return the enclosed text without the closer.
    fn read_balanced_text(&mut self, open: char, close: char) -> String {
        let start = self.index;
        let mut depth = 1_usize;
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => self.index += 2,
                '\'' => {
                    self.index += 1;
                    while self.peek(0).is_some_and(|c| c != '\'') {
                        self.index += 1;
                    }
                    self.index += 1;
                }
                '"' => {
                    self.index += 1;
                    while let Some(c) = self.peek(0) {
                        match c {
                            '\\' => self.index += 2,
                            '"' => break,
                            _ => self.index += 1,
                        }
                    }
                    self.index += 1;
                }
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        let end = self.index.min(self.chars.len());
                        return self.chars[start..end].iter().collect();
                    }
                    self.index += 1;
                }
                c if c == open => {
                    depth += 1;
                    self.index += 1;
                }
                _ => self.index += 1,
            }
        }
        self.flag(OpaqueConstruct::UnbalancedNesting);
        let end = self.index.min(self.chars.len());
        self.index = self.chars.len();
        self.chars[start.min(end)..end].iter().collect()
    }

    fn read_balanced_into_nested(&mut self, open: char, close: char) -> String {
        let inner = self.read_balanced_text(open, close);
        if self.peek(0) == Some(close) {
            self.index += 1;
        }
        self.out.nested.push(inner.clone());
        inner
    }

    fn read_word(&mut self) -> Word {
        let mut word = Word::default();
        let mut name_prefix = true;
        while let Some(c) = self.peek(0) {
            match c {
                c if c.is_whitespace() => break,
                c if is_operator_start(c) => break,
                '\\' => {
                    name_prefix = false;
                    self.index += 1;
                    match self.peek(0) {
                        Some('\n') => self.index += 1,
                        Some(escaped) => {
                            word.text.push(escaped);
                            word.quoted = true;
                            self.index += 1;
                        }
                        None => {}
                    }
                }
                '\'' => {
                    name_prefix = false;
                    word.quoted = true;
                    self.index += 1;
                    let start = self.index;
                    while self.peek(0).is_some_and(|c| c != '\'') {
                        self.index += 1;
                    }
                    if self.peek(0).is_none() {
                        self.flag(OpaqueConstruct::UnterminatedQuote);
                    }
                    word.text.extend(&self.chars[start..self.index]);
                    self.index += 1;
                }
                '"' => {
                    name_prefix = false;
                    word.quoted = true;
                    self.index += 1;
                    self.read_double_quoted(&mut word);
                }
                '$' => {
                    name_prefix = false;
                    self.read_dollar(&mut word, false);
                }
                '`' => {
                    name_prefix = false;
                    self.read_backtick(&mut word);
                }
                '=' if name_prefix && !word.text.is_empty() => {
                    word.assignment = true;
                    name_prefix = false;
                    word.text.push(c);
                    self.index += 1;
                }
                '+' if name_prefix && self.peek(1) == Some('=') && !word.text.is_empty() => {
                    word.assignment = true;
                    name_prefix = false;
                    word.text.push_str("+=");
                    self.index += 2;
                }
                _ => {
                    if !is_name_char(c) || word.text.is_empty() && c.is_ascii_digit() {
                        name_prefix = false;
                    }
                    if matches!(c, '*' | '?' | '[' | '{') {
                        word.glob = true;
                    }
                    if c == '~' && word.text.is_empty() {
                        word.dynamic = true;
                    }
                    word.text.push(c);
                    self.index += 1;
                }
            }
        }
        word
    }

    fn read_double_quoted(&mut self, word: &mut Word) {
        while let Some(c) = self.peek(0) {
            match c {
                '"' => {
                    self.index += 1;
                    return;
                }
                '\\' => {
                    self.index += 1;
                    match self.peek(0) {
                        Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                            word.text.push(escaped);
                            self.index += 1;
                        }
                        Some('\n') => self.index += 1,
                        Some(_) => word.text.push('\\'),
                        None => {}
                    }
                }
                '$' => self.read_dollar(word, true),
                '`' => self.read_backtick(word),
                _ => {
                    word.text.push(c);
                    self.index += 1;
                }
            }
        }
        self.flag(OpaqueConstruct::UnterminatedQuote);
    }

    fn read_dollar(&mut self, word: &mut Word, in_double_quotes: bool) {
        self.index += 1;
        match self.peek(0) {
            Some('(') if self.peek(1) == Some('(') => {
                self.index += 2;
                self.flag(OpaqueConstruct::ArithmeticExpansion);
                let inner = self.read_balanced_text('(', ')');
                self.skip_char(')');
                self.skip_char(')');
                word.text.push_str(&format!("$(({inner}))"));
                word.dynamic = true;
            }
            Some('(') => {
                self.index += 1;
                self.flag(OpaqueConstruct::CommandSubstitution);
                let inner = self.read_balanced_into_nested('(', ')');
                word.text.push_str(&format!("$({inner})"));
                word.dynamic = true;
            }
            Some('{') => {
                self.index += 1;
                let inner = self.read_balanced_text('{', '}');
                self.skip_char('}');
                if inner.contains("$(") || inner.contains('`') {
                    self.scan_expansions(&inner);
                }
                word.text.push_str(&format!("${{{inner}}}"));
                word.dynamic = true;
            }
            Some('\'') if !in_double_quotes => {
                self.index += 1;
                self.flag(OpaqueConstruct::AnsiCQuote);
                let start = self.index;
                while let Some(c) = self.peek(0) {
                    match c {
                        '\\' => self.index += 2,
                        '\'' => break,
                        _ => self.index += 1,
                    }
                }
                let end = self.index.min(self.chars.len());
                word.text.push_str("$'");
                word.text.extend(&self.chars[start..end]);
                word.text.push('\'');
                if self.peek(0).is_none() {
                    self.flag(OpaqueConstruct::UnterminatedQuote);
                }
                self.index += 1;
                word.quoted = true;
                word.dynamic = true;
            }
            Some('"') if !in_double_quotes => {
                // Locale translation: the text is a double-quoted string.
                self.index += 1;
                word.quoted = true;
                self.read_double_quoted(word);
            }
            Some(c) if is_name_char(c) || matches!(c, '?' | '#' | '@' | '*' | '!' | '-' | '$') => {
                word.text.push('$');
                if is_name_char(c) && !c.is_ascii_digit() {
                    while let Some(next) = self.peek(0).filter(|c| is_name_char(*c)) {
                        word.text.push(next);
                        self.index += 1;
                    }
                } else {
                    word.text.push(c);
                    self.index += 1;
                }
                word.dynamic = true;
            }
            _ => word.text.push('$'),
        }
    }

    fn read_backtick(&mut self, word: &mut Word) {
        self.index += 1;
        self.flag(OpaqueConstruct::CommandSubstitution);
        let mut inner = String::new();
        let mut terminated = false;
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => {
                    if let Some(next) = self.peek(1) {
                        if !matches!(next, '`' | '\\' | '$') {
                            inner.push('\\');
                        }
                        inner.push(next);
                    }
                    self.index += 2;
                }
                '`' => {
                    self.index += 1;
                    terminated = true;
                    break;
                }
                _ => {
                    inner.push(c);
                    self.index += 1;
                }
            }
        }
        if !terminated {
            self.flag(OpaqueConstruct::UnterminatedQuote);
        }
        word.text.push_str(&format!("`{inner}`"));
        word.dynamic = true;
        self.out.nested.push(inner);
    }
}

/// Lex text as the inside of a double-quoted string (a heredoc body or a
/// parameter expansion), returning only the nested scripts it carries.
fn lex_double_quoted(text: &str) -> Lexed {
    let mut source = String::with_capacity(text.len() + 2);
    source.push('"');
    for c in text.chars() {
        if c == '"' {
            source.push('\\');
        }
        source.push(c);
    }
    source.push('"');
    lex(&source)
}

fn is_operator_start(c: char) -> bool {
    matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')')
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Reserved words that may precede a command in the same simple-command
/// position (`if cmd`, `then cmd`, `! cmd`).
pub(super) fn is_reserved_prefix(word: &str) -> bool {
    matches!(
        word,
        "if" | "then" | "elif" | "else" | "do" | "while" | "until" | "!" | "time"
    )
}
