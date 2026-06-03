//! Shared helpers for snapshot fixtures.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::fs;
use std::path::PathBuf;

/// Render a slice of `Line`s into a `width × height` buffer and return
/// the result as text (one row per line, trailing whitespace stripped).
///
/// Styles and colors are dropped — snapshots compare characters only,
/// not ANSI.
#[must_use]
pub fn render_lines_to_text(lines: &[Line<'static>], width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    for (y, line) in (0u16..).zip(lines) {
        if y >= height {
            break;
        }
        let mut x = 0u16;
        for span in &line.spans {
            for ch in span.content.chars() {
                if x >= width {
                    break;
                }
                buf[(x, y)].set_char(ch);
                x += 1;
            }
        }
    }
    buf_to_text(&buf, width, height)
}

fn buf_to_text(buf: &Buffer, width: u16, height: u16) -> String {
    let mut out = String::new();
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            row.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        // Strip trailing whitespace so snap files stay clean.
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Compare against (or write the baseline of) a snapshot file.
///
/// `name` is the basename: `tests/snaps/<name>.snap`.
/// File missing -> write `actual` and pass (baseline mode).
/// File present and content differs -> panic with a diff.
pub fn assert_snapshot(name: &str, actual: &str) {
    let path = snap_path(name);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&path, actual).expect("write baseline snapshot");
        eprintln!("[snapshot] baseline written: {}", path.display());
        return;
    }
    let expected = fs::read_to_string(&path).expect("read existing snapshot");
    // Normalize CRLF -> LF so Windows checkouts with autocrlf=true still match.
    let expected = expected.replace("\r\n", "\n");
    let expected = expected.trim_end().to_string();
    let actual_trim = actual.trim_end().to_string();
    if expected != actual_trim {
        let diff = simple_diff(&expected, &actual_trim);
        panic!(
            "snapshot mismatch for `{name}`\n  path: {}\n  diff:\n{diff}",
            path.display()
        );
    }
}

fn snap_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snaps")
        .join(format!("{name}.snap"))
}

fn simple_diff(expected: &str, actual: &str) -> String {
    use std::fmt::Write as _;
    let exp_lines: Vec<&str> = expected.lines().collect();
    let act_lines: Vec<&str> = actual.lines().collect();
    let max = exp_lines.len().max(act_lines.len());
    let mut out = String::new();
    for i in 0..max {
        let e = exp_lines.get(i).copied().unwrap_or("<EOF>");
        let a = act_lines.get(i).copied().unwrap_or("<EOF>");
        if e == a {
            let _ = writeln!(out, "    {e}");
        } else {
            let _ = writeln!(out, "  - {e}");
            let _ = writeln!(out, "  + {a}");
        }
    }
    out
}
