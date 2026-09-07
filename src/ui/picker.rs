//! The inline fuzzy picker shown by a bare `acre`.

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
    // Raw mode must end even when the loop failed, or the terminal is left unusable.
    let _ = disable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown));
    result
}

fn picker_loop<T: Clone>(renderer: &Renderer, title: &str, rows: &[PickerRow<T>]) -> Result<PickerResult<T>> {
    let mut filter = String::new();
    let mut selected = 0usize;
    let mut lines_drawn = 0usize;
    let mut stdout = std::io::stdout().lock();
    loop {
        draw(
            &mut stdout,
            renderer,
            title,
            rows,
            &filter,
            selected,
            &mut lines_drawn,
        )?;
        let event =
            read().map_err(|error| AcreError::new("ACRE_TERMINAL", error.to_string(), exit::ENVIRONMENT))?;
        let Event::Key(key) = event else { continue };
        // Windows reports release events too; acting on both would double every keystroke.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let visible = visible_rows(rows, &filter);
        match key.code {
            // Raw mode swallows the terminal's own Ctrl-C, so we have to honour it ourselves.
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
            // Any edit to the filter resets the selection; the old index points at a different row now.
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
    stdout: &mut impl Write,
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
    write!(stdout, "\r").map_err(|error| AcreError::io("could not draw picker", error))?;
    if *lines_drawn > 0 {
        // Move up over our previous frame and clear it, so the picker redraws in place.
        write!(stdout, "\x1b[{}A\x1b[0J", *lines_drawn)
            .map_err(|error| AcreError::io("could not draw picker", error))?;
    }
    let mut output = vec![format!("<bold>{}</bold>", renderer.value(title)), String::new()];
    // Keep the selection roughly centred in the 13-row window.
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
    write!(
        stdout,
        "{}\r\n",
        output
            .iter()
            // Always a tty here (pick checked), so colour depends only on the user's flags.
            .map(|line| renderer.format(line, true))
            .collect::<Vec<_>>()
            // Raw mode does not translate LF into CRLF; reset the column for every line.
            .join("\r\n")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{CommandContext, GlobalOptions, ShellBridge};

    #[test]
    fn raw_mode_frames_keep_rows_aligned_when_selecting_and_filtering() {
        let renderer = Renderer::new(&CommandContext {
            cwd: Default::default(),
            interactive: true,
            global: GlobalOptions {
                directory: None,
                json: false,
                no_color: true,
                plain: false,
                verbose: false,
            },
            shell: ShellBridge {
                active: false,
                directive_file: None,
                session_id: None,
                pid: None,
            },
        });
        let rows = ["main", "feature/alpha", "feature/beta"].map(|label| PickerRow {
            label: label.to_owned(),
            detail: Some("local branch".to_owned()),
            searchable: label.to_owned(),
            value: (),
        });
        let mut lines_drawn = 0;
        for (filter, selected, expected_rows) in [
            (
                "",
                0,
                vec![
                    "› main  local branch",
                    "  feature/alpha  local branch",
                    "  feature/beta  local branch",
                ],
            ),
            (
                "",
                1,
                vec![
                    "  main  local branch",
                    "› feature/alpha  local branch",
                    "  feature/beta  local branch",
                ],
            ),
            ("beta", 0, vec!["› feature/beta  local branch"]),
            ("missing", 0, vec!["  No matches"]),
        ] {
            let previous_lines = lines_drawn;
            let mut output = Vec::new();
            draw(
                &mut output,
                &renderer,
                "repo",
                &rows,
                filter,
                selected,
                &mut lines_drawn,
            )
            .unwrap();
            let output = String::from_utf8(output).unwrap();
            let prefix = if previous_lines == 0 {
                "\r".to_owned()
            } else {
                format!("\r\x1b[{previous_lines}A\x1b[0J")
            };
            let frame = output.strip_prefix(&prefix).expect("redraw from the left edge");
            // Every newline, including the final one, must reset the column in raw mode.
            assert!(!frame.replace("\r\n", "").contains('\n'));
            let lines = frame
                .strip_suffix("\r\n")
                .unwrap()
                .split("\r\n")
                .collect::<Vec<_>>();
            assert_eq!(&lines[..2], &["repo", ""]);
            assert_eq!(&lines[2..lines.len() - 2], expected_rows);
            assert_eq!(lines[lines.len() - 2], "");
            assert!(lines.last().unwrap().ends_with("Esc leave"));
            assert_eq!(lines_drawn, lines.len());
        }
    }
}
