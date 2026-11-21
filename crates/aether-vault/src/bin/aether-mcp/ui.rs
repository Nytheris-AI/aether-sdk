//! Terminal output for `aether-mcp` commands.
//!
//! Interactive terminals get styled `cliclack` output. Pipes, `NO_COLOR`, and `TERM=dumb` get
//! plain text, so scripts, service managers, and CI logs never see escape codes or prompts.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Fail,
}

/// One line of command output, with an optional hint telling the user how to fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub level: Level,
    pub text: String,
    pub hint: Option<String>,
    /// Printed indented under the previous line in plain output.
    pub nested: bool,
}

impl Line {
    pub fn new(level: Level, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            hint: None,
            nested: false,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn nested(mut self) -> Self {
        self.nested = true;
        self
    }
}

/// Styled output is used only when both output streams are terminals, `NO_COLOR` is unset or
/// empty, and `TERM` is not `dumb`. `cliclack` writes to stderr and the commands print results on
/// stdout, so both must be terminals.
pub fn use_fancy(is_tty: bool, no_color: Option<&str>, term: Option<&str>) -> bool {
    is_tty && no_color.is_none_or(str::is_empty) && term != Some("dumb")
}

pub fn render_plain(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        if line.nested {
            out.push_str("  ");
        }
        out.push_str(&line.text);
        out.push('\n');
        if let Some(hint) = &line.hint {
            out.push_str(if line.nested {
                "    hint: "
            } else {
                "  hint: "
            });
            out.push_str(hint);
            out.push('\n');
        }
    }
    out
}

pub struct Ui {
    fancy: bool,
}

impl Ui {
    pub fn detect() -> Self {
        let is_tty = std::io::stdout().is_terminal() && std::io::stderr().is_terminal();
        let no_color = std::env::var("NO_COLOR").ok();
        let term = std::env::var("TERM").ok();
        Self {
            fancy: use_fancy(is_tty, no_color.as_deref(), term.as_deref()),
        }
    }

    pub fn intro(&self, title: &str) {
        if self.fancy {
            let _ = cliclack::intro(title);
        }
    }

    pub fn outro(&self, message: &str) {
        if self.fancy {
            let _ = cliclack::outro(message);
        }
    }

    pub fn print(&self, lines: &[Line]) {
        if !self.fancy {
            print!("{}", render_plain(lines));
            return;
        }
        for line in lines {
            let _ = match line.level {
                Level::Ok => cliclack::log::success(&line.text),
                Level::Info => cliclack::log::info(&line.text),
                Level::Warn => cliclack::log::warning(&line.text),
                Level::Fail => cliclack::log::error(&line.text),
            };
            if let Some(hint) = &line.hint {
                let _ = cliclack::log::remark(format!("Hint: {hint}"));
            }
        }
    }

    pub fn success(&self, message: &str) {
        self.print(&[Line::new(Level::Ok, message)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fancy_needs_a_terminal_and_no_opt_out() {
        assert!(use_fancy(true, None, Some("xterm-256color")));
        assert!(use_fancy(true, Some(""), None));
        assert!(!use_fancy(false, None, Some("xterm")));
        assert!(!use_fancy(true, Some("1"), Some("xterm")));
        assert!(!use_fancy(true, None, Some("dumb")));
    }

    #[test]
    fn plain_rendering_indents_nested_lines_and_shows_hints() {
        let lines = [
            Line::new(Level::Info, "mcp clients:"),
            Line::new(Level::Warn, "Cursor: not configured (/x)")
                .nested()
                .hint("run it"),
        ];
        assert_eq!(
            render_plain(&lines),
            "mcp clients:\n  Cursor: not configured (/x)\n    hint: run it\n"
        );
    }
}
