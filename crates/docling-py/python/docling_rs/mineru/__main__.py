"""``python -m docling_rs.mineru parse PATH… -o OUT`` — MinerU's ``mineru-kit
parse`` over docling.rs: the same options, the same outputs (``markdown``:
``<stem>.md``; ``middle_json``: ``<stem>.json``; ``zip``: ``<stem>.zip`` of
``ParseResult.save``)."""

from __future__ import annotations

import argparse
import sys
import zipfile
from pathlib import Path
from typing import List, Optional

from . import DEPLOYMENT_TIERS, FileBasedDataWriter, InvalidRequestError, MinerUParser, normalize_page_range_input

_SUFFIX = {"markdown": ".md", "middle_json": ".json", "zip": ".zip"}


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="python -m docling_rs.mineru", description="MinerU-compatible CLI over docling.rs.")
    sub = p.add_subparsers(dest="command")
    parse = sub.add_parser("parse", help="Parse files or directories into markdown, middle JSON, or zip outputs.")
    parse.add_argument("paths", nargs="+", help="files or directories")
    parse.add_argument("-o", "--output", required=True, help="output file (one input) or directory")
    parse.add_argument("--pages", default=None, help="PDF pages: '1-5,8', 'r3-r1' (counted from the end), or 'all'; default: all pages")
    parse.add_argument("--format", default="markdown", choices=sorted(_SUFFIX), help="Output format: markdown, middle_json, zip")
    parse.add_argument("--tier", default=None, choices=DEPLOYMENT_TIERS, help="Parse tier: flash, basic, standard, advanced")
    parse.add_argument("--ocr-mode", default="auto", choices=["auto", "txt", "ocr"], help="OCR mode: auto, txt, ocr")
    return p


def _inputs(paths: List[str]) -> List[Path]:
    out: List[Path] = []
    for raw in paths:
        path = Path(raw).expanduser()
        if path.is_dir():
            out.extend(sorted(p for p in path.iterdir() if p.is_file() and not p.name.startswith(".")))
        else:
            out.append(path)
    return out


def _save(result, dest: Path, fmt: str) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if fmt == "markdown":
        dest.write_bytes(result.markdown().encode("utf-8", errors="replace"))
    elif fmt == "middle_json":
        dest.write_bytes(result.to_json().encode("utf-8", errors="replace"))
    else:
        tmp = dest.parent / f".{dest.stem}"
        result.save(FileBasedDataWriter(str(tmp)))
        with zipfile.ZipFile(dest, "w", zipfile.ZIP_DEFLATED) as zf:
            for child in sorted(tmp.rglob("*")):
                if child.is_file():
                    zf.write(child, arcname=child.relative_to(tmp).as_posix())
        for child in sorted(tmp.rglob("*"), reverse=True):
            child.unlink() if child.is_file() else child.rmdir()
        tmp.rmdir()


def main(argv: Optional[List[str]] = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] != "parse" and not argv[0].startswith("-"):
        argv.insert(0, "parse")
    args = _parser().parse_args(argv)
    if args.command != "parse":
        _parser().print_help()
        return 2
    try:
        pages = normalize_page_range_input(args.pages)
    except InvalidRequestError as exc:
        print(f"error: {exc.message}", file=sys.stderr)
        return 2
    inputs = _inputs(args.paths)
    output = Path(args.output).expanduser()
    many = len(inputs) > 1 or output.is_dir() or (not output.suffix and not output.exists())
    if many and output.suffix in _SUFFIX.values() and len(inputs) > 1:
        print("error: when input is multiple files or directories, --output must be a directory path", file=sys.stderr)
        return 2
    parser = MinerUParser(tier=args.tier or "standard", parse_mode=args.ocr_mode)
    for path in inputs:
        dest = output / f"{path.stem}{_SUFFIX[args.format]}" if many else output
        try:
            result = parser.parse(path, page_range=pages)
        except Exception as exc:  # noqa: BLE001 — a batch reports and goes on
            print(f"error: {path}: {exc}", file=sys.stderr)
            if len(inputs) == 1:
                return 1
            continue
        _save(result, dest, args.format)
        print(f"{path} -> {dest} ({args.format})", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
