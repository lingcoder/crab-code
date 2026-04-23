use std::collections::VecDeque;

/// Queue for user commands submitted while the agent is processing.
///
/// When the user presses Enter during `AppState::Processing`, the input
/// text is pushed onto this queue instead of being sent immediately.
/// After the current agent turn completes, the runner dequeues and
/// submits the next command automatically.
#[derive(Debug, Default)]
pub struct CommandQueue {
    queue: VecDeque<String>,
}

impl CommandQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    pub fn push(&mut self, text: String) {
        self.queue.push_back(text);
    }

    pub fn pop(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order() {
        let mut q = CommandQueue::new();
        q.push("first".into());
        q.push("second".into());
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop(), Some("first".into()));
        assert_eq!(q.pop(), Some("second".into()));
        assert!(q.is_empty());
    }

    #[test]
    fn pop_empty_returns_none() {
        let mut q = CommandQueue::new();
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn clear_empties_queue() {
        let mut q = CommandQueue::new();
        q.push("a".into());
        q.push("b".into());
        q.clear();
        assert!(q.is_empty());
    }
}
