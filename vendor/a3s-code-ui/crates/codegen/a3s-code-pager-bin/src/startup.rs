//! Terminal admission for the interactive pager, before startup side effects.

use super::{Command, PagerArgs};

pub(super) fn non_tty_summary(
    args: &PagerArgs,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    version: &str,
) -> Option<String> {
    let interactive = matches!(args.command, None | Some(Command::Dashboard))
        && args.single.is_none()
        && args.prompt_json.is_none()
        && args.prompt_file.is_none();
    if !interactive || (stdin_is_terminal && stdout_is_terminal) {
        return None;
    }
    let model = args
        .model
        .as_deref()
        .unwrap_or("selected at interactive startup");
    Some(format!(
        "A3S Code {version}\nmodel: {model}\nInteractive TUI requires terminal input and output.\n"
    ))
}

#[cfg(test)]
mod tests;
