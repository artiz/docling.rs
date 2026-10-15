# The docling.rs MCP server, for the catalogs that build a repository's
# Dockerfile and probe the result over stdio — Glama (glama.ai/mcp/servers),
# Docker's MCP registry — and for anyone who wants the server in a container
# without the models: the PyPI launcher (`uvx docling-rs-mcp`, the same
# `docling_rs.mcp` that `docling-rs mcp` runs) on a slim Python image.
# Nothing of this workspace is compiled here, so the build is a pip install;
# the full images (the CLI with the ONNX models baked in, the docling-serve
# HTTP API, their CUDA variants) are crates/docling-serve/Dockerfile.
#
#   docker build -t docling-rs-mcp .
#   docker run -i --rm -v "$PWD:/data" docling-rs-mcp
#
# The server starts and lists its tools without the models; Office, HTML,
# e-mail, text and the other non-image formats convert in full, PDFs from
# their text layer. `docling-rs-mcp --download-models` fetches the layout,
# table and OCR models (~720 MB) into the package's cache — bake them in
# with a `RUN` of it, or mount a models directory at DOCLING_RS_MODELS_DIR.
# `--root /data` confines local paths to the mounted folder; URLs need
# `--allow-url-fetch` in the command (`docling-rs-mcp --help` lists the
# flags, `DOCLING_RS_OCR_LANG` picks the OCR language).

FROM python:3.12-slim

LABEL io.modelcontextprotocol.server.name="io.github.docling-project/docling-rs"

# A released version (`--build-arg DOCLING_RS_MCP_VERSION=2.27.0`); unset,
# the latest on PyPI — what a catalog's build of a commit gets.
ARG DOCLING_RS_MCP_VERSION=""
RUN pip install --no-cache-dir "docling-rs-mcp${DOCLING_RS_MCP_VERSION:+==$DOCLING_RS_MCP_VERSION}"

RUN mkdir -p /data
WORKDIR /data

CMD ["docling-rs-mcp", "--root", "/data"]
