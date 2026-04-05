//! TUI event system — merges crossterm terminal events with agent domain events.

use crab_core::event::Event as AgentEvent;
use crossterm::event::{Event as CtEvent, KeyEvent};
use tokio::sync::mpsc;

/// Events consumed by the TUI main loop.
#[derive(Debug)]
pub enum TuiEvent {
    /// Keyboard event from the terminal.
    Key(KeyEvent),
    /// Terminal resize.
    Resize { width: u16, height: u16 },
    /// Agent domain event (tool use, content delta, etc.).
    Agent(AgentEvent),
    /// Periodic tick for animations (spinner, etc.).
    Tick,
}

/// Spawn the event loop that merges crossterm and agent events into a single channel.
///
/// Returns the receiver. The caller should also hold the `agent_tx` sender and
/// forward `AgentEvent`s into it from the agent task.
///
/// The loop runs until the returned receiver is dropped.
pub fn spawn_event_loop(
    agent_rx: mpsc::UnboundedReceiver<AgentEvent>,
    tick_rate: std::time::Duration,
) -> mpsc::UnboundedReceiver<TuiEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Crossterm reader task
    let ct_tx = tx.clone();
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut reader = crossterm::event::EventStream::new();
        while let Some(Ok(event)) = reader.next().await {
            let tui_event = match event {
                CtEvent::Key(key) => TuiEvent::Key(key),
                CtEvent::Resize(w, h) => TuiEvent::Resize {
                    width: w,
                    height: h,
                },
                // Ignore mouse, focus, paste events for now
                _ => continue,
            };
            if ct_tx.send(tui_event).is_err() {
                break;
            }
        }
    });

    // Agent event forwarder task
    let agent_tx = tx.clone();
    tokio::spawn(async move {
        let mut agent_rx = agent_rx;
        while let Some(event) = agent_rx.recv().await {
            if agent_tx.send(TuiEvent::Agent(event)).is_err() {
                break;
            }
        }
    });

    // Tick timer task
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick_rate);
        loop {
            interval.tick().await;
            if tx.send(TuiEvent::Tick).is_err() {
                break;
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn tui_event_key_variant() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let event = TuiEvent::Key(key);
        assert!(matches!(event, TuiEvent::Key(_)));
    }

    #[test]
    fn tui_event_resize_variant() {
        let event = TuiEvent::Resize {
            width: 120,
            height: 40,
        };
        if let TuiEvent::Resize { width, height } = event {
            assert_eq!(width, 120);
            assert_eq!(height, 40);
        } else {
            panic!("expected Resize");
        }
    }

    #[test]
    fn tui_event_agent_variant() {
        let agent = AgentEvent::ContentDelta {
            index: 0,
            delta: "hello".into(),
        };
        let event = TuiEvent::Agent(agent);
        assert!(matches!(event, TuiEvent::Agent(_)));
    }

    #[test]
    fn tui_event_tick_variant() {
        let event = TuiEvent::Tick;
        assert!(matches!(event, TuiEvent::Tick));
    }

    #[test]
    fn tui_event_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<TuiEvent>();
    }

    #[tokio::test]
    async fn spawn_event_loop_receives_agent_events() {
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        let mut tui_rx = spawn_event_loop(agent_rx, std::time::Duration::from_secs(60));

        agent_tx
            .send(AgentEvent::ContentDelta {
                index: 0,
                delta: "test".into(),
            })
            .unwrap();

        // Should receive the agent event
        let event = tokio::time::timeout(std::time::Duration::from_millis(500), tui_rx.recv())
            .await
            .expect("timeout");

        // Could be a Tick or Agent event — keep consuming until we get Agent
        let mut found_agent = matches!(event, Some(TuiEvent::Agent(_)));
        if !found_agent {
            for _ in 0..10 {
                if let Ok(Some(e)) =
                    tokio::time::timeout(std::time::Duration::from_millis(100), tui_rx.recv()).await
                {
                    if matches!(e, TuiEvent::Agent(_)) {
                        found_agent = true;
                        break;
                    }
                }
            }
        }
        assert!(found_agent, "expected to receive an Agent event");
    }

    #[tokio::test]
    async fn spawn_event_loop_receives_ticks() {
        let (_agent_tx, agent_rx) = mpsc::unbounded_channel();
        let mut tui_rx = spawn_event_loop(agent_rx, std::time::Duration::from_millis(50));

        // Wait for a tick
        let mut found_tick = false;
        for _ in 0..20 {
            if let Ok(Some(e)) =
                tokio::time::timeout(std::time::Duration::from_millis(100), tui_rx.recv()).await
            {
                if matches!(e, TuiEvent::Tick) {
                    found_tick = true;
                    break;
                }
            }
        }
        assert!(found_tick, "expected to receive a Tick event");
    }
}
