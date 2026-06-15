//! AWS Bedrock Runtime response-stream decoding.
//!
//! `InvokeModelWithResponseStream` returns the `vnd.amazon.eventstream` binary
//! framing (prelude + CRC + headers + payload), not plain SSE. Each normal
//! frame carries `:message-type: event` / `:event-type: chunk` and a JSON
//! payload `{"bytes": "<base64>"}`; the decoded base64 is an Anthropic Messages
//! SSE event in the same family the native client already parses. Error frames
//! carry `:message-type: exception`.
//!
//! Framing is decoded with AWS's own `aws-smithy-eventstream` crate rather than
//! a hand-rolled parser.

use aws_smithy_eventstream::frame::{DecodedFrame, MessageFrameDecoder};
use aws_smithy_types::event_stream::{HeaderValue, Message};
use base64::Engine as _;
use bytes::{Bytes, BytesMut};
use futures::stream::{Stream, StreamExt};

use super::convert;
use super::types::AnthropicSseEvent;
use crate::error::ApiError;
use crate::types::StreamEvent;

/// Decode a raw Bedrock event-stream byte stream into `StreamEvent`s.
///
/// `bytes_stream` is the reqwest response body. Bytes are buffered until whole
/// frames are available; each frame is decoded, classified by its
/// `:message-type` header, and mapped to a `StreamEvent` (or an `ApiError` for
/// exception frames / malformed data).
pub fn decode_bedrock_stream<S>(
    bytes_stream: S,
) -> impl Stream<Item = Result<StreamEvent, ApiError>> + Send
where
    S: Stream<Item = reqwest::Result<Bytes>> + Send + Unpin,
{
    struct State<S> {
        stream: S,
        decoder: MessageFrameDecoder,
        buffer: BytesMut,
        upstream_done: bool,
    }

    let state = State {
        stream: bytes_stream,
        decoder: MessageFrameDecoder::new(),
        buffer: BytesMut::new(),
        upstream_done: false,
    };

    futures::stream::unfold(state, |mut state| async move {
        loop {
            // A prior fatal item set this; end the stream.
            if state.upstream_done && state.buffer.is_empty() {
                return None;
            }

            // Try to decode a complete frame from whatever is already buffered.
            match state.decoder.decode_frame(&mut state.buffer) {
                Ok(DecodedFrame::Complete(message)) => {
                    if let Some(item) = frame_to_stream_event(&message) {
                        return Some((item, state));
                    }
                    // Frame carried nothing to surface (ping/unknown); keep going.
                    continue;
                }
                Ok(DecodedFrame::Incomplete) => {}
                Err(e) => {
                    state.upstream_done = true;
                    state.buffer.clear();
                    return Some((
                        Err(ApiError::Sse(format!(
                            "Bedrock event-stream decode error: {e}"
                        ))),
                        state,
                    ));
                }
            }

            if state.upstream_done {
                return None;
            }

            match state.stream.next().await {
                Some(Ok(chunk)) => {
                    state.buffer.extend_from_slice(&chunk);
                }
                Some(Err(e)) => {
                    state.upstream_done = true;
                    state.buffer.clear();
                    return Some((Err(ApiError::Http(e)), state));
                }
                None => {
                    state.upstream_done = true;
                }
            }
        }
    })
}

/// Map one decoded event-stream frame to a `StreamEvent`.
///
/// Returns `None` for frames with no surfaced event (e.g. an unknown
/// `:event-type`, or a ping/keepalive). Exception frames and malformed chunk
/// payloads return `Some(Err(..))`.
fn frame_to_stream_event(message: &Message) -> Option<Result<StreamEvent, ApiError>> {
    let message_type = header_str(message, ":message-type").unwrap_or("event");

    if message_type == "exception" || message_type == "error" {
        return Some(Err(exception_error(message)));
    }

    let event_type = header_str(message, ":event-type").unwrap_or_default();
    if event_type != "chunk" {
        tracing::debug!(event_type, "skipping non-chunk Bedrock event-stream frame");
        return None;
    }

    match anthropic_event_from_chunk(message.payload()) {
        Ok(sse) => convert::sse_event_to_stream_event(sse).map(Ok),
        Err(e) => Some(Err(e)),
    }
}

/// Extract the Anthropic SSE event from a `chunk` frame payload.
///
/// The payload is `{"bytes": "<base64>"}`; the decoded base64 is the event
/// JSON. Unknown event JSON shapes are tolerated by `AnthropicSseEvent`'s
/// `#[serde(other)]` fallback, so they deserialize rather than erroring.
fn anthropic_event_from_chunk(payload: &Bytes) -> Result<AnthropicSseEvent, ApiError> {
    let outer: serde_json::Value = serde_json::from_slice(payload).map_err(ApiError::Json)?;
    let b64 = outer
        .get("bytes")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::Sse("Bedrock chunk payload missing 'bytes' field".to_string()))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| ApiError::Sse(format!("invalid base64 in Bedrock chunk: {e}")))?;
    serde_json::from_slice(&decoded).map_err(ApiError::Json)
}

