use std::io::{IsTerminal, Write};

use serde::Serialize;

use crate::model::CommandContext;
use crate::ui::markup::{escape_markup, render_markup};

pub struct Renderer<'a> {
    context: &'a CommandContext,
}

impl<'a> Renderer<'a> {
    pub fn new(context: &'a CommandContext) -> Self {
        Self { context }
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

    pub fn value(&self, value: impl AsRef<str>) -> String {
        escape_markup(value.as_ref())
    }

    pub fn format(&self, markup: &str, tty: bool) -> String {
        let color = !self.context.global.no_color
            && !self.context.global.plain
            && tty
            && std::env::var_os("NO_COLOR").is_none();
        render_markup(markup, color)
    }
}
