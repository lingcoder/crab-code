//! Frame requester — broadcast channel to signal redraws.
//!
//! Components that need a redraw (e.g., after a state change from a background
//! task) call `request_frame()` to wake the render loop without tight coupling.

use tokio::sync::broadcast;

/// Broadcast-based redraw signal.
///
/// Clone the `FrameRequester` to share it across tasks. Call `request_frame()`
/// to signal that a new frame should be rendered.
#[derive(Clone)]
pub struct FrameRequester {
    tx: broadcast::Sender<()>,
}

impl FrameRequester {
    /// Create a new frame requester with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Signal that a new frame should be rendered.
    pub fn request_frame(&self) {
        // Ignore errors — receivers may have been dropped.
        let _ = self.tx.send(());
    }

    /// Subscribe to frame requests.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }
}

impl Default for FrameRequester {
    fn default() -> Self {
        Self::new(16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_requester_send_does_not_panic() {
        let fr = FrameRequester::new(4);
        fr.request_frame(); // no receivers — should not panic
    }

    #[tokio::test]
    async fn frame_requester_receives_signal() {
        let fr = FrameRequester::new(4);
        let mut rx = fr.subscribe();

        fr.request_frame();
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(result.is_ok());
    }

    #[test]
    fn frame_requester_clone() {
        let fr = FrameRequester::default();
        let fr2 = fr.clone();
        let _rx = fr.subscribe();
        fr2.request_frame();
    }
}
