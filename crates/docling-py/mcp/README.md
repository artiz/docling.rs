# docling-rs-mcp

<!-- mcp-name: io.github.docling-project/docling-rs -->

A [Model Context Protocol](https://modelcontextprotocol.io) server that
converts documents to Markdown for AI agents — PDF with layout analysis,
table structure and OCR, DOCX, PPTX, XLSX, HTML, EPUB, e-mail, scans and
40-odd other formats. It is [docling.rs](https://github.com/docling-project/docling.rs),
a Rust port of IBM's [docling](https://github.com/docling-project/docling),
and it serves **MarkItDown's `convert_to_markdown(uri)` tool** under the same
name and contract: a `markitdown-mcp` setup switches by changing the command.

```bash
uvx docling-rs-mcp --root ~/Documents
```

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "docling": { "command": "uvx", "args": ["docling-rs-mcp", "--root", "/Users/me/Documents"] }
  }
}
```

Claude Code: `claude mcp add docling -- uvx docling-rs-mcp --root ~/Documents`.

## Tools

| Tool | Returns |
|---|---|
| `convert_to_markdown(uri)` | Markdown of the document behind an `http:`/`https:`/`file:`/`data:` URI or a path |
| `convert(uri, to?, images?, options?)` | `md`, `json` (docling's `DoclingDocument`), `html`, `text`, `latex`, `chunks`, `vtt` or `pandoc`, with docling.rs's [conversion options](https://github.com/docling-project/docling.rs/blob/master/docs/OPTIONS.md) — `{"pages": "1-5"}`, `{"ocr_lang": "ch"}`, `{"password": "…"}`, … |
| `list_input_formats()` | the extensions the server converts |

## PDF models

Office, HTML, Markdown and e-mail need no models. PDFs and images use
ONNX models for layout, tables and OCR (~720 MB). Fetch them once:

```bash
uvx docling-rs-mcp --download-models
```

They land in the docling.rs cache (`$DOCLING_RS_CACHE_DIR`, else
`~/.cache/docling.rs`) and the server finds them from then on;
`--download-models=all` adds the code/formula enrichment models (~740 MB
more). Until then PDFs convert from their text layer, as MarkItDown's do.

**macOS:** PyPI has no macOS wheel of `docling-rs` yet, so uv builds it from
source on first use — install a Rust toolchain (`rustup`) and run the
`--download-models` line in a terminal once; the build is cached, and an MCP
client then starts the server without waiting for it.

## What a call may reach

Local paths and `file:` URIs are confined to `--root` (repeatable; the
working directory by default, except in `/`, where it is required).
`http(s)` URLs are fetched only with
`--allow-url-fetch`, and private or loopback addresses stay blocked.
`--http` serves Streamable HTTP on `127.0.0.1:3001` instead of stdio.
`--help` lists every flag; the security model is in
[docs/SECURITY.md](https://github.com/docling-project/docling.rs/blob/master/docs/SECURITY.md#docling-rs-mcp-model-context-protocol-server-669).

The same server ships in the `docling-rs` binary (`docling-rs mcp`) and the
`ghcr.io/docling-project/docling-rs` image, models included; with the
`docling-rs` Python package installed it is also `python -m docling_rs.mcp`.
On NVIDIA GPUs, `uv run --with docling-rs-cuda python -m docling_rs.mcp`.

MIT licensed.
