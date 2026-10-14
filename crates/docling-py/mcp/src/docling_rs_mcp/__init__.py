"""``uvx docling-rs-mcp``: the docling.rs MCP server. The server itself is
:mod:`docling_rs.mcp` in the docling-rs package; this one names the command."""

from docling_rs.mcp import main

__all__ = ["main"]
