#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

impl Operator {
    #[must_use]
    pub const fn enters_insert(self) -> bool {
        matches!(self, Self::Change)
    }

    #[must_use]
    pub const fn key(self) -> char {
        match self {
            Self::Delete => 'd',
            Self::Change => 'c',
            Self::Yank => 'y',
        }
    }
}

#[must_use]
pub const fn parse_operator(ch: char) -> Option<Operator> {
    match ch {
        'd' => Some(Operator::Delete),
        'c' => Some(Operator::Change),
        'y' => Some(Operator::Yank),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_operators() {
        assert_eq!(parse_operator('d'), Some(Operator::Delete));
        assert_eq!(parse_operator('c'), Some(Operator::Change));
        assert_eq!(parse_operator('y'), Some(Operator::Yank));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(parse_operator('x'), None);
        assert_eq!(parse_operator('z'), None);
    }

    #[test]
    fn change_enters_insert() {
        assert!(Operator::Change.enters_insert());
        assert!(!Operator::Delete.enters_insert());
        assert!(!Operator::Yank.enters_insert());
    }

    #[test]
    fn operator_keys() {
        assert_eq!(Operator::Delete.key(), 'd');
        assert_eq!(Operator::Change.key(), 'c');
        assert_eq!(Operator::Yank.key(), 'y');
    }
}
