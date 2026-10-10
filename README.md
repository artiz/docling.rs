# docling.rs

<p align="center">
  <img src="https://raw.githubusercontent.com/docling-project/docling.rs/refs/heads/master/docs/assets/logo.svg" alt="docling.rs — a duck feeding a document into a meat grinder" width="240">
</p>

<p align="center">
  <a href="https://github.com/docling-project/docling.rs/actions/workflows/ci.yml"><img src="https://github.com/docling-project/docling.rs/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://docling-project.github.io/docling.rs/"><img src="https://img.shields.io/badge/demo-live-brightgreen" alt="Live browser demo"></a>
  <a href="https://docs.rs/docling"><img src="https://img.shields.io/docsrs/docling?logo=docs.rs" alt="docs.rs"></a>
  <a href="https://pypi.org/project/docling-rs/"><img src="https://img.shields.io/pypi/pyversions/docling-rs" alt="Python versions"></a>
  <a href="https://crates.io/crates/docling"><img src="https://img.shields.io/crates/msrv/docling?logo=rust&label=rust" alt="Rust MSRV"></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/github/license/docling-project/docling.rs" alt="License MIT"></a>
  <a href="https://lfaidata.foundation/projects/"><img src="https://img.shields.io/badge/LF%20AI%20%26%20Data-003778?logo=linuxfoundation&logoColor=fff&color=0094ff&labelColor=003778" alt="LF AI &amp; Data"></a>
  <br>
  <a href="https://crates.io/crates/docling"><img src="https://img.shields.io/crates/v/docling?logo=rust" alt="crates.io version"></a>
  <a href="https://pypi.org/project/docling-rs/"><img src="https://img.shields.io/pypi/v/docling-rs?logo=pypi&logoColor=fff" alt="PyPI version"></a>
  <a href="https://www.npmjs.com/package/docling.rs"><img src="https://img.shields.io/npm/v/docling.rs?logo=npm&logoColor=fff&label=npm%20docling.rs" alt="npm docling.rs"></a>
  <a href="https://www.npmjs.com/package/docling.rs-wasm"><img src="https://img.shields.io/npm/v/docling.rs-wasm?logo=webassembly&logoColor=fff&label=npm%20docling.rs-wasm" alt="npm docling.rs-wasm"></a>
  <a href="https://github.com/docling-project/docling.rs/releases"><img src="https://img.shields.io/github/v/release/docling-project/docling.rs?label=release&logo=github" alt="GitHub release"></a>
  <br>
  <a href="https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-serve"><img src="https://img.shields.io/badge/ghcr.io-docling--rs--serve-2496ED?logo=docker&logoColor=fff" alt="GHCR docling-rs-serve"></a>
  <a href="https://github.com/docling-project/docling.rs/pkgs/container/docling-rs"><img src="https://img.shields.io/badge/ghcr.io-docling--rs-2496ED?logo=docker&logoColor=fff" alt="GHCR docling-rs (CLI)"></a>
  <a href="https://crates.io/crates/docling"><img src="https://img.shields.io/crates/d/docling?label=crates.io%20downloads" alt="crates.io downloads"></a>
  <a href="https://pepy.tech/projects/docling-rs"><img src="https://static.pepy.tech/badge/docling-rs/month" alt="PyPI downloads/month"></a>
  <a href="https://www.npmjs.com/package/docling.rs"><img src="https://img.shields.io/npm/dm/docling.rs?label=npm%20downloads" alt="npm downloads/month"></a>
  
</p>

