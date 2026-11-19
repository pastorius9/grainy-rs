//! Grainy's AI connection (port of `luma/mcp_protocol.py`, `mcp_tools.py`, `mcp_service.py`, `photo_images.py`,
//! `mcp_io.py` and `mcp_http.py`).
//!
//! - [`protocol`], [`tools`], [`check`]: JSON-RPC 2.0 and the Model Context Protocol, the 17 tool definitions as
//!   data (what `tools/list` returns, byte for byte as the Python app) and the argument checker.
//! - [`toolkit::GrainyTools`]: the tools working, on an [`service::McpService`] (jobs, limits, readiness) and the
//!   window behind the [`host::Host`] trait, reached through a [`host::Gui`] ([`gui::GuiQueue`] for an
//!   application's event loop; [`headless::Headless`] over a catalogue without a window).
//! - [`engine`], [`images`]: previews, cached thumbnails, contact sheets and JPEG replies.
//! - [`stdio`], [`http`]: the servers; [`bridge`]: the private files the stdio bridges read.

pub mod b64;
pub mod bridge;
pub mod check;
pub mod engine;
pub mod gui;
pub mod headless;
pub mod host;
pub mod http;
pub mod images;
pub mod observe;
pub mod pillow;
mod printable;
pub mod protocol;
pub mod pyjson;
pub mod service;
pub mod stdio;
pub mod stub;
pub mod text;
pub mod toolkit;
pub mod tools;
mod util;

pub use check::{check, ServiceError};
pub use host::{Gui, Host};
pub use protocol::{Flavour, Protocol, Reply};
pub use pyjson::dumps;
pub use service::McpService;
pub use toolkit::GrainyTools;
pub use tools::{ToolError, ToolFault, ToolResult, Tools};
