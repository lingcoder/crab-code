//! Configuration for the crab-proto server side.
//!
//! Separated from the actual listener so the composition root can construct
//! a `ServerConfig` ahead of time (loaded from `~/.crab/settings.json`
//! or env vars) and hand it to [`super::RemoteServer::serve`] when it
//! lands.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;

/// Where the server listens and how it authenticates connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Address the WebSocket listener binds to.
    ///
    /// Defaults to `127.0.0.1:4180` — loopback-only is the safe default
    /// for a local-first tool; exposing to `0.0.0.0` should be an
    /// explicit user choice (documented knob).
    pub bind: SocketAddr,

    /// HMAC-SHA256 secret used to issue / verify JWTs. Any string
    /// ≥32 bytes is fine; the host generates a random one on first run
    /// and writes it to `~/.crab/auth/jwt.secret` (0600 on unix).
    pub jwt_secret: String,

    /// Token TTL for newly issued JWTs. Short enough to contain
    /// credential leaks, long enough that users don't re-auth every
    /// five minutes. 24 h is the default — tweak if the device is
    /// known risky (e.g. a shared workstation).
    #[serde(with = "duration_secs")]
    pub jwt_ttl: Duration,

    /// How often the server sends a heartbeat frame to each connected
    /// client. Keeps NAT / load-balancer idle timers alive and gives
    /// the client a quick way to detect server liveness.
    #[serde(with = "duration_secs")]
    pub heartbeat_interval: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:4180".parse().expect("valid loopback default"),
            jwt_secret: String::new(),
            jwt_ttl: Duration::from_secs(24 * 60 * 60),
            heartbeat_interval: Duration::from_secs(30),
        }
    }
}

impl ServerConfig {
    /// Fresh config with a caller-supplied secret. `Default::default()`
    /// leaves `jwt_secret` empty on purpose — callers that forget to
    /// fill it in should hit a sharp error at startup rather than a
    /// silently-insecure runtime.
    pub fn new(jwt_secret: impl Into<String>) -> Self {
        Self {
            jwt_secret: jwt_secret.into(),
            ..Default::default()
        }
    }

    /// Validate the config is usable for real serving. Returns `Err`
    /// if the secret is too short (< 32 bytes) or empty — HMAC-SHA256
    /// is only as strong as its key.
    pub fn validate(&self) -> Result<(), ServerConfigError> {
        if self.jwt_secret.len() < 32 {
            return Err(ServerConfigError::JwtSecretTooShort {
                got: self.jwt_secret.len(),
                min: 32,
            });
        }
        Ok(())
    }
}

/// Configuration-validation errors. Separated from the broader server
/// errors because they are catchable at startup (pre-listen) and worth
/// surfacing as an `Err` variant rather than a runtime panic.
#[derive(Debug, thiserror::Error)]
pub enum ServerConfigError {
    #[error("jwt_secret must be at least {min} bytes, got {got}")]
    JwtSecretTooShort { got: usize, min: usize },
}

/// Serde helper for representing `Duration` as integer seconds on the wire.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(de)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_binds_to_loopback() {
        let c = ServerConfig::default();
        assert!(c.bind.ip().is_loopback(), "default bind must be loopback");
    }

    #[test]
    fn validate_rejects_short_secret() {
        let c = ServerConfig::new("short");
        let err = c.validate().unwrap_err();
        assert!(
            matches!(err, ServerConfigError::JwtSecretTooShort { got, min } if got == 5 && min == 32)
        );
    }

    #[test]
    fn validate_accepts_long_enough_secret() {
        let c = ServerConfig::new("a".repeat(32));
        assert!(c.validate().is_ok());
    }

    #[test]
    fn serde_roundtrip_uses_integer_seconds() {
        let c = ServerConfig::new("x".repeat(32));
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            json.contains("\"jwtTtl\":86400"),
            "duration must serialise as seconds: {json}"
        );
        let back: ServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.jwt_ttl, c.jwt_ttl);
        assert_eq!(back.heartbeat_interval, c.heartbeat_interval);
    }
}
