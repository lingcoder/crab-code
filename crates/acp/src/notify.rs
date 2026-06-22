//! Forward crab engine events to the ACP client as session
//! notifications.

use agent_client_protocol::schema::v1::{
    AgentNotification, ContentBlock, ContentChunk, SessionId, SessionNotification, SessionUpdate,
    TextContent,
};
use agent_client_protocol::{Client, ConnectionTo};
use crab_core::event::{Event, TOOL_ARG_INDEX_BASE};
use tokio::sync::mpsc;

/// Drain crab [`Event`]s and forward ACP-relevant ones as
/// `AgentNotification::SessionNotification` frames.
pub async fn run_event_bridge(
    session_id: SessionId,
    mut event_rx: mpsc::Receiver<Event>,
    cx: ConnectionTo<Client>,
) {
    while let Some(event) = event_rx.recv().await {
        if let Some(update) = event_to_update(&event) {
            let notif = SessionNotification::new(session_id.clone(), update);
            if cx
                .send_notification(AgentNotification::SessionNotification(notif))
                .is_err()
            {
                break;
            }
        }
    }
}

/// Map a crab [`Event`] onto an ACP [`SessionUpdate`], returning `None`
/// for events with no ACP counterpart yet. Content deltas carrying raw
/// tool-call argument JSON (index >= `TOOL_ARG_INDEX_BASE`) are dropped
/// per the `crab-core` event contract.
pub fn event_to_update(event: &Event) -> Option<SessionUpdate> {
    match event {
        Event::ContentDelta { index, delta } if *index < TOOL_ARG_INDEX_BASE => {
            Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::Text(TextContent::new(delta.clone())),
            )))
        }
        Event::ThinkingDelta { delta, .. } => Some(SessionUpdate::AgentThoughtChunk(
            ContentChunk::new(ContentBlock::Text(TextContent::new(delta.clone()))),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_text(update: &SessionUpdate) -> Option<&str> {
        let (SessionUpdate::AgentMessageChunk(chunk) | SessionUpdate::AgentThoughtChunk(chunk)) =
            update
        else {
            return None;
        };
        match &chunk.content {
            ContentBlock::Text(t) => Some(&t.text),
            _ => None,
        }
    }

    #[test]
    fn content_delta_becomes_agent_message_chunk() {
        let update = event_to_update(&Event::ContentDelta {
            index: 0,
            delta: "hello".into(),
        })
        .expect("content delta maps to an update");
        assert!(matches!(update, SessionUpdate::AgentMessageChunk(_)));
        assert_eq!(chunk_text(&update), Some("hello"));
    }

    #[test]
    fn thinking_delta_becomes_agent_thought_chunk() {
        let update = event_to_update(&Event::ThinkingDelta {
            index: 0,
            delta: "hmm".into(),
        })
        .expect("thinking delta maps to an update");
        assert!(matches!(update, SessionUpdate::AgentThoughtChunk(_)));
        assert_eq!(chunk_text(&update), Some("hmm"));
    }

    #[test]
    fn tool_arg_content_delta_is_filtered() {
        let update = event_to_update(&Event::ContentDelta {
            index: TOOL_ARG_INDEX_BASE,
            delta: "{\"path\":".into(),
        });
        assert!(update.is_none());
    }

    #[test]
    fn unrelated_events_have_no_acp_counterpart() {
        assert!(event_to_update(&Event::ContentBlockStop { index: 0 }).is_none());
    }
}
