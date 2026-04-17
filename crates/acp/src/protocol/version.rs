//! ACP protocol version constant.
//!
//! Kept as a string (not semver-split) because ACP's own spec uses
//! string form. Peers with mismatched versions fall back to shared-
//! subset behaviour per the spec.

pub const PROTOCOL_VERSION: &str = "0.1.0";
