"""``docling-rs-mcp``: the docling.rs Model Context Protocol server (#669).

The server is the one the ``docling-rs mcp`` command runs, compiled into this
package's native module, so agents get it with nothing but Python::

    uvx docling-rs-mcp --root ~/Documents       # the launcher package
    python -m docling_rs.mcp --root ~/Documents # with docling-rs installed

Tools: ``convert_to_markdown(uri)`` (MarkItDown's MCP tool, same contract),
``convert(uri, to, images, options)`` and ``list_input_formats()``; stdio by
default, Streamable HTTP with ``--http``. ``--help`` lists the flags.

PDFs and images use the layout, table and OCR models from this package's
cache (``docling_rs.download_models()``); fetch them once with
``docling-rs-mcp --download-models``. Until then PDFs convert from their
text layer.
"""

from __future__ import annotations

import sys
from typing import List, Optional

COMMAND = "docling-rs-mcp"
_CLIENT_CONFIG = (
    '{"mcpServers": {"docling": {"command": "uvx",\n'
    '     "args": ["docling-rs-mcp", "--root", "/Users/me/Documents"]}}}'
)
_MODELS_HINT = (
    "Fetch them once with `uvx docling-rs-mcp --download-models` "
    "(or docling_rs.download_models()) and restart the server."
)
_DOWNLOAD_HELP = (
    "  --download-models   fetch the PDF/image models into the docling.rs cache\n"
    "                      ($DOCLING_RS_CACHE_DIR, else ~/.cache/docling.rs) and exit;\n"
    "                      =all adds the code/formula enrichment models"
)


def main(argv: Optional[List[str]] = None) -> None:
    """Run the server with ``argv`` (default: the command line) and exit
    with its status. stdout is the stdio transport's wire, so everything
    this launcher says goes to stderr."""
    args = list(sys.argv[1:] if argv is None else argv)
    from . import models

    download = [a for a in args if a.split("=")[0] == "--download-models"]
    if download:
        # The models every PDF/image conversion uses; `=all` adds the
        # code/formula enrichment models (~740 MB) only `do_code_enrichment`
        # / `do_formula_enrichment` need.
        everything = download[-1] == "--download-models=all"
        root = models.download_models(progress=True, enrichment=everything)
        print(f"{COMMAND}: models ready in {root}", file=sys.stderr)
        sys.exit(0)
    # The cache becomes the engine's models directory (unless the operator
    # pinned DOCLING_RS_MODELS_DIR), so downloaded models are found from any
    # working directory.
    models.ensure_env()
    from ._native import mcp_main

    sys.exit(mcp_main(args, COMMAND, _CLIENT_CONFIG, _MODELS_HINT, _DOWNLOAD_HELP))


if __name__ == "__main__":
    main()
