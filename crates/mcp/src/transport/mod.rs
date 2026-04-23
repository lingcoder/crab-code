pub mod stdio;
pub mod ws;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use std::future::Future;
use std::pin::Pin;

/// Transport abstraction for MCP JSON-RPC communication.
///
/// Uses `Pin<Box<dyn Future>>` instead of async fn for object safety,
/// since `Box<dyn Transport>` requires the trait to be object-safe.
pub trait Transport: Send + Sync {
    /// Send a request and wait for the corresponding response.
    fn send(
        &self,
        req: JsonRpcRequest,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<JsonRpcResponse>> + Send + '_>>;

    /// Send a notification (fire-and-forget, no response expected).
    fn notify(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>>;

    /// Close the transport connection.
    fn close(&self) -> Pin<Box<dyn Future<Output = crab_core::Result<()>> + Send + '_>>;
}
