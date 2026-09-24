//! Writes human and JSON output, honoring colour preferences per stream.

use std::io::{IsTerminal, Write};

use serde::Serialize;

use crate::cli::CommandContext;
use crate::ui::markup::{escape_markup, render_markup};

pub struct Renderer {
    color: bool,
    plain: bool,
}

impl Renderer {
    pub fn new(context: &CommandContext) -> Self {
        let color =
            !context.global.no_color && !context.global.plain && std::env::var_os("NO_COLOR").is_none();
        Self {
            color,
            plain: context.global.plain,
        }
    }

    pub fn line(&self, markup: impl AsRef<str>) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(
            stdout,
            "{}",
            self.format(markup.as_ref(), std::io::stdout().is_terminal())
        );
    }

    pub fn error(&self, markup: impl AsRef<str>) {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(
            stderr,
            "{}",
            self.format(markup.as_ref(), std::io::stderr().is_terminal())
        );
    }

    // Pretty-printed: the same output serves scripts and a human reading it back.
    pub fn json<T: Serialize>(&self, value: &T) {
        let mut stdout = std::io::stdout().lock();
        let _ = serde_json::to_writer_pretty(&mut stdout, value);
        let _ = writeln!(stdout);
    }

    pub fn raw(&self, value: impl AsRef<[u8]>) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(value.as_ref());
        let _ = stdout.flush();
    }

    /// Escapes user-provided text so it cannot inject markup or control characters.
    pub fn value(&self, value: impl AsRef<str>) -> String {
        escape_markup(value.as_ref())
    }

    pub fn format(&self, markup: &str, tty: bool) -> String {
        // The caller says whether its stream is a tty; a pipe never gets escape codes.
        self.glyphs(&render_markup(markup, self.color && tty))
    }

    /// In plain mode, swaps Acre's own decorative symbols for ASCII. User text such as paths and
    /// branch names passes through unchanged, so printed commands still work when pasted.
    pub fn glyphs(&self, text: &str) -> String {
        if !self.plain {
            return text.to_owned();
        }
        let mut ascii = String::with_capacity(text.len());
        for character in text.chars() {
            match character {
                '→' => ascii.push_str("->"),
                '←' => ascii.push_str("<-"),
                '›' => ascii.push('>'),
                '‹' => ascii.push('<'),
                '↑' => ascii.push('^'),
                '↓' => ascii.push('v'),
                '·' | '•' => ascii.push('-'),
                '’' | '‘' => ascii.push('\''),
                '“' | '”' => ascii.push('"'),
                '…' => ascii.push_str("..."),
                // One cell each, like the symbols they replace, so status columns stay aligned.
                '✓' => ascii.push('+'),
                '✗' => ascii.push('x'),
                value => ascii.push(value),
            }
        }
        ascii
    }
}
