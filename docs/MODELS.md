# Getting the ML models

What the PDF / image / audio pipelines need on disk, where docling.rs looks
for it, and the knobs around it. Back to the [project README](../README.md).

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
[docs/WINDOWS.md](WINDOWS.md) for the MSVC build walkthrough.

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
from source instead: [`scripts/install/pdf_setup.sh`](../CONTRIBUTING.md#test-suites-in-detail). The
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

## Whisper and Parakeet models for audio/ASR

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

## Enrichment models (picture classification, code, formulas)

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

## Picture OCR for non-PDF documents (`--picture-ocr`)

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
* **Resolution and orientation** (#666). `--picture-ocr-scale X` reads the
  pictures at X px per picture pixel under RapidOCR's 2000 px cap — `1.0`
  is RapidOCR's own native-pixel reading, where the default upscales a
  720 × 180 screenshot to 2000 × 500. `--ocr-textline-orientation` adds
  RapidOCR's line handling: oriented crops, tall lines turned 90°,
  upside-down lines turned by the 0/180 classifier
  (`.models/ocr_textline_ori.onnx`) — vertical axis titles, rotated
  headers and sideways photos read. It applies to scanned pages too.
* **`--no-picture-images`** (`keep_picture_images(false)`) drops the image
  bytes from every picture after the pass: a slim JSON/DCLX and a
  placeholder-only Markdown with the text kept.
* **Degradation.** Under `--no-ocr` / `--text-layer-only`, without the OCR
  model, or in a build without the `pdf` feature, one warning and no text —
  the conversion succeeds.

The same switches exist on every surface: the docling-serve request
options (`do_picture_ocr`, `picture_ocr_classes`, `picture_ocr_min_side`,
`picture_ocr_scale`, `ocr_textline_orientation`, `keep_picture_images` —
query, multipart or JSON body), the Python kwargs (also on
`PdfPipelineOptions`; `picture_ocr_classes` takes a list or the
comma-separated string) and the Node options (`doPictureOcr`,
`pictureOcrClasses`, `pictureOcrMinSide`, `pictureOcrScale`,
`ocrTextlineOrientation`, `keepPictureImages`).
## Picture description through a vision model (`--picture-description`)

The other way to read a picture (#670): send it to an OpenAI-compatible
vision endpoint — a hosted model, or LM Studio / Ollama / vLLM on your own
machine — and attach the answer where `--picture-ocr` puts its text. Every
format and every surface: the embedded pictures of DOCX/PPTX/XLSX/HTML, a
PDF's or scan's figure crops, sampled video frames, and (in caption mode) an
image input as a whole. It is MarkItDown's `llm_client` / `llm_model` /
`llm_prompt` and its markitdown-ocr plugin, with the same two prompts:

```bash
export DOCLING_RS_VLM_ENDPOINT=https://api.openai.com/v1 DOCLING_RS_VLM_API_KEY=sk-… DOCLING_RS_VLM_MODEL=gpt-4o-mini
docling-rs --picture-description deck.pptx                      # captions: "Write a detailed caption for this image."
docling-rs --picture-description --picture-description-mode ocr scan.pdf   # markitdown-ocr's "Extract all text from this image …"
docling-rs --picture-description --picture-description-endpoint http://localhost:11434/v1 \
           --picture-description-model llava --picture-description-prompt "Describe the chart's trend." \
           --picture-description-concurrency 4 report.docx
```

```python
from docling_rs import DocumentConverter, InputFormat, PdfFormatOption, PdfPipelineOptions
from docling_rs.datamodel.pipeline_options import PictureDescriptionApiOptions   # docling's class, same fields

opts = PdfPipelineOptions(do_picture_description=True, picture_description_options=PictureDescriptionApiOptions(
    url="http://localhost:11434/v1", params={"model": "llava"}, prompt="Describe the picture."))
doc = DocumentConverter(format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)}).convert("paper.pdf").document
# or, plainly: DocumentConverter(do_picture_description=True, picture_description_endpoint=…, picture_description_model=…)
```

Endpoint, model and key fall back to the `vlm_*` options and then the
`DOCLING_RS_VLM_*` environment; the prompt does not (`DOCLING_RS_VLM_PROMPT`
asks for a page's markup). `picture_ocr_min_side` and `picture_ocr_classes`
select the pictures here too, identical image bytes are sent once per
document, `concurrency` pictures are in flight at once, and the answer lands
as the picture's description with the model as `created_by`. An endpoint
that rejects a picture warns and the rest go on; one that cannot be reached
warns once and the remaining pictures keep no description — the conversion
never fails over it. With `--picture-ocr` on as well, the local models read
only what the endpoint left undescribed. docling-serve maps docling's
`picture_description_api` request option onto this (a request-supplied
endpoint needs `--allow-url-fetch`, like `vlm_endpoint`).

## PII redaction (`--redact-pii`)

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

## INT8 models (faster PDF conversion on CPU — the default)

The `*_int8` assets are post-training quantizations of the same models:
Conv-only static INT8 of the layout detector (calibrated on this repo's PDF
corpus) and dynamic INT8 of the TableFormer decoder. On CPUs with AVX-512
VNNI they make layout inference — the dominant PDF cost — **~2.4× faster**
(~1.4–1.8× end-to-end) at conformance validated as unchanged against the
corpus groundtruth; the TableFormer output is byte-identical. See
[`docs/PDF_CONFORMANCE.md`](PDF_CONFORMANCE.md) for the measurements.

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

(The [example Dockerfile](../examples/Dockerfile) bakes both precisions and
defaults to INT8; build with `--build-arg INT8=0` for pure fp32.)

## GPU execution providers (optional, off by default)

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
CUDA 13 runtime + cuDNN 9 on the machine (NVIDIA driver ≥ 580); the `ort` crate downloads the
matching ONNX Runtime binaries at build time and links the provider
libraries next to the binary — as symlinks into its download cache, so
install them with `cp -L`, next to the installed binary: ONNX Runtime
opens them from the binary's directory, not through `LD_LIBRARY_PATH`
(#664).

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
+ per-file table: [`PDF_CONFORMANCE.md`](PDF_CONFORMANCE.md#measured-on-real-hardware-issue-108);
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
>        nvidia/cuda:13.0.3-cudnn-devel-ubuntu24.04 bash
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

## Model provenance, overrides and process-wide knobs

The layout model and TableFormer are PyTorch→ONNX exports of docling-project's
own models (Apache-2.0 / CDLA-Permissive-2.0 — see
[`docs/MODELS_NOTICE.md`](MODELS_NOTICE.md) for full attribution); the
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

The OCR engine and language are options on every surface — `ocr_engine`
(`ppocr` | `tesseract`, #460) and `ocr_lang` (`en` by default, `ch` for the
model the conformance baselines were pinned against, BCP-47 tags, tessdata
stems under Tesseract; [`docs/OPTIONS.md`](OPTIONS.md#ocr-engines-and-languages))
— or process-wide through `DOCLING_RS_OCR_ENGINE` / `DOCLING_RS_OCR_LANG`.
Explicit `DOCLING_OCR_REC_ONNX` + `DOCLING_OCR_DICT` (a pair — set both)
override the language switch entirely; an install without the English model
falls back to `ch` with a warning. The Python bindings hand their cache over
as `DOCLING_RS_MODELS_DIR` and `download_models()` fetches both pairs, so
`ocr_lang=` works on the documented setup path (#285).
