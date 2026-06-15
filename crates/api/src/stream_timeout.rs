//! Per-item idle timeout wrapper for streaming responses.
//!
//! Bounds the gap between successive stream items without capping the total
//! response duration, so a long extended-thinking turn is never cut off while a
//! genuinely stalled connection still fails fast.

use std::time::Duration;

use futures::stream::{Stream, StreamExt};

use crate::error::ApiError;
use crate::types::StreamEvent;

/// Wrap a `StreamEvent` stream so that more than `idle` elapsing between items
/// yields a single `ApiError::Timeout` and ends the stream.
pub fn with_idle_timeout<'a, S>(
    stream: S,
    idle: Duration,
) -> impl Stream<Item = Result<StreamEvent, ApiError>> + Send + 'a
where
    S: Stream<Item = Result<StreamEvent, ApiError>> + Send + 'a,
{
    futures::stream::unfold(
        (Box::pin(stream), false),
        move |(mut stream, done)| async move {
            if done {
                return None;
            }
            match tokio::time::timeout(idle, stream.next()).await {
                Ok(Some(item)) => Some((item, (stream, false))),
                Ok(None) => None,
                Err(_) => {
                    tracing::debug!(idle_ms = idle.as_millis(), "stream idle timeout exceeded");
                    Some((Err(ApiError::Timeout), (stream, true)))
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_core::model::TokenUsage;
    use futures::stream;

    #[tokio::test]
    async fn passes_through_when_no_stall() {
        let inner = stream::iter(vec![
            Ok(StreamEvent::MessageStop),
            Ok(StreamEvent::MessageStop),
        ]);
        let wrapped = with_idle_timeout(inner, Duration::from_secs(1));
        let collected: Vec<_> = wrapped.collect().await;
        assert_eq!(collected.len(), 2);
        assert!(collected.iter().all(std::result::Result::is_ok));
    }

    #[tokio::test]
    async fn emits_timeout_on_stall() {
        let inner = stream::once(async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(StreamEvent::MessageStop)
        });
        let wrapped = with_idle_timeout(inner, Duration::from_millis(20));
        let collected: Vec<_> = wrapped.collect().await;
        assert_eq!(collected.len(), 1);
        assert!(matches!(collected[0], Err(ApiError::Timeout)));
    }

    #[tokio::test]
    async fn timeout_ends_stream() {
        let inner = stream::once(async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(StreamEvent::MessageStart {
                id: "m".into(),
                usage: TokenUsage::default(),
            })
        });
        let wrapped = with_idle_timeout(inner, Duration::from_millis(20));
        let collected: Vec<_> = wrapped.collect().await;
        // Only the timeout item; the slow inner item is never surfaced.
        assert_eq!(collected.len(), 1);
        assert!(matches!(collected[0], Err(ApiError::Timeout)));
    }
}
