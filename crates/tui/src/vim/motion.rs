#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Down,
    Up,
    Right,
    LineStart,
    LineEnd,
    WordForward,
    WordBackward,
    FirstNonBlank,
    BufferTop,
    BufferBottom,
}

impl Motion {
    #[must_use]
    pub fn apply(self, pos: CursorPos, lines: &[&str]) -> CursorPos {
        if lines.is_empty() {
            return pos;
        }

        let row = pos.row.min(lines.len() - 1);
        let line = lines[row];
        let col = pos.col.min(line.len());

        match self {
            Self::Left => CursorPos {
                row,
                col: col.saturating_sub(1),
            },
            Self::Right => CursorPos {
                row,
                col: (col + 1).min(line.len().saturating_sub(1)),
            },
            Self::Up => {
                let new_row = row.saturating_sub(1);
                CursorPos {
                    row: new_row,
                    col: col.min(lines[new_row].len().saturating_sub(1)),
                }
            }
            Self::Down => {
                let new_row = (row + 1).min(lines.len() - 1);
                CursorPos {
                    row: new_row,
                    col: col.min(lines[new_row].len().saturating_sub(1)),
                }
            }
            Self::LineStart => CursorPos { row, col: 0 },
            Self::LineEnd => CursorPos {
                row,
                col: line.len().saturating_sub(1),
            },
            Self::FirstNonBlank => {
                let first = line
                    .char_indices()
                    .find(|(_, c)| !c.is_whitespace())
                    .map_or(0, |(i, _)| i);
                CursorPos { row, col: first }
            }
            Self::WordForward => {
                let new_col = next_word_start(line, col);
                CursorPos { row, col: new_col }
            }
            Self::WordBackward => {
                let new_col = prev_word_start(line, col);
                CursorPos { row, col: new_col }
            }
            Self::BufferTop => CursorPos { row: 0, col: 0 },
            Self::BufferBottom => {
                let last = lines.len() - 1;
                CursorPos { row: last, col: 0 }
            }
        }
    }
}

fn next_word_start(line: &str, col: usize) -> usize {
    let bytes = line.as_bytes();
    let len = bytes.len();
    if col >= len {
        return col;
    }

    let mut i = col;

    if i < len && is_word_char(bytes[i]) {
        while i < len && is_word_char(bytes[i]) {
            i += 1;
        }
    } else if i < len && !bytes[i].is_ascii_whitespace() {
        while i < len && !is_word_char(bytes[i]) && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }

    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    i.min(len.saturating_sub(1))
}

fn prev_word_start(line: &str, col: usize) -> usize {
    let bytes = line.as_bytes();
    if col == 0 || bytes.is_empty() {
        return 0;
    }

    let mut i = col.min(bytes.len()) - 1;

    while i > 0 && bytes[i].is_ascii_whitespace() {
        i -= 1;
    }

    if is_word_char(bytes[i]) {
        while i > 0 && is_word_char(bytes[i - 1]) {
            i -= 1;
        }
    } else {
        while i > 0 && !is_word_char(bytes[i - 1]) && !bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
    }

    i
}

const fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<&'static str> {
        vec!["hello world", "foo bar", "baz"]
    }

    fn pos(row: usize, col: usize) -> CursorPos {
        CursorPos { row, col }
    }

    #[test]
    fn left_stops_at_zero() {
        let l = lines();
        assert_eq!(Motion::Left.apply(pos(0, 0), &l), pos(0, 0));
    }

    #[test]
    fn left_moves_one() {
        let l = lines();
        assert_eq!(Motion::Left.apply(pos(0, 5), &l), pos(0, 4));
    }

    #[test]
    fn right_moves_one() {
        let l = lines();
        assert_eq!(Motion::Right.apply(pos(0, 0), &l), pos(0, 1));
    }

    #[test]
    fn right_stops_at_end() {
        let l = lines();
        assert_eq!(Motion::Right.apply(pos(0, 10), &l), pos(0, 10));
    }

    #[test]
    fn up_moves_one() {
        let l = lines();
        assert_eq!(Motion::Up.apply(pos(1, 0), &l), pos(0, 0));
    }

    #[test]
    fn up_stops_at_top() {
        let l = lines();
        assert_eq!(Motion::Up.apply(pos(0, 3), &l), pos(0, 3));
    }

    #[test]
    fn down_moves_one() {
        let l = lines();
        assert_eq!(Motion::Down.apply(pos(0, 0), &l), pos(1, 0));
    }

    #[test]
    fn down_stops_at_bottom() {
        let l = lines();
        assert_eq!(Motion::Down.apply(pos(2, 0), &l), pos(2, 0));
    }

    #[test]
    fn down_clamps_col() {
        let l = lines();
        let result = Motion::Down.apply(pos(1, 5), &l);
        assert_eq!(result.row, 2);
        assert!(result.col <= 2);
    }

    #[test]
    fn line_start() {
        let l = lines();
        assert_eq!(Motion::LineStart.apply(pos(0, 5), &l), pos(0, 0));
    }

    #[test]
    fn line_end() {
        let l = lines();
        assert_eq!(Motion::LineEnd.apply(pos(0, 0), &l), pos(0, 10));
    }

    #[test]
    fn first_non_blank() {
        let l = vec!["  hello", "world"];
        assert_eq!(Motion::FirstNonBlank.apply(pos(0, 0), &l), pos(0, 2));
    }

    #[test]
    fn word_forward() {
        let l = vec!["hello world foo"];
        assert_eq!(Motion::WordForward.apply(pos(0, 0), &l), pos(0, 6));
    }

    #[test]
    fn word_backward() {
        let l = vec!["hello world"];
        assert_eq!(Motion::WordBackward.apply(pos(0, 8), &l), pos(0, 6));
    }

    #[test]
    fn buffer_top() {
        let l = lines();
        assert_eq!(Motion::BufferTop.apply(pos(2, 2), &l), pos(0, 0));
    }

    #[test]
    fn buffer_bottom() {
        let l = lines();
        assert_eq!(Motion::BufferBottom.apply(pos(0, 0), &l), pos(2, 0));
    }

    #[test]
    fn empty_lines() {
        let l: Vec<&str> = vec![];
        assert_eq!(Motion::Left.apply(pos(0, 0), &l), pos(0, 0));
    }
}
