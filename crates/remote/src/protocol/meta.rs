//! Protocol-level metadata and reflection helpers.
//!
//! Version constant, shared id type, and the JSON-Schema dump function
//! third-party clients (TS / Swift / Kotlin) use to generate stubs.

use schemars::JsonSchema;

/// Current protocol version as a semver-compatible string.
///
/// Advertised by both sides during the `initialize` handshake; peers
/// MUST reject connections with a different major version via
/// [`super::error::ErrorCode::UnsupportedVersion`].
pub const PROTOCOL_VERSION: &str = "0.1.0";

/// JSON-RPC request id — `u64` instead of JSON-RPC's permissive
/// `number | string | null`.
///
/// Narrowing to `u64` is safe because every known client assigns
/// numeric ids, and a native integer key lets the server hash-key its
/// pending-request map without allocating.
pub type MessageId = u64;

/// Dump the JSON Schema for a type — used by `xtask` / CLI to export
/// the schema file that drives client-stub generation.
///
/// Example:
/// ```
/// use crab_remote::protocol::{dump_schema, InitializeParams};
/// let schema = dump_schema::<InitializeParams>();
/// assert!(schema.contains("InitializeParams"));
/// ```
pub fn dump_schema<T: JsonSchema>() -> String {
    let schema = schemars::schema_for!(T);
    serde_json::to_string_pretty(&schema).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_semver_shaped() {
        let parts: Vec<&str> = PROTOCOL_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "version must be major.minor.patch");
        for p in parts {
            p.parse::<u32>().expect("each version part must be numeric");
        }
    }

    #[test]
    fn schema_dump_is_valid_json() {
        let schema = dump_schema::<super::super::InitializeParams>();
        let v: serde_json::Value = serde_json::from_str(&schema).unwrap();
        assert!(v.is_object());
    }
}
