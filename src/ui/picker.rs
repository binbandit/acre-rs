//! The inline fuzzy picker shown by a bare `acre`.

use std::io::{IsTerminal, Write};

use crossterm::cursor::{Hide, MoveToColumn, Show};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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
    let mut stdout = std::io::stdout();
    let result = execute!(stdout, Hide)
        .map_err(|error| AcreError::io("could not hide picker cursor", error))
        .and_then(|()| picker_loop(renderer, title, rows));
    // Frames leave the cursor at their origin, so cleanup also works after a resize.
    let _ = execute!(stdout, MoveToColumn(0), Clear(ClearType::FromCursorDown), Show);
    // Raw mode must end even when the loop failed, or the terminal is left unusable.
    let _ = disable_raw_mode();
    result
}

fn picker_loop<T: Clone>(renderer: &Renderer, title: &str, rows: &[PickerRow<T>]) -> Result<PickerResult<T>> {
    let mut filter = String::new();
    let mut selected = 0usize;
    let mut stdout = std::io::stdout().lock();
    loop {
        draw(
            &mut stdout,
            renderer,
            title,
            rows,
            &filter,
            selected,
            size().map_err(|error| AcreError::io("could not read terminal size", error))?,
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
    (columns, height): (u16, u16),
) -> Result<()> {
    let visible = visible_rows(rows, filter);
    if selected >= visible.len() {
        selected = visible.len().saturating_sub(1);
    }
    // Leave the last column unused to avoid the terminal's pending autowrap state.
    let width = usize::from(columns.saturating_sub(1));
    let height = usize::from(height.max(1));
    let spacious = height >= 7;
    let show_title = height >= 3;
    let show_footer = height >= 2;
    let chrome = usize::from(show_title) + usize::from(show_footer) + 2 * usize::from(spacious);
    let capacity = (height - chrome).min(13);
    let mut output = Vec::new();
    if show_title {
        output.push(format_line(renderer, &[("bold", title)], width));
    }
    if spacious {
        output.push(String::new());
    }
    // Keep the selection centred where possible and fill the window at either end.
    let offset = selected
        .saturating_sub(capacity / 2)
        .min(visible.len().saturating_sub(capacity));
    for (index, row) in visible.iter().skip(offset).take(capacity).enumerate() {
        let marker = if offset + index == selected { "› " } else { "  " };
        let mut parts = vec![("green", marker), ("blue", row.label.as_str())];
        if let Some(detail) = &row.detail {
            parts.extend([("", "  "), ("dim", detail.as_str())]);
        }
        output.push(format_line(renderer, &parts, width));
    }
    if visible.is_empty() {
        output.push(format_line(renderer, &[("dim", "  No matches")], width));
    }
    if spacious {
        output.push(String::new());
    }
    if show_footer {
        let hint = if width >= 49 {
            "Type to search · ↑↓ select · Enter open · Esc leave"
        } else {
            "↑↓ · Enter · Esc"
        };
        let footer = if filter.is_empty() {
            hint.to_owned()
        } else {
            format!("{filter} · ↑↓ · Enter · Esc")
        };
        output.push(format_line(renderer, &[("dim", &footer)], width));
    }
    // Keep the cursor at the frame's origin between events. A terminal resize can
    // reflow the old rows, so their previous count cannot locate the next frame.
    write!(stdout, "\r\x1b[0J{}", output.join("\r\n"))
        .map_err(|error| AcreError::io("could not draw picker", error))?;
    if output.len() > 1 {
        write!(stdout, "\x1b[{}A", output.len() - 1)
            .map_err(|error| AcreError::io("could not draw picker", error))?;
    }
    write!(stdout, "\r")
        .and_then(|()| stdout.flush())
        .map_err(|error| AcreError::io("could not draw picker", error))
}

fn format_line(renderer: &Renderer, parts: &[(&str, &str)], width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let parts = parts
        .iter()
        .map(|(style, value)| (*style, renderer.value(value)))
        .collect::<Vec<_>>();
    let clipped = parts.iter().map(|(_, value)| value.width()).sum::<usize>() > width;
    let mut remaining = width - usize::from(clipped);
    let mut markup = String::new();
    for (style, value) in parts {
        let mut end = 0;
        for grapheme in value.graphemes(true) {
            let cells = grapheme.width();
            if cells > remaining {
                break;
            }
            remaining -= cells;
            end += grapheme.len();
        }
        if style.is_empty() {
            markup.push_str(&value[..end]);
        } else {
            markup.push_str(&format!("<{style}>{}</{style}>", &value[..end]));
        }
        if end < value.len() {
            break;
        }
    }
    if clipped {
        markup.push('…');
    }
    renderer.format(&markup, true)
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

    fn renderer() -> Renderer {
        Renderer::new(&CommandContext {
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
        })
    }

    fn rows(labels: &[&str]) -> Vec<PickerRow<()>> {
        labels
            .iter()
            .map(|label| PickerRow {
                label: (*label).to_owned(),
                detail: Some("local branch".to_owned()),
                searchable: (*label).to_owned(),
                value: (),
            })
            .collect()
    }

    fn frame(
        terminal: &mut vt100::Parser,
        rows: &[PickerRow<()>],
        filter: &str,
        selected: usize,
        size: (u16, u16),
    ) {
        let mut output = Vec::new();
        draw(&mut output, &renderer(), "repo", rows, filter, selected, size).unwrap();
        terminal.process(&output);
    }

    #[test]
    fn raw_mode_frames_keep_rows_aligned_when_selecting_and_filtering() {
        let rows = rows(&["main", "feature/alpha", "feature/beta"]);
        let mut terminal = vt100::Parser::new(24, 80, 0);
        terminal.process(b"Prior shell output\r\n$ acre\r\n");
        for (filter, selected, expected_rows) in [
            (
                "",
                0,
                "› main  local branch\n  feature/alpha  local branch\n  feature/beta  local branch",
            ),
            (
                "",
                1,
                "  main  local branch\n› feature/alpha  local branch\n  feature/beta  local branch",
            ),
            ("beta", 0, "› feature/beta  local branch"),
            ("missing", 0, "  No matches"),
            (
                "",
                0,
                "› main  local branch\n  feature/alpha  local branch\n  feature/beta  local branch",
            ),
        ] {
            frame(&mut terminal, &rows, filter, selected, (80, 24));
            let contents = terminal.screen().contents();
            assert!(
                contents.starts_with(&format!(
                    "Prior shell output\n$ acre\nrepo\n\n{expected_rows}\n\n"
                )),
                "{contents}"
            );
            assert_eq!(terminal.screen().cursor_position(), (2, 0));
            assert!(contents.ends_with(if filter.is_empty() { "Esc leave" } else { "Esc" }));
        }
        terminal.process(b"\r\x1b[0J");
        assert_eq!(terminal.screen().contents(), "Prior shell output\n$ acre");
    }

    #[test]
    fn long_rows_and_small_terminals_do_not_wrap_or_hide_the_selection() {
        let labels = (0..30)
            .map(|index| format!("feature/{index:02}-{}", "日本語".repeat(30)))
            .collect::<Vec<_>>();
        let rows = rows(&labels.iter().map(String::as_str).collect::<Vec<_>>());
        for (width, height) in [(80, 24), (40, 8), (20, 4), (10, 2), (10, 1)] {
            let mut terminal = vt100::Parser::new(height, width, 0);
            // Start at the bottom as a real shell often does.
            terminal.process(format!("\x1b[{height};1H").as_bytes());
            for selected in [0, 15, 29, 0] {
                frame(&mut terminal, &rows, "", selected, (width, height));
                let contents = terminal.screen().contents();
                assert_eq!(contents.matches('›').count(), 1, "{width}x{height}: {contents}");
                assert!(contents.contains('…'), "{contents}");
                for row in 0..height {
                    assert!(
                        !terminal.screen().row_wrapped(row),
                        "{width}x{height}: {contents}"
                    );
                }
                assert_eq!(terminal.screen().cursor_position().1, 0);
                if height >= 4 {
                    assert!(
                        contents.contains(&format!("› feature/{selected:02}-")),
                        "{contents}"
                    );
                }
            }
        }
    }

    #[test]
    fn resizing_redraws_from_the_origin_without_stale_rows() {
        let rows = rows(&["main", "feature/alpha", "feature/beta"]);
        let mut terminal = vt100::Parser::new(24, 80, 0);
        for (width, height) in [(80, 24), (20, 4), (40, 8), (80, 24)] {
            terminal.set_size(height, width);
            frame(&mut terminal, &rows, "", 2, (width, height));
            let contents = terminal.screen().contents();
            assert_eq!(contents.matches("repo").count(), 1, "{contents}");
            assert_eq!(contents.matches('›').count(), 1, "{contents}");
            assert!(contents.contains("› feature/beta"), "{contents}");
            assert_eq!(terminal.screen().cursor_position(), (0, 0));
        }
        frame(&mut terminal, &rows, &"z".repeat(200), 0, (80, 24));
        let contents = terminal.screen().contents();
        assert!(contents.contains("No matches"));
        assert!(!contents.contains("feature/"));
        for row in 0..24 {
            assert!(!terminal.screen().row_wrapped(row));
        }
    }

    #[test]
    fn clipping_preserves_graphemes_and_escapes_terminal_controls() {
        let renderer = renderer();
        for (text, width, expected) in [
            ("日本語", 5, "日本…"),
            ("👩‍💻abc", 3, "👩‍💻…"),
            ("éabc", 2, "é…"),
            ("hello", 5, "hello"),
            ("hello", 1, "…"),
            ("hello", 0, ""),
            ("<bold>\n", 20, "‹bold›\\x0a"),
        ] {
            assert_eq!(format_line(&renderer, &[("blue", text)], width), expected);
        }
        assert_eq!(
            format_line(
                &renderer,
                &[("green", "› "), ("blue", "日本語"), ("dim", " detail")],
                7
            ),
            "› 日本…"
        );
    }
}
