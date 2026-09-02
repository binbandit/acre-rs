use std::io::{IsTerminal, Write};

use crossterm::cursor::MoveToColumn;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};

use crate::error::{AcreError, Result, exit};
use crate::ui::output::Renderer;
use crate::util::is_subsequence;

#[derive(Debug, Clone)]
pub struct PickerRow<T> {
    pub label: String,
    pub detail: Option<String>,
    pub searchable: String,
    pub value: T,
}

#[derive(Debug, Clone)]
pub enum PickerResult<T> {
    Selected(T),
    Cancelled,
    Interrupted,
}

pub fn pick<T: Clone>(renderer: &Renderer, title: &str, rows: &[PickerRow<T>]) -> Result<PickerResult<T>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(PickerResult::Cancelled);
    }
    enable_raw_mode()
        .map_err(|error| AcreError::new("ACRE_TERMINAL", error.to_string(), exit::ENVIRONMENT))?;
    let result = picker_loop(renderer, title, rows);
    let _ = disable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown));
    result
}

fn picker_loop<T: Clone>(renderer: &Renderer, title: &str, rows: &[PickerRow<T>]) -> Result<PickerResult<T>> {
    let mut filter = String::new();
    let mut selected = 0usize;
    let mut lines_drawn = 0usize;
    loop {
        draw(renderer, title, rows, &filter, selected, &mut lines_drawn)?;
        let event =
            read().map_err(|error| AcreError::new("ACRE_TERMINAL", error.to_string(), exit::ENVIRONMENT))?;
        let Event::Key(key) = event else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let visible = visible_rows(rows, &filter);
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(PickerResult::Interrupted);
            }
            KeyCode::Esc => return Ok(PickerResult::Cancelled),
            KeyCode::Up => {
                selected = if visible.is_empty() {
                    0
                } else {
                    (selected + visible.len() - 1) % visible.len()
                };
            }
            KeyCode::Down => {
                selected = if visible.is_empty() {
                    0
                } else {
                    (selected + 1) % visible.len()
                };
            }
            KeyCode::Enter => {
                if let Some(row) = visible.get(selected) {
                    return Ok(PickerResult::Selected(row.value.clone()));
                }
            }
            KeyCode::Backspace => {
                filter.pop();
                selected = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                filter.push(character);
                selected = 0;
            }
            _ => {}
        }
    }
}

fn draw<T>(
    renderer: &Renderer,
    title: &str,
    rows: &[PickerRow<T>],
    filter: &str,
    mut selected: usize,
    lines_drawn: &mut usize,
) -> Result<()> {
    let visible = visible_rows(rows, filter);
    if selected >= visible.len() {
        selected = visible.len().saturating_sub(1);
    }
    let mut stdout = std::io::stdout();
    if *lines_drawn > 0 {
        write!(stdout, "\x1b[{}A\x1b[0J", *lines_drawn)
            .map_err(|error| AcreError::io("could not draw picker", error))?;
    }
    let mut output = vec![format!("<bold>{}</bold>", renderer.value(title)), String::new()];
    let offset = selected.saturating_sub(6);
    for (index, row) in visible.iter().skip(offset).take(13).enumerate() {
        let marker = if offset + index == selected {
            "<green>›</green>"
        } else {
            " "
        };
        let detail = row
            .detail
            .as_ref()
            .map(|detail| format!("  <dim>{}</dim>", renderer.value(detail)))
            .unwrap_or_default();
        output.push(format!(
            "{marker} <blue>{}</blue>{detail}",
            renderer.value(&row.label)
        ));
    }
    if visible.is_empty() {
        output.push("  <dim>No matches</dim>".to_owned());
    }
    output.push(String::new());
    output.push(format!(
        "<dim>{} · ↑↓ select · Enter open · Esc leave</dim>",
        renderer.value(if filter.is_empty() {
            "Type to search"
        } else {
            filter
        })
    ));
    writeln!(
        stdout,
        "{}",
        output
            .iter()
            .map(|line| renderer.format(line, true))
            .collect::<Vec<_>>()
            .join("\n")
    )
    .map_err(|error| AcreError::io("could not draw picker", error))?;
    stdout
        .flush()
        .map_err(|error| AcreError::io("could not draw picker", error))?;
    *lines_drawn = output.len();
    Ok(())
}

fn visible_rows<'a, T>(rows: &'a [PickerRow<T>], filter: &str) -> Vec<&'a PickerRow<T>> {
    let needle = filter.to_ascii_lowercase();
    if needle.is_empty() {
        return rows.iter().collect();
    }
    rows.iter()
        .filter(|row| {
            let haystack = row.searchable.to_ascii_lowercase();
            haystack.contains(&needle) || is_subsequence(&needle, &haystack)
        })
        .collect()
}
