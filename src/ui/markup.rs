//! A tiny tag markup (`<bold>`, `<green>`) that renders to ANSI colour or plain text.

const TAGS: &[(&str, &str)] = &[
    ("bold", "\x1b[1m"),
    ("dim", "\x1b[2m"),
    ("red", "\x1b[31m"),
    ("green", "\x1b[32m"),
    ("yellow", "\x1b[33m"),
    ("blue", "\x1b[34m"),
    ("magenta", "\x1b[35m"),
    ("cyan", "\x1b[36m"),
];

pub fn escape_markup(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        let code = character as u32;
        if code <= 0x1f || (0x7f..=0x9f).contains(&code) {
            output.push_str(&format!("\\x{code:02x}"));
        } else if character == '<' {
            output.push('‹');
        } else if character == '>' {
            output.push('›');
        } else {
            output.push(character);
        }
    }
    output
}

pub fn render_markup(value: &str, color: bool) -> String {
    transform(value, color)
}

fn transform(value: &str, color: bool) -> String {
    let mut output = String::with_capacity(value.len());
    let mut active: Vec<&str> = Vec::new();
    let bytes = value.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            let next = value[cursor..]
                .find('<')
                .map(|offset| cursor + offset)
                .unwrap_or(value.len());
            output.push_str(&value[cursor..next]);
            cursor = next;
            continue;
        }
        let Some(end_offset) = value[cursor..].find('>') else {
            output.push_str(&value[cursor..]);
            break;
        };
        let end = cursor + end_offset;
        let tag = &value[cursor + 1..end];
        let closing = tag.starts_with('/');
        let name = tag.trim_start_matches('/');
        let Some((_, ansi)) = TAGS.iter().find(|(candidate, _)| *candidate == name) else {
            output.push_str(&value[cursor..=end]);
            cursor = end + 1;
            continue;
        };
        if color {
            if closing {
                if let Some(position) = active.iter().rposition(|style| *style == name) {
                    active.remove(position);
                }
                output.push_str("\x1b[0m");
                for style in &active {
                    if let Some((_, active_ansi)) = TAGS.iter().find(|(candidate, _)| candidate == style) {
                        output.push_str(active_ansi);
                    }
                }
            } else {
                active.push(name);
                output.push_str(ansi);
            }
        }
        cursor = end + 1;
    }
    if color && !active.is_empty() {
        output.push_str("\x1b[0m");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_known_tags() {
        assert_eq!(render_markup("<bold><green>Ready</green></bold>", false), "Ready");
    }

    #[test]
    fn escapes_control_and_markup() {
        assert_eq!(escape_markup("<x>\n"), "‹x›\\x0a");
    }
}