A Rust port of [docling](https://github.com/docling-project/docling): convert
documents into a unified `DoclingDocument` for downstream AI workflows.

**Fast and small:** one static binary, no Python/PyTorch at runtime. The PDF
ML pipeline runs **4.3× faster** than Python docling at **2.3–2.6× less peak
RAM** (methodology, profiling and per-fixture numbers in
[`docs/PDF_CONFORMANCE.md` § Performance](./docs/PDF_CONFORMANCE.md#performance--review--profiling-notes));
declarative formats (DOCX/HTML/XLSX/…) convert **20–60× faster** at
**~60× less memory** (measured with `scripts/test/performance.sh`, Python
docling vs Rust on the same file). Output is checked against upstream for
every format — see [Conformance with Python docling](#conformance-with-python-docling).

The format migration is **complete** — every document format in docling's
pipeline is supported, validated byte-for-byte against live docling. See
[`docs/MIGRATION.md`](./docs/MIGRATION.md) for the full architecture, the Python → Rust
mapping, and per-format conformance.

**▶ [Try it in your browser](https://docling-project.github.io/docling.rs/)** —
the whole converter compiled to wasm: drop a DOCX, PDF, XLSX, EPUB … and get
Markdown, docling JSON, DocLang XML, LaTeX or a Pandoc AST back. Nothing is uploaded; the page runs
entirely on your device, phone included. Scanned pages can be OCR'd there too
(layout + PP-OCR + TableFormer via ONNX Runtime Web) once you point it at the
models. See [`crates/docling-wasm`](./crates/docling-wasm/README.md).

Developed with **Claude Code** and _[TENET](https://github.com/artiz/tenet/tree/master)_ (minimalistic AI-driven development framework).

## Status

Every format in docling's pipeline converts — plus a few docling lacks — to
Markdown, docling JSON, plain text, DocLang `.dclx`, LaTeX, HTML, Pandoc AST,
WebVTT and chunk records, with image extraction. The extension map
(`InputFormat::from_extension`, docling's `FormatToExtensions`) picks the
backend; a file that fails under its extension is checked by content and,
when it is evidently another format (an RTF or HTML file saved as `.doc`),
converted once more as that format with a warning (#556).

| Category | Extensions | Notes |
|---|---|---|
| Text & markup | `.md` `.markdown` `.txt` `.text` `.qmd` `.rmd` · AsciiDoc `.adoc` `.asciidoc` `.asc` · HTML `.html` `.htm` `.xhtml` · MHTML `.mhtml` `.mht` · LaTeX `.tex` `.latex` | text encodings detected like docling's `decode_text`, or set with `encoding`; HTML JSON is docling's item tree; MHTML is parsed as MIME and routed through the HTML backend; LaTeX is docling's backend on a pylatexenc port — multi-file arXiv projects, math, `\newcommand`, `tabular`, figures |
| Word processing | DOCX `.docx` `.docm` `.dotx` `.dotm` · Word 97–2004 `.doc` `.dot` (Word 6/95 and Word for Windows 1.x/2.0 too) · OpenDocument `.odt` `.ott` `.fodt` · OpenOffice 1.x `.sxw` `.stw` `.sxg` · StarWriter `.sdw` `.vor` · AbiWord `.abw` `.zabw` `.awt` · WordPerfect `.wpd` `.wp` `.wp5` `.wp6` `.wpt` · MS Works `.wps` · EPUB `.epub` · RTF `.rtf` | DOCX JSON is docling's item tree; OMML equations come out as LaTeX; numbered headings keep their numbers |
| Presentations | PPTX `.pptx` `.pptm` `.potx` `.potm` `.ppsx` `.ppsm` · PowerPoint 97–2003 `.ppt` `.pot` `.pps` · OpenDocument `.odp` `.otp` `.fodp` · OpenOffice 1.x `.sxi` `.sti` · StarImpress/StarDraw `.sdd` `.sda` | PPTX JSON is docling's item tree — slides, lists, tables, pictures, charts, notes, comments |
| Diagrams | Visio `.vsdx` `.vsdm` · SVG `.svg` | Visio: pages as sections, shape text in reading order, connectors as a relations table; SVG rasterized into the image pipeline, or its `<text>` as paragraphs without ML |
| Spreadsheets | XLSX `.xlsx` `.xlsm` `.xltx` `.xltm` · XLSB `.xlsb` · Excel 97–2004 `.xls` `.xlt` · OpenDocument `.ods` `.ots` `.fods` · OpenOffice 1.x `.sxc` `.stc` · CSV `.csv` `.tsv` · dBase `.dbf` · DIF `.dif` · SYLK `.slk` `.sylk` · Lotus 1-2-3 / Symphony `.wk1` `.wk2` `.wk3` `.wk4` `.wks` `.wrk` `.123` · Quattro Pro `.wq1` `.wq2` `.wb1` `.wb2` `.wb3` `.qpw` · MS Works `.xlr` | cells print what Excel displays — `$12.50`, `10%`, `Feb-25` (#634); merged cells keep their spans; chart sheets expose their chart |
| Apple iWork | Pages `.pages` · Numbers `.numbers` · Keynote `.key` | both container generations (`Index/*.iwa` and iWork '09 XML); Pages and Keynote mirror docling's readers, Keynote charts as classified pictures with their data table; Numbers as sheet/table text (#213) |
| XML dialects | JATS / USPTO / XBRL (`.xml` `.nxml`, content-sniffed) · DocLang `.dclg` | XBRL converts without arelle: `dei` title, text blocks, numeric facts as docling's key-value graph, the taxonomy read offline from `xbrl_taxonomy` |
| PDF & images | `.pdf` · `.png` `.jpg` `.jpeg` `.tif` `.tiff` `.bmp` `.webp` `.gif` · HEIC/HEIF `.heic` `.heif` (`--features heif`) · METS/GBS scan packages `.tar.gz` · DjVu `.djvu` `.djv` | pure-Rust text layer, page renderer and scan raster, ONNX layout + TableFormer + OCR (`docling-pdf`; no native PDF library in the default build, `pdfium` is an opt-in feature — [`docs/PDF_CONFORMANCE.md`](./docs/PDF_CONFORMANCE.md)); JPEG 2000 decodes in Rust, JBIG2 is a placeholder; DjVu reads its hidden text layer without models and falls back to OCR with them (#434) |
| docling native | docling JSON `.json` · DocTags `.doctags` `.dt` · DCLX `.dclx` | DocTags — the token markup docling's VLMs emit — read through docling-core's tolerant parser, the one the VLM pipeline uses (#152) |
| Mainframe data | EBCDIC `.ebc` `.ebcdic` | fixed-width records decoded through a COBOL copybook (docling's `EbcdicLayout`): `ebcdic_layout`, or a `<stem>.layout.json` sidecar |
| Email & subtitles | `.eml` · Outlook `.msg` · WebVTT `.vtt` | `.msg` is projected onto RFC 822 (same output as the `.eml`); `list_attachments` names the attachments, `email_attachments()` converts them (#561); `cid:` images embed under `image_sources=embedded` (#646) |
| Audio | `.wav` `.mp3` `.mpga` `.m4a` `.aac` `.ogg` `.flac` | Rust all the way down: symphonia decodes, Whisper tiny (ONNX, docling's ASR defaults) or the Parakeet TDT 0.6B v3 preset (#508) transcribes; `[time: start-end] text` paragraphs, the timing as the JSON item's track, `--to vtt` for subtitles; language auto-detected or `asr_lang`. Ogg Opus falls back to the `ffmpeg` binary |
| Video | `.mp4` `.avi` `.mov` `.mkv` `.webm` `.mpeg` `.mpg` | the audio track transcribed the same way; with the `ffmpeg` binary on `PATH` (`DOCLING_FFMPEG`), `video_frames` scene-change frames (8 by default, `all` for every cut) interleave as `[time: <ts>]` pictures, tuned by the `video_*` options (#647, #648). AVI needs ffmpeg; without it a video is its transcript |

<details>
<summary><b>Installing ffmpeg</b> (optional — only for video frame sampling)</summary>

Any ffmpeg ≥ 4.x on `PATH` works; docling.rs shells out to the binary and
parses its output, so no dev headers/libraries are needed.

- **Debian/Ubuntu**: `sudo apt-get install ffmpeg`
- **Fedora**: `sudo dnf install ffmpeg-free` (or `ffmpeg` from RPM Fusion)
- **Alpine**: `apk add ffmpeg`
- **macOS**: `brew install ffmpeg`
- **Windows**: `winget install ffmpeg` (or `choco install ffmpeg`, or
  `scoop install ffmpeg`). Installing from a downloaded zip
  ([gyan.dev](https://www.gyan.dev/ffmpeg/builds/) /
  [BtbN](https://github.com/BtbN/FFmpeg-Builds/releases)) also works — either
  add the extracted `bin\` folder to `PATH`, or skip `PATH` entirely and point
  `DOCLING_FFMPEG` at the exe:
  `set DOCLING_FFMPEG=C:\tools\ffmpeg\bin\ffmpeg.exe`

Check with `ffmpeg -version`. `DOCLING_FFMPEG` overrides the binary used on
any OS; the docling-rs-serve Docker image ships ffmpeg preinstalled.
</details>

## Conformance with Python docling

Every output is checked against upstream Python docling. Declarative formats
are compared byte-for-byte against the live library; the PDF/image ML pipeline
is pinned by a deterministic snapshot baseline and scored against docling's
groundtruth in [`docs/PDF_CONFORMANCE.md`](./docs/PDF_CONFORMANCE.md).

Latest full sweep of the declarative corpus, docling **2.129.0**
(`scripts/conformance/full_conformance.py`: Markdown byte-for-byte, JSON
structurally — every key and value but `origin`/`version` and re-encoded image
bytes; ✅ all = every file of the format matches):

| Format | Files | Markdown exact | JSON identical |
|---|---|---|---|
| DOCX | 38 | ✅ all | ✅ all |
| HTML | 32 | ✅ all | 31 |
| PPTX | 8 | ✅ all | ✅ all |
| XLSX | 14 | 13 ² | 13 ² |
| ODF | 7 | ✅ all | ✅ all |
| CSV | 9 | ✅ all | 6 |
| Markdown | 11 | ✅ all | 6 |
| DeepSeek-OCR Markdown | 3 | ✅ all | n/a |
| WebVTT | 4 | ✅ all | ✅ all |
| Email (`.eml`) | 2 | ✅ all | ✅ all |
| iWork Pages | 1 | ✅ all | ✅ all |
| iWork Keynote | 5 | ✅ all | ✅ all |
| EBCDIC | 3 | ✅ all | ✅ all |
| JATS | 7 | ✅ all | ✅ all |
| DocLang | 15 | ✅ all | ✅ all |
| AsciiDoc | 5 | ✅ all | ✅ all |
| USPTO | 9 | ✅ all | ✅ all |
| EPUB | 1 | ✅ all | ✅ all |
| LaTeX | 8 | ✅ all | 7 ¹ |

¹ `1706.03762`: one `tabular`'s cells carry consistent `row_span`/`col_span` where upstream writes the span into the offsets only; PDF figures carry no image payload (upstream renders them with pypdfium2).
² `xlsx_02_sample_sales_data`: 20 date cells print by docling PR #4628's number-format rules (#634), which the released docling does not apply yet.

The JSON column is complete wherever the backend builds docling's item tree
(HTML, DOCX, PPTX, ODF, WebVTT, JATS, AsciiDoc, DocLang, LaTeX, USPTO,
Markdown without raw HTML blocks) or its flat export already has upstream's
shape (XLSX, CSV, EBCDIC); the partial columns are listed with their exact
files in [`docs/MIGRATION.md`](./docs/MIGRATION.md). Markdown is exact on
every file upstream itself converts (the tenth USPTO fixture,
`tables_ipa20180000016.xml`, fails in upstream). Per-format residuals, with the exact files:
[`docs/MIGRATION.md`](./docs/MIGRATION.md).

## Testing

All commands run from the repo workspace root.

```bash
# everything — unit tests + the output-regression suite (pure Rust; no Python/models)
cargo test

# just the regression suite: re-convert every covered source — the upstream
# fixtures each crates/docling/tests/data/<fmt>/mirror.txt lists from the root
# tests/data/<fmt>/sources/ corpus, plus our own under
# crates/docling/tests/data/<fmt>/sources/ — and assert that legacy Markdown,
# strict Markdown, docling JSON and LaTeX match the committed fixtures
cargo test -p docling --test regression

# refresh the fixtures after an *intentional* output change, then review `git diff`
DOCLING_RS_REGEN=1 cargo test -p docling --test regression

# a single crate / a single test (with output)
cargo test -p docling-core
cargo test outputs_match_fixtures -- --nocapture
```

The ML formats (PDF, images, METS) need the ONNX models, so they are
covered by a separate **deterministic snapshot** harness rather than `cargo test`:

```bash
bash scripts/install/pdf_setup.sh           # one-time: export the ONNX models
                                    # (layout + TableFormer; needs a torch/docling Python)
# Updating an existing checkout after a model-format change (e.g. the cached
# TableFormer decoder): `rm -rf .models/tableformer && bash scripts/install/pdf_setup.sh`,
# or re-run `python scripts/install/export_tableformer.py .models/tableformer` directly.

export DOCLING_LAYOUT_ONNX="$(pwd)/models/layout_heron.onnx"
export DOCLING_OCR_REC_ONNX="$(pwd)/models/ocr_rec.onnx"
export DOCLING_OCR_DICT="$(pwd)/models/ppocr_keys_v1.txt"
# Optional (falls back to geometric table reconstruction if unset/missing —
# but the fallback is *silent*, so set these to be sure TableFormer is used,
# especially if you invoke docling.rs from anywhere but the repo root: the
# defaults baked into the binary are relative paths, so a different working
# directory makes them silently miss even when the files exist elsewhere).
export DOCLING_TABLEFORMER_ENCODER="$(pwd)/models/tableformer/encoder.onnx"
export DOCLING_TABLEFORMER_DECODER="$(pwd)/models/tableformer/decoder.onnx"
export DOCLING_TABLEFORMER_BBOX="$(pwd)/models/tableformer/bbox.onnx"
bash scripts/conformance/pdf_conformance.sh     # regenerate + diff the snapshot baseline (94 outputs)
```

## Try it

```bash
# convert a file from the CLI — Markdown to stdout (add --strict for cleaner MD)
cargo run -p docling-cli -- crates/docling/sample.html
cargo run -p docling-cli -- --strict crates/docling/sample.html

# emit docling's native DoclingDocument JSON instead (--to md is the default)
cargo run -p docling-cli -- --to json crates/docling/sample.html
cargo run -p docling-cli -- --to json crates/docling/sample.html > out.json

# PDF/image conversion needs the ML models — see "Getting the ML models" below.
scripts/install/download_dependencies.sh
cargo run -p docling-cli -- document.pdf

# transcribe audio (wav/mp3/flac/ogg/aac/m4a, or an mp4/mov audio track) — the
# Whisper models come from the same download script
cargo run -p docling-cli -- recording.mp3
# …with a named preset (fetch it first: download_dependencies.sh --asr-model=whisper_tiny_en)
cargo run -p docling-cli -- --asr-model whisper_tiny_en recording.mp3

# extract pictures: embed as data URIs, or write ./artifacts/*.png — for any
# input that carries images, docling-JSON included (a `data:` URI or a
# referenced file next to the JSON is read back, #403)
cargo run -p docling-cli -- --images embedded   document.pdf
cargo run -p docling-cli -- --images referenced document.pdf > out.md
cargo run -p docling-cli -- --images referenced document.json > out.md

# stream Markdown to stdout page by page (the CLI's default; --no-stream to buffer)
cargo run -p docling-cli -- document.pdf
cargo run -p docling-cli -- --no-stream document.pdf

# or via the examples
cargo run -p docling --example convert -- crates/docling/sample.md
cargo run -p docling --example stream  -- crates/docling/sample.md

# score HTML output against the latest published docling (installed from PyPI)
scripts/conformance/conformance.sh html

# the full declarative sweep behind the "Conformance with Python docling" table
# (Markdown byte-for-byte + JSON structural, every format or the ones you name)
.venv-compare/bin/python scripts/conformance/full_conformance.py [docx html …]

# diff Python docling vs Rust on one file (installs published docling from PyPI)
scripts/conformance/compare.sh tests/data/html/sources/example_03.html

# benchmark time / CPU / memory: Python docling vs Rust
scripts/test/performance.sh tests/data/html/sources/wiki_duck.html 10
```

The comparison scripts install the latest published Python `docling` from PyPI
into `.venv-compare` automatically on first run. See
[`docs/MIGRATION.md`](./docs/MIGRATION.md) (§7 “Testing” for the differential
and performance scripts, §9 for keeping up with upstream releases).

## Install locally / in CI (one-liner)

`scripts/install/install.sh` installs a self-contained tree — for a dev box or
a pipeline step:

```bash
curl -fsSL https://raw.githubusercontent.com/docling-project/docling.rs/master/scripts/install/install.sh | bash
docling-rs your.pdf > out.md
```

It grabs the **prebuilt CLI binary** from the latest
[GitHub Release](https://github.com/docling-project/docling.rs/releases)
(Linux x64/arm64; `DOCLING_RS_FROM_SOURCE=1` opts out) and only falls back to
building from source when no matching asset exists — in that case it checks
for a Rust toolchain (installs one via rustup if `cargo` is missing) and runs
`cargo build --release -p docling-cli`. Either way it installs the
binary + all models under `/usr/local/docling.rs`, symlinks
`/usr/local/bin/docling-rs`, and writes `/etc/profile.d/docling-rs.sh` with
the `DOCLING_*` exports. The env file is a convenience for other
consumers of the model tree — the CLI itself resolves `.models/`
**relative to its own (symlink-resolved) location**, so the
command works from any directory with no environment at all. ONNX Runtime is
statically linked; nothing else lands outside the prefix.

Knobs (env vars before the call): `DOCLING_RS_PREFIX` (default
`/usr/local/docling.rs`), `DOCLING_RS_BIN_DIR`, `DOCLING_RS_REF` (git ref
to build), `DOCLING_RS_NO_ASR=1` (skip the ~150 MB Whisper models),
`DOCLING_RS_SUDO=0` (never escalate). Re-running is idempotent — it only
fetches missing model files. Uninstall:
`rm -rf /usr/local/docling.rs /usr/local/bin/docling-rs /etc/profile.d/docling-rs.sh`.

## Deploy in a container

### Container Images

The following container images are available on **GitHub Container Registry (GHCR)**, with all native dependencies, ffmpeg, and ONNX models baked in (zero Python runtime dependencies). Both are targets of the same Dockerfile and share their layers:

#### 📦 Distributed Images

| Image | Description | Architectures |
|---|---|---|
| [`ghcr.io/docling-project/docling-rs-serve`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-serve) | High-performance document conversion HTTP API with PDF, DOCX, PPTX, XLSX, HTML, images, and audio/video models pre-installed (CPU). | `linux/amd64`, `linux/arm64` |
| [`ghcr.io/docling-project/docling-rs`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs) | The `docling-rs` CLI with the same models baked in — batch conversion without installing Rust (CPU). Entrypoint `docling-rs`, working directory `/data`. | `linux/amd64`, `linux/arm64` |
| [`ghcr.io/docling-project/docling-rs-serve-cuda`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-serve-cuda) | NVIDIA CUDA 12 GPU-accelerated HTTP conversion API (CUDA 12 + cuDNN 9, Linux x86_64). Tagged like the CPU images: `latest` plus `v1.81.0`, `1.81`, `1`. | `linux/amd64` |
| [`ghcr.io/docling-project/docling-rs-cuda`](https://github.com/docling-project/docling.rs/pkgs/container/docling-rs-cuda) | NVIDIA CUDA 12 GPU-accelerated `docling-rs` CLI. Tagged like the CPU images: `latest` plus `v1.81.0`, `1.81`, `1`. | `linux/amd64` |

```bash
# Run docling-rs-serve HTTP API (CPU):
docker run -p 127.0.0.1:5001:5001 ghcr.io/docling-project/docling-rs-serve:latest

# Or run with NVIDIA GPU acceleration (--gpus all):
docker run --gpus all -p 127.0.0.1:5001:5001 ghcr.io/docling-project/docling-rs-serve-cuda:latest

# Convert a document:
curl -F file=@paper.pdf localhost:5001/v1/convert

# Or use the CLI image on local files (mounted at /data):
docker run --rm -v "$PWD:/data" ghcr.io/docling-project/docling-rs:latest paper.pdf --to md
docker run --gpus all --rm -v "$PWD:/data" ghcr.io/docling-project/docling-rs-cuda:latest paper.pdf --to md
```
### Docker Compose

Launch with [`examples/docker-compose/`](./examples/docker-compose/):

```bash
cd examples/docker-compose
docker compose up -d                        # standalone service (127.0.0.1:5001)
# or: docker compose -f docker-compose.caddy.yml up -d   # with Caddy TLS reverse proxy
```

### Core Container Configuration

| Variable / Option | Default | Description |
|---|---|---|
| `DOCLING_RS_NO_ARENA` | `1` | Disables ONNX Runtime CPU arena to prevent RSS heap ratcheting (#263) |
| `DOCLING_RS_SYSTEM_FONTS` | `0` | The PDF renderer's fallback fonts come only from `.models/fonts` + `DOCLING_RS_FONT_DIRS`, never the host's font directories, so the layout input is the same on every host (#633). Unset to search the host again |
| `DOCLING_RS_FONT_DIRS` | Liberation + DejaVu dirs | The font directories the image installs (`/usr/share/fonts/truetype/{liberation,dejavu}`); extend it to render other scripts with a font you add |
| `DOCLING_RS_MAX_MEMORY_MB` | `0` (or cgroup) | Memory ceiling (MiB); returns 503 + Retry-After when near watermark |
| `DOCLING_RS_MEMORY_WATERMARK_PCT` | `85` | Watermark % above which new requests get HTTP 503 |
| `DOCLING_RS_TF_INTRA` | auto (#262) | Narrows ONNX intra-op thread count for TableFormer decoder sessions |
| `DOCLING_RS_GRAPH_CACHE_DIR` | `$XDG_CACHE_HOME/docling-rs/graphs` (else `~/.cache/…`) | Where ONNX Runtime's optimized graphs are cached between processes (CPU provider only; session creation for the layout model ~0.8 s → ~0.15 s) |
| `DOCLING_RS_NO_GRAPH_CACHE` | `0` | `1` disables the optimized-graph cache (models load and optimize from scratch every process) |
| `DOCLING_RS_OCR_SESSIONS` | worker thread budget (1–8) | Parallel single-thread OCR recognition lanes per worker; output is byte-identical at any count |
| `--concurrency N` | `2` | Max simultaneous conversions in flight; excess requests queue |
| `--warmup` | enabled in image | Load the PDF/image models — layout, OCR, TableFormer and the multi-page worker pool — at startup; `/ready` returns 503 until they are loaded, and stays 503 (`warmup_failed` + the error) if loading fails |
| `/health` vs `/ready` | — | `/health` = liveness (200 immediately); `/ready` = readiness: with `--warmup`, 200 once the models are loaded (`"models": "warm"`); without it, 200 immediately (`"models": "lazy"` — the first PDF/image request loads them) |

For a self-contained CLI image with models exported from PyTorch, [`examples/Dockerfile`](./examples/Dockerfile)
is a 3-stage build that bakes the binary, native libs, and models into a slim runtime stage:

```bash
docker build -f examples/Dockerfile -t docling-rs .
docker run --rm -v "$PWD:/data" docling-rs /data/input.pdf          # Markdown to stdout
docker run --rm -v "$PWD:/data" docling-rs /data/input.pdf --to json
```

Both `linux/amd64` and `linux/arm64` build (#281) — everything in the image
is arch-neutral or built from source.

See [`docs/DEPLOYMENT.md`](./docs/DEPLOYMENT.md) for full deployment documentation,
Prometheus metrics, OpenTelemetry tracing, and production tuning.

## Performance

For the declarative formats (everything but PDF/images), `cargo run --release
-p docling --example profile_declarative` times parse, Markdown export and
JSON export separately for every file in the corpus and lists the slowest.
Parsing is milliseconds per document and Markdown export is negligible; JSON
export dominates on table-heavy documents because docling's schema repeats
every table cell in the `grid`, and it builds a `serde_json::Value` tree
before printing. The table builder now fills that grid from an index instead
of a hash map of cloned cells: on the corpus's heaviest JSON (a 13 MB patent
export) the export went from 256 ms to 179 ms, on an EBCDIC table dump from
123 ms to 66 ms, byte-identical output. Printing the tree is ~10 % of the
remaining cost; the rest is the `Value` allocation itself, so a further
step would be serializing straight from the document.

For the PDF/image ML pipeline, `scripts/test/profile_pdf.sh` runs the release
binary over the PDF corpus with `DOCLING_RS_TIMING=1` and sums the pipeline's
per-stage wall-clock (`crates/docling-pdf/src/timing.rs`) into one table. On
the 88-page corpus the cost is almost entirely model inference: TableFormer
structure recognition (the autoregressive OTSL decode — ~1400 decode steps
across the corpus's tables) and the per-page layout model together account for
~85 % of it, with a one-time ONNX session/graph init paid on the first table
page. Everything outside the models — both page renders, the two resamples,
text-layer parsing and assembly — is under ~6 % combined (measured on the
pdfium chain; the Rust renderer's share is of the same order). There is no
glue-code hot spot to cut here the way the JSON grid was; PDF throughput is
bounded by the layout and TableFormer models, so the levers are the INT8
models, the KV-cached decoder and GPU execution providers, not the Rust
around them.

`scripts/test/performance.sh` runs a representative fixture of each supported type
through both engines (published Python `docling` vs the Rust release binary) and
reports peak RSS, CPU utilization, and conversion time. Ratios below are
docling ÷ docling.rs — bigger means Rust wins by more. The PDF row is the
**default stack** ([INT8 layout](#int8-models-faster-pdf-conversion-on-cpu--the-default) +
KV-cached TableFormer decoder); with `DOCLING_RS_FP32=1` (full-precision
models) the same fixture measures 5.2× less memory, a 6.2× warm speedup and
19.8× end-to-end — see [`docs/PDF_CONFORMANCE.md`](./docs/PDF_CONFORMANCE.md).

| File | Size | Peak-memory ratio | CPU ratio | Warm-conversion speedup |
|---|---:|---:|---:|---:|
| `picture_classification.pdf` (PDF) | 208 KB | **6.5× less** | 0.8× | 10.6× |
| `docx_rich_tables_01.docx` (DOCX) | 3.1 MB | **39× less** | 1.2× | 19× |
| `wiki_duck.html` (HTML) | 240 KB | **57× less** | 1.3× | 47× |
| `elife-56337.nxml` (JATS XML) | 180 KB | **59× less** | 1.2× | 10× |
| `xlsx_04_inflated.xlsx` (XLSX) | 168 KB | **51× less** | 0.9× | 18× |
| `powerpoint_with_image.pptx` (PPTX) | 80 KB | **55× less** | 1.2× | 3.1× |
| `wiki.md` (Markdown) | 8 KB | **57× less** | 1.2× | 1.2× |
| `csv-comma.csv` (CSV) | 4 KB | **64× less** | 1.2× | 0.6× † |

- **Peak memory** is where Rust wins decisively: a declarative conversion holds a
  few MB versus docling's ~750 MB (it imports torch even for non-ML formats). The
  PDF runs the full ML pipeline in both engines (torch vs ONNX), so the gap there
  is 6.5× rather than 50×+, but Rust peaks at 0.37 GB vs docling's 2.4 GB —
  and the PDF converts **28.5× faster end-to-end** (docling re-pays its torch
  import + model load on every invocation).
- **CPU**: recent docling releases run declarative work at ~1.2 cores against
  Rust's single core; on the PDF Rust goes wider (~160%) while finishing an
  order of magnitude sooner.
- **Warm-conversion speedup** isolates the parse/convert work — it times docling
  *in-process* (excluding its ~3 s interpreter + import startup) against the Rust
  whole-process figure. Rust wins on substantial inputs (HTML 47×, DOCX 19×); the
  end-to-end figure, which re-pays docling's startup every invocation, is **300–
  870× faster** for the declarative formats.
- † For trivial inputs (a 4 KB CSV) the conversion itself is microseconds, so Rust's
  own process startup dominates its number while warm-Python excludes startup — the
  warm metric understates Rust there. End-to-end, the CSV is **870× faster** in Rust.

## Layout

| Crate | Role | Python analogue |
|---|---|---|
| `docling-core` | `DoclingDocument` model + serializers | `docling-core` |
| `docling` | `DocumentConverter`, source loading, backends | `docling` |
| `docling-pdf` | PDF/image ML pipeline (pure-Rust text layer + renderer, ONNX layout/table/OCR) | `docling` PDF pipeline |
| `docling-asr` | audio/ASR pipeline (symphonia + ONNX Whisper) | `docling` ASR pipeline |
| `docling-onnx` | shared ONNX Runtime execution-provider selection (`DOCLING_RS_EP`; `cuda` / `tensorrt` / `directml` / `coreml` / `xnnpack` features) for the ML crates | — |
| `docling-cli` | command-line interface (`docling-rs`, plus the `serve` subcommand behind `--features serve`) | `docling.cli` |
| `docling-serve` | HTTP conversion API over a warm pipeline (`docling-serve` binary, `ghcr.io/docling-project/docling-rs-serve` image) | `docling-serve` |
| `docling-ffi` | C ABI (`docling.h` + shared/static library) for C, C++, C#, Go, Java, Swift embedders | — |
| `docling-node` | Node.js / Bun N-API bindings | https://www.npmjs.com/package/docling.rs |
| `docling-py` | Python bindings (strangler-fig drop-in over docling-core) | https://pypi.org/project/docling-rs |
| `docling-wasm` | WebAssembly bindings (declarative converters + PDF text layer + browser OCR) | https://www.npmjs.com/package/docling.rs-wasm |
| `docling-rag` | RAG layer: chunking, embeddings, vector search, REST API | — |

## RAG subsystem

[`crates/docling-rag`](./crates/docling-rag) builds a pluggable
Retrieval-Augmented-Generation layer on top of the converter: it turns documents
into Markdown, chunks them (streaming sliding window, or docling's
hierarchical/hybrid chunkers via `RAG_CHUNKER`), embeds the chunks, and
stores them in a vector database for semantic search. Every external dependency is
a swappable trait — embedders (**Ollama**/Gemini/local-ONNX), vector stores
(**SQLite+sqlite-vec**/PostgreSQL+pgvector), LLM (**OpenRouter**, `deepseek/deepseek-chat` by default),
document sources (**folder**/FTP/SFTP), and message queues
(**in-process**/RabbitMQ/Redis). It ships Hybrid, Multi-Query fusion and HyDE
retrieval plus an evaluation harness to compare configurations and an
API-key-protected REST API (`docling-rag serve`) for document info and
search — with a built-in single-page search UI at `GET /` (API key stored in
the browser's localStorage). A `--features cuda` build runs ingest conversion
*and* the local ONNX embedder on the GPU via the same `DOCLING_RS_EP` switch
as the rest of the stack. Configure it via [`.env`](./.env.example); see the
[crate README](./crates/docling-rag/README.md) for a quickstart on any
documents folder.

## HTTP conversion API — `docling-rs serve`

[`crates/docling-serve`](./crates/docling-serve) is the analogue of Python's
`docling-serve`: a long-running server exposing the converter over HTTP. One
warm PDF/image pipeline (layout/OCR/TableFormer stay loaded) is shared across
requests, so repeat PDF conversions skip the model load (~13× faster than a
cold call on the test fixtures); a semaphore bounds concurrent conversions.
Markdown responses stream (chunked transfer); `/health` + `/ready` suit
container probes, and SIGTERM drains in-flight requests before exit.
`GET /` serves API docs plus an interactive test form — upload or URL in,
streamed result out, with extracted pictures rendered below the text — and
`GET /openapi.yaml` describes the whole API (OpenAPI 3.1), so Swagger UI,
Redoc or a client generator can be pointed straight at a running server:

<p align="center">
  <img src="docs/assets/serve-form.png" alt="docling-rs-serve test form: a converted image with the Markdown result and a gallery of extracted pictures" width="720">
</p>

```bash
cargo run --release -p docling-serve                 # 127.0.0.1:5001
# or: cargo run --release -p docling-cli --features serve -- serve

curl -F file=@paper.pdf localhost:5001/v1/convert                # Markdown
curl -F file=@report.docx 'localhost:5001/v1/convert?to=json'    # docling JSON
curl -F file=@sheet.xlsx  'localhost:5001/v1/convert?to=dclx' -O # DocLang archive
curl -F file=@page.html   'localhost:5001/v1/convert?to=chunks'  # chunk records
curl -F file=@paper.pdf   'localhost:5001/v1/convert?to=images&pages=1-3'  # pages → PNG (base64 JSON)
curl -H 'content-type: application/json' \
     -d '{"url": "https://example.com/doc.pdf", "to": "md"}' \
     localhost:5001/v1/convert     # fetch a URL (needs --allow-url-fetch)

curl -F file=@a.pdf -F file=@b.docx localhost:5001/v1/convert    # batch → JSON results array
curl -F file=@bundle.zip localhost:5001/v1/convert                # every document inside, as a batch (#557)

id=$(curl -F file=@big.pdf localhost:5001/v1/convert/async | jq -r .task_id)
curl localhost:5001/v1/status/$id                                # pending|started|success|failure
curl localhost:5001/v1/result/$id                                # the output, once done
```

Long conversions don't have to hold the connection: `POST /v1/convert/async`
accepts the same request and returns a task id immediately — poll
`/v1/status/{id}`, fetch `/v1/result/{id}` (kept `--result-ttl` seconds,
default 10 min; at most `--queue-size` jobs queue at once). Several `file`
parts in one request convert as a batch with per-item status. PDF/image
responses carry a conversion-confidence report (docling-serve v1.25 parity):
an `X-Docling-Confidence` summary header (grades `poor`/`fair`/`good`/
`excellent` + layout/OCR/parse scores) on every format, and the full per-page
report under a top-level `confidence` key in `to=json` bodies.

### Request options

Options go as query parameters, multipart fields or JSON keys (the body
wins). The output options belong to serve; every conversion option is a
[`ConvertOptions`](docs/OPTIONS.md) field under its wire name.

| Option | Values | Meaning |
|---|---|---|
| `to` | `md` (default) \| `json` \| `html` \| `text` \| `dclx` \| `latex` \| `pandoc` \| `vtt` \| `chunks` \| `images` | the output. `images` skips conversion and rasterizes a PDF's pages to PNG — `{"pages": [{"page", "width", "height", "png_base64"}]}` — honouring `pages` and `scale` (0.1–4.0 px/pt, default 2.0), at most 100 pages per request (`DOCLING_RS_MAX_RASTER_PAGES`) |
| `images` | `placeholder` \| `embedded` | pictures in the Markdown: `<!-- image -->` or `data:` URIs |
| `pandoc_api_version` | `1.23`, … | the Pandoc API a `to=pandoc` caller expects |
| `chunker` · `chunk_tokenizer` · `chunk_max_tokens` · `chunk_merge_peers` | `hierarchical` \| `hybrid` · a server-local path · int · bool | `to=chunks` configuration (#256) |
| `url` · `sources` · `target` | JSON body | a URL input (`--allow-url-fetch`), or docling's service shape (#139): `file` (base64) and `http` sources, `s3` / `azure_blob` / `google_cloud_storage` sources and targets with the `cloud` cargo feature, and the jobkit targets `zip` (one archive of the rendered outputs, nothing outbound) and `put` (HTTP-PUT each output to a pre-signed URL, behind `--allow-url-fetch`) — the `s3_pipeline` example in `/openapi.yaml` |
| every `ConvertOptions` field | its wire name | `strict`, `pages`, `no_ocr`, `password`, `ocr_*`, `heading_hierarchy`, the `do_*_enrichment` and `redact_*` switches, `document_timeout`, `image_sources`, `pipeline=vlm` + `vlm_*`, … — values and defaults in [`docs/OPTIONS.md`](docs/OPTIONS.md) |

Policies that stay with the server: URL inputs, `image_sources=remote` /
`fetch_images`, a request-supplied `vlm_endpoint` and `put` targets need
`--allow-url-fetch`; `image_sources=local` needs `--allow-local-images`;
`xbrl_taxonomy` and `chunk_tokenizer` are server-local relative paths. A
conversion cut by `document_timeout` answers `X-Docling-Status:
partial_success` + `X-Docling-Errors` (batch and async items carry `status`
and `errors`); `redact_pii` reports its counts in `X-Docling-Redaction`.

### Server flags

| Flag | Default | Meaning |
|---|---|---|
| `--addr HOST:PORT` | `127.0.0.1:5001` | bind address — loopback by default; front it with a policy proxy for anything wider |
| `--concurrency N` | 2 | conversions in flight; further requests queue |
| `--max-body-mb N` | 256 | upload cap |
| `--queue-size N` · `--result-ttl SECS` | 16 · 600 | async jobs queued or unfetched at once (429 beyond) · how long a finished result stays fetchable |
| `--warmup` | off | load the PDF/image models at startup; `/ready` is 503 until they are |
| `--allow-url-fetch` | off | URL inputs, remote images, caller-supplied VLM endpoints, `put` targets. Private/loopback targets stay blocked (`DOCLING_RS_ALLOW_PRIVATE_IP_FETCH=1` opts out) and downloads are capped (`DOCLING_RS_MAX_FETCH_BYTES`) |
| `--allow-local-images` | off | accept `image_sources=local` (#646) |
| `--strict` | off | strict Markdown for requests that don't say |
| `--api-key KEY` | unset | `X-Api-Key` on every `/v1` route (#615); `DOCLING_SERVE_API_KEY` when the flag is absent — prefer it, a flag shows in the process list |
| `--max-memory-mb N` | the cgroup limit (`DOCLING_RS_MAX_MEMORY_MB`) | admission control (#263): above 85 % of it (`DOCLING_RS_MEMORY_WATERMARK_PCT`) new conversions get 503 + Retry-After instead of an OOM kill; `0` disables |

Thread pools are cgroup-quota-aware (#262; `DOCLING_RS_TF_INTRA` narrows the
shared TableFormer session) and the server runs with `DOCLING_RS_NO_ARENA=1`,
which measured ~3× lower warm RSS at no latency cost. Observability mirrors
Python docling-serve (#297): `tracing` request logs (`RUST_LOG`), Prometheus
text on `GET /metrics` (request counts by status class, an in-flight gauge, a
latency histogram, conversions by outcome) and, with the opt-in `otel` cargo
feature and `OTEL_EXPORTER_OTLP_ENDPOINT` set, OTLP/gRPC request spans.
Container images, Compose files and tuning: [Deploy in a
container](#deploy-in-a-container) and [`docs/DEPLOYMENT.md`](./docs/DEPLOYMENT.md).

### Drop-in for docling-serve (Open WebUI, n8n, Dify, LangChain)

Clients written against Python docling-serve's API talk to this server
unchanged (#615). It serves upstream's routes next to its own `/v1/convert`:

| Route | Takes | Answers |
|---|---|---|
| `POST /v1/convert/file` | multipart `files` (repeatable) + option fields | docling's `ConvertDocumentResponse` |
| `POST /v1/convert/source` | JSON `{"sources": [{"kind": "file"\|"http", …}], "options": {…}, "target": {"kind": "inbody"\|"zip"}}` (also 0.x `file_sources` / `http_sources`) | the same |
| `POST /v1/convert/{file,source}/async` | the same | `TaskStatusResponse` (`task_id`, `task_status`, …) |
| `GET /v1/status/poll/{task_id}` · `GET /v1/result/{task_id}` | — | the task's status · its `ConvertDocumentResponse` |
| `/v1alpha/…` | aliases of the above for docling-serve 0.x clients | |

One document answers `{"document": {"filename", "md_content", "json_content",
"html_content", "text_content", "doctags_content", "doclang_content"},
"status", "errors", "processing_time", "timings"}` with the `to_formats`
asked for (default `md`) filled and the rest `null`; several documents (or
`target_type=zip`) a zip of `<stem>.<ext>` files. A document that fails to
convert is still a **200** with `status: "failure"` and the reason in
`errors[0].error_message`, which is where these clients look. Options take
upstream's names and types: `do_ocr`, `force_ocr`, `do_table_structure`,
`page_range`, `image_export_mode` (default `embedded`, as upstream),
`md_page_break_placeholder`, `do_pdf_heading_hierarchy`, `md_compact_tables`,
the `do_*_enrichment` switches, `document_timeout`, `images_scale`,
`pipeline=vlm`, `ocr_engine` (the Tesseract spellings select Tesseract;
EasyOCR / RapidOCR / ocrmac use the built-in PP-OCR) and `ocr_lang` (the
first code this build reads). Anything this server has no equivalent for —
`pdf_backend`, `table_mode`, `abort_on_error`, picture-description and preset
knobs — is accepted and ignored, so a client's extra parameters never fail a
conversion. This server's own option names (`strict`, `password`, …) work
there too. `doctags_content` stays `null`: there is no DocTags writer
(`doclang_content` carries its successor). `--api-key KEY`, or upstream's
`DOCLING_SERVE_API_KEY`, requires `X-Api-Key` on every `/v1` route; `/health`,
`/ready`, `/metrics` and the docs page stay open.

**Open WebUI**: *Admin Settings → Documents → Content Extraction Engine =
Docling*, *Docling Server URL* = `http://<host>:5001` (and the API key if you
set one). Its loader posts `files` with `image_export_mode=placeholder` and a
form-feed `md_page_break_placeholder`, and reads `document.md_content` split
into pages. Extra *Docling parameters* JSON (`{"do_ocr": true, "ocr_lang":
["en"]}`) is mapped where it means something here and ignored otherwise.

```bash
curl -F files=@report.pdf -F to_formats=md -F to_formats=text \
     localhost:5001/v1/convert/file | jq '.status, .document.text_content'
curl -H 'content-type: application/json' localhost:5001/v1/convert/source \
     -d '{"sources": [{"kind": "http", "url": "https://arxiv.org/pdf/2206.01062"}],
          "options": {"to_formats": ["md"], "do_ocr": false}}'  # needs --allow-url-fetch
```

## In the browser — `docling-wasm`

The declarative converters (everything except the PDF/image/audio ML
pipelines) compile to `wasm32-unknown-unknown`:
[`crates/docling-wasm`](./crates/docling-wasm) exposes
`convert(bytes, filename, to)` → Markdown / docling JSON / DocLang / LaTeX / HTML / Pandoc AST via
`wasm-bindgen`, so DOCX/HTML/XLSX/PPTX/EPUB/… convert **fully client-side** —
no server, ~3.4 MB gzipped module, no models to download for the declarative
formats (the default-on browser-OCR feature does fetch its ONNX models) —
something Python docling has no equivalent for. Ready to use from npm:

```bash
npm i docling.rs-wasm
```

```js
import { convert } from "docling.rs-wasm";          // bundlers
// import init, { convert } from "docling.rs-wasm/web"; await init();  // no bundler
const markdown = convert(bytes, file.name, "md");
``` Digital PDFs convert too: the wasm build always compiles docling-pdf's
pure-Rust text-layer parser (the `pdf-text` feature of the `docling` crate;
the same extraction as `--text-layer-only`: flat paragraphs, no headings/tables/pictures),
while scanned PDFs get a clear "needs OCR" error instead of an empty
document. The crate ships a drop-a-file demo page under
[`www/`](./crates/docling-wasm/www). Native builds are untouched: the
feature slices behind this (`pdf` / `asr` / `fetch-images`) all stay in the
`docling` default set, so a plain `cargo build` is unchanged.

## Integration — using docling.rs from your language

One engine, several front doors. Every surface takes the same options
(`to`, `strict`, `images`, `no_ocr`, `ocr_mode`, `heading_hierarchy`,
`pages`, …) and returns the same outputs (Markdown, docling JSON, DocLang
`.dclx`, LaTeX, HTML, Pandoc AST, chunk records).

| You write… | Use | Install | Details |
|---|---|---|---|
| Rust | the `docling` crate: `DocumentConverter` + `SourceDocument` | `cargo add docling` | [The API](#the-api) |
| a shell / CI job | the `docling-rs` CLI (`--to md\|json\|html\|dclx\|chunks\|latex\|pandoc\|images`, `--input`/`--output` batch mode) | `cargo install docling-cli` · [release binaries](https://github.com/docling-project/docling.rs/releases) · `ghcr.io/docling-project/docling-rs` | [Batch conversion](#batch-conversion--several-sources---input----output), [Install](#install-locally--in-ci-one-liner) |
| anything that speaks HTTP | `docling-serve`: `POST /v1/convert` (multipart or JSON), async jobs, OpenAPI 3.1 | `docker run -p 5001:5001 ghcr.io/docling-project/docling-rs-serve` · `cargo install docling-serve` | [HTTP conversion API](#http-conversion-api--docling-rs-serve), [docs/DEPLOYMENT.md](./docs/DEPLOYMENT.md) |
| Node.js / Bun / Electron | `docling.rs` (N-API addon): `convertFile`, `convert`, streaming, chunking, warm `Pipeline` | `npm i docling.rs` (`docling.rs-cuda` for GPU) | [Node bindings](#nodejs--bun-bindings), [crate README](./crates/docling-node/README.md) |
| Python | `docling-rs` — a drop-in for docling's `DocumentConverter` over the Rust engine | `pip install docling-rs` (`docling-rs-cuda` for GPU) | [Python bindings](#python-bindings), [migration guide](./crates/docling-py/README.md#migrating-from-python-docling) |
| the browser / Tauri / a PWA | `docling.rs-wasm`: `convert(bytes, name, to)` fully client-side, optional in-browser OCR/layout | `npm i docling.rs-wasm` | [In the browser](#in-the-browser--docling-wasm), [crate README](./crates/docling-wasm/README.md) |
| C, C++, C#/.NET, Go, Java, Swift, Zig, … | `docling-ffi`: `docling_convert(bytes, len, filename, options_json)` behind one `docling.h` | [release archives](https://github.com/docling-project/docling.rs/releases) `docling-ffi-<tag>-<target>` (library + header) | [C ABI](#c-abi-for-embedders--docling-ffi), [language quickstarts](./crates/docling-ffi/README.md#language-quickstarts) |
| a RAG stack | `docling-rag`: chunk → embed → search, REST API and web UI | `cargo install docling-rag` | [RAG subsystem](#rag-subsystem) |
| a LangChain pipeline | `docling_rs.langchain.DoclingLoader` — langchain-docling's loader (and picture descriptions) on the Rust engine | `pip install "docling-rs[langchain]"` | [Python bindings](#python-bindings), [examples](./crates/docling-py/examples/langchain/) |

Which one to pick: **Rust** when you're already in Cargo; **CLI** for scripts
and batch jobs (the ML models load once per run); **serve** when several
services or languages share one warm pipeline — models stay loaded, admission
control keeps memory bounded, and every language has an HTTP client;
**Node / Python** for in-process use from those runtimes with zero extra
services; **wasm** when the document must not leave the user's machine;
**FFI** for everything else that can call a C function. The ML assets
(`.models/`) are the same for all of them — see
[Getting the ML models](#getting-the-ml-models); the Python and Node packages
fetch them on first use, the container images ship them baked in.

```bash
# CLI
docling-rs report.pdf --to md
docling-rs --input ./docs --output ./out --to latex
docling-rs --help        # every flag; --version reports the compiled-in features
docling-rs --list-input-formats    # extensions this binary converts, one per line
docling-rs --list-output-formats   # the --to values, one per line

# HTTP
curl -F file=@report.pdf 'localhost:5001/v1/convert?to=json&heading_hierarchy=true'
```

```js
// Node.js / Bun
import { convertFile } from 'docling.rs'
const { content } = convertFile('report.pdf', { to: 'markdown' })
```

```python
# Python — docling-shaped
from docling_rs import DocumentConverter
doc = DocumentConverter().convert("report.pdf").document
print(doc.export_to_markdown())
```

```js
// Browser (wasm) — nothing leaves the page
import init, { convert } from "docling.rs-wasm/web";
await init();
const tex = convert(new Uint8Array(await file.arrayBuffer()), file.name, "latex");
```

```c
/* C ABI — any language with FFI */
DoclingResult *r = docling_convert(bytes, len, "report.docx", "{\"to\":\"json\"}");
if (!docling_result_error(r)) fwrite(docling_result_output(r), 1, docling_result_output_len(r), stdout);
docling_result_free(r);
```

## The API

```rust
use docling::{DocumentConverter, SourceDocument};

let converter = DocumentConverter::new();
let result = converter
    .convert(SourceDocument::from_file("input.md").unwrap())
    .unwrap();

println!("{}", result.document.export_to_markdown()); // Markdown
println!("{}", result.document.export_to_json());     // docling DoclingDocument JSON
```

A failed conversion is a `ConversionError`; when the document is encrypted,
`err.encryption()` returns the typed reason (#636) — `NeedPassword` and
`WrongPassword` are worth prompting for a password (`needs_password()`),
`NotDecryptable` / `Malformed` are not:

```rust
match converter.convert(SourceDocument::from_file("locked.pdf").unwrap()) {
    Ok(result) => println!("{}", result.document.export_to_markdown()),
    Err(e) if e.encryption().is_some_and(|k| k.needs_password()) => ask_for_password(),
    Err(e) => eprintln!("{e}"),
}
```

`docling-rs serve` answers the same case with `422 {"error": …, "code":
"password_required" | "wrong_password" | "not_decryptable" |
"malformed_encryption"}`; the Python bindings raise `PasswordRequiredError` /
`WrongPasswordError` / `EncryptionError`, subclasses of `ConversionError`.

### One option set, every surface — `ConvertOptions`

Every conversion knob is one serializable struct,
[`docling::ConvertOptions`](crates/docling/src/options.rs) (#577), and every
surface fills it — the CLI from flags, serve from the query string / JSON
body / multipart parts, Python and Node from keyword arguments and option
objects, the C ABI and wasm from one JSON object — then runs the same
`validate()` and `apply()`. The defaults live in
`DocumentConverter::default()`; an unset option means that default
everywhere. The ones most conversions reach for:

| Wire name | CLI | Meaning |
|---|---|---|
| `pages` | `--pages A-B` | PDF page window (#80) |
| `no_ocr` · `text_layer_only` · `force_full_page_ocr` | `--no-ocr` · `--text-layer-only` · `--force-full-page-ocr` | never OCR, layout and tables kept · the text layer alone, no ML · OCR every page even with a text layer |
| `no_table_former` | `--no-table-former` | geometric tables instead of TableFormer |
| `ocr_lang` · `ocr_engine` | `--ocr-lang` · `--ocr-engine` | `en` / `ch` / BCP-47; `ppocr` or `tesseract` (#460) |
| `heading_hierarchy` | `--heading-hierarchy` | infer PDF heading levels (#302) |
| `images_scale` · `page_images` | `--images-scale` · `--page-images` | picture-crop resolution; keep page renders in the JSON (#520) |
| `document_timeout` | `--document-timeout` | per-document budget; the pages done so far become a partial success (#497) |
| `password` | `--password` | encrypted PDF and Office documents (#611, #625) |
| `image_sources` | `--image-sources` | `none` / `embedded` / `local` / `remote` — which `<img>` references resolve (#646) |
| `do_picture_classification` · `do_code_enrichment` · `do_formula_enrichment` · `do_picture_ocr` | `--enrich-picture-classes` · `--enrich-code` · `--enrich-formula` · `--picture-ocr` | the enrichment models (#423, #645) |
| `redact_pii` | `--redact-pii` | PII redaction before export (#621) |
| `pipeline` + `vlm_endpoint`, `vlm_model` | `--pipeline vlm --vlm-endpoint … --vlm-model …` | the remote vision-model pipeline (#77) |
| `strict` · `compact_tables` · `page_break_placeholder` | `--strict` · `--compact-tables` · `--page-break-placeholder` | Markdown dialect, unpadded tables, text between pages |

The full table — every option with its values, default and each surface's
spelling — is [`docs/OPTIONS.md`](docs/OPTIONS.md); an inventory test holds
every surface's documentation to it, so an option cannot ship on one surface
and silently miss another.

```rust
use docling::{ConvertOptions, DocumentConverter};

let options: ConvertOptions = serde_json::from_str(r#"{"pages": "1-3", "no_ocr": true}"#)?;
let converter = DocumentConverter::from_options(&options)?; // validated + applied
```

### Output formats

| `--to` / `to` | Library | What it is |
|---|---|---|
| `md` (default) | `export_to_markdown()` | docling's Markdown byte for byte; `--strict` for the cleaner dialect (below); `--images embedded\|referenced` for pictures |
| `json` | `export_to_json()` | docling-core's `DoclingDocument` wire format (schema 1.10.0): the `body` tree of `$ref`s into `texts` / `groups` / `tables` / `pictures`, labels, list groups, table grids, images as `data:` URIs. Loads straight into Python docling-core (`DoclingDocument.load_from_json`) and round-trips to the same Markdown |
| `html` | `export_to_html()` | docling-core's `HTMLDocSerializer` with its defaults: stylesheet, `<h{level+1}>`, inline groups, tables with spans and rich cells, `<figure>` pictures, MathML formulas (a literal port of `latex2mathml`). Byte-identical to docling-core on the whole declarative corpus (265/265) |
| `text` | `export_to_text()` | docling's `--to text` (#613): the Markdown with the decoration off — no `#`, no emphasis markers, links as their label, code without fences, no escaping; lists, checkboxes and table grids kept |
| `dclx` | `export_to_doclang()`, `docling::dclx::save_as_dclx` | DocLang — docling 2.110's XML of the tree (`<doclang version="0.7">`), pretty-printed like `minidom.toprettyxml`; `.dclx` is the OPC archive `save_as_doclang()` writes (`document.xml` + one PNG part per picture). Reads back in too: `.dclg` / `.dclx` are input formats (15/15 exact vs docling) |
| `latex` | `export_to_latex()` | docling 2.124's `LaTeXDocSerializer` with its defaults: `article` preamble, `\title` + `\maketitle`, sectioning, `itemize` / `enumerate`, `tabular` grids, `figure` placeholders, `verbatim`, `$$…$$`. 93 of 116 fixtures byte-exact against `docling --to latex` (98 once upstream's duplicated formatted list items — docling-core#740 — are normalized away) |
| `pandoc` | `export_to_pandoc_json()` | Pandoc's JSON AST (#515, `pandoc-api-version` 1.23.1.1), so every Pandoc writer — DOCX, ODT, EPUB, reST, Org, Typst, … — sits behind the parser; pictures embedded by default. `--pandoc-api-version` states the version a consumer needs |
| `vtt` | `export_to_vtt()` | WebVTT subtitles (#614, docling-core's `WebVTTDocSerializer`): one cue per timed item — an ASR transcript's segments, a WebVTT input's cues with voices and formatting. Untimed content is not represented |
| `chunks` | `docling::chunker` | both chunkers' records (below) |
| `images` | — | no conversion: a PDF's pages as PNG (`<stem>_page_NNNN.png`; `{"pages": […]}` on serve), honouring `--pages` and `--scale` 0.1–4.0 px/pt (#243) |

Every surface takes the same values — the CLI (batch mode writes
`<stem>.<ext>`, several formats at once with `--to md,json`), serve
(`to=`, the matching content type), Node / wasm / the C ABI (`to:`); the
Python bindings hand back docling-core's own `DoclingDocument`, so upstream's
`export_to_*` / `save_as_*` apply to it directly. Page breaks
(`--page-break-placeholder`) apply to Markdown and text.

```bash
docling-rs paper.pdf --to pandoc | pandoc -f json -t docx -o paper.docx
docling-rs talk.mp3 --to vtt > talk.vtt
docling-rs --to images --pages 2-3 --scale 1.5 paper.pdf   # paper_page_0002.png, paper_page_0003.png
```

**JSON structure.** HTML, DOCX, PPTX, ODF, WebVTT, JATS, AsciiDoc, DocLang,
LaTeX and USPTO build docling's item tree — heading nesting, `inline`
groups with `formatting` / `hyperlink`, rich table cells, `furniture` — so
their JSON is structurally identical to upstream's; the flat backends
(XLSX, CSV, EBCDIC) already have upstream's shape. Tables carry first-class
cells (`table_cells`: text, page-point bbox, spans, header roles — #240),
and `DoclingDocument` exposes them for in-place repair before re-export
(#238): `tables_mut()`, `find_cell_by_bbox`, `set_cell_text`,
`update_cell_by_bbox`; `rows`, `structure` and `cells` are public fields.

**Content layers** (#499, #599). The HTML and Markdown exports render the
`body` layer by default, so page headers and footers (`furniture`),
reviewer comments (`notes`) and hidden content (`invisible`) stay out as
upstream leaves them out; `export_to_html_with_layers` /
`MarkdownExportOptions { layers, traverse_pictures, escape_html,
escape_underscores, image_placeholder, image_mode }` take the set to render
— byte-identical to docling-core for the same sets, and the defaults are
`export_to_markdown()` / `export_to_html()` byte for byte:

```rust
use docling::{ContentLayer, ContentLayers, MarkdownExportOptions};
let html = doc.export_to_html_with_layers(ContentLayers::BODY.with(ContentLayer::Furniture));
let (md, _) = doc.export_to_markdown_with_options(&MarkdownExportOptions {
    layers: ContentLayers::ALL, traverse_pictures: true, ..MarkdownExportOptions::default()
});
```

**Pandoc mapping.** Headings → `Header`, text and inline groups → `Para`
of `Str` / `Space` runs with `Strong` / `Emph` / `Underline` / `Strikeout` /
`Link`, lists → `BulletList` / `OrderedList`, code → `CodeBlock`, formulas
→ `Math`, tables → `Table` with header rows and spans, pictures → `Image`
or a captioned `Figure`, DOCX/ODT footnotes → `Note` at their call site
(#538), key-value and form regions → classed `Div`s with a
`DefinitionList`, every other label → `Div .docling-<label>`. Page
provenance, confidence and furniture have no Pandoc place and are left
out. Every declarative fixture and the PDF corpus pass `pandoc -f json -t
native`; from Python, `docling_rs.pandoc.export_to_pandoc(doc)` /
`save_as_pandoc(doc, path, image_mode=…)`.

### Chunking (docling's Hierarchical & Hybrid chunkers)

`docling_core.transforms.chunker` ported to Rust — the chunkers RAG
pipelines feed to embedding models. `HierarchicalChunker` yields one chunk
per document item (whole lists, triplet-serialized tables, picture
captions) with its heading path; `HybridChunker` refines them with a
tokenizer — splits oversized chunks (item boundaries, then docling's
`semchunk` inside text, tables line by line) and merges undersized
same-heading neighbours. Against docling's chunkers on the 83-document
corpus (`scripts/conformance/chunks_conformance.sh`): hierarchical 98.8 % /
hybrid 96.2 % identical chunk records.

```rust
use docling::chunker::{contextualize, HierarchicalChunker, HybridChunker, HuggingFaceTokenizer};

let chunks = HierarchicalChunker.chunk(&result.document);          // structure-driven
let tok = HuggingFaceTokenizer::from_file(".models/chunk/tokenizer.json", 256)?; // feature "chunking"
for chunk in HybridChunker::new(tok).chunk(&result.document) {
    let embed_me = contextualize(&chunk); // heading path + chunk text
}
```

```python
from docling_rs.chunking import HierarchicalChunker, HybridChunker
chunker = HybridChunker(max_tokens=256)
for chunk in chunker.chunk(doc):
    embed_me = chunker.contextualize(chunk)
```

`download_dependencies.sh` fetches MiniLM's tokenizer to
`.models/chunk/tokenizer.json`, which every surface picks up
(`DOCLING_CHUNK_TOKENIZER`, `DOCLING_CHUNK_MAX_TOKENS` override it;
per run `--chunker hierarchical|hybrid`, `--chunk-tokenizer`,
`--chunk-max-tokens`, `--no-chunk-merge-peers` and the matching serve
fields, #256). Also in the [Node bindings](./crates/docling-node)
(`chunkFile` / `chunkDocument`), the [Python bindings](./crates/docling-py)
(`docling_rs.chunking`) and the [RAG subsystem](./crates/docling-rag).

### Image extraction

Backends that have the image populate `Node::Picture { image }`: the
PDF/image pipeline crops figure regions, DOCX / PPTX / MHTML pull embedded
blobs, Windows metafiles (EMF / WMF) in the office formats are rendered to
PNG in-process (#536). `ImageMode` — docling's `image_mode` — picks how
pictures render:

```rust
use docling::ImageMode;
// self-contained Markdown: ![Image](data:image/png;base64,…)
let (md, _) = result.document.export_to_markdown_with_images(ImageMode::Embedded, "artifacts");
// referenced: ![Image](artifacts/image_000000.png) + the bytes to write
let (md, files) = result.document.export_to_markdown_with_images(ImageMode::Referenced, "artifacts");
for (path, bytes) in files { std::fs::write(path, bytes).unwrap(); }
```

JSON always embeds the images as docling `ImageRef`s; the default Markdown
stays `<!-- image -->`, like docling (the pixels are real, the base64 is
not byte-identical to docling's — a different PNG encoder). Pictures that
are *references* — HTML / EPUB / MHTML / JATS / AsciiDoc / ODF `<img src>`,
Markdown `![…](…)`, an email's `cid:` — stay placeholders by default and
resolve under `image_sources` (#646): `embedded` (`data:` URIs and parts of
the same container; no filesystem, no network), `local` (plus files under
the source's directory), `remote` (plus `http(s)`, confined to
`image_hosts`); `max_image_bytes` / `max_images` / `max_image_total_mb` /
`min_image_bytes` bound a document — details in
[`docs/OPTIONS.md`](docs/OPTIONS.md) and [`docs/SECURITY.md`](docs/SECURITY.md).

### `strict` Markdown (Rust-only)

By default `export_to_markdown()` reproduces docling's output byte-for-byte,
quirks included (`***x*** .`, dropped code-fence languages, `\_` escaping).
`strict(true)` gives cleaner, more conformant Markdown
(`export_to_markdown_with(strict)` per call; Python docling has no such
switch):

```text
legacy:  Foo ***both*** .   |   ``` (lang dropped)   |   Name: \_\_\_
strict:  Foo ***both***.    |   ```rust (lang kept)  |   Name: ___
```

### Streaming Markdown

`convert_streaming` returns the Markdown as an iterator of chunks instead of
one string — for piping a long document to stdout, an HTTP response or a
socket as it is produced:

```rust
use std::io::Write;
use docling::{DocumentConverter, SourceDocument};

let source = SourceDocument::from_file("input.pdf").unwrap();
let mut out = std::io::stdout();
for chunk in DocumentConverter::new().convert_streaming(source).unwrap() {
    out.write_all(chunk.unwrap().as_bytes()).unwrap();
}
```

PDF is where it pays: the pipeline processes pages in parallel and streams
each page's Markdown in document order as soon as it is ready (a one-page
look-ahead lets paragraphs that wrap across a page break still merge). The
conversion runs on a background thread, the iterator applies backpressure,
dropping it cancels the work, and the concatenated chunks are byte-identical
to `export_to_markdown()`. Every image mode streams
(`convert_streaming_images(source, mode)`); `referenced` writes each page's
image files under `artifacts_dir` as that page is emitted and drops the
bytes. Streaming is Markdown-only — JSON needs every node up front. The CLI
streams by default (`--no-stream` buffers; `--to json` always does).

### PDF pipeline switches

All on every surface (library builder, CLI, serve, Python, Node) — values
and defaults in [`docs/OPTIONS.md`](docs/OPTIONS.md):

- `--pages A-B` (#80): converts that 1-based page window only; out-of-window
  pages are skipped before rasterization.
- `--document-timeout SECONDS` (#497): a per-document budget, checked
  between pages. Once spent, the pages already finished become the document
  and the result is docling's `PARTIAL_SUCCESS` with one error item; the CLI
  writes it and exits 0, serve answers `X-Docling-Status: partial_success`,
  a stream ends with a `Timeout` error item.
- `--text-layer-only`: no ML at all — the embedded text cells as flat
  paragraphs, the fastest path; a scanned PDF comes back empty rather than
  erroring. `--no-ocr` (docling's `--no-ocr` since 2.0, #611) keeps layout
  and TableFormer but never runs OCR; `--no-table-former` keeps OCR but
  reconstructs tables geometrically. `--force-full-page-ocr` is the
  opposite: OCR every page from its render even when a text layer exists
  (for layers that lie — broken encodings, garbage subset fonts).
- `--heading-hierarchy` (#302, docling's `HeadingHierarchyModel`): infers
  section-header levels after assembly from the PDF outline, legal/outline
  numbering and font style, in that precedence. Off by default, as in the
  docling groundtruth.
- `--no-text-panels`: keeps every detected picture a picture instead of
  demoting uncaptioned dense-text "pictures" into paragraphs (#173).
- `--skip-empty-cells`, `--compact-tables` (#271): sparse-spreadsheet
  relief — omit empty cells from XLSX table rows; unpadded `| a | b |`
  Markdown tables. `--page-break-placeholder TEXT`: docling's
  `page_break_placeholder` between rendered pages.

Scanned pages come upright before layout and OCR: a `/Rotate` flag is
applied, and a page rotated physically in the raster is probed under each
90° hypothesis on the text detector's lines and un-rotated when a rotated
reading is clearly more confident (#225, #571; `DOCLING_RS_OCR_ORIENTATION=off`
disables it). A one-line paragraph the layout model files as `page_footer`
directly under a heading with no other body — a CV's `Languages` line — is
read as that heading's text instead of vanishing with the furniture.

### OCR

OCR has docling's two stages (#429, #570): RapidOCR's PP-OCRv6 text detector
finds the lines on a scanned page or image — inside the layout regions its
boxes are the recognizer's crops, the lines no region covers come out as
orphan text — and the recognizer reads them, PP-OCRv6 when installed and the
PP-OCRv3 pairs otherwise. On FUNSD's 199 scanned forms word recall is 0.90
against the annotations (Python docling 2.133 with RapidOCR: 0.85). The
default language is `en`; the conformance corpus was generated with the
multilingual `ch` model, so byte-for-byte comparisons with Python docling
run with `--ocr-lang ch`. `--ocr-engine tesseract` (#460) runs the system
`tesseract` binary on the same crops instead, with any of its languages.
Languages, `ocr_mode`, `ocr_scale`, the Tesseract environment and the
detector knobs: [`docs/OPTIONS.md` § OCR](docs/OPTIONS.md#ocr-engines-and-languages).

### VLM pipeline (remote endpoint)

`--pipeline vlm` (#77) replaces the discriminative ML stack with a Vision
Language Model: each page is rendered in Rust and sent to any
OpenAI-compatible vision endpoint — LM Studio, Ollama, vLLM, a hosted
service — with docling's page-conversion prompt, and the answer is parsed by
the DocLang reader. No ONNX models load.

```bash
docling-rs --pipeline vlm \
  --vlm-endpoint http://localhost:11434/v1 \
  --vlm-model granite-docling \
  paper.pdf
```

The same on Node (`pipeline: 'vlm'`, `vlmEndpoint`, …), Python
(`pipeline="vlm", vlm_endpoint=…`) and serve (`pipeline=vlm` + the `vlm_*`
request options). Selecting the pipeline is always explicit: the
`DOCLING_RS_VLM_*` environment supplies values but never switches it on, and
the `vlm_*` options are inert under the standard pipeline. On serve a
request-supplied `vlm_endpoint` needs `--allow-url-fetch`; the safer
deployment pins `DOCLING_RS_VLM_ENDPOINT` / `DOCLING_RS_VLM_MODEL` on the
server and lets requests send only `pipeline=vlm`:

```bash
curl -F file=@paper.pdf 'localhost:5001/v1/convert?pipeline=vlm'
```

Answer grammars are detected per response (#322): DocTags (granite-docling),
DocLang XML, Chandra layout HTML, Unlimited-OCR grounding output and
DeepSeek-OCR Markdown; plain prose degrades to text. Measured against Python
docling's `VlmPipeline` on the same granite-docling endpoint: 87.7 % mean
similarity over 18 fixtures, 3 byte-exact
([docs/PDF_CONFORMANCE.md](./docs/PDF_CONFORMANCE.md)). Flags, env
fallbacks, timeouts, retries and prompts: [`docs/OPTIONS.md` § VLM](docs/OPTIONS.md#vlm-pipeline).

### Headless-browser HTML pre-render (optional)

Almost everything in the HTML backend is pure Rust, but one thing a static
parse can't do is resolve the **CSS cascade** — whether a stylesheet- or
class-driven rule makes an element `display:none` (e.g. a collapsed nav menu).
The optional `--use-web-browser` flag renders the page in the system Chromium
first, drops every element the browser computes as hidden, then feeds the
cleaned HTML through the normal Rust backend (so all structure/table/KVP/
formatting logic still runs in Rust — the browser only decides visibility). It
applies to every HTML-routing input: direct HTML, plus MHTML and EPUB (which
assemble HTML from their archives). It's driven straight from Rust over the
DevTools protocol via
[`headless_chrome`](https://crates.io/crates/headless_chrome) — no Node,
Playwright, or other runtime.

It's gated behind the off-by-default `web-browser` Cargo feature, so the standard
build stays browser-dependency-free:

```bash
cargo run -p docling-cli --features web-browser -- --use-web-browser page.html
```

Chromium is located via `$DOCLING_RS_CHROME`/`$CHROME`, then
`$PLAYWRIGHT_BROWSERS_PATH/chromium`, else autodetected. The page's CSS must be
reachable for the cascade to resolve — inline `<style>` works offline, but a
saved page that links external stylesheets needs those fetchable (with a base
host). Without the feature, `--use-web-browser` is a clear error rather than a
silent no-op.

## Encrypted Office documents

A password-protected `.docx`, `.xlsx`, `.pptx`, `.doc`, `.xls` or `.ppt` converts
with its password (#625 — beyond docling, whose password option opens PDFs
only). It is the same option as the PDF password: `--password` (or docling's
`--pdf-password`), `--password-file PATH` to keep it out of the process list,
`password` in serve, Python and Node (`pdf_password` / `pdfPassword`, the
earlier name, still work). Files with only a *modify*
password convert without one (PowerPoint encrypts them with its built-in
default password, which is tried automatically, as is Excel's). Without the
password — or with a wrong one — the conversion fails saying so; the schemes
are listed in `docs/MIGRATION.md`.

```bash
docling-rs budget.xlsx --password-file ~/.budget-password
curl -F file=@deck.pptx -F password=1234 localhost:5001/v1/convert
```

## Batch conversion — several sources, `--input` / `--output`

One warm process converts many documents (#205, #489): any number of
positional sources — files, directories, quoted globs, ZIP archives — or
`--input GLOB|DIR`, into `--output DIR`. The ML models load once; a failing
file is reported and skipped (non-zero exit at the end, `--abort-on-error`
to stop at the first); output paths print to stdout, progress to stderr.

```bash
docling-rs a.docx sub/b.docx other/c.pdf --output ./converted   # a.md, b.md, c.md
docling-rs --to md,json report.pdf --output ./converted          # report.md + report.json
docling-rs --input '/data/reports/**/*.pdf' --output ./out --to json   # tree kept: out/2024/q1/a.json
docling-rs --input /data/reports --output ./out                  # recursive sweep, convertible files only
docling-rs bundle.zip --output out/                              # every document inside: out/bundle/<entry>.md (#557)
```

| Flag | Meaning |
|---|---|
| `--to FMT[,FMT]` (repeatable) | every format for each document; several need `--output` |
| `--output DIR` · `--output-file PATH` | the output directory · one input, one format, exactly that path (#611) |
| `--output-dirs auto\|flat\|mirror` | layout under `--output` (#496): mirror a directory's / glob's tree, land plain files by stem (default); everything flat as `<stem>.<ext>`; everything by its path relative to the CWD |
| `--images referenced` | each document's pictures in a sibling `<stem>_artifacts/` |
| `--jobs N` | declarative formats in parallel (PDFs share the one warm pipeline) |
| `--abort-on-error` | stop at the first failed file (Python's flag) |
| `--list-input-formats` · `--list-output-formats` | what this build converts, one per line (#603) — ask the binary instead of hard-coding a list |

Every other flag (`--strict`, `--pages`, `--ocr-lang`, `--pipeline vlm`,
enrichment, …) applies to the whole batch. Collision rules, the ZIP limits
and outcomes, email attachments (`EmailAttachments`, Python
`email_attachments()`, Node `emailAttachments()`, #561) and the progress /
exit-code details: [`docs/OPTIONS.md` § CLI batch
mode](docs/OPTIONS.md#cli-batch-mode).

## Node.js / Bun bindings

docling.rs ships as an npm package, [**`docling.rs`**](https://www.npmjs.com/package/docling.rs)
— native TypeScript bindings (built with [napi-rs](https://napi.rs)) that live in
[`crates/docling-node`](./crates/docling-node). It's a real `.node` addon
that loads in both Node.js and Bun (Bun implements N-API — same binary, no
rebuild), exposing the converter with the same knobs as the Rust API: Markdown /
docling JSON output, `strict` mode, image modes, allowed-format restriction,
`fetchImages`, the [remote VLM pipeline](#vlm-pipeline-remote-endpoint)
(`pipeline: 'vlm'`), sync + async (`Promise`) calls, and a `streamFileMarkdown`
async generator.

Install — no Rust toolchain needed, the prebuilt binary for your platform (Linux
x64/arm64, Windows x64) is pulled in automatically:

```bash
npm install docling.rs   # or: bun add docling.rs
```

```ts
import { convert, convertFile, convertFileAsync } from 'docling.rs'

// in-memory bytes → Markdown
const md = convert({ name: 'notes.md', data: Buffer.from('# Hello\n\nWorld **bold**') })
console.log(md.content)

// a file → Markdown or docling JSON (format detected from the extension)
const { content } = convertFile('report.docx')
const json = await convertFileAsync('report.docx', { to: 'json' })
```

A ZIP archive converts as a batch of the documents inside it (#557):
`convertArchiveFile('bundle.zip')` (and `convertArchive`, the `*Async`
variants and the `DocumentConverter` methods) returns one `ArchiveItem` per
entry — `converted` with its `result`, `skipped` with the reason, `failed`
with the error — so one broken document never fails the rest.

Declarative formats (Markdown, HTML, DOCX, XLSX, …) work out of the box. The
PDF/image pipeline needs the ONNX models (none bundled) — so it throws
until you fetch them with `scripts/install/download_dependencies.sh` — see
[Getting the ML models](#getting-the-ml-models) below. `pipeline: 'vlm'` is the
exception: it loads no ONNX models, so it needs nothing on disk.

A reusable `Pipeline` keeps those models warm across many PDFs.

Runnable Node + Bun examples are in
[`crates/docling-node/examples`](./crates/docling-node/examples)
(`npm install && node node-basic.mjs`). See
[`crates/docling-node/README.md`](./crates/docling-node/README.md) for
the full API.

## Python bindings

docling.rs also ships as a PyPI package, **`docling-rs`** — PyO3 bindings (built
with [maturin](https://www.maturin.rs)) in
[`crates/docling-py`](./crates/docling-py). It's a *strangler-fig* drop-in for
docling's Python API: only the document processor is Rust, and
`result.document` is a genuine `docling_core` `DoclingDocument`, so
`export_to_markdown()`, `export_to_dict()`, `export_to_doctags()` and the
chunkers are docling's own Python code.

```python
# was:  from docling.document_converter import DocumentConverter
from docling_rs import DocumentConverter

result = DocumentConverter().convert("report.docx")
print(result.document.export_to_markdown())
data = result.document.export_to_dict()   # docling JSON wire format (schema 1.10.0)
```

`DocumentConverter().convert_archive("bundle.zip")` (#557) converts every
document inside a ZIP — a path, the archive's `bytes` or a `DocumentStream` —
yielding an `ArchiveItem` per entry (`outcome` `converted` / `skipped` /
`failed`, with the `result` or the `error`).

Declarative formats (Markdown, HTML, DOCX, XLSX, …) work with no models; the
PDF/image pipeline downloads the ONNX models on first use via
`docling_rs.download_models()`. On an NVIDIA machine install
**`docling-rs-cuda`** instead (same `docling_rs` module compiled with the CUDA
provider — GPU automatically, CPU fallback). See
[`crates/docling-py/README.md`](./crates/docling-py/README.md) for the full
API, local build steps, and a step-by-step
[migration guide from Python docling](./crates/docling-py/README.md#migrating-from-python-docling)
(swap the install, rewrite the imports, fetch the models — the code below the
imports stays unchanged).

**LangChain:** `pip install "docling-rs[langchain]"` adds
`docling_rs.langchain` — the port of docling's
[langchain-docling](https://github.com/docling-project/docling-langchain)
(`DoclingLoader`, `ExportType`, picture descriptions with any LangChain chat
model), reproducing that package's own test expectations exactly:

```python
# was:  from langchain_docling import DoclingLoader
from docling_rs.langchain import DoclingLoader

docs = DoclingLoader(file_path=["https://arxiv.org/pdf/2408.09869"]).load()  # chunks + docling metadata
```

See the [LangChain section](./crates/docling-py/README.md#langchain) and the
[examples](./crates/docling-py/examples/langchain/) (loader basics, an agentic
RAG with page citations, picture descriptions).

## C ABI for embedders — `docling-ffi`

Embedding from C, C++, C#, Go, Java, Swift or anything else with FFI takes
one shared (or static) library and one header:
[`crates/docling-ffi`](./crates/docling-ffi) exposes a minimal `extern "C"`
surface — `docling_convert()` in, Markdown / docling JSON / DCLX / LaTeX / HTML / Pandoc AST out, with
conversion options as a single JSON object mirroring docling-serve's request
options. The [`include/docling.h`](./crates/docling-ffi/include/docling.h)
header is generated by cbindgen and committed.

```c
DoclingResult *r = docling_convert(bytes, len, "report.docx", "{\"to\":\"md\"}");
if (!docling_result_error(r))
    fwrite(docling_result_output(r), 1, docling_result_output_len(r), stdout);
docling_result_free(r);
```

Prebuilt libraries ship with every
[GitHub Release](https://github.com/docling-project/docling.rs/releases)
(`docling-ffi-<tag>-<target>` — Linux x86_64/aarch64 and Windows x64, library
plus header), so embedders don't need a Rust toolchain or a clone. See
[`crates/docling-ffi/README.md`](./crates/docling-ffi/README.md) for the
options table, build/link steps, quickstarts for C#/.NET, Go, Java and
Swift, and header regeneration.

## Getting the ML models

The PDF/image pipeline needs three ONNX models that aren't bundled in the
crate or the npm addon — RT-DETR layout, PP-OCRv3 recognition, and
TableFormer (optional; tables fall back to geometric reconstruction without
it); the PDF pages themselves are parsed and rendered in pure Rust.
`scripts/install/download_dependencies.sh` fetches all of them from this
repo's [GitHub Releases](https://github.com/docling-project/docling.rs/releases)
(tag `models-v1`) straight into `./.models`, relative to the
current directory — both the Rust CLI/library and the Node.js/Bun bindings
look there by default, so no env vars or extra setup are needed afterwards:

```bash
# from a checkout of this repo, or any directory you'll run docling.rs from:
scripts/install/download_dependencies.sh

# or, without a checkout — e.g. a container build step, or a fresh npm project:
curl -fsSL https://raw.githubusercontent.com/docling-project/docling.rs/master/scripts/install/download_dependencies.sh | sh
```

On **native Windows** (no WSL) use `scripts\install\download_dependencies.bat`
instead — the same models — and see
[docs/WINDOWS.md](./docs/WINDOWS.md) for the MSVC build walkthrough.

| Asset | Destination |
| --- | --- |
| RT-DETR layout | `.models/layout_heron.onnx` |
| PP-OCRv6 rec + dictionary, multilingual (#570 — RapidOCR's and docling's recognizer; preferred for every `ocr_lang` when present) | `.models/ocr_rec_v6.onnx`, `.models/ocr_rec_v6_dict.txt` |
| PP-OCRv3 rec + dictionary, English (the fallback without the v6 pair) | `.models/ocr_rec_en.onnx`, `.models/en_dict.txt` |
| PP-OCRv3 rec + dictionary, multilingual `ch_` (`DOCLING_RS_OCR_LANG=ch` without the v6 pair; the model the PDF conformance baselines were pinned against — weak Latin word spacing) | `.models/ocr_rec.onnx`, `.models/ppocr_keys_v1.txt` |
| PP-OCRv6 text detector (optional, #429 — lines outside layout regions on bitmap pages; without it OCR stays region-scoped; also fetched by the Python `download_models()`, reported by Node's `checkDependencies().ocrDet`, and loaded by the browser demo) | `.models/ocr_det.onnx` |
| TableFormer (optional) | `.models/tableformer/{encoder,decoder,bbox}.onnx` (+ `.data` sidecars where the export needs them); `decoder_kv.onnx` is preferred when present — its current export has a dynamic batch axis, so all tables on a page decode in one lockstep loop (byte-identical to one at a time; an older fixed-batch `decoder_kv.onnx` still works, one table at a time) |
| Whisper tiny (audio/ASR; skip with `--no-asr`) | `.models/asr/{encoder_model,decoder_model}.onnx`, `.models/asr/vocab.json` (+ `added_tokens.json` for language selection) |
| Whisper presets (optional; `--asr-model=<preset>`, repeatable) | `.models/asr/<preset>/…` — English-only (`whisper_tiny_en`, `whisper_base_en`, `whisper_small_en`) and Distil-Whisper (`whisper_distil_small_en`) exports, fetched from Hugging Face |
| Parakeet TDT 0.6B v3 (optional; `--asr-model=parakeet_tdt_0.6b_v3`) | `.models/asr/parakeet_tdt_0.6b_v3/{encoder-model,decoder_joint-model}.int8.onnx` + `vocab.txt` (~670 MB; the fp32 graphs, ~2.5 GB, with `--no-int8`) and `.models/asr/vad/silero_vad.onnx` (Silero VAD v5, ~2 MB) |
| INT8 CPU models (fetched by default; skip with `--no-int8`) | `.models/layout_heron_int8.onnx`, `.models/tableformer/decoder_int8.onnx` (+ `.models/code_formula/decoder_kv_int8.onnx` with `--enrich`) |
| TableFormer encoder, fp16 weights (fetched by default; skip with `--no-int8`) | `.models/tableformer/encoder_fp16.onnx` — the same graph with fp16-stored weights cast back to fp32 at load (#374): half the download, fp32 compute; preferred when present, `DOCLING_RS_FP32=1` opts out |
| DocumentFigureClassifier (picture classification) | `.models/picture_classifier.onnx` |
| CodeFormulaV2 (code/formula enrichment, ~1.3 GB; fetch with `--enrich`) | `.models/code_formula/{vision,embed,decoder_kv}.onnx`, `.models/code_formula/tokenizer.json` |
| NER for PII redaction (#621; `dslim/bert-base-NER`'s ONNX export, MIT, ~430 MB; fetch with `--with-ner`, Python `download_models(ner=True)`) | `.models/ner/{model.onnx,tokenizer.json,config.json}` — `--redact-pii` reads names, organizations and locations with it; without it the pass is pattern-only |

Idempotent — safe to re-run; it skips files already on disk. Pass `--force` to
re-fetch everything, `--no-chunk` to skip the chunker tokenizer, `--embed` to
also fetch the RAG embedder, or set `$DOCLING_RS_MODELS_URL` to fetch from a
different host (your own export, an internal mirror, …). Everything a default
install needs is served from that one host; where the release tag predates a
mirrored asset the script falls back to its upstream home (Hugging Face for
the Whisper and OCR models, PaddleOCR for the dictionaries) —
`$DOCLING_RS_ASR_MODELS_URL` overrides the Whisper host outright, or point
`DOCLING_ASR_{ENCODER,DECODER,VOCAB}` at explicit files. Building the models
from source instead: [`scripts/install/pdf_setup.sh`](#testing). The
docling-parse renderer plugin (the conformance oracle) is fetched only with
`--with-docling-parse`; a build with the opt-in `pdfium` cargo feature looks
for `libpdfium` under `PDFIUM_DYNAMIC_LIB_PATH` / `.pdfium/lib`.

**Fonts.** The pure-Rust page renderer draws fonts a PDF does not embed —
the base-14 Helvetica / Times / Courier most office exports reference — from
the host's font directories (Liberation / DejaVu / URW / Noto on Linux, the
system fonts on macOS and Windows), so a desktop needs nothing extra. A host
with no fonts at all (a slim container, a bare CI runner) renders those
glyphs as placeholder boxes and the layout model sees a different page:
install `fonts-liberation` + `fonts-dejavu-core` (what the published Docker
images do), or `download_dependencies.sh --with-fonts`
(`DOCLING_RS_WITH_FONTS=1`) to drop both families into `.models/fonts/`
(Liberation from Debian's package — needs `ar` — and DejaVu from its GitHub
release, licence texts alongside). `DOCLING_RS_FONT_DIRS` names further
directories at runtime. The face a non-embedded font resolves to is part of
the page image the layout model reads, so two hosts with different fonts
installed can convert the same file to different regions (#633):
`DOCLING_RS_SYSTEM_FONTS=0` limits the search to `.models/fonts` and
`DOCLING_RS_FONT_DIRS` — ship the fonts with the deployment and every host
renders the same page; `DOCLING_RS_DEBUG=1` prints the face each style
resolved to. The published Docker images set exactly that: `=0` with
`DOCLING_RS_FONT_DIRS` pinned to the two packages they install, so a derived
image's extra fonts cannot move the layout (add their directory to
`DOCLING_RS_FONT_DIRS` to use them).

#### Whisper and Parakeet models for audio/ASR

The default run already fetches **Whisper tiny** (multilingual) into
`.models/asr/` — nothing extra is needed for audio inputs:

```bash
scripts/install/download_dependencies.sh          # includes Whisper tiny
scripts/install/download_dependencies.sh --no-asr # …or skip the ~150 MB ASR models
```

Named **model presets** (docling's English-only / Distil-Whisper ASR specs,
the variants with public ONNX exports) are fetched on top with a repeatable
`--asr-model=` flag, each into its own `.models/asr/<preset>/` directory:

```bash
scripts/install/download_dependencies.sh --asr-model=whisper_tiny_en
scripts/install/download_dependencies.sh --asr-model=whisper_base_en --asr-model=whisper_distil_small_en
```

Available presets: `whisper_tiny_en`, `whisper_base_en`, `whisper_small_en`,
`whisper_distil_small_en`. Select one at run time with the CLI's
`--asr-model <preset>`, `DocumentConverter::asr_model(...)` in Rust, the
`asr_model` option in docling-serve requests, or `asrModel` / `asr_model` in
the Node/Python bindings:

```bash
docling-rs --asr-model whisper_tiny_en recording.mp3
```

The multilingual default auto-detects the language per file (`asr_lang`
pins it: `--asr-lang de`, `asr_lang=de` on serve, `asrLang` / `asr_lang` in
the bindings). English-only presets skip detection and always transcribe
English.

**Parakeet TDT 0.6B v3** (#508) is the alternative for non-English speech,
where Whisper tiny/base fall apart and `small` costs three times the CPU:
NVIDIA's multilingual FastConformer transducer (bg, cs, da, de, el, en, es,
et, fi, fr, hr, hu, it, lt, lv, mt, nl, pl, pt, ro, ru, sk, sl, sv, uk —
detected by the model itself, so `asr_lang` is ignored with a warning),
from the [onnx-asr export](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx)
of NVIDIA's checkpoint (CC-BY-4.0):

```bash
scripts/install/download_dependencies.sh --asr-model=parakeet_tdt_0.6b_v3            # int8, ~670 MB
scripts/install/download_dependencies.sh --asr-model=parakeet_tdt_0.6b_v3 --no-int8  # fp32, ~2.5 GB
docling-rs --asr-model parakeet_tdt_0.6b_v3 interview_de.mp3
```

The pipeline (`docling_asr::parakeet`) is a port of
[onnx-asr](https://github.com/istupakov/onnx-asr), the export's reference
runtime: NeMo's 128-mel preprocessor in Rust (pre-emphasis, 512-point STFT,
Slaney mel, per-feature normalization), the FastConformer encoder, and TDT
greedy decoding over the fused prediction-net/joint graph — token logits
plus a duration head that skips 0–4 encoder frames (80 ms each), which is
also where the per-token timestamps come from. Long recordings are cut by
the **Silero VAD** (≤ 20 s speech spans, onnx-asr's defaults) so the
encoder's full attention stays cheap; without the VAD model (or with
`DOCLING_RS_ASR_VAD=off`) an energy-based stand-in splits at pauses and caps
spans at 20 s. Within a span, sentence-final `.`/`?`/`!` start a new
segment, so the output keeps docling's `[time: start-end] text` paragraphs.
The int8 graphs are the default; `DOCLING_RS_FP32=1` (or a GPU execution
provider) prefers the fp32 ones when present.

Verified against onnx-asr 0.12 on identical 16 kHz samples (English, German
and Russian fixtures): the features match its NumPy NeMo preprocessor to
float rounding (≤ 2·10⁻⁵), the TDT loop run on the same encoder output emits
the same tokens at the same frames, and with the fp32 graphs the whole
pipeline's tokens and timestamps are identical to onnx-asr's. The int8
encoder quantizes its activations dynamically, so its output — and the odd
word — shifts with the ONNX Runtime build (1.28 linked here, 1.30 in
onnx-asr's wheel), the same way it does between two onnx-asr installs.
About 0.2× real time on four CPU cores (int8, a 10 s clip in ~2 s).

**Python and Docker.** The Python wheel fetches no ASR models by default;
`download_models(asr_model=…)` takes one preset or a list (`"whisper_tiny"`
for the default model, any name above), writing the same layout into its
cache — `DOCLING_RS_FP32=1` picks Parakeet's fp32 graphs, and
`pdf_models=False` skips the PDF stack for an audio-only install:

```python
import docling_rs
from docling_rs import DocumentConverter

docling_rs.download_models(asr_model="parakeet_tdt_0.6b_v3", pdf_models=False)
doc = DocumentConverter(asr_model="parakeet_tdt_0.6b_v3").convert("interview_de.mp3").document
```

The container images always carry Whisper tiny; bake further presets in at
build time with the `ASR_MODELS` build arg (space- or comma-separated):

```bash
docker build -f crates/docling-serve/Dockerfile --target serve \
  --build-arg ASR_MODELS=parakeet_tdt_0.6b_v3 -t docling-rs-serve .
```

### Enrichment models (picture classification, code, formulas)

docling's optional enrichment stages are ported behind the same opt-in flags
(`PdfPipelineOptions.do_picture_classification` / `do_code_enrichment` /
`do_formula_enrichment`):

```bash
docling-rs --enrich-picture-classes doc.pdf   # classify pictures (26 classes)
docling-rs --enrich-code --enrich-formula doc.pdf
```

```rust
let converter = DocumentConverter::new()
    .do_picture_classification(true)
    .do_code_enrichment(true)
    .do_formula_enrichment(true);
```

* **Picture classification** — `docling-project/DocumentFigureClassifier-v2.5`
  (EfficientNet, 26 figure classes: `bar_chart`, `logo`, `signature`, …). The
  full prediction distribution lands on the JSON picture item as docling's
  `classification` annotation + `meta.classification`; Markdown is unchanged.
  On an ONNX Runtime older than 1.29 (the linked builds carry 1.28) its
  session stops at the `EXTENDED` optimization level (#517): those
  runtimes' x86 NCHWc rewrite turns this graph into one that returns the
  same distribution for every picture (`table` 0.091 first). That costs
  ~5 ms per picture; a 1.29+ library (`ort-load-dynamic`) runs fully
  optimized.
* **Code enrichment** — `docling-project/CodeFormulaV2` (an Idefics3/SmolVLM-
  class VLM exported to ONNX by `scripts/install/export_code_formula.py`, its
  greedy decode verified token-identical to `transformers.generate`). Rewrites
  each code block from its ~120 dpi crop and fills the JSON `code_language`.
* **Formula enrichment** — the same VLM decodes display formulas to LaTeX:
  Markdown renders `$$…$$` instead of `<!-- formula-not-decoded -->`, and the
  JSON formula item carries the LaTeX in `text` (raw glyphs stay in `orig`).

Both models load lazily on the first matching region (a missing model warns
once and skips that pass), and are shared pipeline-wide like TableFormer. The
same three switches exist on every surface: the Python kwargs and the
docling-serve request options (`do_picture_classification`,
`do_code_enrichment`, `do_formula_enrichment` — query, multipart or JSON body,
#423) and the Node options (`doPictureClassification`, `doCodeEnrichment`,
`doFormulaEnrichment`, also on `new Pipeline()`). Mind that CodeFormula is an
autoregressive 256M-parameter VLM — expect seconds per code/formula region on
CPU. Its decoder also ships as dynamic INT8 (`decoder_kv_int8.onnx`, ~165 MB
vs ~655 MB fp32 — 4× less decoder RAM) — fetched with `--enrich` and preferred
automatically when present, like the other INT8 models. Unlike those, it is
*near*-exact rather than byte-exact: greedy decoding has occasional near-tie
tokens the weight rounding can flip (on the conformance fixture, one extra
blank line inside the code block). `DOCLING_RS_FP32=1` opts back into the
byte-exact fp32 decoder.
`scripts/conformance/enrich_conformance.sh` checks the enriched output
against Python docling's on the enrichment test PDFs.

### Picture OCR for non-PDF documents (`--picture-ocr`)

A DOCX or PPTX full of screenshots, an HTML page whose figures are images
of text, the sampled frames of a video — docling leaves the words inside
those pictures unread, since only its PDF/image pipeline runs OCR. Opt in
and the same OCR models read every embedded picture of a non-PDF document
(#645; PDF/image/METS pages are OCR'd by the pipeline already and are left
alone), and the text lands on the picture as docling's description
annotation — exactly where upstream's picture-description (VLM captioning)
models put theirs: Markdown prints it between the caption and the image
placeholder (docling's picture serializer order), the JSON picture item
carries `meta.description` (`created_by` = the engine) plus the
`description` annotation, DCLX writes it structurally and the chunkers put
it in the picture's chunk.

```bash
docling-rs --picture-ocr deck.pptx
docling-rs --picture-ocr --picture-ocr-classes screenshot_from_computer,screenshot_from_manual \
           --picture-ocr-min-side 64 --no-picture-images --to json deck.pptx
```

```rust
let converter = DocumentConverter::new()
    .do_picture_ocr(true)
    .picture_ocr_classes(["screenshot_from_computer", "screenshot_from_manual"])
    .picture_ocr_min_side(64)
    .keep_picture_images(false);
```

* **Which pictures are read.** Every one whose smaller side reaches
  `picture_ocr_min_side` (32 px by default, `DOCLING_RS_PICTURE_OCR_MIN_SIDE`;
  icons and bullets never reach the models); with `picture_ocr_classes`
  only those the DocumentFigureClassifier's top prediction labels as one of
  the listed classes (its 26 — `screenshot_from_computer`, `logo`,
  `photograph`, …; an unknown label is rejected up front). The same image
  embedded twice — a slide master's logo on every slide — is read once.
* **What is attached.** The recognizer's lines in reading order (rows top to
  bottom, left to right within a row), lines under
  `DOCLING_RS_OCR_TEXT_SCORE` dropped as on a scanned page; a picture the
  engine reads no text from gets nothing, so an unused or silent enrichment
  leaves every export byte-identical to today's. The models are the
  pipeline's own — the PP-OCR recognizer with the `ocr_det.onnx` line
  detector when installed, read at docling's image resolution, or Tesseract
  under `--ocr-engine tesseract` — so `ocr_lang` / `ocr_scale` apply.
* **`--no-picture-images`** (`keep_picture_images(false)`) drops the image
  bytes from every picture after the pass: a slim JSON/DCLX and a
  placeholder-only Markdown with the text kept.
* **Degradation.** Under `--no-ocr` / `--text-layer-only`, without the OCR
  model, or in a build without the `pdf` feature, one warning and no text —
  the conversion succeeds.

The same four switches exist on every surface: the docling-serve request
options (`do_picture_ocr`, `picture_ocr_classes`, `picture_ocr_min_side`,
`keep_picture_images` — query, multipart or JSON body), the Python kwargs
(also on `PdfPipelineOptions`; `picture_ocr_classes` takes a list or the
comma-separated string) and the Node options (`doPictureOcr`,
`pictureOcrClasses`, `pictureOcrMinSide`, `keepPictureImages`).
### PII redaction (`--redact-pii`)

A converted document feeds search and RAG pipelines — Markdown, chunks,
embeddings, vector stores, LLM prompts — and personal data in the source
flows into every one of them. Post-processing one export misses the text
that lives elsewhere in the model: the item tree behind the JSON, the link
table, a code block's `orig`, a caption's hyperlink, the pixels of an
embedded image. The opt-in redaction pass (#621; a docling.rs extension —
Python docling has no redaction stage) runs **once, on the
`DoclingDocument`, after the backend and before any serializer, chunker or
stream reads it**, replaces every detected span in place and reports
counts per label — never the values:

```bash
docling-rs --redact-pii contract.docx                       # [EMAIL], [PHONE], [PERSON] …
docling-rs --redact-pii --redact-mode pseudonym deck.pptx   # [EMAIL_1], [EMAIL_2]: consistent per value
docling-rs --redact-pii --redact-kinds email,phone,credit_card \
           --redact-pattern 'case_id=CASE-\d{6}' --redact-images box_out --to json scan.pdf
```

```rust
use docling::{DocumentConverter, RedactionOptions, Replacement};
let converter = DocumentConverter::new().redact_pii(RedactionOptions {
    replacement: Replacement::Pseudonym,
    ..Default::default()
});
let result = converter.convert(source)?;
println!("{:?}", result.redaction.unwrap().counts);   // {"EMAIL": 3, "PHONE": 1, "PERSON": 2}
```

* **What is found.** The built-in pattern detector (pure Rust, wasm too):
  e-mail (Markdown escapes and `mailto:` links included), phone numbers in
  their usual groupings, card numbers (Luhn-checked, so an order number is
  not a card), IBANs (mod-97 + the registry's per-country length), IPv4 /
  IPv6, `user:password@` URL credentials, US SSNs, UK NINOs and Indian
  Aadhaar numbers (Verhoeff) — dates and version strings are not phones.
  With the NER model installed (`scripts/install/download_dependencies.sh
  --with-ner` / Python `download_models(ner=True)`: `dslim/bert-base-NER`'s
  MIT-licensed ONNX export into `.models/ner/` — `model.onnx`,
  `tokenizer.json`, `config.json`; `DOCLING_RS_NER_DIR` overrides the
  directory) also **names, organizations and locations**; without it one
  warning and the pattern kinds only. Your
  own `NAME=REGEX` patterns, deny terms and allow terms plug in through
  `RedactionOptions`; any detector implementing `docling_core::PiiDetector`
  does.
* **What is rewritten.** Every string of the model, wherever it nests:
  headings, paragraphs, list items, inline runs (matched on the joined
  text, mapped back onto the runs), table cells and rich-cell blocks, field
  regions, key-value cells, captions and their hyperlinks, code text and
  `orig`, a formula's `orig` (not its LaTeX), comments, VTT voices, the
  link table, and the item tree the JSON export reads for HTML / DOCX. So
  Markdown, JSON, DCLX, text, LaTeX, Pandoc and the chunkers all come out
  clean; the streaming Markdown of a PDF is byte-identical to the buffered
  one. The document's file name is not touched.
* **Images.** `--redact-images drop` (the default) removes every embedded
  image and page render — nothing unread leaves the document; `box_out`
  OCRs each image with the pipeline's own models and paints a box over the
  lines that carry a value (needs the OCR models; a `--no-ocr` converter and
  the PDF streaming path drop instead, with a warning); `keep` leaves them.
* **Report.** `result.redaction` (`RedactionReport`): counts per label and
  the total; `return_mapping` (library only) adds the `original →
  placeholder` pairs. docling-serve answers with the counts in
  `X-Docling-Redaction` (one document) or `redaction` (batch / async items)
  and never returns the mapping.

Not a compliance guarantee (GDPR, HIPAA, PCI): pattern and model recall are
what they are, there is no cross-node entity detection, pseudonyms do not
persist across documents, and the source file is never rewritten. The same
five switches exist on every surface: `redact_pii`, `redact_mode`,
`redact_kinds`, `redact_pattern`, `redact_images` as docling-serve request
options, Python kwargs (`result.redaction` is the counts dict) and Node
options (`redactPii`, `redactMode`, `redactKinds`, `redactPattern`,
`redactImages`; the result's `redaction`).

### INT8 models (faster PDF conversion on CPU — the default)

The `*_int8` assets are post-training quantizations of the same models:
Conv-only static INT8 of the layout detector (calibrated on this repo's PDF
corpus) and dynamic INT8 of the TableFormer decoder. On CPUs with AVX-512
VNNI they make layout inference — the dominant PDF cost — **~2.4× faster**
(~1.4–1.8× end-to-end) at conformance validated as unchanged against the
corpus groundtruth; the TableFormer output is byte-identical. See
[`docs/PDF_CONFORMANCE.md`](./docs/PDF_CONFORMANCE.md) for the measurements.

**The pipeline uses them automatically** whenever they sit next to the fp32
files at the default paths (`download_dependencies.sh` fetches them by
default; `--no-int8` skips, or build them with `python
scripts/install/quantize_models.py`). To force full precision:

```bash
DOCLING_RS_FP32=1 docling-rs input.pdf          # keep the int8 files, use fp32
# or pin a model explicitly — an explicit path always wins:
export DOCLING_LAYOUT_ONNX=$PWD/models/layout_heron.onnx
export DOCLING_TABLEFORMER_DECODER=$PWD/models/tableformer/decoder.onnx
```

(The [example Dockerfile](./examples/Dockerfile) bakes both precisions and
defaults to INT8; build with `--build-arg INT8=0` for pure fp32.)

### GPU execution providers (optional, off by default)

The ONNX stages (layout, TableFormer, OCR, enrichment, Whisper, the RAG
embedder) run on CPU by default. GPU execution providers compile in behind
cargo features — the standard build keeps zero GPU dependencies:

```bash
cargo build --release -p docling-cli --features cuda      # NVIDIA CUDA (Linux/Windows)
#                                     --features tensorrt # NVIDIA TensorRT (usually with cuda)
#                                     --features directml # DirectML (Windows)
#                                     --features coreml   # CoreML (macOS)
#                                     --features xnnpack  # XNNPACK (CPU-class ARM NEON / x86 SIMD;
#                                                         # needs a self-built ONNX Runtime, see below)
```

Each provider only exists on its OS (ort ships no CoreML build for Linux, no
DirectML outside Windows, no CUDA for macOS) — an impossible pairing now
fails at compile time with a message naming the alternatives, instead of a
linker error at the end of the build.

A CUDA / TensorRT / DirectML build defaults to `auto`: it converts on the
GPU when one is usable and falls back to CPU when not — you chose a GPU
build, so it uses the GPU. CoreML is the exception: it is **opt-in** (#602),
so a `coreml` build converts on CPU until `DOCLING_RS_EP=coreml` (or `auto`,
set by name) asks for it — see the CoreML notes below for why.
`DOCLING_RS_EP` overrides:

```bash
DOCLING_RS_EP=cuda   docling-rs input.pdf   # this provider or fail loudly
DOCLING_RS_EP=coreml docling-rs input.pdf   # CoreML (a coreml build; opt-in)
DOCLING_RS_EP=cpu    docling-rs input.pdf   # force CPU (the default-build behavior)
```

An explicitly named provider that can't initialize (no device, missing
driver/toolkit libs) fails the conversion rather than silently running 10×
slower on CPU; `auto` is the quiet-fallback mode for images deployed on mixed
fleets.

**Session creation is serialized under a GPU provider** (#452). The page
workers, OCR lanes and enrichment models normally open their ONNX Runtime
sessions concurrently so start-up is paid once; the CUDA provider does not
tolerate that on one device — with `DOCLING_RS_PDF_WORKERS` or
`DOCLING_RS_OCR_SESSIONS` above 1, initialization failed intermittently with
`Exception during initialization: … stride > 0 was false`, while the sessions
ran fine side by side once they existed. Every session in the workspace now
takes one process-wide lock while it is being created whenever a non-CPU
provider is registered; inference stays fully parallel, and CPU builds (or
`DOCLING_RS_EP=cpu`) keep the parallel start-up. No configuration needed —
the `=1` workarounds are no longer necessary.

CoreML defaults (#324, #602), measured on an M4 Max:

- **Model format `NeuralNetwork`** (`DOCLING_RS_COREML_FORMAT`:
  `neuralnetwork`|`mlprogram`). It reproduces the CPU provider's Markdown
  byte for byte. `MLProgram` is ~1.8× faster on the GPU but changes the
  layout detections (on a 25-page manual, 14 pages differ: neighbouring
  boxes merge, two-line headings join, reading order shifts — under
  `cpu_only` units too, so it is the format's op lowering, not GPU fp16), so
  it is an opt-in that prints a notice. `MLProgram` was the #324 default,
  adopted because `NeuralNetwork` aborted on the dynamic-shaped layout graph;
  the two defaults below keep such graphs off CoreML now.
- **Per-page layout** (`DOCLING_RS_PDF_LAYOUT_BATCH` defaults to 1 under
  CoreML, not the CUDA-class 4): per-page mode pins the layout graph's batch
  axis, and only a fully static graph is handed to CoreML. The batched
  default left CoreML nothing to run — 0% GPU, and slower than CPU.
- **Static-shaped partitions only** (`DOCLING_RS_COREML_STATIC_SHAPES=0`
  opts back into dynamic placement): dynamic partitions under MLProgram fail
  an MPSGraph assertion as an uncatchable SIGABRT.
- **Compute units `cpu_and_gpu`** (`DOCLING_RS_COREML_UNITS`:
  `all`|`cpu_and_gpu`|`cpu_and_ne`|`cpu_only`): `all` may schedule the fp16
  Neural Engine, which silently corrupts this model's logits (measured
  max|Δlogits| = 6.5 with no error raised) and ran slower than the GPU path.

| CoreML, heron layout (25 pages, 13 tables, M4 Max) | convert | GPU | output |
|---|---|---|---|
| CPU provider, fp32 | 3.7 s | 0% | reference |
| CoreML, batch 4, MLProgram (old defaults) | 8.4 s | 0% | identical |
| CoreML, batch 1, NeuralNetwork (**new defaults**) | 3.5 s | ~41% | byte-identical |
| CoreML, batch 1, MLProgram (opt-in) | 2.05 s | ~64% | differs on 14/25 pages |

**When CoreML pays off** (#324 follow-up): session creation costs **~2 s
per worker and does not parallelize**, so the fixed setup only amortizes
over long-lived processes (`docling-serve`) and large batches — for a
one-shot CLI conversion the CPU provider is usually as fast or faster.
That, and a gain of only ~6% with the CPU-identical defaults, is why CoreML
is **opt-in** (#602): a `coreml` build leaves it unregistered until
`DOCLING_RS_EP=coreml` (or a named `auto`) asks for it — before, it was
registered whenever `DOCLING_RS_EP` was unset. In Python,
`AcceleratorOptions(device="mps")` maps to `coreml` on a wheel built with
it (`maturin build --features coreml`). `DOCLING_RS_DEBUG=1` notes
the compiled-in-but-unused provider; registration prints the setup-cost
notice once.
The `xnnpack` feature adds the XNNPACK provider
(`DOCLING_RS_EP=xnnpack`, thread pool sized by `DOCLING_RS_XNNPACK_THREADS`)
— a CPU-class accelerator for machines without a usable GPU provider; note
that pyke ships no prebuilt ONNX Runtime with the XNNPACK EP, so this
feature requires linking a self-built ONNX Runtime (`ORT_LIB_LOCATION`,
built with `--use_xnnpack`). When a GPU provider is selected, the pipeline automatically prefers
the fp32 models over the int8 defaults — the int8 exports are calibrated for
CPU kernels (an explicit `DOCLING_*_ONNX` path still wins; the Python
bindings no longer set one themselves, #602, so this holds there too). CUDA needs the
CUDA 12 runtime + cuDNN 9 on the machine; the `ort` crate downloads the
matching ONNX Runtime binaries at build time and copies the provider
libraries next to the binary.

The same features exist on every binding: the Python GPU wheel ships as
[`docling-rs-cuda`](https://pypi.org/project/docling-rs-cuda/) on PyPI, the
Node addon ships as [`docling.rs-cuda`](https://www.npmjs.com/package/docling.rs-cuda)
on npm (a small shim whose postinstall downloads the binaries from a GitHub
release — or build from source with `npm run build:cuda`, see
`crates/docling-node/README.md`), and `docling-serve`/`docling-rag` take
`--features cuda` like the CLI.

Measured (RTX 3080 Laptop vs Ryzen 9 5900HX, cold CLI runs): **1.5–2.1×**
end-to-end on multi-page digital PDFs (`2305.03393v1`: 13.6 s → 7.0 s) and
**8.7×** on a 1913-page reference manual (15 min 13 s → 1 min 45 s) — the
bigger the document, the closer to pure ONNX-stage speedup. Break-even for
a cold run sits around 3–4 pages: 1–2-page and OCR-heavy documents stay
faster on CPU unless you amortize EP init with the warm
`Pipeline`/`docling-serve`.
Output is byte-identical to the CPU run on 21 of 22 corpus fixtures (fp32
GPU kernels aren't bit-exact, one heavy fixture drifts by 2 lines). Details
+ per-file table: [`PDF_CONFORMANCE.md`](./docs/PDF_CONFORMANCE.md#measured-on-real-hardware-issue-108);
reproduce with `scripts/test/gpu_benchmark.sh`.

> **Link fails with `undefined symbol: __isoc23_strtol` (Ubuntu ≤ 22.04,
> Debian ≤ 12)?** The static ONNX Runtime binaries `ort` downloads are built
> against glibc ≥ 2.38 (`__isoc23_*` first appears there). On an older glibc,
> link dynamically against Microsoft's official release instead (built on
> glibc 2.28, so it runs anywhere recent) — same ONNX Runtime version the
> pinned `ort` expects:
>
> ```bash
> curl -fLO https://github.com/microsoft/onnxruntime/releases/download/v1.24.2/onnxruntime-linux-x64-gpu-1.24.2.tgz
> tar xf onnxruntime-linux-x64-gpu-1.24.2.tgz
> export ORT_LIB_LOCATION=$PWD/onnxruntime-linux-x64-gpu-1.24.2/lib
> export ORT_PREFER_DYNAMIC_LINK=1
> cargo build --release -p docling-cli --features cuda
> # dynamic linking: libonnxruntime.so must be findable at runtime, too
> export LD_LIBRARY_PATH=$ORT_LIB_LOCATION:$LD_LIBRARY_PATH
> ```
>
> (`ort-sys` re-runs on these env changes — no `cargo clean` needed.)
>
> The alternative is a newer glibc itself. There is no safe way to upgrade
> *only* glibc on a stable distro — every binary on the system links it, no
> backports exist, and installing a 24.04 `.deb` on 22.04 is the classic way
> to get a machine that no longer boots (`ls` and `apt` need glibc too). The
> real options, honest to hacky:
>
> 1. **Upgrade the distro** — this *is* "upgrading glibc":
>    `sudo do-release-upgrade` (22.04 → 24.04 ships glibc 2.39). The only way
>    to get it system-wide; afterwards the static binaries link as-is.
> 2. **Build in a newer-glibc container** (when the OS must stay put):
>    ```bash
>    docker run --rm -it --gpus all -v $PWD:/w -w /w \
>        nvidia/cuda:12.6.2-cudnn-devel-ubuntu24.04 bash
>    # inside: apt-get update && apt-get install -y curl build-essential
>    #         curl https://sh.rustup.rs -sSf | sh -s -- -y && . ~/.cargo/env
>    #         cargo build --release -p docling-cli --features cuda
>    ```
>    Mind that the produced binary then needs glibc ≥ 2.38 **at runtime
>    too** — run it in the same (or a same-based) image.
> 3. **A parallel glibc under `/opt`** (last resort — works, but every run
>    depends on the rpath below):
>    ```bash
>    curl -fLO https://ftp.gnu.org/gnu/glibc/glibc-2.39.tar.xz && tar xf glibc-2.39.tar.xz
>    mkdir glibc-build && cd glibc-build
>    ../glibc-2.39/configure --prefix=/opt/glibc-2.39 && make -j$(nproc) && sudo make install
>    ```
>    The system glibc is untouched; link the build against the parallel one:
>    ```bash
>    export RUSTFLAGS="-C link-arg=-Wl,--dynamic-linker=/opt/glibc-2.39/lib/ld-linux-x86-64.so.2 \
>                      -C link-arg=-Wl,-rpath,/opt/glibc-2.39/lib"
>    cargo build --release -p docling-cli --features cuda
>    ```
>    The binary resolves glibc from `/opt` and everything else (libstdc++,
>    CUDA) from the system — correct, since glibc is backwards-compatible,
>    but fragile: anything run without that interpreter/rpath fails cryptically.

### IBM Z (s390x) and other targets without a prebuilt ONNX Runtime — `ort-load-dynamic`

docling.rs builds for `s390x-unknown-linux-gnu` (#504). The code is
endian-clean — s390x is big-endian, and the docling-core, HTML-export and
pure-Rust PDF renderer/raster suites pass as s390x binaries under
`qemu-user` — so every declarative format and the PDF text layer work out of
the box. What pyke's `ort` cannot provide there is a prebuilt ONNX Runtime
(it ships binaries for x86_64/aarch64 only; upstream ONNX Runtime builds and
runs on s390x — IBM contributes the port — but publishes no s390x release).
The `ort-load-dynamic` cargo feature (CLI, serve, lib, Python, Node, RAG)
therefore `dlopen`s the runtime at the first ML session instead of linking
it:

```bash
# on the mainframe, or cross-compiled with gcc-s390x-linux-gnu as the release workflow does
cargo build --release -p docling-cli --features ort-load-dynamic --target s390x-unknown-linux-gnu
```

The library is looked up as `ORT_DYLIB_PATH` (a path), then
`.models/onnxruntime/libonnxruntime.so` (through the models-dir resolution,
so `DOCLING_RS_MODELS_DIR` covers it), then the dynamic linker's search path
(`LD_LIBRARY_PATH`, a distro package); ONNX Runtime 1.17 or newer, any
build — a `pip install onnxruntime`'s `capi/libonnxruntime.so.1.x.y` works
on x86_64. Without it the ML stages degrade exactly like a missing model:
PDF conversion fails with a message naming the library and the lookup, OCR
and enrichment warn and skip, DOCX/HTML/XLSX/… and `docling-rs --to md` on a
DOCX never touch it.

**The s390x runtime itself** ships in the models release as
`onnxruntime-linux-s390x.tar.gz`: ONNX Runtime 1.29.0, cross-compiled by
`.github/workflows/onnxruntime-s390x.yml` with zig against glibc 2.28, so it
loads on RHEL 8/9-era mainframe Linux. `download_dependencies.sh` fetches it
into `.models/onnxruntime/` on an s390x host (`--with-onnxruntime` /
`--no-onnxruntime` elsewhere); the release attaches the s390x CLI tarball,
the FFI library, the Python wheel (`manylinux_2_28_s390x`) and the npm
platform package, `install.sh` picks them, and the container images publish
a `linux/s390x` variant. Every s390x artifact has the same glibc 2.28 floor.

Then either:

```bash
cargo run -p docling-cli -- document.pdf
```

or, in a Node.js/Bun app:

```bash
npm i docling.rs
```

```js
import { convertFileAsync } from 'docling.rs'
const { content } = await convertFileAsync('document.pdf', { to: 'markdown' })
console.log(content)
```

The layout model and TableFormer are PyTorch→ONNX exports of docling-project's
own models (Apache-2.0 / CDLA-Permissive-2.0 — see
[`docs/MODELS_NOTICE.md`](./docs/MODELS_NOTICE.md) for full attribution); the
OCR model is re-hosted, unmodified, from its own public release — all on
one host for convenience.

To point at files you exported or placed elsewhere instead, set the env vars
directly: `DOCLING_LAYOUT_ONNX`, `DOCLING_OCR_REC_ONNX`, `DOCLING_OCR_DICT`,
`DOCLING_OCR_DET_ONNX`,
`DOCLING_TABLEFORMER_{ENCODER,DECODER,BBOX}`, `DOCLING_CODE_FORMULA_DIR`
(enrichment models) — an env var always wins over the `./.models` default.
Other process-wide knobs: `DOCLING_RS_RENDERER` (`auto`, the default: the
layout/TableFormer/OCR page images come from the pure-Rust renderer;
`docling-parse` renders them with docling-parse's own Blend2D renderer — the
raster docling 2.123+ feeds its models, #478, the reference the baselines are
pinned to — through the plugin library `download_dependencies.sh
--with-docling-parse` fetches into `.docling-parse/` (or
`scripts/install/build_docling_parse_render.sh` builds), warning once and
falling back to the Rust renderer when it is missing; `rust` is the default
spelled out; `pdfium` renders with the pdfium library — docling's pypdfium2
chain — in a build with the `pdfium` cargo feature, and warns once and uses
the Rust renderer otherwise; `DOCLING_PARSE_RENDER_LIB` /
`DOCLING_PARSE_RESOURCES` point at the plugin explicitly), `DOCLING_RS_FONT_DIRS` (extra font
directories for the Rust renderer's fallback faces — fonts a PDF does not
embed; `.models/fonts` and the usual Liberation/DejaVu/URW/Noto system
directories are scanned by default), `DOCLING_RS_SYSTEM_FONTS` (`0` keeps the
host's `$HOME` and system font directories out of that search, so the render
depends only on `.models/fonts` + `DOCLING_RS_FONT_DIRS`, #633), `DOCLING_RS_SCAN_RASTER` (`rust`, the default: an image-only
page's bitmap comes from the pure-Rust raster, pdfium's bytes exactly;
`pdfium` renders it with the library under the `pdfium` feature), `DOCLING_RS_PDF_THREADS` (total thread budget;
`_WORKERS`/`_INTRA` below split it), `DOCLING_RS_TIMING=1` (per-stage
timings on stderr), `DOCLING_RS_MAX_IMAGE_PIXELS` (image-input decompression
cap), `DOCLING_RS_MAX_HTML_DEPTH`, `DOCLING_RS_MAX_PART_BYTES` (HTML/OOXML
parser limits), `DOCLING_RS_MAX_XML_DEPTH` (element nesting any XML input or
OOXML/ODF part may reach, default 512 — deeper files are rejected before the
XML parser, whose recursion would otherwise overflow the stack),
`DOCLING_RS_SHEET_MAX_CELLS` (the used area of one spreadsheet sheet, default
10 million cells — a sheet with a value in `A1` and one in `XFD1048576` is
skipped instead of materializing 17 billion cells), `DOCLING_RS_IMAGE_FETCH_CONCURRENCY`
(parallel `--fetch-images` downloads), `DOCLING_RS_VLM_EXTRA_BODY` (extra JSON
merged into VLM requests).

Hostile inputs degrade, they do not abort: block quotes and lists in
Markdown nest at most 100 deep (markdown-it's `maxNesting`, what docling's
Markdown backend runs on) and deeper structure is skipped; a docling-JSON
document's `children` references are each walked once and at most 256
levels deep, so a cycle or a fan-out graph converts like the tree it
pretends to be; an HTML page nested past `DOCLING_RS_MAX_HTML_DEPTH` is
recognized by a linear tag scan and emitted as text without ever building a
DOM (html5ever's tree builder is quadratic in nesting — 100 000 nested `<div>`
took 37 s); an ODS sheet's regions are found by visiting its cells, not the
box they span; OOXML parts inflate to at most `DOCLING_RS_MAX_PART_BYTES`;
a redirect on a `--fetch-images` download is checked against the
private-address block-list at every hop; and a PDF page whose declared size
would rasterize past `DOCLING_RS_MAX_RENDER_PIXELS` (15000 px/side, ~5000 pt —
above any real page, A0 at the pipeline's 3x supersample is ~10110 px) is
rejected before the renderer or the `image` crate tries to allocate the
multi-gigabyte bitmap, which a few-hundred-byte crafted `MediaBox` otherwise
forces (the `image` crate *panics* rather than erroring when that allocation
fails).

The OCR engine and language are options on every surface — `ocr_engine`
(`ppocr` | `tesseract`, #460) and `ocr_lang` (`en` by default, `ch` for the
model the conformance baselines were pinned against, BCP-47 tags, tessdata
stems under Tesseract; [`docs/OPTIONS.md`](./docs/OPTIONS.md#ocr-engines-and-languages))
— or process-wide through `DOCLING_RS_OCR_ENGINE` / `DOCLING_RS_OCR_LANG`.
Explicit `DOCLING_OCR_REC_ONNX` + `DOCLING_OCR_DICT` (a pair — set both)
override the language switch entirely; an install without the English model
falls back to `ch` with a warning. The Python bindings hand their cache over
as `DOCLING_RS_MODELS_DIR` and `download_models()` fetches both pairs, so
`ocr_lang=` works on the documented setup path (#285).

## Contributing

Bug reports and pull requests are welcome — see
[CONTRIBUTING.md](./CONTRIBUTING.md) for the build/test commands, the
conformance workflow, and the conventions a change is expected to follow.

## License

MIT, matching upstream docling.
