//! Renderable trait and flex layout — inspired by Codex's component composition model.
//!
//! Provides a uniform rendering protocol for all TUI components. Every visual
//! element implements `Renderable`, enabling flex-based layout allocation
//! and composable rendering without ratatui's `Widget` trait limitations.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Core rendering protocol for TUI components.
///
/// Unlike ratatui's `Widget` (which consumes self), `Renderable` takes `&self`
/// so components can be measured (`desired_height`) before rendering.
pub trait Renderable {
    /// Render this component into the given area.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Desired height in rows for the given width. Used by flex layout
    /// to allocate space before rendering.
    fn desired_height(&self, width: u16) -> u16;

    /// Optional cursor position within the rendered area.
    /// Returns `None` if this component doesn't own the cursor.
    fn cursor_position(&self, _area: Rect) -> Option<(u16, u16)> {
        None
    }
}

/// A flex layout item — wraps a renderable with layout hints.
pub struct FlexItem<'a> {
    /// The component to render.
    pub renderable: &'a dyn Renderable,
    /// `None` = use `desired_height` (fixed). `Some(n)` = flex factor for
    /// proportional space allocation.
    pub flex: Option<u16>,
    /// Minimum height this item requires.
    pub min_height: u16,
}

/// Compute a vertical flex layout and render items into the buffer.
///
/// Fixed items (flex=None) get their `desired_height`. Remaining space
/// is distributed to flex items proportionally by their flex factors.
/// Returns the `Rect` assigned to each item.
pub fn flex_layout(items: &[FlexItem<'_>], area: Rect, buf: &mut Buffer) -> Vec<Rect> {
    if items.is_empty() || area.height == 0 {
        return Vec::new();
    }

    let mut rects = Vec::with_capacity(items.len());

    // Pass 1: allocate fixed items, sum flex factors
    let mut fixed_total: u16 = 0;
    let mut flex_total: u16 = 0;
    let mut heights: Vec<u16> = Vec::with_capacity(items.len());

    for item in items {
        match item.flex {
            None => {
                let h = item
                    .renderable
                    .desired_height(area.width)
                    .max(item.min_height);
                heights.push(h);
                fixed_total = fixed_total.saturating_add(h);
            }
            Some(f) => {
                heights.push(0); // placeholder
                flex_total = flex_total.saturating_add(f);
            }
        }
    }

    // Pass 2: distribute remaining space to flex items
    let remaining = area.height.saturating_sub(fixed_total);
    if flex_total > 0 {
        let mut distributed: u16 = 0;
        let mut last_flex_idx = 0;
        for (i, item) in items.iter().enumerate() {
            if let Some(f) = item.flex {
                let share = (u32::from(remaining) * u32::from(f) / u32::from(flex_total)) as u16;
                heights[i] = share.max(item.min_height);
                distributed = distributed.saturating_add(heights[i]);
                last_flex_idx = i;
            }
        }
        // Give rounding remainder to last flex item
        if distributed < remaining {
            heights[last_flex_idx] =
                heights[last_flex_idx].saturating_add(remaining.saturating_sub(distributed));
        }
    }

    // Pass 3: assign rects and render
    let mut y = area.y;
    for (i, item) in items.iter().enumerate() {
        let h = heights[i].min(area.y + area.height - y);
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: h,
        };
        rects.push(rect);
        if h > 0 {
            item.renderable.render(rect, buf);
        }
        y = y.saturating_add(h);
    }

    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub renderable with fixed height.
    struct FixedHeight(u16);
    impl Renderable for FixedHeight {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn desired_height(&self, _width: u16) -> u16 {
            self.0
        }
    }

    #[test]
    fn flex_layout_empty() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let rects = flex_layout(&[], area, &mut buf);
        assert!(rects.is_empty());
    }

    #[test]
    fn flex_layout_single_fixed() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let item = FixedHeight(4);
        let rects = flex_layout(
            &[FlexItem {
                renderable: &item,
                flex: None,
                min_height: 0,
            }],
            area,
            &mut buf,
        );
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].height, 4);
    }

    #[test]
    fn flex_layout_fixed_and_flex() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let header = FixedHeight(4);
        let content = FixedHeight(0);
        let footer = FixedHeight(1);

        let rects = flex_layout(
            &[
                FlexItem {
                    renderable: &header,
                    flex: None,
                    min_height: 0,
                },
                FlexItem {
                    renderable: &content,
                    flex: Some(1),
                    min_height: 1,
                },
                FlexItem {
                    renderable: &footer,
                    flex: None,
                    min_height: 0,
                },
            ],
            area,
            &mut buf,
        );
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].height, 4); // header
        assert_eq!(rects[2].height, 1); // footer
        assert_eq!(rects[1].height, 19); // content = 24 - 4 - 1
    }

    #[test]
    fn flex_layout_y_contiguous() {
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        let a = FixedHeight(3);
        let b = FixedHeight(0);
        let c = FixedHeight(2);

        let rects = flex_layout(
            &[
                FlexItem {
                    renderable: &a,
                    flex: None,
                    min_height: 0,
                },
                FlexItem {
                    renderable: &b,
                    flex: Some(1),
                    min_height: 0,
                },
                FlexItem {
                    renderable: &c,
                    flex: None,
                    min_height: 0,
                },
            ],
            area,
            &mut buf,
        );
        assert_eq!(rects[0].y, 0);
        assert_eq!(rects[1].y, 3);
        assert_eq!(rects[2].y, rects[1].y + rects[1].height);
    }
}
