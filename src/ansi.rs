//! Terminal colour and width helpers.
//!
//! fzf rows and the preview are ANSI-coloured text. To align columns and clip
//! captured screens we must measure what the terminal will *show*, which means
//! skipping escape sequences and counting wide (CJK) characters as two cells.

use unicode_width::UnicodeWidthChar;

/// Length in bytes of the CSI escape sequence starting at `s`, if there is one.
///
/// A CSI sequence is `ESC [`, parameter bytes (0x30–0x3F), intermediate bytes
/// (0x20–0x2F), and one final byte (0x40–0x7E). Parsing by hand keeps
/// clipping linear and avoids a regex per character.
fn csi_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[0] != 0x1b || bytes[1] != b'[' {
        return None;
    }
    let mut i = 2;
    while i < bytes.len() && (0x30..=0x3f).contains(&bytes[i]) {
        i += 1;
    }
    while i < bytes.len() && (0x20..=0x2f).contains(&bytes[i]) {
        i += 1;
    }
    if i < bytes.len() && (0x40..=0x7e).contains(&bytes[i]) {
        Some(i + 1)
    } else {
        None
    }
}

const ANSI: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// SGR parameter for a tmux-style colour: a name, `brightNAME`, `colourN`, `N`, or `#rrggbb`.
fn sgr(colour: &str) -> String {
    let name = colour.to_ascii_lowercase();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(n) = name
        .strip_prefix("colour")
        .or_else(|| name.strip_prefix("color"))
        .filter(|n| digits(n))
    {
        return format!("38;5;{n}");
    }
    if digits(&name) {
        return format!("38;5;{name}");
    }
    if name.len() == 7 && name.starts_with('#') {
        let channel = |i: usize| u8::from_str_radix(&name[i..i + 2], 16);
        if let (Ok(r), Ok(g), Ok(b)) = (channel(1), channel(3), channel(5)) {
            return format!("38;2;{r};{g};{b}");
        }
    }
    if let Some(base) = name.strip_prefix("bright")
        && let Some(index) = ANSI.iter().position(|c| *c == base)
    {
        return (90 + index).to_string();
    }
    if let Some(index) = ANSI.iter().position(|c| *c == name) {
        return (30 + index).to_string();
    }
    "39".to_string()
}

/// `text` in a foreground colour, reset afterwards.
pub fn tint(text: &str, colour: &str) -> String {
    format!("\x1b[{}m{text}\x1b[0m", sgr(colour))
}

/// `text` in bold, reset afterwards.
pub fn bold(text: &str) -> String {
    format!("\x1b[1m{text}\x1b[0m")
}

fn cell_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Remove escape sequences.
pub fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(len) = csi_len(rest) {
            rest = &rest[len..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Number of terminal cells `text` occupies.
pub fn visible_width(text: &str) -> usize {
    strip(text).chars().map(cell_width).sum()
}

/// Pad with spaces on the right up to `width` cells.
pub fn pad(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(visible_width(text));
    format!("{text}{}", " ".repeat(fill))
}

/// Clip one coloured row to `columns` cells without letting it wrap, keeping
/// escape sequences intact and ending with a reset when any were kept.
pub fn clip_line(line: &str, columns: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(len) = csi_len(rest) {
            out.push_str(&rest[..len]);
            rest = &rest[len..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        let w = cell_width(c);
        if width + w > columns {
            break;
        }
        out.push(c);
        width += w;
        rest = &rest[c.len_utf8()..];
    }
    if out.contains('\x1b') {
        out.push_str("\x1b[0m");
    }
    out
}

/// Keep the bottom `rows` non-blank lines of a screen capture, clipped to `columns`.
pub fn fit_frame(frame: &str, columns: usize, rows: usize) -> String {
    let mut lines: Vec<&str> = frame.lines().collect();
    while lines
        .last()
        .is_some_and(|line| strip(line).trim().is_empty())
    {
        lines.pop();
    }
    let start = lines.len().saturating_sub(rows);
    lines[start..]
        .iter()
        .map(|line| clip_line(line, columns))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_follow_tmux_names() {
        let code = |c: &str| tint("x", c).split('m').next().unwrap().to_string();
        assert_eq!(code("blue"), "\x1b[34");
        assert_eq!(code("brightred"), "\x1b[91");
        assert_eq!(code("colour201"), "\x1b[38;5;201");
        assert_eq!(code("12"), "\x1b[38;5;12");
        assert_eq!(code("#ff8800"), "\x1b[38;2;255;136;0");
        assert_eq!(code("nonsense"), "\x1b[39");
    }

    #[test]
    fn width_ignores_escapes_and_counts_wide_chars() {
        assert_eq!(visible_width(&tint("ab", "red")), 2);
        assert_eq!(visible_width("界z"), 3);
        assert_eq!(pad("ab", 4), "ab  ");
    }

    #[test]
    fn frames_clip_and_follow_the_bottom() {
        assert_eq!(
            fit_frame("top\n0123456789ABCD\ncurrent\nlast\n\n", 8, 3),
            "01234567\ncurrent\nlast"
        );
        assert_eq!(fit_frame("abcdef界z", 8, 1), "abcdef界");
        assert_eq!(
            clip_line("\x1b[31mred\x1b[0m tail", 4),
            "\x1b[31mred\x1b[0m \x1b[0m"
        );
    }
}
