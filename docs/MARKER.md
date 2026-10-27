# docling.rs for Marker users

[Marker](https://github.com/datalab-to/marker) (`marker-pdf`, datalab) turns
PDFs into Markdown, JSON, HTML and chunks for LLMs with a layout model and
OCR behind it. docling.rs does the same job with docling's document
understanding — layout analysis, TableFormer table structure and local OCR —
in a Rust engine on ONNX Runtime, and its Python package speaks **Marker's
API**. Switching is the import swap:

```bash
pip install docling-rs
```

```python
# was:  from marker.converters.pdf import PdfConverter
#       from marker.models import create_model_dict
#       from marker.output import text_from_rendered
from docling_rs.marker.converters.pdf import PdfConverter
from docling_rs.marker.models import create_model_dict
from docling_rs.marker.output import text_from_rendered

converter = PdfConverter(artifact_dict=create_model_dict())
rendered = converter("paper.pdf")
text, _, images = text_from_rendered(rendered)
```

Marker is a datalab project; docling.rs is an independent project of the
[docling](https://github.com/docling-project) community (LF AI & Data) and is
not affiliated with it. "Marker-compatible" means the Python API and the
output shapes below, not the same Markdown byte for byte.

## Why switch — measured side by side

Marker 2.0.0 (`marker-pdf[full]`, CPU, its default `fast` mode, a llama.cpp
build for its OCR server) vs docling.rs master after 2.51.3 on 4 vCPU, no GPU
(2026-10-25), with
[`scripts/conformance/marker_compare.py`](../scripts/conformance/marker_compare.py);
the per-format table is in the README's
[Marker comparison](../README.md#conformance-with-marker), the scoring
[below](#how-the-comparison-is-scored). Accuracy is scored against Python
docling's groundtruth, which favours docling.rs by construction; the timings
and memory do not depend on it.

| | Marker | docling.rs |
|---|---|---|
| PDF (18 files), warm, full pipeline | 1356 s (22.6 min) | **64 s** |
| PDF, peak RSS (client + inference server) | 10.3 GB (1.3 + 9.0) | **2.8 GB** |
| One CLI call, cold (median PDF) | 32.6 s | **1.5 s** |
| PDF word F1 / table-cell recall / heading recall | 0.93 / 0.41 / 0.66 | **0.997 / 0.94 / 0.91** |
| DOCX (38 files), warm | 707 s (1 of 38 failed) | **0.54 s** |
| DOCX table-cell / heading recall | 0.57 / 0.73 | **1.00 / 1.00** |
| Install | `marker-pdf` + PyTorch + Surya (~2 GB of wheels, models from Hugging Face on first run) | one wheel, ~90 MB; models ~700 MB once |
| Runtime | Python, PyTorch, a resident Surya inference server per model | one process, ONNX Runtime; no server |

Marker reads Office files, HTML and EPUB by rendering them to PDF
(WeasyPrint, `marker-pdf[full]`) and running the vision models on the
render; docling.rs parses them natively, so headings, tables and lists come
from the document's own structure and no model runs at all. Where the
render fails, the file fails: `docx_external_image.docx` (an image by URL),
three PPTX files (`no embedded image`, an unrecognized shape type, a
`<p:blipFill>` python-pptx rejects), `xlsx_chart_sheet.xlsx` (a chartsheet)
and `xlsx_04_inflated.xlsx`, whose render took the worker to 5.8 GB and an
OOM kill. docling.rs converts all of them.

On CPU, Marker 2's OCR is a 1.2 GB vision-language model (Surya 2, GGUF)
served by llama.cpp, which `pip install marker-pdf` does not install;
without a `llama-server` binary every page its text check flags fails with
a `SpawnError` — 4 of the 18 PDFs, 9 DOCX, 10 HTML files and the EPUB in
this corpus. With it, those pages are where the minutes go: the four PDFs
account for most of the 23 minutes, and a cold `marker_single` on an HTML
file that needs OCR took 29 minutes in the earlier run (server start, model
load, OCR); on this host its OCR model looped on one HTML render for
5,000+ tokens, and the HTML re-run was stopped after 3 h 45 min (one cold
call on an 855-byte file had taken 97 minutes), so the HTML row is the earlier
run's.

## How the comparison is scored

The per-format table is in the README's
[Marker comparison](../README.md#conformance-with-marker).

Accuracy pairs are Marker / docling.rs against the docling groundtruth
Markdown of each file (word-bag F1, recall of table-cell texts and of
headings; Marker's HTML tables count their `<td>` cells). That reference is
Python docling's output, which docling.rs is conformance-tested against, so
these columns favour docling.rs by construction; the timings do not depend
on it; a failed file scores 0. Warm = Marker in-process after a warm-up
conversion with its models loaded once (its inference servers already
running; a batch whose worker dies is rerun one worker per file, so only the
file that killed it fails) vs a docling.rs batch run
(`--jobs 1`; its ONNX models still use every core, as Marker's PyTorch
does). Peak RSS is the conversion process tree — for Marker the client plus
the Surya inference servers it leaves running (`VmHWM`), for docling.rs the
one process. Cold = one `marker_single FILE` with no inference server
running vs one `docling-rs FILE`, median of three.

Marker ran with its defaults on a CPU host, which is its `fast` mode: a
lightweight layout detector and OCR only on blocks whose text layer looks
garbled. Its `balanced` mode (a vision-language layout model and full-page
OCR) is what its published benchmarks use; it needs a GPU to be practical.
docling.rs ran its default full pipeline — layout, TableFormer and OCR on
every page that needs it — and, for PDFs, `--text-layer-only` as the
like-for-like row for Marker's `--disable_ocr`.

## The API

`docling_rs.marker` mirrors Marker 2.0's public modules:

| Marker | docling.rs |
|---|---|
| `marker.converters.pdf.PdfConverter(artifact_dict, processor_list, renderer, llm_service, config)` | same signature; every format docling.rs reads, not only PDF (DOCX, PPTX, XLSX, HTML, EPUB … natively, no PDF render); `processor_list` / `llm_service` accepted and ignored with a warning |
| `converter(filepath)` → rendered; `converter.page_count` | same — a path or a `BytesIO`; `rendered.document` is the `DoclingDocument` (docling.rs extension) |
| `marker.models.create_model_dict()` | same keys; fetches docling.rs's models into `~/.cache/docling.rs` when they are missing |
| `marker.output.text_from_rendered`, `save_output`, `output_exists` | same: `(text, "md"\|"html"\|"json", images)`; `<name>.md` + `<name>_meta.json` + the images in the output folder |
| `MarkdownOutput(markdown, images, metadata)` | same: `images` is `{"_page_0_Picture_3.jpeg": PIL.Image}`, referenced as `![](_page_0_Picture_3.jpeg)`; `metadata.table_of_contents` (`title`, `heading_level`, `page_id`, `polygon`) and `page_stats` (`page_id`, `text_extraction_method`, `block_counts`, `block_metadata`) |
| `HTMLOutput`, `JSONOutput` / `JSONBlockOutput`, `ChunkOutput` / `FlatBlockOutput`, `OCRJSONOutput` | same models; the block tree has Marker's ids (`/page/0/SectionHeader/2`), `polygon` / `bbox` in PDF points from the top-left corner, `section_hierarchy`, per-block `html` with `<content-ref>` placeholders, base64 `images` on picture blocks; JSON keys as Marker writes them |
| `marker.schema.BlockTypes` | same names; docling's labels map onto them (`section_header` → `SectionHeader`, `caption`, `footnote`, `list_item` under a `ListGroup`, `formula` → `Equation`, `code`, `document_index` → `TableOfContents`, `page_header` / `page_footer`, a figure with a caption → `PictureGroup` [`Picture`, `Caption`], a table with one → `TableGroup`) |
| `marker.converters.table.TableConverter` | same: only `Table` / `Form` / `TableOfContents` blocks, OCR off |
| `marker.converters.ocr.OCRConverter` | same output model: one `Line` per text block with its text and box (Surya's characters are not reproduced) |
| `marker.config.parser.ConfigParser` | same: `generate_config_dict`, `get_renderer`, `get_processors`, `get_converter_cls`, `get_llm_service`, `get_output_folder`, `get_base_filename` |
| config `page_range` (0-based list or `"0,5-10"`), `paginate_output` (+ `page_separator`), `disable_ocr`, `force_ocr`, `strip_existing_ocr`, `extract_images` / `--disable_image_extraction`, `keep_pageheader_in_output`, `keep_pagefooter_in_output`, `languages`, `highres_image_dpi` | honoured — `{page}----` separators, OCR off / forced, the furniture layer kept, picture crops at the DPI |
| `mode`, `use_llm`, `llm_service`, `block_correction_prompt`, `redo_inline_math`, `processors`, `debug`, `pdftext_workers`, `force_layout_block`, `html_tables_in_markdown`, `add_block_ids`, `image_extraction_mode` | accepted, ignored with one warning each: docling.rs runs one pipeline and no LLM |
| `marker_single FILE --output_dir … --output_format … --page_range …`, `marker FOLDER --workers … --skip_existing` | `python -m docling_rs.marker FILE\|FOLDER` with the same options and output layout |
| `marker.settings.settings` | `OUTPUT_DIR`, `OUTPUT_IMAGE_FORMAT`, `OUTPUT_ENCODING`, `TORCH_DEVICE` from the same environment variables |

Beyond Marker: `PdfConverter(config={"docling_options": {...}})` passes
options to docling.rs's `DocumentConverter` — `{"text_layer_only": True}`,
`{"ocr_lang": "ch"}`, `{"do_picture_ocr": True}` — and `rendered.document`
is the full `DoclingDocument` (headings, tables with cells, pictures,
provenance) for chunking or anything Markdown cannot carry.

What the output does not reproduce byte for byte: Marker's own Markdown
markup (escaped `\_`, `<sup>` footnote marks, its `$$` spacing); its
`heading_level` numbering (here the level of the Markdown heading docling
writes: a title `1`, a level-1 section header `2`); and
`page_stats.text_extraction_method`, which says `pdftext` unless OCR was
forced — docling.rs does not report per page which pages it OCR'd.

## Not only Python

| You use Marker as… | docling.rs equivalent |
|---|---|
| a Python library | `from docling_rs.marker.converters.pdf import PdfConverter` |
| `marker_single` / `marker` | `docling-rs FILE` prints Markdown (`--to json\|html\|chunks\|…`), `docling-rs --input 'dir/*' --output out` for a folder; [release binaries](https://github.com/docling-project/docling.rs/releases), `cargo install docling-cli`, or `python -m docling_rs.marker` |
| `marker_server` | `docker run -p 5001:5001 ghcr.io/docling-project/docling-rs-serve` — `POST /v1/convert`, and docling-serve's routes for Open WebUI, n8n, Dify ([details](../crates/docling-serve/README.md#drop-in-for-docling-serve-open-webui-n8n-dify-langchain)) |
| `marker_gui` | the [browser demo](https://docling-project.github.io/docling.rs/) (WebAssembly, runs in the page) |
| — | an MCP server (`uvx docling-rs-mcp`), Node.js (`npm i docling.rs`), Rust (`docling` crate), C (`docling-ffi`), the LangChain / LlamaIndex / AutoGen / Agent Framework integrations |

Formats: everything Marker converts (PDF, images, DOCX, PPTX, XLSX, HTML,
EPUB) plus Markdown, CSV, ODF, RTF, legacy `.doc` / `.ppt` / `.xls`, Apple
iWork, DjVu, LaTeX, AsciiDoc, WebVTT, MHTML, `.eml` / `.msg`, Jupyter
notebooks, JATS / USPTO / XBRL, feeds, and local speech recognition for audio
and video.

## Open gaps

None tracked: [open an issue](https://github.com/docling-project/docling.rs/issues)
for what you miss.

## Reproduce the numbers

```bash
python3 -m venv marker-venv && marker-venv/bin/pip install "marker-pdf[full]"
cargo build --release -p docling-cli
scripts/install/download_dependencies.sh
marker-venv/bin/python scripts/conformance/marker_compare.py corpus
```

Run it on your own documents too — that is the comparison that counts.
