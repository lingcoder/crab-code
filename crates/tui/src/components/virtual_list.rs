//! Virtualized, width-aware list renderer for long message histories.
//!
//! Unlike [`crate::components::message_list::MessageList`], which
//! re-renders every message every frame, `VirtualMessageList` caches
//! pre-laid-out `Line`s per `(message_id, width)` and only paints the
//! slice that intersects the current viewport. This keeps scrolling
//! fluid on conversations with thousands of messages.
//!
//! The cache intentionally excludes the tail message when it is marked
//! "streaming": a streaming message's content can grow on every tick,
//! so memoizing it would be a guaranteed miss anyway and a waste of
//! memory.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Widget;

/// Default LRU capacity for laid-out messages.
pub const DEFAULT_CACHE_CAPACITY: usize = 512;

/// A renderable message with a stable identity for cache keying.
#[derive(Debug, Clone)]
pub struct VirtualMessage {
    pub id: u64,
    pub lines: Arc<Vec<Line<'static>>>,
    /// When `true`, the list will skip caching this row. Use for the
    /// streaming tail so each tick redraws fresh content.
    pub streaming: bool,
}

impl VirtualMessage {
    #[must_use]
    pub fn new(id: u64, lines: Vec<Line<'static>>) -> Self {
        Self {
            id,
            lines: Arc::new(lines),
            streaming: false,
        }
    }

    #[must_use]
    pub fn streaming(mut self) -> Self {
        self.streaming = true;
        self
    }
}

/// Scroll bookkeeping.
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewportState {
    /// Number of rows scrolled from the bottom (0 = anchored to tail).
    pub scroll_offset: usize,
}

/// Renderer state — owns the cache across frames.
pub struct VirtualMessageList {
    cache: LruCache<(u64, u16), Arc<Vec<Line<'static>>>>,
    last_total_lines: usize,
}

impl VirtualMessageList {
    #[must_use]
    pub fn new() -> Self {
        let cap = NonZeroUsize::new(DEFAULT_CACHE_CAPACITY).expect("DEFAULT_CACHE_CAPACITY > 0");
        Self {
            cache: LruCache::new(cap),
            last_total_lines: 0,
        }
    }

    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("capacity is non-zero after max(1)");
        Self {
            cache: LruCache::new(cap),
            last_total_lines: 0,
        }
    }

    /// Total number of cached `Line` rows from the last render pass.
    #[must_use]
    pub fn last_total_lines(&self) -> usize {
        self.last_total_lines
    }

    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Drop all cached layouts. Call when theme or width changes
    /// invalidate the entire cache.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    /// Flatten the cached messages into the concrete line buffer for a
    /// given rendering width.
    fn build_flat_lines(
        &mut self,
        messages: &[VirtualMessage],
        width: u16,
    ) -> Vec<Arc<Vec<Line<'static>>>> {
        let mut out = Vec::with_capacity(messages.len());
        for msg in messages {
            if msg.streaming {
                out.push(Arc::clone(&msg.lines));
                continue;
            }
            let key = (msg.id, width);
            let entry = if let Some(cached) = self.cache.get(&key) {
                Arc::clone(cached)
            } else {
                self.cache.put(key, Arc::clone(&msg.lines));
                Arc::clone(&msg.lines)
            };
            out.push(entry);
        }
        out
    }

    /// Paint the viewport. `messages` must be in chronological order;
    /// the last message is anchored to the bottom of `area` and older
    /// messages scroll off the top.
    pub fn render(
        &mut self,
        messages: &[VirtualMessage],
        state: ViewportState,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let width = area.width;
        let rendered = self.build_flat_lines(messages, width);
        let total: usize = rendered.iter().map(|l| l.len()).sum();
        self.last_total_lines = total;

        let visible = area.height as usize;
        let end = total.saturating_sub(state.scroll_offset);
        let start = end.saturating_sub(visible);

        // Walk the flattened list skipping the first `start` rows.
        let mut painted = 0usize;
        let mut skipped = 0usize;
        'outer: for chunk in &rendered {
            for line in chunk.iter() {
                let abs = skipped;
                skipped += 1;
                if abs < start {
                    continue;
                }
                if painted >= visible {
                    break 'outer;
                }
                let y = area.y + painted as u16;
                Widget::render(
                    line.clone(),
                    Rect {
                        x: area.x,
                        y,
                        width,
                        height: 1,
                    },
                    buf,
                );
                painted += 1;
                if skipped >= end {
                    break 'outer;
                }
            }
        }
    }
}

impl Default for VirtualMessageList {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: u64, lines: &[&str]) -> VirtualMessage {
        VirtualMessage::new(
            id,
            lines.iter().map(|s| Line::raw((*s).to_string())).collect(),
        )
    }

    #[test]
    fn caches_static_messages() {
        let mut list = VirtualMessageList::new();
        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);

        let messages = vec![msg(1, &["a", "b"]), msg(2, &["c", "d"])];
        list.render(&messages, ViewportState::default(), area, &mut buf);
        assert_eq!(list.cache_len(), 2);
        // Second render should re-use cache entries.
        list.render(&messages, ViewportState::default(), area, &mut buf);
        assert_eq!(list.cache_len(), 2);
    }

    #[test]
    fn does_not_cache_streaming_tail() {
        let mut list = VirtualMessageList::new();
        let area = Rect::new(0, 0, 80, 5);
        let mut buf = Buffer::empty(area);
        let messages = vec![msg(1, &["a"]), msg(2, &["b"]).streaming()];
        list.render(&messages, ViewportState::default(), area, &mut buf);
        assert_eq!(list.cache_len(), 1);
    }

    #[test]
    fn scroll_offset_walks_backwards() {
        let mut list = VirtualMessageList::new();
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        let messages = vec![msg(1, &["line-a", "line-b"]), msg(2, &["line-c", "line-d"])];
        // offset=0 → show tail (c,d); offset=1 → show (b,c).
        list.render(
            &messages,
            ViewportState { scroll_offset: 1 },
            area,
            &mut buf,
        );
        assert_eq!(list.last_total_lines(), 4);
    }

    #[test]
    fn invalidate_drops_cache() {
        let mut list = VirtualMessageList::new();
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        let messages = vec![msg(1, &["x"])];
        list.render(&messages, ViewportState::default(), area, &mut buf);
        assert_eq!(list.cache_len(), 1);
        list.invalidate();
        assert_eq!(list.cache_len(), 0);
    }

    #[test]
    fn different_widths_cache_separately() {
        let mut list = VirtualMessageList::new();
        let messages = vec![msg(1, &["line"])];

        let area_a = Rect::new(0, 0, 40, 2);
        let mut buf_a = Buffer::empty(area_a);
        list.render(&messages, ViewportState::default(), area_a, &mut buf_a);

        let area_b = Rect::new(0, 0, 80, 2);
        let mut buf_b = Buffer::empty(area_b);
        list.render(&messages, ViewportState::default(), area_b, &mut buf_b);

        assert_eq!(list.cache_len(), 2);
    }
}
