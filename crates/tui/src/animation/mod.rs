//! Frame-driven animation primitives.
//!
//! The UI layer describes animations declaratively; the runner calls
//! [`FrameScheduler::tick`] each frame to advance subscribers and
//! decide whether to request another redraw.
//!
//! Scheduler policy:
//!
//! - When no subscribers are active, the scheduler does nothing and the
//!   runner's idle redraw rate applies.
//! - When subscribers are active, [`FrameScheduler::should_tick`] returns
//!   `true` at most once per `min_interval` (`16ms` → ~60fps by default).
//! - Subscribers are indexed by `Ticket`; dropping the ticket deregisters
//!   automatically, so widgets can own their subscription without extra
//!   bookkeeping.

pub mod shimmer;
pub mod spinner;

pub use shimmer::{ShimmerState, ShimmerSubscriber};
pub use spinner::{BRAILLE_FRAMES, DOTS_FRAMES, LINE_FRAMES, Spinner, SpinnerStyle};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Hard cap for active-animation refresh.
pub const MAX_FPS_INTERVAL: Duration = Duration::from_millis(16);
/// Idle refresh interval when animations are quiescent.
pub const IDLE_FPS_INTERVAL: Duration = Duration::from_millis(100);

/// Opaque handle into the scheduler. Dropping it deregisters.
pub struct Ticket {
    alive: Arc<AtomicBool>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

struct Subscriber {
    alive: Arc<AtomicBool>,
}

/// Schedules animation frames and tracks whether a redraw is due.
pub struct FrameScheduler {
    subscribers: Mutex<Vec<Subscriber>>,
    next_id: AtomicUsize,
    last_tick: Mutex<Option<Instant>>,
    min_interval: Duration,
    idle_interval: Duration,
}

impl FrameScheduler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            next_id: AtomicUsize::new(0),
            last_tick: Mutex::new(None),
            min_interval: MAX_FPS_INTERVAL,
            idle_interval: IDLE_FPS_INTERVAL,
        }
    }

    /// Register a new subscriber; the returned ticket deregisters on drop.
    pub fn subscribe(&self) -> Ticket {
        let _ = self.next_id.fetch_add(1, Ordering::SeqCst);
        let alive = Arc::new(AtomicBool::new(true));
        self.subscribers
            .lock()
            .expect("scheduler lock")
            .push(Subscriber {
                alive: alive.clone(),
            });
        Ticket { alive }
    }

    /// Number of live subscribers right now.
    pub fn active_subscribers(&self) -> usize {
        self.sweep_and_count()
    }

    fn sweep_and_count(&self) -> usize {
        let mut subs = self.subscribers.lock().expect("scheduler lock");
        subs.retain(|s| s.alive.load(Ordering::SeqCst));
        subs.len()
    }

    /// Test whether enough time has elapsed since the last tick to
    /// advance the animation clock. Does not move the clock forward.
    pub fn should_tick(&self, now: Instant) -> bool {
        let last = self.last_tick.lock().expect("scheduler lock");
        let interval = if self.sweep_and_count() == 0 {
            self.idle_interval
        } else {
            self.min_interval
        };
        match *last {
            None => true,
            Some(last) => now.duration_since(last) >= interval,
        }
    }

    /// Advance the animation clock if due; returns `true` when callers
    /// should issue a redraw.
    pub fn tick(&self, now: Instant) -> bool {
        if !self.should_tick(now) {
            return false;
        }
        *self.last_tick.lock().expect("scheduler lock") = Some(now);
        self.sweep_and_count() > 0
    }
}

impl Default for FrameScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scheduler_ticks_but_requests_no_redraw() {
        let scheduler = FrameScheduler::new();
        let t0 = Instant::now();
        assert!(scheduler.should_tick(t0));
        assert!(!scheduler.tick(t0));
    }

    #[test]
    fn subscribed_scheduler_requests_redraw() {
        let scheduler = FrameScheduler::new();
        let _ticket = scheduler.subscribe();
        assert_eq!(scheduler.active_subscribers(), 1);

        let t0 = Instant::now();
        assert!(scheduler.tick(t0));
    }

    #[test]
    fn ticket_drop_deregisters() {
        let scheduler = FrameScheduler::new();
        {
            let _ticket = scheduler.subscribe();
            assert_eq!(scheduler.active_subscribers(), 1);
        }
        assert_eq!(scheduler.active_subscribers(), 0);
    }

    #[test]
    fn should_tick_respects_min_interval() {
        let scheduler = FrameScheduler::new();
        let _ticket = scheduler.subscribe();

        let t0 = Instant::now();
        assert!(scheduler.tick(t0));
        let t1 = t0 + Duration::from_millis(1);
        assert!(!scheduler.should_tick(t1));

        let t2 = t0 + Duration::from_millis(20);
        assert!(scheduler.should_tick(t2));
    }

    #[test]
    fn multiple_subscribers_count_accurately() {
        let scheduler = FrameScheduler::new();
        let a = scheduler.subscribe();
        let b = scheduler.subscribe();
        let c = scheduler.subscribe();
        assert_eq!(scheduler.active_subscribers(), 3);
        drop(b);
        assert_eq!(scheduler.active_subscribers(), 2);
        drop(a);
        drop(c);
        assert_eq!(scheduler.active_subscribers(), 0);
    }
}
