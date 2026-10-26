#!/usr/bin/env python3
"""Dump pylatexenc's node tree in `latex_walker::dump`'s format (#466).

The walker port (`crates/docling/src/backend/latex_walker.rs`) is checked node
for node against pylatexenc, parsed with docling's own context
(`docling.backend.latex.utils.latex_context.LATEX_CONTEXT_DB`, docling 2.142's
metadata / caption / bibliography specs and longtable) — the stream docling's
`LatexDocumentBackend` reads:

    python scripts/dev/pylatexenc_dump.py file.tex [...]    # writes file.tex.py.txt
    DOCLING_LATEX_DUMP=file.tex cargo test -p docling --lib \
        latex_walker::tests::dump_file -- --ignored --nocapture   # file.tex.rs.txt
    diff file.tex.py.txt file.tex.rs.txt

Offsets are written as UTF-8 byte offsets, as the Rust port keeps them.
"""

import sys

from docling.backend.latex.utils.latex_context import LATEX_CONTEXT_DB
from pylatexenc.latexwalker import (
    LatexCharsNode,
    LatexCommentNode,
    LatexEnvironmentNode,
    LatexGroupNode,
    LatexMacroNode,
    LatexMathNode,
    LatexSpecialsNode,
    LatexWalker,
)


def rs_debug(s: str) -> str:
    """Rust's `{:?}` of a `&str`."""
    out = ['"']
    for ch in s:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\0":
            out.append("\\0")
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append("\\u{%x}" % ord(ch))
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def main() -> None:
    for path in sys.argv[1:]:
        text = open(path, encoding="utf-8").read()
        offsets = [0]
        for ch in text:
            offsets.append(offsets[-1] + len(ch.encode("utf-8")))

        def at(n):
            return f"@{offsets[n.pos]}+{offsets[n.pos + n.len] - offsets[n.pos]}"

        out: list[str] = []

        def dump(nodes, indent):
            pad = "  " * indent
            for n in nodes or []:
                if isinstance(n, LatexCharsNode):
                    out.append(f"{pad}Chars {rs_debug(n.chars)} {at(n)}")
                elif isinstance(n, LatexGroupNode):
                    out.append(f"{pad}Group {n.delimiters[0]}{n.delimiters[1]} {at(n)}")
                    dump(n.nodelist, indent + 1)
                elif isinstance(n, LatexCommentNode):
                    out.append(
                        f"{pad}Comment {rs_debug(n.comment)} post={rs_debug(n.comment_post_space)} {at(n)}"
                    )
                elif isinstance(n, LatexMacroNode):
                    out.append(
                        f"{pad}Macro {rs_debug(n.macroname)} post={rs_debug(n.macro_post_space)} {at(n)}"
                    )
                    dump_args(n.nodeargd, indent + 1)
                elif isinstance(n, LatexEnvironmentNode):
                    out.append(f"{pad}Env {rs_debug(n.environmentname)} {at(n)}")
                    dump_args(n.nodeargd, indent + 1)
                    dump(n.nodelist, indent + 1)
                elif isinstance(n, LatexSpecialsNode):
                    out.append(f"{pad}Specials {rs_debug(n.specials_chars)} {at(n)}")
                elif isinstance(n, LatexMathNode):
                    out.append(
                        f"{pad}Math {n.displaytype} {rs_debug(n.delimiters[0])} {rs_debug(n.delimiters[1])} {at(n)}"
                    )
                    dump(n.nodelist, indent + 1)

        def dump_args(argd, indent):
            pad = "  " * indent
            if argd is None:
                out.append(f"{pad}args None")
                return
            out.append(f"{pad}args {rs_debug(argd.argspec)}")
            for arg in argd.argnlist:
                if arg is None:
                    out.append(f"{pad}  -")
                else:
                    dump([arg], indent + 1)

        nodes, _, _ = LatexWalker(
            text, tolerant_parsing=True, latex_context=LATEX_CONTEXT_DB
        ).get_latex_nodes()
        dump(nodes, 0)
        with open(path + ".py.txt", "w", encoding="utf-8") as f:
            f.write("\n".join(out) + ("\n" if out else ""))


if __name__ == "__main__":
    main()
