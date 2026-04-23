//! IPC message protocol between CLI and daemon.
//!
//! Wire format: `[4 bytes: payload_len_le32][payload_json]`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// CLI → Daemon request messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonRequest {
    /// Create a new session or attach to an existing one.
    Attach {
        session_id: Option<String>,
        working_dir: PathBuf,
    },
    /// Disconnect but keep the session running in background.
    Detach { session_id: String },
    /// List all active sessions.
    ListSessions,
    /// Terminate a session.
    KillSession { session_id: String },
    /// Send user input to a session.
    UserInput { session_id: String, content: String },
    /// Health check.
    Ping,
    /// Request graceful shutdown.
    Shutdown,
}

/// Information about a single session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub working_dir: PathBuf,
    pub attached: bool,
    pub created_at_secs: u64,
    pub idle_secs: u64,
}

/// Daemon → CLI response / event push messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonResponse {
    /// Attach succeeded.
    Attached { session_id: String },
    /// Session list.
    Sessions { list: Vec<SessionInfo> },
    /// Forwarded agent event (streamed).
    Event { payload: String },
    /// Error response.
    Error { message: String },
    /// Health check reply.
    Pong,
    /// Shutdown acknowledgement.
    ShuttingDown,
}

/// Encode a message as length-prefixed JSON bytes.
pub fn encode_message<T: Serialize>(msg: &T) -> crab_core::Result<Vec<u8>> {
    let json = serde_json::to_vec(msg)
        .map_err(|e| crab_core::Error::Other(format!("IPC encode error: {e}")))?;
    let len = u32::try_from(json.len())
        .map_err(|_| crab_core::Error::Other("IPC message too large".into()))?;
    let mut buf = Vec::with_capacity(4 + json.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&json);
    Ok(buf)
}

/// Decode a length-prefixed JSON message from a byte buffer.
///
/// Returns the deserialized message and the number of bytes consumed.
/// Returns `Ok(None)` if the buffer doesn't contain a complete message yet.
pub fn decode_message<T: for<'de> Deserialize<'de>>(
    buf: &[u8],
) -> crab_core::Result<Option<(T, usize)>> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let total = 4 + len;
    if buf.len() < total {
        return Ok(None);
    }
    let msg: T = serde_json::from_slice(&buf[4..total])
        .map_err(|e| crab_core::Error::Other(format!("IPC decode error: {e}")))?;
    Ok(Some((msg, total)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_ping() {
        let req = DaemonRequest::Ping;
        let encoded = encode_message(&req).unwrap();
        assert!(encoded.len() > 4);

        let (decoded, consumed): (DaemonRequest, usize) =
            decode_message(&encoded).unwrap().unwrap();
        assert_eq!(consumed, encoded.len());
        assert!(matches!(decoded, DaemonRequest::Ping));
    }

    #[test]
    fn encode_decode_attach() {
        let req = DaemonRequest::Attach {
            session_id: Some("sess-1".into()),
            working_dir: PathBuf::from("/tmp/project"),
        };
        let encoded = encode_message(&req).unwrap();
        let (decoded, _): (DaemonRequest, usize) = decode_message(&encoded).unwrap().unwrap();
        match decoded {
            DaemonRequest::Attach {
                session_id,
                working_dir,
            } => {
                assert_eq!(session_id.as_deref(), Some("sess-1"));
                assert_eq!(working_dir, PathBuf::from("/tmp/project"));
            }
            _ => panic!("expected Attach"),
        }
    }

    #[test]
    fn encode_decode_response_sessions() {
        let resp = DaemonResponse::Sessions {
            list: vec![SessionInfo {
                id: "s1".into(),
                working_dir: PathBuf::from("/home/user"),
                attached: true,
                created_at_secs: 1000,
                idle_secs: 5,
            }],
        };
        let encoded = encode_message(&resp).unwrap();
        let (decoded, _): (DaemonResponse, usize) = decode_message(&encoded).unwrap().unwrap();
        match decoded {
            DaemonResponse::Sessions { list } => {
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].id, "s1");
                assert!(list[0].attached);
            }
            _ => panic!("expected Sessions"),
        }
    }

    #[test]
    fn decode_incomplete_buffer_returns_none() {
        // Too short for length header
        let result: crab_core::Result<Option<(DaemonRequest, usize)>> = decode_message(&[0, 0]);
        assert!(result.unwrap().is_none());

        // Length says 100 bytes but only 10 available
        let mut buf = vec![100, 0, 0, 0]; // len = 100
        buf.extend_from_slice(&[0u8; 10]);
        let result: crab_core::Result<Option<(DaemonRequest, usize)>> = decode_message(&buf);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn decode_invalid_json_returns_error() {
        let mut buf = vec![5, 0, 0, 0]; // len = 5
        buf.extend_from_slice(b"hello"); // not valid JSON
        let result: crab_core::Result<Option<(DaemonRequest, usize)>> = decode_message(&buf);
        assert!(result.is_err());
    }

    #[test]
    fn encode_decode_all_request_variants() {
        let variants: Vec<DaemonRequest> = vec![
            DaemonRequest::Ping,
            DaemonRequest::Shutdown,
            DaemonRequest::ListSessions,
            DaemonRequest::Detach {
                session_id: "s1".into(),
            },
            DaemonRequest::KillSession {
                session_id: "s2".into(),
            },
            DaemonRequest::UserInput {
                session_id: "s3".into(),
                content: "hello".into(),
            },
        ];
        for req in variants {
            let encoded = encode_message(&req).unwrap();
            let decoded: Option<(DaemonRequest, usize)> = decode_message(&encoded).unwrap();
            assert!(decoded.is_some());
        }
    }

    #[test]
    fn encode_decode_all_response_variants() {
        let variants: Vec<DaemonResponse> = vec![
            DaemonResponse::Pong,
            DaemonResponse::ShuttingDown,
            DaemonResponse::Attached {
                session_id: "s1".into(),
            },
            DaemonResponse::Error {
                message: "oops".into(),
            },
            DaemonResponse::Event {
                payload: "{}".into(),
            },
            DaemonResponse::Sessions { list: vec![] },
        ];
        for resp in variants {
            let encoded = encode_message(&resp).unwrap();
            let decoded: Option<(DaemonResponse, usize)> = decode_message(&encoded).unwrap();
            assert!(decoded.is_some());
        }
    }

    #[test]
    fn session_info_serde_roundtrip() {
        let info = SessionInfo {
            id: "test-session".into(),
            working_dir: PathBuf::from("/tmp"),
            attached: false,
            created_at_secs: 12345,
            idle_secs: 60,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "test-session");
        assert_eq!(back.idle_secs, 60);
    }
}
