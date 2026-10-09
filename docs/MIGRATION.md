# Migrating Docling to Rust — docling.rs

A port of [docling](https://github.com/docling-project/docling) from Python to
Rust. This document is the **current status**: what is migrated, how it compares
to upstream docling, and what is intentionally not done yet. (The original
phased plan is kept at the end as history.)

## The migration in numbers

| | Lines of code | Files |
|---|---|---|
| Upstream Python: `docling` 2.114.0 (code wheel `docling-slim`) | 70,132 | 242 |
| Upstream Python: `docling-core` 2.87.1 (document model, serializers, chunkers) | 30,888 | 103 |
| **Upstream total** | **101,020** | **345** |
| docling.rs — the port itself (`docling-core`, `docling`, `docling-pdf`, `docling-asr`, `docling-cli`) | 47,456 | — |
| docling.rs — beyond upstream's packages (HTTP API, RAG, Python/Node/wasm bindings) | 11,327 | — |
| **docling.rs total (`crates/*/src/**.rs`)** | **58,783** | **121** |

Roughly **half the line count for the same behavior** — despite Rust carrying
type/lifetime annotations Python doesn't — because the port reimplements from
observed behavior rather than translating structure, and because byte-for-byte
conformance testing against live docling (not code review) is what pins
correctness. The Python side also leans on compiled dependencies that the Rust
side had to re-port or re-integrate natively (docling-parse's C++ PDF text
extraction became `textparse.rs`; HF `transformers` inference became hand-rolled
ONNX pipelines), so the scope ratio understates the ported surface.

**Timeline:** the first commit landed **2026-06-27**; the migration — every
input format including PDF/ML, ASR and video, plus the serve/RAG/bindings
extras — was done by **2026-07-23**: **26 days**, ~600 commits (development continues
past that point — serve async API, confidence scores, PDF-parity work), migrated by
[artiz](https://github.com/artiz) + Claude (Anthropic's Claude Code, doing the
bulk of the porting under review).

> **Status: the format migration is complete.** Every document format in
> docling's pipeline is supported — including **audio/ASR** (Whisper via ONNX,
> in `docling-asr`) — plus Markdown (legacy + a Rust-only *strict* mode),
> docling-native **JSON** output, **DocLang (`.dclx`)** output (docling 2.110's
> OPC archive), **image extraction**, and **MHTML** (docling#4184's
> `InputFormat.MHTML`). The declarative formats are pure-Rust and checked byte-for-byte
> against *live* docling; the PDF/image/METS ML path lives in `docling-pdf`
> (a pure-Rust PDF text/metadata parser and page renderer — no native PDF library; the docling-parse renderer plugin is a development oracle — + ONNX
> layout/TableFormer/OCR + a port of docling-parse's line sanitizer) and is also
> measured byte-for-byte against live docling — **6 / 14 PDF fixtures exact, 7 / 14
> whitespace-normalized** (see `PDF_CONFORMANCE.md`), with a snapshot baseline
> guarding against regressions. `cargo test` is green (unit tests + a 159-source
> output-regression suite).

**At a glance** (for a first-time reader from the docling side):

| | |
|---|---|
| **What** | A Rust port of docling's converter, backends, and discriminative PDF/ASR pipelines; same `convert → DoclingDocument → export_to_markdown()/json()` shape, single static binary, no Python/torch at runtime |
| **Conformance** | Declarative formats byte-for-byte vs *live* PyPI docling (most 100%, see §2); `.dclx` DocLang output ≈97% mean vs docling's own `.dclx`, OOXML all byte-exact (§2); PDF ML path 6/14 fixtures byte-exact, rest close; every optimization is gated on this not regressing |
| **Performance** | PDF ML pipeline **4.3× faster warm / 4.7× end-to-end** than Python docling at 2.3–2.6× less peak RAM (INT8 + SIMD, conformance-validated); declarative formats 20–60× warm, ~60× less RAM; XLSX sheets / PPTX slides additionally fan out over rayon (~2–3× on many-sheet/slide files, conformance byte-identical); details + methodology in [`PDF_CONFORMANCE.md`](./PDF_CONFORMANCE.md) |
| **Models** | docling's own checkpoints, no retraining: layout heron and TableFormer are format-converted to ONNX by `scripts/install/export_layout.py` / `export_tableformer.py` (CodeFormula by `export_code_formula.py`); PP-OCRv3 and Whisper tiny ship as their upstream ONNX exports; INT8 variants are calibrated post-training quantizations (`scripts/install/quantize_models.py`) |
| **Tracking upstream** | See [§9](#9-keeping-up-with-upstream-docling): conformance is measured against the *latest published* docling on demand, so an upstream release that changes output surfaces as a concrete per-fixture diff |
| **Not ported (by design)** | local in-process VLM full-page inference (§5 — the remote OpenAI-compatible VLM pipeline **is** ported, #77); inline formatting is baked into text rather than structured fields (§4). The optional enrichment models (picture classification, code, formulas) **are** ported — opt-in `do_picture_classification` / `do_code_enrichment` / `do_formula_enrichment`, ONNX like the rest of the stack |

---

## 1. Architecture

The layers mirror docling's:

| Layer | docling (Python) | `docling.rs` (Rust) |
|---|---|---|
| **Data model + serializers** | `docling-core` | `docling-core` — `DoclingDocument`, the `Node` tree, Markdown + JSON serializers, base64 |
| **Converter** | `docling/document_converter.py` | `docling.rs` — `converter.rs` (format dispatch + XML content sniffing) |
| **ZIP input** (#557, docling.rs extension — docling takes no archives) | — | `archive.rs`: `DocumentConverter::convert_archive` converts each document of a `.zip` on its own (lazy per-entry `Converted` / `Skipped` / `Failed`), entries vetted from the central directory before inflating (`ArchiveLimits`: entries, per-entry / total size, compression ratio; unsafe `..` paths, nested archives, encrypted and `__MACOSX` entries skipped); the CLI expands a named `.zip` into batch items (`out/<archive>/<entry>`), serve a `.zip` upload into a batch. Python `DocumentConverter.convert_archive(source)` yields an `ArchiveItem` per entry; Node `convertArchiveFile` / `convertArchive` (+ `*Async`, and the `DocumentConverter` methods) return `ArchiveItem[]`. `InputFormat` gains no variant and `convert()` still rejects `.zip` |
| **Backends** | `docling/backend/*` | `docling.rs` — `backend/*` (one per format) |
| **PDF/ML pipeline** | `docling/pipeline/*`, `docling/models/*` | `docling-pdf` — pure-Rust text layer + renderer, ONNX layout/OCR + assembly |
| **Audio/ASR pipeline** | `docling/pipeline/asr_pipeline.py` | `docling-asr` — symphonia decode + log-mel + ONNX Whisper |
| **Chunking** | `docling-core` chunkers (`HierarchicalChunker`/`HybridChunker`) | `docling-core::chunker`, re-exported as `docling::chunker` |
| **CLI** | `docling/cli` | `docling-cli` (incl. warm batch mode: `SOURCE... --output DIR`, `--input GLOB --output DIR [--jobs N]`, `--abort-on-error`, #489; repeatable `--to`, #491; `--list-input-formats` / `--list-output-formats`, #603 — Pandoc's discovery flags, which Python's CLI lacks: the extensions this build converts and the `--to` values, sorted, one per line) |
| **Beyond upstream's packages** | docling-serve (separate repo) | `docling-serve` (HTTP API), `docling-rag`, Python/Node/wasm bindings, GPU execution providers (`cuda`/`tensorrt`/`directml`/`coreml` features, `DOCLING_RS_EP`) |

```text
crates/
├── docling-core/   # DoclingDocument, Node model, markdown/json/doclang/doctags serializers, chunker.rs, confidence.rs
├── docling/        # DocumentConverter, source/format detection, backend/*.rs, ooxml.rs
├── docling-pdf/    # pdfium_backend (the page walk), textparse, render/, raster/, layout (RT-DETR/ONNX), ocr (PP-OCRv3/ONNX), assemble, mets
├── docling-asr/    # audio decode (symphonia), mel.rs, whisper.rs (ONNX), tokenizer.rs; nemo_mel.rs, parakeet.rs, vad.rs (Parakeet TDT, #508)
├── docling-onnx/   # shared ONNX Runtime EP selection (DOCLING_RS_EP, cuda/tensorrt/directml/coreml features)
├── docling-cli/    # `--strict`, `--to md|json|html|text|dclx|chunks|latex|pandoc`, `--images …`, `--pages`, `--ocr-lang`, serve subcommand
├── docling-node/   # Node.js/Bun N-API bindings (napi-rs), published to npm as `docling.rs`
├── docling-py/     # PyO3 bindings (maturin), published to PyPI as `docling-rs` (strangler-fig over docling-core)
├── docling-rag/    # RAG layer on top of the converter (chunking, embeddings, vector search, REST API)
├── docling-serve/  # HTTP conversion API (docling-serve analogue): sync + async/batch /v1/convert over a warm pipeline
└── docling-wasm/   # WebAssembly bindings: declarative converters + text-layer PDF in the browser
```

The public API is unchanged from day one:

```rust
use docling::{DocumentConverter, SourceDocument};

let result = DocumentConverter::new()
    .convert(SourceDocument::from_file("input.docx")?)?;
println!("{}", result.document.export_to_markdown());   // or .export_to_json()
```

---

### One option set across the surfaces (#577)

Every conversion option is one serializable struct, `docling::ConvertOptions`
(`crates/docling/src/options.rs`), with `validate()` and `apply()` in the
library: the CLI, docling-serve, the C ABI, the wasm module and the Python /
Node bindings parse their own input shape into it and share the rejection
rules, the messages and the mapping onto `DocumentConverter`; the engine
defaults are stated once (`DocumentConverter::default()`). Before, each
surface re-declared the fields and re-implemented the checks — the C ABI had
no `list_attachments`, the Python wrapper dropped five kwargs the native
class took, docling-serve's OpenAPI document was missing five request options
it accepted, and nothing caught any of it. The inventory test
(`crates/docling/tests/options_inventory.rs`) now holds every surface's
documentation to the central table (`docs/OPTIONS.md`, `OPTIONS` in
`options.rs`). docling has no single equivalent — its options are spread
over `PipelineOptions`, per-format backend options and docling-serve's
request model — so this is a docling.rs structure, not a parity item.

**2.0: `no_ocr` means docling's `--no-ocr`** (#611, a breaking change). A
script written for `docling convert` used to lose its headings, tables and
pictures here, because `--no-ocr` was docling.rs's skip-everything fast path.
Since 2.0 `--no-ocr` / `no_ocr` / `noOcr` / `DocumentConverter::no_ocr` is
docling's `do_ocr=False` — layout and TableFormer run, OCR never does —
`--skip-ocr` / `skip_ocr` / `skipOcr` / `::skip_ocr`, its old name, is still
read, and the fast path is `--text-layer-only` / `text_layer_only` /
`textLayerOnly` / `::text_layer_only` (Python's kwarg of that name, and
`do_ocr`, are unchanged). Replaying the corpus, `--text-layer-only` gives
byte for byte what `--no-ocr` gave on 1.104.3. The CLI also takes docling's
spellings `--page-range`, `--image-export-mode`, `--no-tables`,
`--pdf-password` (and the `password` option on every surface — `pdf_password`
before #625, still read — which the engine always had but no caller passed)
and `--output-file`; see
`docs/OPTIONS.md`.

**Encrypted Office documents (#624, #625).** A password-protected `.doc`,
`.docx`, `.xls`, `.xlsx`, `.ppt` or `.pptx` converts with the password —
`--password` / `--password-file` on the CLI, `password` everywhere else (the
PDF password option, renamed from docling's `pdf_password`, which stays
accepted; `--pdf-password` too) — a
docling.rs extension: docling's password option is PDF-only, and an
encrypted Office file fails there. Decrypted: OOXML Agile (Office 2010+) and
Standard (2007) encryption, RC4 CryptoAPI (2002+, 40- to 128-bit) and Office
97/2000 RC4 for the binaries; the decrypted bytes are msoffcrypto-tool's
(the OOXML package, the `.doc` and `.xls` streams byte for byte; a `.ppt`'s
every persist object). Not decrypted: XOR obfuscation (Word/Excel 95), ODF
package encryption, a `.ppt`'s `Pictures` stream (the backend does not read
it); the `dataIntegrity` HMAC is not checked. Without the password the error
is `<format>: document is encrypted (a password is required to open it)`, with
a wrong one `<format>: document is encrypted and the password is wrong` (422
from the server) — where a `.ppt` used to convert to an empty document,
`.docx`/`.pptx` failed as `bad zip`, an ODF package as `no content.xml` (now
"encrypted with an unsupported scheme"), and `.xls`/`.xlsx` in calamine's
words. A `.ppt` is recognized by the `CryptSession10Container` its current
user edit references ([MS-PPT] 2.3.7). Default passwords are tried after the
given one, as Office does without prompting: PowerPoint encrypts a file that
has only a *modify* password with `/01Hannes Ruescher/01`, so those convert
without a password; Excel's `VelvetSweatshop` opens workbooks protected only
against structural edits; Word and Excel leave a modify-password-only file
unencrypted. A mislabelled encrypted file (a `.ppt` named `.pptx`) reports the
encryption, not the declared format's parse error.

**The encryption error is typed (#636; docling has no equivalent — its
backends raise `DocumentLoadError` with a backend-worded message).**
`ConversionError::encryption()` returns the
[`EncryptionError`](../crates/docling-core/src/encryption.rs) behind any of
the above — `NeedPassword`, `WrongPassword` (the two `needs_password()` is
true for), `NotDecryptable(scheme)` (ODF, iWork, WordPerfect, XOR
obfuscation), `Malformed(field)` — found on the error's `source()` chain,
whichever backend raised it (the PDF reader's `PdfError::Encrypted`, the
Office decryptor, iWork, WordPerfect). No message changed: callers that
matched text keep working, callers that match the type stop needing to.
`docling-serve` keeps the 422 and adds `"code"` to the error body
(`password_required` / `wrong_password` / `not_decryptable` /
`malformed_encryption`; on an async job's status and result too — never
on any other error; an encrypted PDF on the warm-pipeline route, which
answered 500, is this 422 now — and not in the upstream-shaped `/v1/convert/file`
envelope, which has no field for it); Python raises the `ConversionError`
subclasses `PasswordRequiredError`, `WrongPasswordError` and, for the
rest, `EncryptionError` (`docling_rs.exceptions`); the CLI message and exit
code are unchanged, as is the Node binding's error (a reason string).

## 2. Format coverage

Conformance is measured against the latest **published** docling (installed from
PyPI; run via `scripts/conformance/conformance.sh <fmt>`), not the committed groundtruth
`.md` (which predates docling-core's current table serializer — see §4).
"Exact" = byte-for-byte.

Full sweep against docling **2.129.0** (2026-09-20,
`scripts/conformance/full_conformance.py` — Markdown byte-for-byte and JSON
structurally, i.e. every key and value but `origin`/`version` and re-encoded
image bytes; the files upstream itself cannot convert are left out):

| Format | Files | Markdown exact | JSON identical | Residual |
|---|---|---|---|---|
| DOCX | 38 | **38** | **38** | — (#527: a JPEG picture is stored as docling's `ImageRef.from_pil` PNG, re-encoded from libjpeg-exact pixels) |
| HTML | 32 | **32** | **31** | `html_code_snippets`: 2.129 maps a `language-python` class to `code_language: Python`, we write `unknown` |
| PPTX | 8 | **8** | **8** | — |
| XLSX | 14 | 13 | 13 | `xlsx_02_sample_sales_data`: 20 date cells print `2024-01-01 0:00:00` by docling PR #4628's rules (#634, `h` unpadded), where the released docling still writes `str(value)` — exact again once that PR merges |
| CSV | 9 | **9** | 6 | a ragged file's missing trailing cell: we record an empty `TableCell`, docling records none (the `grid` agrees) |
| Markdown | 11 | **11** | 6 | the five files with a raw HTML block: upstream re-converts those through its HTML backend over docling-core's HTML export, which has no counterpart here (the flat export stays) |
| DeepSeek-OCR Markdown | 3 | **3** | n/a | — (2.129's first-row-only `column_header` for HTML tables ported) |
| AsciiDoc | 5 | **5** | **5** | — (JSON: docling's `parents`/`indents` tree — title root, headings at their level, nested `list` groups, body-level captions) |
| LaTeX | 8 | **8** | **7** | JSON: docling's body-flat tree from the pylatexenc-faithful walk (#466) — `title`/`text` for the preamble metadata, `section_header` levels, `paragraph` for a chars node's middle part vs `text` for buffered text, `formula` items, `list` groups whose items are `enumerated: false`, `figure` section groups, `Image: <path>` caption items the picture references, `tabular` cells with `\multicolumn`/`\multirow` read the way upstream reads them. `1706.03762` differs in one `tabular` only: upstream writes a span into the cell's end offsets but leaves `row_span`/`col_span` at 1, here the two agree. PDF figures carry no image payload (upstream renders them with pypdfium2); raster figures are embedded as PNG like upstream's `ImageRef.from_pil` |
| JATS | 7 | **7** | **7** | — (JSON: docling's `_walk_linear` parent threading; the `.txt` copy of `elife-56337` now routes to the JATS backend by its JATS DOCTYPE, as docling's format detection does) |
| USPTO | 9 | **9** | **9** | — (JSON: the SAX handlers' `level`/`parents` tree — everything under the title, `ABSTRACT`/`CLAIMS` as level-2 headings holding `paragraph` items, `<heading level>` nesting, table placeholders at `</table>` filled with `XmlTable` cells that list a spanning entry once per covered column; `tables_ipa20180000016` fails in upstream) |
| WebVTT | 4 | **4** | **4** | per-cue `source` (`kind: track`, start/end seconds, identifier, voice), inherited `formatting`, inline `WebVTT cue span` groups |
| ODF | 7 | **7** | **7** | — (#527: `odf_presentation_02`'s JPEG is stored as PNG like docling's) |
| EPUB | 1 | **1** | **1** | — |
| Email (`.eml`) | 2 | **2** | **2** | — (`.msg` is a docling.rs extension) |
| iWork Pages | 1 | **1** | **1** | — (the password-protected and nested-dir fixtures fail in upstream) |
| iWork Keynote | 5 | **5** | **5** | — (docling#4330 + docling#4376, 2.130+; #466 — both container generations and the nested 2018 index; charts as classified pictures with their data table and title caption, the fixture built the way upstream's test builds it: the Numbers chart placed on the deck's table slide) |
| EBCDIC | 3 | **3** | **3** | — |
| DocLang (`.dclg`/`.dclx`) | 15 | **15** | **15** | — (JSON: a call-for-call port of docling-core's `DocLangDocDeserializer` — `doclang_tree.rs` — inline groups, list-item virtual text, rich cells, `<location>` provenance, 512×512 pages; Markdown: docling-core's dropped `<!-- missing-text -->` and hard-break rich-cell padding ported) |

"The flat shape" is the JSON derived from the flat node stream — `text` for
docling's `paragraph`, `-` list markers, no heading nesting or inline groups —
that HTML, DOCX and PPTX left behind for the item tree (`DoclingDocument::tree`);
the same treatment is what closes those columns.

### Declarative formats — pure Rust, no models

| Format | Backend | Status |
|---|---|---|
| Markdown | `markdown.rs` (pulldown-cmark) | **10/10 exact**; **JSON: structurally identical to docling 2.129's on 5/10** — `md_tree.rs` rebuilds marko's CommonMark element tree from a second, extension-free pulldown parse (what upstream's parser sees: `~~x~~` stays text, a pipe table is a paragraph of `\|` lines) and ports `MarkdownDocumentBackend._iterate_elements` call for call: a heading or list item is created lazily on its first run (`title`/`section_header` of level − 1; `list_item` with `enumerated` and a `{start + count}.` marker), a paragraph or heading of several runs opens an `inline` group of one `text` per run with `formatting` (emphasis/strong) and `hyperlink` (AnyUrl-normalized), a code span is a `code` item (the container's own text when it is the only run), a fenced block a `code` with `detect_code_language(hint)`, an image a `picture` whose `"title"` is a body-level caption and whose alt runs follow as text, a nested list hangs off the outer list's last item, a `|` paragraph is buffered into a body-level table (header row 0, delimiter dropped, rows cut/padded to the header's width, formatting markers dropped), and hard/soft breaks merge the next run into the previous text (`\n` / space). The five files upstream routes through its HTML backend (a raw HTML block → `export_to_html` → `HTMLDocumentBackend`) keep the flat export: there is no HTML serializer here to feed that path. (re-verified vs docling 2.129; docling#4202: every text-format input (Markdown, CSV, AsciiDoc, WebVTT, LaTeX, JATS/USPTO XML, …) decodes like docling's `decode_text` — a UTF-32/UTF-8/UTF-16 BOM settles the encoding and is dropped, otherwise strict UTF-8, otherwise windows-1252 with a warning when under half of the non-ASCII bytes form UTF-8 sequences, and an error for *damaged* UTF-8 (≥ 50%) or a byte cp1252 leaves undefined, where an undeclared non-UTF-8 file used to fail outright; docling's `TextBackendOptions.encoding` (`MarkdownBackendOptions(encoding="shift_jis")`) is the `encoding` option on every surface — `DocumentConverter::encoding` / `SourceDocument::with_encoding`, `--encoding`, serve's `encoding` field/param, the Python kwarg, the Node option, the FFI key — a WHATWG label or Python codec name decoded strictly, an undecodable byte or unknown label failing the conversion like docling's `DocumentLoadError`; JSON 2/10 — flat-path bullets carry `marker: "-"` where docling writes `""`); #319 (docling 2.122's docling#3817): tables written without edge pipes (`Region \| Q1` over `--- \| ---`) are normalized up front so pulldown parses them like the canonical spelling; a leading UTF-8 BOM no longer hides the first heading (docling#4109); #466 (docling#4019, 2.126; `line_breaks.md` mirrored, exact in Markdown and JSON): a line break inside a paragraph, heading or list item is kept — a hard break (two trailing spaces or a backslash) joins the runs around it with `\n`, GFM `  \n` once serialized, a soft one with a space, two runs of the same formatting merge into one item across the break (`**Bold A**` ⏎ `**Bold B**` → `**Bold A Bold B**`) and differently formatted ones stay separate items with the `\n` on the second; docling#4313/#4371: a `\|` and every character-reference spelling of `|` (`&#x7C;`, `&verbar;`) is cell content — carried as `&#124;` until the row is split, so it neither opens a table nor adds a column, and in prose stays a literal pipe (`escaped_pipes.md` is our own fixture) docling#4318 (2.130.0-33): a table cell's snippets join the row buffer unstripped, so `**C** Cadre` keeps its space. |
| CSV | `csv.rs` (`csv` crate) | **9/9 exact** (re-verified vs docling 2.129; docling#4260: Python's `strict=True` quoting errors — a closing quote followed by text, a quote left open at EOF — are load errors here too, checked up front since the `csv` crate is lenient about both; JSON 6/9 — for a ragged file we record an empty `TableCell` for a missing trailing cell where docling records none, the `grid` agrees); `.tsv` routes here too (#208 — the delimiter sniffing already covers tabs; a docling.rs extension, upstream accepts only `.csv`); #319: a quoted field spanning lines no longer defeats delimiter sniffing (4 KiB fallback sample, docling#3985) and an Excel-style leading BOM stays out of the first header cell (docling#4098) |
| HTML | `html.rs` (scraper/html5ever) | **32/32 exact** vs docling 2.129 (docling#4287: a zero `colspan`/`rowspan` defaults to 1 like a missing one, so the cell no longer drops out of the grid; spans read python-style — the leading digits when the attribute starts with one, `"2px"` → 2; JSON 31/32 — `html_code_snippets`: 2.129 maps a `language-python` class to `code_language: Python`, we still write `unknown`) modulo the stored corpus's compact-table format (docling 2.126 groundtruth; #371: non-UTF-8 input decodes in BeautifulSoup's `UnicodeDammit` order — BOM, then the declared XML/`<meta charset>`/`http-equiv` encoding (bs4's search windows, WHATWG labels via `encoding_rs`, so `iso-8859-1` reads as windows-1252 like a browser), then strict UTF-8, then windows-1252 — instead of failing the UTF-8 check; bs4's optional chardet/charset_normalizer guess between declaration and fallback is the one step not reproduced; #364: docling#4050's `<figure>` dispatch — every child but the `<figcaption>` is walked, a figure's pictures take the figcaption as caption, a figure without a picture attaches the figcaption to its leading table or emits it as a standalone caption item (`wiki_duck`'s `[Mallard duckling preening](/wiki/Mallard)`); pictures folded into a list item print after the item line with plain newlines, the GFM hard-break rule applying to the item's own text only); `wiki_duck` — rich table cells, caption run spacing, indicator images, `<footer>` furniture all match docling 2.112; #284: an *unclosed* inline tag (`<a name=…>`, `<b>`, `<font>` — endemic in legacy authoring-tool HTML) legally swallows subsequent blocks under html5ever's spec parsing, where Python's parser recovers by reparenting — inline wrappers hiding structured blocks (tables, lists, headings, code, figures) are now block-walked so the structure surfaces; pure text containers under a well-formed `<a href>` keep rendering as links; `aria-hidden` subtrees are dropped like docling's `_is_invisible_tag` does (Wikipedia's decorative logo icon and its sticky-header duplicates), and an anchor wrapping several images hangs its href on each captioned picture; #382: docling#4216's header flags — a `<tr>` of `<th>`s that each span several rows is a pivot table's *row*-header row, so its cells are `row_header` and not `column_header` (they label the rows they span into, and flagging them as column headers made docling-core 2.96 fold the first data row into the Markdown header), and a lone `<th>` beside `<td>` data is a row header too; all 66 cells of `example_08` now match upstream's regenerated groundtruth flag for flag. **JSON: structurally identical to docling's on 32/32** (every key and value but `origin`/`version`; `wiki_duck` modulo one caption item — its stored JSON predates docling#4050, whose figure dispatch the refreshed Markdown groundtruth already shows). The flat node stream that Markdown / DocLang / LaTeX render is not the shape upstream's *JSON* has, so `html_tree.rs` ports `HTMLDocumentBackend`'s document construction call-for-call into a docling-core `ItemTree` (`DoclingDocument::tree`) that the JSON export serializes instead of deriving a tree from the nodes: everything after a heading nests under it (`h1` = title, `h2`–`h6` = `section_header` level `n-1`, `header-N` section groups filling a skipped level); a paragraph is docling's annotated parts (one per source text node with its ancestor `<b>`/`<i>`/`<u>`/`<s>`/`<sub>`/`<sup>`/`<code>` formatting and `<a href>`), adjacent same-annotation parts merged, two or more parts an `inline` group of one `text`/`code` item each carrying `formatting` and `hyperlink`, one part a lone item; `<br>` splits (`\n` for one, a new paragraph for two); a mixed-formatting `<li>` is an empty `list_item` over an inline group, a `<dt>` a bold item whose `<dd>`s form a `descriptions` list group under it, `<ul>` directly inside `<ul>` its own nested group; a rich table cell's items are re-parented under a `rich_cell_group_{tables}_{col}_{row}` group of the table (named with upstream's lazily advanced column, i.e. the *previous* cell's), the cell carrying `ref` to it, cell text docling's raw `get_text`, declared spans unclamped; a picture's caption is a body-level `caption` item; the `<title>`, everything before the first heading and a `<footer>` (a `section` group) are `furniture`; `<details>` a `section` group; `field_value` items carry `kind`; items are numbered in upstream's creation order. Markdown / DocLang / LaTeX / chunks are untouched (they keep reading the nodes); #466 (docling 2.130+): a `<table>`'s own `<caption>` (looked up non-recursively) is a `caption` item created ahead of the table under the current parent and linked from the table's `captions` (`html_rich_table_cells`: four captions, JSON structurally identical to upstream's regenerated groundtruth), text nodes collapse every whitespace run to one space on extraction (docling#3973, so a table flattened into an ancestor's cell reads `I II III IV`, `table_06`), and a GFM task-list item — `<li><input type=checkbox checked> done</li>` with no block content or custom checkbox markup — is one `checkbox_selected`/`checkbox_unselected` item carrying that text under the list group, not a list item plus an empty checkbox (docling#4401) |
| AsciiDoc | `asciidoc.rs` (regex) | **4/4 exact vs docling 2.129** — docling#4290: a rowspan-only cell specifier (`.2+\|`) is recognized (AsciiDoc writes `[colspan][.rowspan]+`, either number optional); a block title in front of a literal block is the code item's *caption*, which docling-core renders after the block; docling 2.127+ parents a heading to its nearest *present* ancestor level and a picture to the current section (both used to hang off the body root and render after the whole tree; the deferral is gone), and emits a block title in front of a list as a **bold paragraph** (`.Procedure` → `**Procedure**`, a list group having no caption slot) — `asciidoc_02`/`_03` groundtruth refreshed to 2.129; #365 ports docling 2.125–2.126. docling#4118: the item pattern widens to `*`, `-`, `.`/`..`/`...`, `1.`, `a.` (a dotted marker carries its own depth, so `..` nests under `.` with no indentation), a `....` literal block becomes a code item, a lone `+` keeps the next literal block or image *inside* the open item (printed after the item line, indented to its depth, like the pictures the HTML backend folds into an `<li>`), and a non-list line now ends the list and is parsed instead of being swallowed. docling#4156: image loading moves behind the converter's `fetch_images` (docling's `fetch_images` + `enable_local_fetch`/`enable_remote_fetch`) — by default an `image::` macro yields a picture with **no** `ImageRef`, where docling ≤ 2.125 fabricated a 128×128, 70-dpi `file://…` one; docling#4173 dropped `max_image_data_base64_bytes`, which was never ported. Numbering follows docling-core: an explicit `12.` marker prints verbatim, every other enumerated item is numbered by its *position among the group's children* — where a nested list is a sibling of the items, so it takes a position (`. a` / `.. b` / `. c` → `1.`, `1.`, `3.`) — and a group whose first item is a bullet renders all of them as bullets. Three deviations: (a) a line that is exactly `word.` stays a paragraph, as it does for docling reading a **stream**; reading the same bytes from a **path** docling keeps the line's newline, so the widened `\w+\.` pattern makes `Intro.` an empty list item and loses the text; (b) symmetrically, our unterminated `....` block matches docling-on-a-path — docling-on-a-stream appends a trailing blank line to it; (c) a paragraph still pending when a list opens is emitted as a paragraph, where docling re-parents it into that list group and glues it to the last item. (A fourth is gone with #385: a break in *explicit* markers — `1.` then `5.`, or `*` beside `1.` — no longer reads as a new list, since the serializers take every list boundary from the backend's flag instead of guessing it from the numbering.) A picture or literal block nested in a list item is carried in the item's own text (this node model is flat), so Markdown, LaTeX and DocLang render it exactly where docling does, while the JSON has no separate `picture`/`code` item for it and an in-list picture keeps no fetched bytes — the same tradeoff the HTML backend already makes for images inside an `<li>`. Pre-existing: the line that closes a table (docling's `elif in_table` arm consumes it) is parsed here instead of dropped, so a text line written directly under the last table row survives; **JSON 4/4 structurally identical to docling 2.129's** — the parser builds docling's item tree alongside the flat nodes, mirroring `_parse`'s `parents` / `indents` bookkeeping: the title is the root, a heading is parented to the nearest present ancestor level and clears the deeper ones, every list opens a `list` group at the next level (a deeper indent nests another group, a dedent pops levels while an outer group exists, a closed list leaves its indent recorded exactly like upstream), a `+`-continued picture or literal block is a child of the open list item, block titles are body-level `caption` items the table / picture / code references (a bold paragraph when they precede a list), and a table's ragged rows keep only the cells they have; #466 (docling 2.130+, own `delimited_blocks.asciidoc` fixture): a `----` listing block is a `code` item like the `....` literal block, a `[source,lang]` attribute line right before it sets `code_language` (the hint trusted over the content, `detect_code_language`), the `====` / `****` / `____` / `--` / `+++` delimiter lines of example, sidebar, quote, open and passthrough blocks are consumed when a matching closer exists and their content read as ordinary lines (docling#4400); a run of `*`s nests a bullet like a run of `.`s nests a number (docling#4403); a table whose rows never completed emits no empty grid (docling#4300) docling#4428 (2.130.0-33): a section header flushes the pending paragraph into the section it was written in. |
| DeepSeek-OCR Markdown | `deepseek.rs` | **3/3 exact vs docling 2.129** — docling's `_parse_table_html` flags a `<th>` as `column_header` on the first row only, so a two-row `<thead>` keeps its second row in the body (spanning cells repeated, numeric columns left-aligned by that text row) where the HTML backend folds both into one ` - `-joined header; the annotated table's header flags are trimmed to row 0 after the HTML parse (`deepseek_example` groundtruth refreshed); (auto-detected VLM-token variant); Unlimited-OCR grounding output (#322 / docling#3944) normalizes into this shape and shares the parser |
| Chandra layout HTML | `chandra.rs` (#322) | docling 2.123–2.125's `parse_chandra_html` semantics on our node model: `data-bbox`/`data-label` divs → headings, text, span-aware tables (Form-held tables included, docling#4135), lists, figures, formulas, code, page furniture; `<br>` is spacing (docling#4092); 0–1000 boxes → DocLang 0–511 `Located` provenance |
| XLSX | `xlsx.rs` (calamine) | **13/13 exact** Markdown vs docling 2.127.0 (groundtruth refreshed to it, the #395 reporter's file mirrored as `xlsx_empty_merge_before_data`), and the **JSON is structurally identical for 13/13** (re-verified against docling 2.129: the flat `Node::Prov` stream already yields upstream's shape, so the spreadsheet backend needs no item tree) — everything but `origin`/`version` and, where a sheet holds a raster image, the PNG bytes PIL re-encodes: items are numbered as docling creates them (a sheet's tables with their labels, then its images, then its charts, in drawing order — the `seq` a `Node::Prov` carries) while the group's children follow docling's position sort, stable over that creation order; every sheet is a page (`pages`, sized like docling's `_find_page_size` — the largest right/bottom edge of its items in cell units, 0×0 for an empty sheet) and every item carries docling's provenance verbatim — its cell-index box, top-left origin, `charspan` `[0, 0]` — through a `Node::Prov` wrapper the JSON reads (the 0–511 DocLang grid cannot round-trip those integers; DocLang, Markdown, LaTeX and the chunker render the wrapped node unchanged); a chart's title caption is a sibling of the picture in the sheet group, listed before it, with the chart's box and a charspan over the caption text, as docling's office backends add it (PPTX charts get the same); `TableData.orientation: rot_0` is written for every table and chart grid; a picture's `image.size` is floats and `image` precedes `annotations` — pydantic field order; #405: a chart *sheet* exposes its chart the way a worksheet's does — the sheet's `absoluteAnchor` drawing (no cell range; docling's bbox is `(0, 0, 0, 0)`) was skipped, leaving the group empty — and every chart picture now carries docling's `meta` in the JSON: `classification` with the chart kind as its one prediction and `tabular_chart.chart_data`, the series reconstructed as a `TableData` (row 0 the series names as column headers, the category column as row headers), for XLSX and PPTX/ODF/DOCX charts alike; `xlsx_chart_sheet` (data sheet + chart sheet + the same chart on a worksheet) is mirrored with 2.127 groundtruth and the stale `xlsx_03_chartsheet` groundtruth refreshed; #410: a merged range is *one* `TableCell` in the JSON — the anchor's offsets with `row_span`/`col_span`, repeated across the grid positions it covers, as docling writes it — where we wrote a 1×1 cell per covered position with the anchor's text copied into each (Markdown was already right, the expanded grid). The serializer derives the spans from the structure overlay's continuation flags, so backends without first-class cells (XLSX, PPTX `rowSpan`/`gridSpan`, DocLang) all get them; `xlsx_merged_range` mirrored, and the refreshed `xlsx_01`/`_04`/`_07` and `powerpoint_sample` JSON now match upstream's cell spans exactly; #395: a sheet is scanned in docling's `_find_true_data_bounds` frame — the union of the valued cells and *every* merged range — so a merge that starts above or left of the first value no longer underflows the rebase onto calamine's clipped range (it panicked), and, as upstream does, an empty merged range no longer seeds a table of its own (docling seeds only from a cell that carries a value) (#366: `.xltx`/`.xltm` templates route to the same reader, docling#4178) (incl. chart captions/classification/data grids); the JSON carries docling's structure too — one `sheet` group per worksheet (named after it, `invisible` content layer for a hidden sheet, which puts its tables in the JSON instead of dropping them) and every cell note as a `comment-{sheet}-{cell}` `comment_section` on the notes layer, with `comments` back-refs on the item whose cell range covers the commented cell (docling's `_find_cell_item`). Note the upstream asymmetry the refs mirror: a spreadsheet comment goes through docling-core's `add_comment`, which links the **note text** (`#/texts/N`), while the docx backend overwrites that with the **group** (`#/groups/N`) so replies group together. Markdown and DocLang are unaffected — docling emits no sheet heading (that `## Sheet1` lives only in older stored groundtruth) and DocLang has no group element; #466 (docling#4353): a threaded comment keeps *every* message of its thread — ordered root-first by the `id`/`parentId` links, replies by timestamp then document order, orphans last — one `[author: …, time: …]: …` note per message inside the cell's single `comment-{sheet}-{cell}` group and one `comments` back-ref per note (`xlsx_comments`: 5 notes, we kept one per cell); and docling#4302: the whole bounding rectangle of a found table is marked visited, so a non-empty cell inside it that is not 4-connected to the region no longer seeds a duplicate fragment table; #634: a cell prints the value Excel *displays* — its number format applied by docling PR #4628's rules (`backend/numfmt.rs`, a port of that PR's `_format_excel_value` / `_format_excel_number` / `_format_excel_date` and openpyxl's `is_date_format`, with the PR's own test table as unit tests and its code run through openpyxl as the oracle for the fixtures): a `$`/`%` section (`"$"#,##0.00`, `0%`, `[$$-409]…`, negative and zero sections) with Excel's half-up rounding (`0.125` at `0%` → `13%`), a date format token by token (`mmm-yy` → `Feb-25`, `"Due "mmm d`, `h:mm AM/PM`, `mm` next to `h`/`ss` as minutes, `h`/`d`/`m` unpadded), the built-in `mm-dd-yy` (14) / `m/d/yy h:mm` (22) as ISO; everything the PR does not interpret — `"EUR"` and other currency words, plain `#,##0`, `00000`, `[h]:mm:ss`, conditions, other locale markers, `#` optional decimals, scientific — stays `str(value)` as before, so no cell reads worse. The codes come from `xl/styles.xml` + each sheet's `c/@s` (calamine exposes none of this); `.xls` reads the BIFF `FORMAT`/`XF` records and the cells' `ixfe`; `.xlsb` keeps printing raw. Released docling (2.135) still writes `str(value)`, so the 20 date cells of `xlsx_02_sample_sales_data` differ from its groundtruth (`0:00:00`) until #4628 merges; the mirrored groundtruth is left as upstream has it. Fixtures: the issue's `xlsx_number_formats_min.xlsx`/`.xls`, docling#4619's "Order" sheet and the PR's review table (`xlsx_number_formats.xlsx`), a 1904-epoch workbook. No opt-out, as the PR has none |
| XLSB (binary Excel 2007+) | `xlsx.rs` → calamine's `Xlsb` reader (#210) | **docling.rs extension — upstream has no XLSB backend**; tables, hidden-sheet layering and page breaks match the XLSX path; drawings/charts/comments/merges aren't exposed by the binary reader |
| PPTX | `pptx.rs` (roxmltree) | **8/8 exact**; **JSON: structurally identical to docling 2.129's on 8/8** (every key and value but `origin`/`version` and the PNG bytes PIL re-encodes) — the flat node stream is not the shape upstream's JSON has, so the slide walk fills docling's `ItemTree` alongside it, as `MsPowerpointDocumentBackend._walk_linear` does: a `chapter` group per slide; `paragraph` (not `text`) for a non-title paragraph; a `list` group per run of bullets whose items carry `marker: ""` (bullet) or `"N."` (numbered); a table holding only its non-empty cells — `_handle_tables` drops the blanks, skips an all-blank table, and makes every first-row cell a column header; a picture at the file's dpi (python-pptx's `Image.dpi`: PNG `pHYs`, JFIF density else EXIF resolution, BMP/TIFF resolution, rounded half-to-even, 72 when absent or outside 1–2048), and no picture at all for an undecodable non-metafile image; a chart as a classified picture with a slide-level caption; speaker notes as a notes-layer `text` with a zero `TOPLEFT` box; review comments as `comment_section` groups named `comment-slide{N}-{idx}` on the notes layer, each holding its note — and every item with upstream's provenance verbatim: the shape's raw EMU box as `BoundingBox.from_tuple((l, t, r, b), TOPLEFT)` — python-pptx's top-left EMU, tagged so since docling#4294 (#466; the box used to be tagged `BOTTOMLEFT`, which read the tuple as `(l, b, r, t)` and swapped the vertical edges — the eight mirrored groundtruth files are upstream's regenerated ones) — with a `charspan` over the item's own text. Slides still convert in parallel; each fragment is renumbered into creation order by `ItemTree::append`. The four groundtruth files that predated docling#4190's per-paragraph charspan (`powerpoint_comments`, `_issue_2663`, `_sample`, `_unrecognized_shape`) are refreshed to 2.129's output; #390 (docling#4190): a chart's title caption is a child of the slide group, listed ahead of the chart picture, with the chart's box and a charspan over the caption text, and every paragraph's `charspan` is its own (docling used to reuse the shape's) — the mirrored `pptx_chart` groundtruth is upstream's post-#4190 copy and our JSON is structurally identical to it; a subtitle placeholder stays a `text` paragraph on purpose (docling#3785, re-settled in #4190) — not a bug; #406: a paragraph whose bullet or numbering comes from the owning shape's list style (`a:txBody/a:lstStyle/a:lvl{N}pPr/a:buChar|a:buAutoNum`, selected by the paragraph's `a:pPr/@lvl`) is a list item, as docling's `_get_effective_list_marker` resolves it — a `buChar`/`buBlip` bullets it, a `buAutoNum` numbers it, a `buNone` keeps it a paragraph, and an indented paragraph with no marker anywhere is a list item (docling's `level > 0` fallback); and since docling#4397 (#466) the level also nests: a paragraph at a deeper `a:pPr/@lvl` opens a `list` group under the last item of the enclosing list, a shallower one pops back, every group numbers its own enumerated items, and a non-list paragraph closes them all — docling's `_OpenList` stack (`pptx_inherited_bullets` indents its inherited level-2 and level-3 items); #627 (docling#4581, upstream `main` after 2.135.0): the cascade continues past the shape — a placeholder reads the layout placeholder with the same `idx` (idx only, as python-pptx's `layout.placeholders.get`; a missing `idx` is 0) and, only when there is one, the master's `p:txStyles` bucket for its type (`body`/`obj`/no type → `bodyStyle`, `title` → `titleStyle`, everything else — `ctrTitle` and `subTitle` included — `otherStyle`); the old "body placeholder is a list" default is gone, so the default template's "Section Header" body and any layout or master `buNone` give paragraphs, a master `buAutoNum` numbers the body, and a subtitle takes `otherStyle`'s marker (with it, a centred title can become a list item, as upstream's does); a numbered group starts at its first paragraph's `a:buAutoNum/@startAt` (docling's `_get_auto_number_start`), and a bullet that joins a numbered group renders `N.` by position in Markdown, as docling-core does — `crates/docling/tests/data/pptx/list_cascade/` (the reporter's A–H decks plus I–K) matches upstream's Markdown and JSON 11/11; #402: a slide's speaker notes and review comments reach the **JSON** as text items on docling's `notes` content layer (they were parsed all along, but only DocLang rendered them, so a consumer reading JSON alone could not see them). Markdown is unchanged — it serializes the body layer. Each slide is also a `chapter` group named `slide-{0-based}`, as docling writes it, and every slide is recorded as a page sized in EMU, so items carry provenance: on `powerpoint_sample` the group tree (9 groups, same refs, names, labels and parents), the page map and every `charspan` are identical to docling's, and the `prov` boxes are docling's raw EMU (they used to be expanded from the 0–511 `<location>` grid, off by up to one grid step). An item with no geometry — a speaker note — carries docling's zero bbox rather than a box spanning the slide. (docling 2.120 parity — #320: 3-D chart variants are `other_chart` without a data grid like python-pptx's unsupported plots (docling#3972), a shape at x = 0 EMU keeps its own bbox (docling#3990); #249: slide shapes convert in visual reading order — top-sorted, 0.05"-tolerance row banding, left-to-right within a row — instead of XML/z-order, at slide level and inside groups); docling#4089 (EMF/WMF pictures kept as positioned pictures, rendered through LibreOffice when it is installed and without image data otherwise) — here they are always rendered, in-process (#536, `metafile.rs`), and a metafile that draws nothing stays the payload-less picture; #575 (a deliberate divergence): equations are kept — PowerPoint writes OMML as an `<a14:m>` paragraph child (python-pptx has no notion of it, so docling's text is the paragraph without the formula), converted here by the DOCX backend's OMML → LaTeX port (`omml.rs`): a paragraph of equations alone is a `formula` item (`$$…$$` in Markdown), one with words carries them inline as `$…$`; and the slide's `mc:AlternateContent` is settled before parsing by the DOCX backends' `mc.rs` — a text box holding an equation is an `a14` Choice with a picture Fallback, so the Choice is read (python-pptx skips the whole block, Choice and Fallback alike; any other Choice it cannot read now yields its Fallback shape instead of nothing). The corpus has no such block in a shape tree (only `p14` slide transitions), so 8/8 is unchanged |
| DOCX | `docx.rs` (roxmltree) | **36/36 exact** vs docling 2.129 (docling#4243: a fragment-only relationship target (`Target="#anchor"`) is dropped as upstream strips it before python-docx opens the package — the hyperlink keeps its text, no target; JSON 36/36 — a JPEG picture is written as the PNG PIL re-encodes it to, from the same libjpeg pixels, #527) modulo the committed corpus's compact-table format (§4; 26/33 byte-identical to upstream's stored `.md`, the other 7 differ only in `\| - \|` vs padded separators) — #385: a list item's `first_in_list` is Word's list identity — a `numId` the previous list item did not carry, or body text since it, opens a new list; the same `numId` continues one across nesting and across an empty spacer paragraph (docling's `_manage_list_structure` and its ListGroup cache) — and the serializers draw every list boundary from that flag, no longer guessing it from a kind flip or a number gap (`docx_lists`/`docx_list_blank_spacer` unchanged, `unit_test_headers_numbered` now nests its list groups as upstream does); #409: a code paragraph's text is python-docx's `Paragraph.text`, as docling's `raw_paragraph_text` is — so an in-paragraph `<w:br/>` (its own run, or between the `w:t`s of one run) is a newline and a tab is a tab, where flattening the subtree to `w:t` alone joined `total = a + b` and `print(total)` into one line of the fenced block; plain table cells read the same way (python-docx's `cell.text`); #408: the trailing run group of a paragraph closes under the format that *opened* it, as in docling's `_get_paragraph_elements` — a whitespace-only run never opens a group, so bold/italic text followed by Word's unformatted trailing-space run keeps its `**…**`/`*…*` (we took the last run's plain format and dropped the markers, in body text and list items alike); both reporter files are mirrored (`docx_bold_tail`, `docx_code_breaks`) and match upstream's Markdown byte-for-byte; #400: a run's inner content follows python-docx's `CT_R.text` — where docling's paragraph text comes from — so `<w:noBreakHyphen/>` is the plain `-` it renders as (dropping it glued `In` + `Transit` into `InTransit`), `<w:ptab/>` is a tab, and a `<w:br>` is a newline only when it is a *line* break: a page or column break has no text equivalent there; #389: docling#3946/#3951 — a table cell Word wrapped in a content control (`w:tr/w:sdt/w:sdtContent/w:tc`, how it writes cover pages and document-property fields) is collected as a cell of its row, so a 1×1 layout table keeps its content and a wrapped cell no longer slides every later cell of the row a column to the left; the mirrored `docx_sdt_table_cells` fixture matches upstream's grid and text items exactly; docling 2.126 parity (#363): docling#3917 resolves a style's numbering through its `basedOn` chain with `numId` and `ilvl` inherited independently (Word's stock `heading 2` carries only `ilvl` and takes `numId` from `heading 1`; refreshed `unit_test_headers_numbered` adds the inherited `2.3` heading), docling#4087 renders list markers with the level's `numFmt` — letter (a…z, aa…), roman, `decimalZero` — per placeholder and per hierarchical part, and keeps the `lvlText` suffix on non-decimal levels (`%2)` → `a)`, refreshed `docx_lists` Test 10); docling 2.125 parity: #320 mirrors docling#3952's content-control-with-picture fixture, already handled by the `<w:sdt>` walk, docling#3729's Strict OOXML pair (`Strict.docx`/`Transitional.docx` — the `purl.oclc.org` namespaces already resolve through the namespace-agnostic walk), docling#3760's refreshed `unit_test_headers_numbered` (a heading whose numbering level has `numFmt none` — Word's invisible numbering — keeps its outline `numPr` but gets no computed `1.2` prefix; only the visible formats decimal/roman/letter/decimalZero number a heading) and docling#4036's textbox groundtruth (pictures anchored inside textboxes) and reviewer comments as first-class `comment_section` groups — each `w:comment` becomes a notes-layer group named `comment-{w:id}` holding the `[author: … , time: …]: text` note, and every body item covered by its `w:commentRangeStart`/`End` (or anchored by a bare `w:commentReference`) carries docling's `comments: [{"$ref": "#/groups/N"}]` back-ref, so the JSON groups/refs are byte-identical to upstream's; Markdown, LaTeX and DocLang are unchanged (DocLang keeps the flat `<layer value="notes"/>` item upstream writes); #248: all sections' headers/footers as furniture incl. first-page + regular pairs — and, a deliberate divergence (#590), the even-page pair when `word/settings.xml` has `<w:evenAndOddHeaders/>`: Word shows that header/footer on every even page, docling's `_add_header_footer` never asks python-docx's `Section` for `even_page_header`/`even_page_footer`, so `HEADER_EVEN`/`FOOTER_EVEN` were missing from both; emitted after the regular part of each kind (first-page → regular → even), skipped when the setting is off (an `even` reference is then a leftover Word ignores too) — `docx_even_headers_footers`, and Word's own save of it, resumed ordered-list numbering per `numId`, body text after a blank spacer stays inside the list group; #270 / docling#3961: headings detected by the style's `w:outlineLvl` — localized/custom heading styles ("Nadpis1", "Rubrik 1") promote by OOXML's own language-independent marker, outlineLvl 9 stays body text, Title/Subtitle-ish styles keep their own branch; the name check also covers the style *id* and one-hop `basedOn` id/name, so a custom style based on "Heading 2" or an English "Heading1" id under a localized display name promotes without any outline level — direct formatting with no style at all stays body text, as in docling). **JSON: structurally identical to docling's on 36/36** (every key and value but `origin`/`version`, and the `image` payload of the pictures upstream renders through LibreOffice — blip-less DrawingML shapes and the textbox shapes of `textbox`, which carry no payload here, and the EMF of `test_emf_docx`, rendered here by `metafile.rs` instead (#536: EMF/WMF records → SVG → resvg PNG, so the pixels differ from LibreOffice's); a raster picture's bytes are the embedded file's where PIL re-encodes to PNG). The flat node stream is not the shape upstream's JSON has, so `docx_tree.rs` ports `MsWordDocumentBackend`'s construction call-for-call into the docling-core `ItemTree` the JSON export serializes (the same XML readers as the Markdown walk — styles, numbering, run text, code detection, list markers, chart parts): everything after a `Title`/heading nests under it, a skipped heading level is a `header-N` section group; a paragraph is python-docx's run/hyperlink/content-control sequence grouped by `Formatting` (`_get_paragraph_elements`; bold also from the paragraph mark's run properties and the style's `basedOn` chain), two or more groups an `inline` group of `text` items each with `formatting` and `hyperlink` (`AnyUrl`-normalized, `Path` otherwise), one group a lone item; `_manage_list_structure`'s open-parents stack (`list` groups per `numId`/`ilvl`, the group cache a blank spacer paragraph keeps and the empty text item it *deletes* when the list resumes, `header-`less `list_item`s with `enumerated`/`marker`, a mixed-format or equation item an empty `list_item` over an inline group); code paragraphs merged into the previous `code` item under upstream's last-child / last-text / same-layer rule with buffered blank lines; standalone `formula` items and inline `text`/`formula` alternation; tables with python-docx's `tblGrid` column count, `gridBefore`, `vMerge` row spans, raw `cell.text`, `$…$` equations, and rich cells (several paragraphs, non-paragraph blocks, blips, formatted runs, a code style) walked in an isolated list context then re-parented under `rich_cell_group_{tables}_{col}_{row}` with the cell's `ref`; a 1×1 table walked as body; pictures per `a:blip`/`v:imagedata` (a `picture_area` group for several, `invisible` layer for a spacer), one payload-less picture per blip-less DrawingML paragraph, charts as classified pictures with `tabular_chart` data and a body-level caption; textboxes (`txbxContent`, VML, `wps:txbx`) a `textbox` section with every paragraph kept — **not** deduplicated by text as upstream's `_handle_textbox_content` does: that dedup existed for the `mc:AlternateContent` copy of a box, which `mc.rs` settles before parsing (#572 — the first `mc:Choice` whose `Requires` namespaces the backend reads, `wps`/`wpg`/`wpc`/`wp14`/`w14`/`a14`, else the `mc:Fallback`, at every level), and it dropped a paragraph legitimately repeated in a box (#576's `textbox_repeated_lines`, the reporter's three-line box, pinned as `.docx` and Word's `.doc` save — 3/3 paragraphs in both, docling keeps 2); a body-level `mc:AlternateContent`, which docling (python-docx's `body.iterchildren()`) skips with its paragraphs, is read too, as Word, its `.doc` save and anydoc read it (`alternate_content_body`, `alternate_content_textbox`; the 38 mirrored fixtures are unchanged), `shape-text` for DrawingML text outside any; headers/footers `page header`/`page footer` furniture sections; reviewer comments `comment_section` groups on the notes layer with `comments` back-refs on the paragraph's items. Markdown / DocLang / LaTeX / chunks read the flat nodes and are unchanged; #466 (docling#4319): a heading style's level clamps into OOXML's 1–9 (`Heading 111` → 9, as `Heading 0` → 1 already did); #466 (docling 2.130+; `docx_list_east_asian_num_fmt`, `docx_list_resumed_after_table` and the extended `docx_lists` mirrored — 38/38 exact, JSON structurally identical for all three): docling#4336's East Asian `w:numFmt` markers — `chineseCounting`, `chineseCountingThousand`, `chineseLegalSimplified`, `ideographDigital`, `ideographTraditional`, `ideographZodiac`, `japaneseCounting`, `decimalFullWidth`, `decimalEnclosedCircle`, rendered per ECMA-376 §17.18.59 with Word 16.112's tie-breaks (grouped Chinese numbers write 〇 for a run of zeros, 十 without a leading 一, an empty marker from 1,000,000) into the level's `lvlText` (`第%1条` → `第九条`, `（一）`, `①`), the item `enumerated` with that `marker` in the JSON; in Markdown docling-core's `orig_list_item_marker_mode=AUTO` rule is applied on our side — a marker of Unicode digits and a dot (`１.`) prints verbatim, one holding an ASCII letter or digit stays a text prefix behind the bullet, one without any (`第九条`, `甲.`) is dropped and the item is a plain `-`; docling#4188: a list starting above `w:ilvl` 0 maps its levels through `_slot_for` (subtracting the start level, clamping shallower items to the list base) so its items stay list items; docling#4305: a cached list group is reused only when nothing but blank paragraphs follows it in its parent — a table in between opens a new group instead of placing the items before it; docling#4374: footnote and endnote bodies (`word/footnotes.xml` / `word/endnotes.xml` through the document's relationships, separator placeholders skipped, a note's paragraphs space-joined) are furniture-layer `footnote` items after the comments — Markdown leaves them out like headers/footers, the JSON carries them (own `footnotes_endnotes.docx` fixture) docling#4282 (2.130.0-33): a `Title`/`Heading` resets the list context with the parents stack, so a list after a numbered heading stays in its section.; #527: a rich table cell spanning several grid positions (`w:gridSpan` / `w:vMerge` holding more than one paragraph, a list or a picture) prints its content once, at its top-left position, the covered positions empty — docling-core's `RichTableCell` is serialized by reference, so Markdown, HTML and LaTeX visit it once; blank paragraphs that a reused list group deletes from such a cell shift docling's cell provenance onto the next live items, and that regrouping (and the empty inline group it leaves on the body, a DocLang `<text></text>`) is reproduced; the JSON wraps list items outside a list group in a `group` ListGroup like docling-core's `validate_misplaced_list_items` and never writes a deleted child as an empty `$ref`, so it loads in docling-core — docling 2.133 parity (#587): `<w:bCs>` (complex-script bold) is no longer a bold signal for the JSON's `formatting` or for the rich-cell test, as upstream's `_get_format_from_run` now spells out — Word emits it as a font-theme artefact next to `w:szCs` without the user applying bold, and writes `<w:b>` even for Arabic text when they do; a cell whose only run property was `bCs` became a RichTableCell with a `<strong>` in the HTML (`docx_bcs_only_cell`, and Word's own save of it) while the Markdown walk, which never read `bCs`, printed it plain — with one deliberate divergence (#586): the re-homed item keeps its children, where docling-core re-adds it from its text alone and a mixed-format item (an empty `list_item` over an `inline` group of runs — a cell's list paragraph after a `numId 0` spacer, the reporter's `docx_list_item_runs_in_cell`) comes back as an empty bullet, its runs gone from docling's JSON after export and from its Markdown/HTML (the JSON docling saves before exporting still has them; our Markdown reads the flat cell text and was never affected); #589: runs inside an inline custom XML markup element (`w:customXml`, the wrapper Word's XML-mapping and some generators put around a span) reach the Markdown — the run walk treats it as the transparent wrapper it is, like `smartTag` / `ins` / `fldSimple` / `sdt`, as the plain-text and JSON walks already did; `Prefix text. INSIDE_CUSTOM_XML Suffix text.` used to print with the middle missing (`docx_custom_xml_inline`; a *block-level* `w:customXml` around paragraphs is still skipped, as docling's `body.iterchildren()` walk skips it); #588 (a deliberate divergence): the glyphs of the symbol-encoded Windows fonts are read as the characters they show — a run in `Symbol`, `Wingdings`, `Wingdings 2`/`3` or `Webdings` (`w:rFonts w:ascii`/`w:hAnsi`) has each glyph code, the byte (`8D`) or its Private Use Area image (`U+F08D`), mapped to Unicode (`Symbol`: Adobe's encoding as the Unicode Consortium publishes it, Greek letters included — `a b p` in Symbol *is* `α β π`; the Wingdings/Webdings families: their Unicode 7.0 encoding, `L2/11-052R`), and a `w:sym` (Insert ▸ Symbol: `w:font` + `w:char`) becomes that glyph too, while a font without a table keeps the code point as Word stores it — python-docx hands docling the PUA code points unchanged (invisible, or tofu) and drops every `w:sym`, so a Symbol bullet, a Wingdings check mark or a formula's Greek vanished from the Markdown; Pandoc's reader maps them, and the output here is Pandoc's line for line (bar Symbol 0x20, a plain space here, Pandoc's no-break one) on the reporter's synthetic file and Word's native save (`docx_symbol_font_glyphs`, `docx_symbol_font_glyphs_word`); `backend/symbol_fonts.rs` holds the tables, the Markdown, JSON and every other export read through them; a symbol font inherited from a *style* rather than set on the run is not resolved; #532 (a deliberate divergence): a table cell whose content is a text box (VML `v:textbox` or DrawingML `wps:txbx`) prints the box's text in that cell in Markdown, LaTeX and DocLang — docling takes such a cell as plain (a text box is not one of `_is_rich_table_cell`'s triggers), so python-docx's `cell.text` leaves it empty and the box lands before the table as a `textbox` section; the JSON, and the HTML rendered from it, keep docling's tree (`docx_textbox_only_cell`, `docx_two_textboxes_cell`); every paragraph of a text box is read, as docling's `.//w:p` does, so a table inside a text box reaches the output (`docx_textbox_table_cell`), and a block content control (`w:sdt`) inside a rich cell is walked like the cell itself — docling's `_walk_linear` — instead of printing empty (`docx_sdt_block_cell`); #545 (a deliberate divergence): a tracked move's destination (`w:moveTo`) reads like a tracked insertion — its text appears where Word shows it, the source (`w:moveFrom`) stays out like a deletion — in Markdown and the JSON alike, where docling (python-docx) drops it (`docx_move_tracked`, `docx_moveto_body`, `docx_moveto_cell`); a JSON table cell's `text` also keeps tracked insertions and move destinations, which python-docx's `_Cell.text` skips — a plain cell holding only inserted text had lost it from the JSON; #628: the two DOCX walks (the flat `Node` stream the Markdown/DocLang/LaTeX read, the item tree the JSON/HTML/Pandoc read) are compared in-crate on every DOCX fixture and on a generated matrix of single-feature packages — every inline/block wrapper (`sdt`, `ins`, `moveTo`, `del`, `customXml`, `smartTag`, `fldSimple`, hyperlink, footnote reference, comment range, VML/DrawingML text box, nested table, list item, heading) in every container (body, cell, nested cell, text box, text box in a cell, block `sdt` in a cell) — as the *set of words* each chain carries (order and formatting are out of scope, #532 places a cell's text box differently on purpose); the first run found that a plain cell's text behind an inline `w:sdt` / `w:customXml` / `w:smartTag` / `w:fldSimple` was in the Markdown and missing from the JSON (python-docx's `CT_P.text` reads `w:r \| w:hyperlink` only) — a deliberate divergence now, like the tracked changes above: the cell's `text` keeps it, bar a checkbox control's ☐/☒ glyph, which stays out as upstream has it since the state is the cell's `checkbox_*` child items (`backend/docx_chain_parity.rs`; the mirrored corpus's JSON is unchanged) |
| DOC (Word 97–2004) | `doc.rs` (native [MS-DOC]: CFB + piece table + PAPX/CHPX/STSH + Escher) | byte-identical Markdown to the DOCX backend on fixtures converted to `.doc` (headings, ordered/bullet lists, tables, bold/italic, and embedded pictures — inline PICF + floating shapes with decoded PNG/JPEG bytes; EMF/WMF BLIPs (inflated per their `OfficeArtMetafileHeader`, a WMF sized by its `ptSize`) rendered to PNG, #536); docling reaches these only by shelling out to LibreOffice (PR 3804); styles that are not the built-in Heading 1–9/Title still promote to headings through their `sprmPOutLvl` (#270 — a LibreOffice-written legacy file names its heading styles in the document language with a "user" sti, leaving the outline level as the only marker); #641: a numbered heading keeps its number — Word attaches the heading's list to the *style* (`sprmPIlfo`/`sprmPIlvl` in the Heading style's PAPX, the paragraphs carrying only `istd`), which the stylesheet reader now keeps and a paragraph without its own list sprms inherits through the `istdBase` chain, `ilfo` and `ilvl` independently (the DOCX backend's `style_numbering`, docling#3917); a heading whose level numbers visibly gets docling's `1` / `1.1` heading prefix (`_is_numbered_heading`, docling#3760: a bullet or `none`-format level leaves the text alone), a non-heading paragraph in a list style becomes the list item its `.docx` twin is — the reporter's file converts byte-identically to its `.docx` and to docling's LibreOffice route (`doc_numbered_heading_styles.doc`/`.docx`); #512: the container's own streams are read from the root storage first, so a `.doc` embedding another Word document (its `WordDocument`/`1Table`/`Data` under `ObjectPool/`) no longer pairs the outer FIB with the inner table stream (`doc: bad piece table`) — output matches LibreOffice → DOCX on the issue's five samples; the FIB is read through its declared `csw`/`cslw`/`cbRgFcLcb` counts with bounds checks (a truncated FIB is an error naming the field, an `fc`/`lcb` beyond the writer's array is "absent", overflowing ranges no longer panic on 32-bit targets), CFB v3 stream sizes ignore an uninitialized high 32 bits ([MS-CFB] 2.6.3), and **Word 6.0/95** (`nFib` 101–105, previously `bad FIB magic`) converts — 8-bit Windows-1252 or, with `fExtChar` (Far East editions), UTF-16; its paragraphs match LibreOffice on Apache POI's Word 6/95 samples (`poi_word95.doc`, `poi_word6_sections2.doc` fixtures) bar LibreOffice's ASCII-flattened quotes, and on POI's Far East `Bug51944.doc` 25/28, the rest off by one Far-East-encoded character each; #640: no longer text-only — the pre-97 PAPX / CHPX bin tables (2-byte page numbers, 7-byte `BX`es, the single-byte sprm set with its own operand sizes, a PAPX counted as `2·cb` bytes), stylesheet (8-bit Pascal names, the built-in Heading `sti`s) and section table (`sprmSGprfIhdt` names the header / footer stories `PlcfHdd` holds) go through the Word 97 story walker, so tables (cell / row marks + `sprmPFInTable` / `sprmPTtp`, multi-paragraph and merged-row cells), headings, bold / italic (a run's 0x80 / 0x81 operand is relative to the style and resolved through the based-on chain — Word 95 writes the same byte for a bold run in a plain style and a plain run in a bold heading style), fields, headers / footers (furniture) and footnotes come out; the issue's two files (`word6_tables`, `word95_structures`, Word 6.0 and Word 95) convert to Markdown byte-identical to the `.docx` Word saved from each, and POI's samples gain their bold runs and a footer; not read: pictures (a Word 6 PICF holds a WMF / DIB — the anchor is a placeholder picture), text boxes, autonumber lists, endnotes, merged cells' spans; other versions get an error naming `wIdent`/`nFib` — except **Word for Windows 1.x/2.0** (#566, `wIdent` 0xA59B/0xA59C/0xA5DB, `nFib` ≤ 63): a flat file, not a compound one, which used to fail as `not a compound file`; its FIB at byte 0 locates the text (`fcMin`/`ccpText`) and, through 6-byte (`fc`, `cb`) bin-table pairs at 0xA0/0xA6, the 512-byte PAPX/CHPX FKP pages, so the main story converts as paragraphs, tables (`\r\x07` cell marks, the row-end paragraph's PAPX) and bold/italic runs — byte-identical Markdown to what Word's own `.docx` re-save of the reporter's file converts to (`word2_pcjs.doc` fixture; docling fails the file); #573: the Word 2.0 stylesheet (`fcStshf`/`cbStshf`, the `ww1` layout — `cstcStd`, names, CHPX, PAPX and ESTCP blocks) is read for the style code each PAPX opens with, so the standard heading styles (`stc` 254 = heading 1 … 246 = heading 9) and a user style based on one convert to the `##`…`#` headings the file's `.docx` re-save gives (`word2_heading_styles.doc`); pictures and fast-saved (`fComplex`) files are not read, the last with an error saying so; #521: a file whose last sector is short — pre-97 writers did not pad to a whole 512-byte sector, so many real-world Word 6/95 files failed with `not a compound file` / `no WordDocument stream` — reads that sector as far as the file goes when only its unused tail is missing (the directory, the mini stream or a stream ending inside it); a sector starting past the end of file, or one the chain still needs bytes from, is a truncated file and fails as `damaged compound file` / `<stream> stream unreadable`, also for `.ppt`, `.msg`, `.wps` and the StarOffice/Quattro containers sharing the reader; #535: the stories after the main text are read too — text boxes (`PlcftxbxTxt`, linked to the anchoring shape by its `lid`) at their anchor as a `textbox` section group, or as more paragraphs of the table cell they are anchored in (where the DOCX backend's Markdown loses them), a text box no anchor places at the end of the body; each section's headers / footers (`PlcfHdd`) as `page_header` / `page_footer` furniture; footnote and endnote bodies as `footnote` furniture (JSON / DocLang only, like the DOCX backend; the Pandoc AST appends them as notes). #574: a header's or footer's text boxes (the `ccpHdrTxbx` story through `PlcftxbxHdrTxt`, anchored by `PlcSpaHdr` at a CP of the header story) are `page_header` / `page_footer` furniture ahead of that part's own paragraphs, where the DOCX backend puts its `textbox` group (`doc_header_textbox.doc`). #588: inserted symbols (`sprmCSymbol` on the `0x28` placeholder, which used to print as `(`) and characters of runs set in a symbol font (`sprmCRgFtc0`/`sprmCRgFtc2` into the font table `SttbfFfn`) read as their Unicode glyphs through the same tables as the DOCX backend (`doc_symbol_font_glyphs`, `doc_symbol_font_glyphs_word` — the same Markdown as their `.docx` twins); a font set by the style alone is not resolved. Comments are not read; #567: body text, headings, list items and the furniture stories are Markdown-escaped like every other backend's text nodes (docling-core's `serialize_run`: `_` → `\_`, then `& < >`) — `OBJ_DIR` no longer opens an emphasis span, the JSON export keeps the raw text, and table cells stay unescaped as in the DOCX backend |
| XLS (Excel 97–2004) | `xls.rs` (calamine BIFF8 + the XLSX region detection) | byte-identical to the XLSX backend on converted fixtures; #634: number formats from the Workbook stream's `FORMAT`/`XF` records and the numeric cells' `ixfe` (`NUMBER`/`RK`/`MULRK`/`FORMULA`), applied by the same rules — the issue's `.xls` matches its `.xlsx` bar the date its writer stored as the built-in `mm-dd-yy` (ISO, as the rules say) |
| PPT (PowerPoint 97–2003) | `ppt.rs` (native [MS-PPT] + OfficeArt shape walker) | **byte-identical to docling's own `.ppt` output** (LibreOffice → its PPTX backend) on the sample fixture and the #627 decks: tables reconstructed from shape-group geometry (spans included), bullet lists (StyleTextProp) and numbered lists (PP9 autonumber), titles, z-order; #623: **version 4** compound files (4096-byte sectors — PowerPoint writes one when an edited `.ppt` is saved in place) used to fail as `damaged compound file`: the shared CFB reader placed sector 0 right after the 512-byte header, where v4 pads the header to a whole sector; it now reads them, also for `.doc`, `.msg`, `.wps` and the StarOffice/Quattro containers (`.xls` goes through calamine, which already did) — `ppt_cfb_v4_edit_save.ppt` fixture; #627: docling reads a `.ppt` through LibreOffice's PPTX export, which writes every paragraph's *resolved* bullet, so the list markers here resolve the same way — a paragraph's own `StyleTextPropAtom` `fHasBullet` (only when its `hasBullet` mask bit is set; the other bullet-flag bits used to read as an explicit "no bullet"), else the bullet its master's `TextMasterStyleAtom` gives its text type and indent level (an unset level takes the one below it; center/half/quarter body start from Body, the centred title from Title — LibreOffice's `PPTStyleSheet`), the master being the one the slide's `SlideAtom.masterIdRef` names (PowerPoint 2007+ writes every layout as a master of its own). A master's numbering lives only in the PP9 extension, which LibreOffice does not turn into a list, so docling — and we — read such a body as paragraphs. Shapes are read in docling's reading order (rows by top edge, 0.05" tolerance, left to right; a group by its frame), not drawing order. On the #627 reporter's seven PowerPoint-saved decks (`crates/docling/tests/data/ppt/list_cascade/`) the Markdown is byte-identical to docling's (an untouched "Title and Content" body was paragraphs, a "Section Header" slide read its title first), and `powerpoint_sample.ppt` now matches docling's `.ppt` output too (its footnote line and rectangle text had been out of reading order); the same eleven decks as **LibreOffice** saves them (`list_cascade_lo/`, `soffice --convert-to ppt` — every paragraph's bullet written out, PP9 autonumbers for the numbered ones, a master per layout) are byte-identical to docling 2.137.0's Markdown on all 11/11 too: list items nest and number per text frame the way docling's PPTX backend does (its `_OpenList` stack, docling#4397) — a deeper indent level nests only relative to the list already open, so a level-2 numbered run after plain text is a flat list, a shallower item pops back, and a bullet joining a numbered group takes the group's numbering (`3.`, not `-`); before, the indent level was the nesting depth outright and a bullet ended the numbered run |
| WebVTT | `webvtt.rs` | **4/4 exact, JSON 4/4 structurally identical to docling 2.129** — the item tree carries each cue's `TrackSource` (`kind: track`, start/end seconds, cue identifier, `<v>` voice), the formatting inherited from `<b>`/`<i>`/`<u>` spans and an inline `WebVTT cue span` group per multi-span line; cue blocks parse like docling-core's `WebVTTCueBlock` (identifier line, cue settings ignored, malformed timings drop the block); #366: karaoke cue timestamps (`<00:00:00.389>`) are stripped without splitting the span (docling-core#744), text after a multi-line voice span stays in reading order (docling#4105), bare CR / CRLF line terminators parse (docling-core#749, docling#4157) — pinned by unit tests |
| EBCDIC (.ebc) | `ebcdic.rs` (native decode tables + copybook layouts, #252) | **3/3 exact** vs live docling 2.119 on the mirrored `ebcdic-parser`-derived corpus — incl. the four-schema packed-decimal sample and `Decimal` rendering in fixed-point at every scale (`0.0000`, `0.0000000` — docling#4296 dropped `str()`'s `0E-7`, #466; `ola013k` groundtruth refreshed); layout via `ebcdic_layout` option (all surfaces) or the `<stem>.layout.json` sidecar (docling.rs convenience; upstream requires explicit backend options) |
| Email (.eml, .msg) | `email.rs` (mail-parser) + `msg.rs` (native CFB/MAPI → RFC 822 projection, #251 — docling reaches .msg via python-oxmsg) | **4/4 exact** incl. both .msg fixtures and the opt-in `list_attachments` section (docling 2.119's `EmailBackendOptions.list_attachments`, plumbed as lib builder / CLI `--list-attachments` / serve `list_attachments` / py kwarg / Node `listAttachments`); `.eml` `Date:` now spells UTC as `+00:00` like Python's `isoformat()` (was `Z`); docling#4242/#4248 (2.128–2.129): header values are collapsed to one line of single-spaced text (`" ".join(value.split())` — subject, display names, attachment names/types), a display name holding an RFC 5322 special (`()<>[]:;@,\"`) is quoted with `\` and `"` escaped so `Name <email>` parses back, and CRLF / a lone CR in a body are normalised before the paragraph split; #466 (docling#4295): a blank or whitespace-only `text/plain` part no longer wins over the real `text/html` beside it — only a part that produced text does, and the HTML fallback now converts the part through the HTML backend and splits its Markdown into paragraphs (docling's `_convert_html_part`), instead of dumping raw markup; **attachment payloads** (#561, a docling.rs extension — docling lists names only): `EmailAttachments` / `DocumentConverter::convert_email_attachments`, py `email_attachments()`, Node `emailAttachments()` list every attachment with a safe name, media type, size, inline flag and the `InputFormat` it converts as (extension → media type → signature), a forwarded `message/rfc822` part or embedded `.msg` as an `.eml` entry; references and OLE objects have no payload; `ArchiveLimits` bound what is kept; #564: a text attachment's payload is the bytes as sent with only the transfer encoding undone (mail-parser's decoded body would re-encode a windows-1252 HTML file as UTF-8 and turn undecodable bytes into U+FFFD), `inline` marks images of the body only (a PDF disposed `inline` is an attachment), names are unique within the message (`report-2.pdf`) and shed bidi-format characters, a forwarded subject with `/` is one name, and the `.msg` fixed-property scan reads each storage at its own header offset with the record type checked (a FILETIME's value bytes used to pass for the attach-method record about once in 11 000 attachments — "no payload"); the bindings carry the detected format into `convert` (py `as_stream().format`, Node `format: att.format`), default their limits from `DOCLING_RS_ZIP_MAX_*` like the converter, and Node gets `emailAttachments*Async` |
| EPUB | `epub.rs` → HTML backend | #366: manifest hrefs are percent-decoded before the archive lookup (docling#4199, `chapter%201.xhtml` → `chapter 1.xhtml`); docling#4261 (2.129): the spine path is `posixpath.normpath(join(opf_dir, href))`, so an href stepping out of the OPF directory (`../Text/ch1.xhtml`) resolves to the archive entry it names; **1/1 exact, JSON 1/1 structurally identical** through the HTML item tree — the flat Markdown path now applies docling's heading rule (`to_single_text_element`: parts joined by spaces, the first formatting found wrapping the whole heading, `## *To the Hibernia*`) and merges adjacent same-annotation text nodes into one run (`simplify_text_elements`: `<b><time>… <abbr>p.m.</abbr></time></b>` is one bold run); #466 (docling#4351): a content document that opens with a UTF-16 byte order mark is decoded as UTF-16 instead of being dropped docling#4293 (2.130.0-33): cross-document links of every content-document extension (`.xhtml`, `.xht`, `.htm`, `.html`) become anchors; a link with a scheme or `//host` is left alone. |
| ODF (odt/ods/odp) | `odf.rs` | **7/7 exact**; **JSON: structurally identical to docling 2.129's on 7/7** (#527: `odf_presentation_02`'s JPEG is written as the PNG PIL re-encodes it to — pixel-identical — and the flat nodes carry the package's pictures too, so DocLang names and packages them) — `odf_tree.rs` ports `opendocument_backend`'s construction call for call over `odf.rs`'s style/run/list/cell readers: a paragraph of several formatting or hyperlink runs (`_normalize_odf_text_runs`: same-format neighbours merged, whitespace at a hyperlink boundary stripped, blank edge runs dropped) is an `inline` group of `text` items, a heading/title of mixed runs an empty block over such a group, a list a `list` group whose items carry `enumerated` and `{counter}{num-suffix}` markers (`.` by default), continue across sibling lists and nest under the previous item, a table's cells all recorded (empty ones too) with spans and `column_header` on row 0, a cell holding lists, several paragraphs, images or a nested table a `RichTableCell` whose content walks into a `rich_cell_group_{tables−1}_{col}_{row}` group under the table, a typed cell's text `str(cell.value)`, an embedded chart a classified picture with `tabular_chart` data, a bitmap part a picture *with* its `image` payload (the package's `Pictures/` parts are decoded for the tree; the flat nodes keep their unloaded placeholders), a presentation one `chapter` group per slide (`slide-{n}`, the slide name as `title` when no element is one), a spreadsheet one `section` group `sheet: {name}` per sheet (`invisible` layer when hidden) with every 4-neighbour data region a table carrying its cell-range provenance (`TOPLEFT`, `charspan [0, 0]`) and the sheet a page sized `right − left` × `bottom − top`. The JSON export also reproduces docling-core's `validate_document` — the pydantic validator every `DoclingDocument` passes through when docling wraps it in a `ConversionResult` — which clamps every provenance box (and a one-page table's cell boxes) into `[0, width] × [0, height]` of its page *in place*; a spreadsheet region whose page was sized `right − left` × `bottom − top` therefore loses its offset in docling's own JSON (`odf_table_with_title_01`: `(1, 3, 3, 10)` → `(1, 3, 2, 9)`), and so does ours. `full_conformance.py` measures against a document wrapped the same way on the native files (docling 2.120.3 parity — #320: `<text:a>` hyperlinks render as Markdown links with docling's target classification and boundary-whitespace rule (docling#3949); a `draw:image` whose `xlink:href` is not a package part is never read from disk and yields no picture unless it is an `http(s)` URL fetched under `fetch_images` (docling#4015) — the phantom placeholder text_document_02.odt used to emit is gone) — slide-title/name headings, shape text, speaker-notes drop, chart classification + data tables, merged-cell semantics (plain repeat vs rich dedup), and text after inline elements (docling 2.115's tail fix, #255 — the old run-tail dropping quirk is gone on both sides); sections walked (docling#3852), dangling `draw:object`s skipped (docling#3876); #466 (docling#4375, ODT): a `text:note` no longer splices its citation and body into the citing sentence — the body is a furniture-layer `footnote` item after the body walk (its descendant text concatenated, as odfpy's `text_content`), out of the Markdown like headers/footers (own `footnotes.odt` fixture) |
| JATS | `jats.rs` (roxmltree) | **6/6 Markdown exact vs docling 2.129**; **5/5 byte-identical to docling-core 2.96's export of docling 2.126's groundtruth** (#391 ports docling#4172, unreleased: a **structured abstract** keeps its `<sec>`s — each a heading one level below the abstract's (`hlevel + 2`, the abstract's being `hlevel + 1`, both with `hlevel` still 0 since the abstract precedes the body walk) with one text item per `<p>`, an untitled section's paragraphs directly under the abstract heading, a sections-only abstract with no plain text item, a section without paragraphs dropped and a nested `<sec>` not descended into — where we flattened every section into one `Title: text` paragraph; the abstract's own heading is its first `title` **or `label`** child (we read `title` only, so a label-only abstract fell back to "Abstract" for us and not for docling); the new `pmc2231364` fixture mirrored with its groundtruth. The nested-list half of docling#4172 does not apply: it wraps a sub-list in a LIST group to silence a docling-core deprecation warning, and our node model carries nesting in the item's level, not in parent refs — Markdown is unchanged there. **Known upstream quirk, replicated:** docling's `_parse_title` joins `elem.text` of each `title-group` child (the text before its first child element), so an `<article-title>` with inline markup is cut at the markup — `pmc2231364`'s title comes out as `# Global transcriptional response of`, as in upstream's groundtruth. We used to emit the full descendant text; `parse_title` now matches docling byte-for-byte (a one-line switch back to `node_text` if upstream fixes it) — no other fixture has markup in its title, which is why it never surfaced; #392 ports docling#4041 (unreleased): under `fetch_images` a `<fig>`'s `<graphic xlink:href>` — direct or inside `<alternatives>`, in document order — is read from the source **file's** directory and embedded (JSON `image`, `--images embedded`; Markdown keeps the placeholder marker), where `add_figure` hardcoded no image and the XML arm was the one image-bearing declarative backend the converter never handed `fetch_images`. docling's contract is kept: an absolute href is refused with a warning but a later relative rendition still counts, a candidate resolving outside the directory aborts the whole figure, an extensionless href is probed with `.jpg .jpeg .png .tif .tiff .gif`, `.svg` and remote hrefs are skipped, an undecodable file warns and falls through, existence is checked before decoding so probe misses do not warn per suffix, and a figure that resolves nothing warns once. An in-memory source or one fetched from a URL embeds nothing (docling's `_load_figure_image` needs a local `base_path`); docling's `source_uri` maps onto `SourceDocument.path`. docling's separate `enable_local_fetch` has no counterpart — our one `fetch_images` flag is the opt-in, as for HTML/AsciiDoc. Upstream's twelve temp-dir cases are unit tests here; (the stored `.md` predates the 2.96 header flattening and uses compact tables); #364: docling#4029 `<ext-link xlink:href>` hyperlinks on inline runs (`[text](url)`, runs coalesce only with equal formatting *and* link, blank hrefs ignored, URLs normalized like pydantic's `AnyUrl`), the new `ptag100.xml` fixture (docling#3726) mirrored, block `<tex-math>` emitted as a formula item so multi-line `$$…$$` bodies keep plain newlines, and a no-break space inside a citation kept verbatim (only ASCII whitespace collapses); **JSON 7/7 structurally identical to docling 2.129's** — `walk_linear` threads docling's `parent` through the item tree: the title item is the root (an empty one when the article has none), authors / affiliations / the abstract heading hang off it, a `<sec>` heading parents what follows — including, as in upstream where `new_parent` is set once per element, the *siblings* after the section (`ptag100`'s back-matter paragraphs under `Funding`) — a `<list>` is a `list` group whose items hold their nested lists as further groups, a paragraph of several runs an `inline` group of `text`/`formula` items with `formatting` and `hyperlink`, figure and table captions body-level `caption` items the picture / table references, a `<fn-group>` a heading over a `footnotes` list of empty items each holding an inline group with the `footnote` text, a `<ref-list>` a level-1 heading over a `list` group of citation items; an XML-looking `.txt` whose DOCTYPE names a JATS DTD converts through this backend (docling's detection reads it as `application/xml`), so `elife-56337.txt` counts too; #466 (docling#4272): a citation `<name>` with only a `surname` or only `given-names` is kept (the present parts, space-joined) rather than skipped, and an empty `<lpage/>` adds no `–` |
| USPTO | `uspto.rs` | **9/9 exact vs docling 2.129, JSON 9/9** — was 1/5; the JSON tree is the one docling's SAX handlers (`PatentUsptoIce`/`GrantV2`/`AppV1`) build: the title opens `parents[2]`, a `<heading level="N">` sits at level N+1 (or the lowest known level) and parents what follows, `ABSTRACT` and `CLAIMS` are level-2 headings under the title with `paragraph` children that leave the level alone, a `<table>` is a placeholder recorded at `</table>` under the current parent and filled by index from a regex re-parse of the raw text (all tables stay empty when the counts differ, as upstream), each spanning `<entry>` listed once per covered column with the whole span on every replica, rows padded to the widest tgroup; the legacy APS plain-text patent (`pftaps*.txt`) goes through a port of docling's `PatentUsptoGrantAps` (title, `ABSTRACT`, `PAC` caption headings, paragraphs, `CLAIMS`; Markdown, JSON item tree and `.dclx` all exact); the last three XML differences were docling's table and claim mechanics, now ported: CALS cells the unified tgroup span pushes past the widest tgroup's column count are clipped like docling-core's `TableData.grid` (`pa20010031492`'s 3 columns, `ipa20110039701`'s 4), a table's cells lose their named entities because docling re-parses each `<table>` with lxml, which knows only the XML built-ins (`ZEOCIN&thinsp;&trade;` → `ZEOCIN`, the built-ins after an undefined one in the same text run dropped too), a `<p>` wrapping `<tables>` keeps its own text as a paragraph after them (`Table 2: …` captions with their substituent lists), ICE claims are one paragraph per `<claim>` (nested `<claim-text>` pieces joined by spaces) with an in-claim table recorded before the CLAIMS heading, and PATDOC claims reproduce the SAX handler's spacing (raw `<PDAT>` concatenation, one space per `</PARA>`, `PARA`-level whitespace riding into the next `<PDAT>`); `tables_ipa20180000016.xml` fails in upstream itself. Earlier: **1/5 exact (2/5 whitespace-normalized)** on the sources live docling converts — it errors on the other 5 (those are validated byte-exact via `.dclx`), and its APS-text *Markdown* export is empty where ours emits the text dump (the `.dclx` matches exactly — §5) |
| XBRL | `xbrl.rs` + `xbrl_dts.rs` | docling's `XbrlDocumentBackend` without arelle (#466): dei facts → title; `textBlockItemType` facts (typed from the taxonomy schema, `…TextBlock`/markup by shape when it is out of reach) → HTML, each block converted with `infer_furniture=False, add_title=False` and merged the way `DoclingDocument.concatenate` numbers items (traversal order, its rich-cell ref quirk included); numeric facts → the `key_value_items[0]` `GraphData` (key cell + value/period/unit/decimals cells, `to_child` concept hierarchy from the presentation linkbase, `summation-item` arcs with `weight:` cells). The taxonomy is discovered offline from the `xbrl_taxonomy` directory (schemaRef → xsd → imports/linkbaseRefs; remote URLs through taxonomy packages' `META-INF/catalog.xml`). **2/2 Markdown exact, 2/2 JSON identical** vs upstream groundtruth (the upstream test passes the fixture's `*-taxonomy` dir). |
| JSON-docling | `docling_json.rs` (serde_json) | #390: a caption's parent survives the round trip — on its picture/table (a PDF conversion), on the item's container ahead of or behind it (a chart, an HTML figure table), or on the body; #403: a picture's `image` (docling's `ImageRef`) comes back with the document — a `data:` URI decoded to the picture's bytes, a *referenced* image (a path or `file:` URL, as `--images referenced` writes them) read from disk relative to the JSON file — so `--images embedded`/`referenced` work on docling-JSON input as they do on the original document, and a JSON→JSON round trip keeps the image; an unreadable reference degrades to the `<!-- image -->` placeholder. Reads docling's native JSON back into the `Node` model ($ref body tree walked through text items' `children` too — docling nests a section's content under its heading, #362 — formatting, list nesting, table grids). Measured by re-serializing docling's *own* documents (`tests/data/pdf/groundtruth/*.json`) and diffing against docling-core 2.96's render of the same file: **14/14 byte-exact** (#384 took it from 479 diff lines to 19, #385 to none — the last 19 were the Markdown serializer's guessed list boundaries). A caption leads its picture and its table and trails its code block (a `CodeItem` is docling's only floating text item); a marker docling-core already considers valid Markdown prints verbatim, so a reference list continuing over a page break keeps its numbering, while any other marker forces a bullet and is kept in front of the text (`- (1) Human Annotation`) and an unmarked item is numbered by its position among the group's children, nested groups included; checkbox items, unclaimed caption items and `$$`-wrapped formulas render, and code and formulas are left unescaped. |
| DocLang (`.dclg`/`.dclx`) | `doclang.rs` (roxmltree) | **15/15 exact vs live docling 2.129** — docling-core ≥ 2.93 serializes a `<field_region>` / `<field_item>` to nothing (the `<!-- missing-text -->` placeholders per container are gone, only each item's key and value remain), and a rich cell's `<text>` is serialized as Markdown *before* the cell flatten, so a `<content>` newline is a GFM hard break and lands as three spaces in the table (`Rich cell   A nested table`) — `docx_rich_cells`/`kvp_data_example` groundtruth refreshed; measured reading the same archives back (`tests/data/doclang`); the inverse of the `.dclx` output serializer, incl. docling's round-trip losses (list-item formatting, hyperlink targets). **JSON 15/15 structurally identical**: `doclang_tree.rs` ports docling-core's `DocLangDocDeserializer` call for call on the same DOM — every mixed (or empty) `<text>` an `inline` group of per-run items with `formatting`, list items as plain text / `<text>`-wrapped / *virtual text* per its `_parse_list` cases (marker `""` unless an `<ldiv>` carries one), OTSL cells with spans and per-cell `<location>` boxes, rich cells as `unspecified` groups under the table, captions and footnotes as body-level items created before their float, a `<label>` as the picture classification (`confidence: 1.0`) and a `<tabular>` as its chart data, `<src>` images read from the archive, `<location>` quartets as `prov` on the 512×512 page grid with one page per `<page_break>`; CDATA sections are re-split the way minidom sees them (blank pieces skipped, verbatim otherwise) |
| DocTags (`.doctags`/`.dt`) | `docling-core::doctags` | reads the SmolDocling/granite-docling token stream back into a `DoclingDocument` (#152) |
| LaTeX | `latex.rs` + `latex_walker.rs` | docling's `LatexDocumentBackend` ported handler for handler on a port of pylatexenc 2.11's tolerant `LatexWalker` (#466): **8/8 Markdown exact, 7/8 JSON identical** vs docling 2.130+ groundtruth — `example_01/02` plus the six arXiv projects (`1706.03762`, `2305.03393`, `2310.06825`, `2412.19437`, `2501.00089`, `arXiv-2501.01300v2`), `\input` files parsed in place. The walker port is checked node for node against pylatexenc's own dump on every fixture (`scripts/dev/pylatexenc_dump.py` … `latex_walker::tests::dump_file`), because upstream's output is the node stream's: pylatexenc's default macro specs (`\paragraph{…}` is unknown to it, so the title becomes a sibling group), chars nodes ending at every macro, the swallowed post-macro space, comments, specials (`~`, `--`, `` `` `` dropped — upstream has no handler for them), tolerant recovery from stray braces and `\end`s. Handlers: the text buffer flushed by structural macros/environments/paragraph breaks (docling#4340), `\newcommand` bodies expanded by text, citations `[key]`, `href` → `[text](url)`, theorem/proof markers, `thebibliography` as a list group, `tabular` with the `\multicolumn`/`\multirow` lookup at the node's *document* offset into the environment's own text (usually a miss, then the macro's groups are cell text). Deviations: PDF figures are payload-less pictures (upstream: pypdfium2 render at 144 dpi); a `tikzpicture` is a payload-less picture without upstream's `meta.code`; `1706.03762`'s one `tabular` carries consistent span fields where upstream's stay 1 docling#4325 (2.130.0-33): `tabular*`, `tabularx` and `longtable` are tables (longtable with docling's own `[{` spec; `\endhead` & co. no cell content). |
| MHTML (.mhtml/.mht) | `mhtml.rs` (mail-parser) → HTML backend | docling's `InputFormat.MHTML` (docling#4184, unreleased — it unwraps the archive inside its HTML backend); #386 mirrors its fixture (`tests/data/mhtml/sources/example.mhtml`, a Blink save of example.com; upstream ships no groundtruth for it — its tests assert programmatically — so its expected outputs live in our regression corpus as `example_docling.mhtml`, beside our older `example.mhtml`, a different save of the same page with the same Markdown) and its test matrix: the root is the first `multipart/related` entity, its `start` parameter names the root part by `Content-ID`, a `multipart/alternative` root yields its last HTML alternative, and a bare `text/html` message is its own root; no `Content-Type`, no `multipart/related`, an empty one, a `start` naming nothing or a non-HTML part, or an empty page **fail** the conversion as docling's do (we used to return an empty document, and to take the first `text/html` part anywhere); image resources are the selected scope's `image/*` parts only, keyed by `Content-Location` verbatim and resolved against the archive base, and by the normalized `cid:` (case-insensitive, brackets stripped), first part wins a duplicate key; the base is the root's `Content-Location` — a remote URL kept, `//host` made `https:`, a local path / `file:` URI / Windows drive path (either slash form) remapped under docling's synthetic `thismessage:/` so `<img src>` and `Content-Location` meet without touching the filesystem; archive images (and `data:` URIs) are embedded only under `fetch_images`, as docling's `HTMLBackendOptions.fetch_images` gates them — the default leaves placeholders (we used to embed unconditionally). Not ported: docling's fallback to its shared image loader (files beside the archive, remote fetches) for a reference the archive lacks. Kept beyond docling: `use_web_browser` pre-renders the page (docling rejects `render_page` for MHTML). Markdown on the mirrored fixture is identical |
| RTF (.rtf) | `rtf.rs` (hand-rolled control-word tokenizer, #209) | **native, where docling reads RTF through LibreOffice** (upstream gained that path after this backend); paragraphs + bold/italic/strike runs, stylesheet/outline headings, `\listtext` lists (incl. multilevel numbering; list identity from `\ls`, or a top-level marker-kind flip when a writer omits it — #385), `\trowd` tables with `\cellx`-grid merge recovery, textbox content, HYPERLINK fields, embedded PNG/JPEG pictures, cp1250/1251/1252 + `\uN` unicode. Conformance (`scripts/conformance/rtf_conformance.sh`): 30 of the 31 corpus files in `tests/data/rtf/sources/` are LibreOffice-generated from the DOCX/DOC fixtures and diffed against **our own conversion of the source document** — 4/30 exact, 6/30 whitespace-normalized; the rest is dominated by LibreOffice round-trip artifacts (style bold/italic materialized into runs, equations linearized to text, checkbox form fields), not parser losses. #387 adds the 31st, `legacy_sample`, mirrored from upstream with **docling's own groundtruth**, so it is diffed against that real reference instead: **exact**, tables rendered without width padding to match the stored corpus (`--compact-tables`). #578: equations — `{\mmath{\*\moMath …}}`, OMML transliterated into control words (`\msSup`, `\mr`, `{\mchr \u8721?}`, …) — are rebuilt as OMML and converted by the DOCX backend's `omml.rs`, so an RTF equation reads as the same equation saved as `.docx` (`Synthetic equation:  $E=mc^{2}$`, docling's inline spacing; a paragraph of equations alone is a `$$…$$` `formula`); the `{\mmathPict …}` picture Word writes after each one for readers without math is dropped, where it used to surface as an `<!-- image -->` ahead of the paragraph and the formula itself was lost. An `\mmath` group holding only a picture (LibreOffice's save of a formula it could not express) still yields that picture. Upstream reads RTF through LibreOffice, whose conversion keeps the equation; the mirrored LibreOffice saves `omml_frac_superscript`, `omml_multi_equation_paragraph` and `table_with_equations` now match their DOCX originals' groundtruth equations |
| SVG (.svg) | `svg.rs` + resvg rasterization (#212) | **docling.rs extension — docling does not accept SVG input**; mirrors the pdf/pdf-text split: ML builds rasterize (resvg, white-backed PNG, ~2048px long side) and ride the image pipeline (layout + OCR + tables); `pdf-text`/wasm builds, `--no-ocr` and `--text-layer-only` extract `<text>` elements directly — transform-aware (translate/scale/rotate/matrix) reading order, root `<title>`/`<desc>` as heading/lead paragraph, unrendered subtrees (`defs`, `clipPath`, `display:none`, …) skipped |
| StarOffice / OpenOffice 1.x & flat ODF (.sxw/.stw/.sxg, .sxi/.sti, .sxc/.stc, .fodt/.fods/.fodp) | `odf.rs` (shared ODF parser + local-name mapping layer, #215) | **docling.rs extension — docling reaches these only via LibreOffice**; the OO1.x predecessor schema differs from ODF mostly in namespace URIs, which this parser never matches on — the mapping layer covers the real deltas: `office:body` as the direct content container (dispatched by `office:class`), `ordered-list`/`unordered-list`, `tab-stop`, `text:level` headings, `style:properties` with `text-crossing-out`/`text-underline`. Flat ODF is the same document XML uncompressed in one file: styles ride the content DOM, embedded charts become inline `draw:object` documents, inline `binary-data` images gate on their decoded raster magic (SVM previews stay out). UOF is out of scope |
| dBase / DIF / SYLK (.dbf, .dif, .slk/.sylk) | `interchange.rs` (#216) | **docling.rs extension — docling reads none of them**; native parsers, content-sniffed inside one backend so a misnamed file still converts. DIF and SYLK are sheet snapshots and run through the same flood-fill region splitting as ODS sheets — a `.dif`/`.slk` LibreOffice saves from a sheet converts **byte-identically** to our conversion of the `.ods` itself (verified on the corpus in `tests/data/interchange/`). dBase converts as one table: field names as the header row, deleted records skipped, `D` dates as ISO, `L` logicals as true/false, memo fields (a `.dbt` sidecar) empty, cp1252 high bytes |
| Lotus 1-2-3 / Symphony / MS Works (.wk1–.wk4, .wks, .wrk, .123) | `lotus.rs` (#216) | **docling.rs extension — docling reads none of them**; native record-stream parsers following Gnumeric's lotus-123 importer, content-sniffed on the BOF so a misnamed file still converts (`.wks` is ambiguous: 1-2-3 rel 1A and MS Works v3 both used it — the BOF opcode decides). WK1/WKS cells (INTEGER/NUMBER/LABEL/FORMULA caches + STRING results), WK3/WK4/123 cells (extended floats, SMALLNUM, packed numbers, FORMULASTRING, multi-sheet), Works v3 cells incl. the packed-f32 SMALL_FLOAT. Sheets split into data regions like ODS: a `.wk1` of a sheet's data converts **byte-identically** to the `.slk` of the same sheet (pinned in `tests/data/lotus/`). Read-verified against LibreOffice's Lotus/Works import on the committed corpus (LO itself drops WK1 string-formula results; we keep the cached STRING record, following Gnumeric). Quattro Pro and the rest of the umbrella stay demand-gated |
| Quattro Pro (.wq1, .wq2, .wb1–.wb3, .qpw) | `quattro.rs` (#216) | **docling.rs extension — docling reaches Quattro Pro only via LibreOffice (libwps)**; native parse of all generations after libwps' readers: DOS 1–4/5 record streams (BOF 0x5120/0x5121; cells `[fmt][col u8][sheet][row i16]` / `[col][sheet][row][style]`, pascal-string labels in CP 437, formula caches + 0x33 string results), Windows 1–8 streams (BOF 0x1001/0x1002, `.wb3` = BOF 0x1007 in the OLE `PerfectOffice_MAIN` stream; C-string labels, CP 1252) and QPW 9–X9 (OLE `NativeContent_MAIN` zones: 0x407 string table, 0x601/0xA01 sheet/column, 0xC01 typed cell runs with list/increment packing and libwps' packed 4-byte floats, 0xC02 string results). Formulas contribute their cached value; each sheet runs through the ODS flood-fill region splitting. Corpus: the six CC0 format-corpus samples (wq1, wq2, wb1, wb2, wb3, qpw) |
| MS Works 6–9 spreadsheet (.xlr) | `xls.rs` via extension routing (#216) | **docling.rs extension**; Works 6–9 saved spreadsheets as an Excel 97 BIFF8 `Workbook` stream in an OLE container under the `.xlr` extension, so the XLS reader (calamine) takes them unchanged. Unverified against a real file — no public `.xlr` sample exists (format-corpus, Tika, LibreOffice and govdocs1 have none); a non-BIFF `.xlr` (Works 5 or older) fails with the XLS reader's error |
| AbiWord (.abw, .zabw, .awt) | `abw.rs` (roxmltree; gzip via flate2 for .zabw, #216) | **docling.rs extension — docling reaches AbiWord only via LibreOffice (libabw)**; native AWML parse: Title/heading-N styles map like DOCX, `<c>` runs carry bold/italic/strike (underline and sub/superscript survive into DocLang inline runs), `xlink:href` anchors become links, list paragraphs (listid + list_label marker, label text and tab dropped) with per-listid numbering, attach-grid tables (spans replicate the anchor docling-style), header/footer sections (incl. -even/-first) dropped as furniture, embedded base64 images extracted as pictures. Corpus: AbiWord-CLI conversions of the DOCX fixtures diffed against our own DOCX conversion — unit_test_formatting is byte-identical; the residue elsewhere is AbiWord import artifacts (Word field codes materialized as text, list numbering downgraded to bullets, merged-cell shifts), not parser losses |
| WordPerfect 5.x / 6.x+ (.wpd, .wp, .wp5, .wp6, .wpt) | `wpd.rs` (#216) | **docling.rs extension — docling reaches WordPerfect only via LibreOffice (libwpd)**; native parse of the `ÿWPC` function-code stream, version from the prefix header: WP 5.0/5.1 (hard/soft returns, hard space/hyphens, `C0` extended characters, `C3`/`C4` attribute pairs, fixed- and variable-length functions skipped by size) and WP 6.x+ (default international chars 1–32, single-byte and `0xD0` end-of-line-group codes with libwpd's semantics — soft line ends `0xCD`–`0xCF`/subgroups 1–3 wrap, deletable soft ends at hyphenation points join the word, hard ends break the paragraph, cell/row/table-off marks build the table with the next cell's column span and bound-from-above flag —, `F0` extended characters, `F1` undo regions — deleted text is dropped — `F2`/`F3` attributes; WP 5.x tables via the `0xDC`/`0xDD` table groups that begin cells and rows). Bold/italic/strike reach Markdown, underline and sub/superscript the DocLang inline runs; table cells/rows collect into a table (padded to the widest row); the WP character sets 0–14 map through Tika's tables (Multinational 1 #9, the typographic apostrophe, to U+2019). Not extracted: header/footer and footnote text (6.x prefix packets, 5.x function payloads), styles/outline numbering. Refused with a targeted error: encrypted documents, WP 4.2 and older (no prefix header), WordPerfect for Macintosh 3.x (file type 44), non-document file types. Corpus: Apache Tika's three WordPerfect fixtures (`tika_wp6.wpd`, govdocs1 `tika_wp50.wp`/`tika_wp51.wp`), LibreOffice's libwpd `WP5.wp`/`WP6.wpd` and the Open Preservation format-corpus WP 6.1 sample; a 1.1 MB WP 6.1 DOS thesis converts to 72 paragraphs with every hyphenated line rejoined (175 fragments before the libwpd code tables) — every string Tika's tests assert is present (`AND FURTHER`, `test1-2`, `Surrounded by her family`, `STUDY RESULTS: Existing condition`, `Seattle nonstop flights.`), the deleted `this was deleted.` is not |
| Microsoft Works word processor 2–9 (.wps) | `wps.rs` (#216) | **docling.rs extension — docling reaches Works only via LibreOffice (libwps)**; native parse after libwps' readers of both generations: Works 2.x DOS / 3 / 4 (`WPS4`: 256-byte header, text limits, BTEC PLC → 128-byte FDPC pages with bold/italic/strike/underline/sub-superscript blobs, CP 850 / CP 1252 text with paragraph/line/page codes, footnote definitions cut out of the body and appended; the Windows versions' OLE `MN0` stream) and Works 2000 / 6–9 (`WPS8`: OLE `CONTENTS` with the chained CHNK index, UTF-16 `TEXT` zone typed by the `STRS` PLC — main, notes, frames, header, footer —, `BTEC` → `FDPC` pages of libwps' tagged font records). Header/footer zones dropped as furniture; Works tables not rebuilt (cell text follows the body as paragraphs); spreadsheets/databases and Works for Macintosh refused with a targeted error. Corpus: LibreOffice's five libwps smoke files (Works 2.00A DOS, 3.0, 4.5, 5.0, 6.0) pin detection and the header/index/zone walk; content decoding is pinned by synthetic WPS4/WPS8 streams in the unit tests — no public corpus with real Works prose exists (govdocs1 has none) |
| StarOffice 5 binaries (.sdw, .sda/.sdd, .vor) | `staroffice5.rs` (CFB via the shared `cfb.rs`, record/chunk walk per libstaroffice's reverse engineering, #215) | **docling.rs extension — docling reaches these only via LibreOffice (libstaroffice)**; text-level extraction. StarWriter (`StarWriterDocument` stream, SW3–SW5): record tree (type byte + 24-bit size, flag-byte prologues), `'T'` text nodes as paragraphs in document order — tables flatten to cell texts, inline redline fragments kept. StarDraw/StarImpress (`StarDrawDocument3` stream): pages (`DrPg`) as sections with outliner (`xV4B`) texts, outline depth as list nesting; master pages, the object-less handout and `~LT~Notizen` notes pages dropped (docling drops speaker notes too). `.vor` templates dispatch by the contained stream. Strings decode as cp1252. StarCalc (.sdc) has a different cell-record model and errors with a targeted save-as-.ods message — a follow-up |
| First-class table cells + repair API | `docling-core` `TableCell` (text, bbox, span rectangle, header roles), `Table::cells`, `Table::{cell_at, cell_text, set_cell_text, cell_bbox, set_cell_bbox, find_cell_by_bbox, update_cell_by_bbox}`, `DoclingDocument::tables[_mut]` (#238, #240) | **docling.rs counterpart of Python docling's `TableCell`**: the PDF TableFormer paths emit real per-cell records (page-point bboxes, row/col spans from the OTSL grid, `ched`/`rhed`/`srow` header roles), the JSON export serializes them verbatim (`table_cells` with `bbox`/span offsets; the grid repeats spanning cells like docling's `TableData.grid`) — so the Python/Node bindings see them — and the DocLang structure overlay derives from them (real `<lcel/>`/`<ucel/>`/`<ched/>` tokens for PDF tables). bbox lookup is best-IoU; updating a spanning cell through any covered position updates the record and every covered grid slot. Declarative tables derive their cells from the structure overlay (DOCX/XLSX merges, HTML spans + `th` headers, ODF covered cells, USPTO CALS) — verified against the mirrored Python groundtruth: 43/56 corpus files with identical per-table (cells, spans, headers), and every xlsx/html span fixture exact; the residue is pre-existing table-count/nested-table divergences and Python backends' empty-cell omission quirks (pptx, one word_tables cell), not cell records |
| Visio (.vsdx, .vsdm) | `visio.rs` (OPC zip + XML, same `Package` machinery as DOCX/PPTX, #214) | **docling.rs extension — docling has no Visio reader**; each page a level-1 section, shape text in reading order (top-to-bottom/left-to-right, group children through the parent coordinate system, master default-text inheritance), connectors resolved via `<Connects>` into a From/To(/Label) relations table; background pages skipped. Legacy binary .vsd and 2003-XML .vdx are follow-ups |
| DjVu (.djvu, .djv) | `djvu.rs` + the `djvu-rs` crate (#434) | **docling.rs extension — docling has no DjVu reader**; pure-Rust decode (MIT, no GPL deps, no DjVuLibre binary), so it runs in every build incl. wasm. Mirrors the pdf/pdf-text split with the **text layer as the default**: the hidden per-page OCR zone tree becomes blank-line-delimited paragraphs (physical scan lines reflowed into prose), `PageBreak` between pages, `--pages A-B` honored; every page opens with a `PageInfo` marker (size in points from the INFO chunk's pixels ÷ dpi) and each paragraph carries the box of its text-layer zones (word → line → paragraph zones, whichever level aligns with the text by count), so the JSON has docling's `pages` map and per-item `prov` like a PDF — what docling-core's `pages` filter and `export_to_markdown(page_break_placeholder=…)` key on; a scan-only DjVu (no text layer on any selected page) rasterizes through `djvu-rs` (fitted to `DOCLING_RS_DJVU_RENDER_PX`, 2500 px) and rides the image ML pipeline, or degrades to an empty document with a warning without ML. Three own fixtures (3/11/84 pages) pinned in the regression suite |
| Apple iWork (.pages, .numbers, .key) | `iwork.rs` (zip + Snappy-framed protobuf IWA, generic wire walk; field/type numbers per numbers-parser / keynote-parser reverse engineering and docling's Pages reader, #213, #318) + `pages.rs`/`pages_iwa.rs`/`pages_xml.rs` (the Pages content model and its two readers, mirroring upstream's `docling/backend/iwork/` package, #383) | **Pages: 4/4 exact** Markdown on upstream's committed `tests/data/pages/` groundtruth (docling#4062, mirrored with its `.json`; #383) — the full `IWorkPagesDocumentBackend` feature set in both container generations: tables **placed in the text flow** at their U+FFFC anchor (not appended), floating **text boxes** and groups after the body, **character formatting** (bold/italic/underline/strikethrough, super/subscript) with the spaces around a run kept and the paragraph trimmed once at its ends, **hyperlinks**, **lists** (bullet/numbered from `TSWP.ListStyle` label types and the list-depth run table; iWork '09 `sf:liststyle`), **inline images** (`TSD.ImageArchive` → `Data/` member, `sf:media`), **headers/footers/footnotes** on the furniture layer, **reviewer comments** on the notes layer with `[author]: text` and `comments` back-refs on the annotated item (docling-core's bare `add_comment` shape — no `comment_section` group), Pages 5.2+ table cell storage (v4/v5 cell layouts, rich-text cells through `TST_TEXT_REF`). Run tables follow upstream exactly: a paragraph-style entry without a reference clears the style, so a template paragraph whose style Pages left unreferenced is body text, and an anonymous (unnamed) paragraph style is body text. iWork '09 `index.xml(.gz)`: `sf:p` runs walked node by node (`sf:span` character styles, `sf:link`), `sf:ghost-text(-ref)` placeholders skipped (#366, docling#4170), `sf:tabular-model` grids, `sf:header/footer/footnotes` furniture, `sf:annotation` comments anchored via `sf:annotation-field`. JSON: body order, body texts, tables, notes-layer comments and their back-refs identical to upstream's groundtruth; **two shared-exporter caveats** — furniture-layer items (page header/footer, footnote) stay out of the JSON as for the other flat-node backends (only the PDF pipeline's page headers/footers reach it), and text items carry no `formatting`/`hyperlink` fields (the formatting reaches Markdown and DocLang). A password-protected package fails with docling's message (Pages hides encryption behind an undefined compression method). Mixed-run paragraphs render as docling-core does: each run wrapped as-is and the runs joined with one space (so `Plain ` + `bold` reads `Plain  **bold**`). **Keynote: 5/5 exact Markdown and structurally identical JSON** on upstream's `tests/data/keynote/` corpus (docling#4330 + charts docling#4376, 2.130+; #466) through `keynote.rs`, `iwork_charts.rs` (the shared `TSCH.ChartDrawableArchive` reader: kind → classification label, the grid by row or column, the title when shown) (the presentation model and its emission), `keynote_iwa.rs` (Keynote 6+ `Index/*.iwa`, also Keynote 2018+'s index zipped a second time into `<dir>/Index.zip` with the `Data/` members left beside it) and `keynote_xml.rs` (iWork '09 `index.apxl`, optionally gzipped), mirroring upstream's `IWorkKeynoteDocumentBackend`: a `chapter` group `slide-{i}` per slide and a page of the slide's own size (1024×768 when unreadable), the slide's drawables in **reading order** — down, then across, top edges within 3.6 pt banding a row, adjacency measured against the previous drawable, unpositioned ones last — each item with the `TOPLEFT` box of the drawable holding it and a `charspan` over its text, the **title placeholder's** paragraphs `title` and every other paragraph `text` (theme style names are localised, so the placeholder decides), lists from the list-style run table **including the ladder a style inherits from its parent** (Keynote leaves the bullets on the theme's style; iWork '09 spells it `sf:parent-ident` through the paragraph style), tables and pictures placed like any drawable, **presenter notes** as notes-layer `text` items with a zero box and **comments** (Keynote's sticky notes, `[author]: text` for 2013+, plain text for '09, which records no author) as notes-layer texts without provenance, the slide-number placeholder and everything a master slide draws left out; an iWork '09 table keeps every datasource cell's grid position (a number cell is an empty cell, not a shift along the column). Charts (`TSCH.ChartDrawableArchive`, docling#4376) are not read yet. **Numbers remains a docling.rs extension** (upstream has no reader): sheets/tables as headings with shared-string cell text as a list; nested-dir and zipped-package layouts unwrap (upstream rejects them); numeric cells and full grid reconstruction are follow-ups |
| HEIC/HEIF (.heic, .heif) | libheif via `docling-pdf/heif` (opt-in cargo feature, #211) | **docling.rs extension** (docling reads HEIC only where Pillow can); content-sniffed (`ftyp` brands — misnamed `.jpg` iPhone photos still route correctly), decoded to RGB and fed to the standard image ML pipeline; without the feature the error says `rebuild with --features heif` instead of a generic decode failure. Native dependency, so wasm/default builds stay pure Rust |

Shared OOXML infrastructure (`ooxml.rs`): a `zip` reader, `.rels` parsing, part
content-type resolution, and image extraction — reused by DOCX/PPTX/XLSX/EPUB.

### ML formats — `docling-pdf`

These run docling's *discriminative* PDF pipeline ported to ONNX. They are now
measured **byte-for-byte against live docling** (the committed PDF groundtruth is
regenerated from it): **6 / 14 exact (7 / 14 whitespace-normalized)**, the rest
close — see `PDF_CONFORMANCE.md`. A deterministic snapshot baseline
(`scripts/conformance/pdf_conformance.sh`) still guards against regressions.

| Format | How |
|---|---|
| PDF | **pure-Rust text parser** (`textparse.rs`, font-advance glyph boxes) + page render — the **pure-Rust renderer** by default (the docling-parse renderer plugin — the canvas docling 2.123+ feeds its models, the renders the baselines are pinned to, #478, `PDF_CONFORMANCE.md` — is a development oracle selected with `DOCLING_RS_RENDERER=docling-parse`) (`render/`: content streams, clips, transparency as docling-parse flattens it, TrueType/CFF/Type 1/Type 3 glyphs and host fallback faces — `.models/fonts`, `DOCLING_RS_FONT_DIRS`, then the host's; #633: `DOCLING_RS_SYSTEM_FONTS=0` stops at the first two, so a deployment that ships its fonts renders the same layout input on every host (the reporter's paper scored its title 0.77 vs 0.83 and found a footnote on Linux only, Liberation Sans vs Arial; pinning the directories made Windows byte-identical to Linux; the published Docker images set `=0` with the two packages they install pinned in `DOCLING_RS_FONT_DIRS`), and `DOCLING_RS_DEBUG=1` names the face each style resolved to — shadings, patterns, images, widgets; mean \|Δ\| 0.84/255 per channel against the shim over the snapshot corpus; on the shim-rendered baselines 60/98 snapshots exact and 429 groundtruth diff lines where the shim itself is 98 / 374; Type1C fonts carrying the deprecated CFF `dotsection` operator draw their periods and i-dots, #531), pdfium's chain only in a build with the opt-in `pdfium` feature under `DOCLING_RS_RENDERER=pdfium` → RT-DETR layout (ONNX) → **TableFormer** table structure (ONNX) → PP-OCRv3 OCR for scanned pages → **docling-parse line sanitizer** (`dp_lines.rs`) + reading-order assembly (#419: layout boxes are refitted to the cells they claim, empty ones dropped and contained orphans folded in before the reading order runs, as docling's `LayoutPostprocessor` does — a model box that cut a line in half no longer strands that line after its paragraph; 2206 82→52, 2305 20→18, normal_4pages 20→16 diff lines vs groundtruth). `--pages A-B` (docling's `page_range`, #80) converts a 1-based page window, skipping the rest before rasterization; `--document-timeout SECONDS` (docling's `document_timeout`, #497) is the per-document budget checked between pages — once spent the pages done so far are the document and the result is `PARTIAL_SUCCESS` with docling's `ErrorItem`, on every surface; `--images referenced` streams each page's image files to the artifacts dir as the page is emitted (memory-bounded, #80); `--ocr-lang en|ch` picks the OCR recognition model (en default — the ch_ conformance model glues Latin words); scanned pages with `/Rotate` are un-rotated to upright before layout/OCR and their geometry mapped back to display coords (all four orientations of `tests/data/scanned/` OCR to the same groundtruth text); table captions attach by reading-order adjacency (docling's `_find_to_captions`, #265) and ride on the table across all exports — Markdown above the grid, JSON `TableItem.captions` refs, DocLang `<caption>`; the JSON carries page headers/footers as body-parented `page_header`/`page_footer` texts on the furniture layer, as upstream writes them (Markdown still leaves them out); the text inside a picture reaches the JSON as that picture's children (docling's `_add_child_elements`), as upstream writes it (Markdown, like docling's, prints only the caption); text outside the page's display box — the CropBox, else the MediaBox — is dropped glyph by glyph like docling-parse does (#529: a FrameMaker print slug, a tiled page's neighbouring text) instead of being clamped onto the page edge; #598: a `JPXDecode` (JPEG 2000) image is decoded (`render/jpx.rs`, `hayro-jpeg2000`, within ±1 of OpenJPEG) instead of drawn as a mid-gray placeholder — on photo pages the blank rectangle left the layout model unsure of the picture (0.64 instead of 0.99): its box ran over the caption below, which nested inside as `text` and vanished from the Markdown, and two stacked photos split by a caption fused into one; decoded, the reporter's NASA pages give docling's result (caption linked and printed, two pictures) at int8 and fp32 alike — `crates/docling/tests/jpx_pictures.rs`; JPX scans (image-only pages) now reach OCR as pixels too; the floor moved to rustc 1.92 for the decoder |
| Images (tiff/webp/png/jpeg/gif/bmp) | the same pipeline, image as a single page; docling#4247 (2.128): the EXIF orientation tag is applied when the frame is loaded (`ImageOps.exif_transpose`), so a portrait photo no longer reaches layout and OCR on its side; #570: OCR reads an image at docling's effective resolution (3 px/pt within RapidOCR's 2000 px longer side) and recognizes the text detector's boxes — on FUNSD's 199 scanned forms word recall against the annotations (the issue's `funsd_repro.py` scoring, `text_items`) is **0.896**, median 0.933, 29 forms under 0.8 (docling-rs 1.96: 0.613 / 150 under 0.8; Python docling 2.133 with RapidOCR: 0.852 / 39 under 0.8), 0.914 on the nine forms #571 lists against Python's 0.915; #571: the orientation probe runs on the detector's boxes with a confidence margin, so no upright form is turned any more (9 of 199 were) |
| METS / Google Books | `.tar.gz` of per-page hOCR + TIFF → cells from hOCR → the same layout+assembly path (no OCR needed) |
| Audio (wav/mp3/flac/ogg/aac/m4a) and video audio tracks (mp4/mov/mkv/webm — docling's `InputFormat.VIDEO`, Phase 1 of #138) | `docling-asr`: **symphonia** decode (no ffmpeg) → 16 kHz mono → ported log-mel front-end → **Whisper tiny** encoder/decoder (ONNX, greedy with OpenAI's timestamp rules — docling's ASR defaults) → `[time: start-end] text` paragraphs in Markdown; the JSON writes each segment as docling 2.135's ASR pipeline does (#614) — a text item of the words with the timing as `source: [{"kind": "track", "start_time", "end_time"}]` (it used to be the `[time: …]` string with no timing field), blank segments without a track and a zero-length one stretched by docling's 1 ms, which `--to vtt` writes as subtitles. Frames (Phase 2 of #138): when the `ffmpeg` binary is present at runtime, up to `--video-frames N` (default 8) scene-change frames (evenly spaced fallback) interleave with the transcript as `[time: <ts>]`-captioned pictures with embedded PNGs; no ffmpeg → transcript only, no audio track → frames only. Codecs symphonia can't decode in-process — Ogg Opus, AVI containers — go through the same optional ffmpeg binary when present (#190); without it they fail with a targeted install hint. Transcription language: auto-detected per file from the first 30-second window (Whisper's `language=None` / docling 2.116 default, #180); pin with `asr_lang` (all surfaces) or `DOCLING_RS_ASR_LANG`; English-only presets skip detection. **Parakeet TDT 0.6B v3** (#508, `asr_model=parakeet_tdt_0.6b_v3`, beyond docling's specs): NVIDIA's multilingual transducer for 25 European languages (language detected by the model), a port of onnx-asr 0.12 — NeMo 128-mel preprocessor in Rust (≤ 2·10⁻⁵ from onnx-asr's NumPy one), TDT greedy decoding (same tokens/frames as onnx-asr on the same encoder output; with the fp32 graphs the whole pipeline is token- and timestamp-identical on the en/de/ru fixtures), Silero VAD spans (≤ 20 s; energy-based pause splitting without it), sentence segments. Models: `download_dependencies.sh --asr-model=<preset>`, Python `download_models(asr_model=…)`, Docker `--build-arg ASR_MODELS=…`. |

### DocLang (`.dclx`) coverage

The `.dclx` DocLang output (§3) is scored against docling's own `.dclx` archives
with `scripts/conformance/dclx_conformance.sh` — the extracted `document.xml`
line-diffed, similarity `= 100·(1 − difflines / max_lines)`. **≈97% mean over the
136-fixture non-PDF corpus** (issue #32 target: ≥90%), per source format.
HTML rich table cells (#328) carry their block content — lists, ordered lists,
nested tables, headings, `<pre>` runs — as `Table::cell_blocks`, so the
DocLang `<fcel/>` bodies match upstream's `RichTableCell` serialization
(`table_03`–`table_06`, `table_with_heading_02`,
`html_inline_group_in_table_cell`, `html_rich_table_cells` and
`hyperlink_05` byte-exact — a picture caption also carries its hyperlink
annotation: an `<a href>` wrapping the image, or the first link inside a
`<figcaption>`, emits the block-form `<caption>` with an `<href uri=…/>`
head and docling's `hyperlink` field on the JSON caption item).
Robustness tracks docling-core 2.88/2.89 (#253): XML-illegal control
characters serialize as visible `[U+XXXX]` markers, a literal `]]>` splits
across CDATA sections, and deep section headers clamp to heading level 6
instead of emitting out-of-range tokens — all round-trip pinned; the reader
side of docling-core#689/#695 (tag-shaped literals in OTSL cells) never
applied here, since the single-pass roxmltree reader doesn't re-parse text
fragments as XML.

| Format | `.dclx` similarity | Format | `.dclx` similarity |
|---|---|---|---|
| CSV / AsciiDoc / Email | **100%** | JATS | 95% |
| XLSX | **100%** | Markdown | 92% |
| DOCX / PPTX | **100%** | LaTeX | 91% |
| USPTO | 98% | HTML | 97% |
| ODF | 96% | WebVTT | 81% |

This effort was tracked as
[issue #32](https://github.com/docling-project/docling.rs/issues/32) — **closed,
both targets met** (non-PDF ≥90%: 94%; PDF ≥50%: 63% at ±2). Its children
(#38–#41, #44, all closed) landed the ODF, USPTO legacy-entity, elife XML,
wiki_duck and APS-plain-text work — `pftaps` is byte-exact (§5). The PDF path
emits full layout `<location>` provenance (text, headings, tables, pictures,
list items, code, and page-header/footer furniture), scored against a
16-fixture DocLang groundtruth with a ±2-grid-unit geometry tolerance —
**63% mean** (§3, `PDF_CONFORMANCE.md`); the residual is model-level
(TableFormer OTSL structure, layout classification — the closed-as-model-level
blockers of `PDF_CONFORMANCE.md`), not serialization.

The same geometry also reaches the **JSON export** (#171): PDF conversions
populate docling's `pages` map (`{"1": {"size": {...}, "page_no": 1}}`) and
per-item `prov` (`page_no` + BOTTOMLEFT-origin bbox in points + `charspan`),
so `DoclingDocument.load_from_json(...)` in Python docling-core gets working
bounding-box highlighting, page attribution and coordinate filtering — from
the CLI, serve, and the Python/Node bindings alike. One caveat vs Python
docling: coordinates round-trip through the DocLang 0–511 grid, so they carry
a quantization of up to ~page-size/512 (≈1.6 pt on A4); `charspan` always
starts at 0 (docling.rs does not track sub-item spans).

Where a picture's or table's **caption item hangs** in the JSON tree follows
the backend, as it does upstream (#390, `docling_core::CaptionParent`): the
PDF pipeline parents a layout caption to the picture or table itself (the
caption is the item's first child, as docling's reading-order model attaches
it); every declarative backend — HTML, JATS, LaTeX, Markdown, EPUB, AsciiDoc,
DeepSeek — leaves docling's `add_text` default, so the caption is a `#/body`
child even when the item sits in a group or under a heading (upstream's
groundtruth: 103 body-parented captions against 36 on pictures and 18 on
tables, all of the latter PDF); the office backends hang a chart's title
caption off the chart's container — the sheet group, the slide `chapter`
group, docx's current parent (docling#4190) — listed ahead of the picture
that references it; and an HTML `<figure>`-wrapped table's figcaption sits
under the table's parent *behind* the table (docling#4050). Until #390 every
caption hung off its own picture or table. A docling-JSON input keeps the
parent it came with. Markdown, DocLang and LaTeX are unaffected — they place
a caption by its item, whatever its parent.

---

### Chunking conformance

docling-core's **HierarchicalChunker** and **HybridChunker** (the RAG chunk
generators) are ported as `docling::chunker` and scored against live docling
running the same chunkers on the same 83-document corpus
(`scripts/conformance/gen_chunks.py` generates the groundtruth,
`scripts/conformance/chunks_conformance.sh` compares the records' text +
headings + contextualization — the payload an embedding model sees):

| Chunker | Identical chunk records | Fully-exact documents |
|---|---|---|
| Hierarchical | **555 / 562 (98.8%)** | 79 / 83 |
| Hybrid (MiniLM tokenizer, 256 tokens) | **300 / 312 (96.2%)** | 76 / 83 |

The port reproduces docling's semantics end-to-end: heading-path metadata with
level shadowing, triplet table serialization over `export_to_dataframe`
semantics (multi-row headers joined with `.`, span-aware header detection,
single-column/flatten fallbacks), rich-cell re-serialization, `semchunk`'s
recursive splitter-hierarchy algorithm, the line-based table splitter (down to
the `\n` it prepends to carried-over segments and the `max_tokens` argument
docling's pydantic model silently drops), and peer merging. Token counts are
byte-compatible with `transformers` (HF `tokenizers` with MiniLM's fixed-length
padding disabled).

On the large-document benchmark (`wiki_duck.html`, 89 hierarchical / 115 hybrid
groundtruth chunks) **100% / 100% of docling's chunk records are reproduced
identically** (order-aligned) — the former HTML-backend model gaps (rich table
cells with inline markup and span de-duplication, figure-caption run spacing,
indicator images, `<br>` annotation-boundary handling) are closed. Corpus-wide:
hierarchical 98.8%, hybrid 96.2% record-identical. The chunker-era
work (checkbox inputs, fragmented-anchor folding, `<button>` blocks) plus the
#81 parity fixes also lifted the HTML `.dclx` similarity: 88% mean (was 84%).

**Item refs (`DocMeta.doc_items`).** The scripts above compare text, headings
and contextualization; the items a chunk lists — what LangChain's `dl_meta`
carries, with each item's page/bbox provenance — are checked separately on
the Python side. The engine chunks a re-imported copy of the document whose
item numbering drifts from the caller's (empty items dropped, HTML/DOCX item
trees, inline runs sharing their paragraph's number), so `docling_rs.chunking`
maps every chunk item back to the caller's document by kind, reading order
and text (an item that cannot be placed is left out, never mis-pointed).
Measured with `docling_rs.langchain.DoclingLoader` against docling-core
2.99's Python `HierarchicalChunker` on the groundtruth JSON of all formats:
of the 101 documents whose chunk texts match, 95 also match `dl_meta`
exactly (80 before the mapping); on docling.rs's own conversions (HTML, DOCX,
Markdown, PPTX, XLSX, AsciiDoc, LaTeX, PDF) every listed text item resolves to
text contained in its chunk (~2,600 items checked, 0 mismatches; before the
mapping 86% of PDF, 79% of HTML, 37% of DOCX and 15% of Markdown items
pointed at the wrong item).

## 3. Output formats

| Output | API / CLI | Notes |
|---|---|---|
| **Markdown (legacy)** | `export_to_markdown()` / default | byte-for-byte docling, quirks included. A blank line separates two *sibling* lists only where the backend flagged the second (`first_in_list`) — never inferred from a kind flip or a number gap (#385). Tracks docling-core 2.97's serializer: a referenced image's link destination is percent-encoded like upstream's `_escape_uri_path` (docling-core#698 — spaces and parentheses in the artifacts dir, `\\` → `/`, absolute Windows and UNC paths as `file://` URLs, URLs component-encoded, never double-encoded; the artifact *file path* handed back stays raw); a table's Markdown header is the leading block of rows on which a `column_header` cell *starts*, stacked header rows flattened per column with ` - ` (`% of Total - Train`), flags that begin below row 0 promoting nothing and unflagged tables keeping row 0 (docling-core#723/#756 — alignment and widths come from the body rows); a row also ends the header block when it carries body text — a non-header cell with text outside the first column — so a form whose first-column labels are flagged `column_header` on every row keeps its values in the body, and a row 0 stopped that way is still the header when no later row is flagged (docling-core#766, 2.97; #604). The pivot-table deviation this port carried is gone: docling#4216 flags a spanning `<th>` row as `row_header` rather than `column_header`, so upstream no longer folds a pivot table's first data row into the header (`Year - 2025 | Month - January`) and `header_row_count` is now docling-core's rule verbatim (docling-core#765 resolved on the backend side); a single newline inside an item's text is a GFM hard line break (`"  \n"`, docling-core#721 — a blank line stays a paragraph break, a heading collapses its newline to a space), a heading inside a rich table cell renders as plain text (docling-core#540), and field regions / field items emit nothing of their own — no more `<!-- missing-text -->` markers (docling-core#724). The hard-break marker is Markdown-only: JSON cell text and LaTeX cells see the raw newlines |
| **Markdown (strict)** | `.strict(true)` / `--strict` | Rust-only cleaner mode — **no docling equivalent** |
| **Markdown (options)** | `export_to_markdown_with_options(&MarkdownExportOptions)` · `MarkdownStreamer::with_export_options` (#599) | docling-core's `MarkdownParams` beyond the image mode: `layers` (`included_content_layers` — page headers/footers, comments and hidden sheets render through the body's serializers when their layer is in the set; a hidden sheet's items take the sheet's layer, as upstream's do), `traverse_pictures` (`CommonParams.traverse_pictures` — a picture's nested text items print after its caption and image, the way `iterate_items(traverse_pictures=True)` yields them; the reporter's bordered FUNSD form, one picture with 36 text children, is empty without it), `escape_html` / `escape_underscores` (the backends escape at build time, so turning one off *undoes* that escaping — `&amp;`/`&lt;`/`&gt;` and `\_` — on text items, headings, captions, list items and form fields, never on code, formulas or table cells, like upstream's `post_process`), `image_placeholder` (an empty one prints nothing: like docling-core's `"\n\n".join(p.text for p in parts if p.text)`, an item that renders empty adds no blank lines, while it still counts for the page-break placeholder, #605). Defaults are upstream's: the default export is byte-identical. Library API only, as requested (Python's CLI has no such flags) — the other surfaces keep the default export |
| **JSON** | `export_to_json()` / `--to json` | docling-core native wire format (schema 1.10.0) |
| **Plain text (`.txt`)** | `export_to_text()` · `MarkdownExportOptions::plain_text()` / `--to text` (#613) | docling's `--to text` — docling-core's `PlainTextDocSerializer` (`export_to_text()`): the Markdown walk without `#`, emphasis / strikethrough markers, link URLs, code fences and backticks, image placeholders, GFM hard breaks or HTML/underscore escaping; bullets, numbers, checkbox marks and table grids stay. No trailing newline, like upstream's `<stem>.txt`. **134/135** declarative fixtures (those whose Markdown matches) identical to `export_to_text()` on the upstream groundtruth JSON; 15 pinned in `tests/plain_text.rs`. Our nodes carry inline Markdown text where upstream carries formatting flags, so the markers are taken back out by a parser; a marker counts only with content tight against it (CommonMark flanking), so literal text such as IBM i's `*USE … *OBJMGT` or `2 * 3 * 4` survives — the deliberate cost is docling's own emphasis around bare whitespace (WebVTT's `* *`), which stays literal. |
| **DocLang (`.dclx`)** | `export_to_doclang()` · `docling::dclx::save_as_dclx()` / `--to dclx` | DocLang 0.7 XML (`<doclang>`), and the OPC archive docling 2.110's `save_as_doclang()` writes — with its `assets/image_NNNNNN_<sha256>.png` parts: same names (index over every body picture, digest over the decoded pixels for PNG and JPEG), pixel-identical content, for all 25 picture references of the upstream `groundtruth_dclx` corpus but one furniture data URI whose PNG compression bytes differ |
| **WebVTT (`.vtt`)** | `export_to_vtt()` · `export_to_vtt_with_options(&VttExportOptions)` / `--to vtt` (#614) | docling's `--to vtt` — docling-core's `WebVTTDocSerializer` as `save_as_vtt` runs it (`omit_voice_end`): a cue per text item with a track `source` (WebVTT cues, ASR segments), identical timings merged into one cue, `<b>`/`<i>`/`<u>` and `<v voice>` spans, upstream's two tag-merging rules ported (its regexes use backreferences); a document without timed text is the `WEBVTT [title]` header, as upstream writes it. **4/4 mirrored WebVTT inputs byte-identical** to docling 2.135 (both `save_as_vtt` and `export_to_vtt` defaults). Deliberate divergence: cue text is escaped (`&amp;`, `&lt;`) where upstream re-parses it and fails on a raw `&` or `<`. |
| **LaTeX (`.tex`)** | `export_to_latex()` / `--to latex` (#317) | docling 2.124's `LaTeXDocSerializer` with default params, scored against upstream's own `docling --to latex`: **93/116 shared declarative fixtures byte-exact** (98 modulo upstream's duplicated formatted list items / headings — #328's HTML `cell_blocks` made the rich-cell bucket exact: in-cell lists render as `\begin{itemize}`, nested tables as nested `tabular`s, in-cell headings as plain text); remaining gaps are underline / sub / superscript (no Markdown form) and a few list groupings. Serve `to=latex`, Node `to: 'latex'`; Python bindings use upstream docling-core's serializer on the reconstructed document. Deviations: headings deeper than `\subsubsection` degrade to `\paragraph`/`\subparagraph` where upstream raises; upstream's duplicated text for formatted items ([docling-core#740](https://github.com/docling-project/docling-core/issues/740)) is not reproduced; docling-core#743 (2.95) fixed the inline-group double serialization (docling-core#740) that this port never reproduced, so the two agree again |
| **HTML (`.html`)** | `export_to_html()` · `export_to_html_with_images(mode, dir)` · `export_to_html_with_layers(ContentLayers)` · `export_to_html_with(&HtmlExportOptions)` / `--to html` (#492, #499) | docling-core's `HTMLDocSerializer` with default params (single-column style, body layer), rendered from the JSON structure the JSON export already reproduces; #499: `HTMLParams.layers` / `export_to_html(included_content_layers=…)` — `ContentLayers` picks the body/furniture/notes/invisible layers rendered (default body only, byte-identical to before), extra items going through the body's serializers like upstream's, pinned against docling-core 2.99's output for DOCX header/footer, comment and all-layer exports (`html_layers_match_docling_core`, 3/3 byte-identical); library API only, the other surfaces still export the body layer; pictures per the image mode like Markdown (`placeholder` = upstream's default, pictures out but for caption and meta). Pinned byte-for-byte against upstream's HTML groundtruth for the ODF/DOCX fixtures (8/8, picture payloads masked — upstream's `save_as_html` re-encodes them through PIL) and against docling-core 2.99's own `export_to_html()` over our exported JSON for the whole declarative corpus: **265/265 byte-identical** — formulas as MathML through `docling_core::mathml`, a port of the `latex2mathml` 3.81 library upstream runs (checked against the Python package on every corpus formula and a synthetic suite, inline and block, error cases included; `<annotation encoding="TeX">` + `<div>` block wrapper as docling-core adds them, `<pre>{latex}</pre>` where the library raises). Served as `text/html` (`to=html`), Node `to: 'html'`, wasm `"html"`; Python needs nothing (`result.document` is upstream's) |
| **Pandoc AST (`.pandoc.json`)** | `export_to_pandoc_json()` · `export_to_pandoc_json_with(&PandocExportOptions)` · `docling_core::pandoc::from_docling_json` / `--to pandoc [--pandoc-api-version 1.23]` (#515) | Rust-only — **no docling equivalent**. The JSON serialization of Pandoc's `Pandoc` type (`pandoc-api-version` 1.23.1.1, Pandoc 3.x) for `pandoc -f json -t docx\|odt\|epub\|rst\|typst…`, walked from the same docling-JSON structure as the HTML/LaTeX exports: headings, nested/ordered lists, inline formatting and links, code, inline/display math, checkboxes, tables (`TableHead`, row/col spans, rich cells, captions), figures (`Para [Image]` / `Figure`; pictures embedded unless the image mode says otherwise, and an `Image` even without a target — classed `docling-placeholder` — so `pandoc -t docx` keeps every picture, #537; chart data as a table), table/picture footnotes as `Note` in the caption and DOCX footnotes / endnotes and ODT notes as `Note` at their reference (#538; a footnote without a recorded call site, e.g. from Python docling's JSON, as a trailing `Note`), key-value graphs and form field regions as `DefinitionList`s, other labels as `Div .docling-<label>`. Not representable, so dropped: provenance (pages, bboxes), confidence and classification meta, form geometry. Asking for another API version is an error (`unsupported Pandoc API version`), never a document Pandoc would reject. Validated: every convertible declarative fixture (305) and the PDF corpus read by `pandoc -f json -t native`; 16 documents pinned as JSON + Pandoc's `native` reading, and a DOCX rebuilt by `pandoc -t docx` checked for its pictures and `word/footnotes.xml` (`crates/docling/tests/pandoc.rs`). Serve `to=pandoc` (+ `pandoc_api_version`, `application/json`), Node `to: 'pandoc'`, FFI `to=pandoc`, wasm `"pandoc"` (and the browser demo's selector), Python `docling_rs.pandoc.export_to_pandoc(doc)` / `save_as_pandoc(doc, path)` |
| **Page breaks in Markdown** | `DocumentConverter::page_break_placeholder(Some(text))` / `--page-break-placeholder TEXT` / serve `md_page_break_placeholder` / Node `pageBreakPlaceholder` | docling-core's `MarkdownParams.page_break_placeholder` (`export_to_markdown(page_break_placeholder=…)`, docling-serve's `md_page_break_placeholder`). Upstream yields a `_PageBreakNode` between two body items whose `prov.page_no` differ (a group counts by its first item) and substitutes the placeholder as a part of its own, joined by the `\n\n` delimiter; docling.rs marks a break between two rendered blocks separated by a page boundary (`PageInfo` per PDF page / sheet, `PageBreak` between slides, DjVu and DocTags pages) — the same result: never first or last, empty or furniture-only pages collapse into one break, an empty placeholder leaves the doubled blank line upstream leaves. `None` (default) = no breaks, docling's default. Buffered and streamed (`MarkdownStreamer::with_page_break_placeholder`) output byte-identical |
| **Image extraction** | `export_to_markdown_with_images(mode, dir)` / `--images` | `placeholder` (default) · `embedded` (base64 data URI) · `referenced` (writes PNG files) |

- **DocLang** reproduces docling-core's `DocLangDocSerializer` (`minidom.toprettyxml`
  layout) directly: headings, rich inline runs (`<bold>`/`<italic>`/`<underline>`/
  `<strikethrough>`/`<sub|superscript>`), lists with enumeration `<marker>`s, OTSL
  tables (`<ched>`/`<fcel>`/`<lcel>`…) with per-cell `<location>`, code, formulas,
  pictures and furniture. Conformance is scored against docling's own `.dclx`
  archives (`scripts/conformance/dclx_conformance.sh`): **≈97% mean similarity over
  the 136-fixture non-PDF corpus** (issue #32's ≥90% target) — every OOXML fixture
  (docx/pptx/xlsx) plus csv/asciidoc/email byte-exact, uspto/jats in the
  mid-to-high 90s, html (#328 rich cells + caption hyperlinks) 97%, md/odf/latex low 90s, webvtt in the 80s (full table
  in §2). `wiki_duck` — the one HTML fixture still short of exact — is at 88%:
  furniture layer tokens, `<rtl>` direction markers, parenthesized link
  destinations and `aria-hidden` chrome are now upstream's; what remains is
  docling's `_list_item_has_segment_siblings` wrap rule (a list item takes a
  `<text>` wrapper only when it *owns* a nested list or picture — even an empty
  `<ul>` counts — where we wrap whenever any deeper item follows in the same
  list), plus `<sup>` reference markers and the footer's inline groups. The format-by-format work was
  tracked as [issue #32](https://github.com/docling-project/docling.rs/issues/32) and its
  children (#38–#41, #44) — all closed, targets met. This is an **output** format;
  a DocLang *input* backend is still out of scope (§5). For **PDF**, where the
  reference `<location>` geometry comes from docling's own layout run, the metric
  is scored with a ±2-grid-unit geometry tolerance (text/structure still
  byte-exact): **52% exact · 63% at ±2** (against the ≥50% target); the remaining
  gap is model-level (TableFormer/layout/reading order), not serialization — see
  [`PDF_CONFORMANCE.md`](./PDF_CONFORMANCE.md).

- **JSON** rebuilds docling's full `body`-tree-of-`$ref`s model from the `Node`
  tree (texts/groups/tables/pictures, labels, list grouping, table grids,
  formula/code items, picture `ImageRef`s). It loads back into Python
  docling-core and **~91% round-tripped** byte-identically to the direct
  Markdown when measured at the JSON-export milestone. A backend whose
  upstream tree the flat nodes cannot express hands the export a
  `docling_core::tree::ItemTree` instead (`DoclingDocument::tree`, built by
  the HTML backend's `html_tree.rs`, the DOCX backend's `docx_tree.rs` and,
  alongside its flat walk, the PPTX backend): docling's items in creation
  order with their parents, children, layers, formatting, hyperlinks,
  comment back-refs and — for a backend with page geometry — their exact
  `prov` (a `TreeProv`: the backend's own box, origin tag and charspan),
  serialized as they are — which is how the HTML, DOCX and PPTX JSON are
  structurally identical to upstream's (see those rows). In the PDF/image
  JSON every text item carries its page and box (#609): a picture's, table's
  or code block's caption its own caption region — not the item's, as
  docling's `ReadingOrderModel._add_caption_or_footnote` assigns it, within
  1.5 pt of upstream's boxes on the groundtruth corpus (the 0–511 grid's step) — and a
  checkbox item its region, so chunks made of captions or checkboxes have a
  page. Glyphs stacked at one x (a chart's y-axis ticks, each set by its own
  `cm`) are separate text cells with their own boxes, as in docling-parse —
  the text layer used to glue them into one cell boxed like the first. A
  checkbox item is docling's `checkbox_selected` / `checkbox_unselected`
  text with the bare option label (it was a `text` item spelling `- [ ] …`;
  the task-list marker is Markdown's rendering). And, a deliberate
  divergence (#609): a line marked as a checkbox on the page is its own
  checkbox item even when the layout model read the checklist as one text
  block — marked by a **drawn** square in front of it (a stroked `re`, or
  four stroked edges as ReportLab draws them, 5–24 pt, unfilled or filled
  white; `checkbox.rs` finds them in the text parser's path walk, ink inside
  checks it, a colour-filled square is a legend swatch and does not count)
  or by a **ballot-box glyph** it opens with (`☐ □ ▢ ◻ ❏ ❐ ❑ ❒` unchecked,
  `☑ ☒ ⊠ ⌧ ▣ 🗹 🗷 🗵` checked; a line with two, `☐ Yes ☐ No`, stays text). The
  glyph leaves the label, and on a region the model did label a checkbox the
  mark sets the state. docling has only the model's labels: on the
  reporter's checklist it prints two garbled paragraphs (`First option Third
  option` / `Second option Fourth option`), here four `- [ ]` items; on a
  `☐ / ☒ / ☑` list it prints `- [ ] ☑ Bread` (Heron reads that box as
  empty), here `- [x] Bread`. Symbol-font boxes that reach the text layer as
  Private Use Area codes or ASCII bytes (Wingdings `o`, `þ`) are not read as
  boxes — without the font the code is ambiguous.
- **Image extraction** is wired for PDF/image (figure-region crops) and DOCX/PPTX
  (embedded blobs) by default, and — opt-in via
  `DocumentConverter::fetch_images` (`--fetch-images`) — for HTML/EPUB `<img src>`:
  `data:` URIs, local files (relative to the source), remote `http(s)` URLs, and
  EPUB archive entries. Off by default, matching docling's `enable_*_fetch=False`.
  JSON always embeds extracted images as data URIs.

---

## 4. Differences from upstream docling

These are deliberate or unavoidable divergences, not bugs.

1. **Simplified document model.** `docling.rs`'s `Node` enum
   (`Heading`/`Paragraph`/`ListItem`/`Code`/`Table`/`Picture`/`Group`) is flatter
   than docling-core's `DocItem` graph. JSON export *reconstructs* the full
   `$ref` wire format from it; JSON input maps the other way.

2. **Inline formatting is baked into text.** Bold/italic/links/inline-math are
   stored as Markdown markers inside the text string, where docling keeps
   structured `formatting`/`hyperlink` fields. Consequence: for those spans the
   exported JSON carries the *rendered* text rather than structured fields, and
   ~9% of JSON→Markdown round-trips differ (URLs/`&`/`_` re-escaped by docling).

3. **`strict` Markdown mode is Rust-only.** Default output reproduces docling's
   legacy quirks (`***x*** .` run-spacing, dropped code-fence languages, `\_` and
   entity re-escaping); `strict` produces cleaner Markdown. docling has no such
   switch. All conformance numbers are measured in **legacy** mode.

4. **Tables use docling-core's padded GitHub format.** All backends emit the
   width-padded `tabulate(tablefmt="github")` tables that current published
   docling produces (columns padded to header-width+2 or the widest data cell,
   numeric columns right-aligned). The `compact_tables` option (`--compact-tables`,
   the serve/Python/Node option) renders the unpadded `| - |` form of
   `export_to_markdown(compact_tables=True)` on every path, streaming included.
   The committed **PDF groundtruth is upstream's own** (`tests/data/pdf/groundtruth`,
   which docling's test suite writes with `compact_tables=True` and `do_ocr=False`),
   so `scripts/conformance/pdf_groundtruth.sh` scores the pipeline with
   `--compact-tables --skip-ocr`; every other format's groundtruth is padded.

5. **The PDF pipeline is discriminative and byte-measured.** Ported from
   docling's standard pipeline:
   - **Layout** — RT-DETR (`docling-layout-heron`) exported to ONNX, run via
     `ort`. Same model family as docling.
   - **Hostile-input limits** — Markdown block nesting stops at 100 levels
     (markdown-it's `maxNesting`, docling's own Markdown parser), deeper
     quotes/lists are skipped; docling-JSON `children` references are walked
     once each and at most 256 deep (a cycle converts like its acyclic twin);
     XML inputs and OOXML/ODF parts nested past `DOCLING_RS_MAX_XML_DEPTH`
     (512) are rejected; a spreadsheet sheet whose used area exceeds
     `DOCLING_RS_SHEET_MAX_CELLS` (10M) is skipped; HTML past
     `DOCLING_RS_MAX_HTML_DEPTH` is emitted as text without a DOM; a PDF page
     rasterizing past `DOCLING_RS_MAX_RENDER_PIXELS` (15000 px/side) is rejected
     before the multi-GB bitmap is allocated.
   - **OCR** — RapidOCR's models via ONNX: the PP-OCRv6 text detector and,
     when installed (`download_dependencies.sh`), the PP-OCRv6 recognizer —
     docling's own RapidOCR default since 2.127 — else PP-OCRv3 recognition.
     Since #570 the recognizer reads the detector's boxes inside layout
     regions (RapidOCR's crops; projection strips only as the fallback) and
     drops lines under RapidOCR's `text_score`; the remaining differences are
     the crop geometry (axis-aligned box vs RapidOCR's perspective crop of the
     quad), RapidOCR's batch width padding and its angle classifier, which is
     not run. **Tesseract**
     (#460, `--ocr-engine tesseract` / `ocr_engine` on every surface) is the
     alternative — docling's `TesseractCliOcrOptions` port: the system binary,
     one subprocess per layout-region crop with `tsv` output, its own
     line/word segmentation inside the crop, orientation from its OSD,
     `ocr_lang` as tessdata stems or BCP-47 tags (`deu+fra`, `zh-Hant`), a
     docling-shaped `TesseractCliOcrOptions` mapped by the Python bindings
     (`lang`, `tesseract_cmd`, `path`, `psm`). Recognition runs
     on the lines inside layout regions; RapidOCR's PP-OCRv6 DB **text
     detector** (#429, the model docling's RapidOCR default runs) then sweeps
     the whole bitmap, and detected lines no recognized cell covers are
     recognized and placed as orphan text — docling's behavior for text its
     layout model gives no cluster (diagram labels, stamps, margin notes).
     Lines inside a kept picture/table stay silent children, as upstream. The
     detector input is capped at 960 px on the longer side by default
     (PaddleOCR's own default; RapidOCR's uncapped rule via
     `DOCLING_RS_OCR_DET_MAX_SIDE=0`) — ~⅓ of the detection time for
     noise-level output differences. The browser pipeline runs the same
     detector through onnxruntime-web when `ocr_det.onnx` is available.
   - **Tables** — **TableFormer** (image encoder + autoregressive OTSL structure
     decoder + cell-bbox decoder, ported to ONNX), on a cv2-exact preprocessed
     crop. Reproduces docling's padded GitHub tables — `2305-pg9` is cell-for-cell
     exact; multi-row headers / spans on the dense papers still differ.
   - **Text** — a **pure-Rust PDF text parser** (`textparse.rs`, on `lopdf`)
     reconstructs glyph boxes from font advance widths + the text/graphics matrices
     (matching docling-parse's geometry, not pdfium's rendered boxes); handles
     Type0/CID + simple fonts, ToUnicode/encodings, Form XObject recursion, a
     glyph-name fallback, and overprint dedup; glyphs are placed in pdfium's
     page frame — the CropBox ∩ MediaBox box with its lower-left corner as
     the origin, inherited through the page tree — so a trimmed or offset
     page lines up with the rendered bitmap and docling's `prov` boxes. It is
     the only text layer (pdfium's text page is gone). Its cells feed a port of
     docling-parse's line sanitizer (`dp_lines.rs`): 3-pass corner-distance
     contraction with gap-proportional space insertion, `enforce_same_font`,
     ligature recomposition, loose-box geometry. Plus docling's markdown escaping,
     docling-parse's typographic-punctuation table (every curly quote → `'`),
     wrap dehyphenation, paragraph-continuation merging, docling's rule-based
     reading-order predictor (2.127's same-row links included, #424) with
     cluster cells joined in docling-parse index order, and false-picture /
     page-number layout fixes. The parser is now the **sole** text
     source, and page count / geometry / `/Rotate` / link annotations are read
     from the same lopdf document (`pdf_meta.rs`), and an image-only page is
     rasterized in pure Rust byte-for-byte like pdfium (`raster/`: pdfium's
     stretch engine + a libjpeg-exact JPEG decoder), every other page by the
     pure-Rust renderer (`render/`) — pdfium itself is an opt-in cargo
     feature (`pdfium`: `DOCLING_RS_RENDERER=pdfium`, a file lopdf cannot
     read) and the default build links no native PDF library.
     Its per-word
     cells reproduce docling-parse's `word_cells` byte-for-byte (377/377 on
     `2305-pg9`), which is what TableFormer matches against; a char-frequency
     validator (`scripts/test/parser_completeness.py`) confirms nothing is silently
     dropped (Form-XObject text and glyph-name-only fonts were the two classes it
     surfaced and fixed).
   - Output is measured **byte-for-byte against live docling** (PDF_CONFORMANCE.md):
     **6 / 14 exact, 7 / 14 whitespace-normalized**, the rest close. The remaining
     gaps are model-level (TableFormer structure on complex tables, layout
     classification, title-page reading order) plus `amt`'s fraction spacing — a
     docling quirk from its embedded-font OS/2 metrics that our single-spaced output
     renders more faithfully; matching it exactly needs a font-metrics layer that
     entangles with the RTL box geometry. The full per-fixture breakdown and the
     model-level blockers live in `PDF_CONFORMANCE.md`.

6. **Extracted image bytes are real but not byte-identical.** Cropped/embedded
   pixels are correct, but the PNG re-encoding differs from docling's, so the
   base64 in `embedded` mode / JSON `ImageRef`s won't match byte-for-byte.

7. **XML format detection sniffs content.** JATS, USPTO and XBRL all use `.xml`;
   the converter routes by content markers (`us-patent` → USPTO, `us-gaap`/`dei`
   → XBRL, else JATS) rather than the extension alone.
   A file whose extension lies gets a second chance (#556): only when the
   conversion as its extension's format *fails* is the content checked —
   RTF's `{\rtf`, the PDF header, a ZIP package's parts (OOXML main part,
   ODF / EPUB `mimetype`), an OLE file's root streams (Word / Excel /
   PowerPoint / Outlook), HTML markup — and the file converted once more as
   that format, with a `docling: warning:` line naming both and the result's
   `format` the one used. A file that converts as named is never inspected,
   so nothing changes for it; when nothing is recognised or the retry fails
   too, the first error stands. docling decides by extension and MIME guess
   up front and fails such files (RTF / HTML / DOCX saved as `.doc`, the
   common case in legacy archives).

8. **Headless-browser pass is opt-in.** Form key-value regions, inline
   visibility, and nested-table cell flattening (docling's exact spacing) are
   all handled statically by default — no browser. Only stylesheet-driven
   (CSS-cascade) visibility suppression needs a rendered page, available behind
   the optional `web-browser` feature / `--use-web-browser` flag (Rust-driven
   Chromium) — see §5.

9. **Confidence pages are keyed 1-based.** The PDF/image pipeline attaches a
   docling-`ConfidenceReport`-shaped report (same grade thresholds, same
   nanmean/nanquantile aggregation; `table_score` unset like upstream) that
   docling-serve surfaces per response (v1.25 parity). Its `pages` map is
   keyed by the **real 1-based page number** — consistent with the JSON
   export's `pages` map and `--pages` windows — where Python docling keys by
   its 0-based internal page index. Unset scores serialize as `null`, not
   `NaN`.

10. **Page rasterization over HTTP** (#243). `to=images` on docling-serve's
    `/v1/convert` (sync, async, batch) renders a PDF's pages to PNG
    without running any conversion — the per-page base64 JSON covers
    the PDF-to-image use case Python docling-serve served; `pages=A-B`
    windows and `scale` (pixels per PDF point, default 2.0 = 144 dpi) apply,
    capped at `DOCLING_RS_MAX_RASTER_PAGES` (100) pages per request. The CLI
    counterpart is `--to images` + `--scale`, writing `<stem>_page_NNNN.png`
    files (no cap — the pages land on the caller's own disk).

11. **`do_ocr` and `do_table_structure` are independent** (#244), as in
    docling: `no_ocr` (`--no-ocr`, serve/Node `no_ocr`/`noOcr`, Python
    `do_ocr=False`; `skip_ocr` before 2.0, still read) keeps layout detection
    and TableFormer but never loads or runs OCR — pixel-only text comes back
    empty instead of erroring. The Python binding's `do_ocr=False` previously
    skipped the whole ML stack (layout and tables included), which docling's
    `do_ocr` never meant; that fast path is the docling.rs-only
    `text_layer_only` (`--text-layer-only` — before 2.0, #611, the CLI's
    `--no-ocr`). A missing OCR model also degrades to the `no_ocr` behavior
    with a one-time warning — docling errors there — matching the repo-wide
    degradation-over-failure convention.

12. **Sparse spreadsheets can skip empty cells** (#271, docling.rs-only
    options, both off by default): `skip_empty_cells` omits empty positions
    from each XLSX/XLS table row instead of materialising every region's
    full bounding box (the JSON keeps the surviving cells at their true
    offsets and spans through first-class cells, so `table_cells` is the
    dense list minus the empties — padding the compacted rows back at export
    used to make it larger than the dense one) (docling pads the box too; the related upstream #3328
    tracks the RAM cost of its one-TableCell-per-cell materialisation on
    large sheets), and `compact_tables` renders Markdown tables unpadded for
    every format. Default output stays byte-for-byte docling.

13. **`OcrMode` and `OcrOptions.scale`** (docling 2.116/2.117, #254) are
    mirrored on every surface as `ocr_mode` (`--ocr-mode`,
    `DOCLING_RS_OCR_MODE`) and `ocr_scale` (`--ocr-scale`,
    `DOCLING_RS_OCR_SCALE`). `pdf_aware_layout_regions` — upstream's new
    default, OCR gated by layout regions and the PDF text layer — is the
    architecture this port always had, so the default behavior is already
    aligned; `full_page` and `layout_regions` both map onto the
    `force_full_page_ocr` machinery (the whole-page vs per-region *detector*
    distinction has no analogue in the det-free PP-OCR recognizer here).
    `ocr_scale` resamples the OCR input from the pipeline's 2.0 px/pt page
    render instead of re-rendering (docling renders natively at
    `72 × scale` dpi, default 3); unset keeps the pinned 144 dpi baseline,
    so conformance snapshots never move. **OCR language spellings** (#388,
    docling#4075's BCP-47 canonicalization): upstream reads a bare value as
    the engine's native code and an `iso:`-prefixed one as a BCP-47 tag
    reduced to language + script (region dropped), then maps it per engine —
    RapidOCR's `en` and `zh-Hans` → `ch`. This engine ships exactly those two
    PP-OCRv3 recognizers, so `ocr_lang` on every surface (`--ocr-lang`,
    `DOCLING_RS_OCR_LANG`, serve, Python, Node, RAG's `RAG_OCR_LANG`) accepts
    `en`/`ch` plus any English or Chinese tag with or without the prefix
    (`en-US`, `eng`, `english`, `zh`, `zh-Hans`, `zh-TW`, `zho`, `chinese`,
    EasyOCR's `ch_sim`); script/region subtags are ignored, a traditional-
    script request gets the multilingual `ch` recognizer (upstream would pick
    RapidOCR's separate `chinese_cht`, not shipped here), and any other
    language is rejected (warn-and-default only for the env var). Under the
    Tesseract engine (#460) the same option is Tesseract's language list:
    stems verbatim (`deu`, `script/Latin`), two-letter BCP-47 tags mapped
    onto ISO 639-2/T stems with docling's deviations (`zh-Hans` → `chi_sim`,
    `sr-Latn` → `srp_latn`, `nb` → `nor`), `en`/`ch` still accepted, the
    installed set checked when the engine loads.

14. **Per-request chunking configuration** (docling 2.117–2.119
    service-datamodel, #256): `to=chunks` accepts `chunker`
    (`hierarchical`|`hybrid`, docling's `ChunkerType`; unset returns both),
    `chunk_tokenizer` (a server-local *relative* `tokenizer.json` path —
    absolute paths and `..` are rejected, unlike upstream's
    fetch-any-HF-model semantics), `chunk_max_tokens` and
    `chunk_merge_peers` on serve, with matching `--chunker`/`--chunk-*` CLI
    flags; the `DOCLING_CHUNK_*` env knobs remain the operator defaults. An
    explicit `chunker=hybrid` without a usable tokenizer fails loudly (400)
    instead of the legacy silent skip. Out of the same sync window but *not*
    ported: upstream's `do_pdf_heading_hierarchy` (`HeadingHierarchyModel`
    — PDF section-header levels from bookmarks/numbering/font style; our
    PDF path emits docling's pre-2.117 flat `##` levels, so the option has
    nothing to switch yet) and `dclx` in the `ArtifactRef` union (only
    meaningful once the sources/targets wire shape lands — #139).

15. **Cloud source/target passthrough** (#139, upstream docling#3795 /
    docling-jobkit): the serve JSON body accepts docling's
    service-datamodel `sources`/`target` shape — `kind`-tagged `file`
    (base64) and `http` (URL + headers) sources always, and `s3` /
    `azure_blob` / `google_cloud_storage` sources *and* targets behind the
    opt-in `cloud` cargo feature (a feature-gated `object_store`
    dependency, so the default build keeps its single-HTTP-stack graph).
    Coordinate field names mirror upstream's `*Coordinates` models;
    a cloud target uploads each converted output as `<stem>.<ext>` under
    the prefix and answers with upstream's `RemoteTargetResult` kind plus
    per-item outcomes. All outbound kinds sit behind `--allow-url-fetch`.
    The jobkit `zip` (one archive of the rendered outputs in the response
    body — a batch download) and `put` (HTTP PUT each output to a
    caller-minted, typically pre-signed, URL — behind `--allow-url-fetch` +
    the SSRF check) targets are covered too (#303), with no
    `object_store`/`cloud`-feature involvement. Not covered, as
    upstream-jobkit-specific or OAuth-bound: `google_drive` and
    `presigned_url` (and with the latter, the `ArtifactRef` union — still
    parked; `put` with a caller-minted URL covers the pre-signed use case).
    Without the feature the cloud kinds parse and answer a clear
    rebuild-with-`--features cloud` error.

16. **Serve observability** (#297, Python docling-serve's OTel
    integration): the same *posture* — Prometheus-style metrics on by
    default, OTLP traces opt-in, `/metrics`+`/health`+`/ready` excluded
    from request telemetry, `OTEL_SERVICE_NAME` defaulting to
    `docling-serve` — but not the same mechanism. Requests log through
    `tracing` (`RUST_LOG`, default `info`); `GET /metrics` serves
    hand-rolled dependency-free counters (requests by status class,
    in-flight gauge, latency histogram with buckets stretched to
    minutes-long ML conversions, conversions by outcome) instead of
    Python's OTel-SDK `PrometheusMetricReader`, so metric *names* differ
    from a Python deployment's. OTLP/gRPC span export sits behind the
    opt-in `otel` cargo feature and activates only when
    `OTEL_EXPORTER_OTLP_ENDPOINT` is set. Not covered: OTLP *metric* and
    *log* export (Prometheus scrape and stderr logs stand in), and
    Python's `OTEL_*` sampler knobs.

17. **PDF heading levels** (#302, docling's `HeadingHierarchyModel`): the
    heading-hierarchy stage is ported — bookmarks (a pure-lopdf outline
    reader: titles, depths, XYZ/FitH/FitBH/FitR destinations) > legal/outline
    numbering (the full marker grammar incl. ambiguous single-letter
    Roman/alpha resolution; docling 2.129's colon/dash/bracket/parenthesized
    separators — `1: Intro`, `(2) Scope`, `A - Annex`, docling#4204 — and bare
    Arabic chapter numbers accepted on a consecutive run from 1, docling#4179) > font style (a hand-rolled port of docling's
    conservative font-name parser plus size clustering), with docling's
    fuzzy bookmark matcher (a `difflib.SequenceMatcher.ratio` port) and
    list-item promotion. Off by default on every surface, like upstream —
    all conformance numbers are measured with it off. Deliberate
    divergences: style aggregates over text-layer *glyphs* rather than
    parsed line cells (same signal, finer granularity), OCR-only headings
    carry no style signal (docling reads OCR cell heights), and enabling
    the stage buffers a streaming Markdown conversion into one chunk (the
    stage needs the whole assembled document; streamed output stays
    byte-identical to buffered).

18. **Image outputs** (#518–#520): docling's `images_scale` and
    `generate_page_images` act on every surface (`--images-scale` /
    `--page-images`, `images_scale` / `page_images` in serve and the FFI,
    `imagesScale` / `pageImages` in Node, the Python kwargs and the
    docling-shaped `PdfPipelineOptions`). Page images land on
    `pages[n].image` in the JSON (docling-core's `PageItem.image`), which
    makes `TableItem.get_image` / `FormulaItem.get_image` work on the loaded
    document. Picture `ImageRef.dpi` is now the crop's real render scale,
    72·scale — 144 for the default 2× crops, where it said 72 (#519); office
    images keep docling's 72, and a docling-JSON input's dpi round-trips.
    Divergences: picture crops are always extracted (docling only with
    `generate_picture_images`), so the Python facade applies `images_scale`
    only once docling would render images and otherwise keeps the 2× crop;
    other scales resample the pipeline's 2.0 px/pt render instead of
    re-rendering the page (CatmullRom ≙ PIL BICUBIC, so above 2.0 it
    upsamples); text-layer-only (`text_layer_only`) pages and streamed Markdown carry
    no page image. Python's `convert(source, page_range=(a, b))` now takes
    docling's per-call window (#518; it was a native-only constructor
    kwarg).

19. **docling-serve's own API** (#615). Besides `/v1/convert`, the server
    answers upstream docling-serve's routes — `POST /v1/convert/file`,
    `/v1/convert/source`, their `/async` variants, `GET
    /v1/status/poll/{task_id}`, `/v1/result/{task_id}`, and the `/v1alpha`
    aliases — with upstream's `ConvertDocumentResponse` /
    `TaskStatusResponse` shapes, so Open WebUI's Docling loader, n8n, Dify
    and LangChain's docling-serve client work unchanged; `--api-key` /
    `DOCLING_SERVE_API_KEY` is upstream's `X-Api-Key`. Upstream's option
    names map onto ours. Deliberate differences: options with no
    equivalent (`pdf_backend`, `table_mode`, `abort_on_error`, picture
    description, presets) are ignored rather than rejected, so a client's
    `DOCLING_PARAMS` never fails a conversion. OCR engines other than
    Tesseract run the built-in PP-OCR, and `ocr_lang` keeps the first code
    this build has a recognizer for. `image_export_mode=referenced` is
    answered as placeholders, since a response body has nowhere to put
    files. `doctags_content` is `null`, because there is no DocTags writer.
    `timings` is `{}`. A failed document answers a `200` envelope with
    `status: "failure"`, as upstream does for a conversion error.

---

## 5. Not migrated / out of scope

Nothing here blocks day-to-day conversion: every remaining item is either a
deliberate scope boundary or a cosmetic, single-fixture polish gap.

**Out of scope by design:**

- **Local VLM full-page inference** (SmolDocling-class models in-process).
  Model-bound; out of scope for the discriminative port. The **remote** VLM
  pipeline (#77) *is* implemented: `--pipeline vlm --vlm-endpoint URL
  --vlm-model NAME` renders pages (pure Rust), converts them through any
  OpenAI-compatible vision endpoint (LM Studio / Ollama / vLLM / hosted) and
  parses the returned DocLang with the existing reader — see the README's
  "VLM pipeline" section. Also on the Node bindings as `pipeline: 'vlm'`
  (`vlmEndpoint` / `vlmModel` / `vlmApiKey` / `vlmPrompt` / `vlmMaxTokens`),
  on the Python bindings as the same-named constructor kwargs, and on serve
  as `pipeline=vlm` + `vlm_*` request options (#304; a request-supplied
  `vlm_endpoint` sits behind `--allow-url-fetch` + the SSRF check, or pin it
  server-side via `DOCLING_RS_VLM_*`). Measured against Python docling's
  `VlmPipeline` on the same granite-docling endpoint: **87.7% mean
  similarity over the 18-fixture PDF corpus, 3 byte-exact** (#311; drift is
  mostly render-scale-induced — see PDF_CONFORMANCE.md). (**Audio/ASR is now done** — see §2; Opus and AVI,
  which symphonia cannot decode, use the optional ffmpeg fallback. The **enrichment
  models are now done** too: DocumentFigureClassifier-v2.5 for
  `do_picture_classification` and CodeFormulaV2 — an Idefics3-class VLM,
  exported to a three-graph ONNX set with a KV-cached greedy decode verified
  token-identical to `transformers.generate` — for `do_code_enrichment` /
  `do_formula_enrichment`; opt-in flags on the converter/CLI/Python bindings,
  and since #423 on docling-serve — `do_picture_classification` /
  `do_code_enrichment` / `do_formula_enrichment` as query, multipart and JSON
  body options, Python docling-serve's names, in the OpenAPI spec and the web
  UI; the warm pipeline is rebuilt when a request's enrichment mix differs
  from the cached instance's, as for the model switches — and the Node
  bindings (`doPictureClassification` / `doCodeEnrichment` /
  `doFormulaEnrichment`, also on `new Pipeline()` — which since #471 reads
  every PDF/image option of `ConverterOptions`, `ocrEngine` / `ocrLang` /
  `ocrMode` / `ocrScale` / `skipOcr` / `forceFullPageOcr` / `noTextPanels` /
  `headingHierarchy` / `pages`, validated like `DocumentConverter`);
  conformance-checked by `scripts/conformance/enrich_conformance.sh`. Since
  #517 the classifier's session stops at ONNX Runtime's `EXTENDED`
  optimization level on runtimes before 1.29 — 1.26–1.28's x86 NCHWc rewrite
  returned one distribution for every picture — so the linked 1.28 builds
  classify like Python docling on 1.29+.)

**Now migrated (previously listed here):**

- **XML DocLang input backend.** Reading `.dclg`/`.dclg.xml` (bare DocLang XML)
  and `.dclx` archives back into a `DoclingDocument` — the corpus gap closed
  itself once `--to dclx` shipped: docling's own `.dclx` groundtruth archives
  are the sources, and docling 2.112 reads them natively (`InputFormat.DCLX`),
  so the backend is scored live like every other format — **15/15 exact**,
  reproducing docling's own round-trip semantics (whitespace collapse vs
  verbatim CDATA/`<content>`, span text re-expansion, the formatting docling
  drops on list items and hyperlink targets).

- **DOCX grouped/anchored drawings and floating text frames.** Blip-less
  DrawingML shapes yield docling's one-rendered-picture-per-paragraph as a
  placeholder (docling rasterizes them through LibreOffice; the Markdown
  placeholder is identical without rendering), pictures are emitted for
  heading/list/checkbox paragraphs too, and the textbox de-duplication matches
  docling's per-paragraph scope — `drawingml` and `textbox` are exact,
  **DOCX is 27/27**.

- **Legacy APS-text patents.** USPTO covers the modern `v4x` XML, the 2001-era
  `pap-v15` applications (`pa`) and `PATDOC`/ST.32 grants (`pg`) with their CALS
  tables, **and** the legacy **APS plain text** (`pftaps`): docling's format
  detection routes a `text/plain` file opening with a `PATN` record to its
  `PatentUsptoGrantAps` parser (title, abstract, caption headings, claims),
  and docling.rs ports it — Markdown, the JSON item tree and the `.dclx` are
  exact ([issue #44](https://github.com/docling-project/docling.rs/issues/44),
  done; the earlier text-dump `.dclx` groundtruth came from routing the file
  through the Markdown backend).

- **ODF presentation frames** — done, **all native files exact**: `.odp`
  slides get their title frame (or slide name) as the title, free shape text,
  chart pictures with classification ("Bar chart") + data tables, and the
  speaker-notes drop; `.odt` merged cells repeat their text like docling's
  plain `TableData` grid (while rich cells dedup), and paragraph runs
  reproduce docling's lxml head-text semantics (a tail after a styled span is
  dropped). Everything else on ODF was already done: mixed-style list
  continuation, empty-list-item level collapse, ODS sheet→table region
  detection with numeric alignment, and rich table cells.

**Minor known gaps (cosmetic, tracked per-fixture):**

- ~~**`wiki_duck` offline rendering.**~~ **Closed** — the HTML corpus is now
  32/32 Markdown-exact against live docling 2.112, `wiki_duck` included. What
  finished it (issue #81): rich table cells serialized with inline markup and
  docling's `visited`-set span de-duplication, `to_single_text_element`
  figure-caption run spacing, `mw:File` indicator images (alt caption +
  placeholder), `<footer>` → furniture layer, and `<br>`
  annotation-boundary handling. The HTML subsystem also covers key-value form
  regions, inline visibility suppression, deep nested-table cell flattening
  with BeautifulSoup whitespace semantics, and — behind the optional
  `web-browser` feature / `--use-web-browser` flag — CSS-cascade visibility
  suppression via Rust-driven Chromium.


---

## 6. Extensions

- **`docling-rag`** — documents → chunking → embeddings → vector search,
  with swappable embedders (Ollama/Gemini/local ONNX), stores
  (SQLite+sqlite-vec / PostgreSQL+pgvector), LLM, sources and queues, plus an
  eval harness and a REST API. See the crate README.
- **`docling-node`** — Node.js/Bun N-API bindings (npm package): the full
  converter surface plus the chunkers, Markdown/chunk streaming, a warm
  `Pipeline` for many PDFs, and the remote VLM pipeline (`pipeline: 'vlm'`).
- **`docling-wasm`** — WebAssembly bindings: the declarative converters (and
  digital PDFs via the opt-in `pdf-text` text-layer feature — the same
  extraction as `--text-layer-only`, no ONNX) run fully client-side in the
  browser, ~1.9 MB gzipped; scanned PDFs return a "needs OCR" error. Python
  docling has no equivalent. See the crate README.
- **`docling-py`** — PyO3 bindings (PyPI package `docling-rs`): a strangler-fig
  drop-in for docling's Python API where the Rust engine is the document
  processor and `result.document` is a genuine `docling_core` `DoclingDocument`,
  so its `export_to_markdown()` / `export_to_dict()` / chunkers are docling's
  own code.
  `docling_rs.langchain` (extra `[langchain]`) ports docling's
  [langchain-docling](https://github.com/docling-project/docling-langchain):
  `DoclingLoader` (`ExportType.DOC_CHUNKS` / `MARKDOWN`, pluggable chunker and
  meta extractor) reproduces that package's own test fixtures exactly, and
  `PictureDescriptionLangChainOptions` describes pictures with any LangChain
  chat model (run on the converted document, every input format). URL sources
  (`convert("https://…")`) download first, as in docling.

- **Picture OCR for non-PDF documents** (#645) — docling only OCRs through
  its PDF/image pipeline, so the text inside a DOCX/PPTX screenshot, an HTML
  figure or a video frame is never read. `do_picture_ocr` (every surface:
  `--picture-ocr`, serve `do_picture_ocr`, Python `do_picture_ocr` /
  `PdfPipelineOptions.do_picture_ocr`, Node `doPictureOcr`) runs the same
  OCR models — the PP-OCR recognizer with the `ocr_det.onnx` line crops at
  docling's image resolution, or Tesseract under `ocr_engine` — over every
  embedded picture of a non-PDF document after the backend, and attaches the
  lines (reading order; `DOCLING_RS_OCR_TEXT_SCORE` applies) as docling's
  picture description — `meta.description` (`created_by` = the engine) plus
  the `PictureDescriptionData` annotation, the shape `PictureDescriptionBaseModel`
  writes, so a docling-core consumer reads it like a VLM caption. Markdown
  prints it between the caption and the placeholder (docling's
  `MarkdownPictureSerializer` order: captions, annotations, image), the chunkers
  put it in the picture's chunk, DocLang / LaTeX / Pandoc are unchanged
  (docling's DocTags carry no description either). Filters:
  `picture_ocr_classes` (the DocumentFigureClassifier's top label must be one
  of the listed classes; a label it never predicts is rejected) and
  `picture_ocr_min_side` (32 px, `DOCLING_RS_PICTURE_OCR_MIN_SIDE`);
  `keep_picture_images=false` drops the image bytes after the pass. Off by
  default — the default exports of every fixture are byte-identical — and a
  picture without text, `no_ocr` / `text_layer_only`, or a missing model
  attaches nothing (one warning). PDF/image/METS inputs are untouched: their
  pages went through the OCR pipeline already. The dedupe is by image bytes,
  so a DOCX (walked into both the flat nodes and the item tree) or a slide
  master's logo is read once. `picture_ocr.docx` (a 720 × 180 two-line
  screenshot and a 16 × 16 icon) is the fixture; the e2e test also reads
  the text drawn into a generated video clip's frame.

## 7. Testing

- **`cargo test`** — unit tests per backend/serializer **plus an output-
  regression suite** (`crates/docling/tests/regression.rs`): every
  declarative source under `crates/docling/tests/data/<fmt>/sources/` is
  converted to legacy Markdown, strict Markdown and docling JSON and compared to
  committed fixtures (159 sources × 3). `DOCLING_RS_REGEN=1` refreshes them.
  The JSON fixtures double as a docling-core load check.
- **Snapshot harness** — `scripts/conformance/pdf_conformance.sh` regenerates and diffs the
  PDF/image/METS baseline (needs the ONNX models + the docling-parse plugin; **94 outputs, all
  matching the committed baseline**).
- **Conformance** — `scripts/conformance/conformance.sh <fmt>` scores a format against the
  latest published docling (installed from PyPI; how-to in §9).
- **VLM / DocLang extras** — `scripts/conformance/vlm_conformance.sh` (VLM
  pipeline vs Python docling's `VlmPipeline`) and
  `scripts/conformance/dclx_pdf_tol_sweep.sh` (geometry-tolerance sweep for
  the PDF `.dclx` diff).
- **Differential / perf** — `scripts/conformance/compare.sh`, `scripts/test/performance.sh`.
  The PDF pipeline's profiling data, the INT8/SIMD optimization results
  (4.3× warm vs Python docling on the ML pipeline), and the remaining
  performance backlog live in [`PDF_CONFORMANCE.md`](./PDF_CONFORMANCE.md).

CI (`.github/workflows/ci.yml`) gates every pull request and master push on
`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo test` (the fast pure-Rust suite — no model downloads). fmt/clippy run on a
**pinned** toolchain (`LINT_TOOLCHAIN` in the workflow) so a new stable can't fail
CI on unrelated commits; tests run on current `stable`. On master it then runs
`scripts/ci/release.sh`: it derives the next version from the conventional-commit
messages since the last `v*` tag (`feat:` → minor, `fix:`/`perf:` → patch, a
`type!:`/`BREAKING CHANGE` → major; docs/chore/ci/etc → no release), bumps the
workspace version, commits + tags it (with `[skip ci]`, via the `RELEASE_PAT`
admin token — needed to satisfy the master ruleset — so it
doesn't loop), and publishes the crates with `scripts/ci/ci_publish.sh` in
dependency order — skipping any version already on crates.io. The uploads
authenticate through crates.io Trusted Publishing (OIDC: the job's
`id-token: write` identity — `docling-project/docling.rs` + `ci.yml` — is what
each crate's crates.io settings name as its trusted publisher; a fresh
30-minute token is minted per crate by `scripts/ci/crates_io_token.sh`), so
there is no registry token secret to rotate.

---

## 8. Goals & design rules (unchanged)

- A tiny, obvious public API — one `DocumentConverter`, one `convert`, one
  `DoclingDocument` you can `export_to_markdown()` / `export_to_json()`.
- Dependency-light pure-Rust parsing for everything that isn't ML.
- Output byte-compatible with docling-core's serializers where it reasonably can
  be, so the port is a drop-in for downstream Markdown/JSON consumers.
- The ML stack is *not* reimplemented in PyTorch-equivalent Rust; it is
  quarantined behind ONNX (`ort`) inference in `docling-pdf`.

---

## 9. Keeping up with upstream docling

The port is built to be *measured against* upstream rather than merely
inspired by it, which makes tracking new docling releases a mechanical
process instead of a guess:

1. **Detect drift.** `scripts/conformance/conformance.sh <fmt>` installs the **latest
   published docling from PyPI** into an isolated venv and byte-diffs both
   engines' Markdown over the committed corpus, per fixture. An upstream
   release that changes output (a serializer tweak, a new label, a model
   bump) shows up as a concrete per-fixture diff — not as silent divergence.
   `scripts/conformance/compare.sh` does the same for a single ad-hoc document.
2. **Classify each diff.** Either upstream changed *serialization/logic* —
   port the change to the matching backend/serializer (the crate layout in §1
   maps one-to-one to docling's modules, so the port target is usually
   obvious) — or upstream shipped *new models*, in which case
   `scripts/install/export_layout.py` / `export_tableformer.py` re-export the new
   checkpoints to ONNX, `scripts/install/quantize_models.py` re-quantizes, and
   `.github/workflows/publish-models.yml` republishes the model release
   (bump the tag when the export itself changes).
3. **Re-gate.** `scripts/conformance/pdf_conformance.sh` (deterministic snapshot baseline)
   plus the 159-source regression suite in `cargo test` confirm nothing else
   moved. The committed PDF groundtruth is regenerated from live docling
   (`scripts/conformance/pdf_groundtruth.sh`) whenever upstream output legitimately
   changes, so "exact" always means *exact against current docling*.
4. **New formats/features** follow the same recipe the existing 30 fo