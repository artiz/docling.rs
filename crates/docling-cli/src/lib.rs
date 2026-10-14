//! The library half of the `docling-rs` command-line tool: the Model Context
//! Protocol server behind `docling-rs mcp` (#669), so embedders run the same
//! server in-process — the Python package's `docling-rs-mcp` launcher calls
//! [`mcp::run_with`] with a [`mcp::Launcher`] of its own.

#[cfg(feature = "mcp")]
pub mod mcp;
