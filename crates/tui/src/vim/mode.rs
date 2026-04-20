use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Visual,
    Command,
}

impl VimMode {
    #[must_use]
    pub const fn is_insert_like(self) -> bool {
        matches!(self, Self::Insert)
    }

    #[must_use]
    pub const fn is_navigable(self) -> bool {
        matches!(self, Self::Normal | Self::Visual)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::Command => "COMMAND",
        }
    }
}

impl fmt::Display for VimMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_normal() {
        assert_eq!(VimMode::default(), VimMode::Normal);
    }

    #[test]
    fn insert_is_insert_like() {
        assert!(VimMode::Insert.is_insert_like());
        assert!(!VimMode::Normal.is_insert_like());
        assert!(!VimMode::Visual.is_insert_like());
        assert!(!VimMode::Command.is_insert_like());
    }

    #[test]
    fn navigable_modes() {
        assert!(VimMode::Normal.is_navigable());
        assert!(VimMode::Visual.is_navigable());
        assert!(!VimMode::Insert.is_navigable());
        assert!(!VimMode::Command.is_navigable());
    }

    #[test]
    fn labels() {
        assert_eq!(VimMode::Normal.label(), "NORMAL");
        assert_eq!(VimMode::Insert.label(), "INSERT");
        assert_eq!(VimMode::Visual.label(), "VISUAL");
        assert_eq!(VimMode::Command.label(), "COMMAND");
    }

    #[test]
    fn display_matches_label() {
        assert_eq!(format!("{}", VimMode::Normal), "NORMAL");
        assert_eq!(format!("{}", VimMode::Insert), "INSERT");
    }
}
