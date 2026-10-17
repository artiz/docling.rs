# docling.rs for MarkItDown users

[MarkItDown](https://github.com/microsoft/markitdown) turns documents into
Markdown for LLMs. docling.rs does the same job with docling's document
understanding behind it — layout analysis, TableFormer table structure and
local OCR — in a Rust engine, and its Python package speaks **MarkItDown's
API**. Switching is one import:

```bash
pip install docling-rs
```

```python
# was:  from markitdown import MarkItDown
from docling_rs.markitdown import MarkItDown

md = MarkItDown()
result = md.convert("report.pdf")   # path, URL, file:/data: URI, stream, requests.Response
print(result.markdown)
```

MarkItDown is a Microsoft project; docling.rs is an independent project of the
[docling](https://github.com/docling-project) community (LF AI & Data) and is
not affiliated with it. "MarkItDown-compatible" means the Python API below, not
the same Markdown byte for byte.

## Why switch — measured side by side

MarkItDown 0.1.8 (`markitdown[all]`) vs docling.rs 2.33.0 on 4 vCPU, no GPU,
with [`scripts/conformance/markitdown_compare.py`](../scripts/conformance/markitdown_compare.py);
the per-format table is in the README's
[MarkItDown comparison](../README.md#conformance-with-microsoft-markitdown),
the scoring [below](#how-the-comparison-is-scored).
Accuracy is scored against Python docling's groundtruth, which favours
docling.rs by construction; the timings do not depend on it.

| | MarkItDown | docling.rs |
|---|---|---|
| DOCX (38 files), warm | 4.22 s | **0.44 s** |
| One CLI call, cold (DOCX) | 1102 ms | **14 ms** |
| DOCX table-cell / heading recall | 0.81 / 0.78 | **1.00 / 1.00** |
| PDF table-cell recall (full pipeline) | 0.31 | **0.94** |
| PDF heading recall (full pipeline) | 0.00 | **0.91** |
| PDF text layer only, 18 files | 7.6 s | **0.5 s** (`text_layer_only`) |
| Text drawn into images (27 files) | 0.00–0.33 without an LLM; 1.00 with GPT-4o (46 API calls, $0.063) | **1.00**, local OCR, offline, no API key |

The full PDF pipeline renders every page and runs neural models — it is
slower than MarkItDown's pdfminer pass (38.4 s vs 7.6 s on the 18 PDFs) and
does the work that turns 0.31 table-cell recall into 0.94 and reads scans at
all. When only the text layer matters, `text_layer_only` is 15× faster than
MarkItDown and still scores higher on word F1 (0.97 vs 0.63).

## How the comparison is scored

The per-format table is in the README's
[MarkItDown comparison](../README.md#conformance-with-microsoft-markitdown).

Accuracy pairs are MarkItDown / docling.rs against the docling groundtruth
Markdown of each file (word-bag F1, recall of table-cell texts and of
headings). That reference is Python docling's output, which docling.rs is
conformance-tested against, so these columns favour docling.rs by
construction; the timings do not depend on it. Warm = MarkItDown in-process
after a warm-up conversion vs a docling.rs batch run (`--jobs 1`; its ONNX
models still use every core, MarkItDown runs on one). MarkItDown raised on
2 of the 46 Office files (`docx_external_image.docx`,
`powerpoint_unrecognized_shape.pptx`), docling.rs on none.
¹ only one XLSX groundtruth carries a heading.

The two PDF rows do different amounts of work. MarkItDown (pdfminer) reads
the text layer only — the like-for-like comparison is docling.rs'
`--text-layer-only`, 15× faster. The full docling.rs pipeline renders every
page and runs neural models on CPU: layout analysis (~0.4 s/page), TableFormer
for every table (~1 s/table) and OCR on pages without a text layer — the
work that turns 0.31 table-cell recall into 0.94 and that reads scans at all.

### Text inside images

MarkItDown's own `markitdown-ocr` test files (27
DOCX/PPTX/XLSX/PDF files with text drawn into pictures and five scanned
PDFs; [`markitdown_ocr_truth.json`](../scripts/conformance/markitdown_ocr_truth.json),
transcribed by hand), share of those words in the output:

| Run | Office pictures (16) | PDF with images (6) | Scanned PDF (5) | Time | Cost |
|---|---|---|---|---|---|
| MarkItDown, no LLM | 0.16 | 0.33 | 0.00 | 1.3 s | — |
| MarkItDown + `markitdown-ocr`, gpt-4o | 1.00 | 1.00 | 1.00 | 43.5 s (46 API calls, 21 870 tokens) | $0.063 |
| docling.rs `--picture-ocr --force-full-page-ocr` | **1.00** | **1.00** | **1.00** | **30.6 s** (`--jobs 4`: 27.0 s) | local, offline |

The non-zero scores without OCR are words the images share with the body
text. docling.rs reads the pictures with its own OCR models (PP-OCR detector
and recognizer, ONNX) — no API key, nothing leaves the machine.

## The API

`docling_rs.markitdown` mirrors MarkItDown 0.1's public API: `MarkItDown`
with `convert`, `convert_local`, `convert_stream`, `convert_uri`,
`convert_url` and `convert_response`; `StreamInfo`; `DocumentConverterResult`
(`.markdown`, `.text_content`, `.title`, `str(result)`); and
`MarkItDownException`, `UnsupportedFormatException`,
`FileConversionException`.

| MarkItDown | docling.rs |
|---|---|
| `MarkItDown().convert(source)` | same — path, `Path`, `http(s)`/`file`/`data` URI, binary stream, `requests.Response` |
| `StreamInfo(extension=…, mimetype=…, charset=…)` hints | same; without hints, the engine's content detection decides (`docling_rs.detect_format`, #672 — signatures and container structure, where MarkItDown asks magika) |
| `keep_data_uris=True` | same — pictures as `data:` URIs |
| `llm_client`, `llm_model`, `llm_prompt` | same request per picture; image inputs get the `# Description:` section. Outside the facade — every surface, every format — `do_picture_description` + `picture_description_endpoint` / `_model` / `_prompt` send the same caption request from the engine (#670) |
| markitdown-ocr plugin (`llm_prompt` for text extraction) | `picture_description_mode=ocr`: markitdown-ocr's prompt, through the same endpoint, for DOCX/PPTX/XLSX pictures and PDF figures alike — or the local, offline `--picture-ocr` |
| `requests_session` | same |
| XLSX `## <sheet>` headings | same |
| ZIP: `## File:` per entry, nested ZIPs recursed | same (`## File:` in the facade; the CLI's `docling-rs bundle.zip` prints `## <entry path>` sections, #677); a nested ZIP is read under `DOCLING_RS_ZIP_MAX_DEPTH` (default 1: skipped; at most 3), its entries as `inner.zip/<path>` |
| YouTube URL → `# YouTube`, `## title`, `### Video Metadata`, `### Description`, `### Transcript` | same (#681); the transcript is the caption cues one per line, timed in the document (`--to vtt`), where MarkItDown joins them into one line; `youtube_transcript_languages` → `asr_lang` (`docling_options={"asr_lang": "de"}`; YouTube's translation when the video has no such track); a video without captions is an error, where MarkItDown returns the metadata alone |
| `markitdown FILE -o out.md` | `python -m docling_rs.markitdown FILE -o out.md` (also `-x`, `-m`, `-c`, `--keep-data-uris`, stdin) |
| EXIF lines on an image (`ImageSize`, `Title`, `Description`, `Artist`, `DateTimeOriginal`, `GPSPosition`, … via exiftool), the audio tag lines (`Title`, `Artist`, `Album`, `Genre`, …) and `### Audio Transcript:`, the EPUB `**Title:** / **Authors:** / …` block | the same lines, from the engine's own readers (#679 — pure Rust, no exiftool; the facade converts with `metadata=json`, and `result.metadata` outside it) |
| `exiftool_path`, `style_map`, `docintel_*`, `cu_*` | accepted, ignored with a warning |
| plugins (`enable_plugins`, the `markitdown.plugin` entry point), `register_converter(converter, priority=…)` | same ([#680](https://github.com/docling-project/docling.rs/issues/680)): a converter at `PRIORITY_SPECIFIC_FILE_FORMAT` or below runs before docling.rs's engine, a generic one when the engine has no format for the input or fails on it. To add a format to every surface of the Python package, not only the facade: `docling_rs.register_backend` ([pluggable backends](../crates/docling-py/README.md#pluggable-backends)) |

Beyond MarkItDown: `MarkItDown(docling_options={...})` passes options to
docling.rs's `DocumentConverter` — `{"text_layer_only": True}`,
`{"ocr_lang": "ch"}`, `{"do_picture_ocr": True}` (OCR the pictures inside
Office files) — and `result.document` is the full `DoclingDocument`
(headings, tables with cells, pictures, provenance) for chunking or anything
Markdown cannot carry. The full PDF pipeline needs the models once:
`python -c "import docling_rs; docling_rs.download_models()"`; until then PDFs
convert from their text layer, as MarkItDown's do.

The migration notes, with what differs in the output, are in the Python
package's [Migrating from MarkItDown](../crates/docling-py/README.md#migrating-from-markitdown).

## Not only Python

| You use MarkItDown as… | docling.rs equivalent |
|---|---|
| a Python library | `from docling_rs.markitdown import MarkItDown` |
| the `markitdown` CLI | `docling-rs FILE` prints Markdown (`--to json\|html\|chunks\|…`); `docling-rs URL`, `cat f \| docling-rs --from docx -`, `data:` URLs (#668); [release binaries](https://github.com/docling-project/docling.rs/releases), `cargo install docling-cli`, or `python -m docling_rs.markitdown` |
| the `markitdown-mcp` MCP server | `uvx docling-rs-mcp` (or `docling-rs mcp`) — the same `convert_to_markdown(uri)` tool for Claude Desktop, Claude Code, Cursor and VS Code, plus `convert` (JSON, HTML, chunks, …) and `list_input_formats`; stdio or Streamable HTTP, in the release binaries and the `ghcr.io/docling-project/docling-rs` image ([setup](../README.md#mcp-server--docling-rs-mcp)) |
| a service | `docker run -p 5001:5001 ghcr.io/docling-project/docling-rs-serve` — `POST /v1/convert`, and docling-serve's routes for Open WebUI, n8n, Dify ([details](../crates/docling-serve/README.md#drop-in-for-docling-serve-open-webui-n8n-dify-langchain)) |
| a LangChain loader | `docling_rs.langchain.DoclingLoader` |
| — | Node.js (`npm i docling.rs`), the browser (`npm i docling.rs-wasm`, [live demo](https://docling-project.github.io/docling.rs/)), Rust (`docling` crate), C (`docling-ffi`) |

Formats: everything MarkItDown converts — YouTube URLs since #681 (the
`_youtube_converter` shape, the transcript as timed cues), plain text
and source code, generic JSON / JSON Lines and XML (kept as code blocks)
included since #671, RSS / Atom feeds since #678 (the `_rss_converter`
shape — and RSS 1.0 RDF feeds, which MarkItDown rejects),
Jupyter notebooks since #673 (and, unlike MarkItDown, with the cell
outputs on request: `ipynb_outputs=text`) — plus ODF, RTF, legacy `.doc`/`.ppt`,
Apple iWork, DjVu, LaTeX, AsciiDoc, WebVTT, MHTML, `.eml`, JATS/USPTO/XBRL,
and local speech recognition for audio and video.

## Open gaps

None tracked: [open an issue](https://github.com/docling-project/docling.rs/issues)
for what you miss.

## Reproduce the numbers

```bash
pip install "markitdown[all]"
cargo build --release -p docling-cli
scripts/install/download_dependencies.sh
python3 scripts/conformance/markitdown_compare.py corpus
```

Run it on your own documents too — that is the comparison that counts.
