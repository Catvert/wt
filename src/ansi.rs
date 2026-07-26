//! Turning the ANSI sequences of an output line into ratatui styles.
//!
//! A project's hooks are scripts: they colour their messages, and so do `docker`,
//! `cargo` and `npm`. Without interpretation the panel would literally show
//! `[36m…[0m`. Non-graphic sequences (cursor movement, erasing) are dropped: they make
//! no sense in a list of lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Splits a line into styled segments.
pub fn to_spans(line: &str, base: Style) -> Vec<Span<'static>> {
    // Carriage return: progress bars rewrite the line. Only the last state matters,
    // just like on a real terminal.
    let line = match line.rfind('\r') {
        Some(i) if i + 1 < line.len() => &line[i + 1..],
        Some(_) => "",
        None => line,
    };

    let mut spans = Vec::new();
    let mut style = base;
    let mut text = String::new();
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\x1b' {
            // A literal tab breaks the rendering alignment.
            if c == '\t' {
                text.push_str("    ");
            } else if !c.is_control() {
                text.push(c);
            }
            continue;
        }

        match chars.next() {
            // CSI … final byte
            Some('[') => {
                let mut params = String::new();
                let mut final_byte = None;
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        final_byte = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if final_byte == Some('m') {
                    if !text.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut text), style));
                    }
                    style = apply_sgr(style, base, &params);
                }
                // Any other CSI sequence is ignored (erase, move…).
            }
            // OSC … BEL or ESC \ (window titles, hyperlinks)
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }
    spans
}

/// Applies an SGR sequence (`ESC[…m`) to the current style.
fn apply_sgr(mut style: Style, base: Style, params: &str) -> Style {
    // `ESC[m` is equivalent to `ESC[0m`.
    if params.is_empty() {
        return base;
    }
    let codes: Vec<u8> = params
        .split(';')
        .map(|p| p.trim().parse::<u8>().unwrap_or(0))
        .collect();

    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => style = base,
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            30..=37 => style = style.fg(basic(codes[i] - 30)),
            90..=97 => style = style.fg(bright(codes[i] - 90)),
            40..=47 => style = style.bg(basic(codes[i] - 40)),
            100..=107 => style = style.bg(bright(codes[i] - 100)),
            39 => style.fg = base.fg,
            49 => style.bg = base.bg,
            // Extended colours: `38;5;n` (256 palette) or `38;2;r;g;b`.
            38 | 48 => {
                let fg = codes[i] == 38;
                let (color, used) = extended(&codes[i + 1..]);
                if let Some(c) = color {
                    style = if fg { style.fg(c) } else { style.bg(c) };
                }
                i += used;
            }
            _ => {}
        }
        i += 1;
    }
    style
}

fn extended(rest: &[u8]) -> (Option<Color>, usize) {
    match rest.first() {
        Some(5) => (rest.get(1).map(|n| Color::Indexed(*n)), 2),
        Some(2) => match (rest.get(1), rest.get(2), rest.get(3)) {
            (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(*r, *g, *b)), 4),
            _ => (None, rest.len()),
        },
        _ => (None, 0),
    }
}

fn basic(n: u8) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn bright(n: u8) -> Color {
    match n {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &str) -> String {
        to_spans(line, Style::default())
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn colours_instead_of_printing_codes() {
        let spans = to_spans("\x1b[36m  ·\x1b[0m .env rewired", Style::default());
        assert_eq!(plain("\x1b[36m  ·\x1b[0m .env rewired"), "  · .env rewired");
        assert_eq!(spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(spans[1].style.fg, None);
    }

    #[test]
    fn ignores_non_graphic_sequences() {
        assert_eq!(plain("a\x1b[2Kb\x1b[1;3Hc"), "abc");
    }

    #[test]
    fn keeps_the_last_progress_bar_state() {
        assert_eq!(plain("10%\r50%\r100%"), "100%");
    }

    #[test]
    fn understands_extended_colours() {
        let spans = to_spans("\x1b[38;5;208mORANGE", Style::default());
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(208)));
        let spans = to_spans("\x1b[38;2;10;20;30mRGB", Style::default());
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn returns_to_the_base_style_on_reset() {
        let base = Style::default().fg(Color::Gray);
        let spans = to_spans("\x1b[31mred\x1b[0mnormal", base);
        assert_eq!(spans[0].style.fg, Some(Color::Red));
        assert_eq!(spans[1].style.fg, Some(Color::Gray));
    }

    #[test]
    fn strips_osc_hyperlinks() {
        assert_eq!(plain("\x1b]8;;http://x\x07text\x1b]8;;\x07"), "text");
    }
}
