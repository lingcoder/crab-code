#[derive(Debug, Clone)]
pub struct AutoScrollState {
    pub user_scrolled_away: bool,
    pub unseen_count: usize,
}

impl AutoScrollState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_scrolled_away: false,
            unseen_count: 0,
        }
    }

    pub fn on_new_message(&mut self) {
        if self.user_scrolled_away {
            self.unseen_count += 1;
        }
    }

    pub fn on_scroll_to_bottom(&mut self) {
        self.user_scrolled_away = false;
        self.unseen_count = 0;
    }

    pub fn on_user_scroll_up(&mut self) {
        self.user_scrolled_away = true;
    }

    #[must_use]
    pub fn should_auto_scroll(&self) -> bool {
        !self.user_scrolled_away
    }

    #[must_use]
    pub fn pill_text(&self) -> Option<String> {
        if self.unseen_count > 0 {
            Some(format!("{} new messages", self.unseen_count))
        } else {
            None
        }
    }
}

impl Default for AutoScrollState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_scrolls_by_default() {
        let state = AutoScrollState::new();
        assert!(state.should_auto_scroll());
        assert!(state.pill_text().is_none());
    }

    #[test]
    fn scroll_away_disables_auto() {
        let mut state = AutoScrollState::new();
        state.on_user_scroll_up();
        assert!(!state.should_auto_scroll());
    }

    #[test]
    fn unseen_count_accumulates() {
        let mut state = AutoScrollState::new();
        state.on_user_scroll_up();
        state.on_new_message();
        state.on_new_message();
        assert_eq!(state.unseen_count, 2);
        assert!(state.pill_text().unwrap().contains('2'));
    }

    #[test]
    fn scroll_to_bottom_resets() {
        let mut state = AutoScrollState::new();
        state.on_user_scroll_up();
        state.on_new_message();
        state.on_scroll_to_bottom();
        assert!(state.should_auto_scroll());
        assert_eq!(state.unseen_count, 0);
    }

    #[test]
    fn no_pill_when_not_scrolled_away() {
        let mut state = AutoScrollState::new();
        state.on_new_message();
        assert!(state.pill_text().is_none());
    }
}
