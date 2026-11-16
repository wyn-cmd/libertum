//! MCP (Model Context Protocol) connectors for Libertum: a server that reads and, when the writer
//! allows it, edits, and a client for other MCP programs; macros are planned in `docs/mcp-plan.md`.
//!
//! This crate knows nothing about the editor or the UI. The application answers the server's
//! questions through the [`Host`] trait.

mod client;
mod host;
mod protocol;
mod server;
pub mod tools;

pub use client::{
    CALL_TIMEOUT, CONNECT_TIMEOUT, ClientError, Connection, MAX_OUTPUT_BYTES, ServerSpec, ToolInfo,
    ToolOutput,
};
pub use host::{ChannelHost, Host, Pending, channel};
pub use protocol::{
    ClientInfo, DocumentInfo, FormatPatch, Heading, HostError, HostResult, Position, Reply,
    Request, SearchMatch, SearchResults, SelectionInfo, StyleInfo, TextSlice,
};
pub use server::{DEFAULT_PORT, ENDPOINT_PATH, RunningServer, ServerError};
