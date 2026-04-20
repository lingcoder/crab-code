//! Per-agent accent colors.
//!
//! When multiple subagents are running concurrently, each one is given a
//! stable color drawn from this 8-slot palette. The palette order is
//! fixed, so an agent that draws slot 3 keeps that color for the life of
//! the session.

use ratatui::style::Color;

/// The 8 distinct agent accent slots, in display order.
pub const AGENTS_PALETTE_DARK: [Color; 8] = [
    Color::Rgb(238, 84, 84),   // red
    Color::Rgb(82, 139, 255),  // blue
    Color::Rgb(88, 214, 141),  // green
    Color::Rgb(240, 196, 25),  // yellow
    Color::Rgb(188, 128, 230), // purple
    Color::Rgb(255, 140, 80),  // orange
    Color::Rgb(240, 128, 192), // pink
    Color::Rgb(100, 210, 220), // cyan
];

/// Light-theme variant, chosen for legibility against a white background.
pub const AGENTS_PALETTE_LIGHT: [Color; 8] = [
    Color::Rgb(170, 20, 20),
    Color::Rgb(25, 85, 200),
    Color::Rgb(25, 140, 70),
    Color::Rgb(175, 125, 0),
    Color::Rgb(120, 60, 175),
    Color::Rgb(200, 85, 20),
    Color::Rgb(180, 60, 130),
    Color::Rgb(20, 125, 145),
];

/// Pick the agent color for a given agent index.
#[must_use]
pub fn agent_color(palette: &[Color; 8], index: usize) -> Color {
    palette[index % palette.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_cycles_through_all_slots() {
        let colors: Vec<_> = (0..8)
            .map(|i| agent_color(&AGENTS_PALETTE_DARK, i))
            .collect();
        let distinct: std::collections::HashSet<_> = colors.iter().copied().collect();
        assert_eq!(distinct.len(), 8);
    }

    #[test]
    fn palette_wraps() {
        let first = agent_color(&AGENTS_PALETTE_DARK, 0);
        let ninth = agent_color(&AGENTS_PALETTE_DARK, 8);
        assert_eq!(first, ninth);
    }
}
