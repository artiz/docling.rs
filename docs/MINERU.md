# docling.rs for MinerU users

[MinerU](https://github.com/opendatalab/MinerU) (OpenDataLab) turns PDFs,
images and Office files into Markdown, Middle JSON, Structured Content and
Content Lists for LLMs, with layout, table and OCR models behind the
higher tiers. docling.rs does the same job with docling's document
understanding — layout analysis, TableFormer table structure and local OCR
— in a Rust engine on ONNX Runtime, and its Python package speaks **MinerU
4's SDK**. Switching is the import swap:

```bash
pip install docling-rs
```

```python
# was:  from mineru.parser import parse
#       from mineru.parser.writer import FileBasedDataWriter
from docling_rs.mineru.parser import parse
from docling_rs.mineru.parser.writer import FileBasedDataWriter

result = parse("paper.pdf", tier="standard", ocr_mode="auto", page_range="1-3")
print(result.markdown())
result.save(FileBasedDataWriter("out/paper"))   # markdown.md, middle_json.json, structured_content.json, images/
```

MinerU is an OpenDataLab project (AGPL-3.0); docling.rs is an independent
project of the [docling](https://github.com/docling-project) community
(LF AI & Data, MIT) and is not affiliated with it. "MinerU-compatible"
means the Python API and the output shapes below, not the same Markdown
byte for byte.

## Why switch — measured side by side

MinerU 4.0.12 (`mineru`, CPU: its `flash` tier without models and its
`basic` tier with its ONNX models) vs docling.rs 2.46.2 on 4 vCPU, no GPU,
with [`scripts/conformance/mineru_compare.py`](../scripts/conformance/mineru_compare.py);
the per-format table is in the README's
[MinerU comparison](../README.md#conformance-with-mineru), the scoring
[below](#how-the-comparison-is-scored). Accuracy is scored against Python
docling's groundtruth, which favours docling.rs by construction; the timings
and memory do not depend on it.

| | MinerU `flash` | MinerU `basic` | docling.rs |
|---|---|---|---|
| PDF (18 files), warm | 22.3 s | 123 s (+ 67 s model load) | **44.0 s** (`--text-layer-only`: 0.69 s) |
| PDF, peak RSS | 1.4 GB | 4.6 GB | **3.1 GB** |
| One CLI call, cold (median PDF) | 3.6 s | 6.5 s | **1.2 s** |
| PDF word F1 / table-cell recall / heading recall | 0.87 / 0.28 / 0.57 | 0.74 / 0.44 / 0.58 | **0.997 / 0.94 / 0.91** |
| DOCX (38 files), warm | 6.36 s | — (flash only) | **0.59 s** |
| DOCX table-cell / heading recall | 0.82 / 0.96 | — | **1.00 / 1.00** |
| Install | `mineru` + DocVortex + Gradio + ONNX Runtime + a llama.cpp build (~1 GB of wheels); `basic` models and the `standard` VLM from Hugging Face / ModelScope on first use | one wheel, ~90 MB; models ~700 MB once |
| Runtime | Python, DocVortex render workers, a llama.cpp VLM server for `standard` / `advanced` | one process, ONNX Runtime; no server |

MinerU's `basic` tier scored below its `flash` tier on word F1 here: on the
DocLayNet paper it kept 6.5 k of the groundtruth's 7.3 k words where `flash`
kept 8.3 k (headers, footers and figure text included), so its layout pass
dropped body text that the text layer had. docling.rs's full pipeline keeps
0.997 of the words.

MinerU's `flash` tier reads the text layer with no model, as docling.rs's
`--text-layer-only` does; its `basic` tier runs small ONNX models for
layout, tables and OCR, as docling.rs's default pipeline does; its
`standard` and `advanced` tiers add a 1.2 B vision-language model through
llama.cpp, which on CPU is a different speed class altogether
(138 s for one page of the DocLayNet paper on this 4 vCPU host, so it is not in the table). Office files, HTML and EPUB are `flash`-only in MinerU
(parsed natively, no models); docling.rs parses them natively at any tier.

## How the comparison is scored

The per-format table is in the README's
[MinerU comparison](../README.md#conformance-with-mineru).

Accuracy pairs are MinerU / docling.rs against the docling groundtruth
Markdown of each file (word-bag F1, recall of table-cell texts — MinerU's
HTML tables count their `<td>` cells — and of headings). That reference is
Python docling's output, which docling.rs is conformance-tested against, so
these columns favour docling.rs by construction; the timings do not depend
on it; a failed file scores 0. Warm = MinerU in-process through
`mineru.parser.parse` after a warm-up conversion (models loaded; a batch
whose worker dies is rerun one worker per file) vs a docling.rs batch run
(`--jobs 1`; its ONNX models still use every core, as MinerU's do). Peak
RSS is the conversion process tree, DocVortex's render workers included,
plus any llama.cpp server left running. Cold = one `mineru-kit parse FILE
-o out.md --tier T` vs one `docling-rs FILE`, median of three.

## The API

`docling_rs.mineru` mirrors MinerU 4.0's public modules:

| MinerU | docling.rs |
|---|---|
| `mineru.parser.parse(path, tier=, ocr_mode=, image_analysis=, page_range=, source_context=, vlm_config=)` → `ParseResult`; `parse_async` | same signature; `tier` `flash` converts from the text layer (OCR only under `ocr_mode="ocr"`), `basic` / `standard` / `advanced` run the full pipeline; `ocr_mode` `txt` = no OCR, `ocr` = every page, `auto` = the engine decides; `image_analysis`, `vlm_config`, `source_context` accepted and ignored; every format docling.rs reads at any tier |
| `ParseResult.pages`, `.markdown(add_markers=, mode=, asset_base_url=, image_renderer=)`, `.structured_content()`, `.to_dict()` / `.to_json()` / `.from_dict()` / `.from_json()`, `.save(writer)`, `.export_pages()` | same; `result.document` is the `DoclingDocument` (docling.rs extension) |
| `save(writer)` → `markdown.md`, `middle_json.json`, `structured_content.json`, `images/page_<p>_<type>_<index>.<ext>` | same layout, same image names; `model_output.json` only when a model output is attached (never, here) |
| Middle JSON (`schema: docvortex.middle`, `schema_version: 2.0`): `metadata` (`file_suffix`, `producer`, `document`), `extensions` (`mineru.tier` / `parse_mode`, `docvortex_layout` page sizes), `pages[].blocks[]` with `type`, `index`, normalized `bbox`, inline-span `content`; `image` / `table` / `code` parents with `*_body` / `*_caption` / `*_footnote` children; `list`; `header` / `footer` / `page_footnote` | same shape from docling's items (`section_header` → `paragraph_title`, `title` → `doc_title`, `caption` / `footnote` children of a picture or table → the annotations, `formula` → `equation`, `code`, `page_header` / `page_footer`, `reference` → `ref_text`); producer `docling.rs`; a document without pages is page `0` without boxes; `to_dict()` leaves a PDF's `image_base64` out, as MinerU's does. MinerU's strict validator is not reproduced |
| `mineru.render.render(middle_json, RenderFormat.MARKDOWN \| HTML \| STRUCTURED_CONTENT \| CONTENT_LIST \| CONTENT_LIST_V2, options=…)`, the `*RenderOptions` classes, `RenderMode.DEFAULT` / `FULL` | same: `#` titles, HTML tables, `$$` equations, fenced code, `![](images/…)` after `save` and `data:` URIs before, `---` page separators and the page auxiliaries in `FULL`; Content List V1 with `bbox` in 0–1000 and `page_idx`, V2 per page; `LATEX`, `DOCX`, `EPUB`, `PDF` raise `NotImplementedError` (`docling-rs --to` has docling's formats) |
| `MinerUParser(tier=, parse_mode=, image_analysis=, vlm_config=)`, `DocumentParser` (`parse`, `parse_async`, `parse_batch`, context manager) | same |
| `mineru.parser.writer.DataWriter`, `FileBasedDataWriter` | same |
| `mineru.parser.page_range`: `'1-5,8'`, `'r3-r1'` (from the end), `'all'`; `normalize_page_range_input`, `parse_page_range`, `expand_page_range` … | same grammar and functions; `InvalidRequestError` with `code` / `message` / `param` |
| `mineru.types.Tier`, `validate_tier`, `BlockType`, `MiddleJson`, `PageInfo` | same |
| `mineru-kit parse PATH… -o OUT --tier --ocr-mode --pages --format markdown\|middle_json\|zip` | `python -m docling_rs.mineru parse …` with the same options and outputs |
| `mineru parse` / `mineru server` / `mineru-kit api-server` / `MinerUApiParser` / the document library | not provided: the service is [docling-serve](../crates/docling-serve/README.md), the MCP server `docling-rs mcp` |

Beyond MinerU: `parse(..., docling_options={...})` passes options to
docling.rs's `DocumentConverter` — `{"ocr_lang": "ch"}`,
`{"do_picture_ocr": True}` — and `result.document` is the full
`DoclingDocument` for anything the Middle JSON cannot carry.

## Not only Python

| You use MinerU as… | docling.rs equivalent |
|---|---|
| a Python library | `from docling_rs.mineru.parser import parse` |
| `mineru-kit parse` | `docling-rs FILE` prints Markdown (`--to json\|html\|chunks\|…`), `docling-rs --input 'dir/*' --output out` for a folder; [release binaries](https://github.com/docling-project/docling.rs/releases), `cargo install docling-cli`, or `python -m docling_rs.mineru` |
| `mineru-kit api-server`, `mineru server` | `docker run -p 5001:5001 ghcr.io/docling-project/docling-rs-serve` — `POST /v1/convert`, and docling-serve's routes for Open WebUI, n8n, Dify ([details](../crates/docling-serve/README.md#drop-in-for-docling-serve-open-webui-n8n-dify-langchain)) |
| `mineru-kit webui` | the [browser demo](https://docling-project.github.io/docling.rs/) (WebAssembly, runs in the page) |
| — | an MCP server (`uvx docling-rs-mcp`), Node.js (`npm i docling.rs`), Rust (`docling` crate), C (`docling-ffi`), the LangChain / LlamaIndex / AutoGen integrations |

Formats: everything MinerU parses (PDF, images, DOCX, PPTX, XLSX, `.doc`
/ `.ppt` / `.xls`, RTF, ODF, EPUB, HTML, MHTML, CSV / TSV) except OFD, plus
Markdown, AsciiDoc, LaTeX, plain text and code (MinerU indexes but does not
parse them), Apple iWork, DjVu, WebVTT, `.eml` / `.msg`, Jupyter notebooks,
JATS / USPTO / XBRL, feeds, and local speech recognition for audio and
video.

## Open gaps

None tracked: [open an issue](https://github.com/docling-project/docling.rs/issues)
for what you miss.

## Reproduce the numbers

```bash
python3 -m venv mineru-venv && mineru-venv/bin/pip install "mineru>=4,<5"
cargo build --release -p docling-cli
scripts/install/download_dependencies.sh
mineru-venv/bin/python scripts/conformance/mineru_compare.py corpus
```

Run it on your own documents too — that is the comparison that counts.
