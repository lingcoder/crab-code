use crab_core::event::Event;
use tokio::sync::mpsc;

/// Messages exchanged between agents via the bus.
#[derive(Debug, Clone)]
pub enum AgentMessage {
    AssignTask { task_id: String, prompt: String },
    TaskComplete { task_id: String, result: String },
    RequestHelp { from: String, message: String },
    Shutdown,
}

/// Inter-agent message bus backed by tokio mpsc channels.
pub struct MessageBus {
    pub tx: mpsc::Sender<AgentMessage>,
    pub rx: mpsc::Receiver<AgentMessage>,
}

impl MessageBus {
    pub fn new(buffer: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        Self { tx, rx }
    }

    pub fn sender(&self) -> mpsc::Sender<AgentMessage> {
        self.tx.clone()
    }
}

/// Create an event channel for agent-to-TUI communication.
///
/// Returns `(sender, receiver)` with the given buffer size.
pub fn event_channel(buffer: usize) -> (mpsc::Sender<Event>, mpsc::Receiver<Event>) {
    mpsc::channel(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_bus_creation() {
        let bus = MessageBus::new(16);
        // Can create a sender clone
        let _tx = bus.sender();
    }

    #[tokio::test]
    async fn message_bus_send_receive() {
        let mut bus = MessageBus::new(16);
        let tx = bus.sender();
        tx.send(AgentMessage::Shutdown).await.unwrap();
        let msg = bus.rx.recv().await.unwrap();
        assert!(matches!(msg, AgentMessage::Shutdown));
    }

    #[tokio::test]
    async fn message_bus_assign_task() {
        let mut bus = MessageBus::new(16);
        let tx = bus.sender();
        tx.send(AgentMessage::AssignTask {
            task_id: "t1".into(),
            prompt: "do stuff".into(),
        })
        .await
        .unwrap();
        let msg = bus.rx.recv().await.unwrap();
        match msg {
            AgentMessage::AssignTask { task_id, prompt } => {
                assert_eq!(task_id, "t1");
                assert_eq!(prompt, "do stuff");
            }
            _ => panic!("expected AssignTask"),
        }
    }

    #[tokio::test]
    async fn event_channel_send_receive() {
        let (tx, mut rx) = event_channel(16);
        tx.send(crab_core::event::Event::TurnStart { turn_index: 0 })
            .await
            .unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(
            event,
            crab_core::event::Event::TurnStart { turn_index: 0 }
        ));
    }
}
