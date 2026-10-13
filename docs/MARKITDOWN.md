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

MarkItDown 0.1.8 (`markitdown[all]`) vs docling.rs 2.16.0 on 4 vCPU, no GPU,
with [`scripts/conformance/markitdown_compare.py`](../scripts/conformance/markitdown_compare.py);
full table and caveats in the README's
[MarkItDown comparison](../README.md#conformance-with-microsoft-markitdown).
Accuracy is scored against Python docling's groundtruth, which favours
docling.rs by construction; the timings do not depend on it.

| | MarkItDown | docling.rs |
|---|---|---|
| DOCX (38 files), warm | 5.04 s | **0.55 s** |
| One CLI call, cold (DOCX) | 1441 ms | **14 ms** |
| DOCX table-cell / heading recall | 0.81 / 0.78 | **1.00 / 1.00** |
| PDF table-cell recall (full pipeline) | 0.31 | **0.94** |
| PDF heading recall (full pipeline) | 0.00 | **0.91** |
| PDF text layer only, 18 files | 10.0 s | **0.7 s** (`text_layer_only`) |
| Text drawn into images (27 files) | 0.00–0.33 without an LLM; 1.00 with GPT-4o (45 API calls, $0.063) | **1.00**, local OCR, offline, no API key |

The full PDF pipeline renders every page and runs neural models — it is
slower than MarkItDown's pdfminer pass (49.3 s vs 10.0 s on the 18 PDFs) and
does the work that turns 0.31 table-cell recall into 0.94 and reads scans at
all. When only the text layer matters, `text_layer_only` is 14× faster than
MarkItDown and still scores higher on word F1 (0.97 vs 0.63).

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
| `StreamInfo(extension=…, mimetype=…, charset=…)` hints | same; without hints, magic bytes and container structure decide |
| `keep_data_uris=True` | same — pictures as `data:` URIs |
| `llm_client`, `llm_model`, `llm_prompt` | same request per picture; image inputs get the `# Description:` section |
| `requests_session` | same |
| XLSX `## <sheet>` headings | same |
| ZIP: `## File:` per entry | same |
| `markitdown FILE -o out.md` | `python -m docling_rs.markitdown FILE -o out.md` (also `-x`, `-m`, `-c`, `--keep-data-uris`, stdin) |
| `exiftool_path`, `style_map`, `docintel_*`, `cu_*` | accepted, ignored with a warning |
| plugins, `register_converter` | not supported ([#680](https://github.com/docling-project/docling.rs/issues/680)) |

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
| a service | `docker run -p 5001:5001 ghcr.io/docling-project/docling-rs-serve` — `POST /v1/convert`, and docling-serve's routes for Open WebUI, n8n, Dify ([details](../README.md#drop-in-for-docling-serve-open-webui-n8n-dify-langchain)) |
| a LangChain loader | `docling_rs.langchain.DoclingLoader` |
| — | Node.js (`npm i docling.rs`), the browser (`npm i docling.rs-wasm`, [live demo](https://docling-project.github.io/docling.rs/)), Rust (`docling` crate), C (`docling-ffi`) |

Formats: everything MarkItDown converts except generic JSON/XML, RSS,
Jupyter notebooks and YouTube URLs, plus ODF, RTF, legacy `.doc`/`.ppt`,
Apple iWork, DjVu, LaTeX, AsciiDoc, WebVTT, MHTML, `.eml`, JATS/USPTO/XBRL,
and local speech recognition for audio and video.

## Open gaps

Tracked, in roughly the order they matter to MarkItDown users:
[#669](https://github.com/docling-project/docling.rs/issues/669) (MCP server),
[#676](https://github.com/docling-project/docling.rs/issues/676) (picture alt text),
[#679](https://github.com/docling-project/docling.rs/issues/679) (document metadata),
[#675](https://github.com/docling-project/docling.rs/issues/675) (charset detection),
[#671](https://github.com/docling-project/docling.rs/issues/671) (plain text, JSON, XML),
[#673](https://github.com/docling-project/docling.rs/issues/673) (Jupyter notebooks),
[#680](https://github.com/docling-project/docling.rs/issues/680) (pluggable backends).

## Reproduce the numbers

```bash
pip install "markitdown[all]"
cargo build --release -p docling-cli
scripts/install/download_dependencies.sh
python3 scripts/conformance/markitdown_compare.py corpus
```

Run it on your own documents too — that is the comparison that counts.
