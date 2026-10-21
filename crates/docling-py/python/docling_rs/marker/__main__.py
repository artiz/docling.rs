"""``python -m docling_rs.marker`` — Marker's ``marker_single FILE`` and
``marker FOLDER`` CLIs over docling.rs: the same options, the same output
layout (``<output_dir>/<name>/<name>.md`` + ``<name>_meta.json`` + images)."""

from __future__ import annotations

import argparse
import glob
import os
import sys
import time
from typing import List, Optional

from . import ConfigParser, create_model_dict, output_exists, save_output, settings


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="python -m docling_rs.marker",
        description="Convert a file (marker_single) or every file of a folder (marker) with docling.rs, Marker-compatible CLI.",
    )
    p.add_argument("path", help="a file, or a folder of files")
    p.add_argument("--output_dir", default=None, help=f"output directory (default: {settings.OUTPUT_DIR})")
    p.add_argument("--output_format", default="markdown", choices=["markdown", "json", "html", "chunks"])
    p.add_argument("--page_range", default=None, help="pages to convert, 0-based: 0,5-10,20")
    p.add_argument("--paginate_output", action="store_true", help="insert Marker's {page}---- separators")
    p.add_argument("--disable_ocr", action="store_true", help="text layer only, never OCR")
    p.add_argument("--force_ocr", action="store_true", help="OCR every page")
    p.add_argument("--strip_existing_ocr", action="store_true", help="OCR every page (same as --force_ocr here)")
    p.add_argument("--disable_image_extraction", action="store_true", help="no images in the output")
    p.add_argument("--keep_pageheader_in_output", action="store_true")
    p.add_argument("--keep_pagefooter_in_output", action="store_true")
    p.add_argument("--languages", default=None, help="OCR language(s), e.g. en or zh")
    p.add_argument("--converter_cls", default=None, help="PdfConverter (default), TableConverter or OCRConverter")
    p.add_argument("--processors", default=None, help="accepted and ignored")
    p.add_argument("--config_json", default=None, help="JSON file with extra config")
    p.add_argument("--mode", default=None, choices=["balanced", "fast"], help="accepted and ignored")
    p.add_argument("--use_llm", action="store_true", help="accepted and ignored")
    p.add_argument("--llm_service", default=None, help="accepted and ignored")
    p.add_argument("--debug", "-d", action="store_true", help="accepted and ignored")
    p.add_argument("--disable_multiprocessing", action="store_true", help="accepted and ignored")
    # the folder CLI's own
    p.add_argument("--workers", type=int, default=None, help="accepted (docling.rs converts sequentially; its models use every core)")
    p.add_argument("--skip_existing", action="store_true", help="skip files whose output exists")
    p.add_argument("--max_files", type=int, default=None)
    p.add_argument("--chunk_idx", type=int, default=0)
    p.add_argument("--num_chunks", type=int, default=1)
    return p


def _files(folder: str, args: argparse.Namespace) -> List[str]:
    files = sorted(f for f in glob.glob(os.path.join(glob.escape(folder), "*")) if os.path.isfile(f))
    n = max(1, int(args.num_chunks))
    size = -(-len(files) // n)
    files = files[args.chunk_idx * size:(args.chunk_idx + 1) * size]
    if args.max_files is not None:
        files = files[: args.max_files]
    return files


def main(argv: Optional[List[str]] = None) -> int:
    args = _parser().parse_args(argv)
    options = {k: v for k, v in vars(args).items() if k not in ("path", "workers", "skip_existing", "max_files", "chunk_idx", "num_chunks")}
    parser = ConfigParser(options)
    models = create_model_dict()
    converter = parser.get_converter_cls()(
        config=parser.generate_config_dict(), artifact_dict=models, processor_list=parser.get_processors(),
        renderer=parser.get_renderer(), llm_service=parser.get_llm_service(),
    )
    files = _files(args.path, args) if os.path.isdir(args.path) else [args.path]
    start = time.time()
    for fpath in files:
        out_folder = parser.get_output_folder(fpath)
        base = parser.get_base_filename(fpath)
        if args.skip_existing and output_exists(out_folder, base):
            continue
        try:
            rendered = converter(fpath)
        except Exception as e:  # noqa: BLE001 — a folder run reports and goes on
            print(f"Error converting {fpath}: {e}", file=sys.stderr)
            if len(files) == 1:
                return 1
            continue
        save_output(rendered, out_folder, base)
        print(f"Saved {args.output_format} to {out_folder}", file=sys.stderr)
    print(f"Total time: {time.time() - start}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
