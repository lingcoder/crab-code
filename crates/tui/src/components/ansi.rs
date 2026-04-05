//! ANSI escape sequence parser → ratatui `Span` conversion.
//!
//! Handles SGR (Select Graphic Rendition) codes in tool output text,
//! converting them to styled ratatui `Span`s for TUI rendering.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Parse a string that may contain ANSI escape sequences into styled `Line`s.
///
/// Each output `Line` corresponds to a newline-delimited segment.
/// ANSI CSI SGR sequences (`\x1b[...m`) are parsed and converted to
/// ratatui `Style`s; all other escape sequences are silently stripped.
pub fn parse_ansi(input: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_style = Style::default();
    let mut buf = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Flush buffered text
            if !buf.is_empty() {
                current_spans.push(Span::styled(std::mem::take(&mut buf), current_style));
            }

            // Expect '[' for CSI
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                let params = consume_csi_params(&mut chars);
                // Only handle SGR (terminated by 'm')
                current_style = apply_sgr(&params, current_style);
            } else {
                // Non-CSI escape: consume the next char (escape type indicator)
                chars.next();
            }
        } else if ch == '\n' {
            if !buf.is_empty() {
                current_spans.push(Span::styled(std::mem::take(&mut buf), current_style));
            }
            lines.push(Line::from(std::mem::take(&mut current_spans)));
        } else {
            buf.push(ch);
        }
    }

    // Flush remaining
    if !buf.is_empty() {
        current_spans.push(Span::styled(buf, current_style));
    }
    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    lines
}

/// Consume CSI parameter bytes until the final byte (0x40-0x7E).
///
/// Returns the final byte and the collected parameter string.
/// For SGR the final byte is 'm'.
fn consume_csi_params(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Vec<u16> {
    let mut param_str = String::new();

    // Collect parameter bytes (0x30-0x3F) and intermediate bytes (0x20-0x2F)
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_digit() || ch == ';' {
            param_str.push(ch);
            chars.next();
        } else {
            // Final byte — consume it
            chars.next();
            break;
        }
    }

    if param_str.is_empty() {
        return vec![0]; // default reset
    }

    param_str
        .split(';')
        .map(|s| s.parse::<u16>().unwrap_or(0))
        .collect()
}

/// Apply SGR parameter codes to the current style.
#[allow(clippy::too_many_lines)]
fn apply_sgr(params: &[u16], mut style: Style) -> Style {
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => style = Style::default(), // reset
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            9 => style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            29 => style = style.remove_modifier(Modifier::CROSSED_OUT),

            // Standard foreground (30-37)
            30..=37 => style = style.fg(standard_color(params[i] - 30)),
            39 => style = Style { fg: None, ..style },
            // Standard background (40-47)
            40..=47 => style = style.bg(standard_color(params[i] - 40)),
            49 => style = Style { bg: None, ..style },

            // Bright foreground (90-97)
            90..=97 => style = style.fg(bright_color(params[i] - 90)),
            // Bright background (100-107)
            100..=107 => style = style.bg(bright_color(params[i] - 100)),

            // Extended color: 38;5;n (256-color) or 38;2;r;g;b (truecolor)
            38 => {
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 if i + 2 < params.len() => {
                            style = style.fg(color_256(params[i + 2]));
                            i += 2;
                        }
                        2 if i + 4 < params.len() => {
                            style = style.fg(Color::Rgb(
                                truncate_u8(params[i + 2]),
                                truncate_u8(params[i + 3]),
                                truncate_u8(params[i + 4]),
                            ));
                            i += 4;
                        }
                        _ => i += 1,
                    }
                }
            }
            // Extended background
            48 => {
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 if i + 2 < params.len() => {
                            style = style.bg(color_256(params[i + 2]));
                            i += 2;
                        }
                        2 if i + 4 < params.len() => {
                            style = style.bg(Color::Rgb(
                                truncate_u8(params[i + 2]),
                                truncate_u8(params[i + 3]),
                                truncate_u8(params[i + 4]),
                            ));
                            i += 4;
                        }
                        _ => i += 1,
                    }
                }
            }
            _ => {} // unknown code — ignore
        }
        i += 1;
    }
    style
}

/// Truncate u16 to u8 (ANSI color values are always 0-255).
const fn truncate_u8(v: u16) -> u8 {
    (v & 0xFF) as u8
}

/// Map 0-7 to the standard 8 terminal colors.
fn standard_color(idx: u16) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::White,
    }
}

/// Map 0-7 to the bright/intense terminal colors.
fn bright_color(idx: u16) -> Color {
    match idx {
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

/// Convert a 256-color index to a ratatui `Color`.
fn color_256(idx: u16) -> Color {
    Color::Indexed(truncate_u8(idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn plain_text_no_escapes() {
        let lines = parse_ansi("hello world");
        assert_eq!(lines.len(), 1);
        assert_eq!(all_text(&lines), "hello world");
    }

    #[test]
    fn multiline_plain_text() {
        let lines = parse_ansi("line1\nline2\nline3");
        assert_eq!(lines.len(), 3);
        assert_eq!(all_text(&lines), "line1\nline2\nline3");
    }

    #[test]
    fn bold_text() {
        let lines = parse_ansi("\x1b[1mhello\x1b[0m");
        assert_eq!(lines.len(), 1);
        assert_eq!(all_text(&lines), "hello");
        let span = &lines[0].spans[0];
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn red_foreground() {
        let lines = parse_ansi("\x1b[31mred text\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(Color::Red));
        assert_eq!(span.content.as_ref(), "red text");
    }

    #[test]
    fn green_background() {
        let lines = parse_ansi("\x1b[42mgreen bg\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.bg, Some(Color::Green));
    }

    #[test]
    fn bright_colors() {
        let lines = parse_ansi("\x1b[91mbright red\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(Color::LightRed));
    }

    #[test]
    fn color_256_foreground() {
        let lines = parse_ansi("\x1b[38;5;208morange\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(Color::Indexed(208)));
    }

    #[test]
    fn truecolor_foreground() {
        let lines = parse_ansi("\x1b[38;2;255;128;0mtrue\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(Color::Rgb(255, 128, 0)));
    }

    #[test]
    fn truecolor_background() {
        let lines = parse_ansi("\x1b[48;2;10;20;30mbg\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.bg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn combined_bold_red() {
        let lines = parse_ansi("\x1b[1;31mhighlight\x1b[0m");
        let span = &lines[0].spans[0];
        assert_eq!(span.style.fg, Some(Color::Red));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn reset_clears_style() {
        let lines = parse_ansi("\x1b[31mred\x1b[0mnormal");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(lines[0].spans[1].style, Style::default());
    }

    #[test]
    fn italic_and_underline() {
        let lines = parse_ansi("\x1b[3;4mfancy\x1b[0m");
        let span = &lines[0].spans[0];
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn strikethrough() {
        let lines = parse_ansi("\x1b[9mstruck\x1b[0m");
        let span = &lines[0].spans[0];
        assert!(span.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn empty_input() {
        let lines = parse_ansi("");
        assert!(lines.is_empty());
    }

    #[test]
    fn escape_without_bracket_stripped() {
        let lines = parse_ansi("\x1bXjunk");
        // The \x1b is consumed but non-CSI; 'X' consumed as final byte-ish,
        // remaining text should appear
        assert_eq!(all_text(&lines), "junk");
    }

    #[test]
    fn multiple_lines_with_color() {
        let lines = parse_ansi("\x1b[32mgreen\nstill green\x1b[0m");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
    }
}
