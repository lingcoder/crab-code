//! Reserved semantic accents — brand / status colors that don't fit
//! the generic `accent` slot because they carry specific meaning.

use ratatui::style::Color;

/// Crab's signature warm terra cotta for dark themes.
///
/// Used by the prompt glyph, the assistant bullet, and the header
/// title. Kept as a `pub const` so components that do not yet carry a
/// [`crate::theme::Theme`] reference can cite a single source of truth.
pub const CLAUDE_DARK: Color = Color::Rgb(218, 119, 86);

/// Brand / status accents used in specific places in the UI.
#[derive(Debug, Clone, Copy)]
pub struct Accents {
    /// Claude's signature accent. Used on the welcome banner, startup
    /// tag lines, the agent-name badge, and similar brand moments.
    pub claude: Color,
    /// Permission-prompt accent. Used as the border and title of the
    /// permission dialog, and on the selected button.
    pub permission: Color,
    /// Fast-mode indicator — shown on the status line when fast mode
    /// is active. Must be distinct from every other status-line color.
    pub fast_mode: Color,
    /// Brief-label color — the inline metadata on message rows
    /// (timestamp, model name, token count) so they read as secondary.
    pub brief_label: Color,
}

impl Accents {
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            // Warm terra cotta — Crab's signature accent on dark backgrounds.
            claude: Color::Rgb(218, 119, 86),
            permission: Color::Rgb(230, 180, 80),
            fast_mode: Color::Rgb(140, 220, 255),
            brief_label: Color::Rgb(140, 140, 150),
        }
    }

    #[must_use]
    pub const fn light() -> Self {
        Self {
            claude: Color::Rgb(164, 78, 44),
            permission: Color::Rgb(185, 128, 0),
            fast_mode: Color::Rgb(25, 120, 180),
            brief_label: Color::Rgb(110, 110, 115),
        }
    }

    #[must_use]
    pub const fn monokai() -> Self {
        Self {
            claude: Color::Rgb(230, 219, 116),
            permission: Color::Rgb(253, 151, 31),
            fast_mode: Color::Rgb(102, 217, 239),
            brief_label: Color::Rgb(117, 113, 94),
        }
    }

    #[must_use]
    pub const fn solarized() -> Self {
        Self {
            claude: Color::Rgb(203, 75, 22),
            permission: Color::Rgb(181, 137, 0),
            fast_mode: Color::Rgb(42, 161, 152),
            brief_label: Color::Rgb(88, 110, 117),
        }
    }
}

impl Default for Accents {
    fn default() -> Self {
        Self::dark()
    }
}
