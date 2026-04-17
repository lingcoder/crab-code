//! Crab-proto server side — accept remote clients that attach to a
//! running crab session.
//!
//! Composition: [`RemoteServer`] owns the axum listener;
//! [`SessionHandler`] is the trait the composition root implements to
//! provide real session behaviour. Each connection runs through
//! [`dispatch::run_connection`] which drives the JSON-RPC dispatch loop
//! against one [`SessionHandle`].

pub mod config;
pub mod dispatch;
pub mod handler;
pub mod listener;

pub use config::{ServerConfig, ServerConfigError};
pub use handler::{
    InboundCmd, OutboundEvent, SessionError, SessionHandle, SessionHandler, attach_result,
    create_result,
};
pub use listener::{RemoteServer, ServeError};