/// Build an `ApiError` from an exception frame, mapping throttling to
/// `RateLimited` and everything else to `Api`.
fn exception_error(message: &Message) -> ApiError {
    let exception_type = header_str(message, ":exception-type")
        .or_else(|| header_str(message, ":error-code"))
        .unwrap_or("unknown");
    let detail = std::str::from_utf8(message.payload())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();

    if exception_type.eq_ignore_ascii_case("throttlingException") {
        return ApiError::RateLimited { retry_after_ms: 0 };
    }
    ApiError::Api {
        status: 0,
        message: format!("Bedrock {exception_type}: {detail}"),
    }
}

/// Read a string-valued event-stream header by name.
fn header_str<'a>(message: &'a Message, name: &str) -> Option<&'a str> {
    message
        .headers()
        .iter()
        .find(|h| h.name().as_str() == name)
        .and_then(|h| match h.value() {
            HeaderValue::String(s) => Some(s.as_str()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_eventstream::frame::write_message_to;
    use aws_smithy_types::event_stream::Header;

    /// Encode a `chunk` event frame wrapping the given Anthropic event JSON.
    fn chunk_frame(event_json: &str) -> Bytes {
        let inner_b64 = base64::engine::general_purpose::STANDARD.encode(event_json);
        let payload = serde_json::json!({ "bytes": inner_b64 }).to_string();
        let msg = Message::new(Bytes::from(payload))
            .add_header(Header::new(
                ":message-type",
                HeaderValue::String("event".into()),
            ))
            .add_header(Header::new(
                ":event-type",
                HeaderValue::String("chunk".into()),
            ))
            .add_header(Header::new(
                ":content-type",
                HeaderValue::String("application/json".into()),
            ));
        let mut buf = Vec::new();
        write_message_to(&msg, &mut buf).unwrap();
        Bytes::from(buf)
    }

    /// Encode an exception frame.
    fn exception_frame(exception_type: &str, message_body: &str) -> Bytes {
        let msg = Message::new(Bytes::from(message_body.to_string()))
            .add_header(Header::new(
                ":message-type",
                HeaderValue::String("exception".into()),
            ))
            .add_header(Header::new(
                ":exception-type",
                HeaderValue::String(exception_type.to_string().into()),
            ));
        let mut buf = Vec::new();
        write_message_to(&msg, &mut buf).unwrap();
        Bytes::from(buf)
    }

    async fn collect(frames: Vec<Bytes>) -> Vec<Result<StreamEvent, ApiError>> {
        let stream = futures::stream::iter(frames.into_iter().map(reqwest::Result::Ok));
        decode_bedrock_stream(stream).collect().await
    }

    #[tokio::test]
    async fn decodes_text_delta_chunk() {
        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let out = collect(vec![chunk_frame(event)]).await;
        assert_eq!(out.len(), 1);
        assert!(matches!(
            &out[0],
            Ok(StreamEvent::ContentDelta { delta, .. }) if delta == "Hello"
        ));
    }

    #[tokio::test]
    async fn reassembles_full_text_across_frames() {
        let frames = vec![
            chunk_frame(
                r#"{"type":"message_start","message":{"id":"m","model":"c","usage":{"input_tokens":1,"output_tokens":0}}}"#,
            ),
            chunk_frame(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello "}}"#,
            ),
            chunk_frame(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"world"}}"#,
            ),
            chunk_frame(r#"{"type":"message_stop"}"#),
        ];
        let out = collect(frames).await;
        let text: String = out
            .into_iter()
            .filter_map(|r| match r {
                Ok(StreamEvent::ContentDelta { delta, .. }) => Some(delta),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello world");
    }

    #[tokio::test]
    async fn frame_split_across_byte_chunks_still_decodes() {
        let frame = chunk_frame(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"split"}}"#,
        );
        // Deliver the frame one byte at a time to exercise the incremental buffer.
        let pieces: Vec<Bytes> = frame
            .iter()
            .map(|b| Bytes::copy_from_slice(&[*b]))
            .collect();
        let out = collect(pieces).await;
        let deltas: Vec<_> = out
            .iter()
            .filter_map(|r| match r {
                Ok(StreamEvent::ContentDelta { delta, .. }) => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["split"]);
    }

    #[tokio::test]
    async fn throttling_exception_maps_to_rate_limited() {
        let out = collect(vec![exception_frame(
            "throttlingException",
            r#"{"message":"slow down"}"#,
        )])
        .await;
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Err(ApiError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn other_exception_maps_to_api_error() {
        let out = collect(vec![exception_frame(
            "validationException",
            r#"{"message":"bad input"}"#,
        )])
        .await;
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Err(ApiError::Api { message, .. })
                if message.contains("validationException") && message.contains("bad input")));
    }

    #[tokio::test]
    async fn unknown_event_chunk_is_skipped_not_fatal() {
        let frames = vec![
            chunk_frame(r#"{"type":"some_future_event","x":1}"#),
            chunk_frame(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
            ),
        ];
        let out = collect(frames).await;
        let deltas: Vec<_> = out
            .iter()
            .filter_map(|r| match r {
                Ok(StreamEvent::ContentDelta { delta, .. }) => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["ok"]);
    }
}
