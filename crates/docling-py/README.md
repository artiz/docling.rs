# docling-py — Python bindings (PyO3)

A **strangler-fig drop-in** for Python docling's common conversion path,
backed by the Rust [docling.rs](https://github.com/docling-project/docling.rs) engine:
same call shape, no torch, ~4× faster PDF conversion at a fraction of the
memory (see [`docs/PDF_CONFORMANCE.md`](../../docs/PDF_CONFORMANCE.md)).

```python
# was:  from docling.document_converter import DocumentConverter
from docling_rs import DocumentConverter

result = DocumentConverter().convert("document.pdf")
print(result.document.export_to_markdown())
data = result.document.export_to_dict()     # docling-core JSON wire format (schema 1.10.0)
```

**Only the document processor is Rust.** The engine parses the input and returns
docling-core's JSON wire format; this package validates it into a genuine
[`docling_core.types.doc.DoclingDocument`](https://github.com/docling-project/docling-core).
So `result.document` **is** the docling object — `export_to_markdown()`,
`export_to_dict()`, `export_to_doctags()`, the serializers, and the
[chunkers](https://github.com/docling-project/docling-core) are docling's own
Python code, unchanged. `docling-core` is a runtime dependency; nothing else from
docling is required for the declarative path.

> **Status: experimental.** The PyPI distribution name is `docling-rs`.
> Releases are published automatically (like the npm package) by the
> [`pypi-publish`](../../.github/workflows/pypi-publish.yml) workflow whenever
> CI cuts a GitHub Release — see [Publishing](#publishing) below. The crate is intentionally outside the repo's
> Cargo workspace and its crates.io publish flow. For development, build and
> install locally as shown next.

## Migrating from Python docling

The package is designed so that a typical docling script moves over by
changing **the install and the imports** — the code below the imports stays
as-is, because `result.document` is a genuine `docling_core` `DoclingDocument`
and all config objects are re-exported docling-shaped.

**1. Swap the package.** `docling-rs` and `docling` can coexist in one
environment (different module names), so you can A/B them during the
transition; drop `docling` once nothing imports it. For the GPU build install
`docling-rs-cuda` **instead of** `docling-rs` (same `docling_rs` module —
never both):

```bash
pip install docling-rs            # CPU wheels: Linux x86-64/arm64/s390x, Windows; sdist elsewhere
# or, with an NVIDIA GPU (Linux x86_64, CUDA 12 + cuDNN 9, glibc ≥ 2.38):
pip install docling-rs-cuda       # converts on the GPU automatically, CPU fallback
pip uninstall docling             # optional — only when you no longer import it
```

**2. Rewrite the imports: replace `docling` with `docling_rs`** — the package
mirrors docling's module layout one-to-one, so it's a mechanical rename
(`sed -i 's/\bdocling\./docling_rs./g'` on the import lines does it):

| Python docling import | docling.rs import |
|---|---|
| `from docling.document_converter import DocumentConverter, PdfFormatOption` | `from docling_rs.document_converter import DocumentConverter, PdfFormatOption` |
| `from docling.datamodel.base_models import InputFormat, DocumentStream` | `from docling_rs.datamodel.base_models import InputFormat, DocumentStream` |
| `from docling.datamodel.pipeline_options import PdfPipelineOptions, AcceleratorOptions, TableFormerMode` | `from docling_rs.datamodel.pipeline_options import …` (same names) |
| `from docling.datamodel.accelerator_options import AcceleratorDevice, AcceleratorOptions` | `from docling_rs.datamodel.accelerator_options import …` (same names) |
| `from docling.datamodel.document import ConversionResult` | `from docling_rs.datamodel.document import ConversionResult` |
| `from docling.exceptions import ConversionError` | `from docling_rs.exceptions import ConversionError` |
| `from docling.chunking import HybridChunker, HierarchicalChunker, BaseChunk` | `from docling_rs.chunking import HybridChunker, HierarchicalChunker, BaseChunk` |
| `from docling.utils.model_downloader import download_models` | `from docling_rs.utils.model_downloader import download_models` |
| `from docling_core.types.doc import …` (types, `ImageRefMode`, serializers) | unchanged — `docling_core` stays a dependency and `result.document` is its `DoclingDocument` |

Every name is *also* exported flat from the package root
(`from docling_rs import DocumentConverter, PdfPipelineOptions, …`) — the
submodules are aliases of the same objects, kept for drop-in parity. An import
docling.rs has no equivalent for (e.g. `docling.backend.*`, VLM pipeline
options) raises `ModuleNotFoundError` at the import line — the honest signal
that the code touches a not-ported area (see step 4).

A minimal script, before and after:

```python
# before                                         # after
from docling.document_converter import (         from docling_rs.document_converter import (
    DocumentConverter,                               DocumentConverter,
)                                                )
conv = DocumentConverter()                       conv = DocumentConverter()
result = conv.convert("report.pdf")              result = conv.convert("report.pdf")
md = result.document.export_to_markdown()        md = result.document.export_to_markdown()
```

**3. Fetch the models once** (PDF/image path only — declarative formats need
none): `python -c "import docling_rs; docling_rs.download_models()"`
(~700 MB to `~/.cache/docling.rs`, idempotent). docling's own model cache is
not reused; torch, transformers and CUDA-for-python are no longer needed —
the engine bundles ONNX Runtime.

**4. Check the divergences** if your code goes beyond the common path:
the full-VLM pipeline (SmolDocling) and per-format backend selection are not
ported; some `PdfPipelineOptions` fields are accepted for compatibility but
inert (`table_structure_options.mode`, …); inline formatting is
rendered into the text rather than structured `formatting` fields. The
[API surface](#api-surface-docling-shaped) table below lists what acts, and
[`docs/MIGRATION.md`](../../docs/MIGRATION.md) §4 the documented output
divergences. `HybridChunker(tokenizer=…)` takes a `tokenizer.json` **path**
(no `transformers`) instead of a HF model name.

**5. GPU** (`docling-rs-cuda`): no code changes — the wheel defaults to
`auto` (GPU when usable, CPU fallback). `DOCLING_RS_EP=cpu` or
`AcceleratorOptions(device="cpu")` forces CPU; `DOCLING_RS_EP=cuda` /
`device="cuda"` pins the GPU and fails loudly instead of falling back. See
[GPU wheel](#gpu-wheel-docling-rs-cuda) for the runtime requirements.

## Try it locally

Needs a Rust toolchain (1.88+, the workspace MSRV) and Python ≥ 3.9.

```bash
cd crates/docling-py

# 1. Build + install into the CURRENT virtualenv (create one first):
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop --release          # compiles the Rust engine, installs `docling-rs`

# 2. One-time model download (~700 MB → ~/.cache/docling.rs), pure Python —
#    fetched from the repo's models-v1 GitHub release, like docling fetches
#    its artifacts. Declarative formats (DOCX/HTML/XLSX/…) skip this entirely.
python -c "import docling_rs; docling_rs.download_models()"

# 3. Convert:
python - <<'PY'
from docling_rs import DocumentConverter

conv = DocumentConverter()
result = conv.convert("../../tests/data/pdf/sources/2305.03393v1-pg9.pdf")
print(result.status)                            # "success"
print(result.document.export_to_markdown()[:400])
PY
```

## API surface (docling-shaped)

| docling.rs | docling counterpart | notes |
|---|---|---|
| `DocumentConverter(format_options=None, *, allowed_formats=None, do_ocr=True, do_table_structure=True, force_full_page_ocr=False, no_text_panels=False, heading_hierarchy=False, do_picture_classification=False, do_code_enrichment=False, do_formula_enrichment=False, do_picture_ocr=False, picture_ocr_classes=None, picture_ocr_min_side=None, keep_picture_images=True, redact_pii=False, redact_mode=None, redact_kinds=None, redact_pattern=None, redact_images=None, fetch_images=False, use_web_browser=False, artifacts_path=None, ocr_lang=None, asr_model=None, asr_lang=None, pipeline=None, vlm_endpoint=None, vlm_model=None, vlm_api_key=None, vlm_prompt=None, vlm_max_tokens=None)` | `DocumentConverter(allowed_formats=…, format_options=…)` | Pass `{InputFormat.PDF: PdfFormatOption(pipeline_options=PdfPipelineOptions(…))}` or the shorthand kwargs; `allowed_formats` restricts conversion; `artifacts_path` overrides the model cache dir. |
| `.convert(path \| url \| DocumentStream) -> ConversionResult` | `.convert(source)` | str / `pathlib.Path` / `http(s)://` URL (downloaded first; the format comes from the file name, else the response's `Content-Type`) / `DocumentStream`. Releases the GIL during conversion. |
| `.convert_all(sources, raises_on_error=True) -> Iterator[ConversionResult]` | same | lazily converts many sources; `raises_on_error=False` yields a `failure` result instead of raising |
| `.initialize_pipeline(format=None)` | same | pre-loads the PDF/image ML models so the first conversion isn't slow and later PDFs reuse the warm pipeline (no-op for non-ML formats; needs the models available) |
| `.convert_bytes(name, data)` | `DocumentStream` | extension of `name` drives format detection |
| `.convert_archive(source) -> Iterator[ArchiveItem]` | — (docling takes no archives) | every document inside a ZIP — a path, `bytes` or a `DocumentStream` — one `ArchiveItem` per entry: `outcome` `converted` (`result`) / `skipped` (`error` = the reason: unsupported type, nested archive, unsafe path, over a `DOCLING_RS_ZIP_MAX_*` limit) / `failed` (`error`); a broken document fails only itself (#557) |
| `InputFormat`, `PdfPipelineOptions`, `PdfFormatOption`, `AcceleratorOptions`, `TableFormerMode`, `DocumentStream`, `ImageRefMode` | same modules | docling-shaped config re-exported from `docling_rs` (see below) |
| `ConversionError` | `docling.exceptions.ConversionError` | raised on a failed conversion; caught by `convert_all(..., raises_on_error=False)` |
| `EncryptionError`, `PasswordRequiredError`, `WrongPasswordError` | — (docling.rs extension, #636) | subclasses of `ConversionError` an encrypted document raises: no password given / the given one is wrong (prompt for one) / a scheme docling.rs cannot decrypt (`EncryptionError` itself); `from docling_rs.exceptions import …` |
| `result.status` / `result.document` / `result.input.file` | same | `.status` is a `ConversionStatus` str-enum (`"success" / "partial_success" / "failure"`); `.document` is a genuine `docling_core` `DoclingDocument` |
| `document.export_to_markdown(...)` | same | docling-core's own method — all of docling's params (`image_placeholder`, `page_break_placeholder`, …) apply |
| `document.export_to_dict()` / `export_to_json()` / `export_to_doctags()` | same | docling-core's own serializers over the wire format |
| `document.save_as_markdown(p)` / `save_as_json(p)` / chunkers | same | anything `docling_core` offers on a `DoclingDocument` works, since it *is* one |
| `docling_rs.email_attachments(source, *, max_entries=None, max_entry_size=None, max_total_size=None) -> list[EmailAttachment]` | — (docling lists attachment names only) | the attachments of an `.eml` / `.msg` (#561) — a path, the message bytes or a `DocumentStream` — as `EmailAttachment(index, name, content_type, format, size, inline, skipped, data)`: `data` the payload when kept (`None` over a limit or without a payload — a reference, an OLE object), `format` the `InputFormat` it converts as (`None` with `skipped` saying why: unsupported type, nested archive, …; the bytes stay available), `name` a safe base name, a forwarded message an `.eml` entry carrying the nested message. `converter.convert(att.as_stream())` converts one. Limits are byte counts (defaults 10 000 attachments, 256 MiB each, 1 GiB in all). |
| `docling_rs.download_models()` | `docling-tools models download` | idempotent; `~/.cache/docling.rs` or `$DOCLING_RS_CACHE_DIR`; INT8 models fetched when hosted and preferred automatically (`DOCLING_RS_FP32=1` opts out); `force=True` re-downloads a stale cache after a model re-publish; `asr_model="parakeet_tdt_0.6b_v3"` (or a list, or `"whisper_tiny"` / the Whisper presets — `docling_rs.models.ASR_MODELS`) adds speech-recognition models for audio/video, none by default; `pdf_models=False` skips the PDF/image models |

Model/env resolution order: explicit `DOCLING_*` env vars → the process CWD
(`models/`, matching the CLI — so a repo checkout uses its own exports) →
the cache dir set by `ensure_env()` (called by the constructor). PDF pages
are parsed and rendered in pure Rust — no native PDF library is fetched.

## Configuration (docling-shaped)

`docling_rs` re-exports docling-shaped config objects — same names and fields, so
docling code reads unchanged:

```python
from docling_rs import DocumentConverter, InputFormat, PdfFormatOption, PdfPipelineOptions, AcceleratorOptions

opts = PdfPipelineOptions(
    do_ocr=False,                                   # skip OCR on scanned pages
    do_table_structure=True,                        # TableFormer table recovery
    accelerator_options=AcceleratorOptions(num_threads=4),
)
conv = DocumentConverter(format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)})
# shorthand: DocumentConverter(do_ocr=False, do_table_structure=True)
```

`document_timeout=90.0` (docling's `PipelineOptions.document_timeout`, #497;
also accepted as `pipeline_options.document_timeout`) is a per-document
budget in seconds for the PDF pipeline, checked between pages: once spent, the
pages done so far are the document, `result.status` is `PARTIAL_SUCCESS` and
`result.errors` holds the reason as docling's `ErrorItem`.

For scanned pages, `ocr_lang="en"|"ch"` picks the OCR recognition model (`en`
is the default — proper Latin word spacing; `ch` is the multilingual
docling-conformance model). BCP-47 tags for either language resolve to the
same two models — `"en-US"`, `"eng"`, `"zh"`, `"zh-Hans"`, `"zh-TW"`, with or
without docling's `iso:` prefix (#388; script and region subtags are ignored,
other languages raise `ValueError`). docling-shaped `ocr_options.lang` lists
map onto the same switch the same way (`["english"]` → `en`, `["chinese"]` /
`["iso:zh-Hans"]` → `ch`; the first entry wins).

The Rust engine acts on `do_ocr`, `do_table_structure`, the opt-in enrichment
flags `do_picture_classification` / `do_code_enrichment` /
`do_formula_enrichment` (the picture classifier is fetched by the default
`scripts/install/download_dependencies.sh` run; the code/formula models need
its `--enrich` flag), `do_picture_ocr` (#645: OCR the pictures embedded in
DOCX/PPTX/HTML/… and video frames with the engine's OCR models — the text
lands in `picture.meta.description` + the `description` annotation, exactly
where `do_picture_description` puts a VLM's; `picture_ocr_classes` (a list
or comma-separated string of DocumentFigureClassifier labels) and
`picture_ocr_min_side` filter the pictures, `keep_picture_images=False`
drops the image bytes afterwards), and
its `--enrich` flag), `redact_pii` (#621, a docling.rs extension: personal
data — e-mail, phone, card numbers, IBANs, IPs, URL credentials, national
IDs, and names / organizations / locations with the NER model under
`.models/ner/` — is replaced in the document model before export, so
`export_to_markdown()`, `export_to_dict()` and the chunkers all see the
redacted text; `redact_mode` is `label` | `pseudonym` | `fixed:<text>`,
`redact_kinds` a list or comma-separated string, `redact_pattern` a list or
newline-joined string of `NAME=REGEX`, `redact_images` `drop` | `box_out` |
`keep`; `result.redaction` is the counts per label), and
`accelerator_options.num_threads` (→ ONNX Runtime intra-op threads via
`DOCLING_RS_PDF_THREADS`), and the image outputs (#520):
`generate_page_images` keeps each page's render as `document.pages[n].image`
(so docling-core's `TableItem.get_image(doc)` works), and `images_scale` sets
the picture-crop / page-image resolution once `generate_picture_images` or
`generate_page_images` is on — docling renders images only then; picture
crops are always extracted here, at the engine's 2.0 px/pt otherwise. Every
picture's `image.dpi` is 72·scale (#519). `convert(source, page_range=(a, b))`
narrows a PDF to that page window per call, as in docling (#518). The
remaining `PdfPipelineOptions` fields (`table_structure_options.mode`, …) are
accepted for API compatibility but do not change the pipeline. `InputFormat`,
`DocumentStream` and `ImageRefMode` are re-exported too (the last straight from
`docling_core`, for `export_to_markdown(image_mode=…)`). A GPU
`accelerator_options.device` (`CUDA`/`MPS`) on a wheel without that provider
warns and falls back to CPU: the prebuilt PyPI wheels ship ONNX Runtime with
the CPU execution provider only (`docling-rs-cuda` adds CUDA). The engine
itself supports CUDA / TensorRT / DirectML / CoreML behind cargo features
(issue #74) — build the wheel from source with e.g. `maturin build --features
cuda` (or `--features coreml` on macOS) and select the provider per process
with `DOCLING_RS_EP=cuda` (see the workspace README). CoreML is opt-in (#602):
a CoreML wheel converts on CPU until `DOCLING_RS_EP=coreml` is set or
`accelerator_options.device` is `MPS`, which maps to it (an explicit
`DOCLING_RS_EP` still wins). Under a GPU provider the engine picks the fp32 models over the int8
ones on its own: `ensure_env()` hands the model cache over as
`DOCLING_RS_MODELS_DIR` and pins no file (#602).

## Chunking

`docling_rs.chunking` ships the **Rust-native** ports of docling's chunkers
(`docling::chunker`), API-shaped like `docling.chunking`:

```python
from docling_rs import DocumentConverter
from docling_rs.chunking import HierarchicalChunker, HybridChunker, WindowChunker

doc = DocumentConverter().convert("report.docx").document

for chunk in HierarchicalChunker().chunk(doc):        # structure-driven
    print(chunk.meta.headings, chunk.text)

chunker = HybridChunker(tokenizer="tokenizer.json", max_tokens=256)
for chunk in chunker.chunk(doc):                       # tokenization-aware
    embed_me = chunker.contextualize(chunk)            # heading path + text

chunker = WindowChunker(max_words=300, overlap=0.05)   # word-window, no tokenizer
for chunk in chunker.chunk(doc):                       # docling-rag's window chunker
    embed_me = chunker.contextualize(chunk)            # '# path' line + body
```

`WindowChunker` is **docling-rag's window chunker**: the document's Markdown is
cut into heading-bounded sections of plain words (markup stripped), and a
fixed window of `max_words` words (default 300) slides over each section with
`overlap` fractional overlap (default 0.05 = 5%). A chunk never crosses a
heading, `chunk.meta.headings` carries the heading path, and
`contextualize(chunk)` renders rag-style — a `# Outer > Inner` context line, a
blank line, then the body. No tokenizer and no ML models are involved, making
it the zero-dependency choice when an approximate chunk size is enough
(`meta.doc_items` is empty — it works on the rendered Markdown, not the
document tree).

Two deltas from docling: `HybridChunker(tokenizer=...)` takes a **path to a
HuggingFace `tokenizer.json`** (loaded natively — no `transformers` install),
and `chunk.meta.doc_items` holds the items' JSON-pointer refs (`"#/texts/12"`)
rather than the item objects — refs into *your* document: the engine chunks a
re-imported copy whose numbering drifts (empty items dropped, HTML/DOCX item
trees, inline runs), so each chunk item is mapped back by kind, reading order
and text; an item that cannot be placed is left out rather than mis-pointed.
`chunk.meta.export_json_dict()` gives docling's `DocMeta` JSON shape. With no
`tokenizer` argument it falls back to MiniLM's tokenizer at
`models/chunk/tokenizer.json` (the download script's location) or the package
cache — `docling_rs.download_models()` fetches it with the other assets. Since
`result.document` is a genuine `docling_core` `DoclingDocument`, docling's own
Python chunkers (`pip install "docling-core[chunking]"`) also keep working on
it — the native classes are the faster, dependency-free path.

### Streaming

`chunk()` **streams natively**: it returns a lazy iterator fed by a Rust
background thread, which hands each chunk to Python as the chunkers produce
it. The full chunk list is never materialized on either side of the FFI
boundary — the first chunk is ready for embedding while the rest of the
document is still being chunked, and a slow consumer throttles the producer
through a bounded queue instead of buffering unboundedly.

```python
from itertools import islice

from docling_rs import DocumentConverter
from docling_rs.chunking import HybridChunker

doc = DocumentConverter().convert("large.html").document
chunker = HybridChunker(tokenizer="tokenizer.json", max_tokens=512)

# Chunks arrive one by one; embed each as soon as it is produced.
for chunk in chunker.chunk(doc):
    index.add(embed(chunker.contextualize(chunk)))

# Laziness composes: this chunks only far enough to produce 10 chunks.
preview = list(islice(chunker.chunk(doc), 10))
```

Abandoning the iterator early (`break`, `islice`, dropping the generator)
cancels the background chunking, and Ctrl-C interrupts a pending `next()`.
Errors (a bad tokenizer path, malformed document JSON) surface on the first
`next()`, not at `chunk()` call time.

## LangChain

`docling_rs.langchain` is the port of docling's
[langchain-docling](https://github.com/docling-project/docling-langchain)
integration — same classes, same parameters, same output — running on the
Rust engine (no PyTorch):

```bash
pip install "docling-rs[langchain]"
```

```python
# was:  from langchain_docling import DoclingLoader
from docling_rs.langchain import DoclingLoader, ExportType

docs = DoclingLoader(file_path=["https://arxiv.org/pdf/2408.09869", "notes.docx"]).load()
docs[0].page_content                      # heading path + chunk text, ready to embed
docs[0].metadata["source"]                # the input
docs[0].metadata["dl_meta"]               # docling's chunk meta: items + page/bbox provenance, headings, origin

DoclingLoader(file_path="report.pdf", export_type=ExportType.MARKDOWN).load()  # one Document per input
```

| Parameter | Default | |
|---|---|---|
| `file_path` | — | path or URL, or an iterable of them |
| `export_type` | `ExportType.DOC_CHUNKS` | one LangChain `Document` per chunk; `ExportType.MARKDOWN` = one per input |
| `chunker` | `docling_rs.chunking.HybridChunker()` | any chunker with `chunk()` / `contextualize()` — the native ones or docling's own |
| `converter` | `docling_rs.DocumentConverter()` | configure it as usual (`do_ocr=False`, `ocr_lang=…`, …); docling's `DocumentConverter` / `DoclingServiceClient` work too |
| `convert_kwargs` | `{}` | extra kwargs for `converter.convert(source=…)` |
| `md_export_kwargs` | `{"image_placeholder": ""}` | kwargs for `export_to_markdown()` (Markdown mode) |
| `meta_extractor` | `MetaExtractor()` | subclass `BaseMetaExtractor` to shape `metadata` |

It reproduces langchain-docling's own test expectations exactly (its
fixtures run in `tests/test_langchain.py`). The default chunker is the native
hybrid chunker with docling's defaults (all-MiniLM-L6-v2 tokenizer, 256
tokens); its `tokenizer.json` (~0.5 MB) is fetched into the model cache on
the first load when not already there — docling likewise pulls it from the
Hugging Face Hub. `lazy_load()` converts and chunks one input at a time.

**Picture descriptions with any LangChain chat model** — langchain-docling's
`PictureDescriptionLangChainOptions`, set on the pipeline options as in
docling (`do_picture_description`; `allow_external_plugins` is accepted but
not needed):

```python
from langchain_openai import ChatOpenAI
from docling_rs import DocumentConverter, InputFormat, PdfFormatOption, PdfPipelineOptions
from docling_rs.langchain import PictureDescriptionLangChainOptions

opts = PdfPipelineOptions(do_picture_description=True)
opts.picture_description_options = PictureDescriptionLangChainOptions(
    llm=ChatOpenAI(model="gpt-4o-mini"), prompt="Describe the image in three sentences.", provenance="gpt-4o-mini"
)
doc = DocumentConverter(format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)}).convert("paper.pdf").document
[p.meta.description.text for p in doc.pictures if p.meta and p.meta.description]
```

The engine already embeds every picture's crop, so the description pass runs
on the converted document (`docling_rs.picture_description`, docling's
selection rules: pictures under 5% of their page skipped,
`classification_allow` / `classification_deny` filters, batches of 8) and
applies to every input format the converter handles, not only PDF.

Runnable examples — loader basics, an agentic RAG with page citations, picture
descriptions — are in [`examples/langchain/`](./examples/langchain/).

## Not covered (yet)

The *local in-process* full-VLM conversion pipeline (SmolDocling) and
per-format *backend* selection. The **remote** VLM pipeline (#304) *is*
covered: `DocumentConverter(pipeline="vlm", vlm_endpoint=…, vlm_model=…)`
(plus optional `vlm_api_key` / `vlm_prompt` / `vlm_max_tokens`, all falling
back to the `DOCLING_RS_VLM_*` environment) sends each PDF page / image to
any OpenAI-compatible vision endpoint instead of running the local ML stack —
no models needed; a bad configuration raises `ValueError` at construction and
a failed conversion raises the catchable `ConversionError`. GPU inference is engine-side only: compiled in via cargo features
(#74) and selected with `DOCLING_RS_EP`, not via `accelerator_options.device`,
and absent from the prebuilt CPU wheels. The document carries rendered text for
inline formatting rather than structured `formatting` fields — see
`docs/MIGRATION.md` §4 for the documented divergences.

## Publishing

Releases are **automatic**, mirroring the npm package: every GitHub Release
CI cuts (`v<version>`, see the repo's release flow) triggers the
[`pypi-publish`](../../.github/workflows/pypi-publish.yml) GitHub Actions
workflow, which builds an `abi3` wheel per platform (Linux x86-64/arm64 as
`manylinux_2_28`, Linux s390x cross-compiled with maturin — it loads ONNX
Runtime at run time, see the repo README's IBM Z section — and Windows x86-64;
one wheel covers every Python ≥ 3.9) plus an
sdist and the `docling-rs-cuda` wheel, and uploads them to PyPI at the release's
version. It can also be run by hand (`workflow_dispatch`) — to re-publish a
version an automatic run skipped or failed on, or to publish a branch build;
files already on PyPI are skipped, so re-runs are safe:

```bash
# From the Actions tab, or:
gh workflow run pypi-publish.yml                 # version from the workspace Cargo.toml
gh workflow run pypi-publish.yml -f tag=v0.16.0 -f cuda_wheel=true
```

No secrets: it publishes via PyPI **Trusted Publishing** (OIDC), like
docling-core — no API token is stored or rotated (the trusted publisher is
registered on PyPI; manage it at *Project → Manage → Publishing*). Re-runs are
idempotent (`skip-existing`). macOS wheels are omitted (no hosted runners here);
macOS users install the sdist, which compiles from source. The ONNX runtime is
bundled in the wheel; the ONNX models are fetched at runtime by `download_models()`.

### GPU wheel: `docling-rs-cuda`

The workflow's `cuda_wheel` input additionally builds a **Linux x86_64** wheel
published as **`docling-rs-cuda`**: the same crate compiled with
`--features cuda`, ONNX Runtime's CUDA provider libraries bundled next to the
native module (found via an `$ORIGIN` rpath — no import-time preload).
It installs the same `docling_rs` module — install *either* `docling-rs` *or*
`docling-rs-cuda`, not both:

```bash
pip install docling-rs-cuda
python -c "import docling_rs; ..."   # GPU used automatically when present
```

The GPU wheel defaults to `auto`: it converts on the GPU when one is usable
and falls back to CPU when not — no environment setup needed.
`DOCLING_RS_EP=cpu` (or `AcceleratorOptions(device="cpu")`) forces CPU;
`DOCLING_RS_EP=cuda` / `device="cuda"` pins the GPU and fails loudly if it
can't initialize. The fp32 models are preferred automatically on GPU, and
**CUDA 12 + cuDNN 9 must be installed on the system** — the wheel ships the
ONNX Runtime provider, not the CUDA toolkit. The wheel is tagged **`manylinux_2_38`**
(glibc ≥ 2.38 at runtime, i.e. Ubuntu 24.04+ / Debian 13+): the CUDA ONNX
Runtime static binaries carry glibc-2.38 symbols, so this floor is inherent —
it is also why the CI job builds on plain `ubuntu-24.04` instead of the
manylinux_2_28 container the CPU wheels use (linking there fails on
`__isoc23_*`). Measured end-to-end on an RTX 3080 Laptop: 1.5–2.1× on
multi-page digital PDFs, 8.7× on a 1913-page manual (see
[`PDF_CONFORMANCE.md`](../../docs/PDF_CONFORMANCE.md#measured-on-real-hardware-issue-108)).

Local build mirroring the CI wheel (order matters — the provider libraries
must exist *and* sit inside `python/docling_rs/` before the wheel is
assembled, or the wheel silently ships without them):

```bash
cd crates/docling-py
export RUSTFLAGS='-C link-arg=-Wl,-rpath,$ORIGIN'
cargo build --release --features cuda            # ort fetches CUDA ONNX Runtime + drops the provider libs
cp target/release/libonnxruntime_providers_{shared,cuda}.so python/docling_rs/
maturin build --release --features cuda          # wheel now includes them (expect ~hundreds of MB)
```

PyPI setup (one-time): `docling-rs-cuda` is a separate PyPI project — register
the same workflow as a trusted publisher there too, and if the wheel exceeds
PyPI's default file-size limit, request a per-project bump (the
`onnxruntime-gpu` package is the precedent).

### Test the release build locally

Reproduce what CI does — build the wheel + sdist and verify both install and run
— before (or instead of) triggering the workflow. Needs a Rust toolchain and
Python ≥ 3.9.

```bash
cd crates/docling-py
python -m venv .venv && source .venv/bin/activate
pip install maturin

# 1. Build the same two artifacts the workflow builds.
maturin build --release --out dist      # dist/docling_rs-<v>-cp39-abi3-<platform>.whl
maturin sdist            --out dist      # dist/docling_rs-<v>.tar.gz  (vendors all crates)

# 2. Smoke-test the WHEEL in a clean env — pip pulls docling-core from the
#    wheel's declared dependency, exactly as an end user would get it.
python -m venv /tmp/wheel-test
/tmp/wheel-test/bin/pip install dist/docling_rs-*.whl
/tmp/wheel-test/bin/python - <<'PY'
from docling_rs import DocumentConverter
r = DocumentConverter().convert("../../tests/data/html/sources/hyperlink_03.html")
assert r.status == "success"
assert type(r.document).__module__.startswith("docling_core")   # the real DoclingDocument
print("wheel OK:", len(r.document.export_to_markdown()), "md chars")
PY

# 3. Verify the SDIST is self-contained: pip compiles the Rust engine from source
#    (this is the exact unpack-and-build path cibuildwheel runs in the manylinux
#    containers, so a green result here means the CI wheel build will work too).
python -m venv /tmp/sdist-test
/tmp/sdist-test/bin/pip install dist/docling_rs-*.tar.gz
/tmp/sdist-test/bin/python -c "import docling_rs; print('sdist build OK')"

# 4. Run the declarative-path test suite (no ML models needed).
pip install pytest docling-core
pytest tests/
```

To exercise the full manylinux wheel build (what `pypa/cibuildwheel` runs) you
need Docker; with a daemon available:

```bash
pipx run cibuildwheel==2.21.3 --platform linux --output-dir wheelhouse .
# env: CIBW_BUILD=cp39-* CIBW_SKIP=*-musllinux*  CIBW_BEFORE_ALL_LINUX="curl … rustup … -y"
```

An optional final rehearsal uploads to **TestPyPI** (needs a TestPyPI token or a
pending publisher there): `pip install twine && twine upload --repository testpypi dist/*`.
